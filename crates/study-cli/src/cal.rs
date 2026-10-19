//! `study cal [--week|--month]` — calendar events from the index, grouped by
//! day (SPEC §8).

use anyhow::Result;
use study_core::config::Config;
use study_core::index::{EventKind, Index};

use crate::app;
use crate::fmt::{date, weekday};
use crate::todo::course_cell;

const DAY: u64 = 86_400;

/// One printed event line.
#[derive(Debug, PartialEq)]
pub struct Row {
    /// `HH:MM`.
    pub time: String,
    /// Moodle's event type: `due`, `open`, `close`, `course`, `user`…
    pub kind: String,
    pub course: String,
    pub title: String,
}

pub async fn run(month: bool) -> Result<()> {
    let config = app::load_config()?;
    let index = app::fresh_index(&config).await?;
    let days = if month { 31 } else { 7 };
    // std has no timezone support, and a date crate for one column is not
    // worth it while the user sits in a single timezone: everything is UTC,
    // including "today", and the output says so once.
    let today = app::now() / DAY * DAY;
    let groups = cal_rows(&index, &config, today, today + days * DAY);
    if groups.is_empty() {
        eprintln!("no events in the next {days} days");
        return Ok(());
    }

    let kind_width = groups
        .iter()
        .flat_map(|(_, rows)| rows)
        .map(|row| row.kind.chars().count())
        .max()
        .unwrap_or(0);
    println!("(times in UTC)");
    for (day, rows) in &groups {
        println!("{} {}", weekday(*day), date(*day));
        for row in rows {
            println!(
                "  {}  {:<kind_width$}  {:<24}  {}",
                row.time, row.kind, row.course, row.title
            );
        }
    }
    Ok(())
}

/// Calendar events starting in `from..to`, grouped by day (unix midnight)
/// in order, and by start time then title within a day. Days without
/// events are left out.
pub fn cal_rows(index: &Index, config: &Config, from: u64, to: u64) -> Vec<(u64, Vec<Row>)> {
    let mut events: Vec<(u64, &study_core::index::Event)> = index
        .events
        .iter()
        .filter(|event| event.kind == EventKind::Calendar)
        .filter_map(|event| event.start.map(|start| (start, event)))
        .filter(|(start, _)| (from..to).contains(start))
        .collect();
    events.sort_by(|a, b| (a.0, &a.1.title).cmp(&(b.0, &b.1.title)));

    let mut groups: Vec<(u64, Vec<Row>)> = Vec::new();
    for (start, event) in events {
        let day = start / DAY * DAY;
        let row = Row {
            time: format!("{:02}:{:02}", start % DAY / 3600, start % 3600 / 60),
            kind: event.status.clone().unwrap_or_default(),
            course: course_cell(index, config, event, 24),
            title: event.title.clone(),
        };
        match groups.last_mut() {
            Some((last, rows)) if *last == day => rows.push(row),
            _ => groups.push((day, vec![row])),
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use study_core::index::{Event, Node, NodeKind, Source, SourceKind};

    use super::*;

    /// 2023-11-14 00:00 UTC, a Tuesday.
    const TODAY: u64 = 1_699_920_000;

    fn event(id: &str, title: &str, kind: EventKind, start: u64, eventtype: &str) -> Event {
        Event {
            id: id.to_string(),
            source: SourceKind::Moodle,
            kind,
            title: title.to_string(),
            start: Some(start),
            due: None,
            course_id: Some("moodle:7".to_string()),
            cmid: None,
            status: Some(eventtype.to_string()),
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
        let mut user = event(
            "e4",
            "Dentist",
            EventKind::Calendar,
            TODAY + DAY + 8 * 3600,
            "user",
        );
        user.course_id = None;
        index.replace_source(
            Source {
                kind: SourceKind::Moodle,
                name: "site".to_string(),
                remote_id: "site".to_string(),
                last_sync: Some(TODAY),
                delta_token: None,
            },
            vec![course],
            vec![],
            vec![
                event(
                    "e1",
                    "Yesterday",
                    EventKind::Calendar,
                    TODAY - 3600,
                    "course",
                ),
                event(
                    "e2",
                    "Quiz closes",
                    EventKind::Calendar,
                    TODAY + 14 * 3600,
                    "close",
                ),
                event(
                    "e3",
                    "Homework due",
                    EventKind::Calendar,
                    TODAY + 9 * 3600 + 30 * 60,
                    "due",
                ),
                user,
                event(
                    "e5",
                    "Also 09:30",
                    EventKind::Calendar,
                    TODAY + 9 * 3600 + 30 * 60,
                    "open",
                ),
                event(
                    "e6",
                    "Next week",
                    EventKind::Calendar,
                    TODAY + 7 * DAY,
                    "course",
                ),
                event(
                    "e7",
                    "Coursework",
                    EventKind::Coursework,
                    TODAY + 3600,
                    "new",
                ),
            ],
        );
        (index, Config::default())
    }

    #[test]
    fn groups_by_day_in_time_order() {
        let (index, config) = sample();
        let groups = cal_rows(&index, &config, TODAY, TODAY + 7 * DAY);

        let days: Vec<u64> = groups.iter().map(|(day, _)| *day).collect();
        assert_eq!(days, [TODAY, TODAY + DAY]);

        let today: Vec<(&str, &str, &str)> = groups[0]
            .1
            .iter()
            .map(|row| (row.time.as_str(), row.kind.as_str(), row.title.as_str()))
            .collect();
        assert_eq!(
            today,
            [
                ("09:30", "open", "Also 09:30"),
                ("09:30", "due", "Homework due"),
                ("14:00", "close", "Quiz closes"),
            ]
        );
        assert_eq!(groups[0].1[0].course, "Algorithms and Data Stru");

        let tomorrow = &groups[1].1;
        assert_eq!(tomorrow.len(), 1);
        assert_eq!(tomorrow[0].time, "08:00");
        assert_eq!(tomorrow[0].kind, "user");
        assert_eq!(tomorrow[0].course, "");
        assert_eq!(tomorrow[0].title, "Dentist");
    }

    #[test]
    fn range_is_half_open_and_uses_aliases() {
        let (index, mut config) = sample();
        config.set_alias("moodle:7", "algo");
        let groups = cal_rows(&index, &config, TODAY, TODAY + 8 * DAY);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[2].0, TODAY + 7 * DAY);
        assert_eq!(groups[2].1[0].title, "Next week");
        assert_eq!(groups[2].1[0].course, "algo");

        assert!(cal_rows(&index, &config, TODAY + 2 * DAY, TODAY + 7 * DAY).is_empty());
    }
}
