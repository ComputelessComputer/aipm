# AI Agent

aipm includes a built-in AI agent that can create, edit, organize, and decompose tasks using natural language. It works in two modes: headless CLI and interactive TUI.

## Headless mode (CLI)

```
aipm "<instruction>"
```

Pass a natural language instruction as a quoted string. The AI reads your current tasks and settings, decides what changes to make, and applies them directly. Output is printed to stderr; the process exits when done.

Examples:

```
aipm "create a task to set up CI/CD pipeline"
aipm "break down all tickets into sub-issues"
aipm "mark the onboarding task as done"
aipm "move all admin tasks to the Personal bucket"
aipm "set high priority on everything due this week"
```

The AI can perform multiple actions in a single instruction — creating tasks, editing fields, decomposing tasks into sub-tasks, and bulk-updating groups of tasks.

A snapshot is automatically taken before any changes are saved, so you can always `aipm undo` if the AI does something unexpected.

## Interactive mode (TUI)

When you launch `aipm` without arguments, you get the interactive TUI. The input field at the bottom of the screen accepts:

- **Free text** — The AI triages it: creates tasks, assigns them to buckets, sets priority and progress.
- **@\<id\> \<instruction\>** — Targets a specific task by ID prefix for AI editing. For example, `@4b01 add sub-tasks for testing and deployment`.
- **/clear** — Clears the AI conversation context (starts a fresh session).
- **/exit** — Quits the app.

The TUI also provides an autocomplete dropdown when you type `@` — it shows matching tasks filtered by ID prefix or title substring, navigable with arrow keys.

## Supported models

aipm supports both OpenAI and Anthropic models. Configure the model in the TUI settings tab or via environment variables:

- `OPENAI_API_KEY` — Required for OpenAI models.
- `ANTHROPIC_API_KEY` — Required for Anthropic models.
- `AIPM_MODEL` — Override the default model (default: `claude-sonnet-4-5`).

Available models:
- **Anthropic**: `claude-opus-4-6`, `claude-opus-4-5`, `claude-sonnet-4-5`
- **OpenAI**: `codex-mini-latest`, `o3`, `o4-mini`

## Configuration

AI settings are stored in `settings.yaml` inside your aipm data directory. You can edit them through the TUI settings tab or by modifying the file directly:

- **enabled** — Toggle AI on/off.
- **model** — Which model to use.
- **api_url** — Custom API endpoint (leave empty for default provider URLs).
- **timeout_secs** — Request timeout in seconds (default: 60).
- **owner_name** — Your name, included in the AI's context so it can personalize task routing.

## Environment variables

- `OPENAI_API_KEY` — OpenAI API key.
- `ANTHROPIC_API_KEY` — Anthropic API key.
- `AIPM_MODEL` — Override the configured model.
- `AIPM_DATA_DIR` — Override the data directory location.
