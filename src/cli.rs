use std::io;

use chrono::Utc;
use uuid::Uuid;

use crate::model::{children_of, BucketDef, Priority, Progress, Task};
use crate::storage::{AiSettings, Storage};

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Try to handle a CLI subcommand. Returns `None` if `args[1]` is not a known
/// subcommand, letting the caller fall through to the AI / TUI paths.
pub fn run_subcommand(args: &[String]) -> Option<io::Result<()>> {
    let sub = args.get(1)?.as_str();
    let rest: Vec<String> = args[2..].to_vec();
    match sub {
        "task" => Some(run_task_cmd(&rest)),
        "bucket" => Some(run_bucket_cmd(&rest)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn find_flag(args: &[String], flag: &str) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == flag {
            return iter.next().cloned();
        }
        if let Some(val) = arg.strip_prefix(&format!("{flag}=")) {
            return Some(val.to_string());
        }
    }
    None
}

fn load() -> (Option<Storage>, Vec<Task>, AiSettings) {
    let storage = Storage::new();
    let tasks = match &storage {
        Some(s) => s.load_tasks().unwrap_or_default(),
        None => Vec::new(),
    };
    let settings = match &storage {
        Some(s) => s.load_settings().unwrap_or_default(),
        None => AiSettings::default(),
    };
    (storage, tasks, settings)
}

fn save_tasks(storage: &Option<Storage>, tasks: &[Task]) {
    if let Some(s) = storage {
        if let Err(err) = s.save_tasks(tasks) {
            eprintln!("Save failed: {err}");
            std::process::exit(1);
        }
    }
}

fn save_settings(storage: &Option<Storage>, settings: &AiSettings) {
    if let Some(s) = storage {
        if let Err(err) = s.save_settings(settings) {
            eprintln!("Settings save failed: {err}");
            std::process::exit(1);
        }
    }
}

fn die(msg: &str) -> ! {
    eprintln!("Error: {msg}");
    std::process::exit(1);
}

fn resolve_task<'a>(tasks: &'a [Task], prefix: &str) -> &'a Task {
    let lower = prefix.to_ascii_lowercase();
    tasks
        .iter()
        .find(|t| {
            let short: String = t.id.to_string().chars().take(lower.len().max(4)).collect();
            short.to_ascii_lowercase().starts_with(&lower)
        })
        .unwrap_or_else(|| die(&format!("No task matching '{prefix}'")))
}

fn parse_priority(s: &str) -> Priority {
    match s.to_ascii_lowercase().as_str() {
        "low" => Priority::Low,
        "med" | "medium" => Priority::Medium,
        "high" => Priority::High,
        "crit" | "critical" => Priority::Critical,
        _ => die(&format!("Unknown priority: {s}")),
    }
}

fn parse_progress(s: &str) -> Progress {
    match s.to_ascii_lowercase().replace('-', " ").as_str() {
        "backlog" => Progress::Backlog,
        "todo" => Progress::Todo,
        "in progress" | "inprogress" | "in_progress" => Progress::InProgress,
        "done" => Progress::Done,
        _ => die(&format!("Unknown progress: {s}")),
    }
}

fn print_json<T: serde::Serialize>(val: &T) {
    println!(
        "{}",
        serde_json::to_string_pretty(val).unwrap_or_else(|_| "null".to_string())
    );
}

// ---------------------------------------------------------------------------
// Task subcommands
// ---------------------------------------------------------------------------

fn run_task_cmd(args: &[String]) -> io::Result<()> {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("list");
    match sub {
        "list" | "ls" => cmd_task_list(),
        "add" | "create" => cmd_task_add(&args[1..]),
        "edit" | "update" => cmd_task_edit(&args[1..]),
        "delete" | "rm" => cmd_task_delete(&args[1..]),
        "show" | "get" => cmd_task_show(&args[1..]),
        other => die(&format!("Unknown task command: {other}")),
    }
}

fn cmd_task_list() -> io::Result<()> {
    let (_, tasks, _) = load();
    print_json(&tasks);
    Ok(())
}

fn cmd_task_add(args: &[String]) -> io::Result<()> {
    let (storage, mut tasks, settings) = load();

    let title = find_flag(args, "--title").unwrap_or_else(|| die("--title is required"));
    let bucket = find_flag(args, "--bucket").unwrap_or_else(|| {
        settings
            .buckets
            .first()
            .map(|b| b.name.clone())
            .unwrap_or_else(|| "Unassigned".to_string())
    });

    let now = Utc::now();
    let mut task = Task::new(bucket, title, now);

    if let Some(desc) = find_flag(args, "--description") {
        task.description = desc;
    }
    if let Some(p) = find_flag(args, "--priority") {
        task.priority = parse_priority(&p);
    }
    if let Some(p) = find_flag(args, "--progress") {
        task.set_progress(parse_progress(&p), now);
    }
    if let Some(d) = find_flag(args, "--due") {
        if let Ok(date) = chrono::NaiveDate::parse_from_str(&d, "%Y-%m-%d") {
            task.due_date = Some(date);
        } else {
            die(&format!("Invalid date format: {d} (expected YYYY-MM-DD)"));
        }
    }
    if let Some(parent_prefix) = find_flag(args, "--parent") {
        let parent = resolve_task(&tasks, &parent_prefix);
        task.parent_id = Some(parent.id);
    }

    print_json(&task);
    tasks.push(task);
    save_tasks(&storage, &tasks);
    Ok(())
}

fn cmd_task_edit(args: &[String]) -> io::Result<()> {
    let prefix = args
        .first()
        .map(|s| s.as_str())
        .unwrap_or_else(|| die("task id required"));
    let (storage, mut tasks, _) = load();
    let task_id = resolve_task(&tasks, prefix).id;
    let now = Utc::now();

    let task = tasks.iter_mut().find(|t| t.id == task_id).unwrap();

    if let Some(t) = find_flag(args, "--title") {
        task.title = t;
        task.updated_at = now;
    }
    if let Some(b) = find_flag(args, "--bucket") {
        task.bucket = b;
        task.updated_at = now;
    }
    if let Some(d) = find_flag(args, "--description") {
        task.description = d;
        task.updated_at = now;
    }
    if let Some(p) = find_flag(args, "--priority") {
        task.priority = parse_priority(&p);
        task.updated_at = now;
    }
    if let Some(p) = find_flag(args, "--progress") {
        task.set_progress(parse_progress(&p), now);
    }
    if let Some(d) = find_flag(args, "--due") {
        if d.is_empty() || d == "none" {
            task.due_date = None;
        } else if let Ok(date) = chrono::NaiveDate::parse_from_str(&d, "%Y-%m-%d") {
            task.due_date = Some(date);
        } else {
            die(&format!("Invalid date format: {d} (expected YYYY-MM-DD)"));
        }
        task.updated_at = now;
    }

    let task_clone = task.clone();
    save_tasks(&storage, &tasks);
    print_json(&task_clone);
    Ok(())
}

fn cmd_task_delete(args: &[String]) -> io::Result<()> {
    let prefix = args
        .first()
        .map(|s| s.as_str())
        .unwrap_or_else(|| die("task id required"));
    let (storage, mut tasks, _) = load();
    let target = resolve_task(&tasks, prefix);
    let target_id = target.id;
    let title = target.title.clone();

    // Cascade: collect all children.
    let child_ids: Vec<Uuid> = children_of(&tasks, target_id)
        .iter()
        .map(|&i| tasks[i].id)
        .collect();
    let all_deleted: Vec<Uuid> = std::iter::once(target_id).chain(child_ids).collect();
    let deleted_count = all_deleted.len();

    tasks.retain(|t| !all_deleted.contains(&t.id));
    // Clean up dependency references.
    for task in &mut tasks {
        task.dependencies.retain(|dep| !all_deleted.contains(dep));
    }

    save_tasks(&storage, &tasks);
    print_json(&serde_json::json!({
        "deleted": target_id.to_string(),
        "title": title,
        "cascade_count": deleted_count,
    }));
    Ok(())
}

fn cmd_task_show(args: &[String]) -> io::Result<()> {
    let prefix = args
        .first()
        .map(|s| s.as_str())
        .unwrap_or_else(|| die("task id required"));
    let (_, tasks, _) = load();
    let task = resolve_task(&tasks, prefix);
    print_json(task);
    Ok(())
}

// ---------------------------------------------------------------------------
// Bucket subcommands
// ---------------------------------------------------------------------------

fn run_bucket_cmd(args: &[String]) -> io::Result<()> {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("list");
    match sub {
        "list" | "ls" => cmd_bucket_list(),
        "add" | "create" => cmd_bucket_add(&args[1..]),
        "rename" => cmd_bucket_rename(&args[1..]),
        "delete" | "rm" => cmd_bucket_delete(&args[1..]),
        other => die(&format!("Unknown bucket command: {other}")),
    }
}

fn cmd_bucket_list() -> io::Result<()> {
    let (_, _, settings) = load();
    print_json(&settings.buckets);
    Ok(())
}

fn cmd_bucket_add(args: &[String]) -> io::Result<()> {
    let name = args
        .first()
        .map(|s| s.as_str())
        .unwrap_or_else(|| die("bucket name required"));
    let (storage, _, mut settings) = load();

    if settings
        .buckets
        .iter()
        .any(|b| b.name.eq_ignore_ascii_case(name))
    {
        die(&format!("Bucket \"{name}\" already exists"));
    }

    let desc = find_flag(args, "--description");
    let bucket = BucketDef {
        name: name.to_string(),
        description: desc,
    };
    print_json(&bucket);
    settings.buckets.push(bucket);
    save_settings(&storage, &settings);
    Ok(())
}

fn cmd_bucket_rename(args: &[String]) -> io::Result<()> {
    let old = args
        .first()
        .map(|s| s.as_str())
        .unwrap_or_else(|| die("old name required"));
    let new = args
        .get(1)
        .map(|s| s.as_str())
        .unwrap_or_else(|| die("new name required"));
    let (storage, mut tasks, mut settings) = load();

    let bucket = settings
        .buckets
        .iter_mut()
        .find(|b| b.name.eq_ignore_ascii_case(old))
        .unwrap_or_else(|| die(&format!("Bucket \"{old}\" not found")));

    let old_name = bucket.name.clone();
    bucket.name = new.to_string();

    let mut moved = 0usize;
    for task in &mut tasks {
        if task.bucket.eq_ignore_ascii_case(&old_name) {
            task.bucket = new.to_string();
            moved += 1;
        }
    }

    save_settings(&storage, &settings);
    if moved > 0 {
        save_tasks(&storage, &tasks);
    }

    print_json(&serde_json::json!({
        "old": old_name,
        "new": new,
        "tasks_updated": moved,
    }));
    Ok(())
}

fn cmd_bucket_delete(args: &[String]) -> io::Result<()> {
    let name = args
        .first()
        .map(|s| s.as_str())
        .unwrap_or_else(|| die("bucket name required"));
    let (storage, mut tasks, mut settings) = load();

    if settings.buckets.len() <= 1 {
        die("Cannot delete the last bucket");
    }

    let pos = settings
        .buckets
        .iter()
        .position(|b| b.name.eq_ignore_ascii_case(name))
        .unwrap_or_else(|| die(&format!("Bucket \"{name}\" not found")));

    let removed = settings.buckets.remove(pos);
    let fallback = settings
        .buckets
        .first()
        .map(|b| b.name.clone())
        .unwrap_or_else(|| "Unassigned".to_string());

    let mut moved = 0usize;
    for task in &mut tasks {
        if task.bucket.eq_ignore_ascii_case(&removed.name) {
            task.bucket = fallback.clone();
            moved += 1;
        }
    }

    save_settings(&storage, &settings);
    if moved > 0 {
        save_tasks(&storage, &tasks);
    }

    print_json(&serde_json::json!({
        "deleted": removed.name,
        "tasks_moved_to": fallback,
        "tasks_moved": moved,
    }));
    Ok(())
}
