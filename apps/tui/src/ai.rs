use chrono::NaiveDate;

use crate::model::Priority;

#[derive(Debug, Clone)]
pub struct NewTaskHints {
    pub bucket: String,
    pub _bucket_locked: bool,
    pub priority: Option<Priority>,
    pub due_date: Option<NaiveDate>,
    pub title: String,
}

pub fn infer_new_task(input: &str, bucket_names: &[String]) -> Option<NewTaskHints> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Allow manual overrides:
    //   bucketname: ...
    // And inline hints:
    //   due:YYYY-MM-DD
    //   p:low|medium|high|critical
    let (bucket_override, rest) = parse_bucket_prefix(trimmed, bucket_names);
    let bucket_locked = bucket_override.is_some();

    let (due_date, rest) = parse_due_date_hint(rest);
    let (priority, title) = parse_priority_hint(&rest);

    let title = title.trim();
    if title.is_empty() {
        return None;
    }

    let bucket = bucket_override.unwrap_or_else(|| default_bucket(bucket_names));

    Some(NewTaskHints {
        bucket,
        _bucket_locked: bucket_locked,
        priority,
        due_date,
        title: title.to_string(),
    })
}

fn parse_bucket_prefix<'a>(input: &'a str, bucket_names: &[String]) -> (Option<String>, &'a str) {
    let lower = input.to_ascii_lowercase();
    for name in bucket_names {
        let prefix = format!("{}:", name.to_ascii_lowercase());
        if lower.starts_with(&prefix) {
            return (Some(name.clone()), input[prefix.len()..].trim_start());
        }
    }
    (None, input)
}

fn parse_due_date_hint(input: &str) -> (Option<NaiveDate>, String) {
    // Naive parse: look for a token like due:2026-02-15
    // If multiple, use the first.
    let mut due = None;
    let mut out_tokens: Vec<&str> = Vec::new();

    for token in input.split_whitespace() {
        if due.is_none() {
            if let Some(value) = token.strip_prefix("due:") {
                if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
                    due = Some(date);
                    continue;
                }
            }
        }
        out_tokens.push(token);
    }

    (due, out_tokens.join(" "))
}

fn parse_priority_hint(input: &str) -> (Option<Priority>, String) {
    let mut prio = None;
    let mut out_tokens: Vec<&str> = Vec::new();

    for token in input.split_whitespace() {
        if prio.is_none() {
            if let Some(value) = token.strip_prefix("p:") {
                prio = match value.to_ascii_lowercase().as_str() {
                    "low" => Some(Priority::Low),
                    "med" | "medium" => Some(Priority::Medium),
                    "high" => Some(Priority::High),
                    "crit" | "critical" => Some(Priority::Critical),
                    _ => None,
                };
                if prio.is_some() {
                    continue;
                }
            }
        }
        out_tokens.push(token);
    }

    (prio, out_tokens.join(" "))
}

/// Return the first bucket name as the default.
pub fn default_bucket(bucket_names: &[String]) -> String {
    bucket_names
        .first()
        .cloned()
        .unwrap_or_else(|| "Unassigned".to_string())
}
