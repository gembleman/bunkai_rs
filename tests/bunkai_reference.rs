//! Differential tests ported from the original Python `bunkai` reference suite
//! (https://github.com/megagonlabs/bunkai, `tests/bunkai_sbd/test_bukai_sbd.py`).
//!
//! They drive the public `segment` API and check that the pipeline produces the
//! exact sentence splits the Python implementation is expected to produce.
//! Where expectations could not be taken verbatim from the reference tests,
//! they were verified against a live invocation of the Python reference.

use bunkai_rs::{Segmentation, segment};

/// Split `text` into sentences according to the pipeline's final boundaries.
fn sentences(text: &str) -> Vec<String> {
    let seg = segment(text);
    boundaries_to_sentences(text, &seg)
}

fn boundaries_to_sentences(text: &str, seg: &Segmentation<'_>) -> Vec<String> {
    // `final_boundaries` holds char (Unicode scalar) offsets; convert them to
    // byte offsets so we can slice the original `&str`.
    let char_to_byte: Vec<usize> = text
        .char_indices()
        .map(|(byte, _)| byte)
        .chain(std::iter::once(text.len()))
        .collect();

    let mut out = Vec::new();
    let mut start = 0usize;
    for &end in &seg.final_boundaries {
        if end >= char_to_byte.len() {
            break;
        }
        if end < start {
            continue;
        }
        out.push(text[char_to_byte[start]..char_to_byte[end]].to_string());
        start = end;
    }
    if start < char_to_byte.len() && char_to_byte[start] < text.len() {
        out.push(text[char_to_byte[start]..].to_string());
    }
    out
}

/// The names of the rules that produced the final layer's spans.
fn final_rule_names(text: &str) -> Vec<String> {
    let seg = segment(text);
    seg.layers
        .last()
        .map(|layer| {
            let mut names: Vec<String> = layer
                .spans
                .iter()
                .map(|s| s.rule_name.to_owned())
                .filter(|n| n != "first")
                .collect();
            names.sort();
            names.dedup();
            names
        })
        .unwrap_or_default()
}

#[test]
fn test_various_inputs_matches_reference() {
    // Case 1
    {
        let text = "これが１文目です。。。。そして、これが２文目…３文目。";
        assert_eq!(
            sentences(text),
            vec![
                "これが１文目です。。。。",
                "そして、これが２文目…",
                "３文目。"
            ]
        );
    }
    // Case 2
    {
        let text = "宿を予約しました♪!まだ2ヶ月も先だけど。早すぎかな(笑)楽しみです★";
        assert_eq!(
            sentences(text),
            vec![
                "宿を予約しました♪!",
                "まだ2ヶ月も先だけど。",
                "早すぎかな(笑)",
                "楽しみです★"
            ]
        );
    }
    // Case 3: emoji
    {
        let text = "宿を予約しました😄まだ2ヶ月も先だけど😄早すぎかな(笑)楽しみです★";
        assert_eq!(
            sentences(text),
            vec![
                "宿を予約しました😄",
                "まだ2ヶ月も先だけど😄",
                "早すぎかな(笑)",
                "楽しみです★"
            ]
        );
    }
    // Case 4: repeated emoji
    {
        let text = "宿を予約しました😄😄😄まだ2ヶ月も先だけど😄😄😄早すぎかな(笑)楽しみです★";
        assert_eq!(
            sentences(text),
            vec![
                "宿を予約しました😄😄😄",
                "まだ2ヶ月も先だけど😄😄😄",
                "早すぎかな(笑)",
                "楽しみです★"
            ]
        );
    }
    // Case 5: facemark with slash-joy
    {
        let text = "宿を予約しました＼(^o^)／まだ2ヶ月も先だけど。早すぎかな(笑)楽しみです★";
        assert_eq!(
            sentences(text),
            vec![
                "宿を予約しました＼(^o^)／",
                "まだ2ヶ月も先だけど。",
                "早すぎかな(笑)",
                "楽しみです★"
            ]
        );
    }
    // Case 6: indirect quote particle after a newline
    {
        let text = "この値段で、こんな夕飯いいの？\nって、くらいおいしかった！";
        assert_eq!(
            sentences(text),
            vec![
                "この値段で、こんな夕飯いいの？\n",
                "って、くらいおいしかった！"
            ]
        );
    }
    // Case 7: bare newlines only
    {
        let text = "\n\n123\n45\n\n6\n  \n7\n";
        assert_eq!(
            sentences(text),
            vec!["\n\n", "123\n", "45\n\n", "6\n  \n", "7\n"]
        );
    }
    // Case 8: mixed facemark / emoji / indirect quote
    {
        let text = "これは入力の入力サンプルです(^o^)絵文字の文末記号も認識します😀引用文も大丈夫？と思いませんか？引用文の過剰分割を防げるんです👍";
        assert_eq!(
            sentences(text),
            vec![
                "これは入力の入力サンプルです(^o^)",
                "絵文字の文末記号も認識します😀",
                "引用文も大丈夫？と思いませんか？",
                "引用文の過剰分割を防げるんです👍",
            ]
        );
    }
    // Case 9: No.1 (number exception)
    {
        let text = "本商品はおすすめ度No.1です。";
        assert_eq!(sentences(text), vec!["本商品はおすすめ度No.1です。"]);
    }
    // Case 10: 「No.1です！」 followed by further text and indirect quote
    {
        let text = "本商品はおすすめ度No.1です！という売り文句の新商品が出ている。しかし、この商品は本当に信用できるのだろうか？私はとても懐疑的である。";
        assert_eq!(
            sentences(text),
            vec![
                "本商品はおすすめ度No.1です！という売り文句の新商品が出ている。",
                "しかし、この商品は本当に信用できるのだろうか？",
                "私はとても懐疑的である。",
            ]
        );
    }
    // Case 11: dangling "No." at the end
    {
        let text = "本商品はおすすめ度No.";
        assert_eq!(sentences(text), vec!["本商品はおすすめ度No."]);
    }
}

#[test]
fn test_various_inputs_expected_rules() {
    // Case 1: emotion + basic
    {
        let text = "これが１文目です。。。。そして、これが２文目…３文目。";
        let mut names = final_rule_names(text);
        names.sort();
        assert_eq!(names, vec!["BasicRule", "EmotionExpressionAnnotator"]);
    }
    // Case 3: emotion + emoji
    {
        let text = "宿を予約しました😄まだ2ヶ月も先だけど😄早すぎかな(笑)楽しみです★";
        let mut names = final_rule_names(text);
        names.sort();
        assert_eq!(names, vec!["EmojiAnnotator", "EmotionExpressionAnnotator"]);
    }
    // Case 5: emotion + basic + facemark
    {
        let text = "宿を予約しました＼(^o^)／まだ2ヶ月も先だけど。早すぎかな(笑)楽しみです★";
        let mut names = final_rule_names(text);
        names.sort();
        assert_eq!(
            names,
            vec![
                "BasicRule",
                "EmotionExpressionAnnotator",
                "FaceMarkDetector"
            ]
        );
    }
    // Case 6: linebreak (indirect quote suppressed the 「？」 boundary)
    {
        let text = "この値段で、こんな夕飯いいの？\nって、くらいおいしかった！";
        let mut names = final_rule_names(text);
        names.sort();
        assert_eq!(names, vec!["LinebreakForceAnnotator"]);
    }
    // Case 7: linebreak
    {
        let text = "\n\n123\n45\n\n6\n  \n7\n";
        let mut names = final_rule_names(text);
        names.sort();
        assert_eq!(names, vec!["LinebreakForceAnnotator"]);
    }
    // Case 8: basic + facemark + emoji
    {
        let text = "これは入力の入力サンプルです(^o^)絵文字の文末記号も認識します😀引用文も大丈夫？と思いませんか？引用文の過剰分割を防げるんです👍";
        let mut names = final_rule_names(text);
        names.sort();
        assert_eq!(
            names,
            vec!["BasicRule", "EmojiAnnotator", "FaceMarkDetector"]
        );
    }
    // Case 9: No.1 → only first remains
    {
        let text = "本商品はおすすめ度No.1です。";
        assert_eq!(final_rule_names(text), Vec::<String>::new());
    }
    // Case 11: dangling No. → only first
    {
        let text = "本商品はおすすめ度No.";
        assert_eq!(final_rule_names(text), Vec::<String>::new());
    }
}

#[test]
fn test_setup_sentence_variations() {
    // From setUp() of TestBunkaiSbd.test_various_inputs. None of these may
    // panic, and the concatenation of the produced sentences must round-trip.
    let texts = [
        "まずは一文目(^!^)つぎに二文目(^^)これ、テスト文なんですけど(笑)本当?にこんなテキストでいいのかな☆",
        "10秒で考えて書いたよ.",
        "(セルフドリンクサービスはすごく良かったです!種類も豊富。)",
        "おすすめ度No.1の和室3.5畳はあります。",
        "おしまい♪",
        "読点で文を分割するという場合もあります、しかし現在は対応していません。",
        "(^ ^) (^　^) 先月泊まりましたが とてもよかったです。",
        "(近日中には冷房に切り替わる予定です。\nいいですね(泣))",
        "30代の夫婦2組(男2.女2)です。",
        "この値段で、こんな夕飯いいの？\nって、くらいおいしかった！",
    ];

    for text in texts {
        let joined: String = sentences(text).join("");
        assert_eq!(joined, text, "sentences must reconstruct input: {text:?}");
        let _ = segment(text);
    }
}

#[test]
fn test_live_reference_corpus() {
    // Sentences verified against the Python reference (BunkaiSentenceBoundaryDisambiguation).
    let cases: &[(&str, Vec<&str>)] = &[
        ("うーん🤔🤔🤔どうしよう", vec!["うーん🤔🤔🤔", "どうしよう"]),
        (
            "ビール🍺のみたい。\u{FE0F}Frankfurtの🍺はKrombacher",
            vec!["ビール🍺のみたい。", "\u{FE0F}Frankfurtの🍺はKrombacher"],
        ),
        (
            "30代の夫婦2組(男2.女2)です。",
            vec!["30代の夫婦2組(男2.", "女2)です。"],
        ),
        ("スタッフ? と話し込み。", vec!["スタッフ? と話し込み。"]),
        ("和室3.5畳", vec!["和室3.5畳"]),
        ("4.5点の評価。次です。", vec!["4.5点の評価。", "次です。"]),
        ("彼は来る？と聞いた。", vec!["彼は来る？と聞いた。"]),
        (
            "「です。ます。」形式の文。",
            vec!["「です。", "ます。", "」形式の文。"],
        ),
        (
            "感情(汗)が止まらない。次(泣)に進む。",
            vec!["感情(汗)", "が止まらない。", "次(泣)に進む。"],
        ),
        (
            "おすすめ度No.1の和室3.5畳はあります。",
            vec!["おすすめ度No.1の和室3.5畳はあります。"],
        ),
    ];
    for &(text, ref expected) in cases {
        assert_eq!(sentences(text), *expected, "mismatch for {text:?}");
    }
}

#[test]
fn test_basic_rule() {
    // From tests/bunkai_sbd/annotator/test_basic_annotator.py
    assert_eq!(
        sentences("１文目。２文目！３文目？"),
        vec!["１文目。", "２文目！", "３文目？"]
    );
    assert_eq!(
        sentences("１文目.２文目．３文目。"),
        vec!["１文目.", "２文目．", "３文目。"]
    );
    // Punctuation runs and trailing whitespace belong to the leading sentence.
    assert_eq!(sentences("良い。 次。"), vec!["良い。 ", "次。"]);
}

#[test]
fn test_emotion_expression() {
    // From tests/bunkai_sbd/annotator/test_emotion_expression_annotator.py
    assert_eq!(
        sentences("１文目（笑）２文目（汗）３文目（泣）"),
        vec!["１文目（笑）", "２文目（汗）", "３文目（泣）"]
    );
    assert_eq!(
        sentences("１文目☆２文目★３文目。"),
        vec!["１文目☆", "２文目★", "３文目。"]
    );
}

#[test]
fn test_emoji_annotator() {
    // From tests/bunkai_sbd/annotator/test_emoji_annotator.py
    assert_eq!(
        sentences("うーん🤔🤔🤔どうしよう"),
        vec!["うーん🤔🤔🤔", "どうしよう"]
    );
    assert_eq!(
        sentences("ビール🍺のみたい。\u{FE0F}Frankfurtの🍺はKrombacher"),
        vec!["ビール🍺のみたい。", "\u{FE0F}Frankfurtの🍺はKrombacher"]
    );
    assert_eq!(sentences("これが文⬆️"), vec!["これが文⬆", "\u{fe0f}"]);
    assert_eq!(
        sentences("１文目😄２文目😚３文目😙４文目😄😙おわり。"),
        vec!["１文目😄", "２文目😚", "３文目😙", "４文目😄😙", "おわり。"]
    );
}

#[test]
fn test_emoji_full_pipeline_boundaries() {
    // `うーん` + 3×U+1F914 + `どうしよう` = 11 chars. 🤔 (Smileys) is a target
    // category, so the emoji run ends at char 6, and the sentinel adds char 11.
    let seg = segment("うーん🤔🤔🤔どうしよう");
    assert_eq!(seg.final_boundaries, vec![6, 11]);
    assert_eq!(seg.layers.last().unwrap().name, "LinebreakForceAnnotator");
}

#[test]
fn test_dot_exception() {
    // From tests/bunkai_sbd/annotator/test_numeric_expression_exception_annotator.py
    assert_eq!(sentences("和室3.5畳"), vec!["和室3.5畳"]);
    assert_eq!(
        sentences("1.5リットル以上のペットボトル"),
        vec!["1.5リットル以上のペットボトル"]
    );
    assert_eq!(
        sentences("四.五畳以上の大きさ"),
        vec!["四.五畳以上の大きさ"]
    );
    assert_eq!(sentences("四五畳以上の大きさ"), vec!["四五畳以上の大きさ"]);
    assert_eq!(
        sentences("15メートル以上の大きさ"),
        vec!["15メートル以上の大きさ"]
    );
}

#[test]
fn test_number_exception() {
    // From tests/bunkai_sbd/annotator/test_number_exception_annotator.py
    assert_eq!(sentences("No.1の商品"), vec!["No.1の商品"]);
    assert_eq!(
        sentences("ROOM No.411を予約しました。"),
        vec!["ROOM No.411を予約しました。"]
    );
    assert_eq!(sentences("おすすめ度No."), vec!["おすすめ度No."]);
}

#[test]
fn test_facemark_subset() {
    // A representative subset of tests/bunkai_sbd/annotator/test_facemark_detector.py.
    // Each (text, expected_end) states the end index of the only detected facemark.
    let cases: &[(&str, usize)] = &[
        ("いい湯でした(^_^)", 11),
        ("品揃えは良い(^*^*)", 12),
        ("食事はよかった(*´∀｀*)", 14),
        ("品揃えは良い(*´艸｀*)", 13),
        ("子供連れによいヽ(=´▽`=)ﾉ", 15),
        ("食事はよかった(^^;)", 12),
        ("いい湯でした(^_^)y-~~~", 11), // `y-~~~` is not suffix-class; verified live
        ("品揃えは良い(^_^)MFG", 11),
        ("いい湯でした(^_^)食事はよかった", 11),
        ("サービスは良好(&gt;_&lt;)", 18), // HTML entities `&gt;` `&lt;` are face-symbol chars
    ];

    for &(text, expected_end) in cases {
        let seg = segment(text);
        let face_marks: Vec<usize> = seg
            .layers
            .iter()
            .find(|layer| layer.name == "FaceMarkDetector")
            .map(|layer| {
                layer
                    .spans
                    .iter()
                    .filter(|span| span.rule_name == "FaceMarkDetector")
                    .map(|span| span.end)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            face_marks,
            vec![expected_end],
            "FaceMarkDetector mismatch for {text:?}"
        );
    }
}

#[test]
fn test_facemark_interior_basic_rule_punctuation() {
    // The reference lets `!` inside `(^!^)` become its own boundary, but does not
    // let inside-facemark text fragments with no punctuation split further.
    // Verified against the live Python reference.
    let text = "まずは一文目(^!^)つぎに二文目(^^)これ、テスト文なんですけど(笑)本当?にこんなテキストでいいのかな☆";
    assert_eq!(
        sentences(text),
        vec![
            "まずは一文目(^!",
            "^)",
            "つぎに二文目(^^)",
            "これ、テスト文なんですけど(笑)",
            "本当?にこんなテキストでいいのかな☆",
        ]
    );
    assert_eq!(
        sentences("品揃えは良い(^!^)MFG"),
        vec!["品揃えは良い(^!", "^)", "MFG"]
    );
}

#[test]
fn test_empty_and_short_inputs() {
    assert_eq!(segment("").final_boundaries, vec![0]);
    // The pipeline always ends on the LinebreakForceAnnotator layer.
    assert_eq!(
        segment("").layers.last().unwrap().name,
        "LinebreakForceAnnotator"
    );
    assert_eq!(segment("").layers[0].name, "first");
    assert_eq!(
        segment("。").layers.last().unwrap().name,
        "LinebreakForceAnnotator"
    );
    assert_eq!(sentences("。"), vec!["。"]);
    assert_eq!(sentences("あ"), vec!["あ"]);
    assert_eq!(sentences("\n"), vec!["\n"]);
}

#[test]
fn test_multibyte_offsets_are_char_offsets() {
    // All offsets must be in Unicode scalar values, not bytes.
    // "日本語です。" ends at char 6; "Englishです。" is 10 more chars → 16.
    let text = "日本語です。Englishです。";
    let seg = segment(text);
    assert_eq!(seg.final_boundaries, vec![6, 16]);
    assert_eq!(sentences(text), vec!["日本語です。", "Englishです。"]);
}

#[test]
fn test_segment_preserves_every_pipeline_layer_snapshot() {
    // This input exercises every annotator and each of the four exception
    // stages.  The expected snapshots are deliberately written as literals:
    // checking only the final boundaries would miss a stage mutating its
    // predecessor in place.
    let text = "★ A?て😀。3.5 No.1(^_^)\n";
    let seg = segment(text);

    assert_eq!(
        seg.layers
            .iter()
            .map(|layer| layer.name)
            .collect::<Vec<_>>(),
        vec![
            "first",
            "FaceMarkDetector",
            "EmotionExpressionAnnotator",
            "EmojiAnnotator",
            "BasicRule",
            "IndirectQuoteExceptionAnnotator",
            "DotExceptionAnnotator",
            "NumberExceptionAnnotator",
            "LinebreakForceAnnotator",
        ]
    );
    assert_eq!(seg.layers.len(), 9);

    type ExpectedSpan<'a> = (
        &'static str,
        usize,
        usize,
        Option<&'static str>,
        Option<&'a str>,
    );
    let expected: &[&[ExpectedSpan<'_>]] = &[
        &[("first", 20, 21, None, None)],
        &[
            ("FaceMarkDetector", 15, 20, Some("facemark"), Some("(^_^)")),
            ("first", 20, 21, None, None),
        ],
        &[
            (
                "EmotionExpressionAnnotator",
                0,
                1,
                Some("EmotionExpressionAnnotator"),
                Some("★"),
            ),
            ("FaceMarkDetector", 15, 20, Some("facemark"), Some("(^_^)")),
            ("first", 20, 21, None, None),
        ],
        &[
            (
                "EmotionExpressionAnnotator",
                0,
                1,
                Some("EmotionExpressionAnnotator"),
                Some("★"),
            ),
            ("EmojiAnnotator", 5, 6, Some("EmojiAnnotator"), Some("😀")),
            ("FaceMarkDetector", 15, 20, Some("facemark"), Some("(^_^)")),
            ("first", 20, 21, None, None),
        ],
        &[
            (
                "EmotionExpressionAnnotator",
                0,
                1,
                Some("EmotionExpressionAnnotator"),
                Some("★"),
            ),
            ("BasicRule", 3, 4, Some("BasicRule"), Some("?")),
            ("EmojiAnnotator", 5, 6, Some("EmojiAnnotator"), Some("😀")),
            ("BasicRule", 6, 7, Some("BasicRule"), Some("。")),
            ("BasicRule", 8, 9, Some("BasicRule"), Some(".")),
            ("BasicRule", 13, 14, Some("BasicRule"), Some(".")),
            ("FaceMarkDetector", 15, 20, Some("facemark"), Some("(^_^)")),
            ("first", 20, 21, None, None),
        ],
        &[
            (
                "EmotionExpressionAnnotator",
                0,
                1,
                Some("EmotionExpressionAnnotator"),
                Some("★"),
            ),
            ("EmojiAnnotator", 5, 6, Some("EmojiAnnotator"), Some("😀")),
            ("BasicRule", 6, 7, Some("BasicRule"), Some("。")),
            ("BasicRule", 8, 9, Some("BasicRule"), Some(".")),
            ("BasicRule", 13, 14, Some("BasicRule"), Some(".")),
            ("FaceMarkDetector", 15, 20, Some("facemark"), Some("(^_^)")),
            ("first", 20, 21, None, None),
        ],
        &[
            (
                "EmotionExpressionAnnotator",
                0,
                1,
                Some("EmotionExpressionAnnotator"),
                Some("★"),
            ),
            ("EmojiAnnotator", 5, 6, Some("EmojiAnnotator"), Some("😀")),
            ("BasicRule", 6, 7, Some("BasicRule"), Some("。")),
            ("FaceMarkDetector", 15, 20, Some("facemark"), Some("(^_^)")),
            ("first", 20, 21, None, None),
        ],
        &[
            (
                "EmotionExpressionAnnotator",
                0,
                1,
                Some("EmotionExpressionAnnotator"),
                Some("★"),
            ),
            ("EmojiAnnotator", 5, 6, Some("EmojiAnnotator"), Some("😀")),
            ("BasicRule", 6, 7, Some("BasicRule"), Some("。")),
            ("FaceMarkDetector", 15, 20, Some("facemark"), Some("(^_^)")),
            ("first", 20, 21, None, None),
        ],
        &[
            (
                "EmotionExpressionAnnotator",
                0,
                1,
                Some("EmotionExpressionAnnotator"),
                Some("★"),
            ),
            ("EmojiAnnotator", 5, 6, Some("EmojiAnnotator"), Some("😀")),
            ("BasicRule", 6, 7, Some("BasicRule"), Some("。")),
            (
                "LinebreakForceAnnotator",
                20,
                21,
                Some("linebreak"),
                Some("\n"),
            ),
            ("first", 20, 21, None, None),
        ],
    ];

    for (layer, expected_spans) in seg.layers.iter().zip(expected) {
        let actual: Vec<ExpectedSpan<'_>> = layer
            .spans
            .iter()
            .map(|span| {
                (
                    span.rule_name,
                    span.start,
                    span.end,
                    span.split_type,
                    span.split_value,
                )
            })
            .collect();
        assert_eq!(
            actual, *expected_spans,
            "snapshot mismatch in {:?}",
            layer.name
        );
    }
}
