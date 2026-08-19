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
///   that is binary-searched instead of being built into a `HashMap`.
/// * `unify_span_annotations` sorts and dedups in place instead of building a
///   `HashMap` keyed by a formatted string.
use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};

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

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SpanRecord<'a> {
    pub(crate) rule_name: &'static str,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) split_type: Option<&'static str>,
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
            rule_name: "first",
            start: sentinel_start,
            end: char_len,
            split_type: None,
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
        let filtered = filter_previous_rule_same_span(spans, self.final_spans());
        self.add_layer(name, filtered);
    }

    fn into_output(self) -> PipelineOutput<'a> {
        let mut bounds: Vec<usize> = Vec::with_capacity(self.layers[self.final_index].spans.len());
        for span in &self.layers[self.final_index].spans {
            bounds.push(span.end);
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
    /// it, so we skip the whole `Vec<usize>` allocation for `split_sentences`.
    char_to_byte: OnceCell<Vec<usize>>,
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
                v.push(byte);
                byte += ch.len_utf8();
            }
            v
        });
        if index >= map.len() {
            self.text.len()
        } else {
            map[index]
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

/// Existence + category of an emoji codepoint, via binary search over the
/// sorted, unique `emoji_data::EMOJI_DATA`.
///
/// Returns `Some(entry)` when the codepoint IS present in the emoji table
/// (`entry` is the stored category, which may itself be `None` for e.g.
/// skin-tone modifiers), and `None` when the codepoint is not an emoji at
/// all. The original port keyed membership by table presence — a codepoint
/// with a `None` category still belongs to emoji *runs* — so callers must
/// distinguish those two cases.
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

/// A hand-written lower-bound loop instead of `slice::binary_search_by_key`
/// so the whole lookup inlines into the scanning hot loop.
#[inline]
fn emoji_lookup(codepoint: u32) -> Option<&'static Option<&'static str>> {
    let data = emoji_data::EMOJI_DATA;
    let (mut lo, mut hi) = if codepoint < 0x3300 {
        (0usize, EMOJI_LOW_END)
    } else if codepoint < 0x1F000 {
        // The entire CJK/kanji range is emoji-free — this is the hot path
        // for ordinary Japanese prose.
        debug_assert_eq!(EMOJI_LOW_END, EMOJI_HIGH_START);
        return None;
    } else {
        (EMOJI_HIGH_START, data.len())
    };
    while lo < hi {
        let mid = (lo + hi) / 2;
        if data[mid].0 < codepoint {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo < data.len() && data[lo].0 == codepoint {
        Some(&data[lo].1)
    } else {
        None
    }
}
const _: () = assert!(EMOJI_LOW_END == EMOJI_HIGH_START);

/// Result of the single-pass candidate scanner: every span the four
/// annotators (FaceMark/Emotion/Emoji/BasicRule) would find, grouped per rule
/// so the pipeline can merge them in the canonical rule order.
/// Rule kind for the fused scanner — routes a hit to the right sink and carries
/// the per-rule annotation strings.
#[derive(Clone, Copy)]
enum Kind {
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
            Kind::Face => "FaceMarkDetector",
            Kind::Emotion => "EmotionExpressionAnnotator",
            Kind::Emoji => "EmojiAnnotator",
            Kind::Basic => "BasicRule",
            Kind::Linebreak => "LinebreakForceAnnotator",
        }
    }

    fn split_type(self) -> &'static str {
        match self {
            Kind::Face => "facemark",
            other => other.rule_name(),
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
            rule_name: kind.rule_name(),
            start,
            end,
            split_type: Some(kind.split_type()),
            split_value: Some(self.view.slice(start, end)),
        };
        match kind {
            Kind::Face => self.face.push(record),
            Kind::Emotion => self.emotion.push(record),
            Kind::Emoji => self.emoji.push(record),
            Kind::Basic => self.basic.push(record),
            Kind::Linebreak => unreachable!(),
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
        if ch == '(' || ch == '（' {
            scan_emotion_word(chars, len, i, out);
            scan_face_mark(chars, len, i, out);
            i += 1;
            continue;
        }
        if is_basic_punct(ch) {
            if i == 0 || !is_basic_punct(chars[i - 1]) {
                scan_basic_run(chars, len, i, out);
            }
            i += 1;
            continue;
        }
        if EMOTION_SYMBOLS.contains(&ch) {
            if i == 0 || !EMOTION_SYMBOLS.contains(&chars[i - 1]) {
                scan_emotion_symbol_run(chars, len, i, out);
            }
            i += 1;
            continue;
        }
        if emoji_lookup(ch as u32).is_some() {
            if i == 0 || emoji_lookup(chars[i - 1] as u32).is_none() {
                scan_emoji_run(chars, len, i, out);
            }
            i += 1;
            continue;
        }
        if is_ws(ch) {
            // Linebreak detection, folded into the same pass: a whitespace run
            // is visited at its start only, and recorded iff it contains a
            // newline (the old separate `find_linebreaks` pass scanned every
            // position again for this). This is independent of the basic-rule
            // trailing-whitespace consumption in the original code, matching
            // the two separate passes exactly.
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
            i += 1;
            continue;
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
    let mut any_target = emoji_lookup(chars[j] as u32)
        .copied()
        .flatten()
        .is_some_and(|cat| TARGET_EMOJI_CATEGORIES.contains(&cat));
    j += 1;
    while j < len {
        let Some(category) = emoji_lookup(chars[j] as u32).copied() else {
            break;
        };
        if category.is_some_and(|cat| TARGET_EMOJI_CATEGORIES.contains(&cat)) {
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
    let out = state.into_output();
    prof::end();
    prof::on_impl_done(prof_base);
    out
}

fn apply_indirect_quote<'a>(view: &TextView<'a>, state: &mut PipelineState<'a>) {
    prof::begin("i.unify");
    // `final_spans()` (the BasicRule layer) already holds the *unique* union
    // of every earlier annotator layer's spans (FaceMark + Emotion + Emoji +
    // BasicRule) and the end-of-text sentinel, because each `add_forward_rule`
    // re-includes the previous layer's spans. The old `INDIRECT_RULE_TARGETS`
    // sweep only re-collected those same records a few times over (every
    // target layer is cumulative), then relied on `unify_span_annotations` to
    // sort+dedup the redundancy away. Build the unique set directly.
    let mut spans: Vec<SpanRecord<'_>> = Vec::with_capacity(state.final_spans().len());
    for span in state.final_spans() {
        if is_exception_particle(view, span.end) {
            continue;
        }
        spans.push(span.clone());
    }
    // (start,end) is unique in the source layer (`filter_previous_rule_same_span`
    // keeps at most one record per key) and the exception filter only removes
    // records, so there are no equal sort keys: an unstable sort cannot differ
    // from the previous stable one, and it avoids the merge-sort temp buffer.
    spans.sort_unstable_by(|a, b| a.start.cmp(&b.start).then(a.end.cmp(&b.end)));
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
            && later.rule_name == earlier.rule_name
            && later.split_value == earlier.split_value
    });
    spans
}

fn apply_dot_exception<'a>(view: &TextView<'a>, state: &mut PipelineState<'a>) {
    let mut filtered: Vec<SpanRecord<'_>> = Vec::with_capacity(state.final_spans().len());
    for span in state.final_spans().iter() {
        if is_exception_numeric(view, span.start) || is_exception_mailaddress(view, span.start) {
            continue;
        }
        filtered.push(span.clone());
    }
    state.add_layer("DotExceptionAnnotator", filtered);
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
    let mut filtered: Vec<SpanRecord<'_>> = Vec::with_capacity(state.final_spans().len());
    for span in state.final_spans().iter() {
        if is_exception_no_parts(view, span.start, span.end, is_dot_value(span.split_value)) {
            continue;
        }
        filtered.push(span.clone());
    }
    state.add_layer("NumberExceptionAnnotator", filtered);
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
    let mut map: HashMap<usize, (usize, usize)> = HashMap::new();
    for &(start, end) in linebreaks {
        map.insert(start, (start, end));
    }

    let mut result: Vec<SpanRecord<'_>> = Vec::with_capacity(state.final_spans().len() + map.len());
    for span in state.final_spans().iter() {
        if let Some((lb_start, lb_end)) = map.remove(&span.end) {
            result.push(SpanRecord {
                rule_name: "LinebreakForceAnnotator",
                start: lb_start,
                end: lb_end,
                split_type: Some("linebreak"),
                split_value: Some(view.slice(lb_start, lb_end)),
            });
        } else {
            result.push(span.clone());
        }
    }

    for (_, (start, end)) in map.into_iter() {
        result.push(SpanRecord {
            rule_name: "LinebreakForceAnnotator",
            start,
            end,
            split_type: Some("linebreak"),
            split_value: Some(view.slice(start, end)),
        });
    }

    state.add_layer("LinebreakForceAnnotator", result);
}

/// Compact span for the boundaries-only fast path used by [`split_sentences`].
/// Only the fields the downstream rules actually read are kept: the offsets
/// each exception checks, and a flag for whether the matched text is exactly
/// `"."` or `"．"` (the sole `split_value` that `NumberException` inspects).
#[derive(Clone, Copy, Debug)]
struct Bound {
    start: usize,
    end: usize,
    dot: bool,
}

/// Boundaries-only sink: merges each fused-scanner hit into the compact `Bound`
/// list directly — no intermediate `SpanRecord`, no byte-map, no per-span
/// `split_value`. Only the `"."` flag NumberException reads is kept, derived
/// from the offsets the same way `split_value` used to be.
struct BoundMerger<'a> {
    active: &'a mut Vec<Bound>,
    seen: &'a mut HashSet<(usize, usize)>,
    chars: &'a [char],
    /// Newline whitespace runs, in scan order (linebreak spans).
    linebreaks: Vec<(usize, usize)>,
}

impl Emit for BoundMerger<'_> {
    fn emit(&mut self, kind: Kind, start: usize, end: usize) {
        if let Kind::Linebreak = kind {
            self.linebreaks.push((start, end));
            return;
        }
        if self.seen.insert((start, end)) {
            // A span whose whole matched text is "." / "．" iff it spans exactly
            // that single character — equivalent to `split_value == "."`/`"．"`.
            let dot = end == start + 1 && matches!(self.chars[start], '.' | '．');
            self.active.push(Bound { start, end, dot });
        }
    }
}

/// Compute the final sentence-boundary offsets *without* materialising any
/// annotation layer — the fast path behind [`split_sentences`].
///
/// It runs the exact same rules as [`segment_impl`], but forgoes everything
/// that exists only to produce the `Layer` records: the cumulative clones in
/// `add_forward_rule`, the sort for the IndirectQuote layer, the `char→byte`
/// map and the `SpanRecord`/`Span` materialisation (the fused scanner emits
/// straight into [`BoundMerger`]). First-wins-per-(start,end) semantics
/// (including the end-of-text sentinel) make the surviving set exactly the
/// final layer of the full pipeline — verify with the golden/differential suites.
pub(crate) fn segment_bounds_only(text: &str) -> Vec<usize> {
    let view = TextView::new(text);
    let char_len = view.char_len();
    let sentinel = Bound {
        start: if char_len == 0 { 0 } else { char_len - 1 },
        end: char_len,
        dot: false,
    };

    let mut active: Vec<Bound> = Vec::new();
    let mut seen: HashSet<(usize, usize)> = HashSet::new();
    seen.insert((sentinel.start, sentinel.end));
    active.push(sentinel);

    let linebreaks = {
        let mut merger = BoundMerger {
            active: &mut active,
            seen: &mut seen,
            chars: &view.chars,
            linebreaks: Vec::new(),
        };
        scan_all_with(&view.chars, &mut merger);
        merger.linebreaks
    };

    active.retain(|c| !is_exception_particle(&view, c.end));
    active.retain(|c| {
        !is_exception_numeric(&view, c.start) && !is_exception_mailaddress(&view, c.start)
    });
    active.retain(|c| !is_exception_no_parts(&view, c.start, c.end, c.dot));
    active = apply_linebreak_force_bounds(active, &linebreaks);

    let mut ends: Vec<usize> = active.iter().map(|c| c.end).collect();
    ends.sort_unstable();
    ends.dedup();
    ends
}

fn apply_linebreak_force_bounds(
    mut active: Vec<Bound>,
    linebreaks: &[(usize, usize)],
) -> Vec<Bound> {
    let mut map: HashMap<usize, (usize, usize)> = HashMap::new();
    for &(start, end) in linebreaks {
        map.insert(start, (start, end));
    }

    let mut result: Vec<Bound> = Vec::with_capacity(active.len() + map.len());
    for c in active.drain(..) {
        if let Some((lb_start, lb_end)) = map.remove(&c.end) {
            // Linebreak spans' matched text is never "." / "．".
            result.push(Bound {
                start: lb_start,
                end: lb_end,
                dot: false,
            });
        } else {
            result.push(c);
        }
    }
    for (_, (start, end)) in map.into_iter() {
        result.push(Bound {
            start,
            end,
            dot: false,
        });
    }
    result
}

fn filter_previous_rule_same_span<'a>(
    current: Vec<SpanRecord<'a>>,
    previous: &[SpanRecord<'a>],
) -> Vec<SpanRecord<'a>> {
    let prev_keys: HashSet<(usize, usize)> =
        previous.iter().map(|span| (span.start, span.end)).collect();
    let mut seen: HashSet<(usize, usize)> = HashSet::new();

    let mut filtered: Vec<SpanRecord<'_>> = Vec::with_capacity(current.len() + previous.len());
    for span in current {
        let key = (span.start, span.end);
        if prev_keys.contains(&key) {
            continue;
        }
        if seen.insert(key) {
            filtered.push(span);
        }
    }

    filtered.extend(previous.iter().cloned());
    filtered
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
            rule_name: record.rule_name,
            start: record.start,
            end: record.end,
            split_type: record.split_type,
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
    prof::begin("materialize");
    let seg: Segmentation<'_> = segment_impl(text).into();
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
    let boundaries = segment_bounds_only(text);

    // `final_boundaries` holds char indices, but we need byte offsets to slice.
    // Walk char_indices once, cutting at each boundary as we go — this avoids
    // materialising a full char-index→byte-offset Vec (one fewer whole-text pass).
    let mut out = Vec::with_capacity(boundaries.len() + 1);
    let mut start_byte = 0usize;
    let mut bi = 0usize; // next boundary to cut at
    for (char_idx, (byte, _)) in text.char_indices().enumerate() {
        if bi < boundaries.len() && char_idx == boundaries[bi] {
            // `byte` is this char's start and also the byte offset of the
            // just-completed sentence's end boundary.
            out.push(&text[start_byte..byte]);
            start_byte = byte;
            bi += 1;
        }
    }
    // Any boundary equal to the char length (end of text) leaves the final
    // sentence to be emitted here. `start_byte` is always a char boundary.
    if start_byte < text.len() {
        out.push(&text[start_byte..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_mark_detection_matches_reference() {
        let text = "顔文字(*^_^*)だよ。";
        let view = TextView::new(text);
        let spans = scan_all(&view).face;
        assert_eq!(spans.len(), 1);
        let span = &spans[0];
        assert_eq!(span.start, 3);
        assert_eq!(span.end, 10);
        assert_eq!(span.split_value, Some("(*^_^*)"));
    }

    #[test]
    fn indirect_quote_handles_question_particle_followed_by_to() {
        let text = "スタッフ? と話し込み。";
        let view = TextView::new(text);
        let spans = scan_all(&view).basic;
        let target = spans
            .iter()
            .find(|span| span.start == 4 && span.end == 6)
            .expect("expected basic rule span");
        assert!(is_exception_particle(&view, target.end));
    }

    #[test]
    fn segment_pipeline_matches_bunkai_for_staff_question_case() {
        let text = "スタッフ? と話し込み。";
        let output = segment_impl(text);
        let final_bounds = output.final_boundaries;
        assert_eq!(final_bounds, vec![12]);
    }

    #[test]
    fn whitespace_class_matches_unicode_white_space() {
        // Spot-check the 25 White_Space codepoints our scanners rely on.
        let ws: &[char] = &[
            '\u{0009}', '\u{000A}', '\u{000B}', '\u{000C}', '\u{000D}', '\u{0020}', '\u{0085}',
            '\u{00A0}', '\u{1680}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}',
            '\u{2005}', '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}', '\u{200A}', '\u{2028}',
            '\u{2029}', '\u{202F}', '\u{205F}', '\u{3000}',
        ];
        for &ch in ws {
            assert!(is_ws(ch), "expected {ch:?} to be whitespace");
        }
        assert!(!is_ws('a'));
        assert!(!is_ws('。'));
    }

    #[test]
    fn unify_keeps_first_of_duplicate_spans() {
        let spans = vec![
            SpanRecord {
                rule_name: "BasicRule",
                start: 0,
                end: 3,
                split_type: Some("BasicRule"),
                split_value: Some("。"),
            },
            SpanRecord {
                rule_name: "BasicRule",
                start: 0,
                end: 3,
                split_type: Some("BasicRule"),
                split_value: Some("。"),
            },
        ];
        let unified = unify_span_annotations(spans);
        assert_eq!(unified.len(), 1);
    }

    #[test]
    fn bounds_only_path_matches_full_pipeline() {
        // The boundaries-only fast path used by split_sentences must produce
        // the exact final boundaries of the layer-building pipeline. Drive
        // both over a deterministic corpus hitting every rule interaction.
        const BITS: &[&str] = &[
            "日本語文章文字境界判定のための手順検証を行う",
            "あいうえおかきくけこさしすせそ",
            "アイウエオカキクケコサシスセソ",
            "abcXYZ0123456789",
            "まだ2ヶ月も先だけど。",
            "早すぎかな(笑)",
            "楽しみです★",
            "。",
            "！",
            "？",
            "．",
            "。",
            "!?",
            "なのかな？",
            "(笑)",
            "(泣)",
            "…",
            "★",
            "☆",
            "♪",
            "…!",
            "(^_^)",
            "(*´∀｀*)",
            "＼(^o^)／",
            "(^-^)",
            "(｀・ω・´)",
            "😀",
            "😄",
            "👍",
            "🎉",
            "❤️",
            "😀😀😀",
            "👍🏽",
            "👩\u{1F3FC}",
            "🍺",
            "🍣",
            "3.5",
            "3．5",
            "No.1",
            "no.5",
            "4.5点",
            "email@example.comは。",
            "0.5畳",
            "１．２",
            "? と",
            "？って",
            "。という",
            "？という",
            "。て",
            "？に",
            "。など",
            "? くらいでした",
            "？ もあり",
            "。ほどでし",
            "\n",
            "\n\n",
            " \n ",
            "\n\u{3000}\n",
            "\u{3000}\n",
            "\u{00A0}",
            "\t",
            "\r",
            "\u{3000}",
            "(",
            ")",
            "（",
            "）",
            "「」",
            "おすすめ度No.1の和室3.5畳はあります。",
            "スタッフ? と話し込み。",
            "この値段で、こんな夕飯いいの？\nって、くらいおいしかった！",
            "宿を予約しました😄まだ2ヶ月も先だけど。",
            "(近日中には冷房に切り替わる予定です。\nいいですね(泣))",
            "30代の夫婦2組(男2.女2)です。",
            "うーん🤔🤔🤔どうしよう",
        ];
        struct Rng(u64);
        impl Rng {
            fn next(&mut self) -> u64 {
                self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
                let mut z = self.0;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
                z ^ (z >> 31)
            }
            fn below(&mut self, n: usize) -> usize {
                (self.next() % n as u64) as usize
            }
        }
        let mut rng = Rng(0xDECAFBAD1234);
        let mut cases: Vec<String> = Vec::new();
        for _ in 0..5000 {
            let len = rng.below(10) + 1;
            let mut s = String::new();
            for _ in 0..len {
                s.push_str(BITS[rng.below(BITS.len())]);
                if rng.below(4) == 0 {
                    s.push_str(BITS[rng.below(BITS.len())]);
                }
            }
            cases.push(s);
        }
        for _ in 0..200 {
            cases.push(String::new());
            cases.push("。".into());
            cases.push("\n".into());
            cases.push("👍👍👍".into());
            cases.push("A.B.C".into());
        }
        cases.extend(BITS.iter().map(|s| s.to_string()));
        for a in BITS {
            for b in BITS {
                cases.push(format!("{a}{b}"));
            }
        }
        for (i, text) in cases.iter().enumerate() {
            let via_layers = segment(text).final_boundaries;
            let via_bounds = segment_bounds_only(text);
            assert_eq!(
                via_bounds, via_layers,
                "case #{i} bounds-only diverges from full pipeline: {text:?}"
            );
            // Property check for the slicing: split_sentences must cut exactly
            // at the pipeline boundaries — lossless, contiguous, and every hard
            // cut aligned with the next boundary offset.
            let sentences = split_sentences(text);
            let mut cum_chars = 0usize;
            for (k, s) in sentences.iter().enumerate() {
                cum_chars += s.chars().count();
                if k + 1 < sentences.len() {
                    assert_eq!(
                        cum_chars, via_layers[k],
                        "split_sentences cut misaligned at case #{i}: {text:?}"
                    );
                }
            }
            assert_eq!(
                cum_chars,
                text.chars().count(),
                "split_sentences lost/duplicated chars at case #{i}: {text:?}"
            );
        }
    }

    // ---- pre-fusion finder equivalence (guards the single-pass scanner) ----
    #[derive(Clone)]
    struct OldEmojiSpan {
        start: usize,
        end: usize,
        any_target: bool,
    }

    fn old_find_face_marks<'a>(view: &TextView<'a>) -> Vec<SpanRecord<'a>> {
        let mut spans: Vec<SpanRecord<'_>> = Vec::new();
        let len = view.char_len();
        let mut idx = 0usize;

        while idx < len {
            let ch = view.char_at(idx).unwrap_or('\0');
            if ch != '(' && ch != '（' {
                idx += 1;
                continue;
            }

            let mut cursor = idx + 1;
            let mut has_symbol2 = false;
            while cursor < len {
                let current = view.char_at(cursor).unwrap_or('\0');
                if !is_face_symbol1(current) {
                    break;
                }
                if is_face_symbol2(current) {
                    has_symbol2 = true;
                }
                cursor += 1;
            }
            if !has_symbol2 {
                idx += 1;
                continue;
            }
            if cursor >= len {
                idx += 1;
                continue;
            }
            let closing = view.char_at(cursor).unwrap_or('\0');
            if closing != ')' && closing != '）' {
                idx += 1;
                continue;
            }
            let mut end = cursor + 1;
            while end < len && is_face_symbol_prefix_suffix(view.char_at(end).unwrap_or('\0')) {
                end += 1;
            }
            let mut start = idx;
            while start > 0 && is_face_symbol_prefix_suffix(view.char_at(start - 1).unwrap_or('\0'))
            {
                start -= 1;
            }

            spans.push(SpanRecord {
                rule_name: "FaceMarkDetector",
                start,
                end,
                split_type: Some("facemark"),
                split_value: Some(view.slice(start, end)),
            });
            idx = end;
        }

        spans
    }

    fn old_find_emotion_expressions<'a>(view: &TextView<'a>) -> Vec<SpanRecord<'a>> {
        let mut spans: Vec<SpanRecord<'_>> = Vec::new();
        let len = view.char_len();

        for idx in 0..len {
            let ch = view.char_at(idx).unwrap_or('\0');
            if ch != '(' && ch != '（' {
                continue;
            }
            for word in EMOTION_WORDS {
                let word_len = word.chars().count();
                let start_word = idx + 1;
                let end_word = start_word + word_len;
                if end_word >= len {
                    continue;
                }
                if view.slice(start_word, end_word) == *word {
                    let closing = view.char_at(end_word).unwrap_or('\0');
                    if closing == ')' || closing == '）' {
                        let end = end_word + 1;
                        spans.push(SpanRecord {
                            rule_name: "EmotionExpressionAnnotator",
                            start: idx,
                            end,
                            split_type: Some("EmotionExpressionAnnotator"),
                            split_value: Some(view.slice(idx, end)),
                        });
                        break;
                    }
                }
            }
        }

        let mut idx = 0usize;
        while idx < len {
            let ch = view.char_at(idx).unwrap_or('\0');
            if !EMOTION_SYMBOLS.contains(&ch) {
                idx += 1;
                continue;
            }
            let start = idx;
            while idx < len && EMOTION_SYMBOLS.contains(&view.char_at(idx).unwrap_or('\0')) {
                idx += 1;
            }
            let mut end = idx;
            if idx < len {
                let next = view.char_at(idx).unwrap_or('\0');
                if EMOTION_PUNCTUATION.contains(&next) {
                    idx += 1;
                    end += 1;
                }
            }
            spans.push(SpanRecord {
                rule_name: "EmotionExpressionAnnotator",
                start,
                end,
                split_type: Some("EmotionExpressionAnnotator"),
                split_value: Some(view.slice(start, end)),
            });
        }

        unify_span_annotations(spans)
    }

    fn old_find_basic_rule<'a>(view: &TextView<'a>) -> Vec<SpanRecord<'a>> {
        let mut spans: Vec<SpanRecord<'_>> = Vec::new();
        let len = view.char_len();
        let mut i = 0usize;
        while i < len {
            if !is_basic_punct(view.char_at(i).unwrap_or('\0')) {
                i += 1;
                continue;
            }
            let start = i;
            while i < len && is_basic_punct(view.char_at(i).unwrap_or('\0')) {
                i += 1;
            }
            while i < len && is_ws(view.char_at(i).unwrap_or('\0')) {
                i += 1;
            }
            let end = i;
            spans.push(SpanRecord {
                rule_name: "BasicRule",
                start,
                end,
                split_type: Some("BasicRule"),
                split_value: Some(view.slice(start, end)),
            });
        }
        spans
    }

    fn old_build_emoji_spans<'a>(view: &TextView<'a>) -> Vec<SpanRecord<'a>> {
        let spans = old_find_emoji_spans(view);
        spans
            .into_iter()
            .filter(|span| span.end > span.start && span.any_target)
            .map(|span| SpanRecord {
                rule_name: "EmojiAnnotator",
                start: span.start,
                end: span.end,
                split_type: Some("EmojiAnnotator"),
                split_value: Some(view.slice(span.start, span.end)),
            })
            .collect()
    }

    fn old_find_emoji_spans(view: &TextView<'_>) -> Vec<OldEmojiSpan> {
        let mut spans: Vec<OldEmojiSpan> = Vec::new();
        let len = view.char_len();
        let mut i = 0usize;
        while i < len {
            // Skip plain characters; a run starts on an emoji codepoint. Table
            // membership is what matters — "emoji" includes codepoints whose
            // stored category is `None`, e.g. skin-tone modifiers.
            let Some(first) = emoji_lookup(view.chars[i] as u32).copied() else {
                i += 1;
                continue;
            };
            let start = i;
            let mut any_target = first.is_some_and(|cat| TARGET_EMOJI_CATEGORIES.contains(&cat));
            i += 1;
            while i < len {
                let Some(category) = emoji_lookup(view.chars[i] as u32).copied() else {
                    break;
                };
                if category.is_some_and(|cat| TARGET_EMOJI_CATEGORIES.contains(&cat)) {
                    any_target = true;
                }
                i += 1;
            }
            spans.push(OldEmojiSpan {
                start,
                end: i,
                any_target,
            });
        }
        spans
    }

    #[test]
    fn fused_scanner_reproduces_original_finders() {
        // The original per-rule finders returned spans in a fixed per-rule
        // order (face marks, emotion words+symbols, emoji runs, basic runs).
        // The fused single-pass scan_all must reproduce each rule's span list
        // *exactly* (values AND order) so the layer output stays identical.
        const BITS: &[&str] = &[
            "日本語文章文字境界判定のための手順検証を行う",
            "あいうえおかきくけこ",
            "abcXYZ0123456789",
            "まだ2ヶ月も先だけど。",
            "早すぎかな(笑)",
            "楽しみです★",
            "。",
            "！",
            "？",
            "．",
            "!",
            "?",
            "。。",
            "!?",
            "なのかな？",
            "(笑)",
            "(泣)",
            "(汗)",
            "…",
            "★",
            "☆",
            "♪",
            "…!",
            "(^_^)",
            "(*´∀｀*)",
            "＼(^o^)／",
            "(^-^)",
            "(^^)",
            "(。・ω・。)",
            "(｀・ω・´)",
            "😀",
            "😄",
            "👍",
            "🎉",
            "❤️",
            "😀😀😀",
            "👍🏽",
            "👩\u{1F3FC}",
            "🍺",
            "🍣",
            "3.5",
            "3．5",
            "No.1",
            "4.5点",
            "email@example.comは。",
            "? と",
            "？って",
            "。という",
            "？に",
            "など",
            "? くらいでした",
            "？ もあり",
            "\n",
            "\n\n",
            " \n ",
            "\n\u{3000}\n",
            "\u{3000}\n",
            "\u{00A0}",
            "(",
            ")",
            "（",
            "）",
            "「」",
            "スタッフ? と話し込み。",
            "この値段で、こんな夕飯いいの？\nって、くらいおいしかった！",
            "宿を予約しました😄まだ2ヶ月も先だけど。",
            "(近日中には冷房に切り替わる予定です。\nいいですね(泣))",
            "30代の夫婦2組(男2.女2)です。",
            "うーん🤔🤔🤔どうしよう",
        ];
        struct Rng(u64);
        impl Rng {
            fn next(&mut self) -> u64 {
                self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
                let mut z = self.0;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
                z ^ (z >> 31)
            }
            fn below(&mut self, n: usize) -> usize {
                (self.next() % n as u64) as usize
            }
        }
        let mut rng = Rng(0xF005BA11);
        let mut cases: Vec<String> = Vec::with_capacity(BITS.len() * BITS.len() + 5000);
        for _ in 0..5000 {
            let mut s = String::new();
            let len = rng.below(10) + 1;
            for _ in 0..len {
                s.push_str(BITS[rng.below(BITS.len())]);
                if rng.below(4) == 0 {
                    s.push_str(BITS[rng.below(BITS.len())]);
                }
            }
            cases.push(s);
        }
        for a in BITS {
            for b in BITS {
                cases.push(format!("{a}{b}"));
            }
        }
        cases.extend(BITS.iter().map(|s| s.to_string()));

        for (i, text) in cases.iter().enumerate() {
            let view = TextView::new(text);
            let scan = scan_all(&view);
            let old_face = old_find_face_marks(&view);
            let old_emotion = old_find_emotion_expressions(&view);
            let old_emoji = old_build_emoji_spans(&view);
            let old_basic = old_find_basic_rule(&view);
            let old_lb = old_find_linebreaks(&view);
            assert_eq!(scan.face, old_face, "case {i} face: {text:?}");
            assert_eq!(scan.emotion, old_emotion, "case {i} emotion: {text:?}");
            assert_eq!(scan.emoji, old_emoji, "case {i} emoji: {text:?}");
            assert_eq!(scan.basic, old_basic, "case {i} basic: {text:?}");
            assert_eq!(scan.linebreaks, old_lb, "case {i} linebreaks: {text:?}");
        }
    }

    /// The linebreak pass before it was folded into the fused scanner.
    fn old_find_linebreaks(view: &TextView<'_>) -> Vec<(usize, usize)> {
        let mut out: Vec<(usize, usize)> = Vec::new();
        let len = view.char_len();
        let mut i = 0usize;
        while i < len {
            if !is_ws(view.char_at(i).unwrap_or('\0')) {
                i += 1;
                continue;
            }
            let start = i;
            let mut has_newline = false;
            while i < len && is_ws(view.char_at(i).unwrap_or('\0')) {
                if view.char_at(i) == Some('\n') {
                    has_newline = true;
                }
                i += 1;
            }
            if has_newline {
                out.push((start, i));
            }
        }
        out
    }
}
