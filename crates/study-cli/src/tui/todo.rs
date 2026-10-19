//! The Todo tab's rows: coursework from the index, soonest due first
//! (SPEC §8). Pure functions over the index so the tab needs no network.

use study_core::config::Config;
use study_core::index::{EventKind, Index};

use crate::fmt;
use crate::sync;
use crate::tui::state::course_label;

#[derive(Debug, Clone, PartialEq)]
pub struct TodoRow {
    pub event_id: String,
    pub due: Option<u64>,
    /// `in 3d`, `2d ago`, `-`.
    pub due_text: String,
    /// Empty when nothing was handed in yet, else `draft`, `submitted`,
    /// `graded 85/100`.
    pub status: String,
    /// The course alias, or its real name without one.
    pub course: String,
    pub title: String,
    pub cmid: Option<String>,
    pub course_id: Option<String>,
    pub url: Option<String>,
}

/// Every coursework event, due ascending with undated work last, then by
/// title. Filtering by `is_current` is the caller's choice.
pub fn todo_rows(index: &Index, config: &Config, now: u64) -> Vec<TodoRow> {
    let mut rows: Vec<TodoRow> = index
        .events
        .iter()
        .filter(|event| event.kind == EventKind::Coursework)
        .map(|event| {
            let course = event
                .course_id
                .as_deref()
                .and_then(|id| index.node(id))
                .map(|node| course_label(config, node))
                .unwrap_or_default();
            TodoRow {
                event_id: event.id.clone(),
                due: event.due,
                due_text: event
                    .due
                    .map_or("-".to_string(), |due| fmt::human_until(due, now)),
                status: status_text(event.status.as_deref(), event.grade.as_deref()),
                course,
                title: event.title.clone(),
                cmid: event.cmid.clone(),
                course_id: event.course_id.clone(),
                url: event.url.clone(),
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        (a.due.is_none(), a.due, &a.title).cmp(&(b.due.is_none(), b.due, &b.title))
    });
    rows
}

/// Shown by default: not handed in, and not overdue by more than two weeks.
pub fn is_current(row: &TodoRow, now: u64) -> bool {
    let done = row.status == "submitted" || row.status.starts_with("graded");
    !done && sync::is_current(row.due, now)
}

fn status_text(status: Option<&str>, grade: Option<&str>) -> String {
    match (status, grade) {
        (Some("graded"), Some(grade)) => format!("graded {grade}"),
        (Some("graded"), None) => "graded".to_string(),
        (Some("draft"), _) => "draft".to_string(),
        (Some("submitted"), _) => "submitted".to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
pub mod tests {
    use study_core::index::{Event, NodeKind, SourceKind};

    use super::*;
    use crate::tui::tree::tests::{node, sample};

    pub fn event(assignid: u64, cmid: u64, course: &str, title: &str, due: Option<u64>) -> Event {
        Event {
            id: format!("moodle:assign:{assignid}"),
            source: SourceKind::Moodle,
            kind: EventKind::Coursework,
            title: title.to_string(),
            start: None,
            due,
            course_id: Some(course.to_string()),
            cmid: Some(cmid.to_string()),
            status: Some("new".to_string()),
            grade: None,
            url: Some(format!("https://moodle.test/mod/assign/view.php?id={cmid}")),
        }
    }

    /// `sample()` plus, in Algebra, a section holding an assign module
    /// (`moodle:mod:7`) whose assignment is due in three days from `now`,
    /// and an undated one in Algorithms.
    pub fn coursework(now: u64) -> Index {
        let mut index = sample();
        let mut assign = node(
            "moodle:mod:7",
            NodeKind::Module,
            Some("moodle:sec:8"),
            "Homework 1",
        );
        assign.modname = Some("assign".to_string());
        index.nodes.push(node(
            "moodle:sec:8",
            NodeKind::Section,
            Some("moodle:2"),
            "Topic 1",
        ));
        index.nodes.push(assign);
        index.events.push(event(
            70,
            7,
            "moodle:2",
            "Homework 1",
            Some(now + 3 * 86_400),
        ));
        index.events.push(event(60, 6, "moodle:1", "Essay", None));
        index
    }

    const NOW: u64 = 1_700_000_000;

    #[test]
    fn rows_sort_by_due_with_undated_last() {
        let mut index = coursework(NOW);
        index
            .events
            .push(event(50, 5, "moodle:1", "Late", Some(NOW - 2 * 86_400)));
        let mut config = Config::default();
        config.set_alias("moodle:1", "algo");
        let rows = todo_rows(&index, &config, NOW);
        let titles: Vec<&str> = rows.iter().map(|row| row.title.as_str()).collect();
        assert_eq!(titles, ["Late", "Homework 1", "Essay"]);
        assert_eq!(rows[0].due_text, "2d ago");
        assert_eq!(rows[1].due_text, "in 3d");
        assert_eq!(rows[2].due_text, "-");
        assert_eq!(rows[0].course, "algo");
        assert_eq!(rows[1].course, "Algebra");
        assert_eq!(rows[1].cmid.as_deref(), Some("7"));
    }

    #[test]
    fn finished_and_long_overdue_work_is_not_current() {
        let mut index = coursework(NOW);
        index.events[0].status = Some("submitted".to_string());
        let mut old = event(50, 5, "moodle:1", "Old", Some(NOW - 20 * 86_400));
        old.status = Some("graded".to_string());
        old.grade = Some("85/100".to_string());
        index.events.push(old);
        index
            .events
            .push(event(40, 4, "moodle:1", "Recent", Some(NOW - 13 * 86_400)));
        let rows = todo_rows(&index, &Config::default(), NOW);
        let current: Vec<&str> = rows
            .iter()
            .filter(|row| is_current(row, NOW))
            .map(|row| row.title.as_str())
            .collect();
        assert_eq!(current, ["Recent", "Essay"]);
        assert_eq!(rows.len(), 4);
        let graded = rows.iter().find(|row| row.title == "Old").unwrap();
        assert_eq!(graded.status, "graded 85/100");
        assert_eq!(rows[1].status, "");
    }
}
