# Agent Rules

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

## CLI CRUD Commands

All subcommands output JSON to stdout. Errors go to stderr with a non-zero exit code.
Task IDs accept short prefixes (4+ hex chars), e.g. `36149d52` or `3614`.

### Task commands

- `aipm task list` — JSON array of all tasks.
- `aipm task show <id>` — single task as JSON.
- `aipm task add --title "X" --bucket "Y"` — create a task. Optional flags: `--priority low|medium|high|critical`, `--progress backlog|todo|in-progress|done`, `--due YYYY-MM-DD`, `--description "..."`, `--parent <id>` (for sub-tasks). Prints the created task as JSON.
- `aipm task edit <id>` — update a task. Pass any combination of: `--title`, `--bucket`, `--description`, `--priority`, `--progress`, `--due` (use `none` to clear). Prints the updated task as JSON.
- `aipm task delete <id>` — delete a task and all its sub-tasks. Prints confirmation JSON.

### Bucket commands

- `aipm bucket list` — JSON array of all buckets.
- `aipm bucket add <name>` — add a bucket. Optional: `--description "..."`.
- `aipm bucket rename <old> <new>` — rename a bucket and update all tasks in it.
- `aipm bucket delete <name>` — delete a bucket; tasks move to the first remaining bucket.

### AI commands (headless)

- `aipm "<instruction>"` — send a natural-language instruction to AI for triage (create/update/delete tasks). Requires an API key.

## General

- Run `cargo fmt` before committing to ensure consistent formatting.
- Run `cargo clippy` and fix any warnings before committing.
- Run `cargo build` after code changes to verify compilation before committing.
- Keep commits small and reviewable.
