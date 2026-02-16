use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Progress {
    Backlog,
    Todo,
    InProgress,
    Done,
}

impl Progress {
    pub const ALL: [Progress; 4] = [
        Progress::Backlog,
        Progress::Todo,
        Progress::InProgress,
        Progress::Done,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Progress::Backlog => "Backlog",
            Progress::Todo => "Todo",
            Progress::InProgress => "In progress",
            Progress::Done => "Done",
        }
    }

    pub fn stage_index(self) -> usize {
        match self {
            Progress::Backlog => 0,
            Progress::Todo => 1,
            Progress::InProgress => 2,
            Progress::Done => 3,
        }
    }

    pub fn advance(self) -> Progress {
        match self {
            Progress::Backlog => Progress::Todo,
            Progress::Todo => Progress::InProgress,
            Progress::InProgress => Progress::Done,
            Progress::Done => Progress::Done,
        }
    }

    pub fn retreat(self) -> Progress {
        match self {
            Progress::Backlog => Progress::Backlog,
            Progress::Todo => Progress::Backlog,
            Progress::InProgress => Progress::Todo,
            Progress::Done => Progress::InProgress,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Priority {
    Low,
    Medium,
    High,
    Critical,
}

impl Priority {
    pub fn title(self) -> &'static str {
        match self {
            Priority::Low => "Low",
            Priority::Medium => "Medium",
            Priority::High => "High",
            Priority::Critical => "Critical",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: Uuid,
    pub bucket: String,
    pub title: String,
    pub description: String,
    pub dependencies: Vec<Uuid>,
    #[serde(default)]
    pub parent_id: Option<Uuid>,
    pub progress: Progress,
    pub priority: Priority,
    pub due_date: Option<NaiveDate>,
    pub created_at: DateTime<Utc>,
    pub start_date: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

impl Task {
    pub fn new(bucket: String, title: String, now: DateTime<Utc>) -> Task {
        Task {
            id: Uuid::new_v4(),
            bucket,
            title,
            description: String::new(),
            dependencies: Vec::new(),
            parent_id: None,
            progress: Progress::Backlog,
            priority: Priority::Medium,
            due_date: None,
            created_at: now,
            start_date: None,
            updated_at: now,
        }
    }

    pub fn is_child(&self) -> bool {
        self.parent_id.is_some()
    }

    pub fn set_progress(&mut self, next: Progress, now: DateTime<Utc>) {
        if self.progress == next {
            return;
        }

        // Track start date specifically when Todo -> In progress.
        if self.progress == Progress::Todo && next == Progress::InProgress {
            self.start_date.get_or_insert(now);
        }

        self.progress = next;
        self.updated_at = now;
    }

    pub fn advance_progress(&mut self, now: DateTime<Utc>) {
        let next = self.progress.advance();
        self.set_progress(next, now);
    }

    pub fn retreat_progress(&mut self, now: DateTime<Utc>) {
        let next = self.progress.retreat();
        self.set_progress(next, now);
    }
}

pub fn children_of(tasks: &[Task], parent_id: Uuid) -> Vec<usize> {
    tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.parent_id == Some(parent_id))
        .map(|(i, _)| i)
        .collect()
}
