use bunkai_rs::{EMOJI_DATA, segment, split_sentences};

const TARGET_CATEGORIES: &[&str] = &["Smileys & Emotion", "Symbols"];

#[test]
fn every_emoji_table_entry_splits_according_to_its_category() {
    let mut mismatches = Vec::new();
    for &(codepoint, category) in EMOJI_DATA {
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

#[test]
fn every_unicode_white_space_char_joins_a_newline_run() {
    let ws: &[char] = &[
        '\u{0009}', '\u{000B}', '\u{000C}', '\u{000D}', '\u{0020}', '\u{0085}', '\u{00A0}',
        '\u{1680}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}',
        '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}', '\u{200A}', '\u{2028}', '\u{2029}',
        '\u{202F}', '\u{205F}', '\u{3000}',
    ];
    for &w in ws {
        let text = format!("あいう{w}\n{w}えおか");
        let got = split_sentences(&text);
        assert_eq!(
            got,
            vec![format!("あいう{w}\n{w}"), "えおか".to_string()],
            "U+{:04X}",
            w as u32
        );
    }
    let got = split_sentences("あいう。\nえおか");
    assert_eq!(got.len(), 2);
    let got = split_sentences("あいうa\nえおか");
    assert_eq!(got, vec!["あいうa\n", "えおか"]);
}

#[test]
fn known_bug_fast_path_skips_e2_81_chars() {
    let text = "日本語の文章⁉次の文章";
    assert_eq!(split_sentences(text), vec!["日本語の文章⁉", "次の文章"]);
    assert_eq!(split_sentences("あいう\u{205F}\nえおか").len(), 2);
}

#[test]
fn general_punctuation_block_fast_path_matches_segment() {
    for cp in 0x2000u32..=0x206F {
        let Some(ch) = char::from_u32(cp) else {
            continue;
        };
        for text in [
            format!("日本語の文章{ch}次の文章"),
            format!("{ch}日本語の文章"),
            format!("日本語の文章{ch}"),
            format!("あいう{ch}\nえおか"),
            format!("かなかなかなかな{ch}{ch}漢字漢字漢字"),
            format!("漢字漢字漢字漢字漢字{ch}。次"),
        ] {
            let chars: Vec<char> = text.chars().collect();
            let mut expected: Vec<String> = Vec::new();
            let mut prev = 0usize;
            let bounds = segment(&text).final_boundaries;
            for &b in bounds.iter().chain(std::iter::once(&chars.len())) {
                if b > prev {
                    expected.push(chars[prev..b].iter().collect());
                    prev = b;
                }
            }
            assert_eq!(split_sentences(&text), expected, "U+{cp:04X} in {text:?}");
        }
    }
}

#[test]
fn segment_reports_final_boundaries_and_last_layer() {
    let seg = segment("これは一文目。これは二文目。");
    assert_eq!(seg.final_boundaries, vec![7, 14]);
    assert_eq!(seg.layers.last().unwrap().name, "LinebreakForceAnnotator");
}

#[test]
fn split_sentences_returns_each_sentence_as_a_substring() {
    let sentences = split_sentences("これは一文目。これは二文目。");
    assert_eq!(sentences, vec!["これは一文目。", "これは二文目。"]);
}
