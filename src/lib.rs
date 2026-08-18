mod core;
mod emoji_data;

pub use core::{Layer, Segmentation, Span, segment, split_sentences};

#[cfg(feature = "python")]
mod python;
