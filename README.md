# aipm

<img width="1608" height="1049" alt="image" src="https://github.com/user-attachments/assets/7f352692-9b9e-4285-9a4f-1ebbc9d650c3" />

[![Watch the demo](https://img.youtube.com/vi/PGFg01h9LHU/maxresdefault.jpg)](https://www.youtube.com/watch?v=PGFg01h9LHU)

A terminal-based AI-powered project manager built in Rust.

## Why aipm?

**Natural language task management** — Just type what you need. "Break down the auth feature into sub-tasks" or "mark all design tasks as done." The AI understands intent and executes through tool calls.

**Email inbox → task list** — Integrates with Apple Mail via MCP to automatically surface actionable emails as task suggestions. Marketing and noise filtered out by AI.

**File-per-task architecture** — Each task is a markdown file with YAML front matter. Grep your way through tasks. Perfect for AI coding agents.

**Full keyboard control** — Vim-style bindings. Arrow keys. No mouse needed. Navigate between buckets, timeline, kanban, and settings instantly.

**Context-aware AI** — Paste GitHub PR/issue URLs and the AI automatically fetches context. No manual copying.

## Features

- **Multiple views**: Buckets (columns), Timeline (chronological), Kanban (progress stages)
- **AI triage**: Natural language → create/update/delete/decompose/bulk-update tasks via tool calls
- **Email suggestions**: Native Apple Mail integration surfaces actionable inbox items
- **URL context**: Auto-fetch GitHub PRs/issues or any URL content
- **CLI mode**: Headless commands for scripting (`task list`, `suggestions sync`, etc.)
- **Undo/history**: Snapshot before every operation, rollback anytime
- **Sub-tasks & dependencies**: Hierarchical tasks with automatic parent progress sync
- **Keyboard-driven**: Full vim-style navigation, no mouse required

## Quick Start

```sh
cargo build --release
./target/release/aipm
```

Set up AI (required for triage and email filtering):

```sh
export ANTHROPIC_API_KEY="sk-ant-..."   # for Claude models
export OPENAI_API_KEY="sk-..."           # for OpenAI models
```

Or configure in Settings tab (`Alt+4`).

## Usage

**TUI**: Just run `aipm` and start typing natural language instructions.

**CLI**: Script-friendly JSON output:
```sh
aipm task list
aipm "create three tasks for the auth feature"
aipm suggestions sync --limit 5
```

**Keybindings**: Press `Alt+1/2/3/4/0` to switch tabs. Full vim-style navigation (`hjkl`, arrows, etc.).

## Documentation

### CLI
- [Task Commands](docs/cli/tasks.md) - CRUD operations for tasks
- [Bucket Commands](docs/cli/buckets.md) - Manage task columns/categories
- [Undo/History](docs/cli/undo.md) - Rollback operations

### Features
- [AI Triage](docs/features/ai.md) - Natural language task management
- [Email Suggestions](docs/features/suggestions.md) - Apple Mail MCP integration

### UI
- [Keybindings](docs/ui/keybindings.md) - Complete keyboard reference

### Data
- [Storage Format](docs/data/storage.md) - File-per-task architecture details

## License

MIT
