//! The Calendar tab's data: a month of days with their events (SPEC §4,
//! §8), on the local calendar like `study cal`.

use jiff::ToSpan;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use study_core::config::Config;
use study_core::index::{Event, EventKind, Index};

use crate::fmt::local;
use crate::todo;
use crate::tui::state::course_label;

/// One calendar day of the shown month.
#[derive(Debug, Clone, PartialEq)]
pub struct Day {
    pub date: Date,
    /// Soonest first.
    pub events: Vec<CalEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CalEntry {
    pub event_id: String,
    pub start: u64,
    /// `HH:MM`.
    pub time: String,
    /// Moodle's `eventtype`: `due`, `open`, `close`, `course`, `user`, …
    pub kind: String,
    /// The course alias, or its real name without one; empty when the
    /// event belongs to no course.
    pub course: String,
    pub title: String,
    pub course_id: Option<String>,
    pub url: Option<String>,
}

/// One `Day` per calendar day of the month holding `date`, each with the
/// calendar events that start on it in `tz`.
pub fn month_days_with_events(
    index: &Index,
    config: &Config,
    tz: &TimeZone,
    date: Date,
) -> Vec<Day> {
    let first = date.first_of_month();
    let mut days: Vec<Day> = first
        .series(1.days())
        .take(date.days_in_month() as usize)
        .map(|date| Day {
            date,
            events: Vec::new(),
        })
        .collect();
    for event in &index.events {
        if event.kind != EventKind::Calendar || todo::in_hidden_course(config, event) {
            continue;
        }
        let Some(start) = event.start else {
            continue;
        };
        let at = local(start, tz);
        if at.date().first_of_month() != first {
            continue;
        }
        let time = at.strftime("%H:%M").to_string();
        days[at.day() as usize - 1]
            .events
            .push(entry(index, config, event, start, time));
    }
    for day in &mut days {
        day.events
            .sort_by(|a, b| (a.start, &a.title).cmp(&(b.start, &b.title)));
    }
    days
}

fn entry(index: &Index, config: &Config, event: &Event, start: u64, time: String) -> CalEntry {
    let course = event
        .course_id
        .as_deref()
        .and_then(|id| index.node(id))
        .map(|node| course_label(config, node))
        .unwrap_or_default();
    CalEntry {
        event_id: event.id.clone(),
        start,
        time,
        kind: event.status.clone().unwrap_or_default(),
        course,
        title: event.title.clone(),
        course_id: event.course_id.clone(),
        url: event.url.clone(),
    }
}

#[cfg(test)]
pub mod tests {
    use jiff::civil::date;
    use jiff::tz::{TimeZone, offset};
    use study_core::index::SourceKind;

    use super::*;
    use crate::tui::tree::tests::sample;

    /// 2026-10-19 12:00 UTC, a Monday.
    const NOON: u64 = 1_792_411_200;
    const UTC: TimeZone = TimeZone::UTC;

    pub fn event(id: u64, course: Option<&str>, title: &str, start: u64) -> Event {
        Event {
            id: format!("moodle:cal:{id}"),
            source: SourceKind::Moodle,
            kind: EventKind::Calendar,
            title: title.to_string(),
            start: Some(start),
            due: None,
            course_id: course.map(str::to_string),
            cmid: None,
            status: Some("due".to_string()),
            grade: None,
            url: Some(format!(
                "https://moodle.test/calendar/view.php?view=day&time={start}"
            )),
        }
    }

    /// `sample()` plus two events on the UTC day holding `now`: "Lab due"
    /// in Algebra at 09:00 and "Exam" in Algorithms at 14:00 (pushed in the
    /// other order), and a user event the day after.
    pub fn calendar(now: u64) -> Index {
        let midnight = now - now % 86_400;
        let mut index = sample();
        index
            .events
            .push(event(1, Some("moodle:1"), "Exam", midnight + 14 * 3600));
        index
            .events
            .push(event(2, Some("moodle:2"), "Lab due", midnight + 9 * 3600));
        index
            .events
            .push(event(3, None, "Dentist", midnight + 86_400 + 8 * 3600));
        index
    }

    #[test]
    fn events_land_on_their_days_sorted_by_time() {
        let mut config = Config::default();
        config.set_alias("moodle:1", "algo");
        let days = month_days_with_events(&calendar(NOON), &config, &UTC, date(2026, 10, 19));
        assert_eq!(days.len(), 31);
        assert_eq!(days[0].date, date(2026, 10, 1));
        assert_eq!(days[30].date, date(2026, 10, 31));
        let titles: Vec<&str> = days[18]
            .events
            .iter()
            .map(|entry| entry.title.as_str())
            .collect();
        assert_eq!(titles, ["Lab due", "Exam"]);
        assert_eq!(days[18].events[0].time, "09:00");
        assert_eq!(days[18].events[0].course, "Algebra");
        assert_eq!(days[18].events[1].course, "algo");
        assert_eq!(days[18].events[1].kind, "due");
        assert_eq!(days[19].events[0].title, "Dentist");
        assert_eq!(days[19].events[0].course, "");
        assert!(days[17].events.is_empty());
        // Another month sees none of them.
        let november = month_days_with_events(&calendar(NOON), &config, &UTC, date(2026, 11, 1));
        assert!(november.iter().all(|day| day.events.is_empty()));
    }

    #[test]
    fn hidden_courses_keep_their_events_off_the_grid() {
        let mut config = Config::default();
        config.ui.hidden_courses.push("moodle:2".to_string());
        let days = month_days_with_events(&calendar(NOON), &config, &UTC, date(2026, 10, 1));
        let titles: Vec<&str> = days[18]
            .events
            .iter()
            .map(|entry| entry.title.as_str())
            .collect();
        assert_eq!(titles, ["Exam"]);
        // Events without a course are never hidden.
        assert_eq!(days[19].events[0].title, "Dentist");
    }

    #[test]
    fn events_move_to_the_local_day() {
        // At UTC-10, the 14:00 UTC exam is 04:00 and 08:00 the next day is
        // 22:00 the same day.
        let west = TimeZone::fixed(offset(-10));
        let days = month_days_with_events(
            &calendar(NOON),
            &Config::default(),
            &west,
            date(2026, 10, 1),
        );
        let day: Vec<(&str, &str)> = days[18]
            .events
            .iter()
            .map(|entry| (entry.time.as_str(), entry.title.as_str()))
            .collect();
        assert_eq!(day, [("04:00", "Exam"), ("22:00", "Dentist")]);
        // Lab due, 09:00 UTC on the 19th, is 23:00 on the 18th.
        assert_eq!(days[17].events[0].title, "Lab due");
        assert_eq!(days[17].events[0].time, "23:00");
    }
}
