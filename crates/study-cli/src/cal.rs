//! `study cal [--week|--month]` — calendar events from the index, grouped by
//! day (SPEC §8).

use anyhow::Result;
use jiff::ToSpan;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use study_core::config::Config;
use study_core::index::{EventKind, Index};

use crate::app;
use crate::fmt::local;
use crate::todo::{course_cell, in_hidden_course};

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
    let tz = TimeZone::system();
    let today = local(app::now(), &tz).date();
    let groups = cal_rows(&index, &config, &tz, today, today.checked_add(days.days())?);
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
    for (day, rows) in &groups {
        println!("{}", day.strftime("%a %Y-%m-%d"));
        for row in rows {
            println!(
                "  {}  {:<kind_width$}  {:<24}  {}",
                row.time, row.kind, row.course, row.title
            );
        }
    }
    Ok(())
}

/// Calendar events starting on the days `from..to` in `tz`, grouped by day
/// in order, and by start time then title within a day. Days without
/// events are left out.
pub fn cal_rows(
    index: &Index,
    config: &Config,
    tz: &TimeZone,
    from: Date,
    to: Date,
) -> Vec<(Date, Vec<Row>)> {
    let mut events: Vec<(u64, &study_core::index::Event)> = index
        .events
        .iter()
        .filter(|event| event.kind == EventKind::Calendar)
        .filter(|event| !in_hidden_course(config, event))
        .filter_map(|event| event.start.map(|start| (start, event)))
        .filter(|(start, _)| (from..to).contains(&local(*start, tz).date()))
        .collect();
    events.sort_by(|a, b| (a.0, &a.1.title).cmp(&(b.0, &b.1.title)));

    let mut groups: Vec<(Date, Vec<Row>)> = Vec::new();
    for (start, event) in events {
        let start = local(start, tz);
        let day = start.date();
        let row = Row {
            time: start.strftime("%H:%M").to_string(),
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
    use jiff::civil::date;
    use study_core::index::{Event, Node, NodeKind, Source, SourceKind};

    use super::*;

    /// 2023-11-14 00:00 UTC, a Tuesday.
    const TODAY: u64 = 1_699_920_000;
    const DAY: u64 = 86_400;

    /// `cal_rows` in UTC from 2023-11-14 for `days` days.
    fn utc_rows(index: &Index, config: &Config, skip: i64, days: i64) -> Vec<(Date, Vec<Row>)> {
        let from = date(2023, 11, 14) + skip.days();
        cal_rows(index, config, &TimeZone::UTC, from, from + days.days())
    }

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
        let groups = utc_rows(&index, &config, 0, 7);

        let days: Vec<Date> = groups.iter().map(|(day, _)| *day).collect();
        assert_eq!(days, [date(2023, 11, 14), date(2023, 11, 15)]);

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
        let groups = utc_rows(&index, &config, 0, 8);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[2].0, date(2023, 11, 21));
        assert_eq!(groups[2].1[0].title, "Next week");
        assert_eq!(groups[2].1[0].course, "algo");

        assert!(utc_rows(&index, &config, 2, 5).is_empty());
    }

    #[test]
    fn hidden_courses_keep_only_courseless_events() {
        let (index, mut config) = sample();
        config.ui.hidden_courses.push("moodle:7".to_string());
        let groups = utc_rows(&index, &config, 0, 7);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, date(2023, 11, 15));
        assert_eq!(groups[0].1[0].title, "Dentist");
    }

    #[test]
    fn days_and_times_follow_the_timezone() {
        let (index, config) = sample();
        let east = TimeZone::fixed(jiff::tz::offset(10));
        let today = date(2023, 11, 14);
        let groups = cal_rows(&index, &config, &east, today, today + 1.days());
        let rows: Vec<(&str, &str)> = groups[0]
            .1
            .iter()
            .map(|row| (row.time.as_str(), row.title.as_str()))
            .collect();
        // Yesterday 23:00 UTC is 09:00 here, and 14:00 UTC already tomorrow.
        assert_eq!(
            rows,
            [
                ("09:00", "Yesterday"),
                ("19:30", "Also 09:30"),
                ("19:30", "Homework due")
            ]
        );
    }

    #[test]
    fn summer_time_moves_the_clock_not_the_day() {
        // Warsaw's rules as a POSIX TZ string, so the test needs no tzdb.
        let warsaw = TimeZone::posix("CET-1CEST,M3.5.0,M10.5.0/3").unwrap();
        let mut index = Index::default();
        // 2026-03-28 and 03-29 at 10:00 UTC, either side of the switch.
        for (id, start) in [("winter", 1_774_692_000), ("summer", 1_774_778_400)] {
            let mut event = event(id, id, EventKind::Calendar, start, "user");
            event.course_id = None;
            index.events.push(event);
        }
        let from = date(2026, 3, 28);
        let groups = cal_rows(&index, &Config::default(), &warsaw, from, from + 2.days());
        assert_eq!(groups[0].0, from);
        assert_eq!(groups[0].1[0].time, "11:00");
        assert_eq!(groups[1].0, date(2026, 3, 29));
        assert_eq!(groups[1].1[0].time, "12:00");
    }
}
