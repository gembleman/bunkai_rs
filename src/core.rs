use std::cell::OnceCell;

use crate::emoji_data;

#[cfg(feature = "prof")]
mod prof {
    use std::cell::RefCell;
    use std::cmp::Reverse;
    use std::time::{Duration, Instant};

    const DUMP_EVERY_CALLS: usize = 100;

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

    pub fn enter() -> usize {
        if on() {
            LOG.with(|l| l.borrow().len())
        } else {
            0
        }
    }

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

    pub fn on_impl_done(base: usize) {
        fold(base);
        let at = CALLS.with(|c| {
            *c.borrow_mut() += 1;
            *c.borrow() % DUMP_EVERY_CALLS
        });
        if at == 0 {
            dump();
        }
    }

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

#[inline]
fn is_basic_punct(ch: char) -> bool {
    matches!(ch, '。' | '!' | '?' | '.' | '！' | '？' | '．')
}

const MAX_WHITE_SPACE_CODEPOINT: u32 = 0x3000;

#[inline]
fn is_ws(ch: char) -> bool {
    if (ch as u32) > MAX_WHITE_SPACE_CODEPOINT {
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

    #[inline]
    fn byte_at(&self, index: usize) -> usize {
        let map = self.char_to_byte.get_or_init(|| {
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

const EMOJI_ABSENT: u8 = 0;
const EMOJI_NON_TARGET: u8 = 1;
const EMOJI_TARGET: u8 = 2;

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

#[inline]
fn emoji_lookup(codepoint: u32) -> u8 {
    if codepoint < 0xA9 {
        return EMOJI_ABSENT;
    }
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
        debug_assert_eq!(EMOJI_LOW_END, EMOJI_HIGH_START);
        return EMOJI_ABSENT;
    }
    let slot = (codepoint - 0x1F000) as usize;
    if slot >= EMOJI_HIGH_LOOKUP.len() {
        return EMOJI_ABSENT;
    }
    EMOJI_HIGH_LOOKUP[slot]
}
const _: () = assert!(EMOJI_LOW_END == EMOJI_HIGH_START);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Kind {
    Sentinel,
    Face,
    Emotion,
    Emoji,
    Basic,
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

trait Emit {
    fn emit(&mut self, kind: Kind, start: usize, end: usize);
}

struct SpanCollector<'v, 'a> {
    view: &'v TextView<'a>,
    face: Vec<SpanRecord<'a>>,
    emotion: Vec<SpanRecord<'a>>,
    emoji: Vec<SpanRecord<'a>>,
    basic: Vec<SpanRecord<'a>>,
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

fn scan_all_with<E: Emit>(chars: &[char], out: &mut E) {
    let len = chars.len();
    let mut i = 0usize;
    while i < len {
        let ch = chars[i];
        match ch {
            '(' | '（' => {
                scan_emotion_word(chars, len, i, out);
                scan_face_mark(chars, len, i, out);
            }
            '。' | '!' | '?' | '.' | '！' | '？' | '．' => {
                if i == 0 || !is_basic_punct(chars[i - 1]) {
                    scan_basic_run(chars, len, i, out);
                }
            }
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

fn apply_linebreak_force<'a>(
    view: &TextView<'a>,
    state: &mut PipelineState<'a>,
    linebreaks: &[(usize, usize)],
) {
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

#[derive(Clone, Copy, Debug)]
struct Bound {
    start: u32,
    end: u32,
}

enum ByteBoundaries {
    WholeText,
    Multiple(Vec<usize>),
}

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

    active.retain(|c| {
        let start = c.start as usize;
        let end = c.end as usize;
        if is_exception_particle_bytes(text, end) {
            return false;
        }
        if is_exception_numeric_bytes(text, start) || is_exception_mailaddress_bytes(text, start) {
            return false;
        }
        let is_dot = byte_bound_is_dot(text, start, end);
        !is_exception_no_parts_bytes(text, start, end, is_dot)
    });

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

#[inline]
fn char_at_byte_opt(bytes: &[u8], b: usize) -> Option<(char, usize)> {
    if b >= bytes.len() {
        return None;
    }
    let (c, n) = char_at_byte(bytes, b);
    Some((c, n))
}

#[inline]
fn prev_char_start(bytes: &[u8], b: usize) -> usize {
    debug_assert!(b > 0 && b <= bytes.len());
    let mut p = b - 1;
    while p > 0 && (bytes[p] & 0xC0) == 0x80 {
        p -= 1;
    }
    p
}

const BORING_SENTINEL: char = 'a';

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

#[inline]
fn cjk_skip_ok(bytes: &[u8], b: usize) -> bool {
    let lead = bytes[b];
    if (0xE4..=0xE9).contains(&lead) {
        return true;
    }
    if lead == 0xE3 {
        if let Some(&second) = bytes.get(b + 1) {
            return (0x81..=0x83).contains(&second);
        }
    }
    false
}

#[inline]
fn read_u64_unaligned(bytes: &[u8], b: usize) -> u64 {
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[b..b + 8]);
    u64::from_le_bytes(buf)
}

fn cjk_skip_run(bytes: &[u8], mut b: usize, len: usize) -> usize {
    const LOW7: u64 = 0x7F7F_7F7F_7F7F_7F7F;
    const REP_E3: u64 = 0xE3E3E3E3E3E3E3E3;
    const TOP: u64 = 0x8080_8080_8080_8080;
    #[inline(always)]
    fn ge(y: u64, k: u64) -> u64 {
        y.wrapping_add((0x80 - k) * 0x0101_0101_0101_0101)
    }
    while b + 8 <= len {
        let w = read_u64_unaligned(bytes, b);
        let y = w & LOW7;
        let e4e9 = w & ge(y, 0x64) & !ge(y, 0x6A) & TOP;
        let x = w ^ REP_E3;
        let e3 = !(((x & LOW7).wrapping_add(LOW7)) | x) & TOP;
        let next = w >> 8;
        let ny = next & LOW7;
        let next_ok = next & ge(ny, 1) & !ge(ny, 4) & TOP;
        let lead = e4e9 | (e3 & next_ok);
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

fn scan_bounds_bytes(text: &str, active: &mut Vec<Bound>, linebreaks: &mut Vec<(usize, usize)>) {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut b = 0usize;
    let mut prev: Option<char> = None;
    let mut prev_is_emoji = false;
    while b < len {
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
        return false;
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
    let mut consumed = vec![false; linebreaks.len()];
    let mut result: Vec<Bound> = Vec::with_capacity(active.len() + linebreaks.len());
    for c in active {
        let end = c.end as usize;
        let replaced = match linebreaks.binary_search_by_key(&end, |&(s, _)| s) {
            Ok(i) if !consumed[i] => {
                consumed[i] = true;
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

fn filter_previous_rule_same_span<'a>(
    current: Vec<SpanRecord<'a>>,
    previous: &[SpanRecord<'a>],
    sentinel_key: (usize, usize),
) -> Vec<SpanRecord<'a>> {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span<'a> {
    pub rule_name: &'static str,
    pub start: usize,
    pub end: usize,
    pub split_type: Option<&'static str>,
    pub split_value: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer<'a> {
    pub name: &'static str,
    pub spans: Vec<Span<'a>>,
}

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

pub fn segment(text: &str) -> Segmentation<'_> {
    let output = segment_impl(text);
    prof::begin("materialize");
    let seg: Segmentation<'_> = output.into();
    prof::end();
    prof::on_segment_done();
    seg
}

pub fn split_sentences(text: &str) -> Vec<&str> {
    split_sentences_impl(text)
}

fn split_sentences_impl(text: &str) -> Vec<&str> {
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
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

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
