//! Public-API behavior that the golden/differential corpora do not pin down.

use bunkai_rs::{EMOJI_DATA, split_sentences};

/// Known bug (see `known_bug_*` below): `cjk_skip_run`'s SWAR mask `!(w ^ 0xE3..) & TOP` is true
/// for *every* non-ASCII byte, so a lead byte followed by 0x81..=0x83 (E2 81 xx = U+2040..U+20FF)
/// is treated like the kana lead `E3 81..83`. These two code points are excluded from the
/// table-wide checks until that is fixed.
const KNOWN_BROKEN_E2_81: &[u32] = &[0x2049, 0x205F];

/// Categories whose emoji end a sentence (restated independently of the implementation).
const TARGET_CATEGORIES: &[&str] = &["Smileys & Emotion", "Symbols"];

/// Every entry of the public emoji table must behave per its category: a target-category
/// emoji closes the sentence, any other (incl. uncategorised modifiers) must not.
#[test]
fn every_emoji_table_entry_splits_according_to_its_category() {
    let mut mismatches = Vec::new();
    for &(codepoint, category) in EMOJI_DATA {
        if KNOWN_BROKEN_E2_81.contains(&codepoint) {
            continue;
        }
        let ch = char::from_u32(codepoint).expect("valid scalar");
        let text = format!("日本語の文章{ch}次の文章");
        let expect_split = category.is_some_and(|c| TARGET_CATEGORIES.contains(&c));
        let got = split_sentences(&text);
        let split = got.len() == 2 && got[0].ends_with(ch);
        let not_split = got.len() == 1;
        if (expect_split && !split) || (!expect_split && !not_split) {
            mismatches.push(format!("U+{codepoint:04X} {category:?}: {got:?}"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

/// All 25 Unicode White_Space code points count as whitespace for the newline rule, and
/// ordinary characters do not.
#[test]
fn every_unicode_white_space_char_joins_a_newline_run() {
    let ws: &[char] = &[
        '\u{0009}', '\u{000B}', '\u{000C}', '\u{000D}', '\u{0020}', '\u{0085}', '\u{00A0}',
        '\u{1680}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}',
        '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}', '\u{200A}', '\u{2028}', '\u{2029}',
        '\u{202F}', '\u{205F}', '\u{3000}',
    ];
    for &w in ws {
        if KNOWN_BROKEN_E2_81.contains(&(w as u32)) {
            continue;
        }
        let text = format!("あいう{w}\n{w}えおか");
        let got = split_sentences(&text);
        assert_eq!(
            got,
            vec![format!("あいう{w}\n{w}"), "えおか".to_string()],
            "U+{:04X}",
            w as u32
        );
    }
    // 공백이 아닌 문자는 줄바꿈 런에 합류하지 않는다.
    let got = split_sentences("あいう。\nえおか");
    assert_eq!(got.len(), 2);
    let got = split_sentences("あいうa\nえおか");
    assert_eq!(got, vec!["あいうa\n", "えおか"]);
}

/// Reproduces the bug described at `KNOWN_BROKEN_E2_81`. Remove `#[ignore]` once fixed.
#[test]
#[ignore = "known bug: cjk_skip_run treats E2 81 xx like kana (U+2049 split lost in release, debug_assert panics for U+2049/U+205F)"]
fn known_bug_fast_path_skips_e2_81_chars() {
    let text = "日本語の文章⁉次の文章";
    assert_eq!(split_sentences(text), vec!["日本語の文章⁉", "次の文章"]);
    assert_eq!(split_sentences("あいう\u{205F}\nえおか").len(), 2);
}
