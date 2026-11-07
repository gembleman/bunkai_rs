# bunkai_rs

Docs (HANDOFF, rationale/<source path>.md, guides): ~/docs-repo/repos/bunkai_rs/ — edit there; the main agent commits it.

Rust port of bunkai (rule-based Japanese sentence-boundary disambiguation), edition 2024.

## Commands
- Build/check: `CARGO_BUILD_JOBS=4 cargo check`
- Tests: `cargo test` (E2E only, no in-crate unit tests)
- Bench: `cargo bench` (criterion, bench `segment`)
- Fixtures: `scripts/generate_golden.py`, `scripts/generate_differential.py`

## Don'ts
- Comments only for why, short (non-obvious invariants, SAFETY, external-bug workarounds; see ~/.claude/rules/code.md); rationale goes to docs-repo `rationale/<source path>.md` under a `path::symbol` heading.
- No unit tests; tests drive the public API or CLI.
- Never commit in a live main checkout; delete with `trash-put`.
- Use a separate CARGO_TARGET_DIR for scratch harnesses; never ~/.rust-target-prod.
