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

### `segment` vs `split_sentences`

Both entry points run the *exact same* rule pipeline (`FaceMarkDetector` …
`LinebreakForceAnnotator`) and therefore produce the **same sentence
boundaries**. They differ in what they return and how much work they do to get
there:

| | [`split_sentences`] | [`segment`] |
| --- | --- | --- |
| Returns | the sentence **substrings** themselves (`Vec<&str>`) | the full [`Segmentation`]: every rule [`Layer`] plus `final_boundaries` |
| Offsets | none exposed (slices are already on char boundaries) | `final_boundaries` (and each `Span.start`/`Span.end`) are *char indices*, not bytes |
| Debugging | — | each [`Span`] says which rule fired and on which substring (`rule_name`, `split_value`) |
| Cost | fast path: scans the UTF-8 bytes once, keeps only 8-byte boundary records | materialises every layer's `Span`s — noticeably slower |
| Use when | you only need the sentences | you need to inspect or log *why* it split where it did |

**Which one should I use?** For the common “split this text into sentences”
case, use `split_sentences` — it is the faster of the two and returns the
strings ready to use. Reach for `segment` when you want to debug a surprising
split (for example why `…? と話し込み。` stays one sentence), or when you need
`final_boundaries` as code-point offsets.

`split_sentences` is *not* a separate, duck-typed implementation: both paths
share the same rules, and the boundaries-only path is verified to produce the
exact same boundaries as the full pipeline (see the
`bounds_only_path_matches_full_pipeline` property test plus the golden and
differential suites). In effect [`segment`] doubles as the reference
implementation that the fast path is tested against.

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

## Formal specification & verification

The algorithm is specified mathematically in [`docs/spec.md`](docs/spec.md):
character-class predicates, every rule as a set comprehension over maximal
runs, the exception filters, and the boundary set `B(s)` — with proofs of
well-formedness, the first-wins priority lemma, the linebreak-replacement
formula and lossless segmentation. `src/bin/spec_model.rs`, an independent,
standalone binary (no shared code with `core.rs`), is a deliberately naive
transcription of *those formulas*; it can be run directly
(`cargo run --bin spec_model`) or as `#[test]`s (`cargo test --bin spec_model`).
It verifies model ≡ pipeline ≡ fast path ≡ Python-derived reference over
15,423 strings (11,550 generated + 3,388 golden + 485 differential), plus the
theorem checks.

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
* `tests/golden_regression.rs` — **auto-generated** 3,400+ golden cases. It
  records the `final_boundaries` of the *pre-optimisation* Rust implementation
  for a seeded pseudo-random corpus (plain text, emoji with/without skin-tone
  modifiers, kaomoji, decimals, `No.`, indirect-quote particles, every kind of
  whitespace incl. `U+3000`, …). Regenerate it with:
  ```console
  $ python3 scripts/generate_golden.py
  ```
  (no extra dependencies; it materialises the original sources from `git HEAD`
  into a scratch crate and runs them).
* `src/bin/spec_model.rs` — the **formal-spec verifier** (see
  [Formal specification & verification](#formal-specification--verification)).
  A standalone binary, transcribed independently of `core.rs`, that checks the
  model against the pipeline, the fast byte path and the Python-derived
  golden/differential corpora, plus the theorem checks. Run it directly with
  `cargo run --bin spec_model`, or as targeted tests with
  `cargo test --bin spec_model`.

## Benchmark

```console
$ cargo bench --bench segment
```

Uses [criterion](https://github.com/bheisler/criterion.rs) with the release
profile (LTO, `codegen-units = 1`). Results for `bunkai_rs` v0.1.0+, on the
built-in Japanese corpus (single thread):

| text size | `segment` | `split_sentences` |
| --- | ---: | ---: |
| 61 B (one sentence) | ~0.40 µs | ~0.10 µs |
| 7 KB (article) | ~26 µs | ~18 µs |
| 56 KB (long doc) | ~211 µs | ~142 µs |

*`segment` materialises every pipeline layer, so it is naturally a bit more
expensive than `split_sentences`, which runs only the boundary computation over
a compact representation — no per-layer span records, no IndirectQuote sort,
no char→byte map, no intermediate span strings, and no membership set (the
only possible (start,end) collision is the end-of-text sentinel, which a final
sort+dedup resolves). `split_sentences` also short-circuits newline-free text:
with no `\n` there is no `LinebreakForceAnnotator` work to fold in, so the
candidate ends are normalised straight into the boundary list. When newlines
*do* appear, they are merged via a binary search over the scanner's already
sorted newline runs instead of building a hash map.*

Relative to the original regex/HashMap port, the optimisation pass makes
`segment` ~5.8× faster and `split_sentences` ~3.5× faster on the 7 KB corpus.
The borrow-based `Span`/`Layer`/`Segmentation` (no per-span `String`
allocations), hand-written scanners for the two regular expressions and the
emoji-table lookup (dropping the `regex` and `once_cell` dependencies; the
emoji lookup fast-rejects the emoji-free kana ranges and the whole CJK/kanji
range before any binary search), direct construction of the unique
IndirectQuote span set (no cross-layer re-collection), a single-pass fused
scanner (generic over the sink, so the boundaries-only path emits compact
16-byte records straight out of the scan, linebreak detection included), a
lazily built char→byte map, set-free first-wins merging, a fused single-pass
exception filter, and the boundaries-only `split_sentences` fast path are the
main contributors.

## Comparison vs. fast-bunkai

[fast-bunkai](https://github.com/hotchpotch/fast-bunkai) is the other main
Rust-based reimplementation of the same bunkai pipeline: a PyO3 Rust core
wrapped by a Python API, on **this machine (AMD Ryzen 7 H 255, release
profile + LTO + `codegen-units=1`)**.

### Rust core vs. Rust core — `split_sentences`, the API real callers use

Almost every caller only needs sentence boundaries, not the full layered
`Span`/`Layer` history — so the comparison that matters is `bunkai_rs`'s
boundaries-only fast path, `split_sentences`, against fast-bunkai's *only*
entry point, `segment()` (fast-bunkai has no dedicated fast path). Measured
natively — both crates linked as plain `rlib`s in one process, single core
pinned (`taskset -c 0`), `perf stat -e cycles,instructions -r 5`+, medians
over 9 samples — on two fresh ~127 KiB corpora:

| corpus | `bunkai_rs::split_sentences` | fast-bunkai `segment()` | speedup |
| --- | ---: | ---: | ---: |
| CJK-heavy (43,664 chars: kana/kanji + Japanese punctuation only) | 7.55 cyc/char (87 µs/call) | 194.0 cyc/char (2.23 ms/call) | **~25.7×** |
| Mixed (56,885 chars: Japanese + Latin + decimals + `No.` ordinals + emoji + kaomoji + line breaks) | 57.1 cyc/char (856 µs/call) | 749.6 cyc/char (11.3 ms/call) | **~13.1×** |

The CJK-heavy gap is the widest because `bunkai_rs`'s SWAR-accelerated
kana/kanji run-skip (see [Benchmark](#benchmark) below) makes pure Japanese
prose nearly free to scan, while fast-bunkai's regex + HashMap/HashSet core
pays its per-character matching cost regardless of script. Both engines
produce **identical sentence boundaries** — verified differentially on 40,083
generated + corpus cases, 0 mismatches.

### Full stack: Python API vs. native API

`bunkai_rs` has no Python binding, so the fair end-to-end comparison is
fast-bunkai's user-facing Python API (`FastBunkai(text)` → sentences, which
builds the full segmentation in the native core and converts it to Python
objects) against `bunkai_rs`'s native `split_sentences`. The original Python
[`bunkai`](https://github.com/megagonlabs/bunkai) is included for context
(its Janome tokenizer dominates the runtime). This table uses the built-in
benchmark's corpus (61 B single sentence / 7 KB article / 56 KB long doc, all
mixed Japanese with emoji, kaomoji, decimals, `No.` ordinals, indirect quotes
and line breaks) rather than the CJK-heavy/Mixed pair above:

| text size | Python `bunkai` | fast-bunkai (Python API) | `bunkai_rs::split_sentences` | speedup vs. fast-bunkai |
| --- | ---: | ---: | ---: | ---: |
| 61 B | 0.25 ms | 8.6 µs | 0.10 µs | ~85× |
| 7 KB | 38.3 ms | 717 µs | 17.7 µs | ~40× |
| 56 KB | 722 ms | 7.6 ms | 142 µs | ~54× |

fast-bunkai's own claim of being ~26–116× faster than Python `bunkai`
reproduces on this corpus; because `bunkai_rs` skips the Python/PyO3 layer
entirely, it is another ~40–85× faster again — around **2,100–5,100× faster
than the original Python bunkai** on this corpus. The Python-API gap is
largest on short text, where fast-bunkai's per-call PyO3 dict-construction and
call overhead dominate the tiny native runtime.

### Reproducing

The `split_sentences`-vs-`segment()` numbers come from a small binary that
links both crates as plain `rlib`s (`bunkai_rs` + `fast_bunkai_native` with
`default-features = false`) in a single process, run under `perf stat`
core-pinned with `taskset -c 0` for cycle-accurate, turbo-noise-resistant
measurement (a bare wall-clock/criterion run is not trusted alone — see
`todo.md`'s optimization log for why). The Python-side numbers come from
driving the same strings through `FastBunkai` / `Bunkai` in a venv
(fast-bunkai's native module built for Linux via `maturin develop --release`).

## License

Apache License 2.0 (matching the upstream [`bunkai`](https://github.com/megagonlabs/bunkai)
project). The bundled `src/emoji_data.rs` is auto-generated from the Unicode emoji
data used by the Python `emoji`/`emojis` packages.
