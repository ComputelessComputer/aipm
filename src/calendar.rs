use std::process::Command;

#[derive(Debug, Clone)]
pub struct CalendarEvent {
    pub id: String,
    pub title: String,
    pub start_date: String,
    pub end_date: String,
    pub calendar_name: String,
    pub location: Option<String>,
    pub notes: Option<String>,
    pub all_day: bool,
}

pub fn get_upcoming_events(days: u32) -> Result<Vec<CalendarEvent>, String> {
    let script = format!(
        r#"
set today to current date
set futureDate to today + ({days} * days)
set output to ""
tell application "Calendar"
    repeat with cal in calendars
        set calName to name of cal
        set evts to (every event of cal whose start date >= today and start date <= futureDate)
        repeat with evt in evts
            set evtId to uid of evt
            set evtTitle to summary of evt
            set evtStart to start date of evt as string
            set evtEnd to end date of evt as string
            set evtAllDay to allday event of evt
            set allDayStr to "false"
            if evtAllDay then set allDayStr to "true"
            set evtLocation to ""
            try
                set evtLocation to location of evt
            end try
            set evtNotes to ""
            try
                set evtNotes to description of evt
            end try
            if evtNotes is missing value then set evtNotes to ""
            if evtLocation is missing value then set evtLocation to ""
            set output to output & evtId & "	" & evtTitle & "	" & evtStart & "	" & evtEnd & "	" & calName & "	" & allDayStr & "	" & evtLocation & "	" & evtNotes & linefeed
        end repeat
    end repeat
end tell
return output
"#
    );

    let output = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|e| format!("Failed to run osascript: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("osascript (Calendar) failed: {stderr}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut events = Vec::new();

    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.splitn(8, '\t').collect();
        if parts.len() < 6 {
            continue;
        }
        events.push(CalendarEvent {
            id: parts[0].to_string(),
            title: parts[1].to_string(),
            start_date: parts[2].to_string(),
            end_date: parts[3].to_string(),
            calendar_name: parts[4].to_string(),
            all_day: parts[5] == "true",
            location: parts
                .get(6)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            notes: parts
                .get(7)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
        });
    }

    Ok(events)
}
