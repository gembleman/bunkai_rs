# bunkai_rs

Rule-based **Japanese sentence-boundary disambiguation**, ported from the
Python library [bunkai](https://github.com/megagonlabs/bunkai) to pure Rust —
with no Japanese-tokenizer dependency.

Given Japanese text, it decides where one sentence ends and the next begins,
even in tricky spots:

| Input | Sentences |
| --- | --- |
| `宿を予約しました😄まだ2ヶ月も先だけど。早すぎかな(笑)楽しみです★` | `宿を予約しました😄` / `まだ2ヶ月も先だけど。` / `早すぎかな(笑)` / `楽しみです★` |
| `この値段で、こんな夕飯いいの？\nって、くらいおいしかった！` | `この値段で、こんな夕飯いいの？\n` / `って、くらいおいしかった！` |
| `スタッフ? と話し込み。` | `スタッフ? と話し込み。` (one sentence: `? と` is an indirect quote) |
| `おすすめ度No.1の和室3.5畳はあります。` | `おすすめ度No.1の和室3.5畳はあります。` (`No.` and `3.5` are not boundaries) |

## Usage

### As a library

```rust
use bunkai_rs::{segment, split_sentences};

// Get the sentences themselves (UTF-8 substrings, safe to slice).
let sentences = split_sentences("宿を予約しました😄まだ2ヶ月も先だけど。");
assert_eq!(sentences, vec!["宿を予約しました😄", "まだ2ヶ月も先だけど。"]);

// Or inspect the full pipeline result: every rule layer + final boundaries.
let seg = segment("宿を予約しました😄まだ");
// `final_boundaries` is a sorted list of character offsets.
assert_eq!(seg.final_boundaries, vec![9, 11]);
// `seg.layers` names every annotator stage (FaceMarkDetector, EmojiAnnotator,
// BasicRule, IndirectQuoteExceptionAnnotator, …) so you can debug what split
// each sentence on.
```

> **Note on offsets.** `Segmentation.final_boundaries` (and each `Span.start` /
> `Span.end`) are measured in **Unicode scalar values** (`char` indices), not
> bytes — this mirrors the Python reference's `find_eos`, whose indices are
> code-point indices. Use [`split_sentences`] when you want substrings instead:
> it converts the boundaries back to byte offsets for you. To convert manually,
> walk `text.char_indices()`.

### As a CLI

Reads UTF-8 text from stdin, prints one sentence per line:

```console
$ printf '宿を予約しました😄楽しみです★' | bunkai_rs
宿を予約しました😄
楽しみです★
```

Useful options:

| Option | Effect |
| --- | --- |
| `-b, --boundaries` | Print the final char-offset boundaries, e.g. `7,13` |
| `-l, --layers` | Print every pipeline layer (name + spans) for debugging |
| `-h, --help` / `-V, --version` | Help / version |

### As a Python extension (`--features python`)

`cargo build --release --features python` produces a CPython extension that
mirrors the Python reference's API:

```python
import _bunkai_rs as bunkai

bunkai.split("宿を予約しました😄楽しみです★")
# ['宿を予約しました😄', '楽しみです★']
bunkai.segment("うーん🤔🤔🤔どうしよう")   # == Bunkai().find_eos(...)
# [6, 11]
```

See [`src/python.rs`](src/python.rs) for build details (maturin / manual).

## How it works

`bunkai_rs` is a direct port of bunkai's `BunkaiSentenceBoundaryDisambiguation`
pipeline. Each annotator produces a layer of candidate sentence-boundary spans;
later rules refine earlier ones. The pipeline order is:

1. [`FaceMarkDetector`] — finds kaomoji such as `(^_^)`, `(*´∀｀*)`, `＼(^o^)／`
   and treats them as valid sentence endings.
2. [`EmotionExpressionAnnotator`] — `(笑)`, `(泣)`, `…`, `★`, `☆`, `♪`, …
3. [`EmojiAnnotator`] — emoji in the `Smileys & Emotion` and `Symbols`
   categories end a sentence (`😄`, `👍`, … but not `🍺`).
4. [`BasicRule`] — the naive punctuators: `。!？．!?…` plus trailing space.
5. [`IndirectQuoteExceptionAnnotator`] — suppresses a boundary when it is
   followed by a quotation particle such as `と`, `て`, `の`, `って`, `に`,
   `など`, `という` (handles the classic `…? と話し込み。`).
6. [`DotExceptionAnnotator`] — keeps `3.5`, `四.五`, and email addresses
   intact.
7. [`NumberExceptionAnnotator`] — keeps `No.1` intact.
8. [`LinebreakForceAnnotator`] — a real newline is always a boundary.

A sentinel `first` layer always ends the text, so `final_boundaries` always
contains the length of the input.

## Trade-offs vs. the original Python

bunkai uses [Janome](https://github.com/mecab/ja/Janome) to morphologically
tokenize the text so that `IndirectQuoteExceptionAnnotator` can look at the
*token* after each candidate boundary. `bunkai_rs` deliberately drops the
tokenizer (and its model data) and instead matches the quoted particles
directly on the raw character stream. The pipeline output matches the Python
reference on all of the upstream test cases; the only divergence found by the
differential harness is when a particle is part of a longer morpheme that
Janome would split differently, e.g.

* `え?とういう質問。` — Janome tokenizes `と` + `ういう` as the single word
  `とう`, so the reference *splits* here; `bunkai_rs` sees the literal `と`
  and keeps it as one sentence.

Most of the time this is actually the more conservative (split-less) choice,
which is what the indirect-quote rule is for. If you need bit-for-bit parity
with Janome, use the Python original.

## Testing

```console
$ cargo test
```

The suite includes:

* `tests/bunkai_reference.rs` — hand-written cases from the upstream test
  files, plus a corpus verified against a live Python reference.
* `tests/differential.rs` — **auto-generated** 485-case differential test. It
  records the `final_boundaries` of the *live* Python `bunkai` for a systematic
  corpus. Regenerate it with:
  ```console
  $ VIRTUAL_ENV=refenv python3 scripts/generate_differential.py
  ```
  (requires a `bunkai`-capable Python venv; see the script header).

## License

Apache License 2.0 (matching the upstream [`bunkai`](https://github.com/megagonlabs/bunkai)
project). The bundled `src/emoji_data.rs` is auto-generated from the Unicode emoji
data used by the Python `emoji`/`emojis` packages.
