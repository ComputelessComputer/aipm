# Tasks

Tasks are the core entity in aipm. Each task represents a unit of work — a bug to fix, a feature to build, a review to complete.

## Task properties

Every task has:

- **id** — A UUID assigned at creation. Commands accept short prefixes (4+ hex characters) to identify tasks, so you rarely need the full ID.
- **title** — A short summary of the work.
- **description** — Optional free-text body with details, links, acceptance criteria, etc.
- **bucket** — Which bucket (category/project) the task belongs to. Defaults to the first configured bucket if omitted.
- **progress** — The current stage of the task. One of:
  - `backlog` — Not yet planned.
  - `todo` — Planned but not started.
  - `in-progress` — Actively being worked on. When a task moves from `todo` to `in-progress`, its start date is automatically recorded.
  - `done` — Completed.
- **priority** — How urgent the task is. One of: `low`, `medium` (default), `high`, `critical`.
- **due_date** — Optional deadline in `YYYY-MM-DD` format.
- **parent_id** — Optional reference to a parent task, making this a sub-task.
- **dependencies** — A list of task IDs that must be completed before this task can begin.
- **created_at** — Timestamp when the task was created.
- **start_date** — Timestamp when the task first entered `in-progress`.
- **updated_at** — Timestamp of the last modification.

## Commands

All task commands output JSON to stdout. Errors are printed to stderr.

### List all tasks

```
aipm task list
```

Returns a JSON array of every task. Aliases: `aipm task ls`.

### Show a single task

```
aipm task show <id>
```

Looks up a task by ID prefix and returns its full JSON representation. Aliases: `aipm task get`.

### Create a task

```
aipm task add --title "Set up CI/CD pipeline" [options]
```

Creates a new task and prints its JSON. `--title` is required; everything else is optional.

Options:
- `--bucket "Team"` — Assign to a bucket (defaults to the first bucket).
- `--priority high` — Set priority. Accepts: `low`, `medium`/`med`, `high`, `critical`/`crit`.
- `--progress todo` — Set initial progress. Accepts: `backlog`, `todo`, `in-progress`, `done`.
- `--due 2026-03-01` — Set a due date.
- `--description "Deploy to staging and production"` — Set the description.
- `--parent <id>` — Make this a sub-task of another task (by ID prefix).

Aliases: `aipm task create`.

### Edit a task

```
aipm task edit <id> [options]
```

Updates one or more fields on an existing task. Only the fields you pass are changed; everything else is left untouched.

Options are the same as `task add`, plus:
- `--due none` — Clear the due date.

Aliases: `aipm task update`.

### Delete a task

```
aipm task delete <id>
```

Deletes the task and **cascades to all its sub-tasks**. Any other tasks that had a dependency on a deleted task will have that dependency reference removed.

Returns JSON with the deleted task's ID, title, and the total number of tasks removed (including children).

Aliases: `aipm task rm`.

## Sub-tasks

A task becomes a sub-task when it has a `parent_id`. You can create sub-tasks in two ways:

1. **CLI**: Pass `--parent <id>` when creating a task.
2. **AI**: Ask the AI to decompose a task (e.g. `aipm "break down the CI/CD task into sub-issues"`).

Deleting a parent task cascades to all its children.

## Output format

All commands return pretty-printed JSON. A single task looks like:

```json
{
  "id": "4b011ecf-b248-406d-aa98-f51f2a37d156",
  "bucket": "Team",
  "title": "Set up CI/CD pipeline",
  "description": "Deploy to staging and production",
  "dependencies": [],
  "parent_id": null,
  "progress": "Todo",
  "priority": "High",
  "due_date": "2026-03-01",
  "created_at": "2026-02-17T01:00:00Z",
  "start_date": null,
  "updated_at": "2026-02-17T01:00:00Z"
}
```
