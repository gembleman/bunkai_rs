/// Core sentence-boundary disambiguation pipeline.
///
/// Design notes for performance:
///
/// * `SpanRecord` borrows its `split_value` from the input text (`&'a str`)
///   instead of owning a `String`, so cloning/collecting spans is a cheap
///   memcpy rather than a heap allocation per record. Ownership is only
///   materialised into `String`s at the public [`Span`] boundary.
/// * The two regexes of the original port (basic punctuators and forced line
///   breaks) are replaced by hand-written scanners that are *byte-for-byte*
///   faithful to what `regex` matched, including its `\s` == `\p{White_Space}`
///   definition (25 codepoints, verified empirically). This removes the
///   byte→char offset translation both regular expressions required.
/// * The emoji category table is a sorted, unique `&[(u32, Option<&str>)]`
///   that is binary-searched instead of being built into a `HashMap`. Hot-path
///   lookup uses a compact three-state `u8`; the public category table remains
///   unchanged.
/// * `unify_span_annotations` sorts and dedups in place instead of building a
///   `HashMap` keyed by a formatted string.
use std::cell::OnceCell;

use crate::emoji_data;

// --- optional built-in stage-timing profiler (feature `prof`, zero-cost off) ---
#[cfg(feature = "prof")]
mod prof {
    use std::cell::RefCell;
    use std::cmp::Reverse;
    use std::time::{Duration, Instant};

    thread_local! {
        static LOG: RefCell<Vec<(&'static str, u128, Instant)>> = const { RefCell::new(Vec::new()) };
        static TOT: RefCell<Vec<(&'static str, u128)>> = const { RefCell::new(Vec::new()) };
        static CALLS: RefCell<usize> = const { RefCell::new(0) };
    }

    pub fn on() -> bool {
        std::env::var("BUNKAI_PROF").is_ok()
    }

    pub fn begin(tag: &'static str) {
        if on() {
            LOG.with(|p| p.borrow_mut().push((tag, 0, Instant::now())));
        }
    }

    pub fn end() {
        if !on() {
            return;
        }
        LOG.with(|p| {
            let mut p = p.borrow_mut();
            if let Some((_t, acc, t0)) = p.last_mut() {
                *acc += t0.elapsed().as_nanos();
            }
        });
    }

    /// Stack depth at the start of a `segment_impl` call, used to fold only
    /// that call's own frames on completion.
    pub fn enter() -> usize {
        if on() {
            LOG.with(|l| l.borrow().len())
        } else {
            0
        }
    }

    /// Fold the frames at/above `base` (this `segment_impl` call's own) into
    /// the cumulative totals. Outer caller frames stay on the stack.
    fn fold(base: usize) {
        let items: Vec<(&'static str, u128)> = LOG.with(|l| {
            l.borrow_mut()
                .drain(base..)
                .map(|(t, ns, _)| (t, ns))
                .collect()
        });
        TOT.with(|tot| {
            let mut tot = tot.borrow_mut();
            for (tag, ns) in items {
                if let Some(e) = tot.iter_mut().find(|e| e.0 == tag) {
                    e.1 += ns;
                } else {
                    tot.push((tag, ns));
                }
            }
        });
    }

    fn dump() {
        let mut rows: Vec<(&'static str, u128)> = TOT.with(|t| t.borrow().clone());
        rows.sort_by_key(|&(_, n)| Reverse(n));
        let grand: u128 = rows.iter().map(|(_, n)| n).sum();
        eprintln!(
            "--- BUNKAI_PROF call#{} total={:.1?} ---",
            CALLS.with(|c| *c.borrow()),
            Duration::from_nanos(grand as u64)
        );
        for (tag, ns) in rows {
            eprintln!(
                "  {:>34} {:>9.3} ms  {:5.1}%",
                tag,
                ns as f64 / 1e6,
                100.0 * ns as f64 / grand.max(1) as f64
            );
        }
    }

    /// End of one `segment_impl` call: fold its frames; print every 100 calls.
    pub fn on_impl_done(base: usize) {
        fold(base);
        let at = CALLS.with(|c| {
            *c.borrow_mut() += 1;
            *c.borrow() % 100
        });
        if at == 0 {
            dump();
        }
    }

    /// End of the public `segment()` boundary: fold the materialise frame and
    /// report the full (segment-specific) picture.
    pub fn on_segment_done() {
        if on() {
            fold(0);
            dump();
            LOG.with(|l| l.borrow_mut().clear());
        }
    }
}

#[cfg(not(feature = "prof"))]
mod prof {
    #[inline(always)]
    pub fn begin(_: &'static str) {}
    #[inline(always)]
    pub fn end() {}
    #[inline(always)]
    pub fn enter() -> usize {
        0
    }
    #[inline(always)]
    pub fn on_impl_done(_: usize) {}
    #[inline(always)]
    pub fn on_segment_done() {}
}

const TARGET_EMOJI_CATEGORIES: &[&str] = &["Smileys & Emotion", "Symbols"];

// Legacy list of layers that used to be swept by the indirect-quote rule
// (first/BasicRule/LinebreakAnnotator/EmojiAnnotator/EmotionExpressionAnnotator/
// FaceMarkDetector). Superseded by collecting the cumulative final layer directly.
const MORPHEME_RULES: &[&[&str]] = &[
    &["て"],
    &["の"],
    &["と"],
    &["って"],
    &["という"],
    &["に"],
    &["など"],
    &["くらい", "の"],
    &["くらい", "です"],
    &["くらい", "でし"],
    &["も", "あり"],
    &["ほど", "でし"],
];

const FACE_SYMBOL_RANGES: &[(char, char)] = &[
    ('!', '/'),
    (':', '@'),
    ('[', '`'),
    ('{', '~'),
    ('！', '／'),
    ('：', '＠'),
    ('［', '｀'),
    ('｛', '～'),
    ('\u{00A1}', '\u{0FFF}'),
    ('\u{1000}', '\u{2FFF}'),
];

const FACE_SYMBOL_EXTRA: &[char] = &['\u{4E00}', '艸'];

const ASCII_ALNUM_RANGES: &[(char, char)] = &[('0', '9'), ('A', 'Z'), ('a', 'z')];
const FULLWIDTH_ALNUM_RANGES: &[(char, char)] = &[('０', '９'), ('Ａ', 'Ｚ'), ('ａ', 'ｚ')];

const EMOTION_WORDS: &[&str] = &[
    "笑", "泣", "汗", "涙", "怒", "嬉", "爆", "驚", "喜", "悲", "謎", "恥", "焦", "苦", "照", "憂",
    "笑い", "わら", "泣き", "怒り", "照れ",
];
const EMOTION_SYMBOLS: &[char] = &['…', '★', '☆', '♪'];
const EMOTION_PUNCTUATION: &[char] = &['。', '!', '?', '！', '？', '．', '.'];

/// The basic punctuator set `[。!?.！？．]`.
#[inline]
fn is_basic_punct(ch: char) -> bool {
    matches!(ch, '。' | '!' | '?' | '.' | '！' | '？' | '．')
}

/// Unicode White\_Space — exactly what `regex`'s `\s` matched (verified over
/// the whole Unicode range, 25 codepoints).
#[inline]
fn is_ws(ch: char) -> bool {
    // All 25 White_Space codepoints are ≤ U+3000; kana/kanji/etc are above it,
    // so a single compare decides the overwhelmingly-common case.
    if (ch as u32) > 0x3000 {
        return false;
    }
    matches!(
        ch,
        '\u{0009}'..='\u{000D}'
            | '\u{0020}'
            | '\u{0085}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
    )
}

/// Internal pipeline record, kept deliberately compact: a 1-byte rule kind plus
/// two 32-bit offsets and the borrowed split text — 32 bytes per record, half
/// of the old 64 with the two `&'static str` fields. The layer path carries
/// these in cumulative copies, so halving the record halves that memcpy volume.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SpanRecord<'a> {
    kind: Kind,
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) split_value: Option<&'a str>,
}

#[derive(Clone)]
pub(crate) struct LayerOutput<'a> {
    pub(crate) name: &'static str,
    pub(crate) spans: Vec<SpanRecord<'a>>,
}

struct PipelineState<'a> {
    layers: Vec<LayerOutput<'a>>,
    final_index: usize,
}

impl<'a> PipelineState<'a> {
    fn new(char_len: usize) -> Self {
        let sentinel_start = if char_len == 0 { 0 } else { char_len - 1 };
        let sentinel = SpanRecord {
            kind: Kind::Sentinel,
            start: sentinel_start as u32,
            end: char_len as u32,
            split_value: None,
        };
        let layer = LayerOutput {
            name: "first",
            spans: vec![sentinel],
        };
        Self {
            layers: vec![layer],
            final_index: 0,
        }
    }

    fn final_spans(&self) -> &[SpanRecord<'a>] {
        &self.layers[self.final_index].spans
    }

    fn add_layer(&mut self, name: &'static str, spans: Vec<SpanRecord<'a>>) {
        let idx = self.layers.len();
        self.layers.push(LayerOutput { name, spans });
        self.final_index = idx;
    }

    fn add_forward_rule(&mut self, name: &'static str, spans: Vec<SpanRecord<'a>>) {
        // The end-of-text sentinel (layer 0), whose key a one-char rule span at
        // the very last position can collide with — see filter_previous_rule_same_span.
        let (sstart, send) = {
            let s = &self.layers[0].spans[0];
            (s.start as usize, s.end as usize)
        };
        let filtered = filter_previous_rule_same_span(spans, self.final_spans(), (sstart, send));
        self.add_layer(name, filtered);
    }

    fn into_output(self) -> PipelineOutput<'a> {
        let mut bounds: Vec<usize> = Vec::with_capacity(self.layers[self.final_index].spans.len());
        for span in &self.layers[self.final_index].spans {
            bounds.push(span.end as usize);
        }
        bounds.sort_unstable();
        bounds.dedup();
        PipelineOutput {
            layers: self.layers,
            final_boundaries: bounds,
        }
    }
}

pub(crate) struct PipelineOutput<'a> {
    pub(crate) layers: Vec<LayerOutput<'a>>,
    pub(crate) final_boundaries: Vec<usize>,
}

pub(crate) struct TextView<'a> {
    text: &'a str,
    chars: Vec<char>,
    /// char index → byte offset. Built lazily on first `slice` (only the
    /// layer-producing path needs it); the boundaries-only path never touches
    /// it, so we skip the whole allocation for `split_sentences`. Offsets are
    /// stored as `u32` (texts beyond 4 GiB would overflow; asserted in debug).
    char_to_byte: OnceCell<Vec<u32>>,
}

impl<'a> TextView<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        Self {
            text,
            chars,
            char_to_byte: OnceCell::new(),
        }
    }

    fn char_len(&self) -> usize {
        self.chars.len()
    }

    #[inline]
    fn char_at(&self, index: usize) -> Option<char> {
        self.chars.get(index).copied()
    }

    /// Byte offset of a character index, clamped to `text.len()`; materialising
    /// the char→byte map on first use.
    #[inline]
    fn byte_at(&self, index: usize) -> usize {
        let map = self.char_to_byte.get_or_init(|| {
            // Build byte offsets in a single pass over the (cheap) char Vec
            // rather than re-decoding UTF-8 bytes.
            let mut v = Vec::with_capacity(self.chars.len() + 1);
            let mut byte = 0usize;
            for ch in &self.chars {
                debug_assert!(byte <= u32::MAX as usize);
                v.push(byte as u32);
                byte += ch.len_utf8();
            }
            v
        });
        if index >= map.len() {
            self.text.len()
        } else {
            map[index] as usize
        }
    }

    fn slice(&self, start: usize, end: usize) -> &'a str {
        let start_byte = self.byte_at(start);
        let end_byte = self.byte_at(end);
        &self.text[start_byte..end_byte]
    }
}

fn is_in_ranges(ch: char, ranges: &[(char, char)]) -> bool {
    ranges.iter().any(|&(start, end)| start <= ch && ch <= end)
}

fn is_face_symbol_prefix_suffix(ch: char) -> bool {
    if matches!(ch, '(' | ')' | '（' | '）') {
        return false;
    }
    is_in_ranges(ch, FACE_SYMBOL_RANGES) || FACE_SYMBOL_EXTRA.contains(&ch)
}

fn is_face_symbol2(ch: char) -> bool {
    is_face_symbol_prefix_suffix(ch)
}

fn is_ascii_alnum(ch: char) -> bool {
    is_in_ranges(ch, ASCII_ALNUM_RANGES)
}

fn is_fullwidth_alnum(ch: char) -> bool {
    is_in_ranges(ch, FULLWIDTH_ALNUM_RANGES)
}

fn is_face_symbol1(ch: char) -> bool {
    is_ascii_alnum(ch) || is_fullwidth_alnum(ch) || is_face_symbol2(ch)
}

/// Compact hot-path result for [`emoji_lookup`]. `EMOJI_NON_TARGET` includes
/// entries whose public category is `None` (for example skin-tone modifiers):
/// they belong to emoji runs, but do not make a run a target run.
const EMOJI_ABSENT: u8 = 0;
const EMOJI_NON_TARGET: u8 = 1;
const EMOJI_TARGET: u8 = 2;

/// Whether a public emoji category contributes to a target emoji run.
const fn emoji_category_is_target(category: Option<&str>) -> bool {
    let Some(category) = category else {
        return false;
    };
    let mut i = 0;
    while i < TARGET_EMOJI_CATEGORIES.len() {
        if str_eq(category, TARGET_EMOJI_CATEGORIES[i]) {
            return true;
        }
        i += 1;
    }
    false
}

/// Existence and target membership of an emoji codepoint, via binary search
/// over the sorted, unique `emoji_data::EMOJI_DATA`.
///
/// The returned `u8` is one of `EMOJI_ABSENT`, `EMOJI_NON_TARGET`, or
/// `EMOJI_TARGET`. The category strings are intentionally not loaded on this
/// hot path; the public `EMOJI_DATA` table remains available unchanged.
///
/// The table has a huge hole between the low block (`0xA9..0x3299`, 170
/// entries) and the plane-1 block (`0x1F004..`, 1230 entries): the whole
/// CJK/kanji range `0x3300..0x1F000` contains no emoji at all. Because most
/// Japanese prose is kana/kanji, we short-circuit that region before touching
/// the binary search — this is where the bulk of the annotation time went.
/// The region bounds are derived at compile time and asserted consistent.
const fn first_ge(data: &[(u32, Option<&'static str>)], threshold: u32) -> usize {
    let mut i = 0usize;
    while i < data.len() {
        if data[i].0 >= threshold {
            return i;
        }
        i += 1;
    }
    data.len()
}
const EMOJI_LOW_END: usize = first_ge(emoji_data::EMOJI_DATA, 0x3300);
const EMOJI_HIGH_START: usize = first_ge(emoji_data::EMOJI_DATA, 0x1F000);

/// Direct-indexed lookup for the plane-1 emoji block: `EMOJI_HIGH_LOOKUP[cp -
/// 0x1F000]` is the codepoint's compact three-state result, or
/// `EMOJI_ABSENT` if the codepoint isn't in the table. Replaces an ~11-iteration
/// binary search (a dependent-load chain with a ~50%-mispredicted data-dependent
/// branch) with a single address-known load. The table is one byte per codepoint
/// (~2.8 KB), and avoids the former index-plus-category loads.
const EMOJI_HIGH_SPAN: usize =
    (emoji_data::EMOJI_DATA[emoji_data::EMOJI_DATA.len() - 1].0 - 0x1F000 + 1) as usize;
const EMOJI_HIGH_LOOKUP: [u8; EMOJI_HIGH_SPAN] = build_emoji_high_lookup();
const fn build_emoji_high_lookup() -> [u8; EMOJI_HIGH_SPAN] {
    let data = emoji_data::EMOJI_DATA;
    let mut table = [EMOJI_ABSENT; EMOJI_HIGH_SPAN];
    let mut i = EMOJI_HIGH_START;
    while i < data.len() {
        let slot = (data[i].0 - 0x1F000) as usize;
        table[slot] = if emoji_category_is_target(data[i].1) {
            EMOJI_TARGET
        } else {
            EMOJI_NON_TARGET
        };
        i += 1;
    }
    table
}
const _: () = assert!(emoji_data::EMOJI_DATA[0].0 >= 0xA9);

/// `&str` equality usable in a `const` context (`PartialEq` for `str` is not
/// yet const-stable). Used to precompute `EMOJI_TARGET` against the
/// `TARGET_EMOJI_CATEGORIES` strings.
const fn str_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    let mut i = 0;
    while i < ab.len() {
        if ab[i] != bb[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Per-`EMOJI_DATA`-index target flag: 1 iff the emoji's category is one of
/// `TARGET_EMOJI_CATEGORIES`. Precomputed so the low-block lookup does a
/// single `bool` test instead of a `&str` byte comparison (opportunity (5) in
/// `docs/optimization-hardware.md`). `EMOJI_DATA`'s `&str` categories stay
/// untouched — they are still used by the spec model and the low/high lookups.
const EMOJI_TARGET_FLAGS: [u8; emoji_data::EMOJI_DATA.len()] = {
    let mut arr = [0u8; emoji_data::EMOJI_DATA.len()];
    let mut i = 0;
    while i < emoji_data::EMOJI_DATA.len() {
        if let Some(c) = emoji_data::EMOJI_DATA[i].1 {
            let mut k = 0;
            while k < TARGET_EMOJI_CATEGORIES.len() {
                if str_eq(c, TARGET_EMOJI_CATEGORIES[k]) {
                    arr[i] = 1;
                    break;
                }
                k += 1;
            }
        }
        i += 1;
    }
    arr
};

/// A hand-written lower-bound loop instead of `slice::binary_search_by_key`
/// so the whole lookup inlines into the scanning hot loop.
///
#[inline]
fn emoji_lookup(codepoint: u32) -> u8 {
    // EMOJI_DATA's minimum is U+00A9 (asserted above), so all ASCII and the
    // remaining C0/C1 controls can be rejected before touching the table.
    if codepoint < 0xA9 {
        return EMOJI_ABSENT;
    }
    // Fast reject for the dominant emoji-free Japanese ranges: kana (hiragana
    // + katakana) and the katakana phonetic extensions sit below 0x3300, so
    // they would otherwise fall into the ~170-entry low-block binary search on
    // every single character. The ranges hold no emoji (verified against the
    // table: the nearest low-block emoji are 0x3030/0x303D/0x3297/0x3299).
    if (0x3040..=0x30FF).contains(&codepoint) || (0x31F0..=0x31FF).contains(&codepoint) {
        return EMOJI_ABSENT;
    }
    let data = emoji_data::EMOJI_DATA;
    if codepoint < 0x3300 {
        let (mut lo, mut hi) = (0usize, EMOJI_LOW_END);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if data[mid].0 < codepoint {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        return if lo < data.len() && data[lo].0 == codepoint {
            if EMOJI_TARGET_FLAGS[lo] == 1 {
                EMOJI_TARGET
            } else {
                EMOJI_NON_TARGET
            }
        } else {
            EMOJI_ABSENT
        };
    }
    if codepoint < 0x1F000 {
        // The entire CJK/kanji range is emoji-free — this is the hot path
        // for ordinary Japanese prose.
        debug_assert_eq!(EMOJI_LOW_END, EMOJI_HIGH_START);
        return EMOJI_ABSENT;
    }
    // Plane-1 block: direct-indexed, no dependent-load binary search (see
    // `EMOJI_HIGH_LOOKUP`).
    let slot = (codepoint - 0x1F000) as usize;
    if slot >= EMOJI_HIGH_LOOKUP.len() {
        return EMOJI_ABSENT;
    }
    EMOJI_HIGH_LOOKUP[slot]
}
const _: () = assert!(EMOJI_LOW_END == EMOJI_HIGH_START);

/// Result of the single-pass candidate scanner: every span the four
/// annotators (FaceMark/Emotion/Emoji/BasicRule) would find, grouped per rule
/// so the pipeline can merge them in the canonical rule order.
/// Rule kind for the fused scanner — routes a hit to the right sink and carries
/// the per-rule annotation strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Kind {
    /// The end-of-text sentinel (layer-0 "first" record).
    Sentinel,
    Face,
    Emotion,
    Emoji,
    Basic,
    /// A whitespace run containing a newline (linebreak). Collected separately:
    /// it is only ever turned into (start,end) spans by the Linebreak stage.
    Linebreak,
}

impl Kind {
    fn rule_name(self) -> &'static str {
        match self {
            Kind::Sentinel => "first",
            Kind::Face => "FaceMarkDetector",
            Kind::Emotion => "EmotionExpressionAnnotator",
            Kind::Emoji => "EmojiAnnotator",
            Kind::Basic => "BasicRule",
            Kind::Linebreak => "LinebreakForceAnnotator",
        }
    }

    fn split_type(self) -> Option<&'static str> {
        match self {
            Kind::Sentinel => None,
            Kind::Face => Some("facemark"),
            Kind::Linebreak => Some("linebreak"),
            other => Some(other.rule_name()),
        }
    }
}

/// Sink for fused-scanner hits. The layer path materialises full `SpanRecord`s
/// (including each `split_value` slice, via the lazily built char→byte map); the
/// boundaries-only path merges straight into the compact `Bound` list, keeping
/// only the offsets Number/Dot exceptions read plus a `"."`-equals flag — so
/// `split_sentences` never allocates the span records or the byte map.
trait Emit {
    fn emit(&mut self, kind: Kind, start: usize, end: usize);
}

/// Layer-producing sink: grouped per rule, in scan order.
struct SpanCollector<'v, 'a> {
    view: &'v TextView<'a>,
    face: Vec<SpanRecord<'a>>,
    emotion: Vec<SpanRecord<'a>>,
    emoji: Vec<SpanRecord<'a>>,
    basic: Vec<SpanRecord<'a>>,
    /// Newline whitespace runs, in scan order (linebreak spans).
    linebreaks: Vec<(usize, usize)>,
}

impl<'v, 'a> Emit for SpanCollector<'v, 'a> {
    fn emit(&mut self, kind: Kind, start: usize, end: usize) {
        if let Kind::Linebreak = kind {
            self.linebreaks.push((start, end));
            return;
        }
        let record = SpanRecord {
            kind,
            start: start as u32,
            end: end as u32,
            split_value: Some(self.view.slice(start, end)),
        };
        match kind {
            Kind::Face => self.face.push(record),
            Kind::Emotion => self.emotion.push(record),
            Kind::Emoji => self.emoji.push(record),
            Kind::Basic => self.basic.push(record),
            Kind::Linebreak | Kind::Sentinel => unreachable!(),
        }
    }
}

struct ScanAll<'a> {
    face: Vec<SpanRecord<'a>>,
    emotion: Vec<SpanRecord<'a>>,
    emoji: Vec<SpanRecord<'a>>,
    basic: Vec<SpanRecord<'a>>,
    linebreaks: Vec<(usize, usize)>,
}

/// Layer-path entry point: run the fused scanner into grouped `SpanRecord`s.
fn scan_all<'a>(view: &TextView<'a>) -> ScanAll<'a> {
    let mut collector = SpanCollector {
        view,
        face: Vec::new(),
        emotion: Vec::new(),
        emoji: Vec::new(),
        basic: Vec::new(),
        linebreaks: Vec::new(),
    };
    scan_all_with(&view.chars, &mut collector);
    ScanAll {
        face: collector.face,
        emotion: collector.emotion,
        emoji: collector.emoji,
        basic: collector.basic,
        linebreaks: collector.linebreaks,
    }
}

/// Fused scanner: find FaceMark, EmotionExpression (words + symbols), Emoji and
/// BasicRule candidates in ONE pass over the character buffer, instead of
/// re-walking the whole text once per annotator.
///
/// It deliberately does *not* consume matched regions (other than the forward
/// scans inside each per-rule extractor): each position is classified once, a
/// rule only fires at the *start* of its run (`i == 0` or the previous char not
/// in that rule's trigger class), which mirrors exactly what the original
/// consuming finders produced — one span per maximal punct / symbol / emoji /
/// run, face marks and emotion words detected at every '(' / '（'. Because the
/// extractors are the original loops verbatim, per-position output is
/// byte-for-byte identical to the four separate finders (regardless of sink).
fn scan_all_with<E: Emit>(chars: &[char], out: &mut E) {
    let len = chars.len();
    let mut i = 0usize;
    while i < len {
        let ch = chars[i];
        // Single dispatch instead of a chain of `if`s: the trigger classes are
        // disjoint, so a jump table is both faster and keeps per-position
        // output identical (verified by fused_scanner_reproduces_original_finders).
        match ch {
            '(' | '（' => {
                scan_emotion_word(chars, len, i, out);
                scan_face_mark(chars, len, i, out);
            }
            // BasicRule punctuators.
            '。' | '!' | '?' | '.' | '！' | '？' | '．' => {
                if i == 0 || !is_basic_punct(chars[i - 1]) {
                    scan_basic_run(chars, len, i, out);
                }
            }
            // Emotion symbols (a maximal run, plus one trailing punctuator).
            '…' | '★' | '☆' | '♪' => {
                if i == 0 || !EMOTION_SYMBOLS.contains(&chars[i - 1]) {
                    scan_emotion_symbol_run(chars, len, i, out);
                }
            }
            _ => {
                if emoji_lookup(ch as u32) != EMOJI_ABSENT {
                    if i == 0 || emoji_lookup(chars[i - 1] as u32) == EMOJI_ABSENT {
                        scan_emoji_run(chars, len, i, out);
                    }
                } else if is_ws(ch) {
                    // Linebreak detection, folded into the same pass: a
                    // whitespace run is visited at its start only, and recorded
                    // iff it contains a newline (the old separate
                    // `find_linebreaks` pass scanned every position again for
                    // this). Independent of the basic-rule trailing-whitespace
                    // consumption in the original code, matching the two
                    // separate passes exactly.
                    if i == 0 || !is_ws(chars[i - 1]) {
                        let start = i;
                        let mut j = i;
                        let mut has_newline = false;
                        while j < len && is_ws(chars[j]) {
                            if chars[j] == '\n' {
                                has_newline = true;
                            }
                            j += 1;
                        }
                        if has_newline {
                            out.emit(Kind::Linebreak, start, j);
                        }
                    }
                }
            }
        }
        i += 1;
    }
}

/// FaceMarkDetector: a kaomoji `(...)` whose body contains at least one
/// face-symbol character, optionally extended by trailing/leading face-symbol
/// chars (e.g. `o(^_^)o`).
fn scan_face_mark<E: Emit>(chars: &[char], len: usize, i: usize, out: &mut E) {
    let mut cursor = i + 1;
    let mut has_symbol2 = false;
    while cursor < len {
        let current = chars[cursor];
        if !is_face_symbol1(current) {
            break;
        }
        if is_face_symbol2(current) {
            has_symbol2 = true;
        }
        cursor += 1;
    }
    if !has_symbol2 || cursor >= len {
        return;
    }
    let closing = chars[cursor];
    if closing != ')' && closing != '）' {
        return;
    }
    let mut end = cursor + 1;
    while end < len && is_face_symbol_prefix_suffix(chars[end]) {
        end += 1;
    }
    let mut start = i;
    while start > 0 && is_face_symbol_prefix_suffix(chars[start - 1]) {
        start -= 1;
    }

    out.emit(Kind::Face, start, end);
}

/// EmotionExpressionAnnotator — `(word)` form, e.g. `(笑)` / `（泣）`.
fn scan_emotion_word<E: Emit>(chars: &[char], len: usize, i: usize, out: &mut E) {
    for word in EMOTION_WORDS {
        let end_word = i + 1 + word.chars().count();
        if end_word >= len {
            continue;
        }
        if chars_range_eq(chars, i + 1, word) {
            let closing = chars[end_word];
            if closing == ')' || closing == '）' {
                out.emit(Kind::Emotion, i, end_word + 1);
                break;
            }
        }
    }
}

/// EmotionExpressionAnnotator — symbol form: a maximal run of `…★☆♪` plus (if
/// present) exactly one following punctuator.
fn scan_emotion_symbol_run<E: Emit>(chars: &[char], len: usize, i: usize, out: &mut E) {
    let start = i;
    let mut j = i;
    while j < len && EMOTION_SYMBOLS.contains(&chars[j]) {
        j += 1;
    }
    let mut end = j;
    if j < len && EMOTION_PUNCTUATION.contains(&chars[j]) {
        end += 1;
    }
    out.emit(Kind::Emotion, start, end);
}

/// EmojiAnnotator: a maximal run of emoji codepoints that contains at least one
/// char in a target category (`Smileys & Emotion` or `Symbols`).
fn scan_emoji_run<E: Emit>(chars: &[char], len: usize, i: usize, out: &mut E) {
    let start = i;
    let mut j = i;
    let mut any_target = emoji_lookup(chars[j] as u32) == EMOJI_TARGET;
    j += 1;
    while j < len {
        let status = emoji_lookup(chars[j] as u32);
        if status == EMOJI_ABSENT {
            break;
        }
        if status == EMOJI_TARGET {
            any_target = true;
        }
        j += 1;
    }
    if any_target {
        out.emit(Kind::Emoji, start, j);
    }
}

/// BasicRule: the naive punctuators `[。!?.！？．]+` plus any trailing whitespace.
fn scan_basic_run<E: Emit>(chars: &[char], len: usize, i: usize, out: &mut E) {
    let start = i;
    let mut j = i;
    while j < len && is_basic_punct(chars[j]) {
        j += 1;
    }
    while j < len && is_ws(chars[j]) {
        j += 1;
    }
    out.emit(Kind::Basic, start, j);
}

/// `chars[start..start + word.len()] == word` — char-vector level compare, so
/// the scanner never needs the byte map or a per-word allocation.
fn chars_range_eq(chars: &[char], start: usize, word: &str) -> bool {
    for (idx, pc) in (start..).zip(word.chars()) {
        if chars.get(idx) != Some(&pc) {
            return false;
        }
    }
    true
}

pub(crate) fn segment_impl(text: &str) -> PipelineOutput<'_> {
    assert_offset_capacity(text);
    let prof_base = prof::enter();
    let view = TextView::new(text);
    prof::begin("new(TextView)");
    let mut state = PipelineState::new(view.char_len());
    prof::end();

    prof::begin("scan_all");
    let scan = scan_all(&view);
    prof::end();

    prof::begin("FaceMarkDetector");
    let face_spans = scan.face;
    state.add_forward_rule("FaceMarkDetector", face_spans);
    prof::end();

    prof::begin("EmotionExpressionAnnotator");
    // The fused scanner emits word/symbol emotions interleaved by position;
    // the original finder produced them in word-then-symbol order. The rule
    // keeps this exact order in its layer, so reapply the original unification
    // (sort + dedup) to reproduce the byte-identical EmotionExpression layer.
    let emotion_spans = unify_span_annotations(scan.emotion);
    state.add_forward_rule("EmotionExpressionAnnotator", emotion_spans);
    prof::end();

    prof::begin("EmojiAnnotator");
    let emoji_spans = scan.emoji;
    state.add_forward_rule("EmojiAnnotator", emoji_spans);
    prof::end();

    prof::begin("BasicRule");
    let basic_rule_spans = scan.basic;
    state.add_forward_rule("BasicRule", basic_rule_spans);
    prof::end();

    prof::begin("IndirectQuote");
    apply_indirect_quote(&view, &mut state);
    prof::end();
    prof::begin("DotException");
    apply_dot_exception(&view, &mut state);
    prof::end();
    prof::begin("NumberException");
    apply_number_exception(&view, &mut state);
    prof::end();
    prof::begin("LinebreakForce");
    apply_linebreak_force(&view, &mut state, &scan.linebreaks);
    prof::end();

    prof::begin("into_output");
    let mut out = state.into_output();
    attach_sentence_closers_chars(&view.chars, &mut out.final_boundaries);
    prof::end();
    prof::on_impl_done(prof_base);
    out
}

fn apply_indirect_quote<'a>(view: &TextView<'a>, state: &mut PipelineState<'a>) {
    prof::begin("i.unify");
    // Every public layer is a snapshot of the cumulative pipeline at that
    // stage.  Filtering a layer in place would silently rewrite the preceding
    // BasicRule snapshot, so clone the compact borrowed records before the
    // exception stage and append the filtered result as a new layer.
    let mut spans = state.final_spans().to_vec();
    spans.retain(|span| !is_exception_particle(view, span.end as usize));
    debug_assert!(
        spans
            .windows(2)
            .all(|w| w[0].start != w[1].start || w[0].end != w[1].end)
    );
    prof::end();
    state.add_layer("IndirectQuoteExceptionAnnotator", spans);
}

fn is_exception_particle(view: &TextView<'_>, end: usize) -> bool {
    if end >= view.char_len() {
        return false;
    }
    let mut idx = end;
    while view.char_at(idx) == Some('\n') && idx + 1 < view.char_len() {
        idx += 1;
    }
    // Fast reject: a rule can only match if the first character after the
    // boundary starts one of the morpheme rules below.
    let Some(next) = view.char_at(idx) else {
        return false;
    };
    if !matches!(
        next,
        'て' | 'の' | 'と' | 'っ' | 'に' | 'な' | 'く' | 'も' | 'ほ'
    ) {
        return false;
    }
    for rule in MORPHEME_RULES {
        if matches_rule(&view.chars, idx, rule) {
            return true;
        }
    }
    false
}

fn matches_rule(chars: &[char], mut index: usize, rule: &[&str]) -> bool {
    for part in rule {
        if !chars_starts_with(chars, index, part) {
            return false;
        }
        index += part.chars().count();
    }
    true
}

/// Char-vector equivalent of `text[index..].starts_with(pattern)` — avoids
/// touching the (lazily built) char→byte map so the boundaries-only path can
/// check indirect-quote particles without materialising it.
fn chars_starts_with(chars: &[char], mut index: usize, pattern: &str) -> bool {
    for pc in pattern.chars() {
        match chars.get(index) {
            Some(&c) if c == pc => index += 1,
            _ => return false,
        }
    }
    true
}

fn unify_span_annotations(spans: Vec<SpanRecord<'_>>) -> Vec<SpanRecord<'_>> {
    let mut spans = spans;
    spans.sort_by(|a, b| a.start.cmp(&b.start).then(a.end.cmp(&b.end)));
    // `dedup_by` receives the *later* element first; removing it keeps the
    // first occurrence of each run, matching the original map-insert semantics.
    spans.dedup_by(|later, earlier| {
        later.start == earlier.start
            && later.end == earlier.end
            && later.kind == earlier.kind
            && later.split_value == earlier.split_value
    });
    spans
}

fn apply_dot_exception<'a>(view: &TextView<'a>, state: &mut PipelineState<'a>) {
    let mut spans = state.final_spans().to_vec();
    spans.retain(|span| {
        !(is_exception_numeric(view, span.start as usize)
            || is_exception_mailaddress(view, span.start as usize))
    });
    state.add_layer("DotExceptionAnnotator", spans);
}

fn is_exception_numeric(view: &TextView<'_>, index: usize) -> bool {
    if index == 0 {
        return false;
    }
    if index + 1 >= view.char_len() {
        return false;
    }
    match view.char_at(index) {
        Some('.') | Some('．') => {}
        _ => return false,
    }
    let prev_char = view.char_at(index - 1).unwrap_or('\0');
    let next_char = view.char_at(index + 1).unwrap_or('\0');
    is_numeric_char(prev_char) && is_numeric_char(next_char)
}

fn is_numeric_char(ch: char) -> bool {
    matches!(
        ch,
        '0' | '1'
            | '2'
            | '3'
            | '4'
            | '5'
            | '6'
            | '7'
            | '8'
            | '9'
            | '０'
            | '１'
            | '２'
            | '３'
            | '４'
            | '５'
            | '６'
            | '７'
            | '８'
            | '９'
            | '〇'
            | '一'
            | '二'
            | '三'
            | '四'
            | '五'
            | '六'
            | '七'
            | '八'
            | '九'
            | '十'
            | '百'
            | '千'
            | '万'
            | '億'
            | '兆'
            | '京'
    )
}

fn is_exception_mailaddress(view: &TextView<'_>, index: usize) -> bool {
    if index == 0 {
        return false;
    }
    if index + 1 >= view.char_len() {
        return false;
    }
    match view.char_at(index) {
        Some('.') | Some('．') => {}
        _ => return false,
    }
    let prev_char = view.char_at(index - 1).unwrap_or('\0');
    let next_char = view.char_at(index + 1).unwrap_or('\0');
    is_mail_char(prev_char) && is_mail_char(next_char)
}

fn is_mail_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
}

fn apply_number_exception<'a>(view: &TextView<'a>, state: &mut PipelineState<'a>) {
    let mut spans = state.final_spans().to_vec();
    spans.retain(|span| {
        !is_exception_no_parts(
            view,
            span.start as usize,
            span.end as usize,
            is_dot_value(span.split_value),
        )
    });
    state.add_layer("NumberExceptionAnnotator", spans);
}

fn is_exception_no_parts(
    view: &TextView<'_>,
    start: usize,
    end: usize,
    is_dot_value: bool,
) -> bool {
    if !is_dot_value {
        return false;
    }
    if start < 2 {
        return false;
    }
    if end >= view.char_len() {
        return false;
    }
    let n_char = view.char_at(start - 2).unwrap_or('\0');
    let o_char = view.char_at(start - 1).unwrap_or('\0');
    if !matches!(n_char, 'N' | 'n' | 'Ｎ' | 'ｎ') {
        return false;
    }
    if !matches!(o_char, 'O' | 'o' | 'Ｏ' | 'ｏ') {
        return false;
    }
    let next_char = view.char_at(end).unwrap_or('\0');
    next_char.is_ascii_digit()
}

#[inline]
fn is_dot_value(value: Option<&str>) -> bool {
    matches!(value, Some(".") | Some("．"))
}

/// Hand-written equivalent of the old `[\n\s]*\n[\n\s]*` regex: a maximal run
/// of Unicode whitespace that contains at least one newline.
fn apply_linebreak_force<'a>(
    view: &TextView<'a>,
    state: &mut PipelineState<'a>,
    linebreaks: &[(usize, usize)],
) {
    // 1:1 transform: each final span is either kept or replaced by the
    // linebreak run whose start equals its end; unmatched linebreaks are
    // appended after. Work on a copy so every previous snapshot remains
    // observable through the public API.
    let mut spans = state.final_spans().to_vec();
    let mut consumed = vec![false; linebreaks.len()];
    for span in spans.iter_mut() {
        let Ok(i) = linebreaks.binary_search_by_key(&(span.end as usize), |&(start, _)| start)
        else {
            continue;
        };
        if !consumed[i] {
            consumed[i] = true;
            let (lb_start, lb_end) = linebreaks[i];
            *span = SpanRecord {
                kind: Kind::Linebreak,
                start: lb_start as u32,
                end: lb_end as u32,
                split_value: Some(view.slice(lb_start, lb_end)),
            };
        }
    }
    for (i, &(start, end)) in linebreaks.iter().enumerate() {
        if !consumed[i] {
            spans.push(SpanRecord {
                kind: Kind::Linebreak,
                start: start as u32,
                end: end as u32,
                split_value: Some(view.slice(start, end)),
            });
        }
    }
    state.add_layer("LinebreakForceAnnotator", spans);
}

/// Compact span for the boundaries-only fast path used by [`split_sentences`].
/// Only the offsets the downstream rules actually read are kept (the
/// `"."`-flag NumberException checks is derived on the fly instead of being
/// stored). Two 32-bit offsets — 8 bytes total — keep the `active` Vec
/// cache-friendly; 2^32 chars (~12 GiB of Japanese text) is far beyond any
/// realistic input.
#[derive(Clone, Copy, Debug)]
struct Bound {
    start: u32,
    end: u32,
}

/// Internal result that avoids allocating a one-element boundary vector when
/// the scanner finds no effective boundary before the end of the input.
enum ByteBoundaries {
    WholeText,
    Multiple(Vec<usize>),
}

/// Compute the final sentence-boundary offsets as *byte* offsets, without
/// materialising any annotation layer, any `Vec<char>` or the `char→byte` map
/// — the fast path behind [`split_sentences`].
///
/// It runs the exact same rules as [`segment_impl`], but scans the UTF-8 bytes
/// directly (decoding inline, once) and emits compact [`Bound`]s straight out
/// of the scanner; no per-layer records or materialisation. Every offset is on
/// a char boundary (spans are only emitted at char starts), so the caller can
/// slice `text` at each one directly. First-wins-per-(start,end) semantics
/// make the surviving set exactly the final layer's boundaries re-expressed in
/// bytes — verified by the golden/differential suites and the
/// bounds-vs-pipeline property test.
pub fn sentence_byte_boundaries(text: &str) -> Vec<usize> {
    match compute_byte_boundaries(text) {
        ByteBoundaries::WholeText => vec![text.len()],
        ByteBoundaries::Multiple(boundaries) => boundaries,
    }
}

fn compute_byte_boundaries(text: &str) -> ByteBoundaries {
    assert_offset_capacity(text);
    let len = text.len();
    let mut active: Vec<Bound> = Vec::new();
    let mut linebreaks: Vec<(usize, usize)> = Vec::new();
    scan_bounds_bytes(text, &mut active, &mut linebreaks);

    // One pass over the candidates (instead of three `retain`s): particle, dot
    // and number exceptions are all per-span and independent.
    active.retain(|c| {
        let start = c.start as usize;
        let end = c.end as usize;
        if is_exception_particle_bytes(text, end) {
            return false;
        }
        if is_exception_numeric_bytes(text, start) || is_exception_mailaddress_bytes(text, start) {
            return false;
        }
        // A span whose whole matched text is "." / "．" iff it spans exactly
        // that single character — byte-length equivalent of
        // `end == start + 1 && chars[start] ∈ {'.','．'}`.
        let is_dot = byte_bound_is_dot(text, start, end);
        !is_exception_no_parts_bytes(text, start, end, is_dot)
    });

    // The end-of-text sentinel is appended below instead of riding through the
    // pipeline as a member: it can never be rejected by an exception (its end
    // is the text length, its start reads nothing for the dot rules) and never
    // anchors a linebreak (no linebreak starts at text end), so carrying it in
    // `active` only added a dead element to every pass.
    //
    // No newline ⇒ no LinebreakForce replacement to fold in (that path used to
    // build a HashMap and copy `active` even when there was nothing to
    // replace). Note the ends are *not* guaranteed sorted or unique even here:
    // spans from different rules can overlap — e.g. a kaomoji whose trailing
    // suffix extension swallows a following `!?…` run — so the sort+dedup below
    // is still required on both paths.
    if linebreaks.is_empty() {
        if active.is_empty() {
            return ByteBoundaries::WholeText;
        }
        let mut ends: Vec<usize> = Vec::with_capacity(active.len() + 1);
        ends.extend(active.iter().map(|c| c.end as usize));
        ends.push(len);
        attach_sentence_closers_bytes(text, &mut ends);
        ends.sort_unstable();
        ends.dedup();
        return ByteBoundaries::Multiple(ends);
    }

    let active = apply_linebreak_force_bounds(active, &linebreaks);
    // The merge may place a duplicate end next to a leftover linebreak, so the
    // (cheap, rare) sort+dedup is still required on this path.
    let mut ends: Vec<usize> = active.iter().map(|c| c.end as usize).collect();
    ends.push(len);
    attach_sentence_closers_bytes(text, &mut ends);
    ends.sort_unstable();
    ends.dedup();
    ByteBoundaries::Multiple(ends)
}

#[inline]
fn assert_offset_capacity(text: &str) {
    u32::try_from(text.len()).expect("bunkai_rs inputs must not exceed u32::MAX bytes");
}

fn attach_sentence_closers_bytes(text: &str, boundaries: &mut [usize]) {
    for end in boundaries.iter_mut() {
        while *end < text.len() {
            let closer = text[*end..].chars().next().expect("end is in bounds");
            if !is_sentence_closer(closer) {
                break;
            }
            *end += closer.len_utf8();
        }
    }
}

fn attach_sentence_closers_chars(chars: &[char], boundaries: &mut Vec<usize>) {
    for end in boundaries.iter_mut() {
        while *end < chars.len() && is_sentence_closer(chars[*end]) {
            *end += 1;
        }
    }
    boundaries.sort_unstable();
    boundaries.dedup();
}

/// Infallible decoding of the scalar value at byte offset `b` of a valid UTF-8
/// slice, plus its byte length. Callers guarantee `b` is on a char boundary and
/// the char is fully in-bounds (the loop / run guards hold everywhere). No
/// Option, no `char::from_u32` validation branch — the `&str` guarantees a
/// valid scalar, so the unchecked conversion below is sound.
#[inline]
fn char_at_byte(bytes: &[u8], b: usize) -> (char, usize) {
    let first = bytes[b];
    let cp = if first < 0x80 {
        first as u32
    } else if first < 0xE0 {
        ((first & 0x1F) as u32) << 6 | (bytes[b + 1] & 0x3F) as u32
    } else if first < 0xF0 {
        ((first & 0x0F) as u32) << 12
            | ((bytes[b + 1] & 0x3F) as u32) << 6
            | (bytes[b + 2] & 0x3F) as u32
    } else {
        ((first & 0x07) as u32) << 18
            | ((bytes[b + 1] & 0x3F) as u32) << 12
            | ((bytes[b + 2] & 0x3F) as u32) << 6
            | (bytes[b + 3] & 0x3F) as u32
    };
    // `first` gives the length directly; the codepoint is valid by construction
    // (the slice is a `&str`), so the indexing and unchecked conversion are
    // sound. One `unsafe` block keeps the doc/caller invariant in one place.
    // SAFETY: `b` is a char boundary within `bytes`; every multi-byte lead is
    // followed in-bounds by its continuation bytes (slices come from `&str`),
    // and `bytes` decodes from a valid UTF-8 string, so `cp` is a valid scalar.
    let (ch, n) = unsafe {
        let ch = char::from_u32_unchecked(cp);
        let n = if first < 0x80 {
            1
        } else if first < 0xE0 {
            2
        } else if first < 0xF0 {
            3
        } else {
            4
        };
        (ch, n)
    };
    (ch, n)
}

/// Decode the scalar value at byte offset `b`, guarded: returns `None` when
/// `b` is past the end of the slice. Used only by the exception predicates
/// (rare, at span boundaries), never the hot scan loop.
#[inline]
fn char_at_byte_opt(bytes: &[u8], b: usize) -> Option<(char, usize)> {
    if b >= bytes.len() {
        return None;
    }
    let (c, n) = char_at_byte(bytes, b);
    Some((c, n))
}

/// Byte offset of the previous char's leading byte, given a char boundary `b`.
/// Walks back over UTF-8 continuation bytes (at most 3).
#[inline]
fn prev_char_start(bytes: &[u8], b: usize) -> usize {
    debug_assert!(b > 0 && b <= bytes.len());
    let mut p = b - 1;
    while p > 0 && (bytes[p] & 0xC0) == 0x80 {
        p -= 1;
    }
    p
}

/// Sentinel for `prev` after a skipped boring run. Any char outside
/// the trigger/whitespace/emoji classes works: every per-position start-check
/// only tests membership in those classes, so a boring char and this sentinel are
/// indistinguishable to each of them (guarded by the differential suite).
const BORING_SENTINEL: char = 'a';

/// Standard SWAR zero-byte test: true when any byte lane in `word` is zero.
#[inline]
fn has_zero_byte(word: u64) -> bool {
    const LO: u64 = 0x0101_0101_0101_0101;
    const HI: u64 = 0x8080_8080_8080_8080;
    word.wrapping_sub(LO) & !word & HI != 0
}

#[inline]
fn is_boring_ascii(byte: u8) -> bool {
    byte.is_ascii() && byte > b' ' && !matches!(byte, b'!' | b'(' | b'.' | b'?')
}

/// Advance over printable ASCII that cannot trigger any scanner rule.
///
/// Eight bytes are accepted at once when every lane is printable ASCII and no
/// lane equals one of the four ASCII trigger bytes. Treating all controls up to
/// space as stops is deliberately conservative: only ASCII whitespace is
/// semantically interesting, but the broader stop set makes the packed range
/// check cheap and cannot skip a trigger.
fn ascii_skip_run(bytes: &[u8], mut b: usize, len: usize) -> usize {
    const HI: u64 = 0x8080_8080_8080_8080;
    const REP_21: u64 = 0x2121_2121_2121_2121;
    const REP_BANG: u64 = 0x2121_2121_2121_2121;
    const REP_OPEN: u64 = 0x2828_2828_2828_2828;
    const REP_DOT: u64 = 0x2E2E_2E2E_2E2E_2E2E;
    const REP_QUESTION: u64 = 0x3F3F_3F3F_3F3F_3F3F;

    while b + 8 <= len {
        let word = read_u64_unaligned(bytes, b);
        if word & HI != 0 {
            break;
        }
        // Setting every lane's high bit prevents cross-lane borrows. After
        // subtracting 0x21, the high bit remains set iff the byte was >= 0x21.
        let printable = (word | HI).wrapping_sub(REP_21) & HI;
        if printable != HI
            || has_zero_byte(word ^ REP_BANG)
            || has_zero_byte(word ^ REP_OPEN)
            || has_zero_byte(word ^ REP_DOT)
            || has_zero_byte(word ^ REP_QUESTION)
        {
            break;
        }
        b += 8;
    }
    while b < len && is_boring_ascii(bytes[b]) {
        b += 1;
    }
    b
}

/// Byte-pattern predicate: is the UTF-8 scalar starting at `b` a "pure
/// kana/kanji" codepoint that can NEVER be a scanner trigger (basic punct /
/// emotion symbol), whitespace, or emoji — so it can be bulk-skipped without
/// decoding?
///
/// Derivation (UTF-8 byte patterns; see `docs/optimization-hardware.md` for the
/// full write-up). A char is skippable iff its bytes are a lead `0xE4..=0xE9`
/// (⇒ U+4000..=0x9FFF, the CJK Unified Ideographs) or a lead `0xE3` whose 2nd
/// byte is `0x81..=0x83` (⇒ U+3040..=0x30FF, hiragana + katakana).
///
/// The `0xE4..=0xE9` range sits inside the emoji-free hole `0x3300..0x1F000`,
/// every White_Space codepoint is ≤ U+3000, and the only basic punctuators are
/// 。=E3 80 82, ！？．=EF BC xx and ASCII `.!?`; the emotion symbols are
/// U+2026/2605/2606/266A. `EMOJI_DATA` has zero entries in U+4000..=0x9FFF, and
/// the kana range is short-circuited by `emoji_lookup` — so neither range holds
/// a trigger, whitespace or emoji.
///
/// Everything else is excluded because it overlaps a trigger/ws/emoji: `0xE3
/// 0x80` (U+3000..=0x303F) holds 　U+3000 (ws), 。U+3002 (basic punct) and
/// 〰U+3030; `0xE3 0x8A` (U+3290..) holds the emoji ⚗ U+3297 and ⚙ U+3299;
/// `0xEF` (fullwidth forms) holds ！U+FF01, ？U+FF1F, ．U+FF0E; `0xE0..=0xE2`
/// holds ws (U+2000s, U+2028/2029/202F/205F) and emotion symbols (…★☆♪);
/// `0xF0..=0xF4` is plane-1 emoji (e.g. 😀 = F0 9F 98 80); and `0x00..=0x7F` is
/// ASCII `.!?` and whitespace. The skip set is therefore provably disjoint
/// from every trigger/ws/emoji codepoint.
#[inline]
fn cjk_skip_ok(bytes: &[u8], b: usize) -> bool {
    let lead = bytes[b];
    if (0xE4..=0xE9).contains(&lead) {
        return true;
    }
    if lead == 0xE3 {
        // The 2nd byte exists for any lead starting a char inside a valid
        // `&str`; the `get` guard is defensive for a trailing partial char.
        if let Some(&second) = bytes.get(b + 1) {
            return (0x81..=0x83).contains(&second);
        }
    }
    false
}

/// Read 8 little-endian bytes from `bytes[b..b+8]` without assuming
/// alignment. Compiles to a single unaligned load on x86-64; no `unsafe`.
#[inline]
fn read_u64_unaligned(bytes: &[u8], b: usize) -> u64 {
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[b..b + 8]);
    u64::from_le_bytes(buf)
}

/// Advance `b` past a maximal run of pure kana/kanji chars (see `cjk_skip_ok`).
///
/// SWAR-accelerated. In valid UTF-8 a 3-byte lead is always followed by two
/// continuation bytes, so a run of skippable chars is fully described by its
/// *lead* bytes at offsets 0, 3, 6 (the in-between continuation bytes are
/// guaranteed by validity and need no check). We load 8 bytes, then compute in
/// parallel — via the subtract-borrow range trick — a mask of which bytes are
/// skippable leads, and advance by 0/3/6 (always a multiple of 3, so the next
/// window stays lead-aligned). A scalar tail finishes any remaining partial
/// window. No `unsafe` and no target-feature intrinsics (stable-only crate).
fn cjk_skip_run(bytes: &[u8], mut b: usize, len: usize) -> usize {
    const REP_E4: u64 = 0xE4E4E4E4E4E4E4E4;
    const REP_E9: u64 = 0xE9E9E9E9E9E9E9E9;
    const REP_E3: u64 = 0xE3E3E3E3E3E3E3E3;
    const REP_81: u64 = 0x8181818181818181;
    const REP_83: u64 = 0x8383838383838383;
    const TOP: u64 = 0x8080_8080_8080_8080;
    while b + 8 <= len {
        let w = read_u64_unaligned(bytes, b);
        // skippable-lead mask: (byte ∈ [0xE4,0xE9]) OR
        // (byte == 0xE3 AND next byte ∈ [0x81,0x83]).
        let e4e9 = !(w.wrapping_sub(REP_E4) | REP_E9.wrapping_sub(w)) & TOP;
        let e3 = !(w ^ REP_E3) & TOP;
        let next = w >> 8; // byte_{k+1} aligned to position k
        let next_ok = !(next.wrapping_sub(REP_81) | REP_83.wrapping_sub(next)) & TOP;
        let lead = e4e9 | (e3 & next_ok);
        // lead bits at byte offsets 0 / 3 / 6.
        let s0 = (lead & 0x0000_0000_0000_0080) != 0;
        let s3 = (lead & 0x0000_0000_8000_0000) != 0;
        let s6 = (lead & 0x8000_0000_0000_0000) != 0;
        if !s0 {
            break;
        } else if !s3 {
            b += 3;
            break;
        } else if !s6 {
            b += 6;
            break;
        } else {
            b += 6;
        }
    }
    while b + 3 <= len && cjk_skip_ok(bytes, b) {
        b += 3;
    }
    b
}

/// Debug-only guard: every char in `[start, end)` is genuinely boring (not a
/// trigger / whitespace / emoji / face-mark opener). If a future edit ever
/// widens `cjk_skip_ok` to include a dangerous byte pattern, this catches it
/// on the very next test run instead of relying on the corpus happening to
/// exercise the regression. Only ever invoked from a `debug_assert!`, so it is
/// compiled in release but never executed there.
fn cjk_run_is_boring(bytes: &[u8], start: usize, end: usize) -> bool {
    let mut b = start;
    while b < end {
        let (c, n) = char_at_byte(bytes, b);
        let boring = c != '('
            && c != '（'
            && !is_basic_punct(c)
            && !is_ws(c)
            && !EMOTION_SYMBOLS.contains(&c)
            && emoji_lookup(c as u32) == EMOJI_ABSENT;
        if !boring {
            return false;
        }
        b += n;
    }
    true
}

/// Byte-level fused scanner, mirroring [`scan_all_with`] over the char vec body
/// for body. Walks the UTF-8 bytes once, decoding each codepoint inline, and
/// pushes *byte*-offset [`Bound`]s directly — no `Vec<char>` build, no char→byte
/// map, no second decode pass. The dispatch and each run rule are byte-for-byte
/// mirrors of the char scanner, so every emit lands on a char boundary and the
/// surviving set is identical to the pipeline's final layer re-expressed in
/// bytes (guarded by the bounds-vs-pipeline property test and the golden suite).
fn scan_bounds_bytes(text: &str, active: &mut Vec<Bound>, linebreaks: &mut Vec<(usize, usize)>) {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut b = 0usize;
    let mut prev: Option<char> = None;
    // Caches `prev.is_some_and(|p| emoji_lookup(p as u32) != EMOJI_ABSENT)` so the
    // dispatch doesn't re-run `emoji_lookup` on `prev` every iteration: none
    // of the punctuation/kaomoji trigger chars matched by the other arms are
    // in `EMOJI_DATA`, so `is_emoji` is exactly that predicate for every `ch`.
    let mut prev_is_emoji = false;
    while b < len {
        // Bulk-skip printable ASCII and pure kana/kanji runs that cannot be a
        // trigger, whitespace or emoji. Dispatching on the first byte avoids
        // paying the CJK SWAR setup on ASCII and vice versa.
        let first = bytes[b];
        let run_end = if is_boring_ascii(first) {
            ascii_skip_run(bytes, b, len)
        } else if first.is_ascii() {
            b
        } else {
            cjk_skip_run(bytes, b, len)
        };
        if run_end != b {
            debug_assert!(cjk_run_is_boring(bytes, b, run_end));
            // All skipped chars are non-trigger / non-ws / non-emoji, so the
            // sentinel below reproduces the per-position "prev" start-checks
            // exactly (any real boring char would pass the same checks).
            prev = Some(BORING_SENTINEL);
            prev_is_emoji = false;
            b = run_end;
            continue;
        }

        let (ch, n) = char_at_byte(bytes, b);
        let mut is_emoji = false;
        match ch {
            '(' | '（' => {
                scan_emotion_word_bytes(bytes, len, b, n, active);
                scan_face_mark_bytes(bytes, len, b, n, active);
            }
            '。' | '!' | '?' | '.' | '！' | '？' | '．' => {
                if !prev.is_some_and(is_basic_punct) {
                    scan_basic_run_bytes(bytes, len, b, active);
                }
            }
            '…' | '★' | '☆' | '♪' => {
                if !prev.is_some_and(|p| EMOTION_SYMBOLS.contains(&p)) {
                    scan_emotion_symbol_run_bytes(bytes, len, b, active);
                }
            }
            _ => {
                is_emoji = emoji_lookup(ch as u32) != EMOJI_ABSENT;
                if is_emoji {
                    if !prev_is_emoji {
                        scan_emoji_run_bytes(bytes, len, b, active);
                    }
                } else if is_ws(ch) {
                    // Linebreak detection, folded into the same pass: a
                    // whitespace run is visited at its start only.
                    if !prev.is_some_and(is_ws) {
                        let start = b;
                        let mut j = b;
                        let mut has_newline = false;
                        while j < len {
                            let (wc, wn) = char_at_byte(bytes, j);
                            if !is_ws(wc) {
                                break;
                            }
                            if wc == '\n' {
                                has_newline = true;
                            }
                            j += wn;
                        }
                        if has_newline {
                            linebreaks.push((start, j));
                        }
                    }
                }
            }
        }
        prev = Some(ch);
        prev_is_emoji = is_emoji;
        b += n;
    }
}

/// `[。!?.！？．]+` plus trailing whitespace.
fn scan_basic_run_bytes(bytes: &[u8], len: usize, start: usize, active: &mut Vec<Bound>) {
    let mut j = start;
    while j < len {
        let (c, n) = char_at_byte(bytes, j);
        if !is_basic_punct(c) {
            break;
        }
        j += n;
    }
    while j < len {
        let (c, n) = char_at_byte(bytes, j);
        if !is_ws(c) {
            break;
        }
        j += n;
    }
    active.push(Bound {
        start: start as u32,
        end: j as u32,
    });
}

/// A maximal run of `…★☆♪` plus (if present) exactly one following punctuator.
fn scan_emotion_symbol_run_bytes(bytes: &[u8], len: usize, start: usize, active: &mut Vec<Bound>) {
    let mut j = start;
    while j < len {
        let (c, n) = char_at_byte(bytes, j);
        if !EMOTION_SYMBOLS.contains(&c) {
            break;
        }
        j += n;
    }
    let mut end = j;
    if j < len {
        let (c, n) = char_at_byte(bytes, j);
        if EMOTION_PUNCTUATION.contains(&c) {
            end = j + n;
        }
    }
    active.push(Bound {
        start: start as u32,
        end: end as u32,
    });
}

/// A maximal run of emoji codepoints containing a target-category char.
fn scan_emoji_run_bytes(bytes: &[u8], len: usize, start: usize, active: &mut Vec<Bound>) {
    let mut j = start;
    let mut any_target = false;
    while j < len {
        let (c, n) = char_at_byte(bytes, j);
        let status = emoji_lookup(c as u32);
        if status == EMOJI_ABSENT {
            break;
        }
        if status == EMOJI_TARGET {
            any_target = true;
        }
        j += n;
    }
    if any_target {
        active.push(Bound {
            start: start as u32,
            end: j as u32,
        });
    }
}

/// `(word)` emotion expression, e.g. `(笑)` / `（泣）`.
fn scan_emotion_word_bytes(
    bytes: &[u8],
    len: usize,
    start: usize,
    trig_len: usize,
    active: &mut Vec<Bound>,
) {
    for word in EMOTION_WORDS {
        let wbytes = word.as_bytes();
        let after = start + trig_len + wbytes.len();
        if after >= len {
            continue;
        }
        if bytes[start + trig_len..].starts_with(wbytes) {
            let (closing, n) = char_at_byte(bytes, after);
            if closing == ')' || closing == '）' {
                active.push(Bound {
                    start: start as u32,
                    end: (after + n) as u32,
                });
                break;
            }
        }
    }
}

/// Kaomoji `(...)` with a face-symbol body, extended by face-symbol chars.
fn scan_face_mark_bytes(
    bytes: &[u8],
    len: usize,
    start: usize,
    trig_len: usize,
    active: &mut Vec<Bound>,
) {
    let mut cur = start + trig_len;
    let mut has_symbol2 = false;
    while cur < len {
        let (c, n) = char_at_byte(bytes, cur);
        if !is_face_symbol1(c) {
            break;
        }
        if is_face_symbol2(c) {
            has_symbol2 = true;
        }
        cur += n;
    }
    if !has_symbol2 || cur >= len {
        return;
    }
    let (closing, n) = char_at_byte(bytes, cur);
    if closing != ')' && closing != '）' {
        return;
    }
    let mut end = cur + n;
    while end < len {
        let (c, n) = char_at_byte(bytes, end);
        if !is_face_symbol_prefix_suffix(c) {
            break;
        }
        end += n;
    }
    let mut st = start;
    while st > 0 {
        let p = prev_char_start(bytes, st);
        let (c, _) = char_at_byte(bytes, p);
        if !is_face_symbol_prefix_suffix(c) {
            break;
        }
        st = p;
    }
    active.push(Bound {
        start: st as u32,
        end: end as u32,
    });
}

/// Byte-level equivalents of the exception predicates; see the `TextView`
/// versions for the reference semantics. Reads are anchored on byte offsets
/// (span boundaries are char-aligned), so "previous char" means stepping back
/// to the previous lead byte.
fn is_exception_particle_bytes(text: &str, end: usize) -> bool {
    let bytes = text.as_bytes();
    let len = bytes.len();
    if end >= len {
        return false;
    }
    let mut idx = end;
    while idx < len && char_at_byte_opt(bytes, idx).is_some_and(|(c, _)| c == '\n') && idx + 1 < len
    {
        idx += 1;
    }
    let Some((next, _)) = char_at_byte_opt(bytes, idx) else {
        return false;
    };
    if !matches!(
        next,
        'て' | 'の' | 'と' | 'っ' | 'に' | 'な' | 'く' | 'も' | 'ほ'
    ) {
        return false;
    }
    for rule in MORPHEME_RULES {
        if matches_rule_bytes(bytes, idx, rule) {
            return true;
        }
    }
    false
}

fn matches_rule_bytes(bytes: &[u8], mut index: usize, rule: &[&str]) -> bool {
    for part in rule {
        if !bytes[index..].starts_with(part.as_bytes()) {
            return false;
        }
        index += part.len();
    }
    true
}

fn is_exception_numeric_bytes(text: &str, start: usize) -> bool {
    let bytes = text.as_bytes();
    if start == 0 {
        return false;
    }
    let Some((dot, dot_len)) = char_at_byte_opt(bytes, start) else {
        return false;
    };
    if dot != '.' && dot != '．' {
        return false;
    }
    let after = start + dot_len;
    if after >= bytes.len() {
        return false;
    }
    let (prev, _) = char_at_byte(bytes, prev_char_start(bytes, start));
    let (next, _) = char_at_byte(bytes, after);
    is_numeric_char(prev) && is_numeric_char(next)
}

fn is_exception_mailaddress_bytes(text: &str, start: usize) -> bool {
    let bytes = text.as_bytes();
    if start == 0 {
        return false;
    }
    let Some((dot, dot_len)) = char_at_byte_opt(bytes, start) else {
        return false;
    };
    if dot != '.' && dot != '．' {
        return false;
    }
    let after = start + dot_len;
    if after >= bytes.len() {
        return false;
    }
    let (prev, _) = char_at_byte(bytes, prev_char_start(bytes, start));
    let (next, _) = char_at_byte(bytes, after);
    is_mail_char(prev) && is_mail_char(next)
}

/// `end == start + 1 && chars[start] ∈ {'.','．'}` in byte terms: the span
/// covers exactly that single character.
#[inline]
fn byte_bound_is_dot(text: &str, start: usize, end: usize) -> bool {
    let bytes = text.as_bytes();
    let Some((c, n)) = char_at_byte_opt(bytes, start) else {
        return false;
    };
    (c == '.' || c == '．') && end == start + n
}

fn is_exception_no_parts_bytes(text: &str, start: usize, end: usize, is_dot: bool) -> bool {
    let bytes = text.as_bytes();
    if !is_dot {
        return false;
    }
    if start == 0 {
        return false;
    }
    let o = prev_char_start(bytes, start);
    if o == 0 {
        return false; // fewer than two chars before the span start
    }
    if end >= bytes.len() {
        return false;
    }
    let n = prev_char_start(bytes, o);
    let (n_char, _) = char_at_byte(bytes, n);
    let (o_char, _) = char_at_byte(bytes, o);
    if !matches!(n_char, 'N' | 'n' | 'Ｎ' | 'ｎ') {
        return false;
    }
    if !matches!(o_char, 'O' | 'o' | 'Ｏ' | 'ｏ') {
        return false;
    }
    let (next_char, _) = char_at_byte(bytes, end);
    next_char.is_ascii_digit()
}

fn apply_linebreak_force_bounds(active: Vec<Bound>, linebreaks: &[(usize, usize)]) -> Vec<Bound> {
    // A HashMap is overkill here: `linebreaks` already arrives from the scanner
    // sorted by ascending start with unique starts, so each bound's end is
    // looked up against the linebreak starts with a binary search — no hashing
    // and, apart from one tiny `consumed` byte array of length L, no heap
    // structure. This path runs only when the text actually contains a newline.
    //
    // The `consumed` flags preserve first-wins semantics: spans from different
    // rules can *overlap* (a kaomoji's trailing suffix swallows a following
    // `!?…` run) and two bounds may therefore share the same end, but each
    // linebreak must anchor (replace) at most one of them. Exactly which bound
    // is replaced doesn't change the resulting boundary *set* (the survivor
    // keeps the shared end either way), so a consumed marker per linebreak —
    // instead of per candidate — is exact.
    let mut consumed = vec![false; linebreaks.len()];
    let mut result: Vec<Bound> = Vec::with_capacity(active.len() + linebreaks.len());
    for c in active {
        let end = c.end as usize;
        let replaced = match linebreaks.binary_search_by_key(&end, |&(s, _)| s) {
            Ok(i) if !consumed[i] => {
                consumed[i] = true;
                // Linebreak spans' matched text is never "." / "．".
                result.push(Bound {
                    start: linebreaks[i].0 as u32,
                    end: linebreaks[i].1 as u32,
                });
                true
            }
            _ => false,
        };
        if !replaced {
            result.push(c);
        }
    }
    for (i, &(s, e)) in linebreaks.iter().enumerate() {
        if !consumed[i] {
            result.push(Bound {
                start: s as u32,
                end: e as u32,
            });
        }
    }
    result
}

/// Merge a fresh rule's spans into the cumulative layer, keeping one record per
/// (start,end) and the earlier rule winning.
///
/// No general membership structure is needed: the four annotators' trigger
/// characters (`(`/`（`, the basic punctuators, `…★☆♪` and the emoji block) are
/// disjoint, so two different rules can never produce the same (start,end). The
/// only key a fresh span can collide with is the end-of-text sentinel — a
/// one-char rule span at the very last position occupies `(len-1, len)` — so
/// first-wins is exactly "skip spans occupying the sentinel's key". Fresh
/// spans from one rule are also internally unique, so the layer stays
/// byte-identical to the old full-sweep set-based version (guarded by the
/// golden/differential suites and the fast-bunkai cross-check).
fn filter_previous_rule_same_span<'a>(
    current: Vec<SpanRecord<'a>>,
    previous: &[SpanRecord<'a>],
    sentinel_key: (usize, usize),
) -> Vec<SpanRecord<'a>> {
    // Both inputs are sorted by `(start, end)`:
    //  • `current` is one rule's hits in scan order, which is start order;
    //  • `previous` is the cumulative layer, kept sorted by this same merge.
    // Merging instead of concatenating keeps the cumulative layer sorted, so
    // `apply_indirect_quote`'s later sort sees already-sorted data (pdqsort
    // then runs in ~O(n)). The sentinel (key == `sentinel_key`) lives only in
    // `previous`, sorts to the tail (largest position), and survives exactly
    // once — matching the old `current-filtered ++ previous` shape.
    let mut out: Vec<SpanRecord<'a>> = Vec::with_capacity(current.len() + previous.len());
    let mut ci = current.into_iter().peekable();
    let mut pi = 0usize;
    while pi < previous.len() || ci.peek().is_some() {
        match (ci.peek(), previous.get(pi)) {
            (Some(c), Some(p)) => {
                if (c.start, c.end) <= (p.start, p.end) {
                    let key = (c.start as usize, c.end as usize);
                    let c = ci.next().expect("peeked current span");
                    if key != sentinel_key {
                        out.push(c);
                    }
                } else {
                    out.push(p.clone());
                    pi += 1;
                }
            }
            (Some(c), None) => {
                let key = (c.start as usize, c.end as usize);
                let c = ci.next().expect("peeked current span");
                if key != sentinel_key {
                    out.push(c);
                }
            }
            (None, Some(p)) => {
                out.push(p.clone());
                pi += 1;
            }
            (None, None) => break,
        }
    }
    out
}

/// A single annotated span produced by one of the pipeline rules.
///
/// All string fields are borrowed and cost no per-span allocation:
/// `rule_name`/`split_type` are `'static` (rule names are fixed refs), and
/// `split_value`, when present, is a slice into the input text.
///
/// Offsets are measured in Unicode scalar values (`char` indices) over the
/// input string, not byte offsets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span<'a> {
    pub rule_name: &'static str,
    pub start: usize,
    pub end: usize,
    pub split_type: Option<&'static str>,
    pub split_value: Option<&'a str>,
}

/// One stage in the segmentation pipeline, named after the rule that produced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer<'a> {
    pub name: &'static str,
    pub spans: Vec<Span<'a>>,
}

/// The full result of segmenting a piece of text.
///
/// `layers` records every intermediate pipeline stage; `final_boundaries`
/// gives the unique character offsets at which the final layer would cut the
/// input into sentences. This value borrows from the input text, which must
/// therefore outlive it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segmentation<'a> {
    pub layers: Vec<Layer<'a>>,
    pub final_boundaries: Vec<usize>,
}

impl<'a> From<SpanRecord<'a>> for Span<'a> {
    fn from(record: SpanRecord<'a>) -> Self {
        Span {
            rule_name: record.kind.rule_name(),
            start: record.start as usize,
            end: record.end as usize,
            split_type: record.kind.split_type(),
            split_value: record.split_value,
        }
    }
}

impl<'a> From<LayerOutput<'a>> for Layer<'a> {
    fn from(layer: LayerOutput<'a>) -> Self {
        Layer {
            name: layer.name,
            spans: layer.spans.into_iter().map(Span::from).collect(),
        }
    }
}

impl<'a> From<PipelineOutput<'a>> for Segmentation<'a> {
    fn from(output: PipelineOutput<'a>) -> Self {
        Segmentation {
            layers: output.layers.into_iter().map(Layer::from).collect(),
            final_boundaries: output.final_boundaries,
        }
    }
}

/// Segment Japanese text into sentence-like units.
///
/// Returns the full [`Segmentation`] result, including every intermediate
/// pipeline layer and the final list of character-offset boundaries. The
/// returned value borrows from `text` (each `split_value` is a slice of it),
/// so `text` must outlive the result — no per-span heap allocation occurs.
///
/// ```
/// use bunkai_rs::segment;
///
/// let text = "これは一文目。これは二文目。";
/// let seg = segment(text);
/// assert_eq!(seg.final_boundaries, vec![7, 14]);
/// assert_eq!(seg.layers.last().unwrap().name, "LinebreakForceAnnotator");
/// ```
pub fn segment(text: &str) -> Segmentation<'_> {
    // Measure only the `PipelineOutput → Segmentation` conversion here. The
    // `segment_impl` stages are timed inside `segment_impl` itself and folded
    // there, so wrapping them in this frame would double-count the whole
    // pipeline under the "materialize" tag.
    let output = segment_impl(text);
    prof::begin("materialize");
    let seg: Segmentation<'_> = output.into();
    prof::end();
    prof::on_segment_done();
    seg
}

/// Split `text` into its sentences, returning the substrings themselves.
///
/// This is a convenience wrapper over [`segment`]: it slices `text` at each of
/// the pipeline's final boundaries. Offsets in [`Segmentation`] are measured
/// in Unicode scalar values, so callers who only need the sentences can use
/// this instead of converting boundaries back to byte offsets themselves.
///
/// ```
/// use bunkai_rs::split_sentences;
///
/// let sentences = split_sentences("これは一文目。これは二文目。");
/// assert_eq!(sentences, vec!["これは一文目。", "これは二文目。"]);
/// ```
/// Run only the boundary computation, never materialising any per-layer span
/// records. This is the fast path used by [`split_sentences`]: it forgoes the
/// cumulative layer bookkeeping, the IndirectQuote sort and the public
/// `Span`/`Layer` structs, which is where most of the cost of [`segment`] lives.
pub fn split_sentences(text: &str) -> Vec<&str> {
    split_sentences_impl(text)
}

fn split_sentences_impl(text: &str) -> Vec<&str> {
    // Avoid allocating a boundary vector for the overwhelmingly common case
    // where the entire non-empty input is one sentence.
    let boundaries = match compute_byte_boundaries(text) {
        ByteBoundaries::WholeText => {
            return if text.is_empty() {
                Vec::new()
            } else {
                vec![text]
            };
        }
        ByteBoundaries::Multiple(boundaries) => boundaries,
    };

    let mut out = Vec::with_capacity(boundaries.len() + 1);
    let mut start = 0usize;
    for &end in &boundaries {
        if end > start {
            out.push(&text[start..end]);
            start = end;
        }
    }
    // A final boundary equal to the byte length leaves the last sentence here.
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// Closing punctuation that belongs to a sentence-ending quote/bracket rather
/// than forming a useful sentence on its own. Both Japanese/full-width and
/// common ASCII/typographic forms are included; consecutive closers are folded
/// together (`。』）」`).
#[inline]
fn is_sentence_closer(c: char) -> bool {
    matches!(
        c,
        ')' | ']'
            | '}'
            | '）'
            | '］'
            | '｝'
            | '〉'
            | '》'
            | '」'
            | '』'
            | '】'
            | '〕'
            | '〗'
            | '〙'
            | '〟'
            | '’'
            | '”'
            | '｠'
    )
}
