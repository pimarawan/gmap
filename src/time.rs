use anyhow::{bail, Result};

pub const DAY: i64 = 86_400;
pub const WEEK: i64 = 7 * DAY;
pub const YEAR: i64 = 365 * DAY + DAY / 4;
pub const MONTH: i64 = YEAR / 12;

/// Days since 1970-01-01, by Howard Hinnant's algorithm:
/// http://howardhinnant.github.io/date_algorithms.html
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 }, m, d)
}

pub fn day(secs: i64) -> i64 {
    secs.div_euclid(DAY)
}

/// 0 = Monday.
pub fn weekday(day: i64) -> usize {
    (day + 3).rem_euclid(7) as usize
}

pub fn date(secs: i64) -> String {
    let (y, m, d) = civil_from_days(day(secs));
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn month(secs: i64) -> String {
    let (y, m, _) = civil_from_days(day(secs));
    format!("{y:04}-{m:02}")
}

pub const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// "3 days", "5 months", "2 years". Rounds down.
pub fn span(secs: i64) -> String {
    let s = secs.max(0);
    let (n, unit) = if s < 2 * DAY {
        return if s < DAY { "today".into() } else { "1 day".into() };
    } else if s < 2 * WEEK {
        (s / DAY, "day")
    } else if s < 2 * MONTH {
        (s / WEEK, "week")
    } else if s < 2 * YEAR {
        (s / MONTH, "month")
    } else {
        (s / YEAR, "year")
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// A length of time: `30d`, `6w`, `3m`, `1y`, `18 months`, `2 years`.
pub fn parse_duration(s: &str) -> Result<i64> {
    let s = s.trim().to_ascii_lowercase();
    let s = s.strip_suffix(" ago").unwrap_or(&s).trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: i64 = match num.parse() {
        Ok(n) => n,
        Err(_) => bail!("'{s}' is not a length of time (try 30d, 6w, 3m or 1y)"),
    };
    let unit = match unit.trim() {
        "d" | "day" | "days" => DAY,
        "w" | "week" | "weeks" => WEEK,
        "m" | "mo" | "month" | "months" => MONTH,
        "y" | "year" | "years" => YEAR,
        _ => bail!("'{s}' is not a length of time (try 30d, 6w, 3m or 1y)"),
    };
    Ok(n * unit)
}

/// A point in time: `2024-03-01`, `2024-03`, `2024`, or a duration before `now`.
pub fn parse_when(s: &str, now: i64) -> Result<i64> {
    let t = s.trim();
    let parts: Vec<&str> = t.split('-').collect();
    if parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) && parts[0].len() == 4 {
        let y: i64 = parts[0].parse()?;
        let m: u32 = parts.get(1).map_or(Ok(1), |p| p.parse())?;
        let d: u32 = parts.get(2).map_or(Ok(1), |p| p.parse())?;
        if parts.len() > 3 || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
            bail!("'{s}' is not a date");
        }
        return Ok(days_from_civil(y, m, d) * DAY);
    }
    Ok(now - parse_duration(t)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trip() {
        for z in [-800_000, -1, 0, 1, 11_016, 19_000, 20_454, 2_000_000] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }

    #[test]
    fn weekdays() {
        assert_eq!(weekday(0), 3); // 1970-01-01 was a Thursday
        assert_eq!(weekday(days_from_civil(2026, 11, 9)), 0);
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("30d").unwrap(), 30 * DAY);
        assert_eq!(parse_duration("6 months ago").unwrap(), 6 * MONTH);
        assert_eq!(parse_duration("1y").unwrap(), YEAR);
        assert!(parse_duration("soon").is_err());
        assert!(parse_duration("3 fortnights").is_err());
    }

    #[test]
    fn whens() {
        let now = 1_000 * DAY;
        assert_eq!(parse_when("2024-03-05", now).unwrap(), days_from_civil(2024, 3, 5) * DAY);
        assert_eq!(parse_when("2024", now).unwrap(), days_from_civil(2024, 1, 1) * DAY);
        assert_eq!(parse_when("2w", now).unwrap(), now - 2 * WEEK);
        assert!(parse_when("2024-13", now).is_err());
    }

    #[test]
    fn spans() {
        assert_eq!(span(3600), "today");
        assert_eq!(span(3 * DAY), "3 days");
        assert_eq!(span(3 * WEEK), "3 weeks");
        assert_eq!(span(YEAR + 1), "12 months");
        assert_eq!(span(5 * YEAR), "5 years");
    }
}
