# Email Suggestions

The email suggestions feature integrates directly with Apple Mail via AppleScript to automatically surface actionable tasks from your inbox.

## Overview

- **Background polling**: Automatically checks for new unread emails every 60 seconds
- **AI filtering**: Uses LLM to identify actionable emails and filter out noise (sales, marketing, etc.)
- **Suggestions tab**: View suggested tasks in the app (press `0` to access)
- **Archive detection**: Automatically removes suggestions and tasks when you archive emails in Mail.app
- **CLI commands**: Sync emails and create tasks from the command line

## Setup

### 1. Enable Email Suggestions

Press `0` to open the Suggestions tab, then press `e` to toggle email suggestions on.

Or via CLI:
```sh
aipm settings --email-suggestions true
```

### 2. Grant Mail.app Access

macOS will prompt you to allow aipm to control Mail.app the first time. Grant the permission.

## Using the Suggestions Tab

Press `0` to open the Suggestions tab (rightmost tab).

### Keybindings

| Key | Action |
|-----|--------|
| `e` | Toggle email suggestions on/off |
| `↑/k` | Navigate up |
| `↓/j` | Navigate down |
| `Enter` | Create task from suggestion (moves to Backlog) |
| `d/x/Backspace/Delete` | Dismiss suggestion |
| `i` | Switch to input tab |
| `Esc` | Focus tab bar |

### How It Works

1. Background thread polls Apple Mail every 60 seconds for unread emails
2. AI analyzes each email to determine if it's actionable
3. Marketing/sales emails are automatically filtered out
4. Actionable emails appear as suggestions in the tab
5. Accept suggestions to create tasks, or dismiss them
6. When you archive an email in Mail.app, the suggestion and any created task are automatically removed

## CLI Commands

### List Suggestions

Preview unread emails and see which ones are actionable:

```sh
aipm suggestions list
```

Output shows:
- Email ID, sender, subject, date
- ✓ Actionable emails with extracted task details
- ✗ Non-actionable emails that were filtered out

### Sync Emails to Tasks

Automatically create tasks from actionable emails:

```sh
aipm suggestions sync
```

Options:
- `--limit N` — Process only the first N emails (default: 10)

Example:
```sh
aipm suggestions sync --limit 5
```

The command:
1. Fetches recent unread emails from Apple Mail
2. Runs AI filtering on each email
3. Creates tasks in the first bucket (Backlog) for actionable emails
4. Returns JSON with count of created tasks

Created tasks include:
- Title and description extracted from email
- Priority determined by AI
- Email sender and ID in description (for reference)

## AI Filtering

The AI filter analyzes emails based on:
- **Subject line** — Looking for action items, requests, deadlines
- **Sender** — Context about who sent it
- **Content** — Body of the email

**Filtered out automatically:**
- Sales and marketing emails
- Newsletters
- Promotional content
- Automated notifications without action items

**Identified as actionable:**
- Meeting requests requiring preparation
- Project updates needing action
- Questions requiring response
- Deadlines and reminders
- Task assignments

## Archive Detection

When you archive or mark an email as read in Mail.app:
1. The background poller detects the change (within 60 seconds)
2. Corresponding suggestions are removed from the Suggestions tab
3. Any tasks created from that email are deleted
4. Changes are persisted automatically

This keeps your task list synchronized with your inbox state.

## Data Flow

```
Mail.app
  ↓ (AppleScript / osascript)
Background Thread
  ↓ (AI Filter)
Suggestions Channel
  ↓ (EmailEvent::NewSuggestion)
App State → Suggestions Tab (0)
  ↓ (User accepts)
Tasks
```

## Troubleshooting

### No suggestions appearing

1. Check email suggestions are enabled in the Suggestions tab (`0`, press `e`)
2. Ensure Mail.app has unread emails
3. Check that aipm has Automation permission for Mail.app (System Settings → Privacy & Security → Automation)

### Suggestions not updating

- Background polling runs every 60 seconds
- Check AI API key is configured for filtering
- Verify network connectivity

### Tasks not deleted when archiving emails

- Archive detection runs on the same 60-second polling cycle
- Ensure the email ID was properly tracked when the task was created
- Check that the email was actually archived (not just marked as read)

## Privacy & Security

- Email content is sent to your configured LLM provider for filtering
- No email data is stored permanently by aipm
- Email IDs are stored in a runtime map to track task-email associations
- Map is cleared when the app exits

If privacy is a concern, you can disable email suggestions and use the CLI commands manually instead of background polling.
