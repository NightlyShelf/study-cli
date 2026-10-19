//! Rendering: tab bar, three columns, footer, overlays (SPEC §4).

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap};
use study_core::index::{Node, NodeKind, SourceKind};

use crate::app;
use crate::fmt::{date, human_age, human_size};
use crate::tui::calendar;
use crate::tui::state::{App, Mode, Pane, Tab, course_label};
use crate::tui::todo::TodoRow;
use crate::tui::tree::{Row, RowKind};

const HELP: &str = "\
j k ↓ ↑     move            Enter   download / open / read
h l ← →     collapse/expand d       download file or whole node
Tab S-Tab   switch pane     a       alias for the course
1 2 3       switch tab      /       filter (Enter keeps, Esc clears)
g g / G     top / bottom    o       open in browser
r           sync now        q Esc   quit / close

Todo tab    Enter   its files in the Files tab
            u       upload a submission draft
            A       show finished and old work too

Calendar    h l / j k   day / week      H L   month    t   today
            J K         event           Enter its course in Files

Teams       Enter on Messages (…) reads the channel, o opens it";

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
        Tab::Calendar => draw_calendar(frame, app, body),
        Tab::Todo => draw_todo(frame, app, body),
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
        .map(|course| ListItem::new(source_line(app, course)))
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

/// The course's label, tagged when it is a team; Moodle is the default
/// and gets no tag.
fn source_line(app: &App, course: &Node) -> Line<'static> {
    let label = Span::raw(course_label(&app.config, course));
    match course.source {
        SourceKind::Moodle => Line::from(label),
        SourceKind::Teams => Line::from(vec![label, Span::raw(" teams").dim()]),
    }
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
        (NodeKind::Section | NodeKind::Folder | NodeKind::Channel, _) => "/",
        (_, Some("page")) => " [page]",
        (_, Some("url")) => " [url]",
        (_, Some("messages")) => " [chat]",
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
    let summary = match course.source {
        SourceKind::Moodle => format!("{} sections", app.index.children(&course.id).len()),
        SourceKind::Teams => "Microsoft Teams".to_string(),
    };
    vec![
        course_label(&app.config, course),
        course.name.clone(),
        summary,
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
    let node = app.index.node(&row.id);
    let url = node
        .and_then(|node| node.web_url.clone())
        .unwrap_or_default();
    let kind = match (&row.kind, row.modname()) {
        (RowKind::Node(NodeKind::Channel, _), _) => "channel",
        (_, modname) => modname.unwrap_or("section"),
    };
    let hint = match row.modname() {
        Some("page") | Some("messages") => "Enter read   o open",
        Some("url") => "o open",
        _ => "Enter expand   d download all",
    };
    let mut lines = vec![row.name.clone(), kind.to_string()];
    // Moodle is the default and goes unsaid, like the Sources tag.
    if node.is_some_and(|node| node.source == SourceKind::Teams) {
        lines.push("Microsoft Teams".to_string());
    }
    lines.extend([url, String::new(), hint.to_string()]);
    lines
}

fn draw_calendar(frame: &mut Frame, app: &App, area: Rect) {
    let [grid, events] =
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(area);
    let paragraph = Paragraph::new(month_grid(app)).block(pane_block("Calendar", true));
    frame.render_widget(paragraph, grid);
    let title = app.cal_day().map(|day| date(day.secs)).unwrap_or_default();
    let width = usize::from(events.width.saturating_sub(2));
    let paragraph = Paragraph::new(day_events(app, width)).block(Block::bordered().title(title));
    frame.render_widget(paragraph, events);
}

/// `October 2026`, the weekday row, then a line per week; `*` marks days
/// with events, the selection is reversed and today underlined.
fn month_grid(app: &App) -> Vec<Line<'static>> {
    let today = calendar::civil(app::now());
    let first = calendar::days_from_civil(app.cal_year, app.cal_month, 1);
    let header = format!("{} {}", calendar::month_name(app.cal_month), app.cal_year);
    let mut lines = vec![
        Line::from(header).bold(),
        Line::from("Mo  Tu  We  Th  Fr  Sa  Su").dim(),
    ];
    let mut week = vec![Span::raw("    ".repeat(calendar::weekday(first) as usize))];
    for (i, day) in app.cal_days.iter().enumerate() {
        let number = i as u32 + 1;
        let marker = if day.events.is_empty() { ' ' } else { '*' };
        let mut span = Span::raw(format!("{number:>2}{marker}"));
        if number == app.cal_day {
            span = span.reversed();
        }
        if (app.cal_year, app.cal_month, number) == today {
            span = span.bold().underlined();
        }
        week.push(span);
        week.push(Span::raw(" "));
        if calendar::weekday(first + i as i64) == 6 {
            lines.push(Line::from(std::mem::take(&mut week)));
        }
    }
    if week.len() > 1 {
        lines.push(Line::from(week));
    }
    lines
}

/// `HH:MM  kind  title` per event with its course dimmed underneath, the
/// selected one bold; the key hints close the pane.
fn day_events(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    match app.cal_day() {
        Some(day) if !day.events.is_empty() => {
            for (i, entry) in day.events.iter().enumerate() {
                let head = format!("{}  {:<6} {}", entry.time, entry.kind, entry.title);
                let span = Span::raw(fit(&head, width));
                lines.push(Line::from(if i == app.cal_sel {
                    span.bold()
                } else {
                    span
                }));
                if !entry.course.is_empty() {
                    let course = fit(&format!("       {}", entry.course), width);
                    lines.push(Line::from(Span::raw(course).dim()));
                }
            }
        }
        _ => lines.push(Line::from("no events")),
    }
    lines.push(Line::from(""));
    lines.push(Line::from("h/l day · j/k week · H/L month").dim());
    lines.push(Line::from("t today · J/K event").dim());
    lines.push(Line::from("Enter files · o open").dim());
    lines
}

fn draw_todo(frame: &mut Frame, app: &App, area: Rect) {
    let [list, detail] =
        Layout::horizontal([Constraint::Percentage(75), Constraint::Percentage(25)]).areas(area);
    draw_todo_list(frame, app, list);
    let text: Vec<Line> = todo_detail(app).into_iter().map(Line::from).collect();
    let paragraph = Paragraph::new(text)
        .block(Block::bordered().title("Detail"))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, detail);
}

fn draw_todo_list(frame: &mut Frame, app: &App, area: Rect) {
    let title = if app.todo_all { "Todo (all)" } else { "Todo" };
    if app.todo_rows.is_empty() {
        let hint = if app.todo_hidden > 0 {
            "no upcoming coursework — A shows everything"
        } else {
            "no upcoming coursework — press r to sync"
        };
        frame.render_widget(Paragraph::new(hint).block(pane_block(title, true)), area);
        return;
    }
    let width = usize::from(area.width.saturating_sub(2));
    let items: Vec<ListItem> = app
        .todo_rows
        .iter()
        .map(|row| ListItem::new(todo_line(row, width)))
        .collect();
    let list = List::new(items)
        .block(pane_block(title, true))
        .highlight_style(highlight(true));
    let height = area.height.saturating_sub(2);
    let mut state = ListState::default()
        .with_selected(Some(app.todo_sel))
        .with_offset(offset(app.todo_sel, app.todo_rows.len(), height));
    frame.render_stateful_widget(list, area, &mut state);
}

/// `due · status · course · title` in fixed columns; overdue work is red.
fn todo_line(row: &TodoRow, width: usize) -> Line<'static> {
    let text = format!(
        "{:<9} {:<10} {:<24} {}",
        row.due_text,
        row.status,
        fit(&row.course, 23),
        row.title
    );
    let span = Span::raw(fit(&text, width));
    if row.due_text.ends_with("ago") && row.status.is_empty() {
        Line::from(span.fg(Color::Red))
    } else {
        Line::from(span)
    }
}

fn todo_detail(app: &App) -> Vec<String> {
    let Some(row) = app.todo_row() else {
        return vec!["no coursework selected".to_string()];
    };
    let due = match row.due {
        Some(due) => format!("due {} ({})", date(due), row.due_text),
        None => "no due date".to_string(),
    };
    let status = if row.status.is_empty() {
        "not handed in".to_string()
    } else {
        row.status.clone()
    };
    vec![
        row.title.clone(),
        row.course.clone(),
        due,
        status,
        String::new(),
        "Enter files · o open · u upload · A all".to_string(),
    ]
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let line = match app.mode {
        Mode::Filter => Line::from(format!("/{}_   {} rows", app.filter, app.rows.len())),
        Mode::Alias => Line::from(format!(
            "alias: {}_   (Enter saves, Esc cancels)",
            app.input
        )),
        Mode::Upload => {
            let title = app.todo_row().map(|row| row.title.as_str()).unwrap_or("");
            Line::from(format!(
                "upload to \"{title}\": {}_   (Enter saves a draft, Esc cancels)",
                app.input
            ))
        }
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
    let area = centered(frame.area(), 70, 60);
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
    use crate::tui::state::tests::{app, calendar_app, teams_app, todo_app};

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
    fn teams_sources_are_tagged_and_channels_look_like_folders() {
        let mut app = teams_app();
        let text = render(&app);
        assert!(text.contains("Machine Learning teams"));
        assert!(text.contains("Databases teams"));
        assert!(text.contains("slides.pdf"));
        assert!(text.contains("Messages (General) [chat]"));
        assert!(text.contains("Microsoft Teams"));
        app.select_course(1);
        app.pane = Pane::Contents;
        let text = render(&app);
        assert!(text.contains("> Labs/"));
        assert!(text.contains("channel"));
        assert!(text.contains("Microsoft Teams"));
        assert!(text.contains("Enter expand"));
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
    fn help_renders() {
        let mut app = app();
        app.mode = Mode::Help;
        let text = render(&app);
        assert!(text.contains("switch pane"));
        assert!(text.contains("its course in Files"));
    }

    #[test]
    fn calendar_tab_shows_the_grid_and_the_days_events() {
        let app = calendar_app();
        let text = render(&app);
        let header = format!("{} {}", calendar::month_name(app.cal_month), app.cal_year);
        assert!(text.contains(&header));
        assert!(text.contains("Mo  Tu  We  Th  Fr  Sa  Su"));
        assert!(text.contains(&format!("{:>2}*", app.cal_day)));
        assert!(text.contains("09:00  due    Lab due"));
        assert!(text.contains("14:00  due    Exam"));
        assert!(text.contains("       algo"));
        assert!(text.contains(&date(app.cal_day().unwrap().secs)));
        assert!(text.contains("t today · J/K event"));
        assert!(!text.contains("no events"));
    }

    #[test]
    fn calendar_tab_without_events() {
        let mut app = app();
        app.tab = Tab::Calendar;
        let text = render(&app);
        assert!(text.contains("no events"));
        assert!(!text.contains('*'));
        assert!(text.contains(&format!("{:>2} ", app.cal_day)));
    }

    #[test]
    fn todo_tab_lists_coursework_with_a_detail_pane() {
        let mut app = todo_app();
        let text = render(&app);
        assert!(text.contains("in 3d"));
        assert!(text.contains("Homework 1"));
        assert!(text.contains("Algebra"));
        assert!(text.contains("Essay"));
        assert!(text.contains("due 20"));
        assert!(text.contains("not handed in"));
        assert!(text.contains("u upload"));
        app.mode = Mode::Upload;
        app.input = "hw.pdf".to_string();
        assert!(render(&app).contains("upload to \"Homework 1\": hw.pdf_"));
    }

    #[test]
    fn todo_tab_empty_states() {
        let mut app = app();
        app.tab = Tab::Todo;
        assert!(render(&app).contains("press r to sync"));
        let mut app = todo_app();
        for event in &mut app.index.events {
            event.status = Some("submitted".to_string());
        }
        app.rebuild_todo();
        assert!(render(&app).contains("A shows everything"));
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
