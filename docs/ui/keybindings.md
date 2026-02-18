# Keybindings

aipm is fully keyboard-driven with vim-style navigation options.

## Global

| Key | Action |
|-----|--------|
| `Ctrl-C` | Quit application |
| `1` | Switch to Buckets tab |
| `2` | Switch to Timeline tab |
| `3` | Switch to Kanban tab |
| `4` | Switch to Suggestions tab |
| `0` | Switch to Settings tab |

## Tab Bar Focus

When the tab bar is focused (yellow highlight):

| Key | Action |
|-----|--------|
| `←/→` | Navigate between tabs |
| `Enter` | Enter selected tab |
| `i` | Jump to input field |

## Input Field

| Key | Action |
|-----|--------|
| `Enter` | Submit input to AI |
| `Esc` | Switch focus to board |
| `/exit` | Quit application |
| `/clear` | Clear AI conversation context |
| `↑/↓` | Navigate input history |
| `Cmd-Backspace` | Delete to start of line |
| `Option-Backspace` | Delete word before cursor |

### AI Input Patterns

- Type text directly for AI triage (creates/updates tasks)
- `@<id> <instruction>` — AI-edit a specific task by ID prefix
- Paste URLs for automatic context fetching (GitHub, generic URLs)

## Board View (Buckets/Kanban)

| Key | Action |
|-----|--------|
| `↑/↓` or `k/j` | Navigate tasks vertically |
| `←/→` or `h/l` | Navigate columns horizontally |
| `Enter` or `e` | Edit selected task |
| `d/x/Backspace/Delete` | Delete task (shows confirmation) |
| `p` or `Space` | Advance task progress |
| `P` | Retreat task progress |
| `Esc` | Focus tab bar |
| `i` | Jump to input field |

## Timeline View

| Key | Action |
|-----|--------|
| `↑/↓` or `k/j` | Navigate tasks |
| `Enter` or `e` | Edit selected task |
| `d/x/Backspace/Delete` | Delete task (shows confirmation) |
| `Esc` | Focus tab bar |
| `i` | Jump to input field |

## Suggestions Tab

| Key | Action |
|-----|--------|
| `↑/↓` or `k/j` | Navigate suggestions |
| `Enter` | Create task from suggestion (moves to Backlog) |
| `d/x/Backspace/Delete` | Dismiss suggestion |
| `Esc` | Focus tab bar |
| `i` | Jump to input field |

## Edit Overlay

When editing a task:

| Key | Action |
|-----|--------|
| `↑/↓` | Navigate fields |
| `Enter` or `e` | Edit field value (text fields) |
| `←/→` | Cycle enum values (Progress, Priority) |
| `Esc` | Close overlay without saving |
| `Enter` (in SubIssues) | Drill into subtask |
| `Backspace` (in SubIssues) | Go back to parent |

### Text Field Editing

When editing title or description:

| Key | Action |
|-----|--------|
| `Esc` | Save and return to field list |
| `←/→` | Move cursor |
| `Backspace` | Delete character before cursor |
| `Cmd-Backspace` | Delete to start of line |
| `Option-Backspace` | Delete word before cursor |
| `Ctrl-U` | Delete to start of line |

### Date Field Editing

For due date field, enter dates in `YYYY-MM-DD` format or use:
- `today` — Set to current date
- `tomorrow` — Set to next day
- `<empty>` — Clear due date

## Settings Tab

| Key | Action |
|-----|--------|
| `↑/↓` | Navigate settings fields |
| `Enter` | Edit selected field |
| `←/→` | Toggle boolean fields |
| `Esc` | Return to settings list (when editing) |
| `Esc` | Focus tab bar (when in list) |

## Delete Confirmation

When confirming a delete:

| Key | Action |
|-----|--------|
| `Enter` or `y` | Confirm delete |
| `Esc` or `n` | Cancel |

## Bucket Header Edit

When editing bucket headers (name/description):

| Key | Action |
|-----|--------|
| `↑/↓` | Navigate fields |
| `Enter` or `e` | Edit field value |
| `Esc` | Close overlay |
| `d/x/Delete` | Delete bucket |

## Paste Support

The app supports pasting text with `Cmd-V` (automatically triggered by terminal):
- Input field: Inserts pasted text
- Edit overlay text fields: Inserts pasted text
- Newlines are converted to spaces for single-line fields
