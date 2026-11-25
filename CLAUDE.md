# gridpnt

## Before working

Requirements live in `spec/`, a separate private repository that is ignored by
this one. If `spec/` exists, read `spec/README.md` first and follow its read
order. If it does not exist, stop and ask — do not guess at requirements.

## Rules

- Never copy text from `spec/` into this repository, commit messages, issues,
  or pull requests. Citing a requirement ID (e.g. `SAFE-003`) is fine.
- Commits are authored as `andrew@datapnt.com` (set in local git config for
  both this repo and `spec/`). Check `git config user.email` before the first
  commit in a fresh clone.
- `spec/` is committed separately: `git -C spec ...`.
- Adding a dependency requires a written justification in the spec first.
- Code is written to be read by a person: small modules, plain names, comments
  that explain why.

## Commands

- `cargo test` — all tests. `tests/guard_corpus.rs` is the safety corpus;
  add the failing case there before changing `src/guard.rs`.
- `cargo fmt && cargo clippy --all-targets -- -D warnings` — must be clean
  before committing; CI enforces both on Linux, macOS, and Windows.
- `src/lib.rs` describes the module layout and its two rules.
