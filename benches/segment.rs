use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

use bunkai_rs::{segment, split_sentences};

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
    medium_text().repeat(8)
}

fn ascii_heavy_text() -> String {
    const FRAGMENT: &str = "Release 2026.08: job #12345 completed in 3.14s; retry=0, status=ok. ";
    FRAGMENT.repeat(128)
}

fn cjk_kana_text() -> String {
    const FRAGMENT: &str =
        "吾輩は猫である。名前はまだ無い。ひらがなとカタカナの文章を読みます。今日は晴れです。";
    FRAGMENT.repeat(96)
}

fn emoji_kaomoji_text() -> String {
    const FRAGMENT: &str = "今日は最高😀😄🎉(^_^)(^o^)(笑)まず乾杯🍻しよう！えっ、本当？🤔👍(>_<)";
    FRAGMENT.repeat(96)
}

fn punctuation_dense_text() -> String {
    const FRAGMENT: &str = "えっ!?本当ですか??……そうです!!! No.1 (重要); 3.14。";
    FRAGMENT.repeat(128)
}

fn low_symbol_heavy_text() -> String {
    const FRAGMENT: &str = concat!(
        "Support © 2026 Acme® — status=ok™; info ℹ️. ",
        "sync ↔ backup ↕ 75% / ready? ",
        "weather ☀ ☁ ☂ ☃; meeting ⌚ 09:30; coffee ☕. ",
        "security ⚠ ✅ ☑; call ☎ / mobile ✆; score ★ ☆. ",
        "reaction ☺ ☹ ❤ ❓ ❗; event ⚽ ♠ ✈. ",
        "low symbols 〰 〽 ㊗ ㊙; plain § ± × ÷ • °. ",
        "Latin café naïve, CJK メモと確認です。\n",
    );
    FRAGMENT.repeat(24)
}

fn no_boundary_text() -> String {
    const FRAGMENT: &str =
        "this is one deliberately long sentence with numbers 12345 and words only ";
    FRAGMENT.repeat(160)
}

fn newline_sparse_text() -> String {
    const LINE: &str = "ログの一行目です。次の処理を続けます。成功しました。";
    let mut text = String::with_capacity(LINE.len() * 96 + 2);
    for i in 0..96 {
        text.push_str(LINE);
        if i == 47 || i == 95 {
            text.push('\n');
        }
    }
    text
}

fn newline_dense_text() -> String {
    const LINE: &str = "短い行です。";
    let mut text = String::with_capacity(LINE.len() * 96);
    for _ in 0..96 {
        text.push_str(LINE);
        text.push('\n');
    }
    text
}

fn bench_case(c: &mut Criterion, name: &str, text: &str) {
    let bytes = text.len();
    let mut group = c.benchmark_group(name);
    group.sample_size(50);
    if bytes != 0 {
        group.throughput(Throughput::Bytes(bytes as u64));
    }
    group.bench_function(format!("segment/{bytes}B"), |b| {
        b.iter(|| {
            let t: &str = black_box(text);
            black_box(segment(t))
        })
    });
    group.bench_function(format!("split_sentences/{bytes}B"), |b| {
        b.iter(|| {
            let t: &str = black_box(text);
            black_box(split_sentences(t))
        });
    });
    group.finish();
}

fn bench_all(c: &mut Criterion) {
    let short = short_text();
    let medium = medium_text();
    let long = long_text();
    bench_case(c, "short", &short);
    bench_case(c, "medium", &medium);
    bench_case(c, "long", &long);

    let matrix = [
        ("ascii-heavy", ascii_heavy_text()),
        ("cjk-kana", cjk_kana_text()),
        ("emoji-kaomoji", emoji_kaomoji_text()),
        ("low-symbol-heavy", low_symbol_heavy_text()),
        ("punctuation-dense", punctuation_dense_text()),
        ("no-boundary", no_boundary_text()),
        ("newline-sparse", newline_sparse_text()),
        ("newline-dense", newline_dense_text()),
    ];
    for (name, text) in &matrix {
        bench_case(c, name, text);
    }

    for (name, text) in [
        ("empty", ""),
        ("one-byte", "a"),
        ("eight-byte", "12345678"),
        ("sixteen-byte", "0123456789abcdef"),
    ] {
        bench_case(c, &format!("tiny/{name}"), text);
    }
}

criterion_group!(benches, bench_all);
criterion_main!(benches);
