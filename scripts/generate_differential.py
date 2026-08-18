#!/usr/bin/env python3
"""Regenerate `tests/differential.rs`.

Runs the live Python `bunkai` reference on a corpus and records each string's
`final_boundaries` (char offsets) as a Rust integration test. This keeps the
Rust port honest against the upstream implementation.

Setup
-----
    uv venv refenv --python 3.10
    VIRTUAL_ENV=refenv uv pip install dataclasses-json janome spans emoji emojis more_itertools toml tqdm
    git clone --depth 1 https://github.com/megagonlabs/bunkai /tmp/bunkai_ref

Usage
-----
    VIRTUAL_ENV=refenv python scripts/generate_differential.py
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
REF = Path(os.environ.get("BUNKAI_REF", "/tmp/bunkai_ref"))

# Inputs where the Rust port intentionally diverges from the Janome-based
# reference, so they must not be machine-compared. Each entry names the corpus
# generator tag that produces it; see build() below for the exact text.
KNOWN_DIVERGENT_TEXTS = {
    # Janome tokenises と + う as the single token とう, so the reference does NOT
    # apply the ["と"] indirect-quote exception here, while the dependency-free
    # string matcher in this port does. (See README, "Trade-offs".)
    "え?とういう質問。",
}


def build() -> list[str]:
    """Systematic corpus of Japanese sentence-boundary inputs."""
    out = []
    ref = [
        "これが１文目です。。。。そして、これが２文目…３文目。",
        "宿を予約しました♪!まだ2ヶ月も先だけど。早すぎかな(笑)楽しみです★",
        "宿を予約しました😄まだ2ヶ月も先だけど😄早すぎかな(笑)楽しみです★",
        "宿を予約しました😄😄😄まだ2ヶ月も先だけど😄😄😄早すぎかな(笑)楽しみです★",
        "宿を予約しました＼(^o^)／まだ2ヶ月も先だけど。早すぎかな(笑)楽しみです★",
        "この値段で、こんな夕飯いいの？\nって、くらいおいしかった！",
        "\n\n123\n45\n\n6\n  \n7\n",
        "これは入力の入力サンプルです(^o^)絵文字の文末記号も認識します😀引用文も大丈夫？と思いませんか？引用文の過剰分割を防げるんです👍",
        "本商品はおすすめ度No.1です。",
        "本商品はおすすめ度No.1です！という売り文句の新商品が出ている。しかし、この商品は本当に信用できるのだろうか？私はとても懐疑的である。",
        "本商品はおすすめ度No.",
        "まずは一文目(^!^)つぎに二文目(^^)これ、テスト文なんですけど(笑)本当?にこんなテキストでいいのかな☆",
        "10秒で考えて書いたよ.",
        "(セルフドリンクサービスはすごく良かったです!種類も豊富。)",
        "おすすめ度No.1の和室3.5畳はあります。",
        "おしまい♪",
        "読点で文を分割するという場合もあります、しかし現在は対応していません。",
        "(^ ^) (^　^) 先月泊まりましたが とてもよかったです。",
        "(近日中には冷房に切り替わる予定です。\nいいですね(泣))",
        "30代の夫婦2組(男2.女2)です。",
        "うーん🤔🤔🤔どうしよう",
        "ビール🍺のみたい。\ufe0fFrankfurtの🍺はKrombacher",
        "スタッフ? と話し込み。",
        "和室3.5畳",
        "4.5点の評価。次です。",
        "彼は来る？と聞いた。",
        "「です。ます。」形式の文。",
        "感情(汗)が止まらない。次(泣)に進む。",
        "品揃えは良い(^_^)MFG",
        "これは\n改行文。次の文。",
        "普通の文。",
        "全角ドット．のテスト。",
        "3．14。",
        "No,そうではない。",
        "笑い声(笑)大爆笑(爆)",
        "…三連ドット",
        "え?とういう質問。",
        "くらいです。くらいでした。",
        "という言い方は古い。",
        "待ってって言ったのに。",
        "…の後に何か。",
    ]
    out.extend(ref)

    prefixes = ["これはテストです", "彼は来る", "それでいい", "おすすめ", "同じ"]
    puncts = ["。", "!", "？", ".", "．", "！", "?", "。。", "!!!", "。。。", "！？"]
    for p in prefixes:
        for q in puncts:
            out.append(p + q + "次続く。")

    parts = ["て", "の", "と", "って", "という", "に", "など", "くらいの", "くらいです", "くらいでし", "もあり", "ほどでし"]
    for pt in parts:
        out.append(f"そうなの？{pt}思う。")
        out.append(f"そうなの? {pt}考える。")
        out.append(f"本当？{pt}違う。")

    for n in ["3.5", "3．5", "３．５", "四.五", "No.1", "No.411", "12.34", "1,234", "5.0"]:
        out.append(f"サイズ{n}の部屋これは。")
        out.append(f"サイズ{n}の部屋。")
        out.append(f"{n}円")

    marks = ["(^_^)", "(^^)", "(^-^)", "(^o^)", "(・_・)", "(>_<)", "(*´∀｀*)", "(笑)", "＼(^o^)／", "o(^o^)o", "T_T", "(´･_･`)"]
    for m in marks:
        for sfx in ["", "次に続く。", "!", "！", "♪", "y", "y-~~~", "V", "-☆", "A", "。"]:
            out.append("品揃えは良い" + m + sfx + "。")
            out.append("いい" + m + "です")

    for sym in ["…", "★", "☆", "♪", "★★", "☆♪", "♪☆"]:
        out.append(f"楽しみ{sym}")
        out.append(f"楽しみ{sym}次")

    for w in ["笑", "泣", "汗", "怒", "爆", "照れ"]:
        out.append(f"（{w}）大変")
        out.append(f"({w})大変。次。")

    for e in ["😄", "😀", "👍", "🤔", "🍺", "⬆️", "❤️", "😭", "🎉"]:
        out.append(f"楽しい{e}次")
        out.append(f"楽しい{e}。")

    out.append("a\nb\nc")
    out.append("文\n文")
    out.append("\n")
    out.append("")
    out.append(" ")
    out.append("\n\n")
    out.append("文中\nの改行。次")
    out.append("？\n")
    out.append("！\n\n！")

    out.append("日本語abcEnglish。次")
    out.append("a。b. c！")
    out.append("ｶﾀｶﾅとﾊﾝｶｸ。次")
    out.append("（（ネスト））括弧。")
    out.append("「」『』かぎかっこ。")
    out.append("絵文字は＿＿ここから😄＿＿。")
    out.append("長い文章の途中に\n改行が入るケース。")
    out.append("4.5Lの水。2.5億円。")
    out.append("テスト。。。。。")
    out.append("。")

    return [t for t in out if t not in KNOWN_DIVERGENT_TEXTS]


def reference_eos(corpus: list[str]) -> list[list[int]]:
    """Compute `find_eos` for every corpus string using the live reference."""
    code = f"""
import json, sys
sys.path.insert(0, {str(REF)!r})
from bunkai.algorithm.bunkai_sbd.bunkai_sbd import BunkaiSentenceBoundaryDisambiguation
corpus = json.load(sys.stdin)
splitter = BunkaiSentenceBoundaryDisambiguation(path_model=None)
out = [splitter.find_eos(t) for t in corpus]
json.dump(out, sys.stdout)
"""
    proc = subprocess.run(
        [sys.executable, "-c", code],
        input=json.dumps(corpus, ensure_ascii=False),
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        sys.exit(f"python reference failed:\n{proc.stderr}")
    return json.loads(proc.stdout)


def main() -> None:
    corpus = build()
    eos = reference_eos(corpus)
    assert len(corpus) == len(eos)

    lines = [
        "// AUTO-GENERATED differential test against the live Python bunkai reference.",
        "// Regenerate with `python3 scripts/generate_differential.py`; do not edit manually.",
        "use bunkai_rs::segment;",
        "",
        "#[test]",
        "fn differential_matches_python_reference() {",
        "    let cases: &[(&str, Vec<usize>)] = &[",
    ]
    for text, ends in zip(corpus, eos):
        esc = json.dumps(text, ensure_ascii=False)
        b = [int(i) for i in ends]
        lines.append(f"        ({esc}, vec!{b}),")
    lines += [
        "    ];",
        "    for (i, (text, expected)) in cases.iter().enumerate() {",
        "        let seg = segment(text);",
        "        assert_eq!(",
        '            seg.final_boundaries, *expected,',
        '            "case #{i} diverges from python reference: {text:?}"',
        "        );",
        "    }",
        "}",
        "",
    ]
    (REPO / "tests" / "differential.rs").write_text("\n".join(lines))
    print(f"wrote {len(corpus)} cases to tests/differential.rs")


if __name__ == "__main__":
    main()
