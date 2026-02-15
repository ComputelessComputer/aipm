use std::collections::HashSet;
use std::env;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::model::{Bucket, Priority, Progress};
use crate::storage::AiSettings;

#[derive(Debug, Clone)]
pub struct ContextTask {
    pub id: Uuid,
    pub bucket: Bucket,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct AiJob {
    pub task_id: Uuid,
    pub title: String,
    pub suggested_bucket: Bucket,
    pub context: Vec<ContextTask>,
    pub lock_bucket: bool,
    pub lock_priority: bool,
    pub lock_due_date: bool,
    /// When set, this is an edit-in-place job (`@instruction`).
    pub edit_instruction: Option<String>,
    /// Formatted snapshot of the task being edited.
    pub task_snapshot: Option<String>,
    /// When set, this is a triage job: AI decides create vs update.
    pub triage_input: Option<String>,
    /// Pre-formatted full task list for triage context.
    pub triage_context: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TaskUpdate {
    pub is_edit: bool,
    pub title: Option<String>,
    pub bucket: Option<Bucket>,
    pub description: Option<String>,
    pub progress: Option<Progress>,
    pub priority: Option<Priority>,
    pub due_date: Option<NaiveDate>,
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum TriageAction {
    /// AI decided to create a new task.
    Create,
    /// AI decided to update an existing task (id prefix).
    Update(String),
    /// AI decided to delete an existing task (id prefix).
    Delete(String),
}

#[derive(Debug, Clone)]
pub struct AiResult {
    pub task_id: Uuid,
    pub update: TaskUpdate,
    pub error: Option<String>,
    pub triage_action: Option<TriageAction>,
}

#[derive(Debug)]
pub struct AiRuntime {
    job_tx: Sender<AiJob>,
    result_rx: Receiver<AiResult>,
}

#[derive(Debug, Clone)]
struct OpenAiConfig {
    api_url: String,
    model: String,
    api_key: String,
    timeout: Duration,
}

impl AiRuntime {
    pub fn from_settings(settings: &AiSettings) -> Option<AiRuntime> {
        if !settings.enabled {
            return None;
        }

        let key = if !settings.api_key.trim().is_empty() {
            settings.api_key.clone()
        } else {
            env::var("OPENAI_API_KEY").ok()?
        };

        let api_url = if !settings.api_url.trim().is_empty() {
            settings.api_url.clone()
        } else {
            env::var("AIPM_OPENAI_URL")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "https://api.openai.com/v1/chat/completions".to_string())
        };

        let model = if !settings.model.trim().is_empty() {
            settings.model.clone()
        } else {
            env::var("AIPM_OPENAI_MODEL")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "gpt-5.2-chat-latest".to_string())
        };

        let timeout = Duration::from_secs(if settings.timeout_secs > 0 {
            settings.timeout_secs
        } else {
            30
        });

        let cfg = OpenAiConfig {
            api_url,
            model,
            api_key: key,
            timeout,
        };

        let (job_tx, job_rx) = mpsc::channel::<AiJob>();
        let (result_tx, result_rx) = mpsc::channel::<AiResult>();

        thread::spawn(move || worker_loop(cfg, job_rx, result_tx));

        Some(AiRuntime { job_tx, result_rx })
    }

    pub fn enqueue(&self, job: AiJob) {
        let _ = self.job_tx.send(job);
    }

    pub fn drain(&self) -> Vec<AiResult> {
        let mut out = Vec::new();
        while let Ok(result) = self.result_rx.try_recv() {
            out.push(result);
        }
        out
    }
}

fn worker_loop(cfg: OpenAiConfig, job_rx: Receiver<AiJob>, result_tx: Sender<AiResult>) {
    for job in job_rx {
        let result = enrich_with_openai(&cfg, &job);
        let _ = result_tx.send(result);
    }
}

fn enrich_with_openai(cfg: &OpenAiConfig, job: &AiJob) -> AiResult {
    if let Some(raw_input) = &job.triage_input {
        return triage_with_openai(cfg, job, raw_input);
    }
    if let Some(instruction) = &job.edit_instruction {
        return edit_task_with_openai(cfg, job, instruction);
    }

    let system = "You are an expert AI project manager. Output ONLY valid JSON. No markdown.";

    let mut context_lines = String::new();
    for task in job.context.iter().take(40) {
        context_lines.push_str(&format!(
            "- {} [{}] {}\n",
            short_id(task.id),
            task.bucket.title(),
            task.title
        ));
    }

    let lock_line = format!(
        "Locked fields: bucket={} priority={} due_date={}",
        job.lock_bucket, job.lock_priority, job.lock_due_date
    );

    let user = format!(
        "New task title: {}\nSuggested bucket: {}\n{}\n\nExisting tasks you may depend on (id_prefix [bucket] title):\n{}\nReturn JSON with keys:\n{{\n  \"bucket\": \"Team\"|\"John\"|\"Admin\",\n  \"description\": string,\n  \"priority\": \"Low\"|\"Medium\"|\"High\"|\"Critical\",\n  \"due_date\": \"YYYY-MM-DD\" | null,\n  \"dependencies\": [\"id_prefix\", ...]\n}}\nRules:\n- If a field is locked, keep it aligned with the suggested value (bucket) or output null/Medium (due/priority) as appropriate.\n- If unsure, keep bucket as suggested.\n- Dependencies must use the provided id_prefix values.\n",
        job.title.trim(),
        job.suggested_bucket.title(),
        lock_line,
        context_lines
    );

    let body = json!({
        "model": cfg.model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user}
        ]
    });

    let resp = ureq::post(&cfg.api_url)
        .set("Authorization", &format!("Bearer {}", cfg.api_key))
        .set("Content-Type", "application/json")
        .timeout(cfg.timeout)
        .send_string(&body.to_string());

    let text = match resp {
        Ok(r) => match r.into_string() {
            Ok(s) => s,
            Err(err) => {
                return AiResult {
                    task_id: job.task_id,
                    update: TaskUpdate::default(),
                    error: Some(format!("AI response read failed: {err}")),
                    triage_action: None,
                }
            }
        },
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            return AiResult {
                task_id: job.task_id,
                update: TaskUpdate::default(),
                error: Some(format!("AI HTTP {}: {}", code, truncate(&body, 200))),
                triage_action: None,
            };
        }
        Err(ureq::Error::Transport(t)) => {
            return AiResult {
                task_id: job.task_id,
                update: TaskUpdate::default(),
                error: Some(format!("AI transport error: {t}")),
                triage_action: None,
            };
        }
    };

    let chat: ChatResponse = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(err) => {
            return AiResult {
                task_id: job.task_id,
                update: TaskUpdate::default(),
                error: Some(format!("AI JSON parse failed: {err}")),
                    triage_action: None,
            }
        }
    };

    let content = chat
        .choices
        .get(0)
        .and_then(|c| c.message.content.as_deref())
        .unwrap_or("");

    let json_text = extract_json_object(content).unwrap_or_else(|| content.trim().to_string());

    let enriched: Enriched = match serde_json::from_str(&json_text) {
        Ok(v) => v,
        Err(err) => {
            return AiResult {
                task_id: job.task_id,
                update: TaskUpdate::default(),
                error: Some(format!(
                    "AI output not valid JSON ({err}): {}",
                    truncate(content, 200)
                )),
                triage_action: None,
            }
        }
    };

    let allowed: HashSet<String> = job
        .context
        .iter()
        .map(|t| short_id(t.id))
        .collect::<HashSet<_>>();

    let mut update = TaskUpdate::default();

    if !job.lock_bucket {
        if let Some(bucket) = enriched.bucket.as_deref().and_then(parse_bucket) {
            update.bucket = Some(bucket);
        }
    }

    let description = enriched
        .description
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| truncate(s, 400).to_string());
    update.description = description;

    if !job.lock_priority {
        if let Some(prio) = enriched
            .priority
            .as_deref()
            .and_then(|s| parse_priority(s.trim()))
        {
            update.priority = Some(prio);
        }
    }

    if !job.lock_due_date {
        if let Some(date_str) = enriched.due_date.as_deref().map(|s| s.trim()) {
            if !date_str.is_empty() {
                if let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                    update.due_date = Some(date);
                }
            }
        }
    }

    if let Some(deps) = enriched.dependencies {
        let mut out = Vec::new();
        for dep in deps.into_iter().take(8) {
            let prefix = dep.trim();
            if prefix.len() < 4 {
                continue;
            }
            let key = prefix.chars().take(8).collect::<String>();
            if allowed.contains(&key) && !out.contains(&key) {
                out.push(key);
            }
        }
        update.dependencies = out;
    }

    AiResult {
        task_id: job.task_id,
        update,
        error: None,
        triage_action: None,
    }
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Debug, Deserialize)]
struct Message {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Enriched {
    title: Option<String>,
    bucket: Option<String>,
    description: Option<String>,
    progress: Option<String>,
    priority: Option<String>,
    due_date: Option<String>,
    dependencies: Option<Vec<String>>,
}

fn extract_json_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(text[start..=end].to_string())
}

fn parse_bucket(input: &str) -> Option<Bucket> {
    match input.to_ascii_lowercase().as_str() {
        "team" => Some(Bucket::Team),
        "john" | "john-only" | "john_only" | "johnonly" => Some(Bucket::John),
        "admin" => Some(Bucket::Admin),
        _ => None,
    }
}

fn parse_priority(input: &str) -> Option<Priority> {
    match input.to_ascii_lowercase().as_str() {
        "low" => Some(Priority::Low),
        "med" | "medium" => Some(Priority::Medium),
        "high" => Some(Priority::High),
        "crit" | "critical" => Some(Priority::Critical),
        _ => None,
    }
}

fn short_id(id: Uuid) -> String {
    id.to_string().chars().take(8).collect::<String>()
}

fn edit_task_with_openai(cfg: &OpenAiConfig, job: &AiJob, instruction: &str) -> AiResult {
    let system = "You are an expert AI project manager. Modify the given task based on the user instruction. Output ONLY valid JSON. No markdown.";

    let snapshot = job.task_snapshot.as_deref().unwrap_or("");

    let mut context_lines = String::new();
    for task in job.context.iter().take(40) {
        context_lines.push_str(&format!(
            "- {} [{}] {}\n",
            short_id(task.id),
            task.bucket.title(),
            task.title
        ));
    }

    let user = format!(
        "Current task:\n{}\n\nInstruction: {}\n\nExisting tasks (id_prefix [bucket] title):\n{}\nReturn JSON with ONLY fields that should change (set unchanged fields to null):\n{{\n  \"title\": string | null,\n  \"bucket\": \"Team\"|\"John\"|\"Admin\" | null,\n  \"description\": string | null,\n  \"progress\": \"Backlog\"|\"Todo\"|\"In progress\"|\"Done\" | null,\n  \"priority\": \"Low\"|\"Medium\"|\"High\"|\"Critical\" | null,\n  \"due_date\": \"YYYY-MM-DD\" | null,\n  \"dependencies\": [\"id_prefix\", ...] | null\n}}\n",
        snapshot,
        instruction,
        context_lines
    );

    let body = json!({
        "model": cfg.model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user}
        ]
    });

    let resp = ureq::post(&cfg.api_url)
        .set("Authorization", &format!("Bearer {}", cfg.api_key))
        .set("Content-Type", "application/json")
        .timeout(cfg.timeout)
        .send_string(&body.to_string());

    let text = match resp {
        Ok(r) => match r.into_string() {
            Ok(s) => s,
            Err(err) => {
                return AiResult {
                    task_id: job.task_id,
                    update: TaskUpdate::default(),
                    error: Some(format!("AI response read failed: {err}")),
                    triage_action: None,
                }
            }
        },
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            return AiResult {
                task_id: job.task_id,
                update: TaskUpdate::default(),
                error: Some(format!("AI HTTP {}: {}", code, truncate(&body, 200))),
                triage_action: None,
            };
        }
        Err(ureq::Error::Transport(t)) => {
            return AiResult {
                task_id: job.task_id,
                update: TaskUpdate::default(),
                error: Some(format!("AI transport error: {t}")),
                triage_action: None,
            };
        }
    };

    let chat: ChatResponse = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(err) => {
            return AiResult {
                task_id: job.task_id,
                update: TaskUpdate::default(),
                error: Some(format!("AI JSON parse failed: {err}")),
                    triage_action: None,
            }
        }
    };

    let content = chat
        .choices
        .get(0)
        .and_then(|c| c.message.content.as_deref())
        .unwrap_or("");

    let json_text = extract_json_object(content).unwrap_or_else(|| content.trim().to_string());

    let enriched: Enriched = match serde_json::from_str(&json_text) {
        Ok(v) => v,
        Err(err) => {
            return AiResult {
                task_id: job.task_id,
                update: TaskUpdate::default(),
                error: Some(format!(
                    "AI output not valid JSON ({err}): {}",
                    truncate(content, 200)
                )),
                triage_action: None,
            }
        }
    };

    let allowed: HashSet<String> = job
        .context
        .iter()
        .map(|t| short_id(t.id))
        .collect::<HashSet<_>>();

    let mut update = TaskUpdate {
        is_edit: true,
        ..TaskUpdate::default()
    };

    update.title = enriched
        .title
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| truncate(s, 200).to_string());

    if let Some(bucket) = enriched.bucket.as_deref().and_then(parse_bucket) {
        update.bucket = Some(bucket);
    }

    update.description = enriched
        .description
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| truncate(s, 400).to_string());

    if let Some(prio) = enriched
        .priority
        .as_deref()
        .and_then(|s| parse_priority(s.trim()))
    {
        update.priority = Some(prio);
    }

    if let Some(prog) = enriched
        .progress
        .as_deref()
        .and_then(|s| parse_progress(s.trim()))
    {
        update.progress = Some(prog);
    }

    if let Some(date_str) = enriched.due_date.as_deref().map(|s| s.trim()) {
        if !date_str.is_empty() {
            if let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                update.due_date = Some(date);
            }
        }
    }

    if let Some(deps) = enriched.dependencies {
        let mut out = Vec::new();
        for dep in deps.into_iter().take(8) {
            let prefix = dep.trim();
            if prefix.len() < 4 {
                continue;
            }
            let key = prefix.chars().take(8).collect::<String>();
            if allowed.contains(&key) && !out.contains(&key) {
                out.push(key);
            }
        }
        update.dependencies = out;
    }

    AiResult {
        task_id: job.task_id,
        update,
        error: None,
        triage_action: None,
    }
}

#[derive(Debug, Deserialize)]
struct TriageEnriched {
    action: Option<String>,
    target_id: Option<String>,
    title: Option<String>,
    bucket: Option<String>,
    description: Option<String>,
    progress: Option<String>,
    priority: Option<String>,
    due_date: Option<String>,
    dependencies: Option<Vec<String>>,
}

fn triage_with_openai(cfg: &OpenAiConfig, job: &AiJob, raw_input: &str) -> AiResult {
    let system = "You are an expert AI project manager. Analyze the user's message and decide whether to CREATE a new task, UPDATE an existing one, or DELETE an existing one. Output ONLY valid JSON. No markdown.";

    let triage_ctx = job.triage_context.as_deref().unwrap_or("");

    let user = format!(
        "User message: \"{}\"\n\nExisting tasks:\n{}\nAnalyze the user's intent:\n- If the message is about an EXISTING task (status update, clarification, etc.), UPDATE it.\n- If the user wants to remove/delete/cancel a task, DELETE it.\n- If it's a genuinely new piece of work, CREATE a new task.\n- For delete: do NOT change any fields, just set action and target_id.\n- Generate a clean, actionable title (do NOT use the user's raw words verbatim).\n- Infer progress from context (e.g. \"already working on X\" → \"In progress\").\n\nReturn JSON:\n{{\n  \"action\": \"create\" | \"update\" | \"delete\",\n  \"target_id\": \"id_prefix\" | null,\n  \"title\": string | null,\n  \"bucket\": \"Team\"|\"John\"|\"Admin\" | null,\n  \"description\": string | null,\n  \"progress\": \"Backlog\"|\"Todo\"|\"In progress\"|\"Done\" | null,\n  \"priority\": \"Low\"|\"Medium\"|\"High\"|\"Critical\" | null,\n  \"due_date\": \"YYYY-MM-DD\" | null,\n  \"dependencies\": [\"id_prefix\", ...] | null\n}}\n",
        raw_input,
        triage_ctx
    );

    let body = json!({
        "model": cfg.model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user}
        ]
    });

    let err_result = |msg: String| AiResult {
        task_id: job.task_id,
        update: TaskUpdate::default(),
        error: Some(msg),
        triage_action: None,
    };

    let resp = ureq::post(&cfg.api_url)
        .set("Authorization", &format!("Bearer {}", cfg.api_key))
        .set("Content-Type", "application/json")
        .timeout(cfg.timeout)
        .send_string(&body.to_string());

    let text = match resp {
        Ok(r) => match r.into_string() {
            Ok(s) => s,
            Err(err) => return err_result(format!("AI response read failed: {err}")),
        },
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            return err_result(format!("AI HTTP {}: {}", code, truncate(&body, 200)));
        }
        Err(ureq::Error::Transport(t)) => {
            return err_result(format!("AI transport error: {t}"));
        }
    };

    let chat: ChatResponse = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(err) => return err_result(format!("AI JSON parse failed: {err}")),
    };

    let content = chat
        .choices
        .get(0)
        .and_then(|c| c.message.content.as_deref())
        .unwrap_or("");

    let json_text = extract_json_object(content).unwrap_or_else(|| content.trim().to_string());

    let triaged: TriageEnriched = match serde_json::from_str(&json_text) {
        Ok(v) => v,
        Err(err) => {
            return err_result(format!(
                "AI output not valid JSON ({err}): {}",
                truncate(content, 200)
            ));
        }
    };

    let action_str = triaged.action.as_deref().unwrap_or("create");
    let triage_action = match action_str {
        "update" => {
            if let Some(target) = triaged.target_id.as_deref().filter(|s| !s.trim().is_empty()) {
                Some(TriageAction::Update(target.trim().to_string()))
            } else {
                Some(TriageAction::Create)
            }
        }
        "delete" => {
            if let Some(target) = triaged.target_id.as_deref().filter(|s| !s.trim().is_empty()) {
                Some(TriageAction::Delete(target.trim().to_string()))
            } else {
                // Can't delete without a target; fall back to create.
                Some(TriageAction::Create)
            }
        }
        _ => Some(TriageAction::Create),
    };

    let mut update = TaskUpdate {
        is_edit: matches!(&triage_action, Some(TriageAction::Update(_))),
        ..TaskUpdate::default()
    };

    update.title = triaged
        .title
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| truncate(s, 200).to_string());

    if let Some(bucket) = triaged.bucket.as_deref().and_then(parse_bucket) {
        update.bucket = Some(bucket);
    }

    update.description = triaged
        .description
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| truncate(s, 400).to_string());

    if let Some(prio) = triaged
        .priority
        .as_deref()
        .and_then(|s| parse_priority(s.trim()))
    {
        update.priority = Some(prio);
    }

    if let Some(prog) = triaged
        .progress
        .as_deref()
        .and_then(|s| parse_progress(s.trim()))
    {
        update.progress = Some(prog);
    }

    if let Some(date_str) = triaged.due_date.as_deref().map(|s| s.trim()) {
        if !date_str.is_empty() {
            if let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                update.due_date = Some(date);
            }
        }
    }

    let allowed: HashSet<String> = job
        .context
        .iter()
        .map(|t| short_id(t.id))
        .collect::<HashSet<_>>();

    if let Some(deps) = triaged.dependencies {
        let mut out = Vec::new();
        for dep in deps.into_iter().take(8) {
            let prefix = dep.trim();
            if prefix.len() < 4 {
                continue;
            }
            let key = prefix.chars().take(8).collect::<String>();
            if allowed.contains(&key) && !out.contains(&key) {
                out.push(key);
            }
        }
        update.dependencies = out;
    }

    AiResult {
        task_id: job.task_id,
        update,
        error: None,
        triage_action,
    }
}

fn parse_progress(input: &str) -> Option<Progress> {
    match input.to_ascii_lowercase().as_str() {
        "backlog" => Some(Progress::Backlog),
        "todo" => Some(Progress::Todo),
        "in progress" | "inprogress" | "in-progress" => Some(Progress::InProgress),
        "done" => Some(Progress::Done),
        _ => None,
    }
}

fn truncate(input: &str, max: usize) -> &str {
    if input.len() <= max {
        return input;
    }
    &input[..max]
}
