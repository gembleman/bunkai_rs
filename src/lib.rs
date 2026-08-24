mod core;
mod emoji_data;

pub use core::{Layer, Segmentation, Span, segment, sentence_byte_boundaries, split_sentences};
pub use emoji_data::EMOJI_DATA;
