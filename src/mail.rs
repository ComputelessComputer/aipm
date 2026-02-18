use std::process::Command;

#[derive(Debug, Clone)]
pub struct Email {
    pub id: String,
    pub subject: String,
    pub sender: String,
    pub date: String,
    pub content: Option<String>,
    pub is_read: bool,
}

pub fn get_recent_emails(limit: u32) -> Result<Vec<Email>, String> {
    let script = format!(
        r#"
tell application "Mail"
    set output to ""
    set msgList to messages of inbox
    set maxCount to {limit}
    if (count of msgList) < maxCount then set maxCount to (count of msgList)
    repeat with i from 1 to maxCount
        set msg to item i of msgList
        set msgId to message id of msg
        set msgSubject to subject of msg
        set msgSender to sender of msg
        set msgDate to date received of msg as string
        set msgContent to content of msg
        set msgRead to read status of msg
        set readStr to "false"
        if msgRead then set readStr to "true"
        set output to output & msgId & "	" & msgSubject & "	" & msgSender & "	" & msgDate & "	" & readStr & "	" & msgContent & linefeed
    end repeat
    return output
end tell
"#
    );

    let output = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|e| format!("Failed to run osascript: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("osascript failed: {stderr}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut emails = Vec::new();

    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.splitn(6, '\t').collect();
        if parts.len() < 5 {
            continue;
        }
        emails.push(Email {
            id: parts[0].to_string(),
            subject: parts[1].to_string(),
            sender: parts[2].to_string(),
            date: parts[3].to_string(),
            is_read: parts[4] == "true",
            content: parts.get(5).map(|s| s.to_string()),
        });
    }

    Ok(emails)
}
