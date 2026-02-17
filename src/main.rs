mod ai;
mod cli;
mod llm;
mod model;
mod storage;

use std::io::{self, Stdout, Write};
use std::time::{Duration, Instant};

use chrono::Utc;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute, queue,
    style::{
        Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
    },
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use uuid::Uuid;

use crate::model::{children_of, Priority, Progress, Task};
use crate::storage::{AiSettings, Storage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Default,
    Timeline,
    Kanban,
    Settings,
}

const MODEL_OPTIONS: &[&str] = &[
    // Anthropic
    "claude-opus-4-6",
    "claude-opus-4-5",
    "claude-sonnet-4-5",
    // OpenAI
    "codex-mini-latest",
    "o3",
    "o4-mini",
];

impl Tab {
    fn next(self) -> Tab {
        match self {
            Tab::Default => Tab::Timeline,
            Tab::Timeline => Tab::Kanban,
            Tab::Kanban => Tab::Settings,
            Tab::Settings => Tab::Default,
        }
    }

    fn prev(self) -> Tab {
        match self {
            Tab::Default => Tab::Settings,
            Tab::Timeline => Tab::Default,
            Tab::Kanban => Tab::Timeline,
            Tab::Settings => Tab::Kanban,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Tabs,
    Board,
    Input,
    Edit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditField {
    Title,
    Description,
    Bucket,
    Progress,
    Priority,
    DueDate,
    SubIssues,
}

impl EditField {
    const ALL: [EditField; 7] = [
        EditField::Title,
        EditField::Description,
        EditField::Bucket,
        EditField::Progress,
        EditField::Priority,
        EditField::DueDate,
        EditField::SubIssues,
    ];

    fn label(self) -> &'static str {
        match self {
            EditField::Title => "Title",
            EditField::Description => "Description",
            EditField::Bucket => "Bucket",
            EditField::Progress => "Progress",
            EditField::Priority => "Priority",
            EditField::DueDate => "Due date",
            EditField::SubIssues => "Sub-issues",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BucketEditField {
    Name,
    Description,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsField {
    OwnerName,
    AiEnabled,
    OpenAiKey,
    AnthropicKey,
    Model,
    ApiUrl,
    Timeout,
    ShowBacklog,
    ShowTodo,
    ShowInProgress,
    ShowDone,
}

impl SettingsField {
    const ALL: [SettingsField; 11] = [
        SettingsField::OwnerName,
        SettingsField::AiEnabled,
        SettingsField::OpenAiKey,
        SettingsField::AnthropicKey,
        SettingsField::Model,
        SettingsField::ApiUrl,
        SettingsField::Timeout,
        SettingsField::ShowBacklog,
        SettingsField::ShowTodo,
        SettingsField::ShowInProgress,
        SettingsField::ShowDone,
    ];

    fn label(self) -> &'static str {
        match self {
            SettingsField::OwnerName => "Owner Name",
            SettingsField::AiEnabled => "AI Enabled",
            SettingsField::OpenAiKey => "OpenAI Key",
            SettingsField::AnthropicKey => "Anthropic Key",
            SettingsField::Model => "Model",
            SettingsField::ApiUrl => "API URL",
            SettingsField::Timeout => "Timeout (sec)",
            SettingsField::ShowBacklog => "Show Backlog",
            SettingsField::ShowTodo => "Show Todo",
            SettingsField::ShowInProgress => "Show In Prog.",
            SettingsField::ShowDone => "Show Done",
        }
    }

    fn is_toggle(self) -> bool {
        matches!(
            self,
            SettingsField::AiEnabled
                | SettingsField::ShowBacklog
                | SettingsField::ShowTodo
                | SettingsField::ShowInProgress
                | SettingsField::ShowDone
        )
    }
}

fn default_bucket_name(settings: &AiSettings) -> String {
    settings
        .buckets
        .first()
        .map(|b| b.name.clone())
        .unwrap_or_else(|| "Unassigned".to_string())
}

fn bucket_names(settings: &AiSettings) -> Vec<String> {
    settings.buckets.iter().map(|b| b.name.clone()).collect()
}

struct App {
    storage: Option<Storage>,
    tasks: Vec<Task>,
    ai: Option<llm::AiRuntime>,
    tab: Tab,
    focus: Focus,

    selected_bucket: usize,
    selected_task_id: Option<Uuid>,

    bucket_scrolls: Vec<usize>,

    input: String,
    input_cursor: usize,
    status: Option<(String, Instant, bool)>,

    edit_task_id: Option<Uuid>,
    edit_field: EditField,
    edit_buf: String,
    editing_text: bool,
    edit_sub_selected: usize,
    edit_parent_stack: Vec<(Uuid, EditField, usize)>,

    timeline_selected: usize,
    timeline_scroll: usize,

    kanban_stage: Progress,
    kanban_selected: Option<Uuid>,
    kanban_scroll: [usize; 4],

    confirm_delete_id: Option<Uuid>,

    bucket_header_selected: bool,
    bucket_edit_active: bool,
    bucket_edit_field: BucketEditField,
    bucket_edit_buf: String,
    bucket_editing_text: bool,

    settings: AiSettings,
    settings_field: SettingsField,
    settings_buf: String,
    settings_editing: bool,

    chat_history: Vec<llm::ChatEntry>,
    last_triage_input: String,

    input_history: Vec<String>,
    input_history_index: Option<usize>,
    input_saved: String,

    at_autocomplete_selected: usize,
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter(stdout: &mut Stdout) -> io::Result<TerminalGuard> {
        terminal::enable_raw_mode()?;
        execute!(stdout, EnterAlternateScreen, Hide)?;
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        let _ = terminal::disable_raw_mode();
        let _ = execute!(stdout, Show, LeaveAlternateScreen);
    }
}

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return Ok(());
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("aipm {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // CLI subcommands: task, bucket.
    if let Some(result) = cli::run_subcommand(&args) {
        return result;
    }

    // CLI mode: `aipm "break down all tickets"` — headless AI, no TUI.
    let positional: Vec<&str> = args[1..]
        .iter()
        .filter(|a| !a.starts_with('-'))
        .map(|s| s.as_str())
        .collect();
    if !positional.is_empty() {
        let instruction = positional.join(" ");
        return run_cli(&instruction);
    }

    let storage = Storage::new();
    let tasks = match &storage {
        Some(s) => match s.load_tasks() {
            Ok(tasks) => tasks,
            Err(err) => {
                eprintln!("Failed to load tasks: {err}");
                Vec::new()
            }
        },
        None => Vec::new(),
    };
    let settings = match &storage {
        Some(s) => s.load_settings().unwrap_or_default(),
        None => AiSettings::default(),
    };

    let bucket_count = settings.buckets.len();
    let mut app = App {
        storage,
        tasks,
        ai: llm::AiRuntime::from_settings(&settings),
        tab: Tab::Default,
        focus: Focus::Input,
        selected_bucket: 0,
        selected_task_id: None,
        bucket_scrolls: vec![0; bucket_count],
        input: String::new(),
        input_cursor: 0,
        status: None,
        edit_task_id: None,
        edit_field: EditField::Title,
        edit_buf: String::new(),
        editing_text: false,
        edit_sub_selected: 0,
        edit_parent_stack: Vec::new(),
        timeline_selected: 0,
        timeline_scroll: 0,
        kanban_stage: Progress::Backlog,
        kanban_selected: None,
        kanban_scroll: [0; 4],
        confirm_delete_id: None,
        bucket_header_selected: false,
        bucket_edit_active: false,
        bucket_edit_field: BucketEditField::Name,
        bucket_edit_buf: String::new(),
        bucket_editing_text: false,
        settings,
        settings_field: SettingsField::AiEnabled,
        settings_buf: String::new(),
        settings_editing: false,
        chat_history: Vec::new(),
        last_triage_input: String::new(),
        input_history: Vec::new(),
        input_history_index: None,
        input_saved: String::new(),
        at_autocomplete_selected: 0,
    };

    ensure_default_selection(&mut app);

    let mut stdout = io::stdout();
    let _guard = TerminalGuard::enter(&mut stdout)?;

    run_app(&mut stdout, &mut app)
}

const TOAST_DURATION: Duration = Duration::from_secs(3);

fn run_app(stdout: &mut Stdout, app: &mut App) -> io::Result<()> {
    let mut needs_redraw = true;
    let mut needs_clear = true; // full screen clear on first draw

    loop {
        if poll_ai(app) {
            needs_redraw = true;
        }

        // Auto-dismiss toast after timeout (skip for persistent toasts).
        if let Some((_, shown_at, persistent)) = &app.status {
            if !persistent && shown_at.elapsed() >= TOAST_DURATION {
                app.status = None;
                needs_redraw = true;
                needs_clear = true; // clear toast remnants
            } else {
                // Redraw to update the countdown ticker / spinner.
                needs_redraw = true;
            }
        }

        if needs_redraw {
            render(stdout, app, needs_clear)?;
            needs_redraw = false;
            needs_clear = false;
        }

        if event::poll(Duration::from_millis(200))? {
            let prev_tab = app.tab;
            let prev_focus = app.focus;
            let prev_edit = app.edit_task_id;
            let prev_confirm = app.confirm_delete_id;
            let prev_bucket_edit = app.bucket_edit_active;
            let prev_header_sel = app.bucket_header_selected;
            let prev_at_ac = input_has_at_prefix(&app.input) && app.focus == Focus::Input;
            match event::read()? {
                Event::Key(key) => {
                    if handle_key(app, key)? {
                        break;
                    }
                    needs_redraw = true;
                    let cur_at_ac = input_has_at_prefix(&app.input) && app.focus == Focus::Input;
                    // Full clear when layout changes significantly.
                    if app.tab != prev_tab
                        || app.focus != prev_focus
                        || app.edit_task_id != prev_edit
                        || app.confirm_delete_id != prev_confirm
                        || app.bucket_edit_active != prev_bucket_edit
                        || app.bucket_header_selected != prev_header_sel
                        || prev_at_ac != cur_at_ac
                    {
                        needs_clear = true;
                    }
                }
                Event::Resize(_, _) => {
                    needs_redraw = true;
                    needs_clear = true;
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    // Ctrl-C always quits.
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Ok(true);
    }

    // Toast dismissal intercepts all keys (skip for persistent toasts).
    if let Some((_, _, persistent)) = &app.status {
        if !persistent {
            app.status = None;
            return Ok(false);
        }
    }

    // Delete confirmation intercepts all keys.
    if app.confirm_delete_id.is_some() {
        return handle_confirm_delete_key(app, key);
    }

    // Bucket edit overlay intercepts all keys.
    if app.bucket_edit_active {
        return handle_bucket_edit_key(app, key);
    }

    // Edit overlay intercepts all keys.
    if app.focus == Focus::Edit {
        return handle_edit_key(app, key);
    }

    // Settings field editing intercepts all keys.
    if app.tab == Tab::Settings && app.settings_editing {
        return handle_settings_edit_key(app, key);
    }

    // Tab bar navigation intercepts all keys.
    if app.focus == Focus::Tabs {
        return handle_tabs_key(app, key);
    }

    // Tab switching with 1/2/3/4 (not while typing in input).
    if app.focus != Focus::Input {
        match key.code {
            KeyCode::Char('1') => {
                app.tab = Tab::Default;
                app.focus = Focus::Input;
                app.status = None;
                return Ok(false);
            }
            KeyCode::Char('2') => {
                app.tab = Tab::Timeline;
                app.focus = Focus::Board;
                app.status = None;
                return Ok(false);
            }
            KeyCode::Char('3') => {
                app.tab = Tab::Kanban;
                app.focus = Focus::Board;
                app.status = None;
                return Ok(false);
            }
            KeyCode::Char('4') => {
                app.tab = Tab::Settings;
                app.focus = Focus::Board;
                app.settings_editing = false;
                app.status = None;
                return Ok(false);
            }
            _ => {}
        }
    }

    match app.tab {
        Tab::Default => handle_default_tab_key(app, key),
        Tab::Timeline => handle_timeline_key(app, key),
        Tab::Kanban => handle_kanban_key(app, key),
        Tab::Settings => handle_settings_key(app, key),
    }
}

fn handle_tabs_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    match key.code {
        KeyCode::Left | KeyCode::Char('h') => {
            app.tab = app.tab.prev();
        }
        KeyCode::Right | KeyCode::Char('l') => {
            app.tab = app.tab.next();
        }
        KeyCode::Enter | KeyCode::Down | KeyCode::Char('j') => {
            app.focus = match app.tab {
                Tab::Default => Focus::Input,
                _ => Focus::Board,
            };
            app.status = None;
        }
        KeyCode::Char('1') => {
            app.tab = Tab::Default;
            app.focus = Focus::Input;
            app.status = None;
        }
        KeyCode::Char('2') => {
            app.tab = Tab::Timeline;
            app.focus = Focus::Board;
            app.status = None;
        }
        KeyCode::Char('3') => {
            app.tab = Tab::Kanban;
            app.focus = Focus::Board;
            app.status = None;
        }
        KeyCode::Char('4') => {
            app.tab = Tab::Settings;
            app.focus = Focus::Board;
            app.settings_editing = false;
            app.status = None;
        }
        _ => {}
    }
    Ok(false)
}

fn sorted_timeline_tasks(tasks: &[Task]) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..tasks.len()).collect();
    indices.sort_by(|&a, &b| {
        let a_start = tasks[a]
            .start_date
            .map(|dt| dt.date_naive())
            .unwrap_or_else(|| tasks[a].created_at.date_naive());
        let b_start = tasks[b]
            .start_date
            .map(|dt| dt.date_naive())
            .unwrap_or_else(|| tasks[b].created_at.date_naive());
        a_start.cmp(&b_start)
    });
    indices
}

fn handle_timeline_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    match key.code {
        KeyCode::Esc => {
            app.focus = Focus::Tabs;
            return Ok(false);
        }
        KeyCode::Char('i') => {
            app.tab = Tab::Default;
            app.focus = Focus::Input;
            return Ok(false);
        }
        KeyCode::Enter | KeyCode::Char('e') => {
            let indices = sorted_timeline_tasks(&app.tasks);
            if let Some(&idx) = indices.get(app.timeline_selected) {
                let task_id = app.tasks[idx].id;
                app.selected_task_id = Some(task_id);
                open_edit_for(app, task_id);
            }
            return Ok(false);
        }
        _ => {}
    }

    let count = app.tasks.len();
    if count == 0 {
        return Ok(false);
    }

    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            if app.timeline_selected == 0 {
                app.timeline_selected = count - 1;
            } else {
                app.timeline_selected -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.timeline_selected = (app.timeline_selected + 1) % count;
        }
        _ => {}
    }

    Ok(false)
}

fn handle_default_tab_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    match app.focus {
        Focus::Input => handle_input_key(app, key),
        Focus::Board => handle_board_key(app, key),
        Focus::Edit => handle_edit_key(app, key),
        Focus::Tabs => Ok(false), // handled earlier in handle_key
    }
}

/// Convert a char-based index into the byte offset within `s`.
fn char_byte_pos(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

/// Return the visible slice of `input` that keeps `cursor_char` on-screen,
/// plus the visual x-offset of the cursor within that slice.
fn input_visible_window(input: &str, cursor_char: usize, max_width: usize) -> (String, usize) {
    use unicode_width::UnicodeWidthChar;

    let chars: Vec<char> = input.chars().collect();
    let cursor_char = cursor_char.min(chars.len());

    let width_before: usize = chars[..cursor_char]
        .iter()
        .map(|c| UnicodeWidthChar::width(*c).unwrap_or(0))
        .sum();

    if input.width() <= max_width {
        return (input.to_string(), width_before);
    }

    if width_before < max_width {
        // Cursor visible when showing from start.
        let mut out = String::new();
        let mut w = 0;
        for &ch in &chars {
            let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
            if w + cw > max_width {
                break;
            }
            out.push(ch);
            w += cw;
        }
        return (out, width_before);
    }

    // Scroll so cursor is near the right edge.
    let mut start = cursor_char;
    let mut vis_w = 0;
    while start > 0 {
        let cw = UnicodeWidthChar::width(chars[start - 1]).unwrap_or(0);
        if vis_w + cw > max_width {
            break;
        }
        start -= 1;
        vis_w += cw;
    }

    let mut out = String::new();
    let mut w = 0;
    for &ch in &chars[start..] {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > max_width {
            break;
        }
        out.push(ch);
        w += cw;
    }

    let cursor_offset: usize = chars[start..cursor_char]
        .iter()
        .map(|c| UnicodeWidthChar::width(*c).unwrap_or(0))
        .sum();

    (out, cursor_offset)
}

/// Compute @ autocomplete completions from the current input.
/// Active when input starts with `@` and the token after `@` has no space yet.
fn at_completions(tasks: &[Task], input: &str) -> Vec<(String, String, String)> {
    let trimmed = input.trim_start();
    if !trimmed.starts_with('@') {
        return Vec::new();
    }
    let after_at = &trimmed[1..];
    if after_at.contains(' ') {
        return Vec::new();
    }
    let query = after_at.to_ascii_lowercase();
    let mut matches: Vec<(String, String, String)> = tasks
        .iter()
        .filter(|t| {
            if query.is_empty() {
                return true;
            }
            let short =
                t.id.to_string()
                    .chars()
                    .take(8)
                    .collect::<String>()
                    .to_ascii_lowercase();
            let title_lower = t.title.to_ascii_lowercase();
            short.starts_with(&query) || title_lower.contains(&query)
        })
        .map(|t| {
            let short = t.id.to_string().chars().take(8).collect::<String>();
            (short, t.title.clone(), t.bucket.clone())
        })
        .collect();
    matches.truncate(20);
    matches
}

/// Check whether the input is in the @ prefix state (autocomplete eligible).
fn input_has_at_prefix(input: &str) -> bool {
    let t = input.trim_start();
    t.starts_with('@') && !t[1..].contains(' ')
}

fn handle_input_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    // @ autocomplete interception.
    let completions = at_completions(&app.tasks, &app.input);
    if !completions.is_empty() {
        match key.code {
            KeyCode::Up => {
                let len = completions.len();
                if app.at_autocomplete_selected == 0 {
                    app.at_autocomplete_selected = len - 1;
                } else {
                    app.at_autocomplete_selected -= 1;
                }
                return Ok(false);
            }
            KeyCode::Down => {
                app.at_autocomplete_selected =
                    (app.at_autocomplete_selected + 1) % completions.len();
                return Ok(false);
            }
            KeyCode::Enter | KeyCode::Tab => {
                let sel = app
                    .at_autocomplete_selected
                    .min(completions.len().saturating_sub(1));
                let (short_id, _, _) = &completions[sel];
                app.input = format!("@{} ", short_id);
                app.input_cursor = app.input.chars().count();
                app.at_autocomplete_selected = 0;
                return Ok(false);
            }
            _ => {
                // Reset selection when typing/editing.
                app.at_autocomplete_selected = 0;
            }
        }
    }

    match key.code {
        KeyCode::Esc => {
            app.focus = Focus::Board;
            app.status = None;
            Ok(false)
        }
        KeyCode::Tab => {
            app.focus = Focus::Board;
            Ok(false)
        }
        KeyCode::Left => {
            if app.input_cursor > 0 {
                app.input_cursor -= 1;
            }
            Ok(false)
        }
        KeyCode::Right => {
            if app.input_cursor < app.input.chars().count() {
                app.input_cursor += 1;
            }
            Ok(false)
        }
        KeyCode::Up => {
            if app.input_history.is_empty() {
                return Ok(false);
            }
            match app.input_history_index {
                None => {
                    app.input_saved = app.input.clone();
                    let idx = app.input_history.len() - 1;
                    app.input_history_index = Some(idx);
                    app.input = app.input_history[idx].clone();
                    app.input_cursor = app.input.chars().count();
                }
                Some(idx) if idx > 0 => {
                    let new_idx = idx - 1;
                    app.input_history_index = Some(new_idx);
                    app.input = app.input_history[new_idx].clone();
                    app.input_cursor = app.input.chars().count();
                }
                _ => {}
            }
            Ok(false)
        }
        KeyCode::Down => {
            match app.input_history_index {
                Some(idx) if idx < app.input_history.len() - 1 => {
                    let new_idx = idx + 1;
                    app.input_history_index = Some(new_idx);
                    app.input = app.input_history[new_idx].clone();
                    app.input_cursor = app.input.chars().count();
                }
                Some(_) => {
                    app.input_history_index = None;
                    app.input = app.input_saved.clone();
                    app.input_saved.clear();
                    app.input_cursor = app.input.chars().count();
                }
                None => {}
            }
            Ok(false)
        }
        KeyCode::Enter => {
            // Push non-empty input to history.
            let trimmed_for_history = app.input.trim().to_string();
            if !trimmed_for_history.is_empty() {
                app.input_history.push(trimmed_for_history);
            }
            app.input_history_index = None;
            app.input_saved.clear();

            if app.input.trim().eq_ignore_ascii_case("/exit") {
                return Ok(true);
            }

            // /clear: reset AI conversation context.
            if app.input.trim().eq_ignore_ascii_case("/clear") {
                app.chat_history.clear();
                app.status = Some(("Context cleared".to_string(), Instant::now(), false));
                app.input.clear();
                app.input_cursor = 0;
                return Ok(false);
            }

            // /buckets: list all buckets.
            if app.input.trim().eq_ignore_ascii_case("/buckets") {
                let names: Vec<String> = app
                    .settings
                    .buckets
                    .iter()
                    .map(|b| {
                        if let Some(desc) = &b.description {
                            format!("{} — {}", b.name, desc)
                        } else {
                            b.name.clone()
                        }
                    })
                    .collect();
                app.status = Some((
                    format!("Buckets: {}", names.join(", ")),
                    Instant::now(),
                    false,
                ));
                app.input.clear();
                app.input_cursor = 0;
                return Ok(false);
            }

            // /bucket add <name>: add a new bucket.
            if let Some(rest) = app.input.trim().strip_prefix("/bucket add ") {
                let name = rest.trim().to_string();
                if name.is_empty() {
                    app.status = Some((
                        "Usage: /bucket add <name>".to_string(),
                        Instant::now(),
                        false,
                    ));
                } else if app
                    .settings
                    .buckets
                    .iter()
                    .any(|b| b.name.eq_ignore_ascii_case(&name))
                {
                    app.status = Some((
                        format!("Bucket \"{}\" already exists", name),
                        Instant::now(),
                        false,
                    ));
                } else {
                    app.settings.buckets.push(crate::model::BucketDef {
                        name: name.clone(),
                        description: None,
                    });
                    app.bucket_scrolls.push(0);
                    persist_settings(app);
                    app.status = Some((format!("Added bucket: {}", name), Instant::now(), false));
                }
                app.input.clear();
                app.input_cursor = 0;
                return Ok(false);
            }

            // /bucket rename <old> <new>: rename a bucket.
            if let Some(rest) = app.input.trim().strip_prefix("/bucket rename ") {
                let parts: Vec<&str> = rest.splitn(2, ' ').collect();
                if parts.len() < 2 || parts[0].trim().is_empty() || parts[1].trim().is_empty() {
                    app.status = Some((
                        "Usage: /bucket rename <old> <new>".to_string(),
                        Instant::now(),
                        false,
                    ));
                } else {
                    let old = parts[0].trim();
                    let new_name = parts[1].trim().to_string();
                    if let Some(bucket) = app
                        .settings
                        .buckets
                        .iter_mut()
                        .find(|b| b.name.eq_ignore_ascii_case(old))
                    {
                        let old_name = bucket.name.clone();
                        bucket.name = new_name.clone();
                        // Update all tasks in that bucket.
                        for task in &mut app.tasks {
                            if task.bucket.eq_ignore_ascii_case(&old_name) {
                                task.bucket = new_name.clone();
                            }
                        }
                        persist_settings(app);
                        persist(app);
                        app.status = Some((
                            format!("Renamed: {} → {}", old_name, new_name),
                            Instant::now(),
                            false,
                        ));
                    } else {
                        app.status = Some((
                            format!("Bucket \"{}\" not found", old),
                            Instant::now(),
                            false,
                        ));
                    }
                }
                app.input.clear();
                app.input_cursor = 0;
                return Ok(false);
            }

            // /bucket desc <name> <description>: set bucket description.
            if let Some(rest) = app.input.trim().strip_prefix("/bucket desc ") {
                let parts: Vec<&str> = rest.splitn(2, ' ').collect();
                if parts.is_empty() || parts[0].trim().is_empty() {
                    app.status = Some((
                        "Usage: /bucket desc <name> <description>".to_string(),
                        Instant::now(),
                        false,
                    ));
                } else {
                    let name = parts[0].trim();
                    let desc = parts
                        .get(1)
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                    if let Some(bucket) = app
                        .settings
                        .buckets
                        .iter_mut()
                        .find(|b| b.name.eq_ignore_ascii_case(name))
                    {
                        let bname = bucket.name.clone();
                        bucket.description = desc.clone();
                        let msg = if desc.is_some() {
                            format!("Updated description for {}", bname)
                        } else {
                            format!("Cleared description for {}", bname)
                        };
                        persist_settings(app);
                        app.status = Some((msg, Instant::now(), false));
                    } else {
                        app.status = Some((
                            format!("Bucket \"{}\" not found", name),
                            Instant::now(),
                            false,
                        ));
                    }
                }
                app.input.clear();
                app.input_cursor = 0;
                return Ok(false);
            }

            // /bucket delete <name>: delete a bucket (moves tasks to first bucket).
            if let Some(rest) = app.input.trim().strip_prefix("/bucket delete ") {
                let name = rest.trim();
                if name.is_empty() {
                    app.status = Some((
                        "Usage: /bucket delete <name>".to_string(),
                        Instant::now(),
                        false,
                    ));
                } else if app.settings.buckets.len() <= 1 {
                    app.status = Some((
                        "Cannot delete the last bucket".to_string(),
                        Instant::now(),
                        false,
                    ));
                } else if let Some(pos) = app
                    .settings
                    .buckets
                    .iter()
                    .position(|b| b.name.eq_ignore_ascii_case(name))
                {
                    let removed_name = app.settings.buckets[pos].name.clone();
                    app.settings.buckets.remove(pos);
                    if pos < app.bucket_scrolls.len() {
                        app.bucket_scrolls.remove(pos);
                    }
                    // Move tasks from deleted bucket to first remaining bucket.
                    let fallback = default_bucket_name(&app.settings);
                    let mut moved = 0usize;
                    for task in &mut app.tasks {
                        if task.bucket.eq_ignore_ascii_case(&removed_name) {
                            task.bucket = fallback.clone();
                            moved += 1;
                        }
                    }
                    // Clamp selected_bucket.
                    if app.selected_bucket >= app.settings.buckets.len() {
                        app.selected_bucket = app.settings.buckets.len().saturating_sub(1);
                    }
                    persist_settings(app);
                    if moved > 0 {
                        persist(app);
                    }
                    let msg = if moved > 0 {
                        format!(
                            "Deleted bucket \"{}\" ({} task{} → {})",
                            removed_name,
                            moved,
                            if moved == 1 { "" } else { "s" },
                            fallback
                        )
                    } else {
                        format!("Deleted bucket \"{}\" (no tasks affected)", removed_name)
                    };
                    app.status = Some((msg, Instant::now(), false));
                    ensure_default_selection(app);
                } else {
                    app.status = Some((
                        format!("Bucket \"{}\" not found", name),
                        Instant::now(),
                        false,
                    ));
                }
                app.input.clear();
                app.input_cursor = 0;
                return Ok(false);
            }

            // Reload tasks from disk before processing input to pick up external changes.
            if let Some(storage) = &app.storage {
                if let Ok(fresh) = storage.reload_tasks() {
                    app.tasks = fresh;
                }
            }

            // @ prefix: edit a specific task (by id) or the selected task via AI.
            if app.input.trim().starts_with('@') {
                let after_at = app
                    .input
                    .trim()
                    .strip_prefix('@')
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if !after_at.is_empty() {
                    let (target_task_id, instruction) =
                        resolve_at_mention(&app.tasks, &after_at, app.selected_task_id);

                    if !instruction.is_empty() {
                        // Detect decompose-intent instructions and route through triage.
                        let lower = instruction.to_ascii_lowercase();
                        let is_decompose = lower.contains("break down")
                            || lower.contains("decompose")
                            || lower.contains("sub-issue")
                            || lower.contains("subissue")
                            || lower.contains("sub-task")
                            || lower.contains("subtask")
                            || lower.contains("split into")
                            || lower.contains("break into");

                        if is_decompose {
                            if let Some(ai) = &app.ai {
                                let context = build_ai_context(&app.tasks);
                                let triage_ctx = build_triage_context(&app.tasks);
                                let triage_input =
                                    annotate_mention(&app.tasks, target_task_id, &instruction);
                                app.last_triage_input = triage_input.clone();
                                ai.enqueue(llm::AiJob {
                                    task_id: Uuid::nil(),
                                    title: String::new(),
                                    suggested_bucket: default_bucket_name(&app.settings),
                                    context,
                                    bucket_names: bucket_names(&app.settings),
                                    lock_bucket: false,
                                    lock_priority: false,
                                    lock_due_date: false,
                                    edit_instruction: None,
                                    task_snapshot: None,
                                    triage_input: Some(triage_input),
                                    triage_context: Some(triage_ctx),
                                    chat_history: app.chat_history.clone(),
                                });
                                app.status =
                                    Some(("AI decomposing…".to_string(), Instant::now(), true));
                            } else {
                                app.status =
                                    Some(("AI not configured".to_string(), Instant::now(), false));
                            }
                        } else if let Some(task_id) = target_task_id {
                            if let Some(task) = app.tasks.iter().find(|t| t.id == task_id) {
                                let snapshot = format_task_snapshot(task);
                                let context = build_ai_context(&app.tasks);
                                if let Some(ai) = &app.ai {
                                    ai.enqueue(llm::AiJob {
                                        task_id,
                                        title: task.title.clone(),
                                        suggested_bucket: task.bucket.clone(),
                                        context,
                                        bucket_names: bucket_names(&app.settings),
                                        lock_bucket: false,
                                        lock_priority: false,
                                        lock_due_date: false,
                                        edit_instruction: Some(instruction),
                                        task_snapshot: Some(snapshot),
                                        triage_input: None,
                                        triage_context: None,
                                        chat_history: Vec::new(),
                                    });
                                    app.status = Some((
                                        format!("AI editing: {}…", task.title),
                                        Instant::now(),
                                        true,
                                    ));
                                } else {
                                    app.status = Some((
                                        "AI not configured".to_string(),
                                        Instant::now(),
                                        false,
                                    ));
                                }
                            }
                        } else {
                            app.status =
                                Some(("No task selected".to_string(), Instant::now(), false));
                        }
                    }
                }
                app.input.clear();
                app.input_cursor = 0;
                return Ok(false);
            }

            let raw_input = app.input.trim().to_string();
            if raw_input.is_empty() {
                return Ok(false);
            }
            app.input.clear();
            app.input_cursor = 0;

            // AI triage: let the AI decide create vs update.
            if let Some(ai) = &app.ai {
                let context = build_ai_context(&app.tasks);
                let triage_ctx = build_triage_context(&app.tasks);
                app.last_triage_input = raw_input.clone();
                ai.enqueue(llm::AiJob {
                    task_id: Uuid::nil(),
                    title: String::new(),
                    suggested_bucket: default_bucket_name(&app.settings),
                    context,
                    bucket_names: bucket_names(&app.settings),
                    lock_bucket: false,
                    lock_priority: false,
                    lock_due_date: false,
                    edit_instruction: None,
                    task_snapshot: None,
                    triage_input: Some(raw_input),
                    triage_context: Some(triage_ctx),
                    chat_history: app.chat_history.clone(),
                });
                app.status = Some(("AI thinking…".to_string(), Instant::now(), true));
            } else {
                // Fallback: local inference when AI is not configured.
                let bnames = bucket_names(&app.settings);
                let maybe = ai::infer_new_task(&raw_input, &bnames);
                if let Some(hints) = maybe {
                    let now = Utc::now();
                    let mut task = Task::new(hints.bucket.clone(), hints.title, now);
                    if let Some(p) = hints.priority {
                        task.priority = p;
                    }
                    if let Some(d) = hints.due_date {
                        task.due_date = Some(d);
                    }
                    app.tasks.push(task);
                    app.status = Some((
                        format!("Created in {}", hints.bucket),
                        Instant::now(),
                        false,
                    ));
                    ensure_default_selection(app);
                    persist(app);
                }
            }
            Ok(false)
        }
        KeyCode::Backspace => {
            if key.modifiers.contains(KeyModifiers::ALT) {
                // Option+Backspace: delete word before cursor.
                while app.input_cursor > 0 {
                    let bp = char_byte_pos(&app.input, app.input_cursor - 1);
                    if !app.input[bp..].starts_with(' ') {
                        break;
                    }
                    app.input.remove(bp);
                    app.input_cursor -= 1;
                }
                while app.input_cursor > 0 {
                    let bp = char_byte_pos(&app.input, app.input_cursor - 1);
                    if app.input[bp..].starts_with(' ') {
                        break;
                    }
                    app.input.remove(bp);
                    app.input_cursor -= 1;
                }
            } else if app.input_cursor > 0 {
                let bp = char_byte_pos(&app.input, app.input_cursor - 1);
                app.input.remove(bp);
                app.input_cursor -= 1;
            }
            Ok(false)
        }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Ctrl+A: move cursor to start.
            app.input_cursor = 0;
            Ok(false)
        }
        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Ctrl+E: move cursor to end.
            app.input_cursor = app.input.chars().count();
            Ok(false)
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Ctrl+U: clear entire input.
            app.input.clear();
            app.input_cursor = 0;
            Ok(false)
        }
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Ctrl+W: delete word before cursor.
            while app.input_cursor > 0 {
                let bp = char_byte_pos(&app.input, app.input_cursor - 1);
                if !app.input[bp..].starts_with(' ') {
                    break;
                }
                app.input.remove(bp);
                app.input_cursor -= 1;
            }
            while app.input_cursor > 0 {
                let bp = char_byte_pos(&app.input, app.input_cursor - 1);
                if app.input[bp..].starts_with(' ') {
                    break;
                }
                app.input.remove(bp);
                app.input_cursor -= 1;
            }
            Ok(false)
        }
        KeyCode::Home => {
            app.input_cursor = 0;
            Ok(false)
        }
        KeyCode::End => {
            app.input_cursor = app.input.chars().count();
            Ok(false)
        }
        KeyCode::Char(ch) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                return Ok(false);
            }
            let bp = char_byte_pos(&app.input, app.input_cursor);
            app.input.insert(bp, ch);
            app.input_cursor += 1;
            Ok(false)
        }
        _ => Ok(false),
    }
}

fn handle_confirm_delete_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    match key.code {
        KeyCode::Enter => {
            if let Some(id) = app.confirm_delete_id.take() {
                app.selected_task_id = Some(id);
                delete_selected(app);
                // If still in the edit overlay, clamp sub-issue selection.
                if app.focus == Focus::Edit {
                    if let Some(parent_id) = app.edit_task_id {
                        let child_count = children_of(&app.tasks, parent_id).len();
                        app.edit_sub_selected =
                            app.edit_sub_selected.min(child_count.saturating_sub(1));
                    }
                }
            }
        }
        KeyCode::Esc => {
            app.confirm_delete_id = None;
        }
        _ => {}
    }
    Ok(false)
}

fn handle_board_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    // ── Bucket header selected ──
    if app.bucket_header_selected {
        match key.code {
            KeyCode::Esc => {
                app.bucket_header_selected = false;
                app.focus = Focus::Tabs;
            }
            KeyCode::Tab | KeyCode::Char('i') => {
                app.bucket_header_selected = false;
                app.focus = Focus::Input;
            }
            KeyCode::Left | KeyCode::Char('h') => {
                let n = app.settings.buckets.len();
                if n > 0 {
                    app.selected_bucket = (app.selected_bucket + n - 1) % n;
                }
            }
            KeyCode::Right | KeyCode::Char('l') => {
                let n = app.settings.buckets.len();
                if n > 0 {
                    app.selected_bucket = (app.selected_bucket + 1) % n;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.bucket_header_selected = false;
                ensure_default_selection(app);
            }
            KeyCode::Enter | KeyCode::Char('e') => {
                open_bucket_edit(app);
            }
            _ => {}
        }
        return Ok(false);
    }

    // ── Normal task-level board keys ──
    match key.code {
        KeyCode::Esc => {
            app.focus = Focus::Tabs;
            return Ok(false);
        }
        KeyCode::Tab | KeyCode::Char('i') => {
            app.focus = Focus::Input;
            return Ok(false);
        }
        KeyCode::Enter | KeyCode::Char('e') => {
            open_edit(app);
            return Ok(false);
        }
        KeyCode::Char('d') | KeyCode::Char('x') | KeyCode::Backspace | KeyCode::Delete => {
            if let Some(id) = app.selected_task_id {
                app.confirm_delete_id = Some(id);
            }
            return Ok(false);
        }
        _ => {}
    }

    match key.code {
        KeyCode::Left | KeyCode::Char('h') => {
            let n = app.settings.buckets.len();
            if n > 0 {
                app.selected_bucket = (app.selected_bucket + n - 1) % n;
            }
            ensure_default_selection(app);
        }
        KeyCode::Right | KeyCode::Char('l') => {
            let n = app.settings.buckets.len();
            if n > 0 {
                app.selected_bucket = (app.selected_bucket + 1) % n;
            }
            ensure_default_selection(app);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let bname = app
                .settings
                .buckets
                .get(app.selected_bucket)
                .map(|b| b.name.as_str())
                .unwrap_or("");
            let bucket_tasks = bucket_task_indices(&app.tasks, bname, &app.settings);
            let at_first = app
                .selected_task_id
                .and_then(|id| bucket_tasks.iter().position(|&idx| app.tasks[idx].id == id))
                .map(|pos| pos == 0)
                .unwrap_or(true);
            if at_first {
                app.bucket_header_selected = true;
                app.selected_task_id = None;
            } else {
                move_selection(app, -1);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let bname = app
                .settings
                .buckets
                .get(app.selected_bucket)
                .map(|b| b.name.as_str())
                .unwrap_or("");
            let bucket_tasks = bucket_task_indices(&app.tasks, bname, &app.settings);
            let at_last = app
                .selected_task_id
                .and_then(|id| bucket_tasks.iter().position(|&idx| app.tasks[idx].id == id))
                .map(|pos| pos >= bucket_tasks.len().saturating_sub(1))
                .unwrap_or(true);
            if at_last {
                app.focus = Focus::Input;
            } else {
                move_selection(app, 1);
            }
        }
        KeyCode::Char('p') => {
            if let Some(id) = app.selected_task_id {
                let now = Utc::now();
                if let Some(task) = app.tasks.iter_mut().find(|t| t.id == id) {
                    let from = task.progress;
                    task.advance_progress(now);
                    app.status = Some((
                        format!(
                            "{}: {} → {}",
                            task.title,
                            from.title(),
                            task.progress.title()
                        ),
                        Instant::now(),
                        false,
                    ));
                    persist(app);
                }
            }
        }
        KeyCode::Char('P') => {
            if let Some(id) = app.selected_task_id {
                let now = Utc::now();
                if let Some(task) = app.tasks.iter_mut().find(|t| t.id == id) {
                    let from = task.progress;
                    task.retreat_progress(now);
                    app.status = Some((
                        format!(
                            "{}: {} → {}",
                            task.title,
                            from.title(),
                            task.progress.title()
                        ),
                        Instant::now(),
                        false,
                    ));
                    persist(app);
                }
            }
        }
        _ => {}
    }

    Ok(false)
}

fn open_bucket_edit(app: &mut App) {
    if app.selected_bucket >= app.settings.buckets.len() {
        return;
    }
    let bucket = &app.settings.buckets[app.selected_bucket];
    app.bucket_edit_field = BucketEditField::Name;
    app.bucket_edit_buf = bucket.name.clone();
    app.bucket_editing_text = false;
    app.bucket_edit_active = true;
}

fn handle_bucket_edit_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    if app.bucket_editing_text {
        match key.code {
            KeyCode::Esc => {
                // Cancel text editing, reload original value.
                load_bucket_edit_buf(app);
                app.bucket_editing_text = false;
            }
            KeyCode::Enter => {
                commit_bucket_edit_buf(app);
                app.bucket_editing_text = false;
            }
            KeyCode::Backspace => {
                app.bucket_edit_buf.pop();
            }
            KeyCode::Char(ch) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL) {
                    app.bucket_edit_buf.push(ch);
                }
            }
            _ => {}
        }
        return Ok(false);
    }

    // Field navigation mode.
    match key.code {
        KeyCode::Esc => {
            app.bucket_edit_active = false;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.bucket_edit_field = match app.bucket_edit_field {
                BucketEditField::Name => BucketEditField::Description,
                BucketEditField::Description => BucketEditField::Name,
            };
            load_bucket_edit_buf(app);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.bucket_edit_field = match app.bucket_edit_field {
                BucketEditField::Name => BucketEditField::Description,
                BucketEditField::Description => BucketEditField::Name,
            };
            load_bucket_edit_buf(app);
        }
        KeyCode::Enter => {
            app.bucket_editing_text = true;
        }
        _ => {}
    }
    Ok(false)
}

fn load_bucket_edit_buf(app: &mut App) {
    if let Some(bucket) = app.settings.buckets.get(app.selected_bucket) {
        app.bucket_edit_buf = match app.bucket_edit_field {
            BucketEditField::Name => bucket.name.clone(),
            BucketEditField::Description => bucket.description.clone().unwrap_or_default(),
        };
    }
}

fn commit_bucket_edit_buf(app: &mut App) {
    let idx = app.selected_bucket;
    let Some(bucket) = app.settings.buckets.get_mut(idx) else {
        return;
    };
    match app.bucket_edit_field {
        BucketEditField::Name => {
            let new_name = app.bucket_edit_buf.trim().to_string();
            if !new_name.is_empty() && new_name != bucket.name {
                let old_name = bucket.name.clone();
                bucket.name = new_name.clone();
                // Update all tasks that reference the old bucket name.
                for task in &mut app.tasks {
                    if task.bucket == old_name {
                        task.bucket = new_name.clone();
                    }
                }
                persist(app);
            }
        }
        BucketEditField::Description => {
            let new_desc = app.bucket_edit_buf.trim().to_string();
            bucket.description = if new_desc.is_empty() {
                None
            } else {
                Some(new_desc)
            };
        }
    }
    persist_settings(app);
}

fn open_edit(app: &mut App) {
    let Some(id) = app.selected_task_id else {
        return;
    };
    open_edit_for(app, id);
}

fn open_edit_for(app: &mut App, task_id: Uuid) {
    let Some(task) = app.tasks.iter().find(|t| t.id == task_id) else {
        return;
    };
    app.edit_task_id = Some(task_id);
    app.edit_field = EditField::Title;
    app.edit_buf = task.title.clone();
    app.editing_text = false;
    app.edit_sub_selected = 0;
    app.edit_parent_stack.clear();
    app.focus = Focus::Edit;
}

fn close_edit(app: &mut App) {
    if let Some((parent_id, field, sub_sel)) = app.edit_parent_stack.pop() {
        if app.tasks.iter().any(|t| t.id == parent_id) {
            app.edit_task_id = Some(parent_id);
            app.edit_field = field;
            let child_count = children_of(&app.tasks, parent_id).len();
            app.edit_sub_selected = sub_sel.min(child_count.saturating_sub(1));
            app.editing_text = false;
            load_edit_buf(app);
            return;
        }
    }
    app.edit_task_id = None;
    app.editing_text = false;
    app.edit_parent_stack.clear();
    app.edit_sub_selected = 0;
    app.focus = Focus::Board;
}

fn delete_selected(app: &mut App) {
    let Some(id) = app.selected_task_id else {
        return;
    };
    if let Some(pos) = app.tasks.iter().position(|t| t.id == id) {
        let title = app.tasks[pos].title.clone();
        // Cascade: remove all children of this task.
        let child_ids: Vec<Uuid> = children_of(&app.tasks, id)
            .iter()
            .map(|&i| app.tasks[i].id)
            .collect();
        app.tasks
            .retain(|t| !child_ids.contains(&t.id) && t.id != id);
        // Clean up any dependency references to the deleted task(s).
        let all_deleted: Vec<Uuid> = std::iter::once(id).chain(child_ids).collect();
        for task in &mut app.tasks {
            task.dependencies.retain(|dep| !all_deleted.contains(dep));
        }
        app.status = Some((format!("Deleted: {title}"), Instant::now(), false));
        ensure_default_selection(app);
        persist(app);
    }
}

fn load_edit_buf(app: &mut App) {
    let Some(id) = app.edit_task_id else {
        return;
    };
    let Some(task) = app.tasks.iter().find(|t| t.id == id) else {
        return;
    };
    app.edit_buf = match app.edit_field {
        EditField::Title => task.title.clone(),
        EditField::Description => task.description.clone(),
        EditField::Bucket => task.bucket.clone(),
        EditField::Progress => task.progress.title().to_string(),
        EditField::Priority => task.priority.title().to_string(),
        EditField::DueDate => task
            .due_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default(),
        EditField::SubIssues => String::new(),
    };
}

fn commit_edit_buf(app: &mut App) {
    let Some(id) = app.edit_task_id else {
        return;
    };
    let Some(task) = app.tasks.iter_mut().find(|t| t.id == id) else {
        return;
    };
    let now = Utc::now();

    match app.edit_field {
        EditField::Title => {
            let trimmed = app.edit_buf.trim().to_string();
            if !trimmed.is_empty() {
                task.title = trimmed;
                task.updated_at = now;
            }
        }
        EditField::Description => {
            task.description = app.edit_buf.trim().to_string();
            task.updated_at = now;
        }
        EditField::Bucket => {
            let input = app.edit_buf.trim();
            let matched = app
                .settings
                .buckets
                .iter()
                .find(|b| b.name.eq_ignore_ascii_case(input));
            if let Some(b) = matched {
                task.bucket = b.name.clone();
                task.updated_at = now;
            }
        }
        EditField::Progress => {
            if let Some(p) = match app.edit_buf.trim().to_ascii_lowercase().as_str() {
                "backlog" => Some(Progress::Backlog),
                "todo" => Some(Progress::Todo),
                "in progress" | "inprogress" | "in-progress" => Some(Progress::InProgress),
                "done" => Some(Progress::Done),
                _ => None,
            } {
                task.set_progress(p, now);
            }
        }
        EditField::Priority => {
            if let Some(p) = match app.edit_buf.trim().to_ascii_lowercase().as_str() {
                "low" => Some(crate::model::Priority::Low),
                "med" | "medium" => Some(crate::model::Priority::Medium),
                "high" => Some(crate::model::Priority::High),
                "crit" | "critical" => Some(crate::model::Priority::Critical),
                _ => None,
            } {
                task.priority = p;
                task.updated_at = now;
            }
        }
        EditField::DueDate => {
            let s = app.edit_buf.trim();
            if s.is_empty() {
                task.due_date = None;
                task.updated_at = now;
            } else if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
                task.due_date = Some(date);
                task.updated_at = now;
            }
        }
        EditField::SubIssues => {}
    }

    persist(app);
}

fn cycle_edit_field_value(app: &mut App, forward: bool) {
    let Some(id) = app.edit_task_id else {
        return;
    };
    let Some(task) = app.tasks.iter_mut().find(|t| t.id == id) else {
        return;
    };
    let now = Utc::now();

    match app.edit_field {
        EditField::Bucket => {
            let names: Vec<String> = bucket_names(&app.settings);
            if !names.is_empty() {
                let cur = names.iter().position(|n| *n == task.bucket).unwrap_or(0);
                let next = if forward {
                    (cur + 1) % names.len()
                } else {
                    (cur + names.len() - 1) % names.len()
                };
                task.bucket = names[next].clone();
                task.updated_at = now;
            }
        }
        EditField::Progress => {
            let next = if forward {
                task.progress.advance()
            } else {
                task.progress.retreat()
            };
            task.set_progress(next, now);
        }
        EditField::Priority => {
            task.priority = if forward {
                match task.priority {
                    crate::model::Priority::Low => crate::model::Priority::Medium,
                    crate::model::Priority::Medium => crate::model::Priority::High,
                    crate::model::Priority::High => crate::model::Priority::Critical,
                    crate::model::Priority::Critical => crate::model::Priority::Low,
                }
            } else {
                match task.priority {
                    crate::model::Priority::Low => crate::model::Priority::Critical,
                    crate::model::Priority::Medium => crate::model::Priority::Low,
                    crate::model::Priority::High => crate::model::Priority::Medium,
                    crate::model::Priority::Critical => crate::model::Priority::High,
                }
            };
            task.updated_at = now;
        }
        _ => {}
    }

    persist(app);
    load_edit_buf(app);
}

fn handle_edit_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    if app.editing_text {
        match key.code {
            KeyCode::Esc => {
                app.editing_text = false;
                load_edit_buf(app);
            }
            KeyCode::Enter => {
                commit_edit_buf(app);
                app.editing_text = false;
                load_edit_buf(app);
            }
            KeyCode::Backspace => {
                if key.modifiers.contains(KeyModifiers::SUPER) {
                    // Cmd+Backspace: delete to start of line.
                    app.edit_buf.clear();
                } else if key.modifiers.contains(KeyModifiers::ALT) {
                    // Option+Backspace: delete word before cursor.
                    while app.edit_buf.ends_with(' ') {
                        app.edit_buf.pop();
                    }
                    while let Some(ch) = app.edit_buf.chars().last() {
                        if ch == ' ' {
                            break;
                        }
                        app.edit_buf.pop();
                    }
                } else {
                    app.edit_buf.pop();
                }
            }
            KeyCode::Char(ch) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL) {
                    app.edit_buf.push(ch);
                }
            }
            _ => {}
        }
        return Ok(false);
    }

    match key.code {
        KeyCode::Esc => {
            close_edit(app);
            ensure_default_selection(app);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.edit_field == EditField::SubIssues {
                if let Some(task_id) = app.edit_task_id {
                    let child_count = children_of(&app.tasks, task_id).len();
                    if child_count > 0 && app.edit_sub_selected > 0 {
                        app.edit_sub_selected -= 1;
                        return Ok(false);
                    }
                }
            }
            let idx = EditField::ALL
                .iter()
                .position(|f| *f == app.edit_field)
                .unwrap_or(0);
            let next = if idx == 0 {
                EditField::ALL.len() - 1
            } else {
                idx - 1
            };
            app.edit_field = EditField::ALL[next];
            if app.edit_field == EditField::SubIssues {
                if let Some(task_id) = app.edit_task_id {
                    let child_count = children_of(&app.tasks, task_id).len();
                    app.edit_sub_selected = child_count.saturating_sub(1);
                }
            }
            load_edit_buf(app);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.edit_field == EditField::SubIssues {
                if let Some(task_id) = app.edit_task_id {
                    let child_count = children_of(&app.tasks, task_id).len();
                    if child_count > 0 && app.edit_sub_selected < child_count - 1 {
                        app.edit_sub_selected += 1;
                        return Ok(false);
                    }
                }
            }
            let idx = EditField::ALL
                .iter()
                .position(|f| *f == app.edit_field)
                .unwrap_or(0);
            let next = (idx + 1) % EditField::ALL.len();
            app.edit_field = EditField::ALL[next];
            if app.edit_field == EditField::SubIssues {
                app.edit_sub_selected = 0;
            }
            load_edit_buf(app);
        }
        KeyCode::Enter | KeyCode::Char('e') => {
            if app.edit_field == EditField::SubIssues {
                if let Some(task_id) = app.edit_task_id {
                    let child_ids: Vec<Uuid> = children_of(&app.tasks, task_id)
                        .iter()
                        .map(|&i| app.tasks[i].id)
                        .collect();
                    if let Some(&child_id) = child_ids.get(app.edit_sub_selected) {
                        app.edit_parent_stack.push((
                            task_id,
                            app.edit_field,
                            app.edit_sub_selected,
                        ));
                        app.edit_task_id = Some(child_id);
                        app.edit_field = EditField::Title;
                        app.edit_sub_selected = 0;
                        load_edit_buf(app);
                    }
                }
            } else {
                match app.edit_field {
                    EditField::Title | EditField::Description | EditField::DueDate => {
                        load_edit_buf(app);
                        app.editing_text = true;
                    }
                    EditField::Bucket | EditField::Progress | EditField::Priority => {
                        cycle_edit_field_value(app, true);
                    }
                    EditField::SubIssues => {}
                }
            }
        }
        KeyCode::Left | KeyCode::Char('h') => match app.edit_field {
            EditField::Bucket | EditField::Progress | EditField::Priority => {
                cycle_edit_field_value(app, false);
            }
            _ => {}
        },
        KeyCode::Right | KeyCode::Char('l') => match app.edit_field {
            EditField::Bucket | EditField::Progress | EditField::Priority => {
                cycle_edit_field_value(app, true);
            }
            _ => {}
        },
        KeyCode::Char('a') => {
            if app.edit_field == EditField::SubIssues {
                if let Some(parent_id) = app.edit_task_id {
                    let parent_bucket = app
                        .tasks
                        .iter()
                        .find(|t| t.id == parent_id)
                        .map(|t| t.bucket.clone())
                        .unwrap_or_else(|| default_bucket_name(&app.settings));
                    let now = Utc::now();
                    let mut child = Task::new(parent_bucket, "New sub-issue".to_string(), now);
                    child.parent_id = Some(parent_id);
                    let child_id = child.id;
                    app.tasks.push(child);
                    persist(app);
                    let child_count = children_of(&app.tasks, parent_id).len();
                    let new_sub_idx = child_count.saturating_sub(1);
                    app.edit_parent_stack
                        .push((parent_id, EditField::SubIssues, new_sub_idx));
                    app.edit_task_id = Some(child_id);
                    app.edit_field = EditField::Title;
                    app.edit_buf = "New sub-issue".to_string();
                    app.editing_text = true;
                    app.edit_sub_selected = 0;
                }
            }
        }
        KeyCode::Char('d') | KeyCode::Char('x') | KeyCode::Backspace | KeyCode::Delete => {
            if app.edit_field == EditField::SubIssues {
                // Delete the selected sub-issue (via confirmation dialog).
                if let Some(parent_id) = app.edit_task_id {
                    let child_ids: Vec<Uuid> = children_of(&app.tasks, parent_id)
                        .iter()
                        .map(|&i| app.tasks[i].id)
                        .collect();
                    if let Some(&child_id) = child_ids.get(app.edit_sub_selected) {
                        app.confirm_delete_id = Some(child_id);
                    }
                }
            } else {
                let id = app.edit_task_id;
                close_edit(app);
                if let Some(task_id) = id {
                    app.selected_task_id = Some(task_id);
                    app.confirm_delete_id = Some(task_id);
                }
            }
        }
        _ => {}
    }

    Ok(false)
}

fn handle_kanban_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    match key.code {
        KeyCode::Esc => {
            app.focus = Focus::Tabs;
            return Ok(false);
        }
        KeyCode::Char('i') => {
            app.tab = Tab::Default;
            app.focus = Focus::Input;
            return Ok(false);
        }
        _ => {}
    }

    match key.code {
        KeyCode::Left | KeyCode::Char('h') => {
            app.kanban_stage = match app.kanban_stage {
                Progress::Backlog => Progress::Done,
                Progress::Todo => Progress::Backlog,
                Progress::InProgress => Progress::Todo,
                Progress::Done => Progress::InProgress,
            };
            ensure_kanban_selection(app);
        }
        KeyCode::Right | KeyCode::Char('l') => {
            app.kanban_stage = match app.kanban_stage {
                Progress::Backlog => Progress::Todo,
                Progress::Todo => Progress::InProgress,
                Progress::InProgress => Progress::Done,
                Progress::Done => Progress::Backlog,
            };
            ensure_kanban_selection(app);
        }
        KeyCode::Up | KeyCode::Char('k') => move_kanban_selection(app, -1),
        KeyCode::Down | KeyCode::Char('j') => move_kanban_selection(app, 1),
        KeyCode::Char('p') | KeyCode::Char(' ') => {
            if let Some(id) = app.kanban_selected {
                let now = Utc::now();
                if let Some(task) = app.tasks.iter_mut().find(|t| t.id == id) {
                    task.advance_progress(now);
                    persist(app);
                    ensure_kanban_selection(app);
                }
            }
        }
        KeyCode::Char('P') => {
            if let Some(id) = app.kanban_selected {
                let now = Utc::now();
                if let Some(task) = app.tasks.iter_mut().find(|t| t.id == id) {
                    task.retreat_progress(now);
                    persist(app);
                    ensure_kanban_selection(app);
                }
            }
        }
        KeyCode::Enter | KeyCode::Char('e') => {
            if let Some(id) = app.kanban_selected {
                app.selected_task_id = Some(id);
                open_edit_for(app, id);
            }
        }
        KeyCode::Char('d') | KeyCode::Char('x') | KeyCode::Backspace | KeyCode::Delete => {
            if let Some(id) = app.kanban_selected {
                app.confirm_delete_id = Some(id);
            }
        }
        _ => {}
    }

    Ok(false)
}

fn kanban_task_ids(tasks: &[Task], stage: Progress) -> Vec<Uuid> {
    let mut ids: Vec<(usize, Uuid)> = tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.progress == stage)
        .map(|(i, t)| (i, t.id))
        .collect();
    ids.sort_by(|a, b| tasks[b.0].created_at.cmp(&tasks[a.0].created_at));
    ids.into_iter().map(|(_, id)| id).collect()
}

fn ensure_kanban_selection(app: &mut App) {
    let ids = kanban_task_ids(&app.tasks, app.kanban_stage);
    if ids.is_empty() {
        app.kanban_selected = None;
        return;
    }
    if let Some(current) = app.kanban_selected {
        if ids.contains(&current) {
            return;
        }
    }
    app.kanban_selected = Some(ids[0]);
}

fn scroll_kanban_to_selected(app: &mut App) {
    let stage_idx = app.kanban_stage.stage_index();
    let ids = kanban_task_ids(&app.tasks, app.kanban_stage);
    let sel_pos = app
        .kanban_selected
        .and_then(|id| ids.iter().position(|i| *i == id))
        .unwrap_or(0);
    let scroll = &mut app.kanban_scroll[stage_idx];
    // Keep at least 1 row of context when possible.
    // We don't know list_slots here, so just ensure selected is >= scroll.
    if sel_pos < *scroll {
        *scroll = sel_pos;
    }
    // Upper bound clamping happens at render time when list_slots is known.
}

fn move_kanban_selection(app: &mut App, delta: i32) {
    let ids = kanban_task_ids(&app.tasks, app.kanban_stage);
    if ids.is_empty() {
        app.kanban_selected = None;
        return;
    }
    let current = app
        .kanban_selected
        .and_then(|id| ids.iter().position(|i| *i == id))
        .unwrap_or(0);
    let len = ids.len() as i32;
    let mut next = current as i32 + delta;
    if next < 0 {
        next = len - 1;
    } else if next >= len {
        next = 0;
    }
    app.kanban_selected = Some(ids[next as usize]);
    scroll_kanban_to_selected(app);
}

fn persist(app: &mut App) {
    let Some(storage) = &app.storage else {
        return;
    };
    if let Err(err) = storage.save_tasks(&app.tasks) {
        app.status = Some((format!("Save failed: {err}"), Instant::now(), false));
    }
}

fn persist_settings(app: &mut App) {
    let Some(storage) = &app.storage else {
        return;
    };
    if let Err(err) = storage.save_settings(&app.settings) {
        app.status = Some((
            format!("Settings save failed: {err}"),
            Instant::now(),
            false,
        ));
    }
}

fn rebuild_ai(app: &mut App) {
    app.ai = llm::AiRuntime::from_settings(&app.settings);
}

fn handle_settings_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    match key.code {
        KeyCode::Esc => {
            app.focus = Focus::Tabs;
            return Ok(false);
        }
        KeyCode::Char('i') => {
            app.tab = Tab::Default;
            app.focus = Focus::Input;
            return Ok(false);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let idx = SettingsField::ALL
                .iter()
                .position(|f| *f == app.settings_field)
                .unwrap_or(0);
            let next = if idx == 0 {
                SettingsField::ALL.len() - 1
            } else {
                idx - 1
            };
            app.settings_field = SettingsField::ALL[next];
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let idx = SettingsField::ALL
                .iter()
                .position(|f| *f == app.settings_field)
                .unwrap_or(0);
            let next = (idx + 1) % SettingsField::ALL.len();
            app.settings_field = SettingsField::ALL[next];
        }
        KeyCode::Enter | KeyCode::Char(' ') => match app.settings_field {
            SettingsField::OwnerName => {
                app.settings_buf = app.settings.owner_name.clone();
                app.settings_editing = true;
            }
            SettingsField::AiEnabled => {
                app.settings.enabled = !app.settings.enabled;
                persist_settings(app);
                rebuild_ai(app);
            }
            SettingsField::OpenAiKey => {
                app.settings_buf = app.settings.openai_api_key.clone();
                app.settings_editing = true;
            }
            SettingsField::AnthropicKey => {
                app.settings_buf = app.settings.anthropic_api_key.clone();
                app.settings_editing = true;
            }
            SettingsField::Model => {
                cycle_model(app, true);
            }
            SettingsField::ApiUrl => {
                app.settings_buf = app.settings.api_url.clone();
                app.settings_editing = true;
            }
            SettingsField::Timeout => {
                app.settings_buf = app.settings.timeout_secs.to_string();
                app.settings_editing = true;
            }
            SettingsField::ShowBacklog => {
                app.settings.show_backlog = !app.settings.show_backlog;
                persist_settings(app);
            }
            SettingsField::ShowTodo => {
                app.settings.show_todo = !app.settings.show_todo;
                persist_settings(app);
            }
            SettingsField::ShowInProgress => {
                app.settings.show_in_progress = !app.settings.show_in_progress;
                persist_settings(app);
            }
            SettingsField::ShowDone => {
                app.settings.show_done = !app.settings.show_done;
                persist_settings(app);
            }
        },
        KeyCode::Left | KeyCode::Right => match app.settings_field {
            SettingsField::AiEnabled => {
                app.settings.enabled = !app.settings.enabled;
                persist_settings(app);
                rebuild_ai(app);
            }
            SettingsField::Model => {
                cycle_model(app, key.code == KeyCode::Right);
            }
            SettingsField::ShowBacklog => {
                app.settings.show_backlog = !app.settings.show_backlog;
                persist_settings(app);
            }
            SettingsField::ShowTodo => {
                app.settings.show_todo = !app.settings.show_todo;
                persist_settings(app);
            }
            SettingsField::ShowInProgress => {
                app.settings.show_in_progress = !app.settings.show_in_progress;
                persist_settings(app);
            }
            SettingsField::ShowDone => {
                app.settings.show_done = !app.settings.show_done;
                persist_settings(app);
            }
            _ => {}
        },
        _ => {}
    }
    Ok(false)
}

fn cycle_model(app: &mut App, forward: bool) {
    let current_idx = MODEL_OPTIONS
        .iter()
        .position(|&m| m == app.settings.model)
        .unwrap_or(0);
    let next = if forward {
        (current_idx + 1) % MODEL_OPTIONS.len()
    } else if current_idx == 0 {
        MODEL_OPTIONS.len() - 1
    } else {
        current_idx - 1
    };
    app.settings.model = MODEL_OPTIONS[next].to_string();
    persist_settings(app);
    rebuild_ai(app);
}

fn handle_settings_edit_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    match key.code {
        KeyCode::Esc => {
            app.settings_editing = false;
        }
        KeyCode::Enter => {
            match app.settings_field {
                SettingsField::OwnerName => {
                    app.settings.owner_name = app.settings_buf.trim().to_string();
                }
                SettingsField::OpenAiKey => {
                    app.settings.openai_api_key = app.settings_buf.clone();
                }
                SettingsField::AnthropicKey => {
                    app.settings.anthropic_api_key = app.settings_buf.clone();
                }
                SettingsField::Model => app.settings.model = app.settings_buf.clone(),
                SettingsField::ApiUrl => app.settings.api_url = app.settings_buf.clone(),
                SettingsField::Timeout => {
                    if let Ok(secs) = app.settings_buf.parse::<u64>() {
                        app.settings.timeout_secs = secs;
                    }
                }
                _ => {}
            }
            persist_settings(app);
            rebuild_ai(app);
            // Stay in edit mode — user presses Esc to leave.
        }
        KeyCode::Backspace => {
            if key.modifiers.contains(KeyModifiers::SUPER) {
                // Cmd+Backspace: delete to start of line.
                app.settings_buf.clear();
            } else if key.modifiers.contains(KeyModifiers::ALT) {
                // Option+Backspace: delete word before cursor.
                while app.settings_buf.ends_with(' ') {
                    app.settings_buf.pop();
                }
                while let Some(ch) = app.settings_buf.chars().last() {
                    if ch == ' ' {
                        break;
                    }
                    app.settings_buf.pop();
                }
            } else {
                app.settings_buf.pop();
            }
        }
        KeyCode::Char(ch) => {
            app.settings_buf.push(ch);
        }
        _ => {}
    }
    Ok(false)
}

fn poll_ai(app: &mut App) -> bool {
    let results = match &app.ai {
        Some(ai) => ai.drain(),
        None => Vec::new(),
    };

    if results.is_empty() {
        return false;
    }

    let mut changed = false;
    for result in results {
        if let Some(err) = result.error {
            app.status = Some((format!("AI error: {}", err), Instant::now(), false));
            continue;
        }

        // Handle triage results: create new task or find & update existing.
        if let Some(triage_action) = &result.triage_action {
            match triage_action {
                llm::TriageAction::Create => {
                    let now = Utc::now();
                    let title = result
                        .update
                        .title
                        .as_deref()
                        .unwrap_or("Untitled")
                        .to_string();
                    let bucket = result
                        .update
                        .bucket
                        .clone()
                        .unwrap_or_else(|| default_bucket_name(&app.settings));
                    let mut task = Task::new(bucket.clone(), title, now);
                    if let Some(desc) = &result.update.description {
                        task.description = desc.clone();
                    }
                    if let Some(progress) = result.update.progress {
                        task.set_progress(progress, now);
                    }
                    if let Some(priority) = result.update.priority {
                        task.priority = priority;
                    }
                    if let Some(due_date) = result.update.due_date {
                        task.due_date = Some(due_date);
                    }
                    if !result.update.dependencies.is_empty() {
                        task.dependencies = resolve_dependency_prefixes(
                            &app.tasks,
                            task.id,
                            &result.update.dependencies,
                        );
                    }
                    let parent_id = task.id;
                    let status_title = task.title.clone();
                    app.tasks.push(task);
                    if !result.sub_task_specs.is_empty() {
                        let count = result.sub_task_specs.len();
                        let mut new_ids: Vec<Uuid> = Vec::with_capacity(count);
                        for spec in result.sub_task_specs.iter() {
                            let sub_bucket = spec.bucket.clone().unwrap_or_else(|| bucket.clone());
                            let mut sub = Task::new(sub_bucket, spec.title.clone(), now);
                            sub.parent_id = Some(parent_id);
                            sub.description = spec.description.clone();
                            if let Some(p) = spec.priority {
                                sub.priority = p;
                            }
                            if let Some(prog) = spec.progress {
                                sub.set_progress(prog, now);
                            }
                            if let Some(due) = spec.due_date {
                                sub.due_date = Some(due);
                            }
                            new_ids.push(sub.id);
                            app.tasks.push(sub);
                        }
                        for (i, spec) in result.sub_task_specs.iter().enumerate() {
                            if spec.depends_on.is_empty() {
                                continue;
                            }
                            let task_id = new_ids[i];
                            let dep_ids: Vec<Uuid> = spec
                                .depends_on
                                .iter()
                                .filter_map(|&idx| new_ids.get(idx).copied())
                                .filter(|&dep_id| dep_id != task_id)
                                .collect();
                            if let Some(t) = app.tasks.iter_mut().find(|t| t.id == task_id) {
                                t.dependencies = dep_ids;
                            }
                        }
                        app.status = Some((
                            format!(
                                "AI created: {} (+{} sub-task{})",
                                status_title,
                                count,
                                if count == 1 { "" } else { "s" }
                            ),
                            Instant::now(),
                            false,
                        ));
                    } else {
                        app.status = Some((
                            format!("AI created: {}", status_title),
                            Instant::now(),
                            false,
                        ));
                    }
                    changed = true;
                }
                llm::TriageAction::Update(prefix) => {
                    let target_id = app.tasks.iter().find_map(|t| {
                        let short = t.id.to_string().chars().take(8).collect::<String>();
                        if short.eq_ignore_ascii_case(prefix) {
                            Some(t.id)
                        } else {
                            None
                        }
                    });
                    if let Some(id) = target_id {
                        let deps = if !result.update.dependencies.is_empty() {
                            resolve_dependency_prefixes(&app.tasks, id, &result.update.dependencies)
                        } else {
                            Vec::new()
                        };
                        if let Some(task) = app.tasks.iter_mut().find(|t| t.id == id) {
                            let now = Utc::now();
                            apply_update(task, &result.update, &deps, now);
                            changed = true;
                        }
                        // Create sub-tasks if the update response includes them.
                        if !result.sub_task_specs.is_empty() {
                            let now = Utc::now();
                            let parent_bucket = app
                                .tasks
                                .iter()
                                .find(|t| t.id == id)
                                .map(|t| t.bucket.clone())
                                .unwrap_or_else(|| default_bucket_name(&app.settings));
                            let count = result.sub_task_specs.len();
                            let mut new_ids: Vec<Uuid> = Vec::with_capacity(count);
                            for spec in result.sub_task_specs.iter() {
                                let bucket =
                                    spec.bucket.clone().unwrap_or_else(|| parent_bucket.clone());
                                let mut task = Task::new(bucket, spec.title.clone(), now);
                                task.parent_id = Some(id);
                                task.description = spec.description.clone();
                                if let Some(p) = spec.priority {
                                    task.priority = p;
                                }
                                if let Some(prog) = spec.progress {
                                    task.set_progress(prog, now);
                                }
                                if let Some(due) = spec.due_date {
                                    task.due_date = Some(due);
                                }
                                new_ids.push(task.id);
                                app.tasks.push(task);
                            }
                            for (i, spec) in result.sub_task_specs.iter().enumerate() {
                                if spec.depends_on.is_empty() {
                                    continue;
                                }
                                let task_id = new_ids[i];
                                let dep_ids: Vec<Uuid> = spec
                                    .depends_on
                                    .iter()
                                    .filter_map(|&idx| new_ids.get(idx).copied())
                                    .filter(|&dep_id| dep_id != task_id)
                                    .collect();
                                if let Some(task) = app.tasks.iter_mut().find(|t| t.id == task_id) {
                                    task.dependencies = dep_ids;
                                }
                            }
                            let title = app
                                .tasks
                                .iter()
                                .find(|t| t.id == id)
                                .map(|t| t.title.clone())
                                .unwrap_or_default();
                            app.status = Some((
                                format!(
                                    "AI updated: {} (+{} sub-task{})",
                                    title,
                                    count,
                                    if count == 1 { "" } else { "s" }
                                ),
                                Instant::now(),
                                false,
                            ));
                        } else {
                            let title = app
                                .tasks
                                .iter()
                                .find(|t| t.id == id)
                                .map(|t| t.title.clone())
                                .unwrap_or_default();
                            app.status =
                                Some((format!("AI updated: {}", title), Instant::now(), false));
                        }
                    } else {
                        app.status = Some((
                            format!("AI: task {} not found", prefix),
                            Instant::now(),
                            false,
                        ));
                    }
                }
                llm::TriageAction::Delete(prefix) => {
                    let target = app.tasks.iter().position(|t| {
                        let short = t.id.to_string().chars().take(8).collect::<String>();
                        short.eq_ignore_ascii_case(prefix)
                    });
                    if let Some(pos) = target {
                        let title = app.tasks[pos].title.clone();
                        app.tasks.remove(pos);
                        app.status =
                            Some((format!("AI deleted: {}", title), Instant::now(), false));
                        changed = true;
                    } else {
                        app.status = Some((
                            format!("AI: task {} not found", prefix),
                            Instant::now(),
                            false,
                        ));
                    }
                }
                llm::TriageAction::Decompose { target_id, specs } => {
                    let now = Utc::now();
                    let count = specs.len();
                    // Resolve parent: prefer AI-specified target_id, fall back to selected.
                    let parent_id = target_id
                        .as_ref()
                        .and_then(|prefix| {
                            app.tasks.iter().find_map(|t| {
                                let short = t.id.to_string().chars().take(8).collect::<String>();
                                if short.eq_ignore_ascii_case(prefix) {
                                    Some(t.id)
                                } else {
                                    None
                                }
                            })
                        })
                        .or(app.selected_task_id);
                    // First pass: create all tasks and collect their Uuids.
                    let mut new_ids: Vec<Uuid> = Vec::with_capacity(count);
                    let default_bucket = specs
                        .first()
                        .and_then(|s| s.bucket.clone())
                        .unwrap_or_else(|| default_bucket_name(&app.settings));
                    for spec in specs.iter() {
                        let bucket = spec
                            .bucket
                            .clone()
                            .unwrap_or_else(|| default_bucket.clone());
                        let mut task = Task::new(bucket, spec.title.clone(), now);
                        task.parent_id = parent_id;
                        task.description = spec.description.clone();
                        if let Some(p) = spec.priority {
                            task.priority = p;
                        }
                        if let Some(prog) = spec.progress {
                            task.set_progress(prog, now);
                        }
                        if let Some(due) = spec.due_date {
                            task.due_date = Some(due);
                        }
                        new_ids.push(task.id);
                        app.tasks.push(task);
                    }
                    // Second pass: resolve depends_on indices to Uuid dependencies.
                    for (i, spec) in specs.iter().enumerate() {
                        if spec.depends_on.is_empty() {
                            continue;
                        }
                        let task_id = new_ids[i];
                        let deps: Vec<Uuid> = spec
                            .depends_on
                            .iter()
                            .filter_map(|&idx| new_ids.get(idx).copied())
                            .filter(|&dep_id| dep_id != task_id)
                            .collect();
                        if let Some(task) = app.tasks.iter_mut().find(|t| t.id == task_id) {
                            task.dependencies = deps;
                        }
                    }
                    app.status = Some((
                        format!(
                            "AI created {} sub-task{}",
                            count,
                            if count == 1 { "" } else { "s" }
                        ),
                        Instant::now(),
                        false,
                    ));
                    changed = true;
                }
                llm::TriageAction::BulkUpdate {
                    targets,
                    instruction,
                } => {
                    // Resolve target IDs: "all" means every task.
                    let task_ids: Vec<Uuid> =
                        if targets.len() == 1 && targets[0].eq_ignore_ascii_case("all") {
                            app.tasks.iter().map(|t| t.id).collect()
                        } else {
                            targets
                                .iter()
                                .filter_map(|prefix| {
                                    app.tasks.iter().find_map(|t| {
                                        let short =
                                            t.id.to_string().chars().take(8).collect::<String>();
                                        if short.eq_ignore_ascii_case(prefix) {
                                            Some(t.id)
                                        } else {
                                            None
                                        }
                                    })
                                })
                                .collect()
                        };

                    if task_ids.is_empty() {
                        app.status = Some((
                            "AI: no matching tasks found".to_string(),
                            Instant::now(),
                            false,
                        ));
                    } else if let Some(ai) = &app.ai {
                        let context = build_ai_context(&app.tasks);
                        for &tid in &task_ids {
                            if let Some(task) = app.tasks.iter().find(|t| t.id == tid) {
                                let snapshot = format_task_snapshot(task);
                                ai.enqueue(llm::AiJob {
                                    task_id: tid,
                                    title: task.title.clone(),
                                    suggested_bucket: task.bucket.clone(),
                                    context: context.clone(),
                                    bucket_names: bucket_names(&app.settings),
                                    lock_bucket: false,
                                    lock_priority: false,
                                    lock_due_date: false,
                                    edit_instruction: Some(instruction.clone()),
                                    task_snapshot: Some(snapshot),
                                    triage_input: None,
                                    triage_context: None,
                                    chat_history: Vec::new(),
                                });
                            }
                        }
                        app.status = Some((
                            format!(
                                "AI updating {} task{}…",
                                task_ids.len(),
                                if task_ids.len() == 1 { "" } else { "s" }
                            ),
                            Instant::now(),
                            true,
                        ));
                    }
                }
            }
            // Update chat history after triage.
            if !app.last_triage_input.is_empty() {
                let summary = app
                    .status
                    .as_ref()
                    .map(|(s, _, _)| s.clone())
                    .unwrap_or_default();
                app.chat_history.push(llm::ChatEntry {
                    user_input: std::mem::take(&mut app.last_triage_input),
                    ai_summary: summary,
                });
                if app.chat_history.len() > 20 {
                    app.chat_history.drain(..app.chat_history.len() - 20);
                }
            }
            if changed {
                ensure_default_selection(app);
                persist(app);
            }
            continue;
        }

        // Non-triage results: enrichment or @ edit.
        let deps = if !result.update.dependencies.is_empty() {
            resolve_dependency_prefixes(&app.tasks, result.task_id, &result.update.dependencies)
        } else {
            Vec::new()
        };

        let parent_id = result.task_id;

        if let Some(task) = app.tasks.iter_mut().find(|t| t.id == parent_id) {
            let now = Utc::now();
            if apply_update(task, &result.update, &deps, now) {
                app.status = Some((format!("AI updated: {}", task.title), Instant::now(), false));
                changed = true;
            }
        }

        // Create actual sub-task records when the edit response includes subtasks.
        if !result.sub_task_specs.is_empty() {
            let now = Utc::now();
            let parent_bucket = app
                .tasks
                .iter()
                .find(|t| t.id == parent_id)
                .map(|t| t.bucket.clone())
                .unwrap_or_else(|| default_bucket_name(&app.settings));
            let count = result.sub_task_specs.len();
            let mut new_ids: Vec<Uuid> = Vec::with_capacity(count);

            for spec in result.sub_task_specs.iter() {
                let bucket = spec.bucket.clone().unwrap_or_else(|| parent_bucket.clone());
                let mut task = Task::new(bucket, spec.title.clone(), now);
                task.parent_id = Some(parent_id);
                task.description = spec.description.clone();
                if let Some(p) = spec.priority {
                    task.priority = p;
                }
                if let Some(prog) = spec.progress {
                    task.set_progress(prog, now);
                }
                if let Some(due) = spec.due_date {
                    task.due_date = Some(due);
                }
                new_ids.push(task.id);
                app.tasks.push(task);
            }

            // Second pass: resolve depends_on indices to Uuid dependencies.
            for (i, spec) in result.sub_task_specs.iter().enumerate() {
                if spec.depends_on.is_empty() {
                    continue;
                }
                let task_id = new_ids[i];
                let dep_ids: Vec<Uuid> = spec
                    .depends_on
                    .iter()
                    .filter_map(|&idx| new_ids.get(idx).copied())
                    .filter(|&dep_id| dep_id != task_id)
                    .collect();
                if let Some(task) = app.tasks.iter_mut().find(|t| t.id == task_id) {
                    task.dependencies = dep_ids;
                }
            }

            app.status = Some((
                format!(
                    "AI created {} sub-task{}",
                    count,
                    if count == 1 { "" } else { "s" }
                ),
                Instant::now(),
                false,
            ));
            changed = true;
        }
    }

    if changed {
        ensure_default_selection(app);
        persist(app);
    }

    true
}

/// Apply a TaskUpdate to a task, returning true if anything changed.
fn apply_update(
    task: &mut Task,
    update: &llm::TaskUpdate,
    deps: &[Uuid],
    now: chrono::DateTime<Utc>,
) -> bool {
    let mut task_changed = false;
    let is_edit = update.is_edit;

    if let Some(new_title) = &update.title {
        let trimmed = new_title.trim();
        if !trimmed.is_empty() && task.title != trimmed {
            task.title = trimmed.to_string();
            task_changed = true;
        }
    }

    if let Some(bucket) = &update.bucket {
        if task.bucket != *bucket {
            task.bucket = bucket.clone();
            task_changed = true;
        }
    }

    if let Some(desc) = &update.description {
        if is_edit || task.description.trim().is_empty() {
            task.description = desc.clone();
            task_changed = true;
        }
    }

    if let Some(progress) = update.progress {
        if task.progress != progress {
            task.set_progress(progress, now);
            task_changed = true;
        }
    }

    if let Some(priority) = update.priority {
        if task.priority != priority {
            task.priority = priority;
            task_changed = true;
        }
    }

    if let Some(due_date) = update.due_date {
        if task.due_date != Some(due_date) {
            task.due_date = Some(due_date);
            task_changed = true;
        }
    }

    if !deps.is_empty() && task.dependencies != deps {
        task.dependencies = deps.to_vec();
        task_changed = true;
    }

    if task_changed {
        task.updated_at = now;
    }

    task_changed
}

fn build_ai_context(tasks: &[Task]) -> Vec<llm::ContextTask> {
    let mut refs: Vec<&Task> = tasks.iter().collect();
    refs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    refs.into_iter()
        .take(40)
        .map(|t| llm::ContextTask {
            id: t.id,
            bucket: t.bucket.clone(),
            title: t.title.clone(),
        })
        .collect()
}

/// Build rich context for triage: full task details so the AI can match intent.
/// Shows parent tasks with their sub-tasks indented to expose the full hierarchy.
fn build_triage_context(tasks: &[Task]) -> String {
    // Collect parent (root) tasks sorted by recency.
    let mut parents: Vec<&Task> = tasks.iter().filter(|t| t.parent_id.is_none()).collect();
    parents.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    let mut out = String::new();
    let mut count = 0usize;
    let limit = 60;

    for t in &parents {
        if count >= limit {
            break;
        }
        let short = t.id.to_string().chars().take(8).collect::<String>();
        let desc = if t.description.trim().is_empty() {
            ""
        } else {
            t.description.trim()
        };
        out.push_str(&format!(
            "- {} [{}] {} | {} | {} | {}\n",
            short,
            t.bucket,
            t.title,
            t.progress.title(),
            t.priority.title(),
            if desc.is_empty() {
                "no description"
            } else {
                desc
            }
        ));
        count += 1;

        // Show children indented under their parent.
        let children = children_of(tasks, t.id);
        for &idx in &children {
            if count >= limit {
                break;
            }
            let child = &tasks[idx];
            let child_short = child.id.to_string().chars().take(8).collect::<String>();
            let child_desc = if child.description.trim().is_empty() {
                ""
            } else {
                child.description.trim()
            };
            out.push_str(&format!(
                "  ↳ {} [{}] {} | {} | {} | {}\n",
                child_short,
                child.bucket,
                child.title,
                child.progress.title(),
                child.priority.title(),
                if child_desc.is_empty() {
                    "no description"
                } else {
                    child_desc
                }
            ));
            count += 1;
        }
    }
    out
}

fn format_task_snapshot(task: &Task) -> String {
    let deps = if task.dependencies.is_empty() {
        "none".to_string()
    } else {
        task.dependencies
            .iter()
            .map(|id| id.to_string()[..8].to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let due = task
        .due_date
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "none".to_string());
    format!(
        "Title: {}\nBucket: {}\nDescription: {}\nProgress: {}\nPriority: {}\nDue: {}\nDependencies: {}",
        task.title,
        task.bucket,
        if task.description.trim().is_empty() { "none" } else { task.description.trim() },
        task.progress.title(),
        task.priority.title(),
        due,
        deps
    )
}

/// Parse `@<id_prefix> <instruction>` – if the first token is a 4-8 hex prefix matching a task,
/// return that task's id + the remaining text; otherwise fall back to `fallback_id` + full text.
fn resolve_at_mention(
    tasks: &[Task],
    text: &str,
    fallback_id: Option<Uuid>,
) -> (Option<Uuid>, String) {
    let trimmed = text.trim();
    let first_space = trimmed.find(' ');
    if let Some(pos) = first_space {
        let token = &trimmed[..pos];
        let rest = trimmed[pos..].trim().to_string();
        let lower = token.to_ascii_lowercase();
        if (4..=8).contains(&lower.len()) && lower.chars().all(|c| c.is_ascii_hexdigit()) {
            if let Some(task) = tasks.iter().find(|t| {
                let short =
                    t.id.to_string()
                        .chars()
                        .take(lower.len())
                        .collect::<String>();
                short.to_ascii_lowercase() == lower
            }) {
                return (Some(task.id), rest);
            }
        }
    }
    (fallback_id, trimmed.to_string())
}

/// Annotate an instruction with the target task context so triage AI knows which task to act on.
fn annotate_mention(tasks: &[Task], target_id: Option<Uuid>, instruction: &str) -> String {
    if let Some(tid) = target_id {
        if let Some(task) = tasks.iter().find(|t| t.id == tid) {
            let short = task.id.to_string().chars().take(8).collect::<String>();
            return format!(
                "[target task: {} \"{}\" in {}] {}",
                short, task.title, task.bucket, instruction
            );
        }
    }
    instruction.to_string()
}

fn resolve_dependency_prefixes(tasks: &[Task], self_id: Uuid, prefixes: &[String]) -> Vec<Uuid> {
    let mut out = Vec::new();
    for prefix in prefixes.iter() {
        let key = prefix
            .trim()
            .chars()
            .take(8)
            .collect::<String>()
            .to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }

        if let Some(id) = tasks.iter().find_map(|t| {
            let short = t.id.to_string().chars().take(8).collect::<String>();
            if short.to_ascii_lowercase() == key {
                Some(t.id)
            } else {
                None
            }
        }) {
            if id != self_id && !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

fn ensure_default_selection(app: &mut App) {
    let bucket_name = app
        .settings
        .buckets
        .get(app.selected_bucket)
        .map(|b| b.name.as_str())
        .unwrap_or("");
    let bucket_tasks = bucket_task_indices(&app.tasks, bucket_name, &app.settings);
    if bucket_tasks.is_empty() {
        app.selected_task_id = None;
        return;
    }

    let still_valid = app.selected_task_id.and_then(|id| {
        bucket_tasks
            .iter()
            .find(|&&idx| app.tasks[idx].id == id)
            .map(|_| id)
    });

    if still_valid.is_none() {
        app.selected_task_id = Some(app.tasks[bucket_tasks[0]].id);
    }

    clamp_bucket_scroll(app, bucket_tasks.len());
}

fn move_selection(app: &mut App, delta: i32) {
    let bucket_name = app
        .settings
        .buckets
        .get(app.selected_bucket)
        .map(|b| b.name.as_str())
        .unwrap_or("");
    let bucket_tasks = bucket_task_indices(&app.tasks, bucket_name, &app.settings);
    if bucket_tasks.is_empty() {
        app.selected_task_id = None;
        return;
    }

    let current_index = app
        .selected_task_id
        .and_then(|id| bucket_tasks.iter().position(|&idx| app.tasks[idx].id == id))
        .unwrap_or(0);

    let len = bucket_tasks.len() as i32;
    let mut next = current_index as i32 + delta;
    if next < 0 {
        next = len - 1;
    } else if next >= len {
        next = 0;
    }

    let next_idx = bucket_tasks[next as usize];
    app.selected_task_id = Some(app.tasks[next_idx].id);

    clamp_bucket_scroll(app, bucket_tasks.len());
}

fn clamp_bucket_scroll(app: &mut App, total: usize) {
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let _ = cols;

    // Keep in sync with `render_default_tab`.
    let y_body_top = 2u16;
    let y_input = rows.saturating_sub(2);
    // Card list starts after title + description rows.
    let y_cards_start = y_body_top + 3;
    let cards_area_height = y_input.saturating_sub(y_cards_start) as usize;

    let visible = visible_cards(cards_area_height);
    let visible = visible.max(1);

    let bucket_name = app
        .settings
        .buckets
        .get(app.selected_bucket)
        .map(|b| b.name.as_str())
        .unwrap_or("");
    let selected_index = app
        .selected_task_id
        .and_then(|id| {
            bucket_task_indices(&app.tasks, bucket_name, &app.settings)
                .iter()
                .position(|&idx| app.tasks[idx].id == id)
        })
        .unwrap_or(0);

    if app.selected_bucket >= app.bucket_scrolls.len() {
        return;
    }
    let scroll = &mut app.bucket_scrolls[app.selected_bucket];

    if total <= visible {
        *scroll = 0;
        return;
    }

    if selected_index < *scroll {
        *scroll = selected_index;
    } else if selected_index >= *scroll + visible {
        *scroll = selected_index.saturating_sub(visible.saturating_sub(1));
    }

    let max_scroll = total.saturating_sub(visible);
    *scroll = (*scroll).min(max_scroll);
}

fn bucket_task_indices(tasks: &[Task], bucket_name: &str, settings: &AiSettings) -> Vec<usize> {
    let mut indices: Vec<usize> = tasks
        .iter()
        .enumerate()
        .filter_map(|(idx, t)| {
            if t.bucket == bucket_name
                && t.parent_id.is_none()
                && settings.is_progress_visible(t.progress)
            {
                Some(idx)
            } else {
                None
            }
        })
        .collect();

    indices.sort_by(|&a, &b| {
        let ta = &tasks[a];
        let tb = &tasks[b];
        tb.progress
            .stage_index()
            .cmp(&ta.progress.stage_index())
            .then_with(|| tb.priority.cmp(&ta.priority))
            .then_with(|| tb.created_at.cmp(&ta.created_at))
    });

    indices
}

fn visible_cards(cards_area_height: usize) -> usize {
    const CARD_HEIGHT: usize = 8; // 7 lines + 1 spacer
    cards_area_height / CARD_HEIGHT
}

fn render(stdout: &mut Stdout, app: &mut App, clear: bool) -> io::Result<()> {
    let (cols, rows) = terminal::size()?;
    if clear {
        queue!(stdout, Clear(ClearType::All))?;
    }
    queue!(stdout, MoveTo(0, 0))?;

    if cols < 60 || rows < 12 {
        queue!(
            stdout,
            MoveTo(2, 1),
            SetForegroundColor(Color::DarkGrey),
            Print("Terminal too small (need ~60x12)."),
            ResetColor
        )?;
        stdout.flush()?;
        return Ok(());
    }

    render_tabs(stdout, app, cols)?;

    match app.tab {
        Tab::Default => render_default_tab(stdout, app, cols, rows)?,
        Tab::Timeline => render_timeline_tab(stdout, app, cols, rows)?,
        Tab::Kanban => render_kanban_tab(stdout, app, cols, rows)?,
        Tab::Settings => render_settings_tab(stdout, app, cols, rows)?,
    }

    if app.bucket_edit_active {
        render_bucket_edit_overlay(stdout, app, cols, rows)?;
    }

    if app.focus == Focus::Edit {
        render_edit_overlay(stdout, app, cols, rows)?;
    }

    if app.confirm_delete_id.is_some() {
        render_delete_confirm(stdout, app, cols, rows)?;
    }

    if app.status.is_some() {
        render_toast(stdout, app, cols, rows)?;
    }

    stdout.flush()?;
    Ok(())
}

fn render_tabs(stdout: &mut Stdout, app: &App, cols: u16) -> io::Result<()> {
    let width = cols as usize;
    let num_buckets = app.settings.buckets.len().max(1);
    let (x_margin, _) = choose_layout(width, num_buckets);
    let mut x: u16 = x_margin as u16;
    let tabs_focused = app.focus == Focus::Tabs;
    for (tab, label) in [
        (Tab::Default, "1 Buckets"),
        (Tab::Timeline, "2 Timeline"),
        (Tab::Kanban, "3 Kanban"),
        (Tab::Settings, "4 Settings"),
    ]
    .iter()
    {
        let is_active = *tab == app.tab;
        let rendered = format!(" {} ", label);
        queue!(stdout, MoveTo(x, 1))?;
        if is_active && tabs_focused {
            // Inverted highlight when tab bar is focused.
            queue!(
                stdout,
                SetForegroundColor(Color::Black),
                SetBackgroundColor(Color::White),
                SetAttribute(Attribute::Bold),
                Print(&rendered),
                SetAttribute(Attribute::Reset),
                ResetColor
            )?;
        } else if is_active {
            // Subtle indicator when inside tab content.
            queue!(
                stdout,
                SetAttribute(Attribute::Bold),
                Print(&rendered),
                SetAttribute(Attribute::Reset)
            )?;
        } else {
            queue!(
                stdout,
                SetForegroundColor(Color::DarkGrey),
                Print(&rendered),
                ResetColor
            )?;
        }
        x += rendered.width() as u16 + 2;
    }
    Ok(())
}

fn render_default_tab(stdout: &mut Stdout, app: &mut App, cols: u16, rows: u16) -> io::Result<()> {
    let width = cols as usize;
    let num_buckets = app.settings.buckets.len().max(1);
    let (x_margin, gap) = choose_layout(width, num_buckets);

    // Top padding below the tabs row.
    let y_body_top = 3u16;
    let y_status = rows.saturating_sub(5);
    let y_sep_top = rows.saturating_sub(4);
    let y_input = rows.saturating_sub(3);
    let y_sep_bottom = rows.saturating_sub(2);
    let y_help = rows.saturating_sub(1);

    let x_input = x_margin as u16;

    let content_width = width.saturating_sub(x_margin * 2);
    let col_width = if num_buckets > 1 {
        content_width.saturating_sub(gap * (num_buckets - 1)) / num_buckets
    } else {
        content_width
    };

    let col_x: Vec<usize> = (0..num_buckets)
        .map(|i| x_margin + i * (col_width + gap))
        .collect();

    for (i, bucket_def) in app.settings.buckets.iter().enumerate() {
        let x = col_x[i] as u16;
        let is_header_selected =
            app.focus == Focus::Board && app.bucket_header_selected && i == app.selected_bucket;

        let title = format!(" {}", bucket_def.name);
        let desc = format!(" {}", bucket_def.description.as_deref().unwrap_or(""));

        if is_header_selected {
            queue!(
                stdout,
                MoveTo(x, y_body_top),
                SetForegroundColor(Color::Black),
                SetBackgroundColor(Color::White),
                SetAttribute(Attribute::Bold),
                Print(pad_to_width(&clamp_text(&title, col_width), col_width)),
                SetAttribute(Attribute::Reset),
                ResetColor
            )?;
            queue!(
                stdout,
                MoveTo(x, y_body_top + 1),
                SetForegroundColor(Color::Black),
                SetBackgroundColor(Color::White),
                Print(pad_to_width(&clamp_text(&desc, col_width), col_width)),
                ResetColor
            )?;
        } else {
            queue!(
                stdout,
                MoveTo(x, y_body_top),
                SetAttribute(Attribute::Bold),
                Print(clamp_text(&title, col_width)),
                SetAttribute(Attribute::Reset)
            )?;
            queue!(
                stdout,
                MoveTo(x, y_body_top + 1),
                SetForegroundColor(Color::DarkGrey),
                Print(clamp_text(&desc, col_width)),
                ResetColor
            )?;
        }
    }

    let y_cards_start = y_body_top + 3;

    for (i, &cx) in col_x.iter().enumerate().take(app.settings.buckets.len()) {
        render_bucket_column(
            stdout,
            app,
            i,
            cx as u16,
            y_cards_start,
            col_width,
            y_status,
        )?;
    }

    // Separator above input.
    let sep = "─".repeat(content_width);
    queue!(
        stdout,
        MoveTo(x_input, y_sep_top),
        SetForegroundColor(Color::DarkGrey),
        Print(&sep),
        ResetColor
    )?;

    // Input
    let prompt = "› ";
    let max_input = width
        .saturating_sub(x_margin * 2)
        .saturating_sub(prompt.width());
    let (shown, cursor_vis_offset) = if app.input.is_empty() {
        (String::new(), 0)
    } else {
        input_visible_window(&app.input, app.input_cursor, max_input)
    };

    queue!(stdout, MoveTo(x_input, y_input))?;
    match app.focus {
        Focus::Input => queue!(stdout, ResetColor)?,
        Focus::Tabs | Focus::Board | Focus::Edit => {
            queue!(stdout, SetForegroundColor(Color::DarkGrey))?
        }
    };
    queue!(stdout, Print(prompt))?;

    if shown.is_empty() {
        queue!(
            stdout,
            SetForegroundColor(Color::DarkGrey),
            Print(pad_to_width(
                &clamp_text(
                    "type a task • @<id> edit • /clear resets AI context",
                    max_input
                ),
                max_input,
            )),
            ResetColor
        )?;
    } else {
        queue!(stdout, Print(pad_to_width(&shown, max_input)), ResetColor)?;
    }

    // Separator below input.
    queue!(
        stdout,
        MoveTo(x_input, y_sep_bottom),
        SetForegroundColor(Color::DarkGrey),
        Print(&sep),
        ResetColor
    )?;

    // Help
    queue!(
        stdout,
        MoveTo(x_input, y_help),
        SetForegroundColor(Color::DarkGrey),
        Print(clamp_text(
            "tab/i input • esc board • ↑/↓/←/→ nav • p advance • @id edit • /clear • 1/2/3 tabs",
            content_width,
        )),
        ResetColor
    )?;

    // @ autocomplete dropdown.
    if app.focus == Focus::Input {
        let completions = at_completions(&app.tasks, &app.input);
        if !completions.is_empty() {
            const MAX_SHOW: usize = 8;
            let max_dropdown = (y_sep_top as usize).saturating_sub(y_body_top as usize + 3);
            let show = completions.len().min(MAX_SHOW).min(max_dropdown);
            if show > 0 {
                let sel = app
                    .at_autocomplete_selected
                    .min(completions.len().saturating_sub(1));
                // Scroll window to keep selected visible.
                let scroll = if sel >= show { sel - show + 1 } else { 0 };
                for (draw_i, (short_id, title, bucket)) in
                    completions.iter().enumerate().skip(scroll).take(show)
                {
                    let row_from_bottom = show - (draw_i - scroll) - 1;
                    let y_row = y_sep_top - 1 - row_from_bottom as u16;
                    let label = format!(" {} {} [{}]", short_id, title, bucket);
                    let padded = pad_to_width(&clamp_text(&label, content_width), content_width);
                    queue!(stdout, MoveTo(x_input, y_row))?;
                    if draw_i == sel {
                        queue!(
                            stdout,
                            SetForegroundColor(Color::Black),
                            SetBackgroundColor(Color::White),
                            Print(&padded),
                            ResetColor
                        )?;
                    } else {
                        queue!(
                            stdout,
                            SetForegroundColor(Color::White),
                            SetBackgroundColor(Color::DarkGrey),
                            Print(&padded),
                            ResetColor
                        )?;
                    }
                }
            }
        }
    }

    // Cursor
    if app.focus == Focus::Input {
        let cursor_x = x_input as usize + prompt.width() + cursor_vis_offset;
        queue!(
            stdout,
            MoveTo((cursor_x as u16).min(cols.saturating_sub(1)), y_input),
            Show
        )?;
    } else {
        queue!(stdout, Hide)?;
    }

    Ok(())
}

fn render_bucket_column(
    stdout: &mut Stdout,
    app: &App,
    bucket_idx: usize,
    x: u16,
    y: u16,
    width: usize,
    max_y: u16,
) -> io::Result<()> {
    const CARD_LINES: usize = 6; // lines 0-5 (title, desc×2, separator, progress, due)

    let bucket_name = &app.settings.buckets[bucket_idx].name;
    let indices = bucket_task_indices(&app.tasks, bucket_name, &app.settings);
    let scroll = app.bucket_scrolls.get(bucket_idx).copied().unwrap_or(0);

    let inner_w = width.saturating_sub(2); // 1 char padding each side
    let mut y_cursor = y;

    for (_pos, &idx) in indices.iter().enumerate().skip(scroll) {
        if y_cursor + CARD_LINES as u16 + 1 > max_y {
            break;
        }

        let task = &app.tasks[idx];
        let is_selected = app.focus == Focus::Board
            && bucket_idx == app.selected_bucket
            && app.selected_task_id == Some(task.id);

        let card_top = y_cursor;

        // Word-wrap description into max 2 lines.
        let desc_text = if task.description.trim().is_empty() {
            "—".to_string()
        } else {
            task.description.trim().to_string()
        };
        let desc_lines = wrap_text(&desc_text, inner_w, 2);

        // Build the field table rows.
        let gauge = progress_gauge(task.progress);
        let due = task
            .due_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "—".to_string());
        // Sub-issue info.
        let child_indices = children_of(&app.tasks, task.id);
        let has_children = !child_indices.is_empty();
        let sub_info = if has_children {
            let done_count = child_indices
                .iter()
                .filter(|&&i| app.tasks[i].progress == Progress::Done)
                .count();
            format!("▸ {}/{} sub-issues", done_count, child_indices.len())
        } else if task.dependencies.is_empty() {
            "→ —".to_string()
        } else {
            format!(
                "→ {}",
                task.dependencies
                    .iter()
                    .take(3)
                    .map(|id| id.to_string()[..8].to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };

        // Table row 1: progress │ priority
        let table_row1 = format!(
            "{} {} │ {}",
            gauge,
            task.progress.title(),
            task.priority.title()
        );
        // Table row 2: due │ sub-issues/deps
        let table_row2 = format!("Due {} │ {}", due, sub_info);

        // Assemble card lines:
        // 0: title (bold)
        // 1: desc line 1 (grey)
        // 2: desc line 2 (grey)
        // 3: separator
        // 4: progress │ priority (colored gauge)
        // 5: due │ deps/sub-count (dim)
        for line_idx in 0..CARD_LINES {
            let y_line = card_top + line_idx as u16;
            queue!(stdout, MoveTo(x, y_line))?;

            if is_selected {
                queue!(
                    stdout,
                    SetForegroundColor(Color::Black),
                    SetBackgroundColor(Color::White)
                )?;
            }

            // Colored gauge for progress line (non-selected only).
            if line_idx == 4 && !is_selected {
                let gc = progress_color(task.progress);
                let gauge_str = format!(" {}", gauge);
                let rest = format!(" {} │ {}", task.progress.title(), task.priority.title());
                let max_rest = width.saturating_sub(gauge_str.width());
                let rest_clamped = clamp_text(&rest, max_rest);
                queue!(
                    stdout,
                    SetForegroundColor(gc),
                    Print(&gauge_str),
                    SetForegroundColor(Color::DarkGrey),
                    Print(&rest_clamped),
                )?;
                let used = gauge_str.width() + rest_clamped.width();
                let pad = width.saturating_sub(used);
                if pad > 0 {
                    queue!(stdout, Print(" ".repeat(pad)))?;
                }
                queue!(stdout, SetAttribute(Attribute::Reset), ResetColor)?;
                continue;
            }

            // Dimmed ID prefix on title line for non-selected cards.
            if line_idx == 0 && !is_selected {
                let short_id = task.id.to_string().chars().take(8).collect::<String>();
                let id_str = format!(" {} ", short_id);
                let title_max = width.saturating_sub(id_str.width());
                let title_str = clamp_text(&task.title, title_max);
                queue!(
                    stdout,
                    SetForegroundColor(Color::DarkGrey),
                    Print(&id_str),
                    ResetColor,
                    SetAttribute(Attribute::Bold),
                    Print(&title_str),
                )?;
                let used = id_str.width() + title_str.width();
                let pad = width.saturating_sub(used);
                if pad > 0 {
                    queue!(stdout, Print(" ".repeat(pad)))?;
                }
                queue!(stdout, SetAttribute(Attribute::Reset), ResetColor)?;
                continue;
            }

            let content = match line_idx {
                0 => {
                    // Title: selected card (bright + bold, includes id).
                    let short_id = task.id.to_string().chars().take(8).collect::<String>();
                    queue!(stdout, SetAttribute(Attribute::Bold))?;
                    format!(" {} {}", short_id, task.title)
                }
                1 => {
                    // Desc line 1.
                    if !is_selected {
                        queue!(stdout, SetForegroundColor(Color::DarkGrey))?;
                    }
                    format!(" {}", desc_lines.first().map(|s| s.as_str()).unwrap_or(""))
                }
                2 => {
                    // Desc line 2.
                    if !is_selected {
                        queue!(stdout, SetForegroundColor(Color::DarkGrey))?;
                    }
                    let l2 = desc_lines.get(1).map(|s| s.as_str()).unwrap_or("");
                    format!(" {}", l2)
                }
                3 => {
                    // Thin separator.
                    if !is_selected {
                        queue!(stdout, SetForegroundColor(Color::DarkGrey))?;
                    }
                    format!(" {}", "─".repeat(inner_w))
                }
                4 => {
                    // Progress │ Priority (selected: single color).
                    format!(" {}", table_row1)
                }
                5 => {
                    // Due │ Deps.
                    if !is_selected {
                        queue!(stdout, SetForegroundColor(Color::DarkGrey))?;
                    }
                    format!(" {}", table_row2)
                }
                _ => String::new(),
            };

            let padded = pad_to_width(&clamp_text(&content, width), width);
            queue!(
                stdout,
                Print(padded),
                SetAttribute(Attribute::Reset),
                ResetColor
            )?;
        }

        y_cursor += CARD_LINES as u16;

        // Render sub-issues below the card.
        if has_children {
            let max_shown = 3usize;
            for &child_idx in child_indices.iter().take(max_shown) {
                if y_cursor >= max_y {
                    break;
                }
                let child = &app.tasks[child_idx];
                let icon = match child.progress {
                    Progress::Done => "\u{25cf}",
                    Progress::InProgress => "\u{25d0}",
                    Progress::Todo => "\u{25cb}",
                    Progress::Backlog => "\u{25cc}",
                };
                let prefix_str = " \u{21b3} ";
                let title_max = width.saturating_sub(prefix_str.width() + icon.width() + 1);
                let title_text = clamp_text(&child.title, title_max);
                queue!(
                    stdout,
                    MoveTo(x, y_cursor),
                    SetForegroundColor(Color::DarkGrey),
                    Print(prefix_str),
                    SetForegroundColor(progress_color(child.progress)),
                    Print(icon),
                    SetForegroundColor(Color::DarkGrey),
                    Print(format!(" {}", title_text)),
                )?;
                let used = prefix_str.width() + icon.width() + 1 + title_text.width();
                let pad = width.saturating_sub(used);
                if pad > 0 {
                    queue!(stdout, Print(" ".repeat(pad)))?;
                }
                queue!(stdout, ResetColor)?;
                y_cursor += 1;
            }
            if child_indices.len() > max_shown && y_cursor < max_y {
                let more_text = format!("    +{} more", child_indices.len() - max_shown);
                queue!(
                    stdout,
                    MoveTo(x, y_cursor),
                    SetForegroundColor(Color::DarkGrey),
                    Print(pad_to_width(&clamp_text(&more_text, width), width)),
                    ResetColor,
                )?;
                y_cursor += 1;
            }
        } else {
            // Blank padding line for cards without children.
            if y_cursor < max_y {
                queue!(stdout, MoveTo(x, y_cursor), Print(pad_to_width("", width)))?;
                y_cursor += 1;
            }
        }

        // Spacer line.
        if y_cursor < max_y {
            queue!(stdout, MoveTo(x, y_cursor), Print(pad_to_width("", width)))?;
            y_cursor += 1;
        }
    }

    // If empty, show hint
    if indices.is_empty() {
        queue!(
            stdout,
            MoveTo(x, y),
            SetForegroundColor(Color::DarkGrey),
            Print(clamp_text(" (empty)", width)),
            ResetColor
        )?;
    }

    Ok(())
}

fn render_timeline_tab(stdout: &mut Stdout, app: &mut App, cols: u16, rows: u16) -> io::Result<()> {
    use chrono::{Datelike, Duration as ChronoDuration, Local};

    let width = cols as usize;
    let (x_margin, _gap) = choose_layout(width, 1);
    let x = x_margin as u16;
    let content_width = width.saturating_sub(x_margin * 2);
    let y_help = rows.saturating_sub(1);

    // Layout: [label_width] | [gantt_width]
    let label_width = 24usize.min(content_width / 3);
    let gantt_width = content_width.saturating_sub(label_width + 3); // 3 for " | "

    // Determine date range (today + 4 weeks by default, expand if tasks go further)
    let today = Local::now().date_naive();
    let mut min_date = today;
    let mut max_date = today + ChronoDuration::days(28);

    for task in &app.tasks {
        let start = task
            .start_date
            .map(|dt| dt.date_naive())
            .unwrap_or_else(|| task.created_at.date_naive());
        let end = task.due_date.unwrap_or(start + ChronoDuration::days(7));
        if start < min_date {
            min_date = start;
        }
        if end > max_date {
            max_date = end;
        }
    }

    let total_days = (max_date - min_date).num_days().max(1) as usize;

    // Header: month labels
    let header_y = 3u16;
    let gantt_x = x + label_width as u16 + 3;

    // Draw month markers
    queue!(stdout, MoveTo(x, header_y))?;
    queue!(
        stdout,
        SetAttribute(Attribute::Bold),
        Print(pad_to_width("Task", label_width)),
        SetAttribute(Attribute::Reset),
        SetForegroundColor(Color::DarkGrey),
        Print(" │ ")
    )?;

    // Generate day markers for the header
    let mut header_str = String::new();
    let mut last_month: Option<u32> = None;
    for i in 0..gantt_width {
        let day_offset = (i * total_days) / gantt_width;
        let date = min_date + ChronoDuration::days(day_offset as i64);
        if last_month != Some(date.month()) {
            let month_name = match date.month() {
                1 => "Jan",
                2 => "Feb",
                3 => "Mar",
                4 => "Apr",
                5 => "May",
                6 => "Jun",
                7 => "Jul",
                8 => "Aug",
                9 => "Sep",
                10 => "Oct",
                11 => "Nov",
                12 => "Dec",
                _ => "",
            };
            // Only show if there's room
            if header_str.len() + 3 <= gantt_width {
                header_str.push_str(month_name);
            }
            last_month = Some(date.month());
        } else if header_str.len() < gantt_width {
            header_str.push(' ');
        }
    }
    queue!(
        stdout,
        Print(clamp_text(&header_str, gantt_width)),
        ResetColor
    )?;

    // Draw today marker position
    let today_offset = (today - min_date).num_days().max(0) as usize;
    let today_col = if total_days > 0 {
        (today_offset * gantt_width) / total_days
    } else {
        0
    };

    // Use sorted_timeline_tasks for consistent ordering with key handler
    let indices = sorted_timeline_tasks(&app.tasks);
    let task_count = indices.len();

    // Clamp selection
    if task_count > 0 {
        if app.timeline_selected >= task_count {
            app.timeline_selected = task_count - 1;
        }
    } else {
        app.timeline_selected = 0;
    }

    // Detail panel at the bottom takes 5 rows (legend_y area)
    let detail_height = if task_count > 0 { 5u16 } else { 0 };
    let list_top = 5u16;
    let list_bottom = rows.saturating_sub(detail_height + 3); // room for detail + help
    let list_height = list_bottom.saturating_sub(list_top) as usize;

    // Scroll
    if task_count > 0 {
        if app.timeline_selected < app.timeline_scroll {
            app.timeline_scroll = app.timeline_selected;
        } else if app.timeline_selected >= app.timeline_scroll + list_height {
            app.timeline_scroll = app
                .timeline_selected
                .saturating_sub(list_height.saturating_sub(1));
        }
        let max_scroll = task_count.saturating_sub(list_height);
        app.timeline_scroll = app.timeline_scroll.min(max_scroll);
    }

    for (vis_row, sorted_pos) in (app.timeline_scroll..task_count)
        .take(list_height)
        .enumerate()
    {
        let task = &app.tasks[indices[sorted_pos]];
        let y = list_top + vis_row as u16;
        let is_selected = sorted_pos == app.timeline_selected;

        // Task label
        const BUCKET_SYMBOLS: &[&str] = &["●", "◆", "■", "▲", "★", "♦"];
        let prefix = if task.is_child() {
            "↳"
        } else {
            let bi = app
                .settings
                .buckets
                .iter()
                .position(|b| b.name == task.bucket)
                .unwrap_or(0);
            BUCKET_SYMBOLS[bi % BUCKET_SYMBOLS.len()]
        };
        let label = format!("{} {}", prefix, task.title);

        queue!(stdout, MoveTo(x, y))?;
        if is_selected {
            queue!(
                stdout,
                SetForegroundColor(Color::Black),
                SetBackgroundColor(Color::White),
                Print(pad_to_width(&clamp_text(&label, label_width), label_width)),
                ResetColor,
                SetForegroundColor(Color::DarkGrey),
                Print(" │ "),
                ResetColor
            )?;
        } else {
            queue!(
                stdout,
                Print(clamp_text(&label, label_width)),
                SetForegroundColor(Color::DarkGrey),
                Print(" │ "),
                ResetColor
            )?;
        }

        // Calculate bar position
        let start = task
            .start_date
            .map(|dt| dt.date_naive())
            .unwrap_or_else(|| task.created_at.date_naive());
        let end = task.due_date.unwrap_or(start + ChronoDuration::days(7));

        let start_offset = (start - min_date).num_days().max(0) as usize;
        let end_offset = (end - min_date).num_days().max(0) as usize;

        let bar_start = (start_offset * gantt_width) / total_days.max(1);
        let bar_end = ((end_offset * gantt_width) / total_days.max(1)).max(bar_start + 1);

        // Build bar with start/end date labels
        let start_label = start.format("%m/%d").to_string();
        let end_label = end.format("%m/%d").to_string();

        let bar_len = bar_end - bar_start;
        let both_len = start_label.len() + 1 + end_label.len(); // "MM/DD .... MM/DD"

        // Color based on progress
        let bar_color = match task.progress {
            Progress::Done => Color::Green,
            Progress::InProgress => Color::Yellow,
            Progress::Todo => Color::Blue,
            Progress::Backlog => Color::DarkGrey,
        };

        // Draw the Gantt bar column by column
        // Pre-build the bar string, then overlay date labels
        let mut bar_chars: Vec<char> = Vec::with_capacity(gantt_width);
        for col in 0..gantt_width {
            if col >= bar_start && col < bar_end {
                bar_chars.push('█');
            } else if col == today_col {
                bar_chars.push('│');
            } else {
                bar_chars.push(' ');
            }
        }

        // Overlay date labels onto the bar area
        if bar_len > both_len {
            // Both labels fit inside the bar
            for (j, ch) in start_label.chars().enumerate() {
                bar_chars[bar_start + j] = ch;
            }
            let end_pos = bar_end - end_label.len();
            for (j, ch) in end_label.chars().enumerate() {
                bar_chars[end_pos + j] = ch;
            }
        } else if bar_len > start_label.len() {
            // Only start label fits inside
            for (j, ch) in start_label.chars().enumerate() {
                bar_chars[bar_start + j] = ch;
            }
            // End label after bar if room
            let after = bar_end + 1;
            if after + end_label.len() <= gantt_width {
                for (j, ch) in end_label.chars().enumerate() {
                    bar_chars[after + j] = ch;
                }
            }
        } else {
            // Labels outside the bar
            if bar_start > start_label.len() {
                let before = bar_start - start_label.len() - 1;
                for (j, ch) in start_label.chars().enumerate() {
                    bar_chars[before + j] = ch;
                }
            }
            let after = bar_end + 1;
            if after + end_label.len() <= gantt_width {
                for (j, ch) in end_label.chars().enumerate() {
                    bar_chars[after + j] = ch;
                }
            }
        }

        // Render bar: color bar chars, dim date label chars
        queue!(stdout, MoveTo(gantt_x, y))?;
        for (col, &ch) in bar_chars.iter().enumerate() {
            let in_bar = col >= bar_start && col < bar_end;
            if ch == '█' {
                queue!(
                    stdout,
                    SetForegroundColor(bar_color),
                    Print('█'),
                    ResetColor
                )?;
            } else if in_bar {
                // Date label char inside bar: show as inverse
                queue!(
                    stdout,
                    SetForegroundColor(Color::Black),
                    SetBackgroundColor(bar_color),
                    Print(ch),
                    ResetColor
                )?;
            } else if ch == '│' {
                queue!(
                    stdout,
                    SetForegroundColor(Color::DarkGrey),
                    Print('│'),
                    ResetColor
                )?;
            } else if ch != ' ' {
                // Date label outside bar
                queue!(
                    stdout,
                    SetForegroundColor(Color::DarkGrey),
                    Print(ch),
                    ResetColor
                )?;
            } else {
                queue!(stdout, Print(' '))?;
            }
        }
    }

    if task_count == 0 {
        queue!(
            stdout,
            MoveTo(x, list_top),
            SetForegroundColor(Color::DarkGrey),
            Print("No tasks yet. Create some in the Buckets tab."),
            ResetColor
        )?;
    }

    // Detail panel for selected task
    if task_count > 0 {
        let detail_y = list_bottom + 1;
        let sep = "─".repeat(content_width);
        queue!(
            stdout,
            MoveTo(x, detail_y.saturating_sub(1)),
            SetForegroundColor(Color::DarkGrey),
            Print(&sep),
            ResetColor
        )?;

        let task = &app.tasks[indices[app.timeline_selected]];
        let start = task
            .start_date
            .map(|dt| dt.date_naive())
            .unwrap_or_else(|| task.created_at.date_naive());
        let end = task.due_date.unwrap_or(start + ChronoDuration::days(7));
        let gauge = progress_gauge(task.progress);
        let desc = if task.description.trim().is_empty() {
            "—"
        } else {
            task.description.trim()
        };

        let line1 = format!(
            "{} │ {} {} │ {} │ {} → {}",
            task.title,
            gauge,
            task.progress.title(),
            task.priority.title(),
            start.format("%Y-%m-%d"),
            end.format("%Y-%m-%d"),
        );
        let line2 = format!("  {}", desc);

        queue!(
            stdout,
            MoveTo(x, detail_y),
            SetAttribute(Attribute::Bold),
            Print(clamp_text(&line1, content_width)),
            SetAttribute(Attribute::Reset)
        )?;
        queue!(
            stdout,
            MoveTo(x, detail_y + 1),
            SetForegroundColor(Color::DarkGrey),
            Print(clamp_text(&line2, content_width)),
            ResetColor
        )?;
    }

    // Legend
    let legend_y = rows.saturating_sub(3);
    queue!(
        stdout,
        MoveTo(x, legend_y),
        SetForegroundColor(Color::DarkGrey),
        Print("│ = today  "),
        SetForegroundColor(Color::Green),
        Print("█ Done  "),
        SetForegroundColor(Color::Yellow),
        Print("█ In Progress  "),
        SetForegroundColor(Color::Blue),
        Print("█ Todo  "),
        SetForegroundColor(Color::DarkGrey),
        Print("█ Backlog"),
        ResetColor
    )?;

    queue!(
        stdout,
        MoveTo(x, y_help),
        SetForegroundColor(Color::DarkGrey),
        Print("↑/↓ select • e edit • 1 buckets • 3 kanban • 4 settings • q quit"),
        ResetColor
    )?;

    Ok(())
}

fn render_kanban_tab(stdout: &mut Stdout, app: &mut App, cols: u16, rows: u16) -> io::Result<()> {
    let width = cols as usize;
    let (x_margin, gap) = choose_layout(width, 4);
    let x = x_margin as u16;
    let y_help = rows.saturating_sub(1);
    let today = Utc::now().date_naive();

    queue!(
        stdout,
        MoveTo(x, 3),
        SetForegroundColor(Color::DarkGrey),
        Print("Kanban (grouped by progress)."),
        ResetColor
    )?;

    let content_width = width.saturating_sub(x_margin * 2);
    let col_width = content_width.saturating_sub(gap * 3) / 4;
    let col_x = [
        x_margin,
        x_margin + col_width + gap,
        x_margin + 2 * (col_width + gap),
        x_margin + 3 * (col_width + gap),
    ];

    const CARD_LINES: u16 = 2; // title line + metadata line
    let list_top = 8u16; // leave room for header + separator
    let list_bottom = y_help.saturating_sub(1);
    let list_height = list_bottom.saturating_sub(list_top) as usize;
    let max_visible = list_height / CARD_LINES as usize;

    for (i, stage) in Progress::ALL.iter().enumerate() {
        let cx = col_x[i] as u16;
        let is_active_col = *stage == app.kanban_stage;
        let ids = kanban_task_ids(&app.tasks, *stage);
        let count = ids.len();
        let stage_idx = stage.stage_index();

        // ── Column header: "Todo (25)" ──
        let header = format!("{} ({})", stage.title(), count);
        queue!(stdout, MoveTo(cx, 5))?;
        if is_active_col {
            queue!(
                stdout,
                SetForegroundColor(progress_color(*stage)),
                SetAttribute(Attribute::Bold),
                SetAttribute(Attribute::Underlined),
                Print(clamp_text(&header, col_width)),
                SetAttribute(Attribute::Reset),
                ResetColor
            )?;
        } else {
            queue!(
                stdout,
                SetForegroundColor(progress_color(*stage)),
                SetAttribute(Attribute::Bold),
                Print(clamp_text(&header, col_width)),
                SetAttribute(Attribute::Reset),
                ResetColor
            )?;
        }

        // Thin separator under header.
        let sep: String = "─".repeat(col_width);
        queue!(
            stdout,
            MoveTo(cx, 6),
            SetForegroundColor(if is_active_col {
                progress_color(*stage)
            } else {
                Color::DarkGrey
            }),
            Print(clamp_text(&sep, col_width)),
            ResetColor
        )?;

        // ── Scrolling ──
        let scroll = &mut app.kanban_scroll[stage_idx];
        if is_active_col {
            let sel_pos = app
                .kanban_selected
                .and_then(|id| ids.iter().position(|x| *x == id))
                .unwrap_or(0);
            if max_visible > 0 {
                if sel_pos >= *scroll + max_visible {
                    *scroll = sel_pos.saturating_sub(max_visible.saturating_sub(1));
                }
                if sel_pos < *scroll {
                    *scroll = sel_pos;
                }
            }
        }
        let scroll_val = *scroll;

        let visible_count = max_visible.min(count.saturating_sub(scroll_val));
        let has_above = scroll_val > 0;
        let has_below = scroll_val + visible_count < count;

        // ── Overflow: above ──
        let mut y_cur = list_top;
        if has_above {
            let above_text = format!("▲ {} above", scroll_val);
            queue!(
                stdout,
                MoveTo(cx, y_cur),
                SetForegroundColor(Color::DarkGrey),
                Print(clamp_text(&above_text, col_width)),
                ResetColor
            )?;
            y_cur += 1;
        }

        // ── Task cards ──
        for id in ids.iter().skip(scroll_val).take(visible_count) {
            if y_cur + CARD_LINES > list_bottom {
                break;
            }
            let task = app.tasks.iter().find(|t| t.id == *id).unwrap();
            let is_selected = is_active_col && app.kanban_selected == Some(*id);

            // Priority bullet.
            let bullet = match task.priority {
                Priority::Critical => "◉",
                Priority::High => "●",
                Priority::Medium => "○",
                Priority::Low => "·",
            };

            // Due date string.
            let due_str: Option<String> = task.due_date.map(|d| {
                if d < today {
                    format!("⚠ {}", d.format("%b %d"))
                } else if d == today {
                    "due today".to_string()
                } else {
                    d.format("%b %d").to_string()
                }
            });

            let meta_line = if let Some(due) = &due_str {
                format!("   {} · {}", task.bucket, due)
            } else {
                format!("   {}", task.bucket)
            };

            // ── Line 1: priority bullet + title ──
            queue!(stdout, MoveTo(cx, y_cur))?;
            if is_selected {
                let full = if task.is_child() {
                    format!(" {} ↳ {}", bullet, task.title)
                } else {
                    format!(" {} {}", bullet, task.title)
                };
                queue!(
                    stdout,
                    SetForegroundColor(Color::Black),
                    SetBackgroundColor(Color::White),
                    Print(pad_to_width(&clamp_text(&full, col_width), col_width)),
                    ResetColor
                )?;
            } else {
                let prefix = format!(" {} ", bullet);
                queue!(
                    stdout,
                    SetForegroundColor(priority_color(task.priority)),
                    Print(&prefix),
                    ResetColor
                )?;
                let title_max = col_width.saturating_sub(prefix.width());
                let title_text = if task.is_child() {
                    format!("↳ {}", task.title)
                } else {
                    task.title.clone()
                };
                queue!(stdout, Print(clamp_text(&title_text, title_max)))?;
            }

            // ── Line 2: bucket + due date ──
            queue!(stdout, MoveTo(cx, y_cur + 1))?;
            if is_selected {
                queue!(
                    stdout,
                    SetForegroundColor(Color::Black),
                    SetBackgroundColor(Color::White),
                    Print(pad_to_width(&clamp_text(&meta_line, col_width), col_width)),
                    ResetColor
                )?;
            } else {
                let is_overdue = task.due_date.is_some_and(|d| d < today);
                let is_due_today = task.due_date == Some(today);
                let meta_color = if is_overdue {
                    Color::Red
                } else if is_due_today {
                    Color::Yellow
                } else {
                    Color::DarkGrey
                };
                queue!(
                    stdout,
                    SetForegroundColor(meta_color),
                    Print(clamp_text(&meta_line, col_width)),
                    ResetColor
                )?;
            }

            y_cur += CARD_LINES;
        }

        // ── Overflow: below ──
        if has_below {
            let below_count = count.saturating_sub(scroll_val + visible_count);
            let below_text = format!("▼ {} more", below_count);
            let y_below = y_cur.min(list_bottom.saturating_sub(1));
            queue!(
                stdout,
                MoveTo(cx, y_below),
                SetForegroundColor(Color::DarkGrey),
                Print(clamp_text(&below_text, col_width)),
                ResetColor
            )?;
        }
    }

    // ── Legend ──
    let legend_y = y_help.saturating_sub(1);
    queue!(
        stdout,
        MoveTo(x, legend_y),
        SetForegroundColor(Color::DarkGrey),
        Print("◉ critical  "),
        SetForegroundColor(Color::Yellow),
        Print("● high  "),
        ResetColor,
        Print("○ medium  "),
        SetForegroundColor(Color::DarkGrey),
        Print("· low"),
        ResetColor
    )?;

    // ── Help bar ──
    queue!(
        stdout,
        MoveTo(x, y_help),
        SetForegroundColor(Color::DarkGrey),
        Print("←/→ columns • ↑/↓ select • p advance • P back • e edit • 1/2 tabs • q quit"),
        ResetColor
    )?;

    Ok(())
}

fn render_settings_tab(stdout: &mut Stdout, app: &App, cols: u16, rows: u16) -> io::Result<()> {
    let width = cols as usize;
    let (x_margin, _) = choose_layout(width, 1);
    let x = x_margin as u16;
    let content_width = width.saturating_sub(x_margin * 2);
    let y_help = rows.saturating_sub(1);

    queue!(
        stdout,
        MoveTo(x, 3),
        SetAttribute(Attribute::Bold),
        Print(" Settings"),
        SetAttribute(Attribute::Reset)
    )?;

    let label_w = 16usize;
    let value_w = content_width.saturating_sub(label_w + 2);

    for (i, field) in SettingsField::ALL.iter().enumerate() {
        let y = 5 + i as u16;
        let is_current = *field == app.settings_field;

        let value = match field {
            SettingsField::OwnerName => {
                if app.settings.owner_name.trim().is_empty() {
                    "John (default)".to_string()
                } else {
                    app.settings.owner_name.clone()
                }
            }
            SettingsField::AiEnabled => {
                if app.settings.enabled {
                    "On".to_string()
                } else {
                    "Off".to_string()
                }
            }
            SettingsField::OpenAiKey => mask_api_key(&app.settings.openai_api_key),
            SettingsField::AnthropicKey => mask_api_key(&app.settings.anthropic_api_key),
            SettingsField::Model => {
                if app.settings.model.is_empty() {
                    "(default)".to_string()
                } else {
                    app.settings.model.clone()
                }
            }
            SettingsField::ApiUrl => {
                if app.settings.api_url.is_empty() {
                    "(default)".to_string()
                } else {
                    app.settings.api_url.clone()
                }
            }
            SettingsField::Timeout => format!("{}s", app.settings.timeout_secs),
            SettingsField::ShowBacklog => if app.settings.show_backlog {
                "\u{2611} On"
            } else {
                "\u{2610} Off"
            }
            .to_string(),
            SettingsField::ShowTodo => if app.settings.show_todo {
                "\u{2611} On"
            } else {
                "\u{2610} Off"
            }
            .to_string(),
            SettingsField::ShowInProgress => if app.settings.show_in_progress {
                "\u{2611} On"
            } else {
                "\u{2610} Off"
            }
            .to_string(),
            SettingsField::ShowDone => if app.settings.show_done {
                "\u{2611} On"
            } else {
                "\u{2610} Off"
            }
            .to_string(),
        };

        let show_value = if is_current && app.settings_editing {
            format!("{}\u{258f}", app.settings_buf)
        } else if is_current && (field.is_toggle() || *field == SettingsField::Model) {
            format!("\u{25c2} {} \u{25b8}", value)
        } else {
            value
        };

        queue!(stdout, MoveTo(x, y))?;
        if is_current {
            queue!(
                stdout,
                SetForegroundColor(Color::Black),
                SetBackgroundColor(Color::White)
            )?;
        }

        let label = format!(" {:<width$}", field.label(), width = label_w);
        let row_text = format!("{}{}", label, clamp_text(&show_value, value_w));
        queue!(
            stdout,
            Print(pad_to_width(
                &clamp_text(&row_text, content_width),
                content_width
            )),
            ResetColor
        )?;
    }

    // AI status.
    let status_y = 5 + SettingsField::ALL.len() as u16 + 1;
    let ai_status = if app.ai.is_some() {
        "AI active \u{2713}"
    } else if !app.settings.enabled {
        "AI disabled"
    } else if app.settings.openai_api_key.trim().is_empty()
        && app.settings.anthropic_api_key.trim().is_empty()
    {
        "No API key configured"
    } else {
        "AI inactive"
    };
    queue!(
        stdout,
        MoveTo(x, status_y),
        SetForegroundColor(Color::DarkGrey),
        Print(format!(" {}", ai_status)),
        ResetColor
    )?;

    // Version.
    queue!(
        stdout,
        MoveTo(x, status_y + 2),
        SetForegroundColor(Color::DarkGrey),
        Print(format!(" aipm v{}", env!("CARGO_PKG_VERSION"))),
        ResetColor
    )?;

    // Cursor.
    if app.settings_editing {
        let field_idx = SettingsField::ALL
            .iter()
            .position(|f| *f == app.settings_field)
            .unwrap_or(0);
        let cy = 5 + field_idx as u16;
        let cx = x as usize + 1 + label_w + app.settings_buf.width();
        queue!(
            stdout,
            MoveTo((cx as u16).min(cols.saturating_sub(1)), cy),
            Show
        )?;
    } else {
        queue!(stdout, Hide)?;
    }

    // Help.
    let help = if app.settings_editing {
        "enter save \u{2022} esc cancel"
    } else {
        "\u{2191}/\u{2193} navigate \u{2022} enter edit \u{2022} \u{2190}/\u{2192} toggle \u{2022} 1/2/3 tabs \u{2022} q quit"
    };
    queue!(
        stdout,
        MoveTo(x, y_help),
        SetForegroundColor(Color::DarkGrey),
        Print(clamp_text(help, content_width)),
        ResetColor
    )?;

    Ok(())
}

fn mask_api_key(key: &str) -> String {
    if key.is_empty() {
        return "(not set)".to_string();
    }
    if key.len() <= 4 {
        return "\u{2022}\u{2022}\u{2022}\u{2022}".to_string();
    }
    let visible = &key[key.len() - 4..];
    format!("\u{2022}\u{2022}\u{2022}\u{2022}{}", visible)
}

fn render_toast(stdout: &mut Stdout, app: &App, cols: u16, rows: u16) -> io::Result<()> {
    let Some((status, shown_at, persistent)) = &app.status else {
        return Ok(());
    };

    let is_error = status.starts_with("AI error") || status.starts_with("Save failed");
    let box_width = (cols as usize).clamp(20, 45);
    let inner_w = box_width.saturating_sub(4);

    // Word-wrap the message.
    let mut lines: Vec<String> = Vec::new();
    let mut current_line = String::new();
    for word in status.split_whitespace() {
        if current_line.is_empty() {
            current_line = word.to_string();
        } else if current_line.width() + 1 + word.width() <= inner_w {
            current_line.push(' ');
            current_line.push_str(word);
        } else {
            lines.push(current_line);
            current_line = word.to_string();
        }
    }
    if !current_line.is_empty() {
        lines.push(current_line);
    }
    let lines = &lines[..lines.len().min(4)]; // Max 4 lines

    let box_height = (lines.len() as u16 + 2).max(3); // border + content + dismiss
                                                      // Right-aligned, 1 column from the right edge.
    let x0 = cols.saturating_sub(box_width as u16 + 1);
    // Grow upward from just above the input field (input is at rows - 3).
    let y_bottom = rows.saturating_sub(4);
    let y0 = y_bottom.saturating_sub(box_height.saturating_sub(1));

    // Clear area.
    for dy in 0..box_height {
        queue!(
            stdout,
            MoveTo(x0, y0 + dy),
            Print(pad_to_width("", box_width))
        )?;
    }

    // Top border.
    let border_color = if is_error {
        Color::Red
    } else {
        Color::DarkGrey
    };
    let border_label = if is_error { "Error" } else { "Info" };
    let border_fill = "\u{2500}".repeat(box_width.saturating_sub(border_label.len() + 6));
    queue!(
        stdout,
        MoveTo(x0, y0),
        SetForegroundColor(border_color),
        Print(clamp_text(
            &format!("\u{250c}\u{2500} {} \u{2500}{} ", border_label, border_fill),
            box_width,
        )),
        ResetColor
    )?;

    // Message lines.
    let inner_x = x0 + 2;
    for (i, line) in lines.iter().enumerate() {
        queue!(
            stdout,
            MoveTo(inner_x, y0 + 1 + i as u16),
            SetForegroundColor(if is_error { Color::Red } else { Color::White }),
            Print(clamp_text(line, inner_w)),
            ResetColor
        )?;
    }

    // Countdown ticker / spinner for persistent toasts.
    if *persistent {
        // Animated spinner for persistent (AI-thinking) toasts.
        let frames = ["⠋", "⠙", "⠸", "⠰", "⠦", "⠇"];
        let idx = (shown_at.elapsed().as_millis() / 200) as usize % frames.len();
        let spinner = frames[idx];
        let hint = format!("{} working…", spinner);
        queue!(
            stdout,
            MoveTo(inner_x, y0 + box_height - 1),
            SetForegroundColor(Color::DarkGrey),
            Print(clamp_text(&hint, inner_w)),
            ResetColor
        )?;
    } else {
        let elapsed = shown_at.elapsed();
        let remaining = TOAST_DURATION.saturating_sub(elapsed);
        let secs_left = remaining.as_secs() + if remaining.subsec_millis() > 0 { 1 } else { 0 };
        let total_ticks = 6usize;
        let filled = (total_ticks as u64 * remaining.as_millis() as u64
            / TOAST_DURATION.as_millis() as u64) as usize;
        let bar: String = "━".repeat(filled) + &"┉".repeat(total_ticks.saturating_sub(filled));
        let ticker = format!("{}s {}", secs_left, bar);
        let ticker_w = ticker.width();

        // Dismiss hint with ticker right-aligned.
        let hint = "any key";
        let gap = inner_w.saturating_sub(hint.width() + ticker_w);
        let dismiss_line = format!("{}{}{}", hint, " ".repeat(gap), ticker);
        queue!(
            stdout,
            MoveTo(inner_x, y0 + box_height - 1),
            SetForegroundColor(Color::DarkGrey),
            Print(clamp_text(&dismiss_line, inner_w)),
            ResetColor
        )?;
    }

    Ok(())
}

fn render_delete_confirm(stdout: &mut Stdout, app: &App, cols: u16, rows: u16) -> io::Result<()> {
    let Some(id) = app.confirm_delete_id else {
        return Ok(());
    };
    let title = app
        .tasks
        .iter()
        .find(|t| t.id == id)
        .map(|t| t.title.as_str())
        .unwrap_or("Unknown");

    let box_width = (cols as usize).clamp(30, 50);
    let box_height = 5u16;
    let x0 = (cols.saturating_sub(box_width as u16)) / 2;
    let y0 = (rows.saturating_sub(box_height)) / 2;

    // Clear overlay area.
    for dy in 0..box_height {
        queue!(
            stdout,
            MoveTo(x0, y0 + dy),
            Print(pad_to_width("", box_width))
        )?;
    }

    // Border top.
    let border_fill: String = "─".repeat(box_width.saturating_sub(14));
    queue!(
        stdout,
        MoveTo(x0, y0),
        SetForegroundColor(Color::Red),
        Print(clamp_text(
            &format!("┌─ Delete? ─{} ", border_fill),
            box_width,
        )),
        ResetColor
    )?;

    // Task title.
    let inner_x = x0 + 2;
    let inner_w = box_width.saturating_sub(4);
    let msg = format!(
        "Delete \"{}\"?",
        clamp_text(title, inner_w.saturating_sub(10))
    );
    queue!(
        stdout,
        MoveTo(inner_x, y0 + 2),
        SetForegroundColor(Color::White),
        Print(clamp_text(&msg, inner_w)),
        ResetColor
    )?;

    // Help line.
    let help = "enter confirm \u{2022} esc cancel";
    queue!(
        stdout,
        MoveTo(inner_x, y0 + box_height - 1),
        SetForegroundColor(Color::DarkGrey),
        Print(clamp_text(help, inner_w)),
        ResetColor
    )?;

    queue!(stdout, Hide)?;

    Ok(())
}

fn render_bucket_edit_overlay(
    stdout: &mut Stdout,
    app: &App,
    cols: u16,
    rows: u16,
) -> io::Result<()> {
    let Some(bucket) = app.settings.buckets.get(app.selected_bucket) else {
        return Ok(());
    };

    let box_width = (cols as usize).clamp(30, 50);
    let label_w = 14usize;
    let value_w = box_width.saturating_sub(label_w + 4);
    let box_height = 7u16;
    let x0 = (cols.saturating_sub(box_width as u16)) / 2;
    let y0 = (rows.saturating_sub(box_height)) / 2;

    // Clear overlay area.
    for dy in 0..box_height {
        queue!(
            stdout,
            MoveTo(x0, y0 + dy),
            Print(pad_to_width("", box_width))
        )?;
    }

    // Border top.
    let border_fill: String = "\u{2500}".repeat(box_width.saturating_sub(16));
    queue!(
        stdout,
        MoveTo(x0, y0),
        SetForegroundColor(Color::DarkGrey),
        Print(clamp_text(
            &format!("\u{250c}\u{2500} Edit Bucket \u{2500}{} ", border_fill),
            box_width,
        )),
        ResetColor
    )?;

    let inner_x = x0 + 2;
    let inner_w = box_width.saturating_sub(4);

    // Fields: Name and Description.
    for (i, (field, label)) in [
        (BucketEditField::Name, "Name"),
        (BucketEditField::Description, "Desc"),
    ]
    .iter()
    .enumerate()
    {
        let y_line = y0 + 2 + i as u16;
        let is_current = *field == app.bucket_edit_field;

        let value = if is_current && app.bucket_editing_text {
            format!("{}\u{258f}", app.bucket_edit_buf)
        } else if is_current {
            match field {
                BucketEditField::Name => bucket.name.clone(),
                BucketEditField::Description => bucket.description.clone().unwrap_or_default(),
            }
        } else {
            match field {
                BucketEditField::Name => bucket.name.clone(),
                BucketEditField::Description => bucket
                    .description
                    .clone()
                    .unwrap_or_else(|| "\u{2014}".to_string()),
            }
        };

        let label_str = format!("{:<width$}", label, width = label_w);
        let row_text = format!("{}{}", label_str, clamp_text(&value, value_w));

        queue!(stdout, MoveTo(inner_x, y_line))?;
        if is_current {
            queue!(
                stdout,
                SetForegroundColor(Color::Black),
                SetBackgroundColor(Color::White),
                Print(pad_to_width(&clamp_text(&row_text, inner_w), inner_w)),
                ResetColor
            )?;
        } else {
            queue!(
                stdout,
                SetForegroundColor(Color::White),
                Print(pad_to_width(&clamp_text(&row_text, inner_w), inner_w)),
                ResetColor
            )?;
        }
    }

    // Help line.
    let help = if app.bucket_editing_text {
        "enter save \u{2022} esc cancel"
    } else {
        "enter edit \u{2022} \u{2191}/\u{2193} fields \u{2022} esc close"
    };
    queue!(
        stdout,
        MoveTo(inner_x, y0 + box_height - 1),
        SetForegroundColor(Color::DarkGrey),
        Print(clamp_text(help, inner_w)),
        ResetColor
    )?;

    // Show cursor when editing text.
    if app.bucket_editing_text {
        let cursor_x = inner_x as usize + label_w + app.bucket_edit_buf.width();
        queue!(
            stdout,
            MoveTo(
                (cursor_x as u16).min(cols.saturating_sub(1)),
                y0 + 2
                    + match app.bucket_edit_field {
                        BucketEditField::Name => 0,
                        BucketEditField::Description => 1,
                    }
            ),
            Show
        )?;
    } else {
        queue!(stdout, Hide)?;
    }

    Ok(())
}

fn render_edit_overlay(stdout: &mut Stdout, app: &App, cols: u16, rows: u16) -> io::Result<()> {
    let Some(id) = app.edit_task_id else {
        return Ok(());
    };
    let Some(task) = app.tasks.iter().find(|t| t.id == id) else {
        return Ok(());
    };

    let box_width = (cols as usize).clamp(40, 60);
    let label_w = 14usize;
    let value_w = box_width.saturating_sub(label_w + 4);

    // Pre-compute wrapped description lines so we can size the box.
    let desc_text = if task.description.trim().is_empty() {
        "—".to_string()
    } else {
        task.description.clone()
    };
    let desc_editing = app.edit_field == EditField::Description && app.editing_text;
    let desc_display = if desc_editing {
        format!("{}▏", app.edit_buf)
    } else {
        desc_text.clone()
    };
    let max_desc_lines = 6usize;
    let desc_wrapped = wrap_text(&desc_display, value_w, max_desc_lines);
    let desc_lines = desc_wrapped.len().max(1);
    let _desc_extra = desc_lines.saturating_sub(1) as u16;

    // Sub-issues section.
    let child_indices = children_of(&app.tasks, task.id);
    let child_visible = child_indices.len().min(5);
    // box_height: 9 (base fields) + desc_lines + 2 (separator + header) + child_visible
    let box_height = (11 + desc_lines as u16 + child_visible as u16).min(rows.saturating_sub(2));
    let x0 = (cols.saturating_sub(box_width as u16)) / 2;
    let y0 = (rows.saturating_sub(box_height)) / 2;

    // Clear overlay area.
    for dy in 0..box_height {
        queue!(
            stdout,
            MoveTo(x0, y0 + dy),
            Print(pad_to_width("", box_width))
        )?;
    }

    // Border top.
    queue!(
        stdout,
        MoveTo(x0, y0),
        SetForegroundColor(Color::DarkGrey),
        Print(clamp_text(
            &format!(
                "┌─ Edit: {} ",
                clamp_text(&task.title, box_width.saturating_sub(12))
            ),
            box_width
        )),
        ResetColor
    )?;

    let inner_x = x0 + 2;
    let inner_w = box_width.saturating_sub(4);

    // Track the current y offset as we render fields.
    let mut y_cursor = y0 + 2;

    for field in EditField::ALL.iter() {
        let is_current = *field == app.edit_field;

        if *field == EditField::Description {
            // Render description as multi-line.
            let label = format!("{:<width$}", field.label(), width = label_w);
            for (li, line) in desc_wrapped.iter().enumerate() {
                queue!(stdout, MoveTo(inner_x, y_cursor))?;
                if is_current {
                    queue!(
                        stdout,
                        SetForegroundColor(Color::Black),
                        SetBackgroundColor(Color::White)
                    )?;
                } else {
                    queue!(stdout, SetForegroundColor(Color::White))?;
                }
                let prefix = if li == 0 {
                    &label
                } else {
                    &" ".repeat(label_w)
                };
                let row_text = format!("{}{}", prefix, clamp_text(line, value_w));
                queue!(
                    stdout,
                    Print(pad_to_width(&clamp_text(&row_text, inner_w), inner_w)),
                    ResetColor
                )?;
                y_cursor += 1;
            }
            continue;
        }

        if *field == EditField::SubIssues {
            // Separator.
            queue!(
                stdout,
                MoveTo(inner_x, y_cursor),
                SetForegroundColor(Color::DarkGrey),
                Print(pad_to_width(&"─".repeat(inner_w.min(40)), inner_w)),
                ResetColor
            )?;
            y_cursor += 1;

            let done_count = child_indices
                .iter()
                .filter(|&&i| app.tasks[i].progress == Progress::Done)
                .count();
            let total = child_indices.len();
            let sub_sel = app.edit_sub_selected.min(total.saturating_sub(1));

            // Header line.
            let label = format!("{:<width$}", "Sub-issues", width = label_w);
            let summary = if total == 0 {
                "○ (none \u{2014} a to add)".to_string()
            } else {
                format!("\u{25d2} {} of {}", done_count, total)
            };
            let header_text = format!("{}{}", label, summary);
            queue!(stdout, MoveTo(inner_x, y_cursor))?;
            if is_current && total == 0 {
                queue!(
                    stdout,
                    SetForegroundColor(Color::Black),
                    SetBackgroundColor(Color::White)
                )?;
            } else {
                queue!(stdout, SetForegroundColor(Color::White))?;
            }
            queue!(
                stdout,
                Print(pad_to_width(&clamp_text(&header_text, inner_w), inner_w)),
                ResetColor
            )?;
            y_cursor += 1;

            // Child lines.
            for (ci, &child_idx) in child_indices.iter().enumerate().take(5) {
                let child = &app.tasks[child_idx];
                let is_sel = is_current && ci == sub_sel;
                let icon = match child.progress {
                    Progress::Done => "\u{25cf}",
                    Progress::InProgress => "\u{25d0}",
                    Progress::Todo => "\u{25cb}",
                    Progress::Backlog => "\u{25cc}",
                };
                let row_text = format!("{}  {} {}", " ".repeat(label_w), icon, child.title);
                queue!(stdout, MoveTo(inner_x, y_cursor))?;
                if is_sel {
                    queue!(
                        stdout,
                        SetForegroundColor(Color::Black),
                        SetBackgroundColor(Color::White)
                    )?;
                } else if is_current {
                    queue!(stdout, SetForegroundColor(Color::White))?;
                } else {
                    queue!(stdout, SetForegroundColor(Color::DarkGrey))?;
                }
                queue!(
                    stdout,
                    Print(pad_to_width(&clamp_text(&row_text, inner_w), inner_w)),
                    ResetColor
                )?;
                y_cursor += 1;
            }

            continue;
        }

        let value = match field {
            EditField::Title => task.title.clone(),
            EditField::Bucket => task.bucket.clone(),
            EditField::Progress => {
                format!(
                    "{} {}",
                    progress_gauge(task.progress),
                    task.progress.title()
                )
            }
            EditField::Priority => task.priority.title().to_string(),
            EditField::DueDate => task
                .due_date
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "—".to_string()),
            EditField::Description | EditField::SubIssues => unreachable!(),
        };

        let show_value = if is_current && app.editing_text {
            format!("{}▏", app.edit_buf)
        } else if is_current
            && matches!(
                field,
                EditField::Bucket | EditField::Progress | EditField::Priority
            )
        {
            format!("◂ {} ▸", value)
        } else {
            value
        };

        queue!(stdout, MoveTo(inner_x, y_cursor))?;
        if is_current {
            queue!(
                stdout,
                SetForegroundColor(Color::Black),
                SetBackgroundColor(Color::White)
            )?;
        } else {
            queue!(stdout, SetForegroundColor(Color::White))?;
        }

        let label = format!("{:<width$}", field.label(), width = label_w);
        let row_text = format!("{}{}", label, clamp_text(&show_value, value_w));
        queue!(
            stdout,
            Print(pad_to_width(&clamp_text(&row_text, inner_w), inner_w)),
            ResetColor
        )?;
        y_cursor += 1;
    }

    // Help line.
    let help_y = y0 + box_height - 1;
    let help = if app.editing_text {
        "enter save • esc cancel"
    } else if app.edit_field == EditField::SubIssues {
        "↑/↓ select • enter open • a add • d delete • esc close"
    } else {
        "↑/↓ field • enter/e edit • ←/→ cycle • d delete • esc close"
    };
    queue!(
        stdout,
        MoveTo(inner_x, help_y),
        SetForegroundColor(Color::DarkGrey),
        Print(clamp_text(help, inner_w)),
        ResetColor
    )?;

    // Cursor in text editing mode.
    if app.editing_text {
        // Compute y position for the current field.
        let mut cy = y0 + 2;
        for field in EditField::ALL.iter() {
            if *field == app.edit_field {
                break;
            }
            if *field == EditField::Description {
                cy += desc_lines as u16;
            } else if *field == EditField::SubIssues {
                cy += 2 + child_visible as u16;
            } else {
                cy += 1;
            }
        }
        // For description, cursor goes on the last wrapped line.
        if app.edit_field == EditField::Description {
            cy += desc_lines.saturating_sub(1) as u16;
            let last_line_w = desc_wrapped.last().map(|s| s.width()).unwrap_or(0);
            let cx = inner_x as usize + label_w + last_line_w;
            queue!(
                stdout,
                MoveTo((cx as u16).min(cols.saturating_sub(1)), cy),
                Show
            )?;
        } else {
            let cx = inner_x as usize + label_w + app.edit_buf.width();
            queue!(
                stdout,
                MoveTo((cx as u16).min(cols.saturating_sub(1)), cy),
                Show
            )?;
        }
    } else {
        queue!(stdout, Hide)?;
    }

    Ok(())
}

fn progress_gauge(progress: Progress) -> String {
    let stage = progress.stage_index();
    let mut out = String::new();
    for i in 0..4 {
        if i <= stage {
            out.push('█');
        } else {
            out.push('░');
        }
    }
    out
}

fn progress_color(progress: Progress) -> Color {
    match progress {
        Progress::Done => Color::Green,
        Progress::InProgress => Color::Yellow,
        Progress::Todo => Color::Blue,
        Progress::Backlog => Color::DarkGrey,
    }
}

fn priority_color(priority: Priority) -> Color {
    match priority {
        Priority::Critical => Color::Red,
        Priority::High => Color::Yellow,
        Priority::Medium => Color::White,
        Priority::Low => Color::DarkGrey,
    }
}

fn wrap_text(text: &str, max_width: usize, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current_line = String::new();
    for word in text.split_whitespace() {
        if current_line.is_empty() {
            current_line = word.to_string();
        } else if current_line.width() + 1 + word.width() <= max_width {
            current_line.push(' ');
            current_line.push_str(word);
        } else {
            lines.push(current_line);
            if lines.len() >= max_lines {
                // Append ellipsis to last line if there's more text.
                if let Some(last) = lines.last_mut() {
                    if last.width() + 1 < max_width {
                        last.push('…');
                    }
                }
                return lines;
            }
            current_line = word.to_string();
        }
    }
    if !current_line.is_empty() {
        lines.push(current_line);
    }
    lines
}

fn clamp_text(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }

    if text.width() <= max_width {
        return text.to_string();
    }

    let ellipsis = "…";
    let max_content = max_width.saturating_sub(ellipsis.width());
    if max_content == 0 {
        return ellipsis.to_string();
    }

    let mut out = String::new();
    let mut w = 0usize;
    for ch in text.chars() {
        let ch_w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + ch_w > max_content {
            break;
        }
        out.push(ch);
        w += ch_w;
    }

    out.push_str(ellipsis);
    out
}

fn pad_to_width(text: &str, width: usize) -> String {
    let mut s = text.to_string();
    let current = s.as_str().width();
    if current < width {
        s.push_str(&" ".repeat(width - current));
    }
    s
}

fn choose_layout(total_width: usize, columns: usize) -> (usize, usize) {
    // Returns (x_margin, gap).
    // Tuned to add more breathing room on wider terminals, while degrading gracefully.
    match columns {
        4 => {
            let min_col = 16usize;
            let mut x_margin = if total_width >= 140 { 4 } else { 2 };
            let mut gap = if total_width >= 140 { 4 } else { 2 };

            loop {
                let content = total_width.saturating_sub(x_margin * 2);
                let col_width = content.saturating_sub(gap * 3) / 4;
                if col_width >= min_col || (x_margin <= 2 && gap <= 1) {
                    return (x_margin, gap);
                }

                if gap > 1 {
                    gap -= 1;
                    continue;
                }
                if x_margin > 2 {
                    x_margin -= 1;
                    continue;
                }
                return (x_margin, gap);
            }
        }
        _ => {
            let min_col = 18usize;
            let mut x_margin = if total_width >= 120 { 4 } else { 2 };
            let mut gap = if total_width >= 120 {
                6
            } else if total_width >= 90 {
                4
            } else {
                2
            };

            loop {
                let content = total_width.saturating_sub(x_margin * 2);
                let gaps = columns.saturating_sub(1);
                let col_width = content.saturating_sub(gap * gaps) / columns.max(1);
                if col_width >= min_col || (x_margin <= 2 && gap <= 2) {
                    return (x_margin, gap);
                }

                if gap > 2 {
                    gap = gap.saturating_sub(2).max(2);
                    continue;
                }
                if x_margin > 2 {
                    x_margin -= 1;
                    continue;
                }
                return (x_margin, gap);
            }
        }
    }
}

fn run_cli(instruction: &str) -> io::Result<()> {
    let storage = Storage::new();
    let mut tasks = match &storage {
        Some(s) => s.load_tasks().unwrap_or_default(),
        None => Vec::new(),
    };
    let settings = match &storage {
        Some(s) => s.load_settings().unwrap_or_default(),
        None => AiSettings::default(),
    };

    let ai = match llm::AiRuntime::from_settings(&settings) {
        Some(ai) => ai,
        None => {
            eprintln!(
                "Error: AI not configured. Set OPENAI_API_KEY or ANTHROPIC_API_KEY, or configure via the Settings tab."
            );
            std::process::exit(1);
        }
    };

    eprintln!("AI processing: \"{}\"", instruction);
    eprintln!();

    let context = build_ai_context(&tasks);
    let triage_ctx = build_triage_context(&tasks);
    ai.enqueue(llm::AiJob {
        task_id: Uuid::nil(),
        title: String::new(),
        suggested_bucket: default_bucket_name(&settings),
        context,
        bucket_names: bucket_names(&settings),
        lock_bucket: false,
        lock_priority: false,
        lock_due_date: false,
        edit_instruction: None,
        task_snapshot: None,
        triage_input: Some(instruction.to_string()),
        triage_context: Some(triage_ctx),
        chat_history: Vec::new(),
    });

    let mut pending = 1u32;
    let timeout = std::time::Duration::from_secs(90);
    let mut total_changes = 0u32;
    let mut saved = false;

    while pending > 0 {
        let result = match ai.recv_blocking(timeout) {
            Some(r) => r,
            None => {
                eprintln!("Timeout waiting for AI response.");
                break;
            }
        };
        pending -= 1;

        if let Some(err) = &result.error {
            eprintln!("  Error: {}", err);
            continue;
        }

        if let Some(action) = result.triage_action.clone() {
            match action {
                llm::TriageAction::Create => {
                    let now = Utc::now();
                    let title = result
                        .update
                        .title
                        .as_deref()
                        .unwrap_or("Untitled")
                        .to_string();
                    let bucket = result
                        .update
                        .bucket
                        .clone()
                        .unwrap_or_else(|| default_bucket_name(&settings));
                    let mut task = Task::new(bucket.clone(), title, now);
                    if let Some(desc) = &result.update.description {
                        task.description = desc.clone();
                    }
                    if let Some(progress) = result.update.progress {
                        task.set_progress(progress, now);
                    }
                    if let Some(priority) = result.update.priority {
                        task.priority = priority;
                    }
                    if let Some(due_date) = result.update.due_date {
                        task.due_date = Some(due_date);
                    }
                    if !result.update.dependencies.is_empty() {
                        task.dependencies = resolve_dependency_prefixes(
                            &tasks,
                            task.id,
                            &result.update.dependencies,
                        );
                    }
                    let parent_id = task.id;
                    println!("  + Created \"{}\" [{}]", task.title, task.bucket);
                    tasks.push(task);
                    total_changes += 1;
                    if !result.sub_task_specs.is_empty() {
                        let count = result.sub_task_specs.len();
                        let mut new_ids: Vec<Uuid> = Vec::with_capacity(count);
                        for spec in result.sub_task_specs.iter() {
                            let sub_bucket = spec.bucket.clone().unwrap_or_else(|| bucket.clone());
                            let mut sub = Task::new(sub_bucket, spec.title.clone(), now);
                            sub.parent_id = Some(parent_id);
                            sub.description = spec.description.clone();
                            if let Some(p) = spec.priority {
                                sub.priority = p;
                            }
                            if let Some(prog) = spec.progress {
                                sub.set_progress(prog, now);
                            }
                            if let Some(due) = spec.due_date {
                                sub.due_date = Some(due);
                            }
                            new_ids.push(sub.id);
                            println!("    ↳ Created sub-task \"{}\"", sub.title);
                            tasks.push(sub);
                        }
                        for (i, spec) in result.sub_task_specs.iter().enumerate() {
                            if spec.depends_on.is_empty() {
                                continue;
                            }
                            let task_id = new_ids[i];
                            let dep_ids: Vec<Uuid> = spec
                                .depends_on
                                .iter()
                                .filter_map(|&idx| new_ids.get(idx).copied())
                                .filter(|&dep_id| dep_id != task_id)
                                .collect();
                            if let Some(t) = tasks.iter_mut().find(|t| t.id == task_id) {
                                t.dependencies = dep_ids;
                            }
                        }
                        total_changes += count as u32;
                    }
                }
                llm::TriageAction::Update(prefix) => {
                    let target_id = tasks.iter().find_map(|t| {
                        let short = t.id.to_string().chars().take(8).collect::<String>();
                        if short.eq_ignore_ascii_case(&prefix) {
                            Some(t.id)
                        } else {
                            None
                        }
                    });
                    if let Some(id) = target_id {
                        let deps = if !result.update.dependencies.is_empty() {
                            resolve_dependency_prefixes(&tasks, id, &result.update.dependencies)
                        } else {
                            Vec::new()
                        };
                        if let Some(task) = tasks.iter_mut().find(|t| t.id == id) {
                            let now = Utc::now();
                            apply_update(task, &result.update, &deps, now);
                            println!("  ~ Updated \"{}\"", task.title);
                            total_changes += 1;
                        }
                        // Create sub-tasks if the update response includes them.
                        if !result.sub_task_specs.is_empty() {
                            let now = Utc::now();
                            let parent_bucket = tasks
                                .iter()
                                .find(|t| t.id == id)
                                .map(|t| t.bucket.clone())
                                .unwrap_or_else(|| default_bucket_name(&settings));
                            let count = result.sub_task_specs.len();
                            let mut new_ids: Vec<Uuid> = Vec::with_capacity(count);
                            for spec in result.sub_task_specs.iter() {
                                let bucket =
                                    spec.bucket.clone().unwrap_or_else(|| parent_bucket.clone());
                                let mut task = Task::new(bucket, spec.title.clone(), now);
                                task.parent_id = Some(id);
                                task.description = spec.description.clone();
                                if let Some(p) = spec.priority {
                                    task.priority = p;
                                }
                                if let Some(prog) = spec.progress {
                                    task.set_progress(prog, now);
                                }
                                if let Some(due) = spec.due_date {
                                    task.due_date = Some(due);
                                }
                                new_ids.push(task.id);
                                println!("    ↳ Created sub-task \"{}\"", task.title);
                                tasks.push(task);
                            }
                            for (i, spec) in result.sub_task_specs.iter().enumerate() {
                                if spec.depends_on.is_empty() {
                                    continue;
                                }
                                let task_id = new_ids[i];
                                let dep_ids: Vec<Uuid> = spec
                                    .depends_on
                                    .iter()
                                    .filter_map(|&idx| new_ids.get(idx).copied())
                                    .filter(|&dep_id| dep_id != task_id)
                                    .collect();
                                if let Some(task) = tasks.iter_mut().find(|t| t.id == task_id) {
                                    task.dependencies = dep_ids;
                                }
                            }
                            total_changes += count as u32;
                        }
                    } else {
                        eprintln!("  Warning: task {} not found", prefix);
                    }
                }
                llm::TriageAction::Delete(prefix) => {
                    let target = tasks.iter().position(|t| {
                        let short = t.id.to_string().chars().take(8).collect::<String>();
                        short.eq_ignore_ascii_case(&prefix)
                    });
                    if let Some(pos) = target {
                        let title = tasks[pos].title.clone();
                        let id = tasks[pos].id;
                        let child_ids: Vec<Uuid> = children_of(&tasks, id)
                            .iter()
                            .map(|&i| tasks[i].id)
                            .collect();
                        tasks.retain(|t| !child_ids.contains(&t.id) && t.id != id);
                        println!("  - Deleted \"{}\"", title);
                        total_changes += 1;
                    } else {
                        eprintln!("  Warning: task {} not found", prefix);
                    }
                }
                llm::TriageAction::Decompose { target_id, specs } => {
                    let now = Utc::now();
                    let parent_uuid = target_id.as_ref().and_then(|prefix| {
                        tasks.iter().find_map(|t| {
                            let short = t.id.to_string().chars().take(8).collect::<String>();
                            if short.eq_ignore_ascii_case(prefix) {
                                Some(t.id)
                            } else {
                                None
                            }
                        })
                    });
                    let parent_title = parent_uuid
                        .and_then(|id| tasks.iter().find(|t| t.id == id))
                        .map(|t| t.title.clone())
                        .unwrap_or_else(|| "(no parent)".to_string());
                    let default_bucket = specs
                        .first()
                        .and_then(|s| s.bucket.clone())
                        .unwrap_or_else(|| default_bucket_name(&settings));
                    let count = specs.len();
                    let mut new_ids: Vec<Uuid> = Vec::with_capacity(count);
                    for spec in specs.iter() {
                        let bucket = spec
                            .bucket
                            .clone()
                            .unwrap_or_else(|| default_bucket.clone());
                        let mut task = Task::new(bucket, spec.title.clone(), now);
                        task.parent_id = parent_uuid;
                        task.description = spec.description.clone();
                        if let Some(p) = spec.priority {
                            task.priority = p;
                        }
                        if let Some(prog) = spec.progress {
                            task.set_progress(prog, now);
                        }
                        if let Some(due) = spec.due_date {
                            task.due_date = Some(due);
                        }
                        new_ids.push(task.id);
                        tasks.push(task);
                    }
                    for (i, spec) in specs.iter().enumerate() {
                        if spec.depends_on.is_empty() {
                            continue;
                        }
                        let task_id = new_ids[i];
                        let dep_ids: Vec<Uuid> = spec
                            .depends_on
                            .iter()
                            .filter_map(|&idx| new_ids.get(idx).copied())
                            .filter(|&dep_id| dep_id != task_id)
                            .collect();
                        if let Some(task) = tasks.iter_mut().find(|t| t.id == task_id) {
                            task.dependencies = dep_ids;
                        }
                    }
                    println!(
                        "  ◆ Decomposed \"{}\" into {} sub-task{}:",
                        parent_title,
                        count,
                        if count == 1 { "" } else { "s" }
                    );
                    for (i, spec) in specs.iter().enumerate() {
                        let deps_str = if spec.depends_on.is_empty() {
                            String::new()
                        } else {
                            let labels: Vec<String> = spec
                                .depends_on
                                .iter()
                                .map(|&idx| format!("{}", idx + 1))
                                .collect();
                            format!(" (after: {})", labels.join(", "))
                        };
                        println!("    {}. {}{}", i + 1, spec.title, deps_str);
                    }
                    total_changes += count as u32;
                }
                llm::TriageAction::BulkUpdate {
                    targets,
                    instruction,
                } => {
                    let task_ids: Vec<Uuid> =
                        if targets.len() == 1 && targets[0].eq_ignore_ascii_case("all") {
                            tasks
                                .iter()
                                .filter(|t| t.parent_id.is_none())
                                .map(|t| t.id)
                                .collect()
                        } else {
                            targets
                                .iter()
                                .filter_map(|prefix| {
                                    tasks.iter().find_map(|t| {
                                        let short =
                                            t.id.to_string().chars().take(8).collect::<String>();
                                        if short.eq_ignore_ascii_case(prefix) {
                                            Some(t.id)
                                        } else {
                                            None
                                        }
                                    })
                                })
                                .collect()
                        };

                    if task_ids.is_empty() {
                        eprintln!("  Warning: no matching tasks found");
                    } else {
                        let context = build_ai_context(&tasks);
                        for &tid in &task_ids {
                            if let Some(task) = tasks.iter().find(|t| t.id == tid) {
                                let snapshot = format_task_snapshot(task);
                                ai.enqueue(llm::AiJob {
                                    task_id: tid,
                                    title: task.title.clone(),
                                    suggested_bucket: task.bucket.clone(),
                                    context: context.clone(),
                                    bucket_names: bucket_names(&settings),
                                    lock_bucket: false,
                                    lock_priority: false,
                                    lock_due_date: false,
                                    edit_instruction: Some(instruction.clone()),
                                    task_snapshot: Some(snapshot),
                                    triage_input: None,
                                    triage_context: None,
                                    chat_history: Vec::new(),
                                });
                            }
                        }
                        eprintln!(
                            "  Updating {} task{}…",
                            task_ids.len(),
                            if task_ids.len() == 1 { "" } else { "s" }
                        );
                        pending += task_ids.len() as u32;
                    }
                }
            }
        } else {
            // Non-triage result: edit response (from BulkUpdate fan-out).
            let parent_id = result.task_id;
            let task_title = tasks
                .iter()
                .find(|t| t.id == parent_id)
                .map(|t| t.title.clone())
                .unwrap_or_else(|| "Unknown".to_string());

            let deps = if !result.update.dependencies.is_empty() {
                resolve_dependency_prefixes(&tasks, parent_id, &result.update.dependencies)
            } else {
                Vec::new()
            };

            if let Some(task) = tasks.iter_mut().find(|t| t.id == parent_id) {
                let now = Utc::now();
                apply_update(task, &result.update, &deps, now);
            }

            if !result.sub_task_specs.is_empty() {
                let now = Utc::now();
                let parent_bucket = tasks
                    .iter()
                    .find(|t| t.id == parent_id)
                    .map(|t| t.bucket.clone())
                    .unwrap_or_else(|| default_bucket_name(&settings));
                let count = result.sub_task_specs.len();
                let mut new_ids: Vec<Uuid> = Vec::with_capacity(count);

                for spec in result.sub_task_specs.iter() {
                    let bucket = spec.bucket.clone().unwrap_or_else(|| parent_bucket.clone());
                    let mut task = Task::new(bucket, spec.title.clone(), now);
                    task.parent_id = Some(parent_id);
                    task.description = spec.description.clone();
                    if let Some(p) = spec.priority {
                        task.priority = p;
                    }
                    if let Some(prog) = spec.progress {
                        task.set_progress(prog, now);
                    }
                    if let Some(due) = spec.due_date {
                        task.due_date = Some(due);
                    }
                    new_ids.push(task.id);
                    tasks.push(task);
                }

                for (i, spec) in result.sub_task_specs.iter().enumerate() {
                    if spec.depends_on.is_empty() {
                        continue;
                    }
                    let task_id = new_ids[i];
                    let dep_ids: Vec<Uuid> = spec
                        .depends_on
                        .iter()
                        .filter_map(|&idx| new_ids.get(idx).copied())
                        .filter(|&dep_id| dep_id != task_id)
                        .collect();
                    if let Some(task) = tasks.iter_mut().find(|t| t.id == task_id) {
                        task.dependencies = dep_ids;
                    }
                }

                println!(
                    "  ◆ \"{}\" → {} sub-task{}:",
                    task_title,
                    count,
                    if count == 1 { "" } else { "s" }
                );
                for (i, spec) in result.sub_task_specs.iter().enumerate() {
                    let deps_str = if spec.depends_on.is_empty() {
                        String::new()
                    } else {
                        let labels: Vec<String> = spec
                            .depends_on
                            .iter()
                            .map(|&idx| format!("{}", idx + 1))
                            .collect();
                        format!(" (after: {})", labels.join(", "))
                    };
                    println!("    {}. {}{}", i + 1, spec.title, deps_str);
                }
                total_changes += count as u32;
            } else {
                println!("  ~ Updated \"{}\"", task_title);
                total_changes += 1;
            }
        }
    }

    // Persist.
    if total_changes > 0 {
        if let Some(s) = &storage {
            if let Err(err) = s.save_tasks(&tasks) {
                eprintln!("Save failed: {err}");
            } else {
                saved = true;
            }
        }
    }

    eprintln!();
    if total_changes == 0 {
        eprintln!("No changes.");
    } else {
        eprintln!(
            "Done. {} change{} applied{}.",
            total_changes,
            if total_changes == 1 { "" } else { "s" },
            if saved { " and saved" } else { "" }
        );
    }

    Ok(())
}

fn print_help() {
    println!("aipm - AI-powered project manager");
    println!();
    println!("Usage:");
    println!("  aipm                            Open the interactive TUI");
    println!("  aipm \"<instruction>\"             Run AI instruction headlessly (no TUI)");
    println!("  aipm task <command>              Task CRUD (see below)");
    println!("  aipm bucket <command>            Bucket CRUD (see below)");
    println!("  aipm --help");
    println!("  aipm --version");
    println!();
    println!("Task commands (output JSON):");
    println!("  aipm task list                   List all tasks");
    println!("  aipm task show <id>              Show a single task");
    println!(
        "  aipm task add --title \"X\" [--bucket \"Y\"] [--priority low|medium|high|critical]"
    );
    println!("      [--progress backlog|todo|in-progress|done] [--due YYYY-MM-DD]");
    println!("      [--description \"...\"] [--parent <id>]");
    println!("  aipm task edit <id> [--title \"X\"] [--bucket \"Y\"] [--priority ...]");
    println!("      [--progress ...] [--due YYYY-MM-DD|none] [--description \"...\"]");
    println!("  aipm task delete <id>            Delete task and its sub-tasks");
    println!();
    println!("Bucket commands (output JSON):");
    println!("  aipm bucket list                 List all buckets");
    println!("  aipm bucket add <name> [--description \"...\"]");
    println!("  aipm bucket rename <old> <new>");
    println!("  aipm bucket delete <name>        Moves tasks to first remaining bucket");
    println!();
    println!("AI examples:");
    println!("  aipm \"break down all tickets into sub-issues\"");
    println!("  aipm \"mark the onboarding task as done\"");
    println!("  aipm \"create a task to set up CI/CD pipeline\"");
    println!();
    println!("TUI input (tab 1):");
    println!("  <text>                  (AI routes into your configured buckets)");
    println!("  @<id> <instruction>      (AI-edit a specific task by ID prefix)");
    println!("  /clear                   (clear AI conversation context)");
    println!("  /exit                    (quit the app)");
    println!();
    println!("Environment:");
    println!("  OPENAI_API_KEY=...                (for gpt-* models)");
    println!("  ANTHROPIC_API_KEY=...             (for claude-* models)");
    println!("  AIPM_MODEL=...                    (default: claude-sonnet-4-5)");
    println!("  AIPM_DATA_DIR=...                 (override data directory)");
}
