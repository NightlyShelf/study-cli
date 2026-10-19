//! Output helpers: aligned columns on a terminal, TSV when piped (SPEC §3).

use std::io::IsTerminal;

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

/// `5m`, `3h`, `3d`, `2w`, then a plain date; `-` when unknown.
pub fn human_age(mtime: Option<u64>, now: u64) -> String {
    let Some(mtime) = mtime else {
        return "-".to_string();
    };
    match span(now.saturating_sub(mtime)) {
        Some(span) => span,
        None => date(mtime),
    }
}

/// `in 3d`, `in 5h`, `2d ago`, then a plain date either way.
pub fn human_until(due: u64, now: u64) -> String {
    if due >= now {
        return match span(due - now) {
            Some(span) => format!("in {span}"),
            None => date(due),
        };
    }
    match span(now - due) {
        Some(span) => format!("{span} ago"),
        None => date(due),
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

/// `YYYY-MM-DD` from unix seconds. The arithmetic is Howard Hinnant's
/// `civil_from_days`, which saves a date crate for one column.
pub fn date(secs: u64) -> String {
    let days = (secs / 86_400) as i64 + 719_468;
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
    let mut year = year_of_era + era * 400;
    if month <= 2 {
        year += 1;
    }
    format!("{year:04}-{month:02}-{day:02}")
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
        assert_eq!(human_until(now + 300, now), "in 5m");
        assert_eq!(human_until(now + 5 * 3600, now), "in 5h");
        assert_eq!(human_until(now + 3 * 86_400, now), "in 3d");
        assert_eq!(human_until(now + 20 * 86_400, now), "in 2w");
        assert_eq!(human_until(now - 2 * 86_400, now), "2d ago");
        assert_eq!(human_until(now - 90 * 86_400, now), "2023-08-16");
        assert_eq!(human_until(now + 90 * 86_400, now), "2024-02-12");
    }

    #[test]
    fn dates() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(date(1_700_000_000), "2023-11-14");
    }
}
