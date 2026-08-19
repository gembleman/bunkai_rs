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
const INDIRECT_RULE_TARGETS: &[&str] = &[
    "first",
    "BasicRule",
    "LinebreakAnnotator",
    "EmojiAnnotator",
    "EmotionExpressionAnnotator",
    "FaceMarkDetector",
];

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

#[derive(Clone, Debug)]
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
    name_to_index: HashMap<&'static str, usize>,
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
        let mut name_to_index = HashMap::new();
        name_to_index.insert("first", 0);
        Self {
            layers: vec![layer],
            name_to_index,
            final_index: 0,
        }
    }

    fn final_spans(&self) -> &[SpanRecord<'a>] {
        &self.layers[self.final_index].spans
    }

    fn get_layer(&self, name: &'static str) -> Option<&[SpanRecord<'a>]> {
        self.name_to_index
            .get(name)
            .map(|idx| self.layers[*idx].spans.as_slice())
    }

    fn add_layer(&mut self, name: &'static str, spans: Vec<SpanRecord<'a>>) {
        let idx = self.layers.len();
        self.layers.push(LayerOutput { name, spans });
        self.name_to_index.insert(name, idx);
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
    char_to_byte: Vec<usize>,
}

impl<'a> TextView<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        // Build byte offsets in a second pass over the (cheap) char Vec rather
        // than re-decoding UTF-8 bytes.
        let mut char_to_byte = Vec::with_capacity(chars.len() + 1);
        let mut byte = 0usize;
        for ch in &chars {
            char_to_byte.push(byte);
            byte += ch.len_utf8();
        }
        Self {
            text,
            chars,
            char_to_byte,
        }
    }

    fn char_len(&self) -> usize {
        self.chars.len()
    }

    #[inline]
    fn char_at(&self, index: usize) -> Option<char> {
        self.chars.get(index).copied()
    }

    /// Byte offset of a character index, clamped to `text.len()`.
    #[inline]
    fn byte_at(&self, index: usize) -> usize {
        if index >= self.char_to_byte.len() {
            self.text.len()
        } else {
            self.char_to_byte[index]
        }
    }

    fn slice(&self, start: usize, end: usize) -> &'a str {
        let start_byte = self.byte_at(start);
        let end_byte = self.byte_at(end);
        &self.text[start_byte..end_byte]
    }

    /// Byte-wise `starts_with` at a character index. `index` must be a char
    /// boundary (it always is: every caller hands it a previously decoded
    /// character index).
    #[inline]
    fn starts_with(&self, index: usize, pattern: &str) -> bool {
        let Some(&start_byte) = self.char_to_byte.get(index) else {
            return false;
        };
        self.text[start_byte..].starts_with(pattern)
    }
}

#[derive(Clone)]
struct EmojiSpan {
    start: usize,
    end: usize,
    /// Whether any character in the run belongs to a target category.
    any_target: bool,
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

fn find_face_marks<'a>(view: &TextView<'a>) -> Vec<SpanRecord<'a>> {
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
        while start > 0 && is_face_symbol_prefix_suffix(view.char_at(start - 1).unwrap_or('\0')) {
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

fn find_emotion_expressions<'a>(view: &TextView<'a>) -> Vec<SpanRecord<'a>> {
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

pub(crate) fn segment_impl(text: &str) -> PipelineOutput<'_> {
    let prof_base = prof::enter();
    let view = TextView::new(text);
    prof::begin("new(TextView)");
    let mut state = PipelineState::new(view.char_len());
    prof::end();

    prof::begin("FaceMarkDetector");
    let face_spans = find_face_marks(&view);
    state.add_forward_rule("FaceMarkDetector", face_spans);
    prof::end();

    prof::begin("EmotionExpressionAnnotator");
    let emotion_spans = find_emotion_expressions(&view);
    state.add_forward_rule("EmotionExpressionAnnotator", emotion_spans);
    prof::end();

    prof::begin("EmojiAnnotator");
    let emoji_spans = build_emoji_spans(&view);
    state.add_forward_rule("EmojiAnnotator", emoji_spans);
    prof::end();

    prof::begin("BasicRule");
    let basic_rule_spans = find_basic_rule(&view);
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
    apply_linebreak_force(&view, &mut state);
    prof::end();

    prof::begin("into_output");
    let out = state.into_output();
    prof::end();
    prof::on_impl_done(prof_base);
    out
}

/// Hand-written equivalent of the old `[。!?.！？．]+\s*` regex: a maximal run
/// of basic punctuators, then any run of Unicode whitespace.
fn find_basic_rule<'a>(view: &TextView<'a>) -> Vec<SpanRecord<'a>> {
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

fn build_emoji_spans<'a>(view: &TextView<'a>) -> Vec<SpanRecord<'a>> {
    let spans = find_emoji_spans(view);
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

fn find_emoji_spans(view: &TextView<'_>) -> Vec<EmojiSpan> {
    let mut spans: Vec<EmojiSpan> = Vec::new();
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
        spans.push(EmojiSpan {
            start,
            end: i,
            any_target,
        });
    }
    spans
}

fn apply_indirect_quote<'a>(view: &TextView<'a>, state: &mut PipelineState<'a>) {
    prof::begin("i.unify");
    let total_target: usize = INDIRECT_RULE_TARGETS
        .iter()
        .filter_map(|&t| state.get_layer(t).map(|l| l.len()))
        .sum();
    let mut collected: Vec<SpanRecord<'_>> = Vec::with_capacity(total_target);
    for &target in INDIRECT_RULE_TARGETS {
        if let Some(layer) = state.get_layer(target) {
            for span in layer {
                if is_exception_particle(view, span.start, span.end) {
                    continue;
                }
                collected.push(span.clone());
            }
        }
    }
    let unified = unify_span_annotations(collected);
    prof::end();
    state.add_layer("IndirectQuoteExceptionAnnotator", unified);
}

fn is_exception_particle(view: &TextView<'_>, _start: usize, end: usize) -> bool {
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
        if matches_rule(view, idx, rule) {
            return true;
        }
    }
    false
}

fn matches_rule(view: &TextView<'_>, mut index: usize, rule: &[&str]) -> bool {
    for part in rule {
        if !view.starts_with(index, part) {
            return false;
        }
        index += part.chars().count();
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
        if is_exception_no(view, span) {
            continue;
        }
        filtered.push(span.clone());
    }
    state.add_layer("NumberExceptionAnnotator", filtered);
}

fn is_exception_no(view: &TextView<'_>, span: &SpanRecord<'_>) -> bool {
    let value = span.split_value;
    if value != Some(".") && value != Some("．") {
        return false;
    }
    if span.start < 2 {
        return false;
    }
    if span.end >= view.char_len() {
        return false;
    }
    let n_char = view.char_at(span.start - 2).unwrap_or('\0');
    let o_char = view.char_at(span.start - 1).unwrap_or('\0');
    if !matches!(n_char, 'N' | 'n' | 'Ｎ' | 'ｎ') {
        return false;
    }
    if !matches!(o_char, 'O' | 'o' | 'Ｏ' | 'ｏ') {
        return false;
    }
    let next_char = view.char_at(span.end).unwrap_or('\0');
    next_char.is_ascii_digit()
}

/// Hand-written equivalent of the old `[\n\s]*\n[\n\s]*` regex: a maximal run
/// of Unicode whitespace that contains at least one newline.
fn find_linebreaks(view: &TextView<'_>) -> Vec<(usize, usize)> {
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

fn apply_linebreak_force<'a>(view: &TextView<'a>, state: &mut PipelineState<'a>) {
    let mut map: HashMap<usize, (usize, usize)> = HashMap::new();
    for (start, end) in find_linebreaks(view) {
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
/// Offsets are measured in Unicode scalar values (`char` indices) over the
/// input string, not byte offsets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub rule_name: String,
    pub start: usize,
    pub end: usize,
    pub split_type: Option<String>,
    pub split_value: Option<String>,
}

/// One stage in the segmentation pipeline, named after the rule that produced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer {
    pub name: String,
    pub spans: Vec<Span>,
}

/// The full result of segmenting a piece of text.
///
/// `layers` records every intermediate pipeline stage; `final_boundaries`
/// gives the unique character offsets at which the final layer would cut the
/// input into sentences.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segmentation {
    pub layers: Vec<Layer>,
    pub final_boundaries: Vec<usize>,
}

impl<'a> From<SpanRecord<'a>> for Span {
    fn from(record: SpanRecord<'a>) -> Self {
        Span {
            rule_name: record.rule_name.to_string(),
            start: record.start,
            end: record.end,
            split_type: record.split_type.map(|s| s.to_string()),
            split_value: record.split_value.map(|s| s.to_string()),
        }
    }
}

impl<'a> From<LayerOutput<'a>> for Layer {
    fn from(layer: LayerOutput<'a>) -> Self {
        Layer {
            name: layer.name.to_string(),
            spans: layer.spans.into_iter().map(Span::from).collect(),
        }
    }
}

impl<'a> From<PipelineOutput<'a>> for Segmentation {
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
/// pipeline layer and the final list of character-offset boundaries.
pub fn segment(text: &str) -> Segmentation {
    prof::begin("materialize");
    let seg: Segmentation = segment_impl(text).into();
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
/// Run the full pipeline but only keep the final boundary offsets.
///
/// This is the fast path used by [`split_sentences`]: it never materialises
/// the per-layer span records into owned [`Span`]s, which is where most of the
/// allocation cost of [`segment`] lives.
pub fn split_sentences(text: &str) -> Vec<&str> {
    split_sentences_impl(text)
}

fn split_sentences_impl(text: &str) -> Vec<&str> {
    let boundaries = segment_impl(text).final_boundaries;

    // final_boundaries holds char indices; map them back to byte offsets.
    let char_to_byte: Vec<usize> = text
        .char_indices()
        .map(|(byte, _)| byte)
        .chain(std::iter::once(text.len()))
        .collect();

    let mut out = Vec::with_capacity(boundaries.len());
    let mut start = 0usize;
    for &end in &boundaries {
        if end >= char_to_byte.len() || end < start {
            continue;
        }
        out.push(&text[char_to_byte[start]..char_to_byte[end]]);
        start = end;
    }
    if start < char_to_byte.len() && char_to_byte[start] < text.len() {
        out.push(&text[char_to_byte[start]..]);
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
        let spans = find_face_marks(&view);
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
        let spans = find_basic_rule(&view);
        let target = spans
            .iter()
            .find(|span| span.start == 4 && span.end == 6)
            .expect("expected basic rule span");
        assert!(is_exception_particle(&view, target.start, target.end));
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
}
