//! Key handling: one function per mode (SPEC §4.4).

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use study_core::index::NodeKind;

use crate::app;
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
        KeyCode::Char('j') | KeyCode::Down => app.move_sel(1),
        KeyCode::Char('k') | KeyCode::Up => app.move_sel(-1),
        KeyCode::Char('g') if pending_g => app.move_sel(isize::MIN / 2),
        KeyCode::Char('g') => app.pending_g = true,
        KeyCode::Char('G') => app.move_sel(isize::MAX / 2),
        KeyCode::Tab | KeyCode::BackTab => toggle_pane(app),
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
        KeyCode::Char('r') => tasks::spawn_sync(app),
        _ => {}
    }
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

/// Enter is polymorphic (SPEC §4.2): download a file, read a page, open a
/// link, otherwise toggle the node.
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
    use crate::tui::state::tests::app;

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
