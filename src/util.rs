use chrono::{Datelike, Local, TimeZone};

/// Formats an amount for display, e.g. `1234.5` -> `1,234.50`, `500.0` -> `500`.
pub fn money(value: f64) -> String {
    let negative = value < 0.0;
    let abs = value.abs();
    let rounded = (abs * 100.0).round() / 100.0;
    let whole = rounded.trunc();
    let frac = ((rounded - whole) * 100.0).round() as u64;
    let mut digits = format!("{}", whole as u64);
    let mut grouped = String::new();
    while digits.len() > 3 {
        let split = digits.len() - 3;
        grouped.insert_str(0, &format!(",{}", &digits[split..]));
        digits.truncate(split);
    }
    grouped.insert_str(0, &digits);
    let text = if frac == 0 {
        grouped
    } else {
        format!("{}.{:02}", grouped, frac)
    };
    if negative {
        format!("-{}", text)
    } else {
        text
    }
}

pub const MONTHS: [&str; 12] = [
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

pub fn now_unix() -> u64 {
    Local::now().timestamp().max(0) as u64
}

fn local(unix: u64) -> chrono::DateTime<Local> {
    Local
        .timestamp_opt(unix as i64, 0)
        .single()
        .unwrap_or_else(|| Local.timestamp_opt(0, 0).unwrap())
}

/// Local month key, `YYYY-MM`.
pub fn current_month() -> String {
    month_key(now_unix())
}

pub fn month_key(unix: u64) -> String {
    let now = local(unix);
    format!("{:04}-{:02}", now.year(), now.month())
}

/// `2026-11` -> `November 2026`.
pub fn month_name(key: &str) -> String {
    match parse_key(key) {
        Some((year, month)) => format!("{} {}", MONTHS[(month - 1) as usize], year),
        None => key.to_string(),
    }
}

/// `2026-11` -> `November`.
pub fn month_word(key: &str) -> String {
    match parse_key(key) {
        Some((_, month)) => MONTHS[(month - 1) as usize].to_string(),
        None => key.to_string(),
    }
}

/// `November 2026` -> `2026-11`. A missing year means the current year.
pub fn month_key_from_name(name: &str) -> Option<String> {
    let cleaned = name.replace('\'', "").replace(',', " ");
    let mut parts = cleaned.split_whitespace();
    let first = parts.next()?;
    let month = MONTHS.iter().position(|month| month.eq_ignore_ascii_case(first))? + 1;
    let year = parts
        .next()
        .and_then(|token| {
            token
                .trim_matches(|c: char| !c.is_ascii_digit())
                .parse::<i32>()
                .ok()
        })
        .unwrap_or_else(|| Local::now().year());
    Some(format!("{:04}-{:02}", year, month))
}

fn parse_key(key: &str) -> Option<(i32, u32)> {
    let (year, month) = key.split_once('-')?;
    let year = year.parse::<i32>().ok()?;
    let month = month.parse::<u32>().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }
    Some((year, month))
}

/// The month key preceding `month` (`2026-01` -> `2025-12`).
pub fn previous_month(month: &str) -> String {
    let (year, mon) = parse_key(month).unwrap_or((1970, 1));
    if mon <= 1 {
        format!("{:04}-12", year - 1)
    } else {
        format!("{:04}-{:02}", year, mon - 1)
    }
}

/// Local timestamp for status lines, `YYYY-MM-DD HH:MM`.
pub fn timestamp(unix: u64) -> String {
    local(unix).format("%Y-%m-%d %H:%M").to_string()
}

/// 0 = Sunday, 5 = Friday, 6 = Saturday.
pub fn weekday(unix: u64) -> u32 {
    local(unix).weekday().num_days_from_sunday()
}

pub fn is_friday(unix: u64) -> bool {
    weekday(unix) == 5
}

/// Friday through Sunday: the days a weekly report is due.
pub fn is_report_day(unix: u64) -> bool {
    matches!(weekday(unix), 0 | 5 | 6)
}

pub fn week_of_month(unix: u64) -> u32 {
    let day = local(unix).day();
    (((day - 1) / 7) + 1).min(5)
}
