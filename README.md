# aipm

<img width="1608" height="1049" alt="image" src="https://github.com/user-attachments/assets/7f352692-9b9e-4285-9a4f-1ebbc9d650c3" />

A terminal-based AI-powered project manager built in Rust.

## Features

- **Buckets view** — organize tasks across Team, John-only, and Admin columns
- **Timeline view** — chronological task overview
- **Kanban view** — drag tasks through Backlog → Todo → In Progress → Done
- **Settings** — configure AI model, API key, and other options from within the app
- **AI triage & tools** — natural language input is routed through AI tool calls to create, update, delete, decompose, or bulk-update tasks
- **URL context** — paste a link (GitHub PR/issue or any URL) and the AI automatically fetches and incorporates its content
- **Delete confirmation** — modal prompt before deleting any task
- **Keyboard-driven** — full navigation via keyboard with tab bar focus, arrow keys, and vim-style bindings

## Keybindings

| Context | Key | Action |
|---------|-----|--------|
| Global | `Ctrl-C` | Quit |
| Global | `1/2/3/4` | Switch tabs |
| Input | `/exit` | Quit |
| Tab bar | `←/→` | Navigate tabs |
| Tab bar | `Enter` | Enter selected tab |
| Input | `Enter` | Create task |
| Input | `Esc` | Switch to board |
| Board | `↑/↓/←/→` | Navigate tasks and columns |
| Board | `Enter/e` | Edit selected task |
| Board | `d/x/Backspace/Delete` | Delete task (with confirmation) |
| Board | `p/Space` | Advance progress |
| Board | `P` | Retreat progress |
| Board | `Esc` | Focus tab bar |
| Edit | `↑/↓` | Navigate fields |
| Edit | `Enter/e` | Edit field value |
| Edit | `←/→` | Cycle enum fields |
| Edit | `Esc` | Close overlay |

## Setup

### Requirements

- Rust 1.70+
- An OpenAI API key (for AI features)

### Build & run

```sh
cargo build --release
./target/release/aipm
```

### AI configuration

Set your API key for your preferred provider:

```sh
export ANTHROPIC_API_KEY="sk-ant-..."   # for Claude models
export OPENAI_API_KEY="sk-..."           # for OpenAI models
```

Or configure it in the Settings tab (`4`) within the app. The default model is `claude-sonnet-4-5`.

## CLI commands

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

## AI tools

The AI agent uses tool calling to decide how to handle each input. Available tools:

| Tool | Description |
|------|-------------|
| `create_task` | Create a new task with title, bucket, description, priority, progress, due date, and optional subtasks |
| `update_task` | Update fields on an existing task by ID prefix |
| `delete_task` | Delete a task by ID prefix |
| `decompose_task` | Break a task into smaller subtasks with dependency ordering |
| `bulk_update_tasks` | Apply an instruction across multiple tasks at once |

The AI also automatically fetches context from URLs included in the input:
- **GitHub PRs/Issues** — fetches title, author, state, and body via the GitHub API
- **Generic URLs** — fetches the page, extracts the title and a text snippet

## Data storage

Each task is stored as an individual markdown file with YAML front matter in a `tasks/` directory:

```
<data_dir>/
  tasks/
    550e8400-implement-user-auth.md
    660e8400-setup-oauth-flow.md
    ...
  settings.yaml
```

Task file format:

```yaml
---
id: "550e8400-e29b-41d4-a716-446655440000"
title: "Implement user authentication"
bucket: Team
progress: InProgress
priority: High
due_date: "2026-03-01"
parent_id: "660e8400-e29b-41d4-a716-446655440001"
dependencies:
  - "770e8400-e29b-41d4-a716-446655440002"
created_at: "2026-02-10T12:00:00Z"
updated_at: "2026-02-15T14:30:00Z"
---
Description goes here as the markdown body.
```

This file-per-task architecture makes tasks naturally navigable by AI coding agents:

```sh
grep -r "priority: Critical" tasks/   # find all critical tasks
grep -r "bucket: Team" tasks/          # find all team tasks
ls tasks/                               # browse tasks by name
cat tasks/550e8400-*.md                 # read a specific task
```

Default data directory:

- **macOS**: `~/Library/Application Support/aipm/`
- **Linux**: `$XDG_DATA_HOME/aipm/` or `~/.local/share/aipm/`

Override with `AIPM_DATA_DIR` environment variable.

Existing `tasks.json` data is automatically migrated to the new format on first run.

## License

MIT
