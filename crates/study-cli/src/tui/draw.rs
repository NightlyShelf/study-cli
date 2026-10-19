//! Rendering: tab bar, three columns, footer, overlays (SPEC §4).

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap};
use study_core::index::NodeKind;

use crate::app;
use crate::fmt::{human_age, human_size};
use crate::tui::state::{App, Mode, Pane, Tab, course_label};
use crate::tui::tree::{Row, RowKind};

const HELP: &str = "\
j k ↓ ↑     move            Enter   download / open / read
h l ← →     collapse/expand d       download file or whole node
Tab S-Tab   switch pane     a       alias for the course
1 2 3       switch tab      /       filter (Enter keeps, Esc clears)
g g / G     top / bottom    o       open in browser
r           sync now        q Esc   quit / close";

pub fn draw(frame: &mut Frame, app: &App) {
    let [tabs, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    draw_tabs(frame, app, tabs);
    match app.tab {
        Tab::Files => draw_files(frame, app, body),
        Tab::Calendar => frame.render_widget(Paragraph::new("Calendar: not implemented yet"), body),
        Tab::Todo => frame.render_widget(Paragraph::new("Todo: not implemented yet"), body),
    }
    draw_footer(frame, app, footer);
    match app.mode {
        Mode::Reader => draw_reader(frame, app),
        Mode::Help => draw_help(frame),
        _ => {}
    }
}

fn draw_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let selected = match app.tab {
        Tab::Files => 0,
        Tab::Calendar => 1,
        Tab::Todo => 2,
    };
    let tabs = Tabs::new(["Files", "Calendar", "Todo"])
        .select(selected)
        .highlight_style(Style::new().bold().fg(Color::Yellow));
    frame.render_widget(tabs, area);
}

fn draw_files(frame: &mut Frame, app: &App, area: Rect) {
    let [sources, contents, detail] = Layout::horizontal([
        Constraint::Percentage(25),
        Constraint::Percentage(50),
        Constraint::Percentage(25),
    ])
    .areas(area);
    draw_sources(frame, app, sources);
    draw_contents(frame, app, contents);
    draw_detail(frame, app, detail);
}

fn draw_sources(frame: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .courses
        .iter()
        .filter_map(|id| app.index.node(id))
        .map(|course| ListItem::new(course_label(&app.config, course)))
        .collect();
    let list = List::new(items)
        .block(pane_block("Sources", app.pane == Pane::Sources))
        .highlight_style(highlight(app.pane == Pane::Sources));
    let height = area.height.saturating_sub(2);
    let mut state = ListState::default()
        .with_selected(Some(app.course_sel))
        .with_offset(offset(app.course_sel, app.courses.len(), height));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_contents(frame: &mut Frame, app: &App, area: Rect) {
    let title = match app.course() {
        Some(course) => course_label(&app.config, course),
        None => "Contents".to_string(),
    };
    let width = usize::from(area.width.saturating_sub(2));
    let items: Vec<ListItem> = app
        .rows
        .iter()
        .map(|row| ListItem::new(row_line(row, app, width)))
        .collect();
    let list = List::new(items)
        .block(pane_block(&title, app.pane == Pane::Contents))
        .highlight_style(highlight(app.pane == Pane::Contents));
    let height = area.height.saturating_sub(2);
    let mut state = ListState::default()
        .with_selected(Some(app.row_sel))
        .with_offset(offset(app.row_sel, app.rows.len(), height));
    frame.render_stateful_widget(list, area, &mut state);
}

/// `  > name/` with the size right-aligned for files.
fn row_line(row: &Row, app: &App, width: usize) -> Line<'static> {
    let indent = "  ".repeat(usize::from(row.depth));
    let (marker, suffix, size) = match &row.kind {
        RowKind::File(size) => ("  ", "", human_size(*size)),
        RowKind::Node(kind, modname) => {
            let open = if app.expanded.contains(&row.id) {
                "v "
            } else {
                "> "
            };
            (open, node_suffix(*kind, modname.as_deref()), String::new())
        }
    };
    let text = format!("{indent}{marker}{}{suffix}", row.name);
    let name = fit(&text, width.saturating_sub(size.len() + 1));
    let gap = width.saturating_sub(name.chars().count() + size.len());
    Line::from(vec![
        Span::raw(name),
        Span::raw(" ".repeat(gap)),
        Span::raw(size).dim(),
    ])
}

fn node_suffix(kind: NodeKind, modname: Option<&str>) -> &'static str {
    match (kind, modname) {
        (NodeKind::Section | NodeKind::Folder, _) => "/",
        (_, Some("page")) => " [page]",
        (_, Some("url")) => " [url]",
        _ => " [module]",
    }
}

fn draw_detail(frame: &mut Frame, app: &App, area: Rect) {
    let lines = match (app.pane, app.row()) {
        (Pane::Contents, Some(row)) => row_detail(app, row),
        _ => course_detail(app),
    };
    let text: Vec<Line> = lines.into_iter().map(Line::from).collect();
    let paragraph = Paragraph::new(text)
        .block(Block::bordered().title("Detail"))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn course_detail(app: &App) -> Vec<String> {
    let Some(course) = app.course() else {
        return vec!["no courses — run `study sync`".to_string()];
    };
    vec![
        course_label(&app.config, course),
        course.name.clone(),
        format!("{} sections", app.index.children(&course.id).len()),
        String::new(),
        "a alias   o open".to_string(),
    ]
}

fn row_detail(app: &App, row: &Row) -> Vec<String> {
    let file = app.index.files.iter().find(|file| file.id == row.id);
    if let (RowKind::File(size), Some(file)) = (&row.kind, file) {
        return vec![
            file.name.clone(),
            human_size(*size),
            format!("modified {}", human_age(file.mtime, app::now())),
            file.mime.clone().unwrap_or_default(),
            String::new(),
            "Enter download".to_string(),
        ];
    }
    let url = app
        .index
        .node(&row.id)
        .and_then(|node| node.web_url.clone())
        .unwrap_or_default();
    let hint = match row.modname() {
        Some("page") => "Enter read   o open",
        Some("url") => "o open",
        _ => "Enter expand   d download all",
    };
    vec![
        row.name.clone(),
        row.modname().unwrap_or("section").to_string(),
        url,
        String::new(),
        hint.to_string(),
    ]
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let line = match app.mode {
        Mode::Filter => Line::from(format!("/{}_   {} rows", app.filter, app.rows.len())),
        Mode::Alias => Line::from(format!(
            "alias: {}_   (Enter saves, Esc cancels)",
            app.input
        )),
        Mode::Reader => Line::from("j k scroll   g g / G ends   o open   Esc close"),
        Mode::Help => Line::from("any key closes"),
        Mode::Browse => browse_footer(app),
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn browse_footer(app: &App) -> Line<'static> {
    let status = if app.error {
        Span::raw(app.status.clone()).fg(Color::Red)
    } else {
        Span::raw(app.status.clone())
    };
    let mut spans = vec![status];
    if !app.filter.is_empty() {
        spans.push(Span::raw(format!("   /{}", app.filter)).dim());
    }
    if app.busy {
        spans.push(Span::raw("   …").dim());
    }
    let synced = app
        .index
        .sources
        .first()
        .and_then(|source| source.last_sync);
    let synced = human_age(synced, app::now());
    spans.push(Span::raw(format!("   synced {synced} ago   ? help")).dim());
    Line::from(spans)
}

fn draw_reader(frame: &mut Frame, app: &App) {
    let Some(reader) = &app.reader else {
        return;
    };
    let area = centered(frame.area(), 80, 80);
    let visible: Vec<Line> = reader
        .lines
        .iter()
        .skip(reader.scroll)
        .map(|line| Line::from(line.as_str()))
        .collect();
    let title = format!(" {} ", reader.title);
    let paragraph = Paragraph::new(visible).block(Block::bordered().title(title));
    frame.render_widget(Clear, area);
    frame.render_widget(paragraph, area);
}

fn draw_help(frame: &mut Frame) {
    let area = centered(frame.area(), 70, 50);
    let paragraph = Paragraph::new(HELP).block(Block::bordered().title(" Keys "));
    frame.render_widget(Clear, area);
    frame.render_widget(paragraph, area);
}

fn pane_block(title: &str, focused: bool) -> Block<'static> {
    let block = Block::bordered().title(title.to_string());
    if focused {
        block.border_style(Style::new().fg(Color::Yellow))
    } else {
        block
    }
}

fn highlight(focused: bool) -> Style {
    if focused {
        Style::new().add_modifier(Modifier::REVERSED)
    } else {
        Style::new().add_modifier(Modifier::BOLD)
    }
}

fn centered(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let [area] = Layout::horizontal([Constraint::Percentage(percent_x)])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::vertical([Constraint::Percentage(percent_y)])
        .flex(Flex::Center)
        .areas(area);
    area
}

/// First visible item: the selection stays centred once past the middle,
/// without leaving blank rows at the end.
fn offset(selected: usize, len: usize, height: u16) -> usize {
    let height = usize::from(height).max(1);
    selected
        .saturating_sub(height / 2)
        .min(len.saturating_sub(height))
}

/// Cuts `text` to `width` columns with an ellipsis.
fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let keep: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{keep}…")
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::tui::reader::Reader;
    use crate::tui::state::tests::app;

    fn render(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn browse_shows_courses_rows_and_footer() {
        let mut app = app();
        app.set_status("synced");
        let text = render(&app);
        assert!(text.contains("algo"));
        assert!(text.contains("Algebra"));
        assert!(text.contains("Lecture1.pdf"));
        assert!(text.contains("2.0K"));
        assert!(text.contains("Exercises/"));
        assert!(text.contains("Syllabus [page]"));
        assert!(text.contains("? help"));
    }

    #[test]
    fn reader_overlay_shows_the_page() {
        let mut app = app();
        app.mode = Mode::Reader;
        app.reader = Some(Reader {
            title: "Syllabus".to_string(),
            lines: vec!["Welcome to the course".to_string()],
            scroll: 0,
            web_url: None,
        });
        let text = render(&app);
        assert!(text.contains("Syllabus"));
        assert!(text.contains("Welcome to the course"));
        assert!(text.contains("Esc close"));
    }

    #[test]
    fn other_tabs_and_help_render() {
        let mut app = app();
        app.tab = Tab::Calendar;
        assert!(render(&app).contains("not implemented yet"));
        app.mode = Mode::Help;
        assert!(render(&app).contains("switch pane"));
    }

    #[test]
    fn offset_and_fit() {
        assert_eq!(offset(0, 100, 10), 0);
        assert_eq!(offset(7, 100, 10), 2);
        assert_eq!(offset(99, 100, 10), 90);
        assert_eq!(offset(3, 4, 10), 0);
        assert_eq!(fit("abc", 3), "abc");
        assert_eq!(fit("abcdef", 4), "abc…");
    }
}
