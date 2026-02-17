# Undo & History

aipm automatically takes a snapshot of your data before every mutating CLI or AI operation. If something goes wrong — an accidental delete, an AI triage that went sideways — you can roll back to the previous state in one command.

## How it works

Before any of these operations execute, aipm saves a full snapshot of all tasks and settings:

- `aipm task add`, `aipm task edit`, `aipm task delete`
- `aipm bucket add`, `aipm bucket rename`, `aipm bucket delete`
- `aipm "<instruction>"` (AI triage, only when changes are actually made)

Snapshots are stored as JSON files in the `history/` directory inside your aipm data folder. Each snapshot contains the complete state of all tasks and settings at that point in time, along with a label describing what operation was about to happen.

Snapshots are capped at 50 entries. When the limit is exceeded, the oldest snapshots are automatically removed.

Note: TUI (interactive) changes do not create snapshots, since those changes are incremental and can be manually corrected in-place.

## Commands

### Undo the last operation

```
aipm undo
```

Restores tasks and settings to the state captured in the most recent snapshot, then removes that snapshot from history.

You can chain multiple undos to walk further back:

```
aipm undo   # undoes the last operation
aipm undo   # undoes the one before that
```

Returns JSON indicating which operation was undone:

```json
{
  "restored_before": "task delete 4b01"
}
```

If there's no history available, the command exits with an error.

### View undo history

```
aipm history
```

Lists all available snapshots as a JSON array, ordered oldest to newest:

```json
[
  {
    "seq": 1,
    "label": "task add",
    "timestamp": "2026-02-17T01:48:31Z"
  },
  {
    "seq": 2,
    "label": "ai triage",
    "timestamp": "2026-02-17T02:10:05Z"
  }
]
```

Each entry shows:
- **seq** — The sequence number of the snapshot.
- **label** — A description of the operation that was about to run when the snapshot was taken.
- **timestamp** — When the snapshot was created.
