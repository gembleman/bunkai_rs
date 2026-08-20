# bunkai_rs

A pure-Rust port of Python `bunkai` for **rule-based Japanese sentence-boundary disambiguation**. No tokenizer.

## Usage

```rust
use bunkai_rs::{segment, split_sentences};
let sentences = split_sentences("宿を予約しました😄まだ2ヶ月も先だけど。");
```

- `split_sentences`: returns `Vec<&str>` substrings. Fast path.
- `segment`: full pipeline (`Layer` + `final_boundaries`); char offsets.

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
