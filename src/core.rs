use once_cell::sync::Lazy;
use regex::Regex;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use crate::emoji_data;

const BASIC_RULE_RE: &str = "[。!?.！？．]+\\s*";
const LINEBREAK_RE: &str = "[\\n\\s]*\\n[\\n\\s]*";

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

static BASIC_RULE_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(BASIC_RULE_RE).unwrap());
static LINEBREAK_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(LINEBREAK_RE).unwrap());

static EMOJI_CATEGORY_MAP: Lazy<HashMap<u32, Option<&'static str>>> = Lazy::new(|| {
    let mut map = HashMap::with_capacity(emoji_data::EMOJI_DATA.len());
    for (codepoint, category) in emoji_data::EMOJI_DATA.iter().copied() {
        map.insert(codepoint, category);
    }
    map
});

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

#[derive(Clone, Debug)]
pub(crate) struct SpanRecord {
    pub(crate) rule_name: &'static str,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) split_type: Option<&'static str>,
    pub(crate) split_value: Option<String>,
}

#[derive(Clone)]
pub(crate) struct LayerOutput {
    pub(crate) name: &'static str,
    pub(crate) spans: Vec<SpanRecord>,
}

struct PipelineState {
    layers: Vec<LayerOutput>,
    name_to_index: HashMap<&'static str, usize>,
    final_index: usize,
}

impl PipelineState {
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

    fn final_spans(&self) -> &[SpanRecord] {
        &self.layers[self.final_index].spans
    }

    fn get_layer(&self, name: &'static str) -> Option<&[SpanRecord]> {
        self.name_to_index
            .get(name)
            .map(|idx| self.layers[*idx].spans.as_slice())
    }

    fn add_layer(&mut self, name: &'static str, spans: Vec<SpanRecord>) {
        let idx = self.layers.len();
        self.layers.push(LayerOutput { name, spans });
        self.name_to_index.insert(name, idx);
        self.final_index = idx;
    }

    fn add_forward_rule(&mut self, name: &'static str, spans: Vec<SpanRecord>) {
        let filtered = filter_previous_rule_same_span(spans, self.final_spans());
        self.add_layer(name, filtered);
    }

    fn into_output(self) -> PipelineOutput {
        let mut unique: HashSet<usize> = HashSet::new();
        for span in &self.layers[self.final_index].spans {
            unique.insert(span.end);
        }
        let mut final_boundaries: Vec<usize> = unique.into_iter().collect();
        final_boundaries.sort_unstable();
        PipelineOutput {
            layers: self.layers,
            final_boundaries,
        }
    }
}

pub(crate) struct PipelineOutput {
    pub(crate) layers: Vec<LayerOutput>,
    pub(crate) final_boundaries: Vec<usize>,
}

pub(crate) struct TextView<'a> {
    text: &'a str,
    chars: Vec<char>,
    char_to_byte: Vec<usize>,
}

impl<'a> TextView<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        let mut chars = Vec::new();
        let mut char_to_byte = Vec::new();
        for (byte_idx, ch) in text.char_indices() {
            chars.push(ch);
            char_to_byte.push(byte_idx);
        }
        Self {
            text,
            chars,
            char_to_byte,
        }
    }

    fn text(&self) -> &'a str {
        self.text
    }

    fn char_len(&self) -> usize {
        self.chars.len()
    }

    fn char_at(&self, index: usize) -> Option<char> {
        self.chars.get(index).copied()
    }

    fn slice(&self, start: usize, end: usize) -> &'a str {
        let start_byte = if start >= self.char_to_byte.len() {
            self.text.len()
        } else {
            self.char_to_byte[start]
        };
        let end_byte = if end >= self.char_to_byte.len() {
            self.text.len()
        } else {
            self.char_to_byte[end]
        };
        &self.text[start_byte..end_byte]
    }

    fn byte_to_char_index(&self, byte: usize) -> usize {
        match self.char_to_byte.binary_search(&byte) {
            Ok(idx) => idx,
            Err(idx) => idx,
        }
    }

    fn starts_with(&self, index: usize, pattern: &str) -> bool {
        let pattern_chars: Vec<char> = pattern.chars().collect();
        if index + pattern_chars.len() > self.char_len() {
            return false;
        }
        for (offset, ch) in pattern_chars.iter().enumerate() {
            if self.chars[index + offset] != *ch {
                return false;
            }
        }
        true
    }
}

#[derive(Clone)]
struct EmojiSpan {
    start: usize,
    end: usize,
    categories: Vec<Option<&'static str>>,
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

fn find_face_marks(view: &TextView<'_>) -> Vec<SpanRecord> {
    let mut spans: Vec<SpanRecord> = Vec::new();
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
            split_value: Some(view.slice(start, end).to_string()),
        });
        idx = end;
    }

    spans
}

fn find_emotion_expressions(view: &TextView<'_>) -> Vec<SpanRecord> {
    let mut spans: Vec<SpanRecord> = Vec::new();
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
                        split_value: Some(view.slice(idx, end).to_string()),
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
            split_value: Some(view.slice(start, end).to_string()),
        });
    }

    unify_span_annotations(spans)
}

pub(crate) fn segment_impl(text: &str) -> PipelineOutput {
    let view = TextView::new(text);
    let mut state = PipelineState::new(view.char_len());

    let face_spans = find_face_marks(&view);
    state.add_forward_rule("FaceMarkDetector", face_spans);

    let emotion_spans = find_emotion_expressions(&view);
    state.add_forward_rule("EmotionExpressionAnnotator", emotion_spans);

    let emoji_spans = build_emoji_spans(&view);
    state.add_forward_rule("EmojiAnnotator", emoji_spans);

    let basic_rule_spans =
        build_spans_from_regex(&view, "BasicRule", Some("BasicRule"), &BASIC_RULE_REGEX);
    state.add_forward_rule("BasicRule", basic_rule_spans);

    apply_indirect_quote(&view, &mut state);
    apply_dot_exception(&view, &mut state);
    apply_number_exception(&view, &mut state);
    apply_linebreak_force(&view, &mut state);

    state.into_output()
}

fn build_spans_from_regex(
    view: &TextView<'_>,
    rule_name: &'static str,
    split_type: Option<&'static str>,
    regex: &Regex,
) -> Vec<SpanRecord> {
    regex
        .find_iter(view.text())
        .map(|m| {
            let start = view.byte_to_char_index(m.start());
            let end = view.byte_to_char_index(m.end());
            SpanRecord {
                rule_name,
                start,
                end,
                split_type,
                split_value: Some(view.slice(start, end).to_string()),
            }
        })
        .collect()
}

fn build_emoji_spans(view: &TextView<'_>) -> Vec<SpanRecord> {
    let spans = find_emoji_spans(view);
    spans
        .into_iter()
        .filter(|span| span.end > span.start)
        .filter(|span| {
            span.categories
                .iter()
                .flatten()
                .any(|cat| TARGET_EMOJI_CATEGORIES.contains(cat))
        })
        .map(|span| SpanRecord {
            rule_name: "EmojiAnnotator",
            start: span.start,
            end: span.end,
            split_type: Some("EmojiAnnotator"),
            split_value: Some(view.slice(span.start, span.end).to_string()),
        })
        .collect()
}

fn find_emoji_spans(view: &TextView<'_>) -> Vec<EmojiSpan> {
    let mut spans: Vec<EmojiSpan> = Vec::new();
    let mut in_span = false;
    let mut start = 0usize;
    let mut categories: Vec<Option<&'static str>> = Vec::new();

    for idx in 0..view.char_len() {
        let ch = view.char_at(idx).unwrap_or('\0');
        let maybe_category = EMOJI_CATEGORY_MAP.get(&(ch as u32));
        if maybe_category.is_none() {
            if in_span {
                spans.push(EmojiSpan {
                    start,
                    end: idx,
                    categories: categories.clone(),
                });
                categories.clear();
                in_span = false;
            }
            continue;
        }

        if !in_span {
            in_span = true;
            start = idx;
        }
        categories.push(*maybe_category.unwrap());

        let next_is_emoji = if idx + 1 < view.char_len() {
            let next_ch = view.char_at(idx + 1).unwrap_or('\0') as u32;
            EMOJI_CATEGORY_MAP.contains_key(&next_ch)
        } else {
            false
        };
        if !next_is_emoji {
            spans.push(EmojiSpan {
                start,
                end: idx + 1,
                categories: categories.clone(),
            });
            categories.clear();
            in_span = false;
        }
    }

    if in_span {
        spans.push(EmojiSpan {
            start,
            end: view.char_len(),
            categories,
        });
    }

    spans
}

fn apply_indirect_quote(view: &TextView<'_>, state: &mut PipelineState) {
    let mut collected: Vec<SpanRecord> = Vec::new();
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
    state.add_layer("IndirectQuoteExceptionAnnotator", unified);
}

fn is_exception_particle(view: &TextView<'_>, _start: usize, end: usize) -> bool {
    if end >= view.char_len() {
        return false;
    }
    let mut idx = end;
    if idx >= view.char_len() {
        return false;
    }
    while view.char_at(idx) == Some('\n') && idx + 1 < view.char_len() {
        idx += 1;
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

fn unify_span_annotations(spans: Vec<SpanRecord>) -> Vec<SpanRecord> {
    let mut map: HashMap<String, SpanRecord> = HashMap::new();
    for span in spans {
        let key = format!(
            "{}-{}-{}/{}",
            span.start,
            span.end,
            span.rule_name,
            span.split_value.as_deref().unwrap_or("")
        );
        map.entry(key).or_insert(span);
    }
    let mut values: Vec<SpanRecord> = map.into_values().collect();
    values.sort_by(|a, b| match a.start.cmp(&b.start) {
        Ordering::Equal => a.end.cmp(&b.end),
        other => other,
    });
    values
}

fn apply_dot_exception(view: &TextView<'_>, state: &mut PipelineState) {
    let mut filtered: Vec<SpanRecord> = Vec::new();
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

fn apply_number_exception(view: &TextView<'_>, state: &mut PipelineState) {
    let mut filtered: Vec<SpanRecord> = Vec::new();
    for span in state.final_spans().iter() {
        if is_exception_no(view, span) {
            continue;
        }
        filtered.push(span.clone());
    }
    state.add_layer("NumberExceptionAnnotator", filtered);
}

fn is_exception_no(view: &TextView<'_>, span: &SpanRecord) -> bool {
    let value = span.split_value.as_deref();
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

fn apply_linebreak_force(view: &TextView<'_>, state: &mut PipelineState) {
    let mut map: HashMap<usize, (usize, usize)> = HashMap::new();
    for mat in LINEBREAK_REGEX.find_iter(view.text()) {
        let start = view.byte_to_char_index(mat.start());
        let end = view.byte_to_char_index(mat.end());
        map.insert(start, (start, end));
    }

    let mut result: Vec<SpanRecord> = Vec::new();
    for span in state.final_spans().iter() {
        if let Some((lb_start, lb_end)) = map.remove(&span.end) {
            result.push(SpanRecord {
                rule_name: "LinebreakForceAnnotator",
                start: lb_start,
                end: lb_end,
                split_type: Some("linebreak"),
                split_value: Some(view.slice(lb_start, lb_end).to_string()),
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
            split_value: Some(view.slice(start, end).to_string()),
        });
    }

    state.add_layer("LinebreakForceAnnotator", result);
}

fn filter_previous_rule_same_span(
    current: Vec<SpanRecord>,
    previous: &[SpanRecord],
) -> Vec<SpanRecord> {
    let prev_keys: HashSet<(usize, usize)> =
        previous.iter().map(|span| (span.start, span.end)).collect();
    let mut seen: HashSet<(usize, usize)> = HashSet::new();

    let mut filtered: Vec<SpanRecord> = Vec::new();
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

impl From<SpanRecord> for Span {
    fn from(record: SpanRecord) -> Self {
        Span {
            rule_name: record.rule_name.to_string(),
            start: record.start,
            end: record.end,
            split_type: record.split_type.map(|s| s.to_string()),
            split_value: record.split_value,
        }
    }
}

impl From<LayerOutput> for Layer {
    fn from(layer: LayerOutput) -> Self {
        Layer {
            name: layer.name.to_string(),
            spans: layer.spans.into_iter().map(Span::from).collect(),
        }
    }
}

impl From<PipelineOutput> for Segmentation {
    fn from(output: PipelineOutput) -> Self {
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
    segment_impl(text).into()
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
pub fn split_sentences(text: &str) -> Vec<&str> {
    let boundaries = segment(text).final_boundaries;

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
        assert_eq!(span.split_value.as_deref(), Some("(*^_^*)"));
    }

    #[test]
    fn indirect_quote_handles_question_particle_followed_by_to() {
        let text = "スタッフ? と話し込み。";
        let view = TextView::new(text);
        let spans =
            build_spans_from_regex(&view, "BasicRule", Some("BasicRule"), &BASIC_RULE_REGEX);
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
}
