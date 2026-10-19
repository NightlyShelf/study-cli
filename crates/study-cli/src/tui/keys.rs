//! Key handling: one function per mode (SPEC §4.4).

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use study_core::index::NodeKind;

use crate::app;
use crate::tui::calendar;
use crate::tui::state::{App, Mode, Pane, Tab};
use crate::tui::tasks;
use crate::tui::tree::{self, RowKind};

pub fn handle(app: &mut App, key: KeyEvent) {
    // Windows terminals also report key releases; only presses count.
    if key.kind == KeyEventKind::Release {
        return;
    }
    match app.mode {
        Mode::Browse => browse(app, key.code),
        Mode::Filter => filter(app, key.code),
        Mode::Alias => alias(app, key.code),
        Mode::Reader => reader(app, key.code),
        Mode::Help => app.mode = Mode::Browse,
        Mode::Upload => upload(app, key.code),
    }
}

fn browse(app: &mut App, code: KeyCode) {
    let pending_g = std::mem::take(&mut app.pending_g);
    match code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Esc if app.filter.is_empty() => app.quit = true,
        KeyCode::Esc => {
            app.filter.clear();
            app.rebuild_rows();
        }
        KeyCode::Char('?') => app.mode = Mode::Help,
        KeyCode::Char('1') => app.tab = Tab::Files,
        KeyCode::Char('2') => app.tab = Tab::Calendar,
        KeyCode::Char('3') => app.tab = Tab::Todo,
        KeyCode::Tab | KeyCode::BackTab => toggle_pane(app),
        KeyCode::Char('r') => tasks::spawn_sync(app),
        // Everything above is shared; the rest depends on the tab. The
        // calendar moves by dates, so it takes over j/k/g/G as well.
        _ if app.tab == Tab::Calendar => calendar(app, code, pending_g),
        KeyCode::Char('j') | KeyCode::Down => app.move_sel(1),
        KeyCode::Char('k') | KeyCode::Up => app.move_sel(-1),
        KeyCode::Char('g') if pending_g => app.move_sel(isize::MIN / 2),
        KeyCode::Char('g') => app.pending_g = true,
        KeyCode::Char('G') => app.move_sel(isize::MAX / 2),
        _ if app.tab == Tab::Todo => todo(app, code),
        KeyCode::Char('h') | KeyCode::Left => collapse(app),
        KeyCode::Char('l') | KeyCode::Right => expand(app),
        KeyCode::Enter => enter(app),
        KeyCode::Char('d') => download(app),
        KeyCode::Char('a') => start_alias(app),
        KeyCode::Char('/') => {
            app.mode = Mode::Filter;
            app.pane = Pane::Contents;
        }
        KeyCode::Char('o') => open(app),
        _ => {}
    }
}

fn calendar(app: &mut App, code: KeyCode, pending_g: bool) {
    let (year, month, day) = (app.cal_year, app.cal_month, app.cal_day);
    match code {
        KeyCode::Char('h') | KeyCode::Left => move_day(app, -1),
        KeyCode::Char('l') | KeyCode::Right => move_day(app, 1),
        KeyCode::Char('j') | KeyCode::Down => move_day(app, 7),
        KeyCode::Char('k') | KeyCode::Up => move_day(app, -7),
        KeyCode::Char('H') => {
            let (year, month) = calendar::add_months(year, month, -1);
            app.set_date(year, month, day);
        }
        KeyCode::Char('L') => {
            let (year, month) = calendar::add_months(year, month, 1);
            app.set_date(year, month, day);
        }
        KeyCode::Char('t') => {
            let (year, month, day) = calendar::civil(app::now());
            app.set_date(year, month, day);
        }
        KeyCode::Char('g') if pending_g => app.set_date(year, month, 1),
        KeyCode::Char('g') => app.pending_g = true,
        KeyCode::Char('G') => app.set_date(year, month, calendar::month_days(year, month)),
        KeyCode::Char('J') => app.move_event_sel(1),
        KeyCode::Char('K') => app.move_event_sel(-1),
        KeyCode::Char('o') => {
            let url = app.cal_entry().and_then(|entry| entry.url.clone());
            app.open_in_browser(url.as_deref());
        }
        KeyCode::Enter => calendar_enter(app),
        _ => {}
    }
}

fn move_day(app: &mut App, delta: i64) {
    let (year, month, day) = calendar::add_days(app.cal_year, app.cal_month, app.cal_day, delta);
    app.set_date(year, month, day);
}

/// Shows the event's course in the Files tab; the event itself has no
/// node to land on.
fn calendar_enter(app: &mut App) {
    let Some(entry) = app.cal_entry() else {
        app.set_status("no events on this day");
        return;
    };
    let Some(course_id) = entry.course_id.clone() else {
        app.set_error("this event belongs to no course");
        return;
    };
    let Some(course) = app.courses.iter().position(|id| *id == course_id) else {
        app.set_error("the course is hidden in config.toml");
        return;
    };
    app.select_course(course);
    app.tab = Tab::Files;
    app.pane = Pane::Sources;
}

fn todo(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Enter => todo_enter(app),
        KeyCode::Char('o') => {
            let url = app.todo_row().and_then(|row| row.url.clone());
            app.open_in_browser(url.as_deref());
        }
        KeyCode::Char('A') => {
            app.todo_all = !app.todo_all;
            app.rebuild_todo();
        }
        KeyCode::Char('u') if app.todo_row().is_some() => {
            app.input.clear();
            app.mode = Mode::Upload;
        }
        _ => {}
    }
}

/// Jumps to the assignment's module in the Files tab, where its handouts
/// hang, opening the section and the module on the way.
fn todo_enter(app: &mut App) {
    let Some(row) = app.todo_row().cloned() else {
        return;
    };
    let (Some(course_id), Some(cmid)) = (row.course_id, row.cmid) else {
        app.set_error("this assignment has no files");
        return;
    };
    let Some(course) = app.courses.iter().position(|id| *id == course_id) else {
        app.set_error("the course is hidden in config.toml");
        return;
    };
    let node_id = format!("moodle:mod:{cmid}");
    if let Some(parent) = app
        .index
        .node(&node_id)
        .and_then(|node| node.parent.clone())
    {
        app.expanded.insert(parent);
    }
    app.expanded.insert(node_id.clone());
    app.filter.clear();
    app.select_course(course);
    app.tab = Tab::Files;
    app.pane = Pane::Contents;
    match app.rows.iter().position(|row| row.id == node_id) {
        Some(i) => app.row_sel = i,
        None => app.set_error("assignment not in the files tree — press r to sync"),
    }
}

fn upload(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => app.mode = Mode::Browse,
        KeyCode::Backspace => {
            app.input.pop();
        }
        KeyCode::Char(c) => app.input.push(c),
        KeyCode::Enter => start_upload(app),
        _ => {}
    }
}

fn start_upload(app: &mut App) {
    app.mode = Mode::Browse;
    let Some(row) = app.todo_row() else {
        return;
    };
    let Some(assignid) = row
        .event_id
        .strip_prefix("moodle:assign:")
        .and_then(|id| id.parse::<u64>().ok())
    else {
        app.set_error("only Moodle assignments take uploads");
        return;
    };
    let path = resolve_path(app.input.trim(), &app.download_dir);
    if !path.is_file() {
        app.set_error(format!("no such file: {}", path.display()));
        return;
    }
    tasks::spawn_upload(app, assignid, path);
}

/// `~/x` from the home directory, `x` from the launch directory.
fn resolve_path(input: &str, cwd: &std::path::Path) -> PathBuf {
    if let Some(rest) = input.strip_prefix("~/")
        && let Some(home) = std::env::home_dir()
    {
        return home.join(rest);
    }
    cwd.join(input)
}

fn toggle_pane(app: &mut App) {
    app.pane = match app.pane {
        Pane::Sources => Pane::Contents,
        Pane::Contents => Pane::Sources,
    };
}

/// Closes the focused node, or jumps to its parent when it is not open;
/// at the top level it moves focus back to the Sources pane.
fn collapse(app: &mut App) {
    if app.pane == Pane::Sources {
        return;
    }
    let Some(row) = app.row() else {
        app.pane = Pane::Sources;
        return;
    };
    if row.is_node() && app.expanded.contains(&row.id) {
        let id = row.id.clone();
        app.set_open(&id, false);
        return;
    }
    let depth = row.depth;
    match (0..app.row_sel).rev().find(|&i| app.rows[i].depth < depth) {
        Some(parent) => app.row_sel = parent,
        None => app.pane = Pane::Sources,
    }
}

fn expand(app: &mut App) {
    if app.pane == Pane::Sources {
        app.pane = Pane::Contents;
        return;
    }
    let Some(row) = app.row() else {
        return;
    };
    if row.is_node() {
        let id = row.id.clone();
        app.set_open(&id, true);
    }
}

/// Enter is polymorphic (SPEC §4.2): download a file, read a page or a
/// channel's messages, open a link, otherwise toggle the node.
fn enter(app: &mut App) {
    if app.pane == Pane::Sources {
        app.pane = Pane::Contents;
        return;
    }
    let Some(row) = app.row().cloned() else {
        return;
    };
    match (&row.kind, row.modname()) {
        (RowKind::File(_), _) => download(app),
        (RowKind::Node(..), Some("messages")) => tasks::spawn_messages(app, &row.id),
        (RowKind::Node(..), Some("page")) => tasks::spawn_page(app, &row.id),
        (RowKind::Node(..), Some("url")) => open(app),
        (RowKind::Node(..), _) if tree::has_children(&app.index, &row.id) => {
            let open = !app.expanded.contains(&row.id);
            app.set_open(&row.id, open);
        }
        (RowKind::Node(kind, _), _) => app.set_status(format!("{} is empty", kind_name(*kind))),
    }
}

/// The focused file, or everything under the focused node.
fn download(app: &mut App) {
    if app.pane == Pane::Sources {
        app.set_error("pick a section or folder to download");
        return;
    }
    let Some(row) = app.row().cloned() else {
        return;
    };
    if row.is_node() {
        tasks::spawn_download_node(app, &row.id);
        return;
    }
    if let Some(file) = app.index.files.iter().find(|file| file.id == row.id) {
        tasks::spawn_download(app, file.clone());
    }
}

fn start_alias(app: &mut App) {
    let Some(course) = app.course() else {
        return;
    };
    app.input = app.config.alias_for(&course.id).unwrap_or("").to_string();
    app.mode = Mode::Alias;
}

/// Opens the focused item in the browser: the course, a node's page, or a
/// file through its tokenised download link.
fn open(app: &mut App) {
    if app.pane == Pane::Sources {
        let url = app.course().and_then(|course| course.web_url.clone());
        app.open_in_browser(url.as_deref());
        return;
    }
    let Some(row) = app.row().cloned() else {
        return;
    };
    let url = if row.is_node() {
        app.index
            .node(&row.id)
            .and_then(|node| node.web_url.clone())
    } else {
        let file = app.index.files.iter().find(|file| file.id == row.id);
        match (&app.client, file) {
            (Some(client), Some(file)) => Some(client.file_url(&file.download_url)),
            _ => None,
        }
    };
    app.open_in_browser(url.as_deref());
}

fn filter(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            app.filter.clear();
            app.mode = Mode::Browse;
        }
        KeyCode::Enter => app.mode = Mode::Browse,
        KeyCode::Backspace => {
            app.filter.pop();
        }
        KeyCode::Char(c) => app.filter.push(c),
        _ => return,
    }
    app.rebuild_rows();
}

fn alias(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => app.mode = Mode::Browse,
        KeyCode::Backspace => {
            app.input.pop();
        }
        KeyCode::Char(c) => app.input.push(c),
        KeyCode::Enter => save_alias(app),
        _ => {}
    }
}

fn save_alias(app: &mut App) {
    app.mode = Mode::Browse;
    let alias = app.input.trim().to_string();
    let Some(course_id) = app.course_id().map(str::to_string) else {
        return;
    };
    if alias.is_empty() {
        app.config.aliases.remove(&course_id);
    } else {
        app.config.set_alias(&course_id, &alias);
    }
    match app::save_config(&app.config) {
        Ok(()) => app.set_status(format!("alias: {alias}")),
        Err(err) => app.set_error(format!("{err:#}")),
    }
}

fn reader(app: &mut App, code: KeyCode) {
    let pending_g = std::mem::take(&mut app.pending_g);
    let height = reader_height(app);
    let Some(reader) = app.reader.as_mut() else {
        app.mode = Mode::Browse;
        return;
    };
    match code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.reader = None;
            app.mode = Mode::Browse;
        }
        KeyCode::Char('j') | KeyCode::Down => reader.scroll_by(1, height),
        KeyCode::Char('k') | KeyCode::Up => reader.scroll_by(-1, height),
        KeyCode::PageDown | KeyCode::Char(' ') => reader.scroll_by(height as isize, height),
        KeyCode::PageUp => reader.scroll_by(-(height as isize), height),
        KeyCode::Char('g') if pending_g => reader.scroll = 0,
        KeyCode::Char('g') => app.pending_g = true,
        KeyCode::Char('G') => reader.scroll_to_end(height),
        KeyCode::Char('o') => {
            let url = reader.web_url.clone();
            app.open_in_browser(url.as_deref());
        }
        _ => {}
    }
}

/// Lines visible in the reader box; `draw.rs` sizes it the same way.
fn reader_height(app: &App) -> usize {
    usize::from(app.height) * 4 / 5
}

fn kind_name(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Section => "section",
        NodeKind::Folder => "folder",
        NodeKind::Course | NodeKind::Team => "course",
        NodeKind::Channel => "channel",
        NodeKind::Module => "module",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::state::tests::{app, calendar_app, teams_app, todo_app};

    fn press(app: &mut App, code: KeyCode) {
        handle(app, KeyEvent::from(code));
    }

    #[test]
    fn j_and_k_move_and_clamp() {
        let mut app = app();
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.pane, Pane::Contents);
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.row_sel, 1);
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.row_sel, 5);
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.row_sel, 5);
        press(&mut app, KeyCode::Char('g'));
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.row_sel, 0);
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.row_sel, 0);
    }

    #[test]
    fn slash_filters_live_and_esc_clears() {
        let mut app = app();
        press(&mut app, KeyCode::Char('/'));
        assert_eq!(app.mode, Mode::Filter);
        for c in "ex1".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        assert_eq!(app.filter, "ex1");
        assert_eq!(app.rows.len(), 3);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.mode, Mode::Browse);
        assert_eq!(app.rows.len(), 3);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, Mode::Browse);
        assert!(app.filter.is_empty());
        assert_eq!(app.rows.len(), 6);
    }

    #[test]
    fn a_on_sources_prompts_for_the_alias() {
        let mut app = app();
        press(&mut app, KeyCode::Char('a'));
        assert_eq!(app.mode, Mode::Alias);
        assert_eq!(app.input, "algo");
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, Mode::Browse);
    }

    #[test]
    fn h_and_l_collapse_and_expand() {
        let mut app = app();
        app.pane = Pane::Contents;
        app.row_sel = 2; // Exercises/
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.rows[3].name, "ex1.pdf");
        press(&mut app, KeyCode::Char('h'));
        assert_eq!(app.rows.len(), 6);
        // A closed node jumps to its parent; the parent closes on the next h.
        press(&mut app, KeyCode::Char('h'));
        assert_eq!(app.row_sel, 0);
        press(&mut app, KeyCode::Char('h'));
        assert_eq!(app.rows.len(), 3);
        // Nothing above a closed top-level row: focus goes to Sources.
        press(&mut app, KeyCode::Char('h'));
        assert_eq!(app.pane, Pane::Sources);
    }

    #[test]
    fn enter_on_a_todo_row_lands_on_the_module_in_files() {
        let mut app = todo_app();
        assert_eq!(app.todo_rows[0].title, "Homework 1");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.tab, Tab::Files);
        assert_eq!(app.pane, Pane::Contents);
        assert_eq!(app.course_id(), Some("moodle:2"));
        assert_eq!(app.rows[app.row_sel].id, "moodle:mod:7");
    }

    #[test]
    fn todo_keys_toggle_all_and_prompt_for_an_upload() {
        let mut app = todo_app();
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.todo_sel, 1);
        press(&mut app, KeyCode::Char('A'));
        assert!(app.todo_all);
        press(&mut app, KeyCode::Char('A'));
        assert!(!app.todo_all);
        press(&mut app, KeyCode::Char('u'));
        assert_eq!(app.mode, Mode::Upload);
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(app.input, "x");
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, Mode::Browse);
        // A missing file is an error in the footer, not a task.
        press(&mut app, KeyCode::Char('u'));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.mode, Mode::Browse);
        assert!(app.error);
        assert!(app.status.starts_with("no such file"));
    }

    #[test]
    fn calendar_keys_move_by_day_week_and_month() {
        let mut app = calendar_app();
        let (year, month, day) = calendar::civil(app::now());
        let next = calendar::add_months(year, month, 1);
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.cal_day, calendar::month_days(year, month));
        // Past the last day: the next month comes in, on its first day.
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(
            (app.cal_year, app.cal_month, app.cal_day),
            (next.0, next.1, 1)
        );
        press(&mut app, KeyCode::Char('H'));
        assert_eq!((app.cal_year, app.cal_month, app.cal_day), (year, month, 1));
        press(&mut app, KeyCode::Char('L'));
        assert_eq!((app.cal_year, app.cal_month), next);
        press(&mut app, KeyCode::Char('t'));
        assert_eq!(
            (app.cal_year, app.cal_month, app.cal_day),
            (year, month, day)
        );
        press(&mut app, KeyCode::Char('k'));
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(
            (app.cal_year, app.cal_month, app.cal_day),
            (year, month, day)
        );
        press(&mut app, KeyCode::Char('g'));
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.cal_day, 1);
        assert_eq!(app.tab, Tab::Calendar);
    }

    #[test]
    fn enter_on_a_calendar_event_selects_its_course_in_files() {
        let mut app = calendar_app();
        press(&mut app, KeyCode::Char('J'));
        assert_eq!(app.cal_entry().unwrap().title, "Exam");
        press(&mut app, KeyCode::Char('K'));
        assert_eq!(app.cal_entry().unwrap().title, "Lab due");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.tab, Tab::Files);
        assert_eq!(app.pane, Pane::Sources);
        assert_eq!(app.course_id(), Some("moodle:2"));
        // A day without events only says so.
        app.tab = Tab::Calendar;
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.tab, Tab::Calendar);
        assert_eq!(app.status, "no events on this day");
    }

    #[test]
    fn paths_resolve_against_home_or_cwd() {
        let cwd = std::path::Path::new("/work");
        assert_eq!(resolve_path("hw.pdf", cwd), PathBuf::from("/work/hw.pdf"));
        assert_eq!(
            resolve_path("/abs/hw.pdf", cwd),
            PathBuf::from("/abs/hw.pdf")
        );
        let home = std::env::home_dir().unwrap();
        assert_eq!(resolve_path("~/hw.pdf", cwd), home.join("hw.pdf"));
    }

    #[test]
    fn enter_on_messages_without_a_teams_login_says_so() {
        let mut app = teams_app();
        assert_eq!(app.course_id(), Some("teams:T1"));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.row().unwrap().id, "teams:msg:C1");
        press(&mut app, KeyCode::Enter);
        assert!(app.error);
        assert!(app.status.starts_with("not logged in to Teams"));
        assert_eq!(app.mode, Mode::Browse);
        assert!(!app.busy);
    }

    #[test]
    fn q_quits_and_help_closes_on_any_key() {
        let mut app = app();
        press(&mut app, KeyCode::Char('?'));
        assert_eq!(app.mode, Mode::Help);
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(app.mode, Mode::Browse);
        press(&mut app, KeyCode::Char('q'));
        assert!(app.quit);
    }
}
