# Email Suggestions

The email suggestions feature integrates with Apple Mail via the Model Context Protocol (MCP) to automatically surface actionable tasks from your inbox.

## Overview

- **Background polling**: Automatically checks for new unread emails every 60 seconds
- **AI filtering**: Uses LLM to identify actionable emails and filter out noise (sales, marketing, etc.)
- **Suggestions tab**: View suggested tasks in the app (press `F12` to access)
- **Archive detection**: Automatically removes suggestions and tasks when you archive emails in Mail.app
- **CLI commands**: Sync emails and create tasks from the command line

## Setup

### 1. Install Apple Mail MCP Server

Install the Apple Mail MCP server from:
https://playbooks.com/mcp/patrickfreyer/apple-mail-mcp

Follow the installation instructions to set up the Python script.

### 2. Configure MCP in aipm

In the Settings tab (`F4`), configure:
- **MCP Enabled**: Toggle to `On`
- **MCP Python Path**: Path to Python executable (e.g. `/usr/bin/python3`)
- **MCP Script Path**: Path to the MCP server script

Or set via environment variables:
```sh
export AIPM_MCP_ENABLED=true
export AIPM_MCP_PYTHON_PATH=/usr/bin/python3
export AIPM_MCP_SCRIPT_PATH=/path/to/apple-mail-mcp.py
```

### 3. Grant Mail.app Access

The MCP server needs permission to access Mail.app. macOS will prompt you the first time.

## Using the Suggestions Tab

Press `0` to open the Suggestions tab (rightmost tab).

### Keybindings

| Key | Action |
|-----|--------|
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
# Create tasks from up to 5 actionable emails
aipm suggestions sync --limit 5
```

The command:
1. Fetches recent unread emails via MCP
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
  ↓ (MCP JSON-RPC)
Background Thread
  ↓ (AI Filter)
Suggestions Channel
  ↓ (EmailEvent::NewSuggestion)
App State → Suggestions Tab (Tab 0)
  ↓ (User accepts)
Tasks
```

## Troubleshooting

### No suggestions appearing

1. Check MCP is enabled in Settings (`4`)
2. Verify Python path and script path are correct
3. Ensure Mail.app has unread emails
4. Check that Python script has Mail.app permissions

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

If privacy is a concern, you can disable MCP in Settings and use the CLI commands manually instead of background polling.
