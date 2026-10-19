//! Every piece of TUI state in one struct, plus the small transitions the
//! key handler needs.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use study_core::config::Config;
use study_core::index::{Index, Node, NodeKind};
use study_moodle::MoodleClient;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::app;
use crate::tui::calendar::{self, CalEntry, Day};
use crate::tui::reader::Reader;
use crate::tui::tasks::Msg;
use crate::tui::todo::{self, TodoRow};
use crate::tui::tree::{self, Row};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Files,
    Calendar,
    Todo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Sources,
    Contents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Browse,
    Filter,
    Alias,
    Reader,
    Help,
    /// Typing the path of a file to hand in (Todo tab).
    Upload,
}

pub struct App {
    pub config: Config,
    pub index: Index,
    /// `None` when not logged in: browsing still works, the network does not.
    pub client: Option<MoodleClient>,
    /// Where downloads land: the directory `study` was started from.
    pub download_dir: PathBuf,
    pub tab: Tab,
    pub pane: Pane,
    pub mode: Mode,
    /// Course node ids shown in the Sources pane.
    pub courses: Vec<String>,
    pub course_sel: usize,
    /// Ids of the open nodes in the content tree.
    pub expanded: HashSet<String>,
    pub rows: Vec<Row>,
    pub row_sel: usize,
    pub filter: String,
    /// The alias or upload path being typed.
    pub input: String,
    /// Coursework shown in the Todo tab, soonest due first.
    pub todo_rows: Vec<TodoRow>,
    pub todo_sel: usize,
    /// Show finished and long-overdue work too (`A`).
    pub todo_all: bool,
    /// How many rows `todo_all` would reveal.
    pub todo_hidden: usize,
    /// The month shown in the Calendar tab and the selected day (1-based).
    pub cal_year: i64,
    pub cal_month: u32,
    pub cal_day: u32,
    /// Which of the selected day's events `Enter` and `o` act on.
    pub cal_sel: usize,
    pub cal_days: Vec<Day>,
    pub reader: Option<Reader>,
    /// Pages fetched this session, by node id, so reopening is instant.
    pub pages: HashMap<String, Reader>,
    pub status: String,
    pub error: bool,
    pub busy: bool,
    /// A first `g` was pressed; the next one jumps to the top.
    pub pending_g: bool,
    /// Terminal size, refreshed every tick; the reader wraps and pages by it.
    pub width: u16,
    pub height: u16,
    pub tx: UnboundedSender<Msg>,
    pub rx: UnboundedReceiver<Msg>,
    pub quit: bool,
}

impl App {
    pub fn new(
        config: Config,
        index: Index,
        client: Option<MoodleClient>,
        download_dir: PathBuf,
    ) -> App {
        let (tx, rx) = mpsc::unbounded_channel();
        let (cal_year, cal_month, cal_day) = calendar::civil(app::now());
        let mut app = App {
            config,
            index: Index::default(),
            client,
            download_dir,
            tab: Tab::Files,
            pane: Pane::Sources,
            mode: Mode::Browse,
            courses: Vec::new(),
            course_sel: 0,
            expanded: HashSet::new(),
            rows: Vec::new(),
            row_sel: 0,
            filter: String::new(),
            input: String::new(),
            todo_rows: Vec::new(),
            todo_sel: 0,
            todo_all: false,
            todo_hidden: 0,
            cal_year,
            cal_month,
            cal_day,
            cal_sel: 0,
            cal_days: Vec::new(),
            reader: None,
            pages: HashMap::new(),
            status: String::new(),
            error: false,
            busy: false,
            pending_g: false,
            width: 80,
            height: 24,
            tx,
            rx,
            quit: false,
        };
        app.replace_index(index);
        app
    }

    /// Swaps in a freshly synced index and rebuilds everything derived
    /// from it. Sections start open; folders and modules start closed.
    pub fn replace_index(&mut self, index: Index) {
        self.index = index;
        let hidden = &self.config.ui.hidden_courses;
        self.courses = self
            .index
            .courses()
            .iter()
            .filter(|course| !hidden.contains(&course.id))
            .map(|course| course.id.clone())
            .collect();
        for node in &self.index.nodes {
            if node.kind == NodeKind::Section {
                self.expanded.insert(node.id.clone());
            }
        }
        self.course_sel = self.course_sel.min(self.courses.len().saturating_sub(1));
        self.rebuild_rows();
        self.rebuild_todo();
        self.rebuild_calendar();
    }

    pub fn course_id(&self) -> Option<&str> {
        self.courses.get(self.course_sel).map(String::as_str)
    }

    pub fn course(&self) -> Option<&Node> {
        self.index.node(self.course_id()?)
    }

    pub fn row(&self) -> Option<&Row> {
        self.rows.get(self.row_sel)
    }

    pub fn todo_row(&self) -> Option<&TodoRow> {
        self.todo_rows.get(self.todo_sel)
    }

    pub fn cal_day(&self) -> Option<&Day> {
        self.cal_days.get((self.cal_day as usize).saturating_sub(1))
    }

    pub fn cal_entry(&self) -> Option<&CalEntry> {
        self.cal_day()?.events.get(self.cal_sel)
    }

    pub fn rebuild_calendar(&mut self) {
        self.cal_days = calendar::month_days_with_events(
            &self.index,
            &self.config,
            self.cal_year,
            self.cal_month,
        );
        let events = self.cal_day().map_or(0, |day| day.events.len());
        self.cal_sel = self.cal_sel.min(events.saturating_sub(1));
    }

    /// Selects a date, swapping the month's days in when it changes.
    pub fn set_date(&mut self, year: i64, month: u32, day: u32) {
        self.cal_day = day.clamp(1, calendar::month_days(year, month));
        self.cal_sel = 0;
        if (year, month) != (self.cal_year, self.cal_month) {
            self.cal_year = year;
            self.cal_month = month;
            self.rebuild_calendar();
        }
    }

    pub fn move_event_sel(&mut self, delta: isize) {
        let events = self.cal_day().map_or(0, |day| day.events.len());
        self.cal_sel = shifted(self.cal_sel, delta, events);
    }

    pub fn rebuild_todo(&mut self) {
        let now = app::now();
        let all = todo::todo_rows(&self.index, &self.config, now);
        let total = all.len();
        self.todo_rows = all
            .into_iter()
            .filter(|row| self.todo_all || todo::is_current(row, now))
            .collect();
        self.todo_hidden = total - self.todo_rows.len();
        self.todo_sel = self.todo_sel.min(self.todo_rows.len().saturating_sub(1));
    }

    pub fn rebuild_rows(&mut self) {
        self.rows = match self.course_id() {
            Some(course_id) => {
                tree::build_rows(&self.index, course_id, &self.expanded, &self.filter)
            }
            None => Vec::new(),
        };
        self.row_sel = self.row_sel.min(self.rows.len().saturating_sub(1));
    }

    pub fn select_course(&mut self, i: usize) {
        if self.courses.is_empty() {
            return;
        }
        self.course_sel = i.min(self.courses.len() - 1);
        self.row_sel = 0;
        self.rebuild_rows();
    }

    /// Moves the selection of the focused pane by `delta`, clamped.
    pub fn move_sel(&mut self, delta: isize) {
        if self.tab == Tab::Todo {
            self.todo_sel = shifted(self.todo_sel, delta, self.todo_rows.len());
            return;
        }
        match self.pane {
            Pane::Sources => {
                self.select_course(shifted(self.course_sel, delta, self.courses.len()))
            }
            Pane::Contents => self.row_sel = shifted(self.row_sel, delta, self.rows.len()),
        }
    }

    pub fn set_open(&mut self, id: &str, open: bool) {
        if open {
            self.expanded.insert(id.to_string());
        } else {
            self.expanded.remove(id);
        }
        self.rebuild_rows();
    }

    pub fn set_status(&mut self, text: impl Into<String>) {
        self.status = text.into();
        self.error = false;
    }

    pub fn set_error(&mut self, text: impl Into<String>) {
        self.status = text.into();
        self.error = true;
    }

    pub fn open_in_browser(&mut self, url: Option<&str>) {
        let Some(url) = url else {
            self.set_error("nothing to open");
            return;
        };
        match open::that_detached(url) {
            Ok(()) => self.set_status(format!("opened {url}")),
            Err(err) => self.set_error(format!("cannot open browser: {err}")),
        }
    }
}

/// `i + delta` kept inside `0..len`.
fn shifted(i: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    i.saturating_add_signed(delta).min(len - 1)
}

/// The alias when one is set, else the real name; the drawer truncates.
pub fn course_label(config: &Config, node: &Node) -> String {
    match config.alias_for(&node.id) {
        Some(alias) => alias.to_string(),
        None => node.name.clone(),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::tui::tree::tests::sample;

    pub fn app() -> App {
        let mut config = Config::default();
        config.set_alias("moodle:1", "algo");
        App::new(config, sample(), None, PathBuf::from("/tmp"))
    }

    /// `app()` on the Todo tab with two assignments (see `todo::tests`).
    pub fn todo_app() -> App {
        let mut app = app();
        app.replace_index(crate::tui::todo::tests::coursework(app::now()));
        app.tab = Tab::Todo;
        app
    }

    /// `app()` on the Calendar tab with two events today (see
    /// `calendar::tests`).
    pub fn calendar_app() -> App {
        let mut app = app();
        app.replace_index(crate::tui::calendar::tests::calendar(app::now()));
        app.tab = Tab::Calendar;
        app
    }

    #[test]
    fn new_opens_sections_of_the_first_course() {
        let app = app();
        assert_eq!(app.courses, ["moodle:1", "moodle:2"]);
        assert_eq!(app.rows.len(), 6);
        assert_eq!(course_label(&app.config, app.course().unwrap()), "algo");
    }

    #[test]
    fn selection_moves_and_clamps() {
        let mut app = app();
        app.pane = Pane::Contents;
        app.move_sel(100);
        assert_eq!(app.row_sel, 5);
        app.move_sel(-100);
        assert_eq!(app.row_sel, 0);

        app.pane = Pane::Sources;
        app.move_sel(1);
        assert_eq!(app.course_sel, 1);
        assert!(app.rows.is_empty());
        app.move_sel(1);
        assert_eq!(app.course_sel, 1);
    }

    #[test]
    fn todo_rows_follow_the_index_and_the_all_toggle() {
        let mut app = todo_app();
        assert_eq!(app.todo_rows.len(), 2);
        assert_eq!(app.todo_hidden, 0);
        app.move_sel(5);
        assert_eq!(app.todo_sel, 1);
        app.index.events[0].status = Some("submitted".to_string());
        app.rebuild_todo();
        assert_eq!(app.todo_rows.len(), 1);
        assert_eq!(app.todo_hidden, 1);
        assert_eq!(app.todo_sel, 0);
        app.todo_all = true;
        app.rebuild_todo();
        assert_eq!(app.todo_rows.len(), 2);
    }

    #[test]
    fn calendar_opens_on_today_and_follows_set_date() {
        let mut app = calendar_app();
        let (year, month, day) = calendar::civil(app::now());
        assert_eq!(
            (app.cal_year, app.cal_month, app.cal_day),
            (year, month, day)
        );
        assert_eq!(app.cal_days.len() as u32, calendar::month_days(year, month));
        assert_eq!(app.cal_day().unwrap().events.len(), 2);
        app.move_event_sel(5);
        assert_eq!(app.cal_sel, 1);
        assert_eq!(app.cal_entry().unwrap().title, "Exam");
        // A day past the month's end clamps; a new month rebuilds the days.
        app.set_date(2024, 2, 31);
        assert_eq!((app.cal_year, app.cal_month, app.cal_day), (2024, 2, 29));
        assert_eq!(app.cal_days.len(), 29);
        assert_eq!(app.cal_sel, 0);
        assert!(app.cal_entry().is_none());
    }

    #[test]
    fn hidden_courses_are_skipped() {
        let mut config = Config::default();
        config.ui.hidden_courses.push("moodle:1".to_string());
        let app = App::new(config, sample(), None, PathBuf::from("/tmp"));
        assert_eq!(app.courses, ["moodle:2"]);
    }
}
