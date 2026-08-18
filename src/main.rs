//! Command-line interface for `bunkai_rs`.
//!
//! Reads Japanese text from stdin, runs the sentence-boundary pipeline, and
//! writes the result to stdout. By default each sentence is printed on its own
//! line.

use std::env;
use std::io::{self, Read, Write};
use std::process::ExitCode;

use bunkai_rs::{segment, split_sentences};

enum Mode {
    Sentences,
    Boundaries,
    Layers,
}

const USAGE: &str = "\
bunkai_rs — Japanese sentence-boundary disambiguation

USAGE:
    bunkai_rs [OPTIONS]

Reads text from stdin and prints one sentence per line to stdout.

OPTIONS:
    -b, --boundaries     Print the final character-offset boundaries
                         (comma-separated, on a single line) instead of sentences
    -l, --layers         Print every pipeline layer (name + spans) for debugging
    -h, --help           Print help information
    -V, --version        Print version information
";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("bunkai_rs {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    let mode = resolve_mode(&args);
    if let Err(message) = mode {
        eprintln!("error: {message}\n");
        eprint!("{USAGE}");
        return ExitCode::from(2);
    }
    let mode = mode.unwrap();

    let mut buffer = Vec::new();
    if let Err(error) = io::stdin().read_to_end(&mut buffer) {
        eprintln!("error: could not read stdin: {error}");
        return ExitCode::FAILURE;
    }
    let text = match std::str::from_utf8(&buffer) {
        Ok(text) => text.trim_end_matches('\n').trim_end_matches('\r'),
        Err(_) => {
            eprintln!("error: input is not valid UTF-8");
            return ExitCode::FAILURE;
        }
    };

    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    if let Err(error) = emit(&mut out, mode, text) {
        eprintln!("error: {error}");
        return ExitCode::FAILURE;
    }
    if let Err(error) = out.flush() {
        eprintln!("error: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn resolve_mode(args: &[String]) -> Result<Mode, String> {
    let mut flags: Vec<&String> = Vec::new();
    for arg in args {
        match arg.as_str() {
            "-b" | "--boundaries" => flags.push(arg),
            "-l" | "--layers" => flags.push(arg),
            other if other.starts_with('-') => {
                if other != "--" {
                    return Err(format!("unknown option `{other}`"));
                }
            }
            _ => return Err(format!("unexpected argument `{arg}`")),
        }
    }
    if flags.is_empty() {
        return Ok(Mode::Sentences);
    }
    if flags.len() > 1 {
        return Err("only one output mode may be given".to_string());
    }
    match flags[0].as_str() {
        "-b" | "--boundaries" => Ok(Mode::Boundaries),
        "-l" | "--layers" => Ok(Mode::Layers),
        _ => unreachable!(),
    }
}

fn emit(out: &mut dyn Write, mode: Mode, text: &str) -> io::Result<()> {
    match mode {
        Mode::Sentences => {
            for sentence in split_sentences(text) {
                writeln!(out, "{sentence}")?;
            }
            Ok(())
        }
        Mode::Boundaries => {
            let boundaries = segment(text).final_boundaries;
            let joined: Vec<String> = boundaries.iter().map(ToString::to_string).collect();
            writeln!(out, "{}", joined.join(","))
        }
        Mode::Layers => {
            let segmentation = segment(text);
            for layer in &segmentation.layers {
                write!(out, "[{:<28}]", layer.name)?;
                for span in &layer.spans {
                    write!(
                        out,
                        " {}-{} {}",
                        span.start,
                        span.end,
                        color(span.rule_name.as_str())
                    )?;
                    if let Some(value) = &span.split_value {
                        write!(out, " {value:?}")?;
                    }
                }
                writeln!(out)?;
            }
            let joined: Vec<String> = segmentation
                .final_boundaries
                .iter()
                .map(ToString::to_string)
                .collect();
            writeln!(out, "final_boundaries: {}", joined.join(","))?;
            Ok(())
        }
    }
}

/// Shorthand description of a split rule for the `--layers` view.
fn color(rule: &str) -> &str {
    match rule {
        "BasicRule" => "basic",
        "DotExceptionAnnotator" => "dot",
        "EmojiAnnotator" => "emoji",
        "EmotionExpressionAnnotator" => "emotion",
        "FaceMarkDetector" => "facemark",
        "first" => "first",
        "IndirectQuoteExceptionAnnotator" => "indirect",
        "LinebreakForceAnnotator" => "linebreak",
        "NumberExceptionAnnotator" => "number",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_mode_defaults_to_sentences() {
        assert!(matches!(resolve_mode(&[]), Ok(Mode::Sentences)));
        assert!(matches!(resolve_mode(&["--".into()]), Ok(Mode::Sentences)));
        assert!(matches!(resolve_mode(&["-b".into()]), Ok(Mode::Boundaries)));
        assert!(matches!(
            resolve_mode(&["--layers".into()]),
            Ok(Mode::Layers)
        ));
    }

    #[test]
    fn resolve_mode_rejects_conflicts_and_unknown_flags() {
        assert!(resolve_mode(&["-b".into(), "-l".into()]).is_err());
        assert!(resolve_mode(&["--nope".into()]).is_err());
        assert!(resolve_mode(&["text".into()]).is_err());
    }

    #[test]
    fn emit_writes_sentences_one_per_line() {
        let mut buf = Vec::new();
        emit(&mut buf, Mode::Sentences, "これは一文目。これは二文目。").unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out, "これは一文目。\nこれは二文目。\n");
    }

    #[test]
    fn emit_writes_comma_separated_boundaries() {
        let mut buf = Vec::new();
        emit(&mut buf, Mode::Boundaries, "日本語です。Englishです。").unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "6,16\n");
    }

    #[test]
    fn emit_layers_shows_each_pipeline_stage() {
        let mut buf = Vec::new();
        emit(&mut buf, Mode::Layers, "スタッフ? と話し込み。").unwrap();
        let out = String::from_utf8(buf).unwrap();
        // Every pipeline stage is listed, in order, ending with linebreak.
        let names = [
            "first",
            "FaceMarkDetector",
            "EmotionExpressionAnnotator",
            "EmojiAnnotator",
            "BasicRule",
            "IndirectQuoteExceptionAnnotator",
            "DotExceptionAnnotator",
            "NumberExceptionAnnotator",
            "LinebreakForceAnnotator",
        ];
        for name in names {
            assert!(out.contains(name), "layer {name} missing from output");
        }
        assert!(out.contains("final_boundaries: 12"));
    }
}
