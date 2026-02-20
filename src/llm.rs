use std::collections::HashSet;
use std::env;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use chrono::{Local, NaiveDate};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::model::{Priority, Progress};
use crate::storage::AiSettings;

#[derive(Debug, Clone)]
pub struct ContextTask {
    pub id: Uuid,
    pub bucket: String,
    pub title: String,
}

/// A single exchange in the conversation history.
#[derive(Debug, Clone)]
pub struct ChatEntry {
    pub user_input: String,
    pub ai_summary: String,
}

#[derive(Debug, Clone)]
pub struct AiJob {
    pub task_id: Uuid,
    pub title: String,
    pub suggested_bucket: String,
    pub context: Vec<ContextTask>,
    pub bucket_names: Vec<String>,
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
    /// Recent conversation history for triage context.
    pub chat_history: Vec<ChatEntry>,
    /// Free-text bio the user wrote about themselves.
    pub user_profile: String,
    /// Auto-remembered facts about the user.
    pub memory_facts: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TaskUpdate {
    pub is_edit: bool,
    pub title: Option<String>,
    pub bucket: Option<String>,
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
    /// AI decided to update multiple tasks at once.
    BulkUpdate {
        targets: Vec<String>,
        instruction: String,
    },
    /// AI decided to decompose a complex task into sub-issues.
    Decompose {
        target_id: Option<String>,
        specs: Vec<SubTaskSpec>,
    },
    /// AI wants to remember a fact about the user (pending confirmation).
    RememberFact(String),
}

#[derive(Debug, Clone)]
pub struct SubTaskSpec {
    pub title: String,
    pub description: String,
    pub bucket: Option<String>,
    pub priority: Option<Priority>,
    pub progress: Option<Progress>,
    pub due_date: Option<NaiveDate>,
    /// Indices into the sibling subtasks array (resolved to Uuids in main.rs).
    pub depends_on: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct AiResult {
    pub task_id: Uuid,
    pub update: TaskUpdate,
    pub error: Option<String>,
    pub triage_action: Option<TriageAction>,
    pub sub_task_specs: Vec<SubTaskSpec>,
}

#[derive(Debug)]
pub struct AiRuntime {
    job_tx: Sender<AiJob>,
    result_rx: Receiver<AiResult>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provider {
    OpenAi,
    Anthropic,
}

fn detect_provider(model: &str) -> Provider {
    if model.starts_with("claude-") {
        Provider::Anthropic
    } else {
        Provider::OpenAi
    }
}

#[derive(Debug, Clone)]
struct LlmConfig {
    provider: Provider,
    api_url: String,
    model: String,
    api_key: String,
    timeout: Duration,
}

fn build_config(settings: &AiSettings) -> Option<LlmConfig> {
    if !settings.enabled {
        return None;
    }

    let model = if !settings.model.trim().is_empty() {
        settings.model.clone()
    } else {
        env::var("AIPM_MODEL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                env::var("AIPM_OPENAI_MODEL")
                    .ok()
                    .filter(|s| !s.trim().is_empty())
            })
            .unwrap_or_else(|| "claude-sonnet-4-5".to_string())
    };

    let provider = detect_provider(&model);

    let settings_key = match provider {
        Provider::Anthropic => &settings.anthropic_api_key,
        Provider::OpenAi => &settings.openai_api_key,
    };
    let key = if !settings_key.trim().is_empty() {
        settings_key.clone()
    } else {
        match provider {
            Provider::Anthropic => env::var("ANTHROPIC_API_KEY").ok()?,
            Provider::OpenAi => env::var("OPENAI_API_KEY").ok()?,
        }
    };

    let default_url = match provider {
        Provider::Anthropic => "https://api.anthropic.com/v1/messages",
        Provider::OpenAi => "https://api.openai.com/v1/chat/completions",
    };

    let api_url = if !settings.api_url.trim().is_empty() {
        let saved = settings.api_url.trim();
        if (provider == Provider::Anthropic
            && saved == "https://api.openai.com/v1/chat/completions")
            || (provider == Provider::OpenAi && saved == "https://api.anthropic.com/v1/messages")
        {
            default_url.to_string()
        } else {
            settings.api_url.clone()
        }
    } else {
        env::var("AIPM_API_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                env::var("AIPM_OPENAI_URL")
                    .ok()
                    .filter(|s| !s.trim().is_empty())
            })
            .unwrap_or_else(|| default_url.to_string())
    };

    let timeout = Duration::from_secs(if settings.timeout_secs > 0 {
        settings.timeout_secs
    } else {
        60
    });

    Some(LlmConfig {
        provider,
        api_url,
        model,
        api_key: key,
        timeout,
    })
}

impl AiRuntime {
    pub fn from_settings(settings: &AiSettings) -> Option<AiRuntime> {
        let cfg = build_config(settings)?;

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

    /// Blocking receive for CLI mode. Returns None on timeout.
    pub fn recv_blocking(&self, timeout: Duration) -> Option<AiResult> {
        self.result_rx.recv_timeout(timeout).ok()
    }
}

const MAX_PARALLEL_JOBS: usize = 8;

fn worker_loop(cfg: LlmConfig, job_rx: Receiver<AiJob>, result_tx: Sender<AiResult>) {
    let cfg = Arc::new(cfg);
    let active = Arc::new((Mutex::new(0usize), Condvar::new()));

    for job in job_rx {
        let cfg = Arc::clone(&cfg);
        let tx = result_tx.clone();
        let active = Arc::clone(&active);

        {
            let (lock, cvar) = &*active;
            let mut count = lock.lock().unwrap();
            while *count >= MAX_PARALLEL_JOBS {
                count = cvar.wait(count).unwrap();
            }
            *count += 1;
        }

        thread::spawn(move || {
            let result = enrich_task(&cfg, &job);
            let _ = tx.send(result);

            let (lock, cvar) = &*active;
            let mut count = lock.lock().unwrap();
            *count -= 1;
            cvar.notify_one();
        });
    }
}

/// Whether an LLM error is transient and worth retrying.
fn is_retryable(err: &str) -> bool {
    // Transport / timeout errors.
    if err.contains("transport error") || err.contains("response read failed") {
        return true;
    }
    // Rate-limit or server errors.
    if err.contains("HTTP 429") || err.contains("HTTP 5") {
        return true;
    }
    false
}

const MAX_RETRIES: u32 = 2;
const RETRY_DELAYS: [u64; 2] = [1, 3];

/// Execute `f` with up to MAX_RETRIES retries on transient failures.
/// The closure receives the attempt number (0-based) so callers can
/// increase timeouts on subsequent attempts.
fn with_retry<T, F: Fn(u32) -> Result<T, String>>(f: F) -> Result<T, String> {
    let mut last_err = String::new();
    for attempt in 0..=MAX_RETRIES {
        match f(attempt) {
            Ok(val) => return Ok(val),
            Err(err) => {
                if attempt < MAX_RETRIES && is_retryable(&err) {
                    thread::sleep(Duration::from_secs(RETRY_DELAYS[attempt as usize]));
                    last_err = err;
                    continue;
                }
                return Err(err);
            }
        }
    }
    Err(last_err)
}

// ---------------------------------------------------------------------------
// URL content fetching
// ---------------------------------------------------------------------------

struct UrlContext {
    url: String,
    summary: String,
}

/// Extract URLs from text.
fn extract_urls(text: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for token in text.split_whitespace() {
        let t = token.trim_matches(|c: char| c == '<' || c == '>' || c == ',' || c == ')');
        if t.starts_with("http://") || t.starts_with("https://") {
            urls.push(t.to_string());
        }
    }
    urls
}

/// Try to parse a GitHub PR/issue URL into (owner, repo, number, kind).
fn parse_github_url(url: &str) -> Option<(String, String, String, &'static str)> {
    // https://github.com/{owner}/{repo}/pull/{number}
    // https://github.com/{owner}/{repo}/issues/{number}
    let path = url.strip_prefix("https://github.com/")?;
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 4 {
        return None;
    }
    let owner = parts[0].to_string();
    let repo = parts[1].to_string();
    let kind = match parts[2] {
        "pull" => "pulls",
        "issues" => "issues",
        _ => return None,
    };
    let number = parts[3].split('?').next()?.split('#').next()?;
    if number.chars().all(|c| c.is_ascii_digit()) {
        Some((owner, repo, number.to_string(), kind))
    } else {
        None
    }
}

#[derive(Debug, Deserialize)]
struct GhPr {
    title: Option<String>,
    body: Option<String>,
    state: Option<String>,
    user: Option<GhUser>,
}

#[derive(Debug, Deserialize)]
struct GhUser {
    login: Option<String>,
}

/// Fetch context for a list of URLs. Best-effort; failures are silently skipped.
fn fetch_url_contexts(urls: &[String], timeout: Duration) -> Vec<UrlContext> {
    let urls: Vec<&String> = urls.iter().take(15).collect();
    if urls.is_empty() {
        return Vec::new();
    }

    thread::scope(|s| {
        let handles: Vec<_> = urls
            .iter()
            .map(|url| s.spawn(|| fetch_single_url(url, timeout)))
            .collect();

        handles
            .into_iter()
            .filter_map(|h| h.join().ok().flatten())
            .collect()
    })
}

fn fetch_single_url(url: &str, timeout: Duration) -> Option<UrlContext> {
    if let Some((owner, repo, number, kind)) = parse_github_url(url) {
        let api_url = format!(
            "https://api.github.com/repos/{}/{}/{}/{}",
            owner, repo, kind, number
        );
        let resp = ureq::get(&api_url)
            .set("Accept", "application/vnd.github+json")
            .set("User-Agent", "aipm")
            .timeout(timeout)
            .call()
            .ok()?;
        let text = resp.into_string().ok()?;
        let pr: GhPr = serde_json::from_str(&text).ok()?;
        let title = pr.title.as_deref().unwrap_or("Untitled");
        let author = pr
            .user
            .as_ref()
            .and_then(|u| u.login.as_deref())
            .unwrap_or("unknown");
        let state = pr.state.as_deref().unwrap_or("unknown");
        let body_snippet = pr
            .body
            .as_deref()
            .map(|b| truncate(b.trim(), 300))
            .unwrap_or("");
        let label = if kind == "pulls" { "PR" } else { "Issue" };
        let summary = format!(
            "{} #{}: {} (by {}, {})\n{}",
            label, number, title, author, state, body_snippet
        );
        return Some(UrlContext {
            url: url.to_string(),
            summary,
        });
    }

    // Generic URL: fetch and strip HTML.
    let resp = ureq::get(url)
        .set("User-Agent", "aipm")
        .timeout(timeout)
        .call()
        .ok()?;
    let body = resp.into_string().ok()?;
    let title = extract_html_title(&body).unwrap_or_default();
    let text = strip_html_tags(&body);
    let snippet = truncate(text.trim(), 500);
    let summary = if title.is_empty() {
        snippet.to_string()
    } else {
        format!("{}\n{}", title, snippet)
    };
    if summary.trim().is_empty() {
        return None;
    }
    Some(UrlContext {
        url: url.to_string(),
        summary,
    })
}

fn extract_html_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")? + 6;
    let after_tag = lower[start..].find('>')? + start + 1;
    let end = lower[after_tag..].find("</title")? + after_tag;
    let title = html[after_tag..end].trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

fn strip_html_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    for ch in html.chars() {
        if ch == '<' {
            in_tag = true;
        } else if ch == '>' {
            in_tag = false;
            out.push(' ');
        } else if !in_tag {
            out.push(ch);
        }
    }
    // Collapse whitespace.
    let mut result = String::new();
    let mut prev_space = false;
    for ch in out.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                result.push(' ');
                prev_space = true;
            }
        } else {
            result.push(ch);
            prev_space = false;
        }
    }
    result
}

/// Compute a timeout that scales with prompt size so large requests
/// (triage with many tasks, decompose, bulk updates) get more time.
/// Adds ~10 s per 4 KiB of prompt, capped at 3× base (minimum 120 s).
fn scaled_timeout(base: Duration, body: &serde_json::Value, attempt: u32) -> Duration {
    let body_len = body.to_string().len();
    let extra_secs = (body_len / 4096) as u64 * 10;
    let total = base.as_secs() + extra_secs;
    let cap = base.as_secs().saturating_mul(3).max(120);
    let scaled = Duration::from_secs(total.min(cap));
    // Bump 50 % per retry so a timed-out request gets a longer second chance.
    scaled + Duration::from_secs(scaled.as_secs() * attempt as u64 / 2)
}

/// Send a raw HTTP request to the LLM and return the response body.
fn send_llm_request(
    cfg: &LlmConfig,
    body: &serde_json::Value,
    timeout: Duration,
) -> Result<String, String> {
    let mut req = ureq::post(&cfg.api_url)
        .set("Content-Type", "application/json")
        .timeout(timeout);

    req = match cfg.provider {
        Provider::OpenAi => req.set("Authorization", &format!("Bearer {}", cfg.api_key)),
        Provider::Anthropic => req
            .set("x-api-key", &cfg.api_key)
            .set("anthropic-version", "2023-06-01"),
    };

    let resp = req.send_string(&body.to_string());

    match resp {
        Ok(r) => r
            .into_string()
            .map_err(|err| format!("AI response read failed: {err}")),
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            Err(format!("AI HTTP {}: {}", code, truncate(&body, 200)))
        }
        Err(ureq::Error::Transport(t)) => Err(format!("AI transport error: {t}")),
    }
}

/// Send a system+user prompt to the configured LLM and return the text content.
fn call_llm(cfg: &LlmConfig, system: &str, user: &str) -> Result<String, String> {
    let body = match cfg.provider {
        Provider::OpenAi => json!({
            "model": cfg.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ]
        }),
        Provider::Anthropic => json!({
            "model": cfg.model,
            "max_tokens": 4096,
            "system": system,
            "messages": [
                {"role": "user", "content": user}
            ]
        }),
    };

    let text = with_retry(|attempt| {
        let timeout = scaled_timeout(cfg.timeout, &body, attempt);
        send_llm_request(cfg, &body, timeout)
    })?;

    match cfg.provider {
        Provider::OpenAi => {
            let chat: ChatResponse = serde_json::from_str(&text)
                .map_err(|err| format!("AI JSON parse failed: {err}"))?;
            Ok(chat
                .choices
                .first()
                .and_then(|c| c.message.content.as_deref())
                .unwrap_or("")
                .to_string())
        }
        Provider::Anthropic => {
            let resp: AnthropicResponse = serde_json::from_str(&text)
                .map_err(|err| format!("AI JSON parse failed: {err}"))?;
            Ok(resp
                .content
                .iter()
                .filter_map(|b| {
                    if b.block_type == "text" {
                        b.text.as_deref()
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join(""))
        }
    }
}

/// Send a prompt with tool definitions and return the first tool call.
/// Returns (tool_name, parsed_arguments) on success.
fn call_llm_with_tools(
    cfg: &LlmConfig,
    system: &str,
    user: &str,
    tools: &serde_json::Value,
) -> Result<(String, serde_json::Value), String> {
    let body = match cfg.provider {
        Provider::OpenAi => json!({
            "model": cfg.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ],
            "tools": tools,
            "tool_choice": "required"
        }),
        Provider::Anthropic => json!({
            "model": cfg.model,
            "max_tokens": 4096,
            "system": system,
            "messages": [
                {"role": "user", "content": user}
            ],
            "tools": tools,
            "tool_choice": {"type": "any"}
        }),
    };

    let text = with_retry(|attempt| {
        let timeout = scaled_timeout(cfg.timeout, &body, attempt);
        send_llm_request(cfg, &body, timeout)
    })?;

    match cfg.provider {
        Provider::OpenAi => {
            let chat: ChatResponse = serde_json::from_str(&text)
                .map_err(|err| format!("AI JSON parse failed: {err}"))?;
            let tool_call = chat
                .choices
                .first()
                .and_then(|c| c.message.tool_calls.as_ref())
                .and_then(|tc| tc.first())
                .ok_or_else(|| "AI returned no tool call".to_string())?;
            let args: serde_json::Value = serde_json::from_str(&tool_call.function.arguments)
                .map_err(|err| format!("AI tool args parse failed: {err}"))?;
            Ok((tool_call.function.name.clone(), args))
        }
        Provider::Anthropic => {
            let resp: AnthropicResponse = serde_json::from_str(&text)
                .map_err(|err| format!("AI JSON parse failed: {err}"))?;
            let tool_block = resp
                .content
                .iter()
                .find(|b| b.block_type == "tool_use")
                .ok_or_else(|| "AI returned no tool_use block".to_string())?;
            let name = tool_block
                .name
                .clone()
                .ok_or_else(|| "tool_use block missing name".to_string())?;
            let input = tool_block
                .input
                .clone()
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            Ok((name, input))
        }
    }
}

fn enrich_task(cfg: &LlmConfig, job: &AiJob) -> AiResult {
    if let Some(raw_input) = &job.triage_input {
        return triage_task(cfg, job, raw_input);
    }
    if let Some(instruction) = &job.edit_instruction {
        return edit_task(cfg, job, instruction);
    }

    let today = Local::now().format("%Y-%m-%d").to_string();
    let system = format!(
        "Today is {today}. You are an expert AI project manager. Output ONLY valid JSON. No markdown."
    );

    let mut context_lines = String::new();
    for task in job.context.iter().take(40) {
        context_lines.push_str(&format!(
            "- {} [{}] {}\n",
            short_id(task.id),
            task.bucket,
            task.title
        ));
    }

    let lock_line = format!(
        "Locked fields: bucket={} priority={} due_date={}",
        job.lock_bucket, job.lock_priority, job.lock_due_date
    );

    let bucket_enum = job
        .bucket_names
        .iter()
        .map(|n| format!("\"{}\"", n))
        .collect::<Vec<_>>()
        .join("|");

    let user = format!(
        "New task title: {}\nSuggested bucket: {}\n{}\n\nExisting tasks you may depend on (id_prefix [bucket] title):\n{}\nReturn JSON with keys:\n{{\n  \"bucket\": {bucket_enum},\n  \"description\": string,\n  \"priority\": \"Low\"|\"Medium\"|\"High\"|\"Critical\",\n  \"due_date\": \"YYYY-MM-DD\" | null,\n  \"dependencies\": [\"id_prefix\", ...]\n}}\nRules:\n- If a field is locked, keep it aligned with the suggested value (bucket) or output null/Medium (due/priority) as appropriate.\n- If unsure, keep bucket as suggested.\n- Dependencies must use the provided id_prefix values.\n",
        job.title.trim(),
        &job.suggested_bucket,
        lock_line,
        context_lines
    );

    let content = match call_llm(cfg, &system, &user) {
        Ok(text) => text,
        Err(err) => {
            return AiResult {
                task_id: job.task_id,
                update: TaskUpdate::default(),
                error: Some(err),
                triage_action: None,
                sub_task_specs: Vec::new(),
            }
        }
    };

    let json_text = extract_json_object(&content).unwrap_or_else(|| content.trim().to_string());

    let enriched: Enriched = match serde_json::from_str(&json_text) {
        Ok(v) => v,
        Err(err) => {
            return AiResult {
                task_id: job.task_id,
                update: TaskUpdate::default(),
                error: Some(format!(
                    "AI output not valid JSON ({err}): {}",
                    truncate(&content, 200)
                )),
                triage_action: None,
                sub_task_specs: Vec::new(),
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
        if let Some(bucket) = enriched
            .bucket
            .as_deref()
            .and_then(|b| parse_bucket(b, &job.bucket_names))
        {
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
        sub_task_specs: Vec::new(),
    }
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ChatMessage,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    content: Option<String>,
    tool_calls: Option<Vec<ChatToolCall>>,
}

#[derive(Debug, Deserialize)]
struct ChatToolCall {
    function: ChatToolFunction,
}

#[derive(Debug, Deserialize)]
struct ChatToolFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContentBlock>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
    name: Option<String>,
    input: Option<serde_json::Value>,
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
    subtasks: Option<Vec<SubTaskEnriched>>,
}

fn extract_json_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(text[start..=end].to_string())
}

fn parse_bucket(input: &str, valid_names: &[String]) -> Option<String> {
    let lower = input.to_ascii_lowercase();
    valid_names
        .iter()
        .find(|n| n.to_ascii_lowercase() == lower)
        .cloned()
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

fn edit_task(cfg: &LlmConfig, job: &AiJob, instruction: &str) -> AiResult {
    let today = Local::now().format("%Y-%m-%d").to_string();
    let system = format!(
        "Today is {today}. You are an expert AI project manager. Modify the given task based on the user instruction. Output ONLY valid JSON. No markdown."
    );

    let snapshot = job.task_snapshot.as_deref().unwrap_or("");

    let mut context_lines = String::new();
    for task in job.context.iter().take(40) {
        context_lines.push_str(&format!(
            "- {} [{}] {}\n",
            short_id(task.id),
            task.bucket,
            task.title
        ));
    }

    let bucket_enum = job
        .bucket_names
        .iter()
        .map(|n| format!("\"{}\"", n))
        .collect::<Vec<_>>()
        .join("|");

    let user = format!(
        "Current task:\n{}\n\nInstruction: {}\n\nExisting tasks (id_prefix [bucket] title):\n{}\nReturn JSON with ONLY fields that should change (set unchanged fields to null):\n{{\n  \"title\": string | null,\n  \"bucket\": {bucket_enum} | null,\n  \"description\": string | null,\n  \"progress\": \"Backlog\"|\"Todo\"|\"In progress\"|\"Done\" | null,\n  \"priority\": \"Low\"|\"Medium\"|\"High\"|\"Critical\" | null,\n  \"due_date\": \"YYYY-MM-DD\" | null,\n  \"dependencies\": [\"id_prefix\", ...] | null,\n  \"subtasks\": [{{\"title\": string, \"description\": string, \"bucket\": {bucket_enum}, \"priority\": \"Low\"|\"Medium\"|\"High\"|\"Critical\", \"progress\": \"Backlog\"|\"Todo\"|\"In progress\"|\"Done\", \"due_date\": \"YYYY-MM-DD\" | null, \"depends_on\": [0-based index, ...]}}] | null\n}}\nRules:\n- If the instruction asks to create sub-issues, sub-tasks, break down, or decompose the task, return them as entries in the \"subtasks\" array. NEVER write sub-task lists, numbered breakdowns, or step-by-step plans into the \"description\" field.\n- depends_on is an array of 0-based indices into the subtasks array representing execution order. Use it to express sequential dependencies between subtasks.\n- Subtasks inherit the parent task's bucket and priority unless the instruction specifies otherwise.\n- Only include fields that should change. Set unchanged fields to null.\n",
        snapshot,
        instruction,
        context_lines
    );

    let content = match call_llm(cfg, &system, &user) {
        Ok(text) => text,
        Err(err) => {
            return AiResult {
                task_id: job.task_id,
                update: TaskUpdate::default(),
                error: Some(err),
                triage_action: None,
                sub_task_specs: Vec::new(),
            }
        }
    };

    let json_text = extract_json_object(&content).unwrap_or_else(|| content.trim().to_string());

    let enriched: Enriched = match serde_json::from_str(&json_text) {
        Ok(v) => v,
        Err(err) => {
            return AiResult {
                task_id: job.task_id,
                update: TaskUpdate::default(),
                error: Some(format!(
                    "AI output not valid JSON ({err}): {}",
                    truncate(&content, 200)
                )),
                triage_action: None,
                sub_task_specs: Vec::new(),
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

    if let Some(bucket) = enriched
        .bucket
        .as_deref()
        .and_then(|b| parse_bucket(b, &job.bucket_names))
    {
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

    let sub_task_specs: Vec<SubTaskSpec> = enriched
        .subtasks
        .unwrap_or_default()
        .into_iter()
        .filter_map(|st| {
            let title = st
                .title
                .as_deref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())?
                .to_string();
            Some(SubTaskSpec {
                title: truncate(&title, 200).to_string(),
                description: st
                    .description
                    .as_deref()
                    .map(|s| truncate(s.trim(), 400).to_string())
                    .unwrap_or_default(),
                bucket: st
                    .bucket
                    .as_deref()
                    .and_then(|b| parse_bucket(b, &job.bucket_names)),
                priority: st
                    .priority
                    .as_deref()
                    .and_then(|s| parse_priority(s.trim())),
                progress: st
                    .progress
                    .as_deref()
                    .and_then(|s| parse_progress(s.trim())),
                due_date: st
                    .due_date
                    .as_deref()
                    .and_then(|s| NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()),
                depends_on: st.depends_on.unwrap_or_default(),
            })
        })
        .take(12)
        .collect();

    AiResult {
        task_id: job.task_id,
        update,
        error: None,
        triage_action: None,
        sub_task_specs,
    }
}

// ---------------------------------------------------------------------------
// Tool-calling argument structs for triage
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SubTaskArg {
    title: String,
    description: Option<String>,
    bucket: Option<String>,
    priority: Option<String>,
    progress: Option<String>,
    due_date: Option<String>,
    depends_on: Option<Vec<usize>>,
}

#[derive(Debug, Deserialize)]
struct CreateTaskArgs {
    title: String,
    bucket: String,
    description: Option<String>,
    priority: Option<String>,
    progress: Option<String>,
    due_date: Option<String>,
    dependencies: Option<Vec<String>>,
    subtasks: Option<Vec<SubTaskArg>>,
}

#[derive(Debug, Deserialize)]
struct UpdateTaskArgs {
    target_id: String,
    title: Option<String>,
    bucket: Option<String>,
    description: Option<String>,
    priority: Option<String>,
    progress: Option<String>,
    due_date: Option<String>,
    dependencies: Option<Vec<String>>,
    subtasks: Option<Vec<SubTaskArg>>,
}

#[derive(Debug, Deserialize)]
struct DeleteTaskArgs {
    target_id: String,
}

#[derive(Debug, Deserialize)]
struct DecomposeTaskArgs {
    target_id: Option<String>,
    subtasks: Vec<SubTaskArg>,
}

#[derive(Debug, Deserialize)]
struct BulkUpdateTasksArgs {
    target_ids: Vec<String>,
    instruction: String,
}

#[derive(Debug, Deserialize)]
struct RememberFactArgs {
    fact: String,
}

#[derive(Debug, Deserialize)]
struct SubTaskEnriched {
    title: Option<String>,
    description: Option<String>,
    bucket: Option<String>,
    priority: Option<String>,
    progress: Option<String>,
    due_date: Option<String>,
    depends_on: Option<Vec<usize>>,
}

// ---------------------------------------------------------------------------
// Tool schema definitions for triage
// ---------------------------------------------------------------------------

fn subtask_schema(bucket_names: &[String]) -> serde_json::Value {
    let bucket_values: Vec<serde_json::Value> = bucket_names.iter().map(|n| json!(n)).collect();
    json!({
        "type": "object",
        "properties": {
            "title": {"type": "string", "description": "Short, actionable subtask title"},
            "description": {"type": "string", "description": "Brief description"},
            "bucket": {"type": "string", "enum": bucket_values},
            "priority": {"type": "string", "enum": ["Low", "Medium", "High", "Critical"]},
            "progress": {"type": "string", "enum": ["Backlog", "Todo", "In progress", "Done"]},
            "due_date": {"type": "string", "description": "YYYY-MM-DD format"},
            "depends_on": {
                "type": "array",
                "items": {"type": "integer"},
                "description": "0-based indices into the subtasks array for ordering"
            }
        },
        "required": ["title"]
    })
}

fn make_tool_def(
    provider: Provider,
    name: &str,
    description: &str,
    schema: serde_json::Value,
) -> serde_json::Value {
    match provider {
        Provider::OpenAi => json!({
            "type": "function",
            "function": {
                "name": name,
                "description": description,
                "parameters": schema
            }
        }),
        Provider::Anthropic => json!({
            "name": name,
            "description": description,
            "input_schema": schema
        }),
    }
}

fn triage_tool_defs(provider: Provider, bucket_names: &[String]) -> serde_json::Value {
    let st = subtask_schema(bucket_names);
    let bucket_values: Vec<serde_json::Value> = bucket_names.iter().map(|n| json!(n)).collect();
    json!([
        make_tool_def(
            provider,
            "create_task",
            "Create a new task. Use ONLY when the user describes genuinely new work that does NOT overlap with any existing task or sub-task. Always check existing tasks first.",
            json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "Short, actionable title"},
                    "bucket": {"type": "string", "enum": bucket_values.clone()},
                    "description": {"type": "string", "description": "Brief task description"},
                    "priority": {"type": "string", "enum": ["Low", "Medium", "High", "Critical"]},
                    "progress": {"type": "string", "enum": ["Backlog", "Todo", "In progress", "Done"]},
                    "due_date": {"type": "string", "description": "YYYY-MM-DD format"},
                    "dependencies": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "id_prefix values of existing tasks this depends on"
                    },
                    "subtasks": {
                        "type": "array",
                        "items": st.clone(),
                        "description": "Sub-tasks to create under this task"
                    }
                },
                "required": ["title", "bucket"]
            })
        ),
        make_tool_def(
            provider,
            "update_task",
            "Update an existing task. PREFER this over create_task when similar work already exists. Use for status changes, field updates, or adding sub-tasks.",
            json!({
                "type": "object",
                "properties": {
                    "target_id": {"type": "string", "description": "id_prefix of the task to update"},
                    "title": {"type": "string", "description": "New title"},
                    "bucket": {"type": "string", "enum": bucket_values},
                    "description": {"type": "string"},
                    "priority": {"type": "string", "enum": ["Low", "Medium", "High", "Critical"]},
                    "progress": {"type": "string", "enum": ["Backlog", "Todo", "In progress", "Done"]},
                    "due_date": {"type": "string", "description": "YYYY-MM-DD format"},
                    "dependencies": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "id_prefix values of existing tasks this depends on"
                    },
                    "subtasks": {
                        "type": "array",
                        "items": st.clone(),
                        "description": "Sub-tasks to create under this task"
                    }
                },
                "required": ["target_id"]
            })
        ),
        make_tool_def(
            provider,
            "delete_task",
            "Delete an existing task.",
            json!({
                "type": "object",
                "properties": {
                    "target_id": {"type": "string", "description": "id_prefix of the task to delete"}
                },
                "required": ["target_id"]
            })
        ),
        make_tool_def(
            provider,
            "decompose_task",
            "Break down a task into smaller sub-tasks. Use when asked to decompose, split, or create sub-issues.",
            json!({
                "type": "object",
                "properties": {
                    "target_id": {"type": "string", "description": "id_prefix of the task to decompose (if known)"},
                    "subtasks": {
                        "type": "array",
                        "items": st,
                        "description": "Sub-tasks to create"
                    }
                },
                "required": ["subtasks"]
            })
        ),
        make_tool_def(
            provider,
            "bulk_update_tasks",
            "Update multiple tasks at once. Use when the instruction affects many tasks.",
            json!({
                "type": "object",
                "properties": {
                    "target_ids": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "id_prefix values, or [\"all\"] for every task"
                    },
                    "instruction": {"type": "string", "description": "What to change"}
                },
                "required": ["target_ids", "instruction"]
            })
        ),
        make_tool_def(
            provider,
            "remember_fact",
            "Save a durable fact about the user for future context. Use sparingly — only for broadly useful information like their name, role, team, or strong preferences. Never use for task-specific details.",
            json!({
                "type": "object",
                "properties": {
                    "fact": {
                        "type": "string",
                        "description": "A concise, self-contained fact about the user (max 120 chars)"
                    }
                },
                "required": ["fact"]
            })
        ),
    ])
}

fn parse_subtask_args(args: Option<Vec<SubTaskArg>>, bucket_names: &[String]) -> Vec<SubTaskSpec> {
    args.unwrap_or_default()
        .into_iter()
        .filter_map(|st| {
            let title = st.title.trim().to_string();
            if title.is_empty() {
                return None;
            }
            Some(SubTaskSpec {
                title: truncate(&title, 200).to_string(),
                description: st
                    .description
                    .as_deref()
                    .map(|s| truncate(s.trim(), 400).to_string())
                    .unwrap_or_default(),
                bucket: st
                    .bucket
                    .as_deref()
                    .and_then(|b| parse_bucket(b, bucket_names)),
                priority: st
                    .priority
                    .as_deref()
                    .and_then(|s| parse_priority(s.trim())),
                progress: st
                    .progress
                    .as_deref()
                    .and_then(|s| parse_progress(s.trim())),
                due_date: st
                    .due_date
                    .as_deref()
                    .and_then(|s| NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()),
                depends_on: st.depends_on.unwrap_or_default(),
            })
        })
        .take(12)
        .collect()
}

fn resolve_deps(deps: Option<Vec<String>>, allowed: &HashSet<String>) -> Vec<String> {
    let mut out = Vec::new();
    for dep in deps.unwrap_or_default().into_iter().take(8) {
        let prefix = dep.trim();
        if prefix.len() < 4 {
            continue;
        }
        let key = prefix.chars().take(8).collect::<String>();
        if allowed.contains(&key) && !out.contains(&key) {
            out.push(key);
        }
    }
    out
}

fn triage_task(cfg: &LlmConfig, job: &AiJob, raw_input: &str) -> AiResult {
    let err_result = |msg: String| AiResult {
        task_id: job.task_id,
        update: TaskUpdate::default(),
        error: Some(msg),
        triage_action: None,
        sub_task_specs: Vec::new(),
    };

    // Fetch URL contexts for any links in the user's message.
    let urls = extract_urls(raw_input);
    let url_contexts = if urls.is_empty() {
        Vec::new()
    } else {
        fetch_url_contexts(&urls, cfg.timeout)
    };

    let today = Local::now().format("%Y-%m-%d").to_string();
    let mut system = format!(
        "Today is {today}. You are an expert AI project manager. Analyze the user's message and call \
        the appropriate tool.\n\
        Rules:\n\
        - CRITICAL: Before creating ANY task, carefully check ALL existing tasks AND their sub-tasks \
        (lines starting with ↳). If a task or sub-task with similar meaning already exists, use \
        update_task instead of create_task. NEVER create a duplicate.\n\
        - When adding sub-tasks, check the parent's existing sub-tasks first. Do NOT create sub-tasks \
        that overlap with ones already listed.\n\
        - Generate clean, actionable titles (do NOT use the user's raw words verbatim).\n\
        - Infer progress from context (e.g. \"already working on X\" → \"In progress\").\n\
        - If the user asks to break down, decompose, split, or create sub-tasks, use decompose_task.\n\
        - When updating a task with multiple items/links that map to sub-tasks, include them in the subtasks array.\n\
        - NEVER put sub-task breakdowns or numbered lists into the description field. Use the subtasks array.\n\
        - Subtasks inherit the parent task's bucket and priority unless specified otherwise.\n\
        - For delete: just call delete_task with the target_id.\n\
        - When the user mentions a task (by name or reference) and follows with an instruction, \
        assume the instruction applies to that task or its subtasks — use update_task or \
        decompose_task targeting that task rather than creating something new.\n\
        - If the user shares something durable and broadly useful about themselves (name, role, team, \
        strong preference), call remember_fact. Do NOT remember task-specific details."
    );
    if !job.user_profile.is_empty() {
        system.push_str(&format!("\n\nUser profile: {}", job.user_profile));
    }
    if !job.memory_facts.is_empty() {
        system.push_str("\n\nKnown facts about the user:");
        for fact in &job.memory_facts {
            system.push_str(&format!("\n- {fact}"));
        }
    }

    let triage_ctx = job.triage_context.as_deref().unwrap_or("");

    let mut user_prompt = format!(
        "User message: \"{}\"\n\nExisting tasks:\n{}",
        raw_input, triage_ctx
    );

    if !url_contexts.is_empty() {
        user_prompt.push_str("\nReferenced URLs:\n");
        for ctx in &url_contexts {
            user_prompt.push_str(&format!("- {}\n  {}\n", ctx.url, ctx.summary));
        }
    }

    if !job.chat_history.is_empty() {
        user_prompt.push_str("\nRecent conversation:\n");
        for entry in job.chat_history.iter().rev().take(10).rev() {
            user_prompt.push_str(&format!(
                "User: {}\nResult: {}\n\n",
                entry.user_input, entry.ai_summary
            ));
        }
    }

    let tools = triage_tool_defs(cfg.provider, &job.bucket_names);

    let (tool_name, args) = match call_llm_with_tools(cfg, &system, &user_prompt, &tools) {
        Ok(result) => result,
        Err(err) => return err_result(err),
    };

    let allowed: HashSet<String> = job.context.iter().map(|t| short_id(t.id)).collect();

    match tool_name.as_str() {
        "create_task" => {
            let parsed: CreateTaskArgs = match serde_json::from_value(args) {
                Ok(v) => v,
                Err(e) => return err_result(format!("Failed to parse create_task args: {e}")),
            };
            let sub_task_specs = parse_subtask_args(parsed.subtasks, &job.bucket_names);
            AiResult {
                task_id: job.task_id,
                update: TaskUpdate {
                    is_edit: false,
                    title: Some(truncate(parsed.title.trim(), 200).to_string()),
                    bucket: parse_bucket(&parsed.bucket, &job.bucket_names),
                    description: parsed
                        .description
                        .as_deref()
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .map(|s| truncate(s, 400).to_string()),
                    progress: parsed
                        .progress
                        .as_deref()
                        .and_then(|s| parse_progress(s.trim())),
                    priority: parsed
                        .priority
                        .as_deref()
                        .and_then(|s| parse_priority(s.trim())),
                    due_date: parsed
                        .due_date
                        .as_deref()
                        .and_then(|s| NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()),
                    dependencies: resolve_deps(parsed.dependencies, &allowed),
                },
                error: None,
                triage_action: Some(TriageAction::Create),
                sub_task_specs,
            }
        }
        "update_task" => {
            let parsed: UpdateTaskArgs = match serde_json::from_value(args) {
                Ok(v) => v,
                Err(e) => return err_result(format!("Failed to parse update_task args: {e}")),
            };
            let target = parsed.target_id.trim().to_string();
            let sub_task_specs = parse_subtask_args(parsed.subtasks, &job.bucket_names);
            let triage_action = if target.is_empty() {
                Some(TriageAction::Create)
            } else {
                Some(TriageAction::Update(target))
            };
            let is_edit = matches!(&triage_action, Some(TriageAction::Update(_)));
            AiResult {
                task_id: job.task_id,
                update: TaskUpdate {
                    is_edit,
                    title: parsed
                        .title
                        .as_deref()
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .map(|s| truncate(s, 200).to_string()),
                    bucket: parsed
                        .bucket
                        .as_deref()
                        .and_then(|b| parse_bucket(b, &job.bucket_names)),
                    description: parsed
                        .description
                        .as_deref()
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .map(|s| truncate(s, 400).to_string()),
                    progress: parsed
                        .progress
                        .as_deref()
                        .and_then(|s| parse_progress(s.trim())),
                    priority: parsed
                        .priority
                        .as_deref()
                        .and_then(|s| parse_priority(s.trim())),
                    due_date: parsed
                        .due_date
                        .as_deref()
                        .and_then(|s| NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()),
                    dependencies: resolve_deps(parsed.dependencies, &allowed),
                },
                error: None,
                triage_action,
                sub_task_specs,
            }
        }
        "delete_task" => {
            let parsed: DeleteTaskArgs = match serde_json::from_value(args) {
                Ok(v) => v,
                Err(e) => return err_result(format!("Failed to parse delete_task args: {e}")),
            };
            let target = parsed.target_id.trim().to_string();
            if target.is_empty() {
                return err_result("delete_task: empty target_id".to_string());
            }
            AiResult {
                task_id: job.task_id,
                update: TaskUpdate::default(),
                error: None,
                triage_action: Some(TriageAction::Delete(target)),
                sub_task_specs: Vec::new(),
            }
        }
        "decompose_task" => {
            let parsed: DecomposeTaskArgs = match serde_json::from_value(args) {
                Ok(v) => v,
                Err(e) => return err_result(format!("Failed to parse decompose_task args: {e}")),
            };
            let specs = parse_subtask_args(Some(parsed.subtasks), &job.bucket_names);
            if specs.is_empty() {
                return err_result("decompose_task: no subtasks provided".to_string());
            }
            let target_id = parsed
                .target_id
                .as_deref()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            AiResult {
                task_id: job.task_id,
                update: TaskUpdate::default(),
                error: None,
                triage_action: Some(TriageAction::Decompose {
                    target_id,
                    specs: specs.clone(),
                }),
                sub_task_specs: Vec::new(),
            }
        }
        "bulk_update_tasks" => {
            let parsed: BulkUpdateTasksArgs = match serde_json::from_value(args) {
                Ok(v) => v,
                Err(e) => {
                    return err_result(format!("Failed to parse bulk_update_tasks args: {e}"))
                }
            };
            let targets: Vec<String> = parsed
                .target_ids
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if targets.is_empty() {
                return err_result("bulk_update_tasks: empty target_ids".to_string());
            }
            AiResult {
                task_id: job.task_id,
                update: TaskUpdate::default(),
                error: None,
                triage_action: Some(TriageAction::BulkUpdate {
                    targets,
                    instruction: parsed.instruction,
                }),
                sub_task_specs: Vec::new(),
            }
        }
        "remember_fact" => {
            let parsed: RememberFactArgs = match serde_json::from_value(args) {
                Ok(v) => v,
                Err(e) => return err_result(format!("Failed to parse remember_fact args: {e}")),
            };
            let fact = parsed.fact.trim().to_string();
            if fact.is_empty() {
                return err_result("remember_fact: empty fact".to_string());
            }
            AiResult {
                task_id: job.task_id,
                update: TaskUpdate::default(),
                error: None,
                triage_action: Some(TriageAction::RememberFact(fact)),
                sub_task_specs: Vec::new(),
            }
        }
        other => err_result(format!("Unknown tool: {other}")),
    }
}

fn parse_progress(input: &str) -> Option<Progress> {
    match input.to_ascii_lowercase().as_str() {
        "backlog" => Some(Progress::Backlog),
        "todo" => Some(Progress::Todo),
        "in progress" | "inprogress" | "in-progress" => Some(Progress::InProgress),
        "done" => Some(Progress::Done),
        "archived" => Some(Progress::Archived),
        _ => None,
    }
}

fn truncate(input: &str, max: usize) -> &str {
    if input.len() <= max {
        return input;
    }
    &input[..max]
}

// ---------------------------------------------------------------------------
// Vision / image ingestion
// ---------------------------------------------------------------------------

fn call_llm_with_image(
    cfg: &LlmConfig,
    system: &str,
    user_text: &str,
    image_base64: &str,
    media_type: &str,
) -> Result<String, String> {
    let body = match cfg.provider {
        Provider::OpenAi => json!({
            "model": cfg.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": [
                    {
                        "type": "image_url",
                        "image_url": {
                            "url": format!("data:{};base64,{}", media_type, image_base64)
                        }
                    },
                    {"type": "text", "text": user_text}
                ]}
            ]
        }),
        Provider::Anthropic => json!({
            "model": cfg.model,
            "max_tokens": 4096,
            "system": system,
            "messages": [
                {"role": "user", "content": [
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": media_type,
                            "data": image_base64
                        }
                    },
                    {"type": "text", "text": user_text}
                ]}
            ]
        }),
    };

    let text = with_retry(|attempt| {
        let timeout = scaled_timeout(cfg.timeout, &body, attempt);
        send_llm_request(cfg, &body, timeout)
    })?;

    match cfg.provider {
        Provider::OpenAi => {
            let chat: ChatResponse = serde_json::from_str(&text)
                .map_err(|err| format!("AI JSON parse failed: {err}"))?;
            Ok(chat
                .choices
                .first()
                .and_then(|c| c.message.content.as_deref())
                .unwrap_or("")
                .to_string())
        }
        Provider::Anthropic => {
            let resp: AnthropicResponse = serde_json::from_str(&text)
                .map_err(|err| format!("AI JSON parse failed: {err}"))?;
            Ok(resp
                .content
                .iter()
                .filter_map(|b| {
                    if b.block_type == "text" {
                        b.text.as_deref()
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join(""))
        }
    }
}

pub fn extract_from_image(
    settings: &AiSettings,
    image_data: &[u8],
    media_type: &str,
) -> Result<String, String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    let cfg = build_config(settings)
        .ok_or_else(|| "AI not configured. Set ANTHROPIC_API_KEY or OPENAI_API_KEY.".to_string())?;

    let image_base64 = STANDARD.encode(image_data);

    let system = "You are an expert at extracting actionable tasks from images. \
        Analyze the image and identify all actionable items, tasks, to-dos, requests, or follow-ups.";

    let user_text = "Extract all actionable tasks from this image. \
        Return a concise natural language instruction that a project manager could execute \
        to create these tasks. Include relevant details like due dates, priorities, and context. \
        Do NOT return JSON. Return plain text instructions, e.g. \
        'Create a high-priority task to complete 2025 Corporate Tax Form by Feb 28. \
        Also create a task for 2025 DE Franchise Form.'";

    call_llm_with_image(&cfg, system, user_text, &image_base64, media_type)
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct SuggestedTask {
    pub title: String,
    pub description: String,
    pub priority: String,
}

pub fn filter_email_for_suggestions(
    settings: &AiSettings,
    subject: &str,
    sender: &str,
    content: &str,
) -> Result<Option<SuggestedTask>, String> {
    let cfg = build_config(settings)
        .ok_or_else(|| "AI not configured. Set ANTHROPIC_API_KEY or OPENAI_API_KEY.".to_string())?;

    let system = "You are an AI assistant that filters emails to identify actionable tasks. \
        Your job is to determine if an email contains something the user needs to act on. \
        Ignore: newsletters, marketing, promotional emails, automated reports, spam. \
        Look for: requests, deadlines, follow-ups, meeting invitations, pending items.";

    let user = format!(
        "Analyze this email and determine if it requires action.\n\n\
        From: {}\nSubject: {}\n\n{}\n\n\
        If this email requires action, respond with JSON: {{\"actionable\": true, \"title\": \"short task title\", \"description\": \"brief summary\", \"priority\": \"Low|Medium|High|Critical\"}}\n\
        If NOT actionable (newsletter, spam, marketing, etc.), respond with: {{\"actionable\": false}}",
        sender, subject, truncate(content, 800)
    );

    let response = call_llm(&cfg, system, &user)?;
    let json_text = extract_json_object(&response).unwrap_or_else(|| response.trim().to_string());

    #[derive(serde::Deserialize)]
    struct FilterResponse {
        actionable: bool,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        priority: Option<String>,
    }

    let parsed: FilterResponse = serde_json::from_str(&json_text)
        .map_err(|e| format!("Failed to parse filter response: {e}"))?;

    if !parsed.actionable {
        return Ok(None);
    }

    Ok(Some(SuggestedTask {
        title: parsed.title.unwrap_or_else(|| subject.to_string()),
        description: parsed
            .description
            .unwrap_or_else(|| format!("From: {}", sender)),
        priority: parsed.priority.unwrap_or_else(|| "Medium".to_string()),
    }))
}
