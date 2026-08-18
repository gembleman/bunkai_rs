#!/usr/bin/env python3
"""Regenerate tests/golden_regression.rs.

The golden file records the `final_boundaries` produced by the *original*
(pre-optimisation) bunkai_rs for a large, deterministic pseudo-random corpus,
so the optimised implementation can be proven byte-for-byte equivalent.

It works by materialising the original sources (`git show HEAD:src/...`) into a
scratch Cargo crate, building the generator binary, running it, and writing its
stdout (a self-contained Rust test) to tests/golden_regression.rs.

Requires: `cargo` and `rustc` on PATH, and this repo to be a git checkout whose
HEAD contains the pre-optimisation sources.

Usage:
    python3 scripts/generate_golden.py
"""

import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

GEN_SRC = r'''
//! Generate a golden regression corpus using the ORIGINAL bunkai_rs
//! implementation (regex-based, HashMap emoji, owned Strings). Emits a
//! self-contained Rust test that the optimised crate can compile and run.

use bunkai_orig::segment;

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

const BITS: &[&str] = &[
    "あいうえおかきくけこさしすせそたちつてとなにぬねの",
    "はひふへほまみむめもやゆよらりるれろわをんがぎぐげご",
    "アイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホ",
    "日本語文章文字境界判定のための手順検証を行う",
    "平成生年月日株式会社東京都中央区内の説明",
    "abcXYZ0123456789abcdef例外的なラテンつき",
    "。", "！", "？", "．", "!", "?", "。。", "!?", "なのかな？", "です。",
    "これで終わり。", "あれ？", "本当にいいの！", "まさか…",
    "(笑)", "(泣)", "(汗)", "…", "★", "☆", "♪", "…!",
    "(^_^)", "(*´∀｀*)", "＼(^o^)／", "(^-^)", "(。・ω・。)", "(^!^)", "(^^)", "(｀・ω・´)",
    "😀", "😄", "👍", "🎉", "❤️", "😀😀😀", "😄😄",
    "🍺", "🍣", "🚗", "☕", "🍺🍺",
    "👍🏽", "👍\u{1F3FB}", "👩\u{1F3FC}",
    "3.5", "3．5", "No.1", "no.5", "4.5点", "email@example.comは。。", "0.5畳", "１．２",
    "? と", "？って", "。という", "？という", "。て", "？に", "。など", "? くらいでした",
    "？ もあり", "。ほどでし",
    "\n", "\n\n", " \n ", "\n\u{3000}\n", "\u{3000}\n", "\u{00A0}", "\t", "\r", "\u{3000}",
    "(", ")", "（", "）", "「」", "「ありがとうございます。」",
    "おすすめ度No.1の和室3.5畳はあります。", "スタッフ? と話し込み。",
    "この値段で、こんな夕飯いいの？\nって、くらいおいしかった！",
    "宿を予約しました😄まだ2ヶ月も先だけど。",
];

fn main() {
    let n_random = std::env::var("GOLDEN_RANDOM").ok().and_then(|v| v.parse().ok()).unwrap_or(3000);
    let mut rng = Rng(0xC0FFEE12345);
    let mut cases: Vec<String> = Vec::new();
    for _ in 0..n_random {
        let mut s = String::new();
        let n = rng.below(12) + 1;
        for _ in 0..n {
            let frag = BITS[rng.below(BITS.len())];
            if rng.below(8) == 0 {
                s.push_str(frag);
                s.push_str(frag);
            } else {
                s.push_str(frag);
            }
        }
        cases.push(s);
    }
    for _ in 0..300 {
        let k = rng.below(4);
        cases.push(match k {
            0 => String::new(),
            1 => BITS[rng.below(BITS.len())].to_string(),
            2 => "。".to_string(),
            _ => "\n".to_string(),
        });
    }
    for frag in BITS {
        cases.push(frag.to_string());
    }

    let mut out = String::new();
    out.push_str("// AUTO-GENERATED golden regression vs the original implementation.\n");
    out.push_str("// Regenerate with scripts/generate_golden.py (see repo root). Do not edit.\n");
    out.push_str("use bunkai_rs::segment;\n\n#[test]\nfn golden_regression_matches_original() {\n    let cases: &[(&str, &[usize])] = &[\n");
    for c in &cases {
        let seg = segment(c);
        out.push_str(&format!("        ({:?}, &{:?}),\n", c, seg.final_boundaries));
    }
    out.push_str("    ];\n    for (text, expected) in cases {\n        let seg = segment(text);\n        assert_eq!(&seg.final_boundaries, expected, \"text: {text:?}\");\n    }\n}\n");
    print!("{out}");
}
'''

CARGO_TOML = """\
[package]
name = "bunkai_orig"
version = "0.1.0"
edition = "2021"
[dependencies]
once_cell = "1.21"
regex = "1.11"
[[bin]]
name = "gen_golden"
path = "src/bin/gen_golden.rs"
"""


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="bunkai_golden_") as td:
        tmp = Path(td)
        (tmp / "src" / "bin").mkdir(parents=True)
        (tmp / "Cargo.toml").write_text(CARGO_TOML)
        for name in ("lib.rs", "core.rs", "emoji_data.rs"):
            src = subprocess.run(
                ["git", "show", f"HEAD:src/{name}"],
                cwd=ROOT,
                check=True,
                capture_output=True,
                text=True,
            ).stdout
            (tmp / "src" / name).write_text(src)
        (tmp / "src" / "bin" / "gen_golden.rs").write_text(GEN_SRC)

        subprocess.run(
            ["cargo", "run", "--release", "--bin", "gen_golden"],
            cwd=tmp,
            check=True,
            capture_output=True,
            timeout=600,
        )
        gen = subprocess.run(
            [str(tmp / "target" / "release" / "gen_golden")],
            cwd=tmp,
            check=True,
            capture_output=True,
            text=True,
            timeout=600,
        )
        (ROOT / "tests" / "golden_regression.rs").write_text(gen.stdout)
        print(f"wrote tests/golden_regression.rs with {gen.stdout.count('("')} cases")


if __name__ == "__main__":
    main()
