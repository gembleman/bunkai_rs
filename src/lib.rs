mod core;
mod emoji_data;
// Executable transcription of docs/spec.md, used to verify the pipeline
// against the formal specification (test-only; see docs/spec.md §8).
#[cfg(test)]
mod spec_model;

pub use core::{Layer, Segmentation, Span, segment, split_sentences};
