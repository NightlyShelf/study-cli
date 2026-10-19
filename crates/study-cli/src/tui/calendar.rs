//! The Calendar tab's data: a month of days with their events (SPEC §4,
//! §8), plus the date arithmetic the grid and the keys need. Everything is
//! UTC, like `study cal` on the CLI — one timezone, no date crate.

use study_core::config::Config;
use study_core::index::{Event, EventKind, Index};

use crate::tui::state::course_label;

/// One calendar day of the shown month.
#[derive(Debug, Clone, PartialEq)]
pub struct Day {
    /// Midnight UTC.
    pub secs: u64,
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

/// One `Day` per calendar day of `year`/`month`, each holding the
/// calendar events that start on it.
pub fn month_days_with_events(index: &Index, config: &Config, year: i64, month: u32) -> Vec<Day> {
    let first = days_from_civil(year, month, 1);
    let mut days: Vec<Day> = (0..month_days(year, month))
        .map(|i| Day {
            secs: ((first + i64::from(i)) * 86_400).max(0) as u64,
            events: Vec::new(),
        })
        .collect();
    for event in &index.events {
        if event.kind != EventKind::Calendar {
            continue;
        }
        let Some(start) = event.start else {
            continue;
        };
        // Before the month: negative offset; after it: past the end.
        let Ok(offset) = usize::try_from((start / 86_400) as i64 - first) else {
            continue;
        };
        let Some(day) = days.get_mut(offset) else {
            continue;
        };
        day.events.push(entry(index, config, event, start));
    }
    for day in &mut days {
        day.events
            .sort_by(|a, b| (a.start, &a.title).cmp(&(b.start, &b.title)));
    }
    days
}

fn entry(index: &Index, config: &Config, event: &Event, start: u64) -> CalEntry {
    let course = event
        .course_id
        .as_deref()
        .and_then(|id| index.node(id))
        .map(|node| course_label(config, node))
        .unwrap_or_default();
    let minutes = start % 86_400 / 60;
    CalEntry {
        event_id: event.id.clone(),
        start,
        time: format!("{:02}:{:02}", minutes / 60, minutes % 60),
        kind: event.status.clone().unwrap_or_default(),
        course,
        title: event.title.clone(),
        course_id: event.course_id.clone(),
        url: event.url.clone(),
    }
}

/// `(year, month, day)` of a unix timestamp.
pub fn civil(secs: u64) -> (i64, u32, u32) {
    civil_from_days((secs / 86_400) as i64)
}

/// Howard Hinnant's `civil_from_days`; `days` counts from 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month as u32, day as u32)
}

/// Days since 1970-01-01; Hinnant's `days_from_civil`, the inverse above.
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let shifted_month = if month > 2 {
        i64::from(month) - 3
    } else {
        i64::from(month) + 9
    };
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

pub fn month_days(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        _ => 28,
    }
}

/// 0 = Monday … 6 = Sunday. Day 0, 1970-01-01, was a Thursday.
pub fn weekday(days: i64) -> u32 {
    (days + 3).rem_euclid(7) as u32
}

pub fn add_days(year: i64, month: u32, day: u32, delta: i64) -> (i64, u32, u32) {
    civil_from_days(days_from_civil(year, month, day) + delta)
}

pub fn add_months(year: i64, month: u32, delta: i64) -> (i64, u32) {
    let months = year * 12 + i64::from(month) - 1 + delta;
    (months.div_euclid(12), months.rem_euclid(12) as u32 + 1)
}

pub fn month_name(month: u32) -> &'static str {
    const NAMES: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    NAMES[(month as usize).clamp(1, 12) - 1]
}

#[cfg(test)]
pub mod tests {
    use study_core::index::SourceKind;

    use super::*;
    use crate::tui::tree::tests::sample;

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

    /// `sample()` plus two events on the day holding `now`: "Lab due" in
    /// Algebra at 09:00 and "Exam" in Algorithms at 14:00 (pushed in the
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
    fn civil_and_days_round_trip() {
        for (year, month, day) in [
            (1970, 1, 1),
            (2000, 2, 29),
            (2024, 2, 29),
            (2026, 10, 19),
            (2026, 12, 31),
            (1969, 12, 31),
        ] {
            let days = days_from_civil(year, month, day);
            assert_eq!(civil_from_days(days), (year, month, day));
        }
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(951_782_400), (2000, 2, 29));
        assert_eq!(days_from_civil(1969, 12, 31), -1);
    }

    #[test]
    fn month_lengths_and_weekdays() {
        assert_eq!(month_days(2024, 2), 29);
        assert_eq!(month_days(2025, 2), 28);
        assert_eq!(month_days(1900, 2), 28);
        assert_eq!(month_days(2000, 2), 29);
        assert_eq!(month_days(2026, 10), 31);
        assert_eq!(month_days(2026, 11), 30);
        assert_eq!(weekday(0), 3);
        assert_eq!(weekday(days_from_civil(2026, 10, 19)), 0);
        assert_eq!(weekday(days_from_civil(2026, 10, 18)), 6);
    }

    #[test]
    fn days_and_months_add_across_boundaries() {
        assert_eq!(add_days(2026, 10, 31, 1), (2026, 11, 1));
        assert_eq!(add_days(2026, 1, 1, -1), (2025, 12, 31));
        assert_eq!(add_days(2024, 2, 28, 1), (2024, 2, 29));
        assert_eq!(add_months(2026, 12, 1), (2027, 1));
        assert_eq!(add_months(2026, 1, -1), (2025, 12));
        assert_eq!(add_months(2026, 10, -22), (2024, 12));
        assert_eq!(month_name(10), "October");
    }

    #[test]
    fn events_land_on_their_days_sorted_by_time() {
        let now = days_from_civil(2026, 10, 19) as u64 * 86_400 + 12 * 3600;
        let mut config = Config::default();
        config.set_alias("moodle:1", "algo");
        let days = month_days_with_events(&calendar(now), &config, 2026, 10);
        assert_eq!(days.len(), 31);
        assert_eq!(days[0].secs, days_from_civil(2026, 10, 1) as u64 * 86_400);
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
        let november = month_days_with_events(&calendar(now), &config, 2026, 11);
        assert!(november.iter().all(|day| day.events.is_empty()));
    }
}
