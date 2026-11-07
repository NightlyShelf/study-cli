//! Output helpers: aligned columns on a terminal, TSV when piped (SPEC §3).

use std::io::IsTerminal;

use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;

pub fn print_rows(rows: &[Vec<String>]) {
    if !std::io::stdout().is_terminal() {
        for row in rows {
            println!("{}", row.join("\t"));
        }
        return;
    }

    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0; columns];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    for row in rows {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate() {
            line.push_str(cell);
            if i + 1 < row.len() {
                line.push_str(&" ".repeat(widths[i] - cell.chars().count() + 2));
            }
        }
        println!("{}", line.trim_end());
    }
}

/// `512`, `2.1K`, `3.4M`, `1.2G`; `-` when unknown.
pub fn human_size(size: Option<u64>) -> String {
    let Some(size) = size else {
        return "-".to_string();
    };
    if size < 1024 {
        return size.to_string();
    }
    let mut value = size as f64 / 1024.0;
    for unit in ["K", "M", "G", "T"] {
        if value < 1024.0 {
            return format!("{value:.1}{unit}");
        }
        value /= 1024.0;
    }
    format!("{value:.1}P")
}

/// `5m`, `3h`, `3d`, `2w`, then a plain date; `-` when unknown. The date is
/// UTC: it is a file's age two months or more back, where a day either way
/// does not matter and a timezone argument through `ls` and `whoami` would.
pub fn human_age(mtime: Option<u64>, now: u64) -> String {
    let Some(mtime) = mtime else {
        return "-".to_string();
    };
    match span(now.saturating_sub(mtime)) {
        Some(span) => span,
        None => date(mtime, &TimeZone::UTC),
    }
}

/// `in 3d`, `in 5h`, `2d ago`, then a plain date in `tz` either way. Due
/// dates are often local midnight, which is the day before in UTC.
pub fn human_until(due: u64, now: u64, tz: &TimeZone) -> String {
    if due >= now {
        return match span(due - now) {
            Some(span) => format!("in {span}"),
            None => date(due, tz),
        };
    }
    match span(now - due) {
        Some(span) => format!("{span} ago"),
        None => date(due, tz),
    }
}

/// `5m`, `3h`, `3d`, `2w`; `None` beyond 60 days, where a date reads better.
fn span(secs: u64) -> Option<String> {
    match secs {
        0..3600 => Some(format!("{}m", secs / 60)),
        3600..86_400 => Some(format!("{}h", secs / 3600)),
        86_400..1_209_600 => Some(format!("{}d", secs / 86_400)),
        1_209_600..5_184_000 => Some(format!("{}w", secs / 604_800)),
        _ => None,
    }
}

/// The wall-clock date and time of unix seconds in `tz`.
pub fn local(secs: u64, tz: &TimeZone) -> DateTime {
    tz.to_datetime(Timestamp::from_second(secs as i64).unwrap_or_default())
}

/// `YYYY-MM-DD` of unix seconds in `tz`.
pub fn date(secs: u64, tz: &TimeZone) -> String {
    local(secs, tz).date().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(human_size(None), "-");
        assert_eq!(human_size(Some(512)), "512");
        assert_eq!(human_size(Some(2150)), "2.1K");
        assert_eq!(human_size(Some(3 * 1024 * 1024 + 400_000)), "3.4M");
        assert_eq!(human_size(Some(1024 * 1024 * 1024)), "1.0G");
    }

    #[test]
    fn ages() {
        let now = 1_700_000_000;
        assert_eq!(human_age(None, now), "-");
        assert_eq!(human_age(Some(now - 300), now), "5m");
        assert_eq!(human_age(Some(now - 3 * 3600), now), "3h");
        assert_eq!(human_age(Some(now - 3 * 86_400), now), "3d");
        assert_eq!(human_age(Some(now - 15 * 86_400), now), "2w");
        assert_eq!(human_age(Some(0), now), "1970-01-01");
    }

    #[test]
    fn due_in() {
        let now = 1_700_000_000;
        let utc = TimeZone::UTC;
        assert_eq!(human_until(now + 300, now, &utc), "in 5m");
        assert_eq!(human_until(now + 5 * 3600, now, &utc), "in 5h");
        assert_eq!(human_until(now + 3 * 86_400, now, &utc), "in 3d");
        assert_eq!(human_until(now + 20 * 86_400, now, &utc), "in 2w");
        assert_eq!(human_until(now - 2 * 86_400, now, &utc), "2d ago");
        assert_eq!(human_until(now - 90 * 86_400, now, &utc), "2023-08-16");
        assert_eq!(human_until(now + 90 * 86_400, now, &utc), "2024-02-12");
        // 22:13 UTC is past midnight two hours east.
        let east = TimeZone::fixed(jiff::tz::offset(2));
        assert_eq!(human_until(now + 90 * 86_400, now, &east), "2024-02-13");
    }

    #[test]
    fn dates_follow_the_timezone() {
        assert_eq!(date(0, &TimeZone::UTC), "1970-01-01");
        assert_eq!(date(951_782_400, &TimeZone::UTC), "2000-02-29");
        assert_eq!(date(1_700_000_000, &TimeZone::UTC), "2023-11-14");
        // 2023-11-14 22:13 UTC is already the 15th two hours east.
        let east = TimeZone::fixed(jiff::tz::offset(2));
        assert_eq!(date(1_700_000_000, &east), "2023-11-15");
        assert_eq!(local(1_700_000_000, &east).hour(), 0);
    }
}
