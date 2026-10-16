# bunkai_rs

A pure-Rust port of Python `bunkai` for **rule-based Japanese sentence-boundary disambiguation**. No tokenizer.

## Usage

```rust,ignore
use bunkai_rs::{segment, split_sentences};
let sentences = split_sentences("宿を予約しました😄まだ2ヶ月も先だけど。");
```

- `split_sentences`: returns `Vec<&str>` substrings. Fast path.
- `segment`: full pipeline (`Layer` + `final_boundaries`); char offsets.

## API

- `segment(text) -> Segmentation<'_>`: Returns the full [`Segmentation`] result, including every intermediate pipeline layer and the final list of character-offset boundaries. The returned value borrows from `text` (each `split_value` is a slice of it), so `text` must outlive the result — no per-span heap allocation occurs.
- `split_sentences(text) -> Vec<&str>`: Returns the sentences of `text` as substrings of it. Boundaries come from the boundaries-only computation that `sentence_byte_boundaries` exposes, so no annotation layers or spans are built; use `segment` when the per-rule layers are needed. Empty input yields no sentences; text with no boundary yields the whole text as one sentence.
- `sentence_byte_boundaries(text) -> Vec<usize>`: Compute the final sentence-boundary offsets as *byte* offsets, without materialising any annotation layer, any `Vec<char>` or the `char→byte` map — the fast path behind [`split_sentences`]. Every offset is on a char boundary (spans are only emitted at char starts), so the caller can slice `text` at each one directly.
- `Segmentation { layers, final_boundaries }`: The full result of segmenting a piece of text. `layers` records every intermediate pipeline stage; `final_boundaries` gives the unique character offsets of the sentence ends. Closing brackets/quotes right after a final-layer cut are attached to the preceding sentence, so a boundary can sit past the final layer's span end (`「あ。」次`: final-layer span ends `[3, 5]`, `final_boundaries` `[4, 5]`). This value borrows from the input text, which must therefore outlive it.
- `Layer { name, spans }`: One stage in the segmentation pipeline, named after the rule that produced it.
- `Span { rule_name, start, end, split_type, split_value }`: A single annotated span produced by one of the pipeline rules. All string fields are borrowed and cost no per-span allocation: `rule_name`/`split_type` are `'static` (rule names are fixed refs), and `split_value`, when present, is a slice into the input text. Offsets are measured in Unicode scalar values (`char` indices) over the input string, not byte offsets.

## How it works

8 stages:
1. FaceMarkDetector — kaomoji
2. EmotionExpressionAnnotator — (笑), ★
3. EmojiAnnotator — emoji
4. BasicRule — 。！？
5. IndirectQuoteExceptionAnnotator — `? と`
6. DotExceptionAnnotator — 3.5, emails
7. NumberExceptionAnnotator — No.1
8. LinebreakForceAnnotator — newlines are boundaries

## Verification

`src/bin/spec_model.rs` verifies model ≡ pipeline ≡ fast path ≡ Python ref over 15k strings.

## Performance

`cargo bench`: `segment` ~26µs, `split_sentences` ~18µs (7KB). ~3.5–5.8× faster optimized; ~13–26× vs fast-bunkai, ~5,100× vs Python.

## License

Apache License 2.0.
