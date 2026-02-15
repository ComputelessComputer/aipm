use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::Task;

#[derive(Debug, Serialize, Deserialize)]
struct Store {
    version: u32,
    tasks: Vec<Task>,
}

#[derive(Debug, Clone)]
pub struct Storage {
    path: PathBuf,
}

impl Storage {
    pub fn new() -> Option<Storage> {
        let dir = data_dir()?;
        Some(Storage {
            path: dir.join("tasks.json"),
        })
    }

    pub fn load_tasks(&self) -> io::Result<Vec<Task>> {
        if !self.path.is_file() {
            return Ok(Vec::new());
        }
        let contents = fs::read_to_string(&self.path)?;
        let store: Store = serde_json::from_str(&contents)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
        Ok(store.tasks)
    }

    pub fn save_tasks(&self, tasks: &[Task]) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let store = Store {
            version: 1,
            tasks: tasks.to_vec(),
        };
        let json = serde_json::to_string_pretty(&store)
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;

        let tmp_path = self.path.with_extension("json.tmp");
        fs::write(&tmp_path, json)?;
        fs::rename(&tmp_path, &self.path)?;
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
