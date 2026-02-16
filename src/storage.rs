use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;

use fracturedjson::Formatter as FjFormatter;
use serde::{Deserialize, Serialize};

use crate::model::{Progress, Task};

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
}

fn default_owner_name() -> String {
    "John".to_string()
}

fn default_true() -> bool {
    true
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
            // Best-effort: if the key looks like an Anthropic key, put it there.
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

#[derive(Debug, Serialize, Deserialize)]
struct Store {
    version: u32,
    tasks: Vec<Task>,
}

#[derive(Debug, Clone)]
pub struct Storage {
    dir: PathBuf,
}

impl Storage {
    pub fn new() -> Option<Storage> {
        let dir = data_dir()?;
        Some(Storage { dir })
    }

    pub fn load_tasks(&self) -> io::Result<Vec<Task>> {
        let path = self.dir.join("tasks.json");
        if !path.is_file() {
            return Ok(Vec::new());
        }
        let contents = fs::read_to_string(&path)?;
        let store: Store = serde_json::from_str(&contents)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
        Ok(store.tasks)
    }

    /// Reload tasks from disk, returning the latest state.
    /// Call this before processing new input to pick up external changes.
    pub fn reload_tasks(&self) -> io::Result<Vec<Task>> {
        self.load_tasks()
    }

    pub fn save_tasks(&self, tasks: &[Task]) -> io::Result<()> {
        let path = self.dir.join("tasks.json");
        fs::create_dir_all(&self.dir)?;

        let store = Store {
            version: 1,
            tasks: tasks.to_vec(),
        };
        let json = serde_json::to_string(&store)
            .map_err(|err| io::Error::other(err.to_string()))?;
        let formatted = format_json(&json);

        let tmp_path = path.with_extension("json.tmp");
        fs::write(&tmp_path, formatted)?;
        fs::rename(&tmp_path, &path)?;
        Ok(())
    }

    pub fn load_settings(&self) -> io::Result<AiSettings> {
        let path = self.dir.join("settings.json");
        if !path.is_file() {
            return Ok(AiSettings::default());
        }
        let contents = fs::read_to_string(&path)?;
        let mut settings: AiSettings = serde_json::from_str(&contents)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
        settings.migrate_legacy_key();
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &AiSettings) -> io::Result<()> {
        let path = self.dir.join("settings.json");
        fs::create_dir_all(&self.dir)?;
        let json = serde_json::to_string(settings)
            .map_err(|err| io::Error::other(err.to_string()))?;
        let formatted = format_json(&json);
        let tmp_path = path.with_extension("json.tmp");
        fs::write(&tmp_path, formatted)?;
        fs::rename(&tmp_path, &path)?;
        Ok(())
    }
}

/// Format JSON using FracturedJson for compact, human-readable output.
fn format_json(json: &str) -> String {
    let mut fj = FjFormatter::new();
    fj.options.max_total_line_length = 100;
    fj.options.max_inline_complexity = 1;
    fj.options.indent_spaces = 2;
    match fj.reformat(json, 0) {
        Ok(formatted) => formatted,
        Err(_) => json.to_string(),
    }
}

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
