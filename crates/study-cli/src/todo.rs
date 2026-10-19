//! `study todo [--all]` — assignments from the index, nearest due first (SPEC §8).

use anyhow::Result;
use study_core::config::Config;
use study_core::index::{Event, EventKind, Index};

use crate::app;
use crate::fmt::{human_until, print_rows};
use crate::sync;

pub async fn run(all: bool) -> Result<()> {
    let config = app::load_config()?;
    let index = app::fresh_index(&config).await?;
    let rows = todo_rows(&index, &config, app::now(), all);
    if rows.is_empty() {
        eprintln!("nothing to do");
    }
    print_rows(&rows);
    Ok(())
}

/// Rows of `due, status, course, title`. Without `all`, done and long-past
/// assignments are left out.
pub fn todo_rows(index: &Index, config: &Config, now: u64, all: bool) -> Vec<Vec<String>> {
    let mut events: Vec<&Event> = index
        .events
        .iter()
        .filter(|event| event.kind == EventKind::Coursework)
        .filter(|event| all || is_open(event, now))
        .collect();
    // `is_none` first so undated assignments sort after every real due date.
    events.sort_by(|a, b| {
        (a.due.is_none(), a.due, &a.title).cmp(&(b.due.is_none(), b.due, &b.title))
    });
    events
        .iter()
        .map(|event| {
            vec![
                event
                    .due
                    .map_or("-".to_string(), |due| human_until(due, now)),
                status_cell(event),
                course_cell(index, config, event, 30),
                event.title.clone(),
            ]
        })
        .collect()
}

fn is_open(event: &Event, now: u64) -> bool {
    if matches!(event.status.as_deref(), Some("submitted" | "graded")) {
        return false;
    }
    sync::is_current(event.due, now)
}

/// Blank for a new or unknown status, so the open work stands out.
fn status_cell(event: &Event) -> String {
    match event.status.as_deref() {
        Some("draft") => "draft".to_string(),
        Some("submitted") => "submitted".to_string(),
        Some("graded") => event.grade.clone().unwrap_or_else(|| "graded".to_string()),
        _ => String::new(),
    }
}

/// The course alias when set, else its name cut to `width` characters.
pub fn course_cell(index: &Index, config: &Config, event: &Event, width: usize) -> String {
    let Some(course_id) = event.course_id.as_deref() else {
        return String::new();
    };
    if let Some(alias) = config.alias_for(course_id) {
        return alias.to_string();
    }
    let Some(course) = index.node(course_id) else {
        return String::new();
    };
    course.name.chars().take(width).collect()
}

#[cfg(test)]
mod tests {
    use study_core::index::{Node, NodeKind, Source, SourceKind};

    use super::*;

    const NOW: u64 = 1_700_000_000;
    const DAY: u64 = 86_400;

    fn event(id: &str, title: &str, due: Option<u64>, status: Option<&str>) -> Event {
        Event {
            id: id.to_string(),
            source: SourceKind::Moodle,
            kind: EventKind::Coursework,
            title: title.to_string(),
            start: None,
            due,
            course_id: Some("moodle:7".to_string()),
            cmid: None,
            status: status.map(str::to_string),
            grade: None,
            url: None,
        }
    }

    fn sample() -> (Index, Config) {
        let mut index = Index::default();
        let course = Node {
            id: "moodle:7".to_string(),
            source: SourceKind::Moodle,
            parent: None,
            kind: NodeKind::Course,
            modname: None,
            name: "Algorithms and Data Structures for Engineers".to_string(),
            remote_id: "7".to_string(),
            web_url: None,
        };
        let mut graded = event("e5", "Graded", Some(NOW - 3 * DAY), Some("graded"));
        graded.grade = Some("85.00 / 100.00".to_string());
        index.replace_source(
            Source {
                kind: SourceKind::Moodle,
                name: "site".to_string(),
                remote_id: "site".to_string(),
                last_sync: Some(NOW),
                delta_token: None,
            },
            vec![course],
            vec![],
            vec![
                event("e1", "Undated", None, Some("new")),
                event("e2", "Soon", Some(NOW + 3 * DAY), None),
                event("e3", "Overdue", Some(NOW - 2 * DAY), Some("draft")),
                event("e4", "Done", Some(NOW + 5 * DAY), Some("submitted")),
                graded,
                event("e6", "Ancient", Some(NOW - 30 * DAY), Some("new")),
                event("e7", "Later", Some(NOW + 20 * DAY), Some("new")),
            ],
        );
        (index, Config::default())
    }

    #[test]
    fn open_work_sorted_by_due_with_undated_last() {
        let (index, config) = sample();
        let rows = todo_rows(&index, &config, NOW, false);
        let titles: Vec<&str> = rows.iter().map(|row| row[3].as_str()).collect();
        assert_eq!(titles, ["Overdue", "Soon", "Later", "Undated"]);
        assert_eq!(rows[0][0], "2d ago");
        assert_eq!(rows[0][1], "draft");
        assert_eq!(rows[1][0], "in 3d");
        assert_eq!(rows[1][1], "");
        assert_eq!(rows[3][0], "-");
        assert_eq!(rows[0][2], "Algorithms and Data Structures");
    }

    #[test]
    fn all_includes_done_and_ancient() {
        let (index, mut config) = sample();
        config.set_alias("moodle:7", "algo");
        let rows = todo_rows(&index, &config, NOW, true);
        let titles: Vec<&str> = rows.iter().map(|row| row[3].as_str()).collect();
        assert_eq!(
            titles,
            [
                "Ancient", "Graded", "Overdue", "Soon", "Done", "Later", "Undated"
            ]
        );
        assert_eq!(rows[1][1], "85.00 / 100.00");
        assert_eq!(rows[4][1], "submitted");
        assert!(rows.iter().all(|row| row[2] == "algo"));
    }
}
