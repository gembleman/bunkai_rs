use bunkai_rs::{segment, sentence_byte_boundaries, split_sentences};

#[test]
fn byte_skip_boundary_safety() {
    const KANA: &[&str] = &[
        "日本語の文章",
        "かなかな",
        "漢字かな",
        "カタカナ文章",
        "のですます",
        "あいうえお",
        "たちつてと",
        "さしすせそ",
        "たちつてとなにぬねの",
    ];
    const DANGER: &[&str] = &[
        "。", "　", "〰", "！", "？", "．", "⚗", "⚙", "😀", "\n", " ", ".", "!", "?",
    ];
    let mut cases: Vec<String> = Vec::new();
    for k in KANA {
        for d in DANGER {
            cases.push(format!("{k}{d}{k}"));
            cases.push(format!("{d}{k}"));
            cases.push(format!("{k}{d}"));
            cases.push(format!("{k}\n{k}"));
            cases.push(format!("{k}{d}\n{k}"));
            cases.push(format!("{k}\n{d}{k}"));
        }
        cases.push(format!("{k}{k}{k}{k}"));
    }
    for a in KANA {
        for b in KANA {
            cases.push(format!("{a}{b}"));
        }
    }
    cases.push("⚗漢字⚙かな😀です。".into());
    cases.push("漢字⚗⚙かな😀です。".into());
    cases.push("かな⚗漢字⚙かな😀です？".into());
    for danger in ["!", "(", ".", "?", " ", "\t", "\n", "😀", "。"] {
        cases.push(format!("abcdefgh{danger}ijklmnop"));
        cases.push(format!("01234567{danger}89ABCDEF"));
    }
    cases.push("abcdefghijklmnopqrstuvwxyz0123456789".into());
    for text in &cases {
        let via_bounds = sentence_byte_boundaries(text);
        let via_pipeline = segment(text).final_boundaries;
        let mut via_pipeline_bytes: Vec<usize> = Vec::with_capacity(via_pipeline.len());
        let mut bi = 0usize;
        for (char_idx, (byte, _)) in text.char_indices().enumerate() {
            if bi < via_pipeline.len() && char_idx == via_pipeline[bi] {
                via_pipeline_bytes.push(byte);
                bi += 1;
            }
        }
        if bi < via_pipeline.len() {
            debug_assert_eq!(via_pipeline[bi], text.chars().count());
            via_pipeline_bytes.push(text.len());
        }
        assert_eq!(
            via_bounds, via_pipeline_bytes,
            "byte-skip diverged on {text:?}"
        );
    }
}

#[test]
fn bounds_only_path_matches_full_pipeline() {
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
        let via_bounds = sentence_byte_boundaries(text);
        let mut via_layers_bytes: Vec<usize> = Vec::with_capacity(via_layers.len());
        let mut bi = 0usize;
        for (char_idx, (byte, _)) in text.char_indices().enumerate() {
            if bi < via_layers.len() && char_idx == via_layers[bi] {
                via_layers_bytes.push(byte);
                bi += 1;
            }
        }
        if bi < via_layers.len() {
            debug_assert_eq!(via_layers[bi], text.chars().count());
            via_layers_bytes.push(text.len());
        }
        assert_eq!(
            via_bounds, via_layers_bytes,
            "case #{i} bounds-only diverges from full pipeline: {text:?}"
        );
        let sentences = split_sentences(text);
        let mut cum_bytes = 0usize;
        for (k, s) in sentences.iter().enumerate() {
            cum_bytes += s.len();
            if k + 1 < sentences.len() {
                assert_eq!(
                    cum_bytes, via_bounds[k],
                    "split_sentences cut misaligned at case #{i}: {text:?}"
                );
            }
        }
        assert_eq!(
            cum_bytes,
            text.len(),
            "split_sentences lost/duplicated chars at case #{i}: {text:?}"
        );
    }
}

#[test]
fn boundary_fast_path_handles_single_sentence_and_empty_inputs() {
    assert_eq!(
        sentence_byte_boundaries("plain ASCII without a terminator").len(),
        1
    );
    assert_eq!(
        sentence_byte_boundaries("小数は3.5です"),
        vec!["小数は3.5です".len()]
    );
    assert_eq!(sentence_byte_boundaries("plain"), vec![5]);
    assert_eq!(split_sentences("plain"), vec!["plain"]);
    assert!(split_sentences("").is_empty());
}
