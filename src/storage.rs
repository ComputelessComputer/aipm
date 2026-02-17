use std::collections::HashSet;
use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::{BucketDef, Priority, Progress, Task};

// ---------------------------------------------------------------------------
// AiSettings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSettings {
    pub enabled: bool,
    #[serde(default)]
    pub openai_api_key: String,
    #[serde(default)]
    pub anthropic_api_key: String,
    /// Legacy single key — migrated into the per-provider fields on load.
    #[serde(default, skip_serializing)]
    api_key: String,
    pub model: String,
    pub api_url: String,
    pub timeout_secs: u64,
    #[serde(default = "default_owner_name")]
    pub owner_name: String,
    #[serde(default = "default_true")]
    pub show_backlog: bool,
    #[serde(default = "default_true")]
    pub show_todo: bool,
    #[serde(default = "default_true")]
    pub show_in_progress: bool,
    #[serde(default)]
    pub show_done: bool,
    #[serde(default = "default_buckets")]
    pub buckets: Vec<BucketDef>,
}

fn default_owner_name() -> String {
    "John".to_string()
}

fn default_true() -> bool {
    true
}

fn default_buckets() -> Vec<BucketDef> {
    vec![
        BucketDef {
            name: "Personal".to_string(),
            description: Some("Your own tasks, reviews, and personal direction".to_string()),
        },
        BucketDef {
            name: "Team".to_string(),
            description: Some("Onboarding, coordination, guiding your crew".to_string()),
        },
        BucketDef {
            name: "Admin".to_string(),
            description: Some("Taxes, accounting, admin chores".to_string()),
        },
    ]
}

impl Default for AiSettings {
    fn default() -> Self {
        AiSettings {
            enabled: true,
            openai_api_key: String::new(),
            anthropic_api_key: String::new(),
            api_key: String::new(),
            model: "claude-sonnet-4-5".to_string(),
            api_url: String::new(),
            timeout_secs: 60,
            owner_name: "John".to_string(),
            show_backlog: true,
            show_todo: true,
            show_in_progress: true,
            show_done: false,
            buckets: default_buckets(),
        }
    }
}

impl AiSettings {
    pub fn is_progress_visible(&self, progress: Progress) -> bool {
        match progress {
            Progress::Backlog => self.show_backlog,
            Progress::Todo => self.show_todo,
            Progress::InProgress => self.show_in_progress,
            Progress::Done => self.show_done,
        }
    }

    /// Migrate the legacy single `api_key` into per-provider fields.
    pub fn migrate_legacy_key(&mut self) {
        if !self.api_key.is_empty() {
            if self.api_key.starts_with("sk-ant-") {
                if self.anthropic_api_key.is_empty() {
                    self.anthropic_api_key = self.api_key.clone();
                }
            } else if self.openai_api_key.is_empty() {
                self.openai_api_key = self.api_key.clone();
            }
            self.api_key.clear();
        }
    }
}

// ---------------------------------------------------------------------------
// Legacy JSON format (for migration)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct LegacyStore {
    #[allow(dead_code)]
    version: u32,
    tasks: Vec<Task>,
}

// ---------------------------------------------------------------------------
// Front-matter serde types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct TaskFrontMatter {
    id: String,
    title: String,
    bucket: String,
    progress: String,
    priority: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    due_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    dependencies: Vec<String>,
    created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_date: Option<String>,
    updated_at: String,
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Storage {
    dir: PathBuf,
}

impl Storage {
    pub fn new() -> Option<Storage> {
        let dir = data_dir()?;
        let storage = Storage { dir };
        // Auto-migrate from legacy JSON if needed.
        if let Err(err) = storage.migrate_from_json() {
            eprintln!("Migration warning: {err}");
        }
        // Migrate "John" / "John-only" buckets to "Personal".
        if let Err(err) = storage.migrate_bucket_names() {
            eprintln!("Bucket migration warning: {err}");
        }
        Some(storage)
    }

    // -- Tasks ---------------------------------------------------------------

    pub fn load_tasks(&self) -> io::Result<Vec<Task>> {
        let tasks_dir = self.dir.join("tasks");
        if !tasks_dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut tasks = Vec::new();
        for entry in fs::read_dir(&tasks_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let content = fs::read_to_string(&path)?;
            match parse_task_file(&content) {
                Ok(task) => tasks.push(task),
                Err(err) => {
                    eprintln!("Warning: skipping {}: {}", path.display(), err);
                }
            }
        }
        Ok(tasks)
    }

    /// Reload tasks from disk, returning the latest state.
    pub fn reload_tasks(&self) -> io::Result<Vec<Task>> {
        self.load_tasks()
    }

    pub fn save_tasks(&self, tasks: &[Task]) -> io::Result<()> {
        let tasks_dir = self.dir.join("tasks");
        fs::create_dir_all(&tasks_dir)?;

        // Collect expected filenames so we can remove stale files.
        let mut expected_files: HashSet<String> = HashSet::new();

        for task in tasks {
            let filename = task_filename(task);
            expected_files.insert(filename.clone());

            let path = tasks_dir.join(&filename);
            let content = serialize_task_file(task);

            // Only write if content changed (avoid unnecessary disk writes).
            if path.is_file() {
                if let Ok(existing) = fs::read_to_string(&path) {
                    if existing == content {
                        continue;
                    }
                }
            }

            let tmp_path = path.with_extension("md.tmp");
            fs::write(&tmp_path, &content)?;
            fs::rename(&tmp_path, &path)?;
        }

        // Remove stale files: deleted tasks or renamed slugs.
        if let Ok(entries) = fs::read_dir(&tasks_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.ends_with(".md") {
                    continue;
                }
                if !expected_files.contains(&name) {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }

        Ok(())
    }

    // -- Settings ------------------------------------------------------------

    pub fn load_settings(&self) -> io::Result<AiSettings> {
        // Try YAML first, then fall back to legacy JSON.
        let yaml_path = self.dir.join("settings.yaml");
        if yaml_path.is_file() {
            let contents = fs::read_to_string(&yaml_path)?;
            let mut settings: AiSettings = serde_yaml::from_str(&contents)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
            settings.migrate_legacy_key();
            return Ok(settings);
        }

        let json_path = self.dir.join("settings.json");
        if json_path.is_file() {
            let contents = fs::read_to_string(&json_path)?;
            let mut settings: AiSettings = serde_json::from_str(&contents)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
            settings.migrate_legacy_key();
            // Migrate: save as YAML and archive JSON.
            if let Ok(yaml) = serde_yaml::to_string(&settings) {
                let _ = fs::write(&yaml_path, yaml);
                let _ = fs::rename(&json_path, json_path.with_extension("json.bak"));
            }
            return Ok(settings);
        }

        Ok(AiSettings::default())
    }

    pub fn save_settings(&self, settings: &AiSettings) -> io::Result<()> {
        let path = self.dir.join("settings.yaml");
        fs::create_dir_all(&self.dir)?;
        let yaml =
            serde_yaml::to_string(settings).map_err(|err| io::Error::other(err.to_string()))?;
        let tmp_path = path.with_extension("yaml.tmp");
        fs::write(&tmp_path, yaml)?;
        fs::rename(&tmp_path, &path)?;
        Ok(())
    }

    // -- Migration -----------------------------------------------------------

    fn migrate_from_json(&self) -> io::Result<()> {
        let json_path = self.dir.join("tasks.json");
        let tasks_dir = self.dir.join("tasks");

        if !json_path.is_file() {
            return Ok(());
        }

        // Only migrate if tasks/ dir is empty or doesn't exist.
        if tasks_dir.is_dir() {
            let has_md = fs::read_dir(&tasks_dir)?
                .flatten()
                .any(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("md"));
            if has_md {
                return Ok(());
            }
        }

        let contents = fs::read_to_string(&json_path)?;
        let store: LegacyStore = serde_json::from_str(&contents)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;

        fs::create_dir_all(&tasks_dir)?;

        for task in &store.tasks {
            let filename = task_filename(task);
            let path = tasks_dir.join(&filename);
            let content = serialize_task_file(task);
            fs::write(&path, content)?;
        }

        // Archive the old JSON file.
        let bak = json_path.with_extension("json.bak");
        fs::rename(&json_path, &bak)?;

        eprintln!(
            "Migrated {} tasks from tasks.json -> tasks/ directory",
            store.tasks.len()
        );

        Ok(())
    }

    /// Rename legacy "John" and "John-only" buckets to "Personal".
    fn migrate_bucket_names(&self) -> io::Result<()> {
        let tasks_dir = self.dir.join("tasks");
        if !tasks_dir.is_dir() {
            return Ok(());
        }

        let old_names: &[&str] = &["John", "John-only"];
        let new_name = "Personal";

        for entry in fs::read_dir(&tasks_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let content = fs::read_to_string(&path)?;
            let mut changed = content.clone();
            for old in old_names {
                // Match the YAML front-matter line exactly.
                let from = format!("bucket: {}", old);
                let to = format!("bucket: {}", new_name);
                if changed.contains(&from) {
                    changed = changed.replace(&from, &to);
                }
            }
            if changed != content {
                fs::write(&path, &changed)?;
            }
        }

        // Also migrate bucket definitions in settings.
        let yaml_path = self.dir.join("settings.yaml");
        if yaml_path.is_file() {
            let raw = fs::read_to_string(&yaml_path)?;
            let mut settings: AiSettings = serde_yaml::from_str(&raw)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            let mut dirty = false;
            for bucket in &mut settings.buckets {
                if old_names
                    .iter()
                    .any(|o| bucket.name.eq_ignore_ascii_case(o))
                {
                    bucket.name = new_name.to_string();
                    dirty = true;
                }
            }
            // Deduplicate: if migration created two "Personal" entries, keep only the first.
            if dirty {
                let mut seen = false;
                settings.buckets.retain(|b| {
                    if b.name == new_name {
                        if seen {
                            return false;
                        }
                        seen = true;
                    }
                    true
                });
                let yaml = serde_yaml::to_string(&settings)
                    .map_err(|e| io::Error::other(e.to_string()))?;
                fs::write(&yaml_path, yaml)?;
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// File naming
// ---------------------------------------------------------------------------

fn short_id(id: Uuid) -> String {
    id.to_string().chars().take(8).collect()
}

fn slug_from_title(title: &str) -> String {
    let mut slug = String::new();
    let mut prev_hyphen = false;

    for ch in title.chars().take(200) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_hyphen = false;
        } else if !prev_hyphen && !slug.is_empty() {
            slug.push('-');
            prev_hyphen = true;
        }
    }

    let trimmed = slug.trim_end_matches('-');
    let truncated: String = trimmed.chars().take(50).collect();
    truncated.trim_end_matches('-').to_string()
}

fn task_filename(task: &Task) -> String {
    let prefix = short_id(task.id);
    let slug = slug_from_title(&task.title);
    if slug.is_empty() {
        format!("{}.md", prefix)
    } else {
        format!("{}-{}.md", prefix, slug)
    }
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

fn progress_to_str(p: Progress) -> &'static str {
    match p {
        Progress::Backlog => "Backlog",
        Progress::Todo => "Todo",
        Progress::InProgress => "InProgress",
        Progress::Done => "Done",
    }
}

fn priority_to_str(p: Priority) -> &'static str {
    match p {
        Priority::Low => "Low",
        Priority::Medium => "Medium",
        Priority::High => "High",
        Priority::Critical => "Critical",
    }
}

fn serialize_task_file(task: &Task) -> String {
    let fm = TaskFrontMatter {
        id: task.id.to_string(),
        title: task.title.clone(),
        bucket: task.bucket.clone(),
        progress: progress_to_str(task.progress).to_string(),
        priority: priority_to_str(task.priority).to_string(),
        due_date: task.due_date.map(|d| d.format("%Y-%m-%d").to_string()),
        parent_id: task.parent_id.map(|id| id.to_string()),
        dependencies: task.dependencies.iter().map(|id| id.to_string()).collect(),
        created_at: task.created_at.to_rfc3339(),
        start_date: task.start_date.map(|dt| dt.to_rfc3339()),
        updated_at: task.updated_at.to_rfc3339(),
    };

    let yaml = serde_yaml::to_string(&fm).unwrap_or_default();
    let description = task.description.trim();

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&yaml);
    out.push_str("---\n");
    if !description.is_empty() {
        out.push_str(description);
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

fn parse_task_file(content: &str) -> Result<Task, String> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Err("missing front matter delimiter".to_string());
    }
    let after_first = &trimmed[3..].trim_start_matches(['\r', '\n']);
    let end = after_first
        .find("\n---")
        .ok_or_else(|| "missing closing front matter delimiter".to_string())?;

    let yaml_str = &after_first[..end];
    let body_start = end + 4; // skip "\n---"
    let body = after_first
        .get(body_start..)
        .unwrap_or("")
        .trim_start_matches(['\r', '\n']);

    let fm: TaskFrontMatter =
        serde_yaml::from_str(yaml_str).map_err(|err| format!("YAML parse error: {err}"))?;

    let id = Uuid::parse_str(&fm.id).map_err(|err| format!("invalid id: {err}"))?;

    let bucket = fm.bucket.clone();

    let progress = match fm.progress.to_ascii_lowercase().as_str() {
        "backlog" => Progress::Backlog,
        "todo" => Progress::Todo,
        "inprogress" | "in progress" | "in-progress" => Progress::InProgress,
        "done" => Progress::Done,
        other => return Err(format!("unknown progress: {other}")),
    };

    let priority = match fm.priority.to_ascii_lowercase().as_str() {
        "low" => Priority::Low,
        "medium" | "med" => Priority::Medium,
        "high" => Priority::High,
        "critical" | "crit" => Priority::Critical,
        other => return Err(format!("unknown priority: {other}")),
    };

    let due_date = fm
        .due_date
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d"))
        .transpose()
        .map_err(|err| format!("invalid due_date: {err}"))?;

    let parent_id = fm
        .parent_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|err| format!("invalid parent_id: {err}"))?;

    let dependencies: Vec<Uuid> = fm
        .dependencies
        .iter()
        .filter_map(|s| Uuid::parse_str(s).ok())
        .collect();

    let created_at = DateTime::parse_from_rfc3339(&fm.created_at)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|err| format!("invalid created_at: {err}"))?;

    let start_date = fm
        .start_date
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| DateTime::parse_from_rfc3339(s).map(|dt| dt.with_timezone(&Utc)))
        .transpose()
        .map_err(|err| format!("invalid start_date: {err}"))?;

    let updated_at = DateTime::parse_from_rfc3339(&fm.updated_at)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|err| format!("invalid updated_at: {err}"))?;

    Ok(Task {
        id,
        bucket,
        title: fm.title,
        description: body.trim_end().to_string(),
        dependencies,
        parent_id,
        progress,
        priority,
        due_date,
        created_at,
        start_date,
        updated_at,
    })
}

// ---------------------------------------------------------------------------
// Snapshots (undo history)
// ---------------------------------------------------------------------------

const MAX_SNAPSHOTS: usize = 50;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub label: String,
    pub timestamp: DateTime<Utc>,
    pub tasks: Vec<Task>,
    pub settings: AiSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub seq: u64,
    pub label: String,
    pub timestamp: DateTime<Utc>,
}

impl Storage {
    fn history_dir(&self) -> PathBuf {
        self.dir.join("history")
    }

    pub fn snapshot(&self, label: &str) {
        if let Err(err) = self.snapshot_inner(label) {
            eprintln!("Snapshot warning: {err}");
        }
    }

    fn snapshot_inner(&self, label: &str) -> io::Result<()> {
        let hist = self.history_dir();
        fs::create_dir_all(&hist)?;

        let tasks = self.load_tasks().unwrap_or_default();
        let settings = self.load_settings().unwrap_or_default();
        let now = Utc::now();
        let seq = self.next_seq();

        let snap = Snapshot {
            label: label.to_string(),
            timestamp: now,
            tasks,
            settings,
        };

        let filename = format!("{:05}-{}.json", seq, now.format("%Y%m%dT%H%M%S"));
        let path = hist.join(&filename);
        let json = serde_json::to_string(&snap).map_err(|err| io::Error::other(err.to_string()))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &path)?;

        self.trim_history();
        Ok(())
    }

    pub fn undo(&self) -> io::Result<String> {
        let files = self.sorted_snapshot_files();
        let latest = files
            .last()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "No undo history available"))?;

        let content = fs::read_to_string(latest)?;
        let snap: Snapshot = serde_json::from_str(&content)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;

        self.save_tasks(&snap.tasks)?;
        self.save_settings(&snap.settings)?;

        let label = snap.label.clone();
        fs::remove_file(latest)?;
        Ok(label)
    }

    pub fn list_history(&self) -> Vec<HistoryEntry> {
        let files = self.sorted_snapshot_files();
        let mut entries = Vec::new();
        for path in &files {
            if let Ok(content) = fs::read_to_string(path) {
                if let Ok(snap) = serde_json::from_str::<Snapshot>(&content) {
                    let seq = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .and_then(|n| n.split('-').next())
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(0);
                    entries.push(HistoryEntry {
                        seq,
                        label: snap.label,
                        timestamp: snap.timestamp,
                    });
                }
            }
        }
        entries
    }

    fn sorted_snapshot_files(&self) -> Vec<PathBuf> {
        let hist = self.history_dir();
        let mut files: Vec<PathBuf> = fs::read_dir(&hist)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
            .map(|e| e.path())
            .collect();
        files.sort();
        files
    }

    fn next_seq(&self) -> u64 {
        self.sorted_snapshot_files()
            .last()
            .and_then(|p| {
                p.file_name()?
                    .to_str()?
                    .split('-')
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
            .map(|n| n + 1)
            .unwrap_or(1)
    }

    fn trim_history(&self) {
        let files = self.sorted_snapshot_files();
        if files.len() > MAX_SNAPSHOTS {
            for path in &files[..files.len() - MAX_SNAPSHOTS] {
                let _ = fs::remove_file(path);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Data directory
// ---------------------------------------------------------------------------

fn data_dir() -> Option<PathBuf> {
    if let Ok(path) = env::var("AIPM_DATA_DIR") {
        if !path.trim().is_empty() {
            return Some(PathBuf::from(path));
        }
    }

    if let Ok(path) = env::var("XDG_DATA_HOME") {
        if !path.trim().is_empty() {
            return Some(PathBuf::from(path).join("aipm"));
        }
    }

    if let Ok(home) = env::var("HOME") {
        let app_support = PathBuf::from(&home)
            .join("Library")
            .join("Application Support");
        if app_support.is_dir() {
            return Some(app_support.join("aipm"));
        }

        return Some(
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("aipm"),
        );
    }

    None
}
