mod ai;
mod llm;
mod model;
mod storage;

use std::cmp::Ordering;
use std::io::{self, Stdout, Write};
use std::time::Duration;

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

use crate::model::{Bucket, Progress, Task};
use crate::storage::Storage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Default,
    Timeline,
    Kanban,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Board,
    Input,
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
    status: Option<String>,
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

    let mut app = App {
        storage,
        tasks,
        ai: llm::AiRuntime::from_env(),
        tab: Tab::Default,
        focus: Focus::Input,
        selected_bucket: Bucket::Team,
        selected_task_id: None,
        scroll_team: 0,
        scroll_john: 0,
        scroll_admin: 0,
        input: String::new(),
        status: None,
    };

    ensure_default_selection(&mut app);

    let mut stdout = io::stdout();
    let _guard = TerminalGuard::enter(&mut stdout)?;

    run_app(&mut stdout, &mut app)
}

fn run_app(stdout: &mut Stdout, app: &mut App) -> io::Result<()> {
    let mut needs_redraw = true;

    loop {
        if poll_ai(app) {
            needs_redraw = true;
        }

        if needs_redraw {
            render(stdout, app)?;
            needs_redraw = false;
        }

        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) => {
                    if handle_key(app, key)? {
                        break;
                    }
                    needs_redraw = true;
                }
                Event::Resize(_, _) => {
                    needs_redraw = true;
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    // Global quit
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc if app.focus == Focus::Board => return Ok(true),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(true),
        _ => {}
    }

    // Global tab switching
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
        _ => {}
    }

    match app.tab {
        Tab::Default => handle_default_tab_key(app, key),
        Tab::Timeline | Tab::Kanban => handle_readonly_tab_key(app, key),
    }
}

fn handle_readonly_tab_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => Ok(true),
        KeyCode::Char('i') => {
            app.tab = Tab::Default;
            app.focus = Focus::Input;
            Ok(false)
        }
        _ => Ok(false),
    }
}

fn handle_default_tab_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    match app.focus {
        Focus::Input => handle_input_key(app, key),
        Focus::Board => handle_board_key(app, key),
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
            let maybe = ai::infer_new_task(&app.input);
            if let Some(hints) = maybe {
                let lock_bucket = hints.bucket_locked;
                let lock_priority = hints.priority.is_some();
                let lock_due_date = hints.due_date.is_some();

                let now = Utc::now();
                let mut task = Task::new(hints.bucket, hints.title, now);
                if let Some(p) = hints.priority {
                    task.priority = p;
                }
                if let Some(d) = hints.due_date {
                    task.due_date = Some(d);
                }

                let context = build_ai_context(&app.tasks);
                let task_id = task.id;
                let task_title = task.title.clone();
                let suggested_bucket = task.bucket;

                app.tasks.push(task);
                app.input.clear();

                if let Some(ai) = &app.ai {
                    ai.enqueue(llm::AiJob {
                        task_id,
                        title: task_title,
                        suggested_bucket,
                        context,
                        lock_bucket,
                        lock_priority,
                        lock_due_date,
                    });
                    app.status = Some(format!(
                        "Created in {} • AI thinking…",
                        suggested_bucket.title()
                    ));
                } else {
                    app.status = Some(format!("Created in {}", suggested_bucket.title()));
                }

                ensure_default_selection(app);
                persist(app);
            }
            Ok(false)
        }
        KeyCode::Backspace => {
            app.input.pop();
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

fn handle_board_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => return Ok(true),
        KeyCode::Tab | KeyCode::Char('i') => {
            app.focus = Focus::Input;
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
        KeyCode::Down | KeyCode::Char('j') => move_selection(app, 1),
        KeyCode::Char('p') | KeyCode::Char(' ') => {
            if let Some(id) = app.selected_task_id {
                let now = Utc::now();
                if let Some(task) = app.tasks.iter_mut().find(|t| t.id == id) {
                    let from = task.progress;
                    task.advance_progress(now);
                    app.status = Some(format!(
                        "{}: {} → {}",
                        task.title,
                        from.title(),
                        task.progress.title()
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
                    app.status = Some(format!(
                        "{}: {} → {}",
                        task.title,
                        from.title(),
                        task.progress.title()
                    ));
                    persist(app);
                }
            }
        }
        _ => {}
    }

    Ok(false)
}

fn persist(app: &mut App) {
    let Some(storage) = &app.storage else {
        return;
    };
    if let Err(err) = storage.save_tasks(&app.tasks) {
        app.status = Some(format!("Save failed: {err}"));
    }
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
            app.status = Some(format!("AI error: {}", err));
            continue;
        }

        let deps = if !result.update.dependencies.is_empty() {
            resolve_dependency_prefixes(&app.tasks, result.task_id, &result.update.dependencies)
        } else {
            Vec::new()
        };

        let Some(task) = app.tasks.iter_mut().find(|t| t.id == result.task_id) else {
            continue;
        };

        let now = Utc::now();
        let mut task_changed = false;

        if let Some(bucket) = result.update.bucket {
            if task.bucket != bucket {
                task.bucket = bucket;
                task_changed = true;
            }
        }

        if let Some(desc) = result.update.description {
            if task.description.trim().is_empty() {
                task.description = desc;
                task_changed = true;
            }
        }

        if let Some(priority) = result.update.priority {
            if task.priority != priority {
                task.priority = priority;
                task_changed = true;
            }
        }

        if let Some(due_date) = result.update.due_date {
            if task.due_date != Some(due_date) {
                task.due_date = Some(due_date);
                task_changed = true;
            }
        }

        if !deps.is_empty() {
            if task.dependencies != deps {
                task.dependencies = deps;
                task_changed = true;
            }
        }

        if task_changed {
            task.updated_at = now;
            changed = true;
            app.status = Some(format!("AI updated: {}", task.title));
        }
    }

    if changed {
        ensure_default_selection(app);
        persist(app);
    }

    true
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
        .filter_map(|(idx, t)| if t.bucket == bucket { Some(idx) } else { None })
        .collect();

    indices.sort_by(|&a, &b| {
        let ta = &tasks[a];
        let tb = &tasks[b];
        tb.created_at.cmp(&ta.created_at)
    });

    indices
}

fn visible_cards(cards_area_height: usize) -> usize {
    const CARD_HEIGHT: usize = 7; // 6 lines + 1 spacer
    cards_area_height / CARD_HEIGHT
}

fn render(stdout: &mut Stdout, app: &mut App) -> io::Result<()> {
    let (cols, rows) = terminal::size()?;
    queue!(stdout, Clear(ClearType::All), MoveTo(0, 0))?;

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

    render_tabs(stdout, app)?;

    match app.tab {
        Tab::Default => render_default_tab(stdout, app, cols, rows)?,
        Tab::Timeline => render_timeline_tab(stdout, app, cols, rows)?,
        Tab::Kanban => render_kanban_tab(stdout, app, cols, rows)?,
    }

    stdout.flush()?;
    Ok(())
}

fn render_tabs(stdout: &mut Stdout, app: &App) -> io::Result<()> {
    let mut x: u16 = 2;
    for (tab, label) in [
        (Tab::Default, "1 Default"),
        (Tab::Timeline, "2 Timeline"),
        (Tab::Kanban, "3 Kanban"),
    ]
    .iter()
    {
        let is_active = *tab == app.tab;
        let rendered = format!("[{}]", label);
        queue!(stdout, MoveTo(x, 0))?;
        if is_active {
            queue!(
                stdout,
                SetAttribute(Attribute::Underlined),
                Print(&rendered),
                SetAttribute(Attribute::Reset)
            )?;
        } else {
            queue!(stdout, Print(&rendered))?;
        }
        x += rendered.len() as u16 + 1;
    }
    Ok(())
}

fn render_default_tab(stdout: &mut Stdout, app: &mut App, cols: u16, rows: u16) -> io::Result<()> {
    let width = cols as usize;
    let (x_margin, gap) = choose_layout(width, 3);

    // Top padding below the tabs row.
    let y_body_top = 2u16;
    let y_status = rows.saturating_sub(3);
    let y_input = rows.saturating_sub(2);
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

        let title = format!(" {}", bucket.title());
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
    let cards_area_height = y_input.saturating_sub(y_cards_start) as usize;
    let visible = visible_cards(cards_area_height).max(1);

    for (i, bucket) in Bucket::ALL.iter().enumerate() {
        render_bucket_column(
            stdout,
            app,
            *bucket,
            col_x[i] as u16,
            y_cards_start,
            col_width,
            visible,
        )?;
    }

    // Status
    if let Some(status) = &app.status {
        queue!(
            stdout,
            MoveTo(x_input, y_status),
            SetForegroundColor(Color::DarkGrey),
            Print(clamp_text(status, width.saturating_sub(x_margin * 2))),
            ResetColor
        )?;
    }

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
        Focus::Input => queue!(stdout, SetForegroundColor(Color::White))?,
        Focus::Board => queue!(stdout, SetForegroundColor(Color::DarkGrey))?,
    };
    queue!(stdout, Print(prompt))?;

    if shown.is_empty() {
        queue!(
            stdout,
            SetForegroundColor(Color::DarkGrey),
            Print(clamp_text(
                "type a task + enter (e.g. \"prepare onboarding\")",
                max_input
            )),
            ResetColor
        )?;
    } else {
        queue!(stdout, Print(&shown), ResetColor)?;
    }

    // Help
    queue!(
        stdout,
        MoveTo(x_input, y_help),
        SetForegroundColor(Color::DarkGrey),
        Print(
            "tab/i focus input • esc board • ↑/↓/←/→ (or hjkl) • p advance • P back • 1/2/3 tabs • q quit"
        ),
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
    visible: usize,
) -> io::Result<()> {
    const CARD_LINES: usize = 6;
    const CARD_HEIGHT: usize = 7;

    let indices = bucket_task_indices(&app.tasks, bucket);
    let scroll = match bucket {
        Bucket::Team => app.scroll_team,
        Bucket::John => app.scroll_john,
        Bucket::Admin => app.scroll_admin,
    };

    for (pos, &idx) in indices.iter().enumerate().skip(scroll).take(visible) {
        let task = &app.tasks[idx];
        let is_selected = app.focus == Focus::Board
            && bucket == app.selected_bucket
            && app.selected_task_id == Some(task.id);

        let card_top = y + ((pos - scroll) * CARD_HEIGHT) as u16;

        let deps = if task.dependencies.is_empty() {
            "none".to_string()
        } else {
            task.dependencies
                .iter()
                .take(3)
                .map(|id| id.to_string()[..8].to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };

        let gauge = progress_gauge(task.progress);
        let due = task
            .due_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "—".to_string());

        let lines = [
            format!("{}", task.title),
            format!(
                "Description: {}",
                if task.description.trim().is_empty() {
                    "—"
                } else {
                    task.description.trim()
                }
            ),
            format!("Dependencies: {}", deps),
            format!("Progress: {} {}", gauge, task.progress.title()),
            format!("Priority: {}", task.priority.title()),
            format!("Due: {}", due),
        ];

        for (line_idx, line) in lines.iter().enumerate().take(CARD_LINES) {
            let y_line = card_top + line_idx as u16;
            queue!(stdout, MoveTo(x, y_line))?;
            if is_selected {
                queue!(
                    stdout,
                    SetForegroundColor(Color::Black),
                    SetBackgroundColor(Color::White)
                )?;
            } else {
                queue!(stdout, SetForegroundColor(Color::White))?;
            }

            let line = format!(" {}", line);
            let padded = pad_to_width(&clamp_text(&line, width), width);
            queue!(stdout, Print(padded), ResetColor)?;
        }

        // Spacer line
        queue!(
            stdout,
            MoveTo(x, card_top + CARD_LINES as u16),
            Print(pad_to_width("", width))
        )?;
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

fn render_timeline_tab(stdout: &mut Stdout, app: &App, cols: u16, rows: u16) -> io::Result<()> {
    let width = cols as usize;
    let (x_margin, _gap) = choose_layout(width, 1);
    let x = x_margin as u16;
    let y_help = rows.saturating_sub(1);

    queue!(
        stdout,
        MoveTo(x, 2),
        SetForegroundColor(Color::DarkGrey),
        Print("Timeline (sorted by due date)."),
        ResetColor
    )?;

    let mut tasks: Vec<&Task> = app.tasks.iter().collect();
    tasks.sort_by(|a, b| match (a.due_date, b.due_date) {
        (Some(da), Some(db)) => da.cmp(&db),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => b.created_at.cmp(&a.created_at),
    });

    let list_top = 4u16;
    let list_height = rows.saturating_sub(list_top + 2) as usize;

    for (i, task) in tasks.iter().take(list_height).enumerate() {
        let due = task
            .due_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "—".to_string());
        let line = format!(
            "{}  {:<10}  {:<11}  {:<11}  {}",
            due,
            task.bucket.title(),
            task.progress.title(),
            task.priority.title(),
            task.title
        );
        queue!(
            stdout,
            MoveTo(x, list_top + i as u16),
            Print(clamp_text(&line, width.saturating_sub(x_margin * 2)))
        )?;
    }

    queue!(
        stdout,
        MoveTo(x, y_help),
        SetForegroundColor(Color::DarkGrey),
        Print("1 default • 3 kanban • q quit"),
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
        MoveTo(x, 2),
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
        queue!(
            stdout,
            MoveTo(x, 4),
            SetAttribute(Attribute::Bold),
            Print(clamp_text(stage.title(), col_width)),
            SetAttribute(Attribute::Reset)
        )?;

        let mut tasks: Vec<&Task> = app.tasks.iter().filter(|t| t.progress == *stage).collect();
        tasks.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        let list_top = 6u16;
        let list_height = rows.saturating_sub(list_top + 2) as usize;

        for (row, task) in tasks.iter().take(list_height).enumerate() {
            let line = format!("{} · {}", task.bucket.title(), task.title);
            queue!(
                stdout,
                MoveTo(x, list_top + row as u16),
                Print(clamp_text(&line, col_width))
            )?;
        }
    }

    queue!(
        stdout,
        MoveTo(x, y_help),
        SetForegroundColor(Color::DarkGrey),
        Print("1 default • 2 timeline • q quit"),
        ResetColor
    )?;

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

fn print_help() {
    println!("aipm - AI task manager (TUI)");
    println!();
    println!("Usage:");
    println!("  aipm");
    println!("  aipm --help");
    println!("  aipm --version");
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
    println!("  Optional OpenAI enrichment (async). Set OPENAI_API_KEY to enable.");
    println!("  AIPM_AI=off|auto|openai           (default: auto)");
    println!("  AIPM_OPENAI_MODEL=...             (default: gpt-4o-mini)");
    println!(
        "  AIPM_OPENAI_URL=...               (default: https://api.openai.com/v1/chat/completions)"
    );
    println!("  AIPM_OPENAI_TIMEOUT_SECS=30       (default: 30)");
    println!();
    println!("Data:");
    println!("  Stored at $AIPM_DATA_DIR/tasks.json if set, otherwise in your OS data dir.");
}
