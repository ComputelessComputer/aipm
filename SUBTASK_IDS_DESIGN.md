# Subtask ID Display Design

## Current State
- Parent tasks show 8-character short IDs (e.g., `b68532fd`)
- Subtasks display inline below parent with: ` ↳ ○ Title` (no ID shown)
- Subtasks have full UUIDs in the data model but they're not visible in the TUI

## Problem
Users can't easily reference subtasks by ID for:
- `@id` autocomplete to edit subtasks
- Natural language commands referencing specific subtasks
- Quick identification in conversation with LLM

## Design Options

### Option 1: Hierarchical ID (Parent.Child)
Show subtasks as `parent-id.N` where N is the child index (1-based).

**Example:**
```
 b68532fd Product Kanban
   ↳ ○ b68532fd.1 Set up GitHub Projects board
   ↳ ○ b68532fd.2 Configure columns and labels
```

**Pros:**
- Visually shows parent-child relationship
- More compact than full IDs
- Easy to understand hierarchy at a glance
- N stable within a parent (doesn't change unless siblings deleted)

**Cons:**
- Requires index calculation at render time
- Need to handle ID resolution logic (parse `b68532fd.2` → find parent → get 2nd child)
- Index changes if earlier siblings are deleted
- Not a "real" ID (synthetic)

### Option 2: Short ID for Subtasks (Same as Parent)
Give each subtask its own 8-character short ID from its UUID.

**Example:**
```
 b68532fd Product Kanban
   ↳ ○ 3f8a2c91 Set up GitHub Projects board
   ↳ ○ 7d4e1b05 Configure columns and labels
```

**Pros:**
- Consistent with parent task ID format
- Real ID that maps directly to task UUID
- No special parsing logic needed
- Stable - never changes

**Cons:**
- Loses visual parent-child connection
- IDs look unrelated even though they're parent-child
- Takes more horizontal space

### Option 3: Relative Short ID (Last 4 chars)
Show only last 4 characters of subtask UUID to save space.

**Example:**
```
 b68532fd Product Kanban
   ↳ ○ 2c91 Set up GitHub Projects board
   ↳ ○ 1b05 Configure columns and labels
```

**Pros:**
- More compact than full 8-char ID
- Still a real ID portion
- Visually distinct from parent (shorter)

**Cons:**
- Higher collision risk (16^4 = 65,536 combinations)
- Need logic to resolve 4-char to full UUID
- May be ambiguous in large projects

### Option 4: No ID, Use Positional Reference
Keep current display, use `@parent-id/N` for autocomplete.

**Example:**
```
 b68532fd Product Kanban
   ↳ ○ Set up GitHub Projects board
   ↳ ○ Configure columns and labels

Autocomplete: @b68532fd/1, @b68532fd/2
```

**Pros:**
- Clean UI (no clutter)
- Natural syntax for parent/child reference
- Saves horizontal space

**Cons:**
- Indirect reference (requires parent context)
- Index changes if siblings deleted
- Can't see ID in UI directly

## Recommendation: Option 2 (Short ID for Subtasks)

**Rationale:**
1. **Consistency**: Matches parent task ID format
2. **Simplicity**: No special parsing or index tracking
3. **Stability**: IDs never change, unlike positional indices
4. **@autocomplete ready**: Works with existing autocomplete logic
5. **LLM friendly**: LLM can reference subtasks by ID naturally

**Implementation:**
- Modify subtask rendering (line 4440-4452 in main.rs)
- Insert short_id after the `↳` prefix and before the progress icon
- Format: ` ↳ abcd1234 ○ Title`
- Ensure adequate spacing/width calculation

**Code Change:**
```rust
let child = &app.tasks[child_idx];
let short_id = child.id.to_string().chars().take(8).collect::<String>();
let icon = match child.progress { ... };
let prefix_str = " \u{21b3} ";
let id_str = format!("{} ", short_id);

// Update width calculation to account for ID
let title_max = width.saturating_sub(
    prefix_str.width() + id_str.width() + icon.width() + 1
);
let title_text = clamp_text(&child.title, title_max);

queue!(
    stdout,
    MoveTo(x, y_cursor),
    SetForegroundColor(Color::DarkGrey),
    Print(prefix_str),
    Print(&id_str),  // ADD THIS
    SetForegroundColor(progress_color(child.progress)),
    Print(icon),
    SetForegroundColor(Color::DarkGrey),
    Print(format!(" {}", title_text)),
)?;
```
