# aipm CLI Reference

All mutating commands automatically create an undo snapshot before executing.
All commands that return data output JSON to stdout; errors go to stderr.
Task IDs accept short prefixes (4+ hex characters).

## Task commands

- `aipm task list` — list all tasks
- `aipm task show <id>` — show a single task by ID prefix
- `aipm task add --title "X" [--bucket "Y"] [--priority low|medium|high|critical] [--progress backlog|todo|in-progress|done] [--due YYYY-MM-DD] [--description "..."] [--parent <id>]`
- `aipm task edit <id> [--title "X"] [--bucket "Y"] [--priority ...] [--progress ...] [--due YYYY-MM-DD|none] [--description "..."]`
- `aipm task delete <id>` — deletes task and its sub-tasks

## Bucket commands

- `aipm bucket list` — list all buckets
- `aipm bucket add <name> [--description "..."]`
- `aipm bucket rename <old> <new>`
- `aipm bucket delete <name>` — moves tasks to first remaining bucket

## Undo / History

- `aipm undo` — restore state before the last CLI/AI mutation
- `aipm history` — list available undo snapshots (output JSON)
- Snapshots are capped at 50; oldest are trimmed automatically

## AI commands

- `aipm "<instruction>"` — run an AI instruction headlessly (e.g. `aipm "break down all tickets"`)
