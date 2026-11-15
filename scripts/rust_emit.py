import subprocess

# Reference boundaries are the original (pre-closing-mark) ones; the helper
# reproduces the port's closing-mark attachment so tests compare like for like.
WITH_CLOSING_MARKS = r"""
fn with_closing_marks(text: &str, expected: &[usize]) -> Vec<usize> {
    let chars: Vec<char> = text.chars().collect();
    let mut adjusted = expected.to_vec();
    for end in &mut adjusted {
        while *end < chars.len()
            && matches!(
                chars[*end],
                ')' | ']'
                    | '}'
                    | '）'
                    | '］'
                    | '｝'
                    | '〉'
                    | '》'
                    | '」'
                    | '』'
                    | '】'
                    | '〕'
                    | '〗'
                    | '〙'
                    | '〟'
                    | '’'
                    | '”'
                    | '｠'
            )
        {
            *end += 1;
        }
    }
    adjusted.sort_unstable();
    adjusted.dedup();
    adjusted
}
"""


def rustfmt(source: str) -> str:
    proc = subprocess.run(
        ["rustfmt", "--edition", "2024", "--emit", "stdout"],
        input=source,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        raise SystemExit(f"rustfmt failed:\n{proc.stderr}")
    return proc.stdout
