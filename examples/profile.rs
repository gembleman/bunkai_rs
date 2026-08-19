//! Profiling harness: run the hot path repeatedly so `perf` can sample it.
//!   perf record --call-graph dwarf ./target/release/examples/profile
//!   perf report
use bunkai_rs::{segment, split_sentences};

fn corpus() -> String {
    let mut out = String::with_capacity(64 * 1024);
    for i in 0..2000 {
        out.push_str("宿を予約しました😄まだ2ヶ月も先だけど。早すぎかな(笑)楽しみです★");
        out.push_str("スタッフ? と話し込み。おすすめ度No.1の和室3.5畳はあります。");
        out.push_str("これは入力の入力サンプルです(^o^)絵文字も認識します😀👍");
        out.push_str("30代の夫婦2組(男2.女2)です。Bunkai の No.19 は 12.34 円。");
        out.push_str(&format!("{:?} 回目。\n次の文。それでは…", i % 100));
    }
    out
}

fn main() {
    let text = corpus();
    let bytes = text.len();
    let iters = 400;

    // warmup
    std::hint::black_box(segment(&text));

    let t0 = std::time::Instant::now();
    for _ in 0..iters {
        std::hint::black_box(segment(&text));
    }
    let seg = t0.elapsed();

    let t1 = std::time::Instant::now();
    for _ in 0..iters {
        std::hint::black_box(split_sentences(&text));
    }
    let sp = t1.elapsed();

    eprintln!(
        "corpus={}B iters={iters}\n  segment        = {:?} ({:?}/iter)\n  split_sentences= {:?} ({:?}/iter)",
        bytes,
        seg,
        seg / iters,
        sp,
        sp / iters
    );
}
