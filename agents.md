# Agent Rules

## Commit Discipline

- **Commit after every discrete action.** Each meaningful change (e.g. adding a feature, fixing a bug, refactoring, updating docs, adding a test) must be committed individually before moving on.
- Use concise, imperative commit messages (e.g. `add bucket column reordering`, `fix off-by-one in timeline view`).
- Do not batch unrelated changes into a single commit.
- If a task involves multiple steps, commit after each step — not all at the end.
- Include `Co-Authored-By: Warp <agent@warp.dev>` at the end of every commit message.

## General

- Run `cargo fmt` before committing to ensure consistent formatting.
- Run `cargo clippy` and fix any warnings before committing.
- Run `cargo build` after code changes to verify compilation before committing.
- Keep commits small and reviewable.
