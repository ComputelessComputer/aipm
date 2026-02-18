use std::io;

use chrono::Utc;
use uuid::Uuid;

use crate::model::{children_of, compute_parent_progress, BucketDef, Priority, Progress, Task};
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
        "settings" => Some(run_settings_cmd(&rest)),
        "suggestions" => Some(run_suggestions_cmd(&rest)),
        "undo" => Some(cmd_undo()),
        "history" => Some(cmd_history()),
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
    if let Some(s) = &storage {
        s.snapshot("task add");
    }

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
    if let Some(s) = &storage {
        s.snapshot(&format!("task edit {prefix}"));
    }
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
    let mut progress_changed = false;
    if let Some(p) = find_flag(args, "--progress") {
        task.set_progress(parse_progress(&p), now);
        progress_changed = true;
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
    if progress_changed {
        sync_parent_progress(&mut tasks, task_id, now);
    }
    save_tasks(&storage, &tasks);
    print_json(&task_clone);
    Ok(())
}

fn sync_parent_progress(tasks: &mut [Task], child_id: Uuid, now: chrono::DateTime<Utc>) {
    let parent_id = match tasks
        .iter()
        .find(|t| t.id == child_id)
        .and_then(|t| t.parent_id)
    {
        Some(pid) => pid,
        None => return,
    };
    let child_progresses: Vec<Progress> = tasks
        .iter()
        .filter(|t| t.parent_id == Some(parent_id))
        .map(|t| t.progress)
        .collect();
    if let Some(new_progress) = compute_parent_progress(&child_progresses) {
        if let Some(parent) = tasks.iter_mut().find(|t| t.id == parent_id) {
            if parent.progress != new_progress {
                parent.set_progress(new_progress, now);
            }
        }
    }
}

fn cmd_task_delete(args: &[String]) -> io::Result<()> {
    let prefix = args
        .first()
        .map(|s| s.as_str())
        .unwrap_or_else(|| die("task id required"));
    let (storage, mut tasks, _) = load();
    if let Some(s) = &storage {
        s.snapshot(&format!("task delete {prefix}"));
    }
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
    sync_parent_progress(&mut tasks, target_id, Utc::now());

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
    if let Some(s) = &storage {
        s.snapshot(&format!("bucket add {name}"));
    }

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
    if let Some(s) = &storage {
        s.snapshot(&format!("bucket rename {old} {new}"));
    }

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
    if let Some(s) = &storage {
        s.snapshot(&format!("bucket delete {name}"));
    }

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

// ---------------------------------------------------------------------------
// Settings subcommands
// ---------------------------------------------------------------------------

fn run_settings_cmd(args: &[String]) -> io::Result<()> {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("show");
    match sub {
        "show" | "get" => cmd_settings_show(),
        "update" | "set" => cmd_settings_update(&args[1..]),
        other => die(&format!("Unknown settings command: {other}")),
    }
}

fn cmd_settings_show() -> io::Result<()> {
    let (_, _, settings) = load();
    print_json(&settings);
    Ok(())
}

fn parse_bool_flag(val: &str) -> bool {
    match val.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => true,
        "false" | "0" | "no" | "off" => false,
        _ => die(&format!(
            "Invalid boolean value: {val} (expected true/false)"
        )),
    }
}

fn cmd_settings_update(args: &[String]) -> io::Result<()> {
    let (storage, _, mut settings) = load();
    if let Some(s) = &storage {
        s.snapshot("settings update");
    }

    if let Some(v) = find_flag(args, "--owner-name") {
        settings.owner_name = v;
    }
    if let Some(v) = find_flag(args, "--ai-enabled") {
        settings.enabled = parse_bool_flag(&v);
    }
    if let Some(v) = find_flag(args, "--openai-api-key") {
        settings.openai_api_key = v;
    }
    if let Some(v) = find_flag(args, "--anthropic-api-key") {
        settings.anthropic_api_key = v;
    }
    if let Some(v) = find_flag(args, "--model") {
        settings.model = v;
    }
    if let Some(v) = find_flag(args, "--api-url") {
        settings.api_url = v;
    }
    if let Some(v) = find_flag(args, "--timeout") {
        settings.timeout_secs = v
            .parse::<u64>()
            .unwrap_or_else(|_| die(&format!("Invalid timeout: {v}")));
    }
    if let Some(v) = find_flag(args, "--show-backlog") {
        settings.show_backlog = parse_bool_flag(&v);
    }
    if let Some(v) = find_flag(args, "--show-todo") {
        settings.show_todo = parse_bool_flag(&v);
    }
    if let Some(v) = find_flag(args, "--show-in-progress") {
        settings.show_in_progress = parse_bool_flag(&v);
    }
    if let Some(v) = find_flag(args, "--show-done") {
        settings.show_done = parse_bool_flag(&v);
    }
    if let Some(v) = find_flag(args, "--email-suggestions") {
        settings.email_suggestions_enabled = parse_bool_flag(&v);
    }

    save_settings(&storage, &settings);
    print_json(&settings);
    Ok(())
}

// ---------------------------------------------------------------------------
// Undo / History subcommands
// ---------------------------------------------------------------------------

fn cmd_undo() -> io::Result<()> {
    let (storage, _, _) = load();
    let storage = storage.unwrap_or_else(|| die("No data directory found"));
    let label = storage.undo()?;
    print_json(&serde_json::json!({
        "restored_before": label,
    }));
    Ok(())
}

fn cmd_history() -> io::Result<()> {
    let (storage, _, _) = load();
    let storage = storage.unwrap_or_else(|| die("No data directory found"));
    let entries = storage.list_history();
    print_json(&entries);
    Ok(())
}

// ---------------------------------------------------------------------------
// Suggestions subcommands
// ---------------------------------------------------------------------------

fn run_suggestions_cmd(args: &[String]) -> io::Result<()> {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("list");
    match sub {
        "list" | "ls" => cmd_suggestions_list(),
        "sync" => cmd_suggestions_sync(&args[1..]),
        other => die(&format!("Unknown suggestions command: {other}")),
    }
}

fn cmd_suggestions_list() -> io::Result<()> {
    let (_, _, settings) = load();
    if !settings.email_suggestions_enabled {
        die("Email suggestions are not enabled. Enable it in the Suggestions tab (0) first.");
    }

    let emails = crate::mail::get_recent_emails(10).map_err(io::Error::other)?;

    let unread: Vec<_> = emails.iter().filter(|e| !e.is_read).collect();

    println!("Found {} unread emails:", unread.len());
    for email in unread {
        println!("\nID: {}", email.id);
        println!("From: {}", email.sender);
        println!("Subject: {}", email.subject);
        println!("Date: {}", email.date);

        // Try AI filtering
        if let Ok(Some(suggestion)) = crate::llm::filter_email_for_suggestions(
            &settings,
            &email.subject,
            &email.sender,
            email.content.as_deref().unwrap_or(""),
        ) {
            println!("✓ Actionable:");
            println!("  Title: {}", suggestion.title);
            println!("  Priority: {}", suggestion.priority);
            if !suggestion.description.is_empty() {
                println!("  Description: {}", suggestion.description);
            }
        } else {
            println!("✗ Not actionable (filtered out)");
        }
    }

    Ok(())
}

fn cmd_suggestions_sync(args: &[String]) -> io::Result<()> {
    let (storage, mut tasks, settings) = load();
    if let Some(s) = &storage {
        s.snapshot("suggestions sync");
    }

    if !settings.email_suggestions_enabled {
        die("Email suggestions are not enabled. Enable it in the Suggestions tab (0) first.");
    }

    let emails = crate::mail::get_recent_emails(10).map_err(io::Error::other)?;

    let limit = if let Some(limit_str) = find_flag(args, "--limit") {
        limit_str.parse::<usize>().unwrap_or(10)
    } else {
        10
    };

    let mut created = 0;
    for email in emails.iter().filter(|e| !e.is_read).take(limit) {
        if let Ok(Some(suggestion)) = crate::llm::filter_email_for_suggestions(
            &settings,
            &email.subject,
            &email.sender,
            email.content.as_deref().unwrap_or(""),
        ) {
            let priority = match suggestion.priority.to_ascii_lowercase().as_str() {
                "low" => Priority::Low,
                "medium" => Priority::Medium,
                "high" => Priority::High,
                "critical" => Priority::Critical,
                _ => Priority::Medium,
            };

            let now = Utc::now();
            let bucket = settings
                .buckets
                .first()
                .map(|b| b.name.clone())
                .unwrap_or_else(|| "Unassigned".to_string());

            let mut task = Task::new(bucket, suggestion.title.clone(), now);
            task.description = format!(
                "{}\n\nFrom: {}\nEmail ID: {}",
                suggestion.description, email.sender, email.id
            );
            task.priority = priority;
            task.progress = Progress::Backlog;

            println!("Created task from email: {}", task.title);
            tasks.push(task);
            created += 1;
        }
    }

    save_tasks(&storage, &tasks);
    print_json(&serde_json::json!({
        "created": created,
    }));
    Ok(())
}
