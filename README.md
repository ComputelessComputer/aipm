# aipm

A terminal-based AI-powered project manager built in Rust.

## Features

- **Buckets view** — organize tasks across Team, John-only, and Admin columns
- **Timeline view** — chronological task overview
- **Kanban view** — drag tasks through Backlog → Todo → In Progress → Done
- **Settings** — configure AI model, API key, and other options from within the app
- **AI enrichment** — tasks are automatically enriched with descriptions, priorities, due dates, and dependency suggestions via OpenAI
- **Delete confirmation** — modal prompt before deleting any task
- **Keyboard-driven** — full navigation via keyboard with tab bar focus, arrow keys, and vim-style bindings

## Keybindings

| Context | Key | Action |
|---------|-----|--------|
| Global | `Ctrl-C` | Quit |
| Global | `q` | Quit (from board/tab focus) |
| Global | `1/2/3/4` | Switch tabs |
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

Set your API key via environment variable:

```sh
export OPENAI_API_KEY="sk-..."
```

Or configure it in the Settings tab (`4`) within the app. The default model is `gpt-5.2-chat-latest`.

## Data storage

Tasks and settings are persisted to JSON files in:

- **macOS**: `~/Library/Application Support/aipm/`
- **Linux**: `$XDG_DATA_HOME/aipm/` or `~/.local/share/aipm/`

Override with `AIPM_DATA_DIR` environment variable.

## License

MIT
