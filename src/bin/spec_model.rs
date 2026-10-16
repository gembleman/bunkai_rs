use std::collections::HashMap;

use bunkai_rs::EMOJI_DATA;

#[inline]
fn is_basic_punct(c: char) -> bool {
    matches!(c, '。' | '!' | '?' | '.' | '！' | '？' | '．')
}

#[inline]
fn is_ws(c: char) -> bool {
    matches!(
        c,
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

#[inline]
fn is_emotion_symbol(c: char) -> bool {
    matches!(c, '…' | '★' | '☆' | '♪')
}

#[inline]
fn is_emotion_punct(c: char) -> bool {
    is_basic_punct(c)
}

const EMOTION_WORDS: &[&str] = &[
    "笑", "泣", "汗", "涙", "怒", "嬉", "爆", "驚", "喜", "悲", "謎", "恥", "焦", "苦", "照", "憂",
    "笑い", "わら", "泣き", "怒り", "照れ",
];

fn in_face_ranges(c: char) -> bool {
    matches!(
        c,
        '!'..='/' | ':'..='@' | '['..='`' | '{'..='~'
            | '！'..='／' | '：'..='＠' | '［'..='｀' | '｛'..='～'
            | '\u{00A1}'..='\u{0FFF}' | '\u{1000}'..='\u{2FFF}'
    )
}

#[inline]
fn is_face_prefix_suffix(c: char) -> bool {
    !matches!(c, '(' | ')' | '（' | '）') && (in_face_ranges(c) || matches!(c, '\u{4E00}' | '艸'))
}

#[inline]
fn is_face_symbol2(c: char) -> bool {
    is_face_prefix_suffix(c)
}

#[inline]
fn is_face_symbol1(c: char) -> bool {
    c.is_ascii_alphanumeric() || is_fullwidth_alnum(c) || is_face_symbol2(c)
}

#[inline]
fn is_fullwidth_alnum(c: char) -> bool {
    matches!(c, '０'..='９' | 'Ａ'..='Ｚ' | 'ａ'..='ｚ')
}

#[inline]
fn is_numeric_char(c: char) -> bool {
    matches!(
        c,
        '0'..='9'
            | '０'..='９'
            | '〇'
            | '一' | '二' | '三' | '四' | '五' | '六' | '七' | '八' | '九'
            | '十' | '百' | '千' | '万' | '億' | '兆' | '京'
    )
}

#[inline]
fn is_mail_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
}

#[inline]
fn is_no_n(c: char) -> bool {
    matches!(c, 'N' | 'n' | 'Ｎ' | 'ｎ')
}

#[inline]
fn is_no_o(c: char) -> bool {
    matches!(c, 'O' | 'o' | 'Ｏ' | 'ｏ')
}

fn emoji_entry(c: char) -> Option<Option<&'static str>> {
    let cp = c as u32;
    let (mut lo, mut hi) = (0usize, EMOJI_DATA.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if EMOJI_DATA[mid].0 < cp {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo < EMOJI_DATA.len() && EMOJI_DATA[lo].0 == cp {
        Some(EMOJI_DATA[lo].1)
    } else {
        None
    }
}

const TARGET_EMOJI_CATEGORIES: &[&str] = &["Smileys & Emotion", "Symbols"];

const PARTICLE_FIRST_CHARS: &[char] = &['て', 'の', 'と', 'っ', 'に', 'な', 'く', 'も', 'ほ'];

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Face,
    Emotion,
    Emoji,
    Basic,
    Sentinel,
    Linebreak,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Span {
    l: usize,
    r: usize,
    kind: Kind,
}

fn run_end(chars: &[char], mut i: usize, pred: impl Fn(char) -> bool) -> usize {
    while i < chars.len() && pred(chars[i]) {
        i += 1;
    }
    i
}

fn is_run_start(chars: &[char], i: usize, pred: impl Fn(char) -> bool) -> bool {
    i == 0 || !pred(chars[i - 1])
}

fn chars_starts_with(chars: &[char], start: usize, s: &str) -> bool {
    let mut i = start;
    for pc in s.chars() {
        match chars.get(i) {
            Some(&c) if c == pc => i += 1,
            _ => return false,
        }
    }
    true
}

fn face_marks(chars: &[char]) -> Vec<Span> {
    let n = chars.len();
    let mut out = Vec::new();
    for (i, &ch) in chars.iter().enumerate() {
        if ch != '(' && ch != '（' {
            continue;
        }
        let mut cur = i + 1;
        let mut has_symbol2 = false;
        while cur < n && is_face_symbol1(chars[cur]) {
            if is_face_symbol2(chars[cur]) {
                has_symbol2 = true;
            }
            cur += 1;
        }
        if !has_symbol2 || cur >= n {
            continue;
        }
        if chars[cur] != ')' && chars[cur] != '）' {
            continue;
        }
        let mut r = cur + 1;
        while r < n && is_face_prefix_suffix(chars[r]) {
            r += 1;
        }
        let mut l = i;
        while l > 0 && is_face_prefix_suffix(chars[l - 1]) {
            l -= 1;
        }
        out.push(Span {
            l,
            r,
            kind: Kind::Face,
        });
    }
    out
}

fn emotion_words(chars: &[char]) -> Vec<Span> {
    let n = chars.len();
    let mut out = Vec::new();
    for (i, &ch) in chars.iter().enumerate() {
        if ch != '(' && ch != '（' {
            continue;
        }
        for w in EMOTION_WORDS {
            let end_word = i + 1 + w.chars().count();
            if end_word >= n {
                continue;
            }
            if chars_starts_with(chars, i + 1, w) && matches!(chars[end_word], ')' | '）') {
                out.push(Span {
                    l: i,
                    r: end_word + 1,
                    kind: Kind::Emotion,
                });
                break;
            }
        }
    }
    out
}

fn emotion_symbols(chars: &[char]) -> Vec<Span> {
    let n = chars.len();
    let mut out = Vec::new();
    for i in 0..n {
        if !is_emotion_symbol(chars[i]) || !is_run_start(chars, i, is_emotion_symbol) {
            continue;
        }
        let j = run_end(chars, i, is_emotion_symbol);
        let r = if j < n && is_emotion_punct(chars[j]) {
            j + 1
        } else {
            j
        };
        out.push(Span {
            l: i,
            r,
            kind: Kind::Emotion,
        });
    }
    out
}

fn emoji_runs(chars: &[char]) -> Vec<Span> {
    let n = chars.len();
    let mut out = Vec::new();
    for i in 0..n {
        if emoji_entry(chars[i]).is_none() || !is_run_start(chars, i, |c| emoji_entry(c).is_some())
        {
            continue;
        }
        let j = run_end(chars, i, |c| emoji_entry(c).is_some());
        let any_target = chars[i..j]
            .iter()
            .any(|&c| matches!(emoji_entry(c), Some(Some(cat)) if TARGET_EMOJI_CATEGORIES.contains(&cat)));
        if any_target {
            out.push(Span {
                l: i,
                r: j,
                kind: Kind::Emoji,
            });
        }
    }
    out
}

fn basic_runs(chars: &[char]) -> Vec<Span> {
    let n = chars.len();
    let mut out = Vec::new();
    for i in 0..n {
        if !is_basic_punct(chars[i]) || !is_run_start(chars, i, is_basic_punct) {
            continue;
        }
        let j = run_end(chars, i, is_basic_punct);
        let k = run_end(chars, j, is_ws);
        out.push(Span {
            l: i,
            r: k,
            kind: Kind::Basic,
        });
    }
    out
}

fn linebreaks(chars: &[char]) -> Vec<(usize, usize)> {
    let n = chars.len();
    let mut out = Vec::new();
    for i in 0..n {
        if !is_ws(chars[i]) || !is_run_start(chars, i, is_ws) {
            continue;
        }
        let j = run_end(chars, i, is_ws);
        if chars[i..j].contains(&'\n') {
            out.push((i, j));
        }
    }
    out
}

fn is_exception_particle(chars: &[char], b: usize) -> bool {
    let n = chars.len();
    if b >= n {
        return false;
    }
    let mut p = b;
    while p < n && chars[p] == '\n' && p + 1 < n {
        p += 1;
    }
    let Some(&next) = chars.get(p) else {
        return false;
    };
    if !PARTICLE_FIRST_CHARS.contains(&next) {
        return false;
    }
    MORPHEME_RULES
        .iter()
        .any(|rule| rule_matches(chars, p, rule))
}

fn rule_matches(chars: &[char], mut p: usize, rule: &[&str]) -> bool {
    for part in rule {
        if !chars_starts_with(chars, p, part) {
            return false;
        }
        p += part.chars().count();
    }
    true
}

fn is_exception_numeric(chars: &[char], l: usize) -> bool {
    let n = chars.len();
    if l == 0 || l + 1 >= n {
        return false;
    }
    if chars[l] != '.' && chars[l] != '．' {
        return false;
    }
    is_numeric_char(chars[l - 1]) && is_numeric_char(chars[l + 1])
}

fn is_exception_mailaddress(chars: &[char], l: usize) -> bool {
    let n = chars.len();
    if l == 0 || l + 1 >= n {
        return false;
    }
    if chars[l] != '.' && chars[l] != '．' {
        return false;
    }
    is_mail_char(chars[l - 1]) && is_mail_char(chars[l + 1])
}

fn is_exception_no_parts(chars: &[char], l: usize, r: usize) -> bool {
    let n = chars.len();
    if !(r == l + 1 && (chars[l] == '.' || chars[l] == '．')) {
        return false;
    }
    if l < 2 || r >= n {
        return false;
    }
    is_no_n(chars[l - 2]) && is_no_o(chars[l - 1]) && chars[r].is_ascii_digit()
}

pub(crate) fn spec_boundaries(text: &str) -> Vec<usize> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let sentinel_key = if n == 0 { (0, 0) } else { (n - 1, n) };

    let mut spans: Vec<Span> = Vec::new();
    for s in basic_runs(&chars) {
        if (s.l, s.r) != sentinel_key {
            spans.push(s);
        }
    }
    for s in emoji_runs(&chars) {
        if (s.l, s.r) != sentinel_key {
            spans.push(s);
        }
    }
    let mut emotion = emotion_words(&chars);
    emotion.extend(emotion_symbols(&chars));
    emotion.sort_by_key(|s| (s.l, s.r));
    emotion.dedup_by_key(|s| (s.l, s.r));
    for s in emotion {
        if (s.l, s.r) != sentinel_key {
            spans.push(s);
        }
    }
    for s in face_marks(&chars) {
        if (s.l, s.r) != sentinel_key {
            spans.push(s);
        }
    }
    spans.push(Span {
        l: sentinel_key.0,
        r: sentinel_key.1,
        kind: Kind::Sentinel,
    });

    spans.retain(|s| !is_exception_particle(&chars, s.r));
    spans.retain(|s| !is_exception_numeric(&chars, s.l) && !is_exception_mailaddress(&chars, s.l));
    spans.retain(|s| !is_exception_no_parts(&chars, s.l, s.r));

    let lbreaks = linebreaks(&chars);
    let mut lb_by_start: HashMap<usize, (usize, usize)> =
        lbreaks.iter().map(|&(b, j)| (b, (b, j))).collect();
    let mut final_spans: Vec<Span> = Vec::with_capacity(spans.len() + lbreaks.len());
    for s in spans {
        if let Some(&(l, r)) = lb_by_start.get(&s.r) {
            lb_by_start.remove(&s.r);
            final_spans.push(Span {
                l,
                r,
                kind: Kind::Linebreak,
            });
        } else {
            final_spans.push(s);
        }
    }
    final_spans.extend(lb_by_start.into_values().map(|(l, r)| Span {
        l,
        r,
        kind: Kind::Linebreak,
    }));

    let mut bounds: Vec<usize> = final_spans.iter().map(|s| s.r).collect();
    attach_sentence_closers(&chars, &mut bounds);
    bounds.sort_unstable();
    bounds.dedup();
    bounds
}

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

fn attach_sentence_closers(chars: &[char], boundaries: &mut [usize]) {
    for end in boundaries {
        while *end < chars.len() && is_sentence_closer(chars[*end]) {
            *end += 1;
        }
    }
}

use std::collections::HashSet;
use std::process::exit;

use bunkai_rs::{segment, sentence_byte_boundaries, split_sentences};

fn corpus() -> Vec<String> {
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

    let mut rng = Rng(0x5EED_FACE);
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
    cases
}

fn parse_case_file(src: &str) -> Vec<(String, Vec<usize>)> {
    let b = src.as_bytes();
    let mut cases = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        if b[i] == b'"' {
            let (text, after) = parse_string(b, i);
            i = match try_parse_bounds_list(b, after) {
                Some((list, next)) => {
                    cases.push((text, list));
                    next
                }
                None => after,
            };
        } else {
            i += 1;
        }
    }
    cases
}

fn parse_string(b: &[u8], open: usize) -> (String, usize) {
    debug_assert_eq!(b[open], b'"');
    let mut out = String::new();
    let mut i = open + 1;
    loop {
        match b[i] {
            b'"' => return (out, i + 1),
            b'\\' => match b[i + 1] {
                b'u' => {
                    debug_assert_eq!(b[i + 2], b'{');
                    let mut j = i + 3;
                    while b[j] != b'}' {
                        j += 1;
                    }
                    let h = std::str::from_utf8(&b[i + 3..j]).unwrap();
                    out.push(char::from_u32(u32::from_str_radix(h, 16).unwrap()).unwrap());
                    i = j + 1;
                }
                b'x' => {
                    let h = std::str::from_utf8(&b[i + 2..i + 4]).unwrap();
                    out.push(char::from_u32(u32::from_str_radix(h, 16).unwrap()).unwrap());
                    i += 4;
                }
                _ => {
                    let ch = match b[i + 1] {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'0' => '\0',
                        b'\\' => '\\',
                        b'"' => '"',
                        b'\'' => '\'',
                        other => {
                            panic!("unhandled escape \\{} in corpus file", other as char)
                        }
                    };
                    out.push(ch);
                    i += 2;
                }
            },
            _ => {
                let n = utf8_len(b[i]);
                out.push_str(std::str::from_utf8(&b[i..i + n]).unwrap());
                i += n;
            }
        }
    }
}

fn try_parse_bounds_list(b: &[u8], mut i: usize) -> Option<(Vec<usize>, usize)> {
    let skip_ws = |mut i: usize| {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        i
    };
    i = skip_ws(i);
    if i >= b.len() || b[i] != b',' {
        return None;
    }
    i = skip_ws(i + 1);
    if b.get(i) == Some(&b'&') {
        i = skip_ws(i + 1);
        if b.get(i) != Some(&b'[') {
            return None;
        }
        i += 1;
    } else if b.get(i..i + 4) == Some(b"vec!") {
        i += 4;
        i = skip_ws(i);
        if b.get(i) != Some(&b'[') {
            return None;
        }
        i += 1;
    } else {
        return None;
    }
    let mut list = Vec::new();
    loop {
        i = skip_ws(i);
        if b.get(i) == Some(&b']') {
            return Some((list, i + 1));
        }
        if b.get(i).is_none_or(|&c| !c.is_ascii_digit()) {
            return None;
        }
        let mut v = 0usize;
        while b.get(i).is_some_and(|&c| c.is_ascii_digit()) {
            v = v * 10 + (b[i] - b'0') as usize;
            i += 1;
        }
        list.push(v);
        i = skip_ws(i);
        if b.get(i) == Some(&b',') {
            i += 1;
        }
    }
}

fn utf8_len(first: u8) -> usize {
    if first < 0x80 {
        1
    } else if first < 0xE0 {
        2
    } else if first < 0xF0 {
        3
    } else {
        4
    }
}

fn char_to_byte(text: &str, char_idx: usize) -> usize {
    if char_idx >= text.chars().count() {
        return text.len();
    }
    text.char_indices().nth(char_idx).unwrap().0
}

fn spec_boundaries_bytes(text: &str) -> Vec<usize> {
    spec_boundaries(text)
        .into_iter()
        .map(|b| char_to_byte(text, b))
        .collect()
}

fn check_generated() -> Result<(), String> {
    let cases = corpus();
    let mut checked = 0usize;
    for text in &cases {
        let spec = spec_boundaries(text);
        let impl_bounds = segment(text).final_boundaries;
        if spec != impl_bounds {
            return Err(format!("spec vs segment diverged on {text:?}"));
        }
        if sentence_byte_boundaries(text) != spec_boundaries_bytes(text) {
            return Err(format!("spec vs fast path diverged on {text:?}"));
        }
        let sentences = split_sentences(text);
        if sentences.concat() != *text {
            return Err(format!("losslessness broken on {text:?}"));
        }
        if !text.is_empty() && sentences.iter().any(|s| s.is_empty()) {
            return Err(format!("empty sentence produced on {text:?}"));
        }
        checked += 1;
    }
    if checked < 10_000 {
        return Err(format!("corpus unexpectedly small: {checked}"));
    }
    Ok(())
}

fn check_golden() -> Result<(), String> {
    let cases = parse_case_file(include_str!("../../tests/golden_regression.rs"));
    if cases.len() != 3_388 {
        return Err(format!("golden case count drifted: {}", cases.len()));
    }
    for (text, expected) in &cases {
        let chars: Vec<char> = text.chars().collect();
        let mut expected = expected.clone();
        attach_sentence_closers(&chars, &mut expected);
        expected.sort_unstable();
        expected.dedup();
        if spec_boundaries(text) != expected {
            return Err(format!(
                "spec vs golden diverged on {text:?} (expected {expected:?})"
            ));
        }
    }
    Ok(())
}

fn check_differential() -> Result<(), String> {
    let cases = parse_case_file(include_str!("../../tests/differential.rs"));
    if cases.len() < 485 {
        return Err(format!("differential case count drifted: {}", cases.len()));
    }
    for (text, expected) in &cases {
        let chars: Vec<char> = text.chars().collect();
        let mut expected = expected.clone();
        attach_sentence_closers(&chars, &mut expected);
        expected.sort_unstable();
        expected.dedup();
        if spec_boundaries(text) != expected {
            return Err(format!(
                "spec vs differential diverged on {text:?} (expected {expected:?})"
            ));
        }
    }
    Ok(())
}

fn check_wellformedness() -> Result<(), String> {
    let cases = corpus();
    for text in &cases {
        let n = text.chars().count();
        let b = spec_boundaries(text);
        if !b.windows(2).all(|w| w[0] < w[1]) {
            return Err(format!("not strictly increasing: {text:?}"));
        }
        if !b.iter().all(|&x| x <= n) {
            return Err(format!("boundary out of range: {text:?}"));
        }
        if b.last() != Some(&n) {
            return Err(format!("text-end boundary missing: {text:?}"));
        }
        if n == 0 {
            if b != vec![0] {
                return Err("empty text must yield {0}".into());
            }
        } else if b.contains(&0) {
            return Err(format!("zero boundary on non-empty: {text:?}"));
        }
    }
    Ok(())
}

fn check_first_wins() -> Result<(), String> {
    let cases = corpus();
    for text in &cases {
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        let sentinel_key = if n == 0 { (0, 0) } else { (n - 1, n) };
        let mut spans = Vec::new();
        spans.extend(basic_runs(&chars));
        spans.extend(emoji_runs(&chars));
        spans.extend(emotion_words(&chars));
        spans.extend(emotion_symbols(&chars));
        spans.extend(face_marks(&chars));
        let mut by_key: HashMap<(usize, usize), Vec<Kind>> = HashMap::new();
        for s in spans {
            by_key.entry((s.l, s.r)).or_default().push(s.kind);
        }
        for ((l, r), kinds) in by_key {
            if (l, r) == sentinel_key {
                continue;
            }
            if kinds.len() != 1 {
                return Err(format!(
                    "two rules shared a key ({l},{r}): {kinds:?} in {text:?}"
                ));
            }
        }
    }
    Ok(())
}

fn check_lemma2() -> Result<(), String> {
    let cases = corpus();
    for text in &cases {
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        let sentinel_key = if n == 0 { (0, 0) } else { (n - 1, n) };

        let mut spans: Vec<Span> = Vec::new();
        for s in basic_runs(&chars) {
            if (s.l, s.r) != sentinel_key {
                spans.push(s);
            }
        }
        for s in emoji_runs(&chars) {
            if (s.l, s.r) != sentinel_key {
                spans.push(s);
            }
        }
        let mut emotion = emotion_words(&chars);
        emotion.extend(emotion_symbols(&chars));
        emotion.sort_by_key(|s| (s.l, s.r));
        emotion.dedup_by_key(|s| (s.l, s.r));
        for s in emotion {
            if (s.l, s.r) != sentinel_key {
                spans.push(s);
            }
        }
        for s in face_marks(&chars) {
            if (s.l, s.r) != sentinel_key {
                spans.push(s);
            }
        }
        spans.push(Span {
            l: sentinel_key.0,
            r: sentinel_key.1,
            kind: Kind::Sentinel,
        });
        spans.retain(|s| !is_exception_particle(&chars, s.r));
        spans.retain(|s| {
            !is_exception_numeric(&chars, s.l) && !is_exception_mailaddress(&chars, s.l)
        });
        spans.retain(|s| !is_exception_no_parts(&chars, s.l, s.r));

        let lbreaks = linebreaks(&chars);

        let mut lb: HashMap<usize, (usize, usize)> =
            lbreaks.iter().map(|&(b, j)| (b, (b, j))).collect();
        let mut sim: Vec<usize> = Vec::new();
        for s in &spans {
            if let Some(&(_, r)) = lb.get(&s.r) {
                lb.remove(&s.r);
                sim.push(r);
            } else {
                sim.push(s.r);
            }
        }
        sim.extend(lb.into_values().map(|(_, r)| r));

        let lb_starts: HashSet<usize> = lbreaks.iter().map(|&(b, _)| b).collect();
        let mut l4_ends: Vec<usize> = spans.iter().map(|s| s.r).collect();
        l4_ends.sort_unstable();
        l4_ends.dedup();
        let mut formula: Vec<usize> = Vec::new();
        for &(_, j) in &lbreaks {
            formula.push(j);
        }
        for &b in &l4_ends {
            let cnt = spans.iter().filter(|s| s.r == b).count();
            if cnt >= 2 || !lb_starts.contains(&b) {
                formula.push(b);
            }
        }
        attach_sentence_closers(&chars, &mut formula);
        formula.sort_unstable();
        formula.dedup();

        let mut sim2 = sim;
        attach_sentence_closers(&chars, &mut sim2);
        sim2.sort_unstable();
        sim2.dedup();
        if sim2 != formula {
            return Err(format!("Lemma 2 formula failed on {text:?}"));
        }
        if sim2 != spec_boundaries(text) {
            return Err(format!("replacement vs pipeline on {text:?}"));
        }
    }
    Ok(())
}

struct Check {
    name: &'static str,
    run: fn() -> Result<(), String>,
}

fn all_checks() -> Vec<Check> {
    vec![
        Check {
            name: "T4 model==pipeline==fastpath (generated 11,550)",
            run: check_generated,
        },
        Check {
            name: "T5 model==golden (3,388)",
            run: check_golden,
        },
        Check {
            name: "T5 model==differential (485)",
            run: check_differential,
        },
        Check {
            name: "T1 well-formedness (generated)",
            run: check_wellformedness,
        },
        Check {
            name: "T6 first-wins priority + Lemma 1",
            run: check_first_wins,
        },
        Check {
            name: "Lemma 2 linebreak formula",
            run: check_lemma2,
        },
    ]
}

fn main() {
    let mut failed = 0usize;
    for c in all_checks() {
        match (c.run)() {
            Ok(()) => println!("[ ok ] {}", c.name),
            Err(e) => {
                eprintln!("[FAIL] {}:\n  {e}", c.name);
                failed += 1;
            }
        }
    }
    if failed == 0 {
        println!("spec_model verification: all checks passed");
    } else {
        eprintln!("{failed} check(s) failed");
        exit(1);
    }
}
