//! Benchmark the bunkai_rs sentence-boundary disambiguation pipeline.
//!
//! Run with:
//!   cargo bench                      # everything
//!   cargo bench --bench segment      # just this file
//!
//! The `bench` Cargo profile inherits from `[profile.release]` in Cargo.toml,
//! so LTO + codegen-units=1 apply to the measured build. Three text sizes are
//! measured for both public entry points:
//!
//! * `short`  — a single sentence (~30 bytes), e.g. a chat message or a tweet.
//! * `medium` — ~7 KB, a typical article or chat log.
//! * `long`   — ~56 KB, a long document or batch of posts.

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;

use bunkai_rs::{segment, split_sentences};

/// A few KB of realistic Japanese text mixing everything the pipeline has to
/// handle: plain periods, question/exclamation marks, emoji, kaomoji, emotion
/// expressions, decimal numbers, "No." ordinals, indirect quotes and line
/// breaks.
fn medium_text() -> String {
    const SENTENCES: &[&str] = &[
        "宿を予約しました😄まだ2ヶ月も先だけど。早すぎかな(笑)楽しみです★",
        "この値段で、こんな夕飯いいの？\nって、くらいおいしかった！",
        "スタッフ? と話し込み。",
        "おすすめ度No.1の和室3.5畳はあります。",
        "これは入力の入力サンプルです(^o^)絵文字も認識します😀引用文も大丈夫？と思いませんか？👍",
        "まずは一文目(^!^)つぎに二文目(^^)これ、テスト文なんですけど(笑)本当?にこんなテキストでいいのかな☆",
        "10秒で考えて書いたよ.",
        "ビール🍺のみたい。️Frankfurtの🍺はKrombacher",
        "「です。ます。」形式の文。",
        "感情(汗)が止まらない。次(泣)に進む。",
        "これは\n改行文。次の文。",
        "全角ドット．のテスト。",
        "本商品はおすすめ度No.1です！という売り文句の新商品が出ている。しかし、本当に信用できるのだろうか？私はとても懐疑的である。",
        "30代の夫婦2組(男2.女2)です。",
        "うーん🤔🤔🤔どうしよう",
        "こんにちは。お元気ですか？私は元気です。今日はいい天気ですね。",
        "このパソコンの画面は、横3.5インチ縦2.3インチで、重さはNo.1軽量です。",
        "彼は来る？と聞いた。\nそれは残念だね…",
        "品揃えは良い(^_^)MFG",
        "おしまい♪また明日！",
    ];

    // ~100 sentences: realistic single-shot input (~7 KB).
    let mut out = String::with_capacity(16 * 1024);
    for s in SENTENCES.iter().cycle().take(100) {
        out.push_str(s);
        out.push('。');
    }
    out
}

fn short_text() -> String {
    "宿を予約しました😄まだ２ヶ月も先だけど。".to_owned()
}

fn long_text() -> String {
    medium_text().repeat(8) // ~56 KB
}

macro_rules! define_benches {
    ($c:ident, $size:ident, $make:expr) => {
        let text = $make;
        let bytes = text.len();
        let mut group = $c.benchmark_group(format!("{}", stringify!($size)));
        group.sample_size(50);
        group.bench_function(format!("segment/{}B", bytes), |b| {
            b.iter(|| {
                let t: &str = black_box(&text);
                black_box(segment(t))
            })
        });
        group.bench_function(format!("split_sentences/{}B", bytes), |b| {
            b.iter(|| {
                let t: &str = black_box(&text);
                black_box(split_sentences(t))
            });
        });
        group.finish();
    };
}

fn bench_all(c: &mut Criterion) {
    define_benches!(c, short, short_text());
    define_benches!(c, medium, medium_text());
    define_benches!(c, long, long_text());
}

criterion_group!(benches, bench_all);
criterion_main!(benches);
