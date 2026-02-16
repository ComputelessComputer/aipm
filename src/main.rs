mod ai;
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

use crate::model::{children_of, Bucket, Progress, Task};
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
enum SettingsField {
    OwnerName,
    AiEnabled,
    OpenAiKey,
    AnthropicKey,
    Model,
    ApiUrl,
    Timeout,
}

impl SettingsField {
    const ALL: [SettingsField; 7] = [
        SettingsField::OwnerName,
        SettingsField::AiEnabled,
        SettingsField::OpenAiKey,
        SettingsField::AnthropicKey,
        SettingsField::Model,
        SettingsField::ApiUrl,
        SettingsField::Timeout,
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
        }
    }
}

fn bucket_display_title(bucket: Bucket, owner_name: &str) -> String {
    match bucket {
        Bucket::Team => "Team".to_string(),
        Bucket::John => {
            let name = if owner_name.trim().is_empty() {
                "John"
            } else {
                owner_name.trim()
            };
            format!("{}-only", name)
        }
        Bucket::Admin => "Admin".to_string(),
    }
}

struct App {
    storage: Option<Storage>,
    tasks: Vec<Task>,
    ai: Option<llm::AiRuntime>,
    tab: Tab,
    focus: Focus,

    selected_bucket: Bucket,
    selected_task_id: Option<Uuid>,

    scroll_team: usize,
    scroll_john: usize,
    scroll_admin: usize,

    input: String,
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
    _kanban_scroll: [usize; 4],

    confirm_delete_id: Option<Uuid>,

    settings: AiSettings,
    settings_field: SettingsField,
    settings_buf: String,
    settings_editing: bool,
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

    let mut app = App {
        storage,
        tasks,
        ai: llm::AiRuntime::from_settings(&settings),
        tab: Tab::Default,
        focus: Focus::Input,
        selected_bucket: Bucket::Team,
        selected_task_id: None,
        scroll_team: 0,
        scroll_john: 0,
        scroll_admin: 0,
        input: String::new(),
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
        _kanban_scroll: [0; 4],
        confirm_delete_id: None,
        settings,
        settings_field: SettingsField::AiEnabled,
        settings_buf: String::new(),
        settings_editing: false,
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
            match event::read()? {
                Event::Key(key) => {
                    if handle_key(app, key)? {
                        break;
                    }
                    needs_redraw = true;
                    // Full clear when layout changes significantly.
                    if app.tab != prev_tab
                        || app.focus != prev_focus
                        || app.edit_task_id != prev_edit
                        || app.confirm_delete_id != prev_confirm
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

    // Global quit from board focus.
    if key.code == KeyCode::Char('q') && app.focus == Focus::Board {
        return Ok(true);
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
        KeyCode::Char('q') => return Ok(true),
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
        KeyCode::Char('q') => return Ok(true),
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

fn handle_input_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
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
        KeyCode::Enter => {
            if app.input.trim().eq_ignore_ascii_case("exit") {
                return Ok(true);
            }

            // Reload tasks from disk before processing input to pick up external changes.
            if let Some(storage) = &app.storage {
                if let Ok(fresh) = storage.reload_tasks() {
                    app.tasks = fresh;
                }
            }

            // @ prefix: edit the selected task via AI.
            if app.input.trim().starts_with('@') {
                let instruction = app
                    .input
                    .trim()
                    .strip_prefix('@')
                    .unwrap_or("")
                    .trim()
                    .to_string();
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
                        // Route through triage so decompose action can create real sub-tasks.
                        if let Some(ai) = &app.ai {
                            let context = build_ai_context(&app.tasks);
                            let triage_ctx = build_triage_context(&app.tasks);
                            ai.enqueue(llm::AiJob {
                                task_id: Uuid::nil(),
                                title: String::new(),
                                suggested_bucket: Bucket::Team,
                                context,
                                lock_bucket: false,
                                lock_priority: false,
                                lock_due_date: false,
                                edit_instruction: None,
                                task_snapshot: None,
                                triage_input: Some(instruction),
                                triage_context: Some(triage_ctx),
                            });
                            app.status =
                                Some(("AI decomposing…".to_string(), Instant::now(), true));
                        } else {
                            app.status =
                                Some(("AI not configured".to_string(), Instant::now(), false));
                        }
                    } else if let Some(task_id) = app.selected_task_id {
                        if let Some(task) = app.tasks.iter().find(|t| t.id == task_id) {
                            let snapshot = format_task_snapshot(task);
                            let context = build_ai_context(&app.tasks);
                            if let Some(ai) = &app.ai {
                                ai.enqueue(llm::AiJob {
                                    task_id,
                                    title: task.title.clone(),
                                    suggested_bucket: task.bucket,
                                    context,
                                    lock_bucket: false,
                                    lock_priority: false,
                                    lock_due_date: false,
                                    edit_instruction: Some(instruction),
                                    task_snapshot: Some(snapshot),
                                    triage_input: None,
                                    triage_context: None,
                                });
                                app.status = Some((
                                    format!("AI editing: {}…", task.title),
                                    Instant::now(),
                                    true,
                                ));
                            } else {
                                app.status =
                                    Some(("AI not configured".to_string(), Instant::now(), false));
                            }
                        }
                    } else {
                        app.status = Some(("No task selected".to_string(), Instant::now(), false));
                    }
                }
                app.input.clear();
                return Ok(false);
            }

            let raw_input = app.input.trim().to_string();
            if raw_input.is_empty() {
                return Ok(false);
            }
            app.input.clear();

            // AI triage: let the AI decide create vs update.
            if let Some(ai) = &app.ai {
                let context = build_ai_context(&app.tasks);
                let triage_ctx = build_triage_context(&app.tasks);
                ai.enqueue(llm::AiJob {
                    task_id: Uuid::nil(),
                    title: String::new(),
                    suggested_bucket: Bucket::Team,
                    context,
                    lock_bucket: false,
                    lock_priority: false,
                    lock_due_date: false,
                    edit_instruction: None,
                    task_snapshot: None,
                    triage_input: Some(raw_input),
                    triage_context: Some(triage_ctx),
                });
                app.status = Some(("AI thinking…".to_string(), Instant::now(), true));
            } else {
                // Fallback: local inference when AI is not configured.
                let maybe = ai::infer_new_task(&raw_input);
                if let Some(hints) = maybe {
                    let now = Utc::now();
                    let mut task = Task::new(hints.bucket, hints.title, now);
                    if let Some(p) = hints.priority {
                        task.priority = p;
                    }
                    if let Some(d) = hints.due_date {
                        task.due_date = Some(d);
                    }
                    app.tasks.push(task);
                    app.status = Some((
                        format!(
                            "Created in {}",
                            bucket_display_title(hints.bucket, &app.settings.owner_name)
                        ),
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
                // Option+Backspace: delete last word.
                let trimmed = app.input.trim_end().len();
                app.input.truncate(trimmed);
                while !app.input.is_empty() && !app.input.ends_with(' ') {
                    app.input.pop();
                }
            } else {
                app.input.pop();
            }
            Ok(false)
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Ctrl+U: clear entire input.
            app.input.clear();
            Ok(false)
        }
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Ctrl+W: delete last word.
            let trimmed = app.input.trim_end().len();
            app.input.truncate(trimmed);
            while !app.input.is_empty() && !app.input.ends_with(' ') {
                app.input.pop();
            }
            Ok(false)
        }
        KeyCode::Char(ch) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                return Ok(false);
            }
            app.input.push(ch);
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
    match key.code {
        KeyCode::Char('q') => return Ok(true),
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
            app.selected_bucket = match app.selected_bucket {
                Bucket::Team => Bucket::Admin,
                Bucket::John => Bucket::Team,
                Bucket::Admin => Bucket::John,
            };
            ensure_default_selection(app);
        }
        KeyCode::Right | KeyCode::Char('l') => {
            app.selected_bucket = match app.selected_bucket {
                Bucket::Team => Bucket::John,
                Bucket::John => Bucket::Admin,
                Bucket::Admin => Bucket::Team,
            };
            ensure_default_selection(app);
        }
        KeyCode::Up | KeyCode::Char('k') => move_selection(app, -1),
        KeyCode::Down | KeyCode::Char('j') => {
            let bucket_tasks = bucket_task_indices(&app.tasks, app.selected_bucket);
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
        EditField::Bucket => bucket_display_title(task.bucket, &app.settings.owner_name),
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
            if let Some(b) = match app.edit_buf.trim().to_ascii_lowercase().as_str() {
                "team" => Some(Bucket::Team),
                "john" | "john-only" => Some(Bucket::John),
                "admin" => Some(Bucket::Admin),
                _ => None,
            } {
                task.bucket = b;
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
            task.bucket = if forward {
                match task.bucket {
                    Bucket::Team => Bucket::John,
                    Bucket::John => Bucket::Admin,
                    Bucket::Admin => Bucket::Team,
                }
            } else {
                match task.bucket {
                    Bucket::Team => Bucket::Admin,
                    Bucket::John => Bucket::Team,
                    Bucket::Admin => Bucket::John,
                }
            };
            task.updated_at = now;
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
                app.edit_buf.pop();
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
        KeyCode::Esc | KeyCode::Char('q') => {
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
                        .map(|t| t.bucket)
                        .unwrap_or(Bucket::Team);
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
        KeyCode::Char('q') => return Ok(true),
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
        KeyCode::Char('q') => return Ok(true),
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
        },
        KeyCode::Left => match app.settings_field {
            SettingsField::AiEnabled => {
                app.settings.enabled = !app.settings.enabled;
                persist_settings(app);
                rebuild_ai(app);
            }
            SettingsField::Model => {
                cycle_model(app, false);
            }
            _ => {}
        },
        KeyCode::Right => match app.settings_field {
            SettingsField::AiEnabled => {
                app.settings.enabled = !app.settings.enabled;
                persist_settings(app);
                rebuild_ai(app);
            }
            SettingsField::Model => {
                cycle_model(app, true);
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
            app.settings_editing = false;
            persist_settings(app);
            rebuild_ai(app);
        }
        KeyCode::Backspace => {
            app.settings_buf.pop();
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
                    let bucket = result.update.bucket.unwrap_or(Bucket::Team);
                    let mut task = Task::new(bucket, title, now);
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
                    app.status =
                        Some((format!("AI created: {}", task.title), Instant::now(), false));
                    app.tasks.push(task);
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
                                .map(|t| t.bucket)
                                .unwrap_or(Bucket::Team);
                            let count = result.sub_task_specs.len();
                            let mut new_ids: Vec<Uuid> = Vec::with_capacity(count);
                            for spec in result.sub_task_specs.iter() {
                                let bucket = spec.bucket.unwrap_or(parent_bucket);
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
                    let default_bucket =
                        specs.first().and_then(|s| s.bucket).unwrap_or(Bucket::Team);
                    for spec in specs.iter() {
                        let bucket = spec.bucket.unwrap_or(default_bucket);
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
                                    suggested_bucket: task.bucket,
                                    context: context.clone(),
                                    lock_bucket: false,
                                    lock_priority: false,
                                    lock_due_date: false,
                                    edit_instruction: Some(instruction.clone()),
                                    task_snapshot: Some(snapshot),
                                    triage_input: None,
                                    triage_context: None,
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
                .map(|t| t.bucket)
                .unwrap_or(Bucket::Team);
            let count = result.sub_task_specs.len();
            let mut new_ids: Vec<Uuid> = Vec::with_capacity(count);

            for spec in result.sub_task_specs.iter() {
                let bucket = spec.bucket.unwrap_or(parent_bucket);
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

    if let Some(bucket) = update.bucket {
        if task.bucket != bucket {
            task.bucket = bucket;
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
            bucket: t.bucket,
            title: t.title.clone(),
        })
        .collect()
}

/// Build rich context for triage: full task details so the AI can match intent.
fn build_triage_context(tasks: &[Task]) -> String {
    let mut refs: Vec<&Task> = tasks.iter().collect();
    refs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    let mut out = String::new();
    for t in refs.iter().take(40) {
        let short = t.id.to_string().chars().take(8).collect::<String>();
        let desc = if t.description.trim().is_empty() {
            ""
        } else {
            t.description.trim()
        };
        out.push_str(&format!(
            "- {} [{}] {} | {} | {} | {}\n",
            short,
            t.bucket.title(),
            t.title,
            t.progress.title(),
            t.priority.title(),
            if desc.is_empty() {
                "no description"
            } else {
                desc
            }
        ));
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
        task.bucket.title(),
        if task.description.trim().is_empty() { "none" } else { task.description.trim() },
        task.progress.title(),
        task.priority.title(),
        due,
        deps
    )
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
    let bucket_tasks = bucket_task_indices(&app.tasks, app.selected_bucket);
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
    let bucket_tasks = bucket_task_indices(&app.tasks, app.selected_bucket);
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

    let selected_index = app
        .selected_task_id
        .and_then(|id| {
            bucket_task_indices(&app.tasks, app.selected_bucket)
                .iter()
                .position(|&idx| app.tasks[idx].id == id)
        })
        .unwrap_or(0);

    let scroll = match app.selected_bucket {
        Bucket::Team => &mut app.scroll_team,
        Bucket::John => &mut app.scroll_john,
        Bucket::Admin => &mut app.scroll_admin,
    };

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

fn bucket_task_indices(tasks: &[Task], bucket: Bucket) -> Vec<usize> {
    let mut indices: Vec<usize> = tasks
        .iter()
        .enumerate()
        .filter_map(|(idx, t)| {
            if t.bucket == bucket && t.parent_id.is_none() {
                Some(idx)
            } else {
                None
            }
        })
        .collect();

    indices.sort_by(|&a, &b| {
        let ta = &tasks[a];
        let tb = &tasks[b];
        tb.created_at.cmp(&ta.created_at)
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
    let (x_margin, _) = choose_layout(width, 3);
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
    let (x_margin, gap) = choose_layout(width, 3);

    // Top padding below the tabs row.
    let y_body_top = 3u16;
    let y_status = rows.saturating_sub(5);
    let y_sep_top = rows.saturating_sub(4);
    let y_input = rows.saturating_sub(3);
    let y_sep_bottom = rows.saturating_sub(2);
    let y_help = rows.saturating_sub(1);

    let x_input = x_margin as u16;

    let content_width = width.saturating_sub(x_margin * 2);
    let col_width = content_width.saturating_sub(gap * 2) / 3;

    let col_x = [
        x_margin,
        x_margin + col_width + gap,
        x_margin + 2 * (col_width + gap),
    ];

    for (i, bucket) in Bucket::ALL.iter().enumerate() {
        let x = col_x[i] as u16;

        let title = format!(
            " {}",
            bucket_display_title(*bucket, &app.settings.owner_name)
        );
        queue!(
            stdout,
            MoveTo(x, y_body_top),
            SetAttribute(Attribute::Bold),
            Print(clamp_text(&title, col_width)),
            SetAttribute(Attribute::Reset)
        )?;

        let desc = format!(" {}", bucket.description());
        queue!(
            stdout,
            MoveTo(x, y_body_top + 1),
            SetForegroundColor(Color::DarkGrey),
            Print(clamp_text(&desc, col_width)),
            ResetColor
        )?;
    }

    let y_cards_start = y_body_top + 3;

    for (i, bucket) in Bucket::ALL.iter().enumerate() {
        render_bucket_column(
            stdout,
            app,
            *bucket,
            col_x[i] as u16,
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
    let shown = if app.input.is_empty() {
        String::new()
    } else {
        clamp_text(&app.input, max_input)
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
                    "type a task + enter (e.g. \"prepare onboarding\")",
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
            "tab/i input • esc board • ↑/↓/←/→ navigate • p advance • 1/2/3 tabs • exit quits",
            content_width,
        )),
        ResetColor
    )?;

    // Cursor
    if app.focus == Focus::Input {
        let cursor_x = x_input as usize + prompt.width() + shown.width();
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
    bucket: Bucket,
    x: u16,
    y: u16,
    width: usize,
    max_y: u16,
) -> io::Result<()> {
    const CARD_LINES: usize = 6; // lines 0-5 (title, desc×2, separator, progress, due)

    let indices = bucket_task_indices(&app.tasks, bucket);
    let scroll = match bucket {
        Bucket::Team => app.scroll_team,
        Bucket::John => app.scroll_john,
        Bucket::Admin => app.scroll_admin,
    };

    let inner_w = width.saturating_sub(2); // 1 char padding each side
    let mut y_cursor = y;

    for (_pos, &idx) in indices.iter().enumerate().skip(scroll) {
        if y_cursor + CARD_LINES as u16 + 1 > max_y {
            break;
        }

        let task = &app.tasks[idx];
        let is_selected = app.focus == Focus::Board
            && bucket == app.selected_bucket
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

            let content = match line_idx {
                0 => {
                    // Title: bright default foreground + bold.
                    if !is_selected {
                        queue!(stdout, ResetColor, SetAttribute(Attribute::Bold))?;
                    } else {
                        queue!(stdout, SetAttribute(Attribute::Bold))?;
                    }
                    format!(" {}", task.title)
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
            queue!(
                stdout,
                MoveTo(x, y_cursor),
                Print(pad_to_width("", width))
            )?;
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
        let prefix = if task.is_child() {
            "↳"
        } else {
            match task.bucket {
                Bucket::Team => "●",
                Bucket::John => "◆",
                Bucket::Admin => "■",
            }
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

fn render_kanban_tab(stdout: &mut Stdout, app: &App, cols: u16, rows: u16) -> io::Result<()> {
    let width = cols as usize;
    let (x_margin, gap) = choose_layout(width, 4);
    let x = x_margin as u16;

    let y_help = rows.saturating_sub(1);

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

    for (i, stage) in Progress::ALL.iter().enumerate() {
        let x = col_x[i] as u16;
        let is_active_col = *stage == app.kanban_stage;
        queue!(stdout, MoveTo(x, 5))?;
        if is_active_col {
            queue!(
                stdout,
                SetForegroundColor(progress_color(*stage)),
                SetAttribute(Attribute::Bold),
                SetAttribute(Attribute::Underlined),
                Print(clamp_text(stage.title(), col_width)),
                SetAttribute(Attribute::Reset),
                ResetColor
            )?;
        } else {
            queue!(
                stdout,
                SetForegroundColor(progress_color(*stage)),
                SetAttribute(Attribute::Bold),
                Print(clamp_text(stage.title(), col_width)),
                SetAttribute(Attribute::Reset),
                ResetColor
            )?;
        }

        let ids = kanban_task_ids(&app.tasks, *stage);

        let list_top = 7u16;
        let list_height = rows.saturating_sub(list_top + 2) as usize;

        for (row, id) in ids.iter().take(list_height).enumerate() {
            let task = app.tasks.iter().find(|t| t.id == *id).unwrap();
            let is_selected = is_active_col && app.kanban_selected == Some(*id);
            let line = if task.is_child() {
                format!(" ↳ {}", task.title)
            } else {
                format!(
                    " {} · {}",
                    bucket_display_title(task.bucket, &app.settings.owner_name),
                    task.title
                )
            };
            queue!(stdout, MoveTo(x, list_top + row as u16))?;
            if is_selected {
                queue!(
                    stdout,
                    SetForegroundColor(Color::Black),
                    SetBackgroundColor(Color::White),
                    Print(pad_to_width(&clamp_text(&line, col_width), col_width)),
                    ResetColor
                )?;
            } else {
                queue!(stdout, Print(clamp_text(&line, col_width)))?;
            }
        }
    }

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
        };

        let show_value = if is_current && app.settings_editing {
            format!("{}\u{258f}", app.settings_buf)
        } else if is_current && matches!(field, SettingsField::AiEnabled | SettingsField::Model) {
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
            EditField::Bucket => bucket_display_title(task.bucket, &app.settings.owner_name),
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
                let col_width = content.saturating_sub(gap * 2) / 3;
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
        suggested_bucket: Bucket::Team,
        context,
        lock_bucket: false,
        lock_priority: false,
        lock_due_date: false,
        edit_instruction: None,
        task_snapshot: None,
        triage_input: Some(instruction.to_string()),
        triage_context: Some(triage_ctx),
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
                    let bucket = result.update.bucket.unwrap_or(Bucket::Team);
                    let mut task = Task::new(bucket, title, now);
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
                    println!(
                        "  + Created \"{}\" [{}]",
                        task.title,
                        bucket_display_title(task.bucket, &settings.owner_name)
                    );
                    tasks.push(task);
                    total_changes += 1;
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
                                .map(|t| t.bucket)
                                .unwrap_or(Bucket::Team);
                            let count = result.sub_task_specs.len();
                            let mut new_ids: Vec<Uuid> = Vec::with_capacity(count);
                            for spec in result.sub_task_specs.iter() {
                                let bucket = spec.bucket.unwrap_or(parent_bucket);
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
                    let default_bucket =
                        specs.first().and_then(|s| s.bucket).unwrap_or(Bucket::Team);
                    let count = specs.len();
                    let mut new_ids: Vec<Uuid> = Vec::with_capacity(count);
                    for spec in specs.iter() {
                        let bucket = spec.bucket.unwrap_or(default_bucket);
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
                                    suggested_bucket: task.bucket,
                                    context: context.clone(),
                                    lock_bucket: false,
                                    lock_priority: false,
                                    lock_due_date: false,
                                    edit_instruction: Some(instruction.clone()),
                                    task_snapshot: Some(snapshot),
                                    triage_input: None,
                                    triage_context: None,
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
                    .map(|t| t.bucket)
                    .unwrap_or(Bucket::Team);
                let count = result.sub_task_specs.len();
                let mut new_ids: Vec<Uuid> = Vec::with_capacity(count);

                for spec in result.sub_task_specs.iter() {
                    let bucket = spec.bucket.unwrap_or(parent_bucket);
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
    println!("  aipm --help");
    println!("  aipm --version");
    println!();
    println!("CLI examples:");
    println!("  aipm \"break down all tickets into sub-issues\"");
    println!("  aipm \"mark the onboarding task as done\"");
    println!("  aipm \"create a task to set up CI/CD pipeline\"");
    println!();
    println!("New task input (tab 1):");
    println!("  <text>                  (AI routes into Team/John/Admin)");
    println!("  team: <text>             (force Team bucket)");
    println!("  john: <text>             (force John-only bucket)");
    println!("  admin: <text>            (force Admin bucket)");
    println!("  due:YYYY-MM-DD           (set due date, e.g. due:2026-02-20)");
    println!("  p:low|medium|high|critical (set priority, e.g. p:high)");
    println!();
    println!("AI:");
    println!("  Supports OpenAI and Anthropic models. Set the appropriate API key to enable.");
    println!("  OPENAI_API_KEY=...                (for gpt-* models)");
    println!("  ANTHROPIC_API_KEY=...             (for claude-* models)");
    println!("  AIPM_MODEL=...                    (default: gpt-5.2-chat-latest)");
    println!("  AIPM_API_URL=...                  (auto-detected from model)");
    println!();
    println!("Data:");
    println!("  Stored at $AIPM_DATA_DIR/tasks.json if set, otherwise in your OS data dir.");
}
