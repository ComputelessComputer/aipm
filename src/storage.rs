use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::Task;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSettings {
    pub enabled: bool,
    pub api_key: String,
    pub model: String,
    pub api_url: String,
    pub timeout_secs: u64,
    #[serde(default = "default_owner_name")]
    pub owner_name: String,
}

fn default_owner_name() -> String {
    "John".to_string()
}

impl Default for AiSettings {
    fn default() -> Self {
        AiSettings {
            enabled: true,
            api_key: String::new(),
            model: "gpt-5.2-chat-latest".to_string(),
            api_url: "https://api.openai.com/v1/chat/completions".to_string(),
            timeout_secs: 30,
            owner_name: "John".to_string(),
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

    pub fn save_tasks(&self, tasks: &[Task]) -> io::Result<()> {
        let path = self.dir.join("tasks.json");
        fs::create_dir_all(&self.dir)?;

        let store = Store {
            version: 1,
            tasks: tasks.to_vec(),
        };
        let json = serde_json::to_string_pretty(&store)
            .map_err(|err| io::Error::other(err.to_string()))?;

        let tmp_path = path.with_extension("json.tmp");
        fs::write(&tmp_path, json)?;
        fs::rename(&tmp_path, &path)?;
        Ok(())
    }

    pub fn load_settings(&self) -> io::Result<AiSettings> {
        let path = self.dir.join("settings.json");
        if !path.is_file() {
            return Ok(AiSettings::default());
        }
        let contents = fs::read_to_string(&path)?;
        let settings: AiSettings = serde_json::from_str(&contents)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &AiSettings) -> io::Result<()> {
        let path = self.dir.join("settings.json");
        fs::create_dir_all(&self.dir)?;
        let json = serde_json::to_string_pretty(settings)
            .map_err(|err| io::Error::other(err.to_string()))?;
        let tmp_path = path.with_extension("json.tmp");
        fs::write(&tmp_path, json)?;
        fs::rename(&tmp_path, &path)?;
        Ok(())
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
