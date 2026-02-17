---
alwaysApply: true
---

## Commit Discipline

- **Commit after every discrete action.** Each meaningful change (e.g. adding a feature, fixing a bug, refactoring, updating docs, adding a test) must be committed individually before moving on.
- Use concise, imperative commit messages (e.g. `add bucket column reordering`, `fix off-by-one in timeline view`).
- Do not batch unrelated changes into a single commit.
- If a task involves multiple steps, commit after each step — not all at the end.
- Include `Co-Authored-By: Warp <agent@warp.dev>` at the end of every commit message.

## Releases

- When asked to create a release: bump the version in `Cargo.toml`, commit, push, then create the release with `gh release create`.
- Releases must be published immediately — do **not** use `--draft`.
- Include release notes with concise, descriptive bullet points explaining what changed (e.g. `- Add @ autocomplete dropdown for selecting tasks by ID or title`). Do not just list version numbers or raw commit messages.
- Each bullet should describe the user-facing change, not implementation details.

## Comments

- By default, avoid writing comments at all.
- If you write one, it should be about "Why", not "What".

## CLI CRUD Commands

### Task commands (output JSON)
- `aipm task list` — list all tasks
- `aipm task show <id>` — show a single task by ID prefix
- `aipm task add --title "X" [--bucket "Y"] [--priority low|medium|high|critical] [--progress backlog|todo|in-progress|done] [--due YYYY-MM-DD] [--description "..."] [--parent <id>]`
- `aipm task edit <id> [--title "X"] [--bucket "Y"] [--priority ...] [--progress ...] [--due YYYY-MM-DD|none] [--description "..."]`
- `aipm task delete <id>` — deletes task and its sub-tasks

### Bucket commands (output JSON)
- `aipm bucket list` — list all buckets
- `aipm bucket add <name> [--description "..."]`
- `aipm bucket rename <old> <new>`
- `aipm bucket delete <name>` — moves tasks to first remaining bucket

### Undo / History
- `aipm undo` — restore state before the last CLI/AI mutation (snapshots are taken automatically before each mutating command)
- `aipm history` — list available undo snapshots (output JSON)
- Snapshots are capped at 50; oldest are trimmed automatically

### AI commands
- `aipm "<instruction>"` — run an AI instruction headlessly (e.g. `aipm "break down all tickets"`)

## General

- Avoid creating unnecessary structs, enums, or traits if they are not shared. Prefer inlining types when they're only used in one place.
- Run `cargo fmt` before committing to ensure consistent formatting.
- Run `cargo clippy` and fix any warnings before committing.
- Run `cargo check` periodically while making Rust changes to catch errors early — don't wait until the end.
- Run `cargo build` after code changes to verify compilation before committing.
- Keep commits small and reviewable.
