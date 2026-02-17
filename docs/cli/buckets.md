# Buckets

Buckets are how aipm organizes tasks into categories. Think of them as projects, teams, or areas of responsibility — any grouping that makes sense for how you work.

Every task belongs to exactly one bucket. When you create a task without specifying a bucket, it's assigned to the first bucket in your configuration.

## Bucket properties

- **name** — The display name (e.g. "Personal", "Team", "Admin"). Must be unique (case-insensitive).
- **description** — Optional text explaining what the bucket is for. Helps the AI agent route tasks to the right bucket during triage.

## Default buckets

A fresh aipm installation comes with three buckets:

- **Personal** — Your own tasks, reviews, and personal direction.
- **Team** — Onboarding, coordination, guiding your crew.
- **Admin** — Taxes, accounting, admin chores.

You can rename, delete, or add buckets at any time.

## Commands

All bucket commands output JSON to stdout. Errors are printed to stderr.

### List all buckets

```
aipm bucket list
```

Returns a JSON array of all configured buckets with their names and descriptions. Aliases: `aipm bucket ls`.

### Add a bucket

```
aipm bucket add "Engineering" --description "Core product development"
```

Creates a new bucket. The name is the first positional argument and is required. Bucket names are unique (case-insensitive) — adding a duplicate will fail.

Options:
- `--description "..."` — Set a description for the bucket.

Aliases: `aipm bucket create`.

### Rename a bucket

```
aipm bucket rename "Admin" "Operations"
```

Renames a bucket. Takes two positional arguments: the current name and the new name. All tasks that belonged to the old bucket are automatically moved to the new name.

Returns JSON with the old name, new name, and how many tasks were updated.

### Delete a bucket

```
aipm bucket delete "Admin"
```

Removes a bucket. You cannot delete the last remaining bucket. When a bucket is deleted, all of its tasks are moved to the first remaining bucket.

Returns JSON with the deleted bucket name, the fallback bucket tasks were moved to, and how many tasks were moved.

Aliases: `aipm bucket rm`.

## How the AI uses buckets

When you send a natural language instruction to aipm (e.g. `aipm "set up a CI/CD pipeline"`), the AI agent decides which bucket to assign the task to based on the bucket names and descriptions. Writing good bucket descriptions helps the AI make better routing decisions.
