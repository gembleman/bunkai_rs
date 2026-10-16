# HANDOFF

## 2026-10-16 no-comments (chore/no-comments)
- done: ~680 comment lines removed; pub API docs moved to README (now also crate docs via `#![doc = include_str!("../README.md")]`); doctests -> tests/public_api_e2e.rs; rationale in docs/rationale/bunkai_rs.md (whitelisted in .gitignore).
- next: merge into master (Forgejo); decide generator drift in issue #1.
- caveat: README usage block is `rust,ignore` so it doesn't become a doctest; cargo test 27/27.
