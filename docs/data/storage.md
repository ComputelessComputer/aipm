# Data Storage

aipm uses a file-per-task architecture where each task is stored as an individual markdown file with YAML front matter.

## Directory Structure

```
<data_dir>/
  tasks/
    550e8400-implement-user-auth.md
    660e8400-setup-oauth-flow.md
    ...
  settings.yaml
  history/
    snapshot-2026-02-17-120000.json
    snapshot-2026-02-17-130000.json
    ...
```

## Task File Format

Each task is stored as a markdown file with YAML front matter:

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
Multi-line descriptions are supported.
```

## Fields

- **id**: UUID v4 identifier
- **title**: Task name
- **bucket**: Column/category (e.g. "Team", "Personal")
- **progress**: One of `Backlog`, `Todo`, `InProgress`, `Done`
- **priority**: One of `Low`, `Medium`, `High`, `Critical`
- **due_date**: Optional ISO date (YYYY-MM-DD)
- **parent_id**: Optional UUID of parent task (for sub-tasks)
- **dependencies**: Array of task UUIDs this task depends on
- **created_at**: ISO 8601 timestamp
- **updated_at**: ISO 8601 timestamp
- **start_date**: Optional timestamp when task moved to InProgress

## Data Directory Locations

Default locations by platform:

- **macOS**: `~/Library/Application Support/aipm/`
- **Linux**: `$XDG_DATA_HOME/aipm/` or `~/.local/share/aipm/`

Override with environment variable:

```sh
export AIPM_DATA_DIR=/path/to/custom/location
```

## File-per-Task Benefits

This architecture makes tasks naturally accessible to AI agents and command-line tools:

```sh
# Find all critical tasks
grep -r "priority: Critical" tasks/

# Find all team tasks
grep -r "bucket: Team" tasks/

# Browse tasks by name
ls tasks/

# Read a specific task
cat tasks/550e8400-*.md

# Count tasks by progress
grep -l "progress: InProgress" tasks/*.md | wc -l
```

## Migration

Existing `tasks.json` data is automatically migrated to the file-per-task format on first run. The old file is preserved as a backup.

## Settings File

Settings are stored in `settings.yaml`:

```yaml
owner_name: "John"
enabled: true
openai_api_key: "sk-..."
anthropic_api_key: "sk-ant-..."
model: "claude-sonnet-4-5"
api_url: ""
timeout_secs: 30
show_backlog: true
show_todo: true
show_in_progress: true
show_done: true
buckets:
  - name: "Team"
    description: "Team-wide tasks"
  - name: "Personal"
    description: "Individual tasks"
mcp_enabled: false
mcp_python_path: "/usr/bin/python3"
mcp_script_path: ""
```

## History / Undo

State snapshots are saved in `history/` before each CLI or AI operation. See [CLI Undo](../cli/undo.md) for details.
