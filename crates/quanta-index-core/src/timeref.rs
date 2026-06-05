use std::time::{SystemTime, UNIX_EPOCH};

pub fn parse_search_timeref_ms(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(ms) = parse_rfc3339_timeref_ms(trimmed) {
        return Some(ms);
    }
    if let Some(ms) = parse_named_month_date_ms(trimmed) {
        return Some(ms);
    }
    if let Some(ms) = parse_human_relative_timeref_ms(trimmed) {
        return Some(ms);
    }
    parse_duration_timeref_ms(trimmed)
}

pub fn parse_rev_at_time_spec(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    let payload = trimmed.strip_prefix("at.time(")?.strip_suffix(')')?;
    let payload = payload.trim();
    (!payload.is_empty()).then_some(payload)
}

pub fn is_rev_at_time_spec(value: &str) -> bool {
    parse_rev_at_time_spec(value).is_some()
}

fn parse_rfc3339_timeref_ms(value: &str) -> Option<u64> {
    if let Some(ms) = parse_rfc3339_datetime_ms(value) {
        return Some(ms);
    }
    parse_rfc3339_date_only_ms(value)
}

fn parse_rfc3339_date_only_ms(value: &str) -> Option<u64> {
    let (year, rest) = parse_year_prefix(value)?;
    let (month, day, rest) = parse_month_day(rest)?;
    if !rest.is_empty() {
        return None;
    }
    unix_ms_from_utc_parts(year, month, day, 0, 0, 0, 0)
}

fn parse_rfc3339_datetime_ms(value: &str) -> Option<u64> {
    let (year, rest) = parse_year_prefix(value)?;
    let (month, day, rest) = parse_month_day(rest)?;
    if !rest.starts_with('T') {
        return None;
    }
    let rest = &rest[1..];
    let (hour, minute, second, fraction_ms, rest) = parse_time_of_day(rest)?;
    if rest != "Z" {
        return None;
    }
    unix_ms_from_utc_parts(year, month, day, hour, minute, second, fraction_ms)
}

fn parse_year_prefix(value: &str) -> Option<(u32, &str)> {
    if value.len() < 5 || !value.as_bytes().get(4).is_some_and(|b| *b == b'-') {
        return None;
    }
    let year = value.get(..4)?.parse().ok()?;
    Some((year, &value[5..]))
}

fn parse_month_day(rest: &str) -> Option<(u32, u32, &str)> {
    if rest.len() < 5 || !rest.as_bytes().get(2).is_some_and(|b| *b == b'-') {
        return None;
    }
    let month = rest.get(..2)?.parse().ok()?;
    let day = rest.get(3..5)?.parse().ok()?;
    Some((month, day, &rest[5..]))
}

fn parse_time_of_day(rest: &str) -> Option<(u32, u32, u32, u32, &str)> {
    if rest.len() < 8 || rest.as_bytes().get(2) != Some(&b':') {
        return None;
    }
    let hour = rest.get(..2)?.parse().ok()?;
    if rest.as_bytes().get(5) != Some(&b':') {
        return None;
    }
    let minute = rest.get(3..5)?.parse().ok()?;
    let mut second_end = 6;
    while second_end < rest.len() && rest.as_bytes()[second_end].is_ascii_digit() {
        second_end += 1;
    }
    let second = rest.get(6..second_end)?.parse().ok()?;
    let mut fraction_ms = 0u32;
    let mut tail = &rest[second_end..];
    if tail.starts_with('.') {
        tail = &tail[1..];
        let mut digits = 0u32;
        let mut places = 0u32;
        for ch in tail.chars() {
            if !ch.is_ascii_digit() {
                break;
            }
            digits = digits
                .saturating_mul(10)
                .saturating_add(u32::from(ch as u8 - b'0'));
            places += 1;
            tail = &tail[ch.len_utf8()..];
        }
        if places == 0 {
            return None;
        }
        while places < 3 {
            digits = digits.saturating_mul(10);
            places += 1;
        }
        fraction_ms = digits;
    }
    Some((hour, minute, second, fraction_ms, tail))
}

fn parse_named_month_date_ms(value: &str) -> Option<u64> {
    let normalized = value
        .trim()
        .trim_end_matches(',')
        .split_whitespace()
        .map(|token| token.trim_end_matches(',').to_ascii_lowercase())
        .collect::<Vec<_>>();
    let [month_name, day_token, year_token] = normalized.as_slice() else {
        return None;
    };
    let month = month_name_to_number(month_name)?;
    let day: u32 = day_token.parse().ok()?;
    let year: u32 = year_token.parse().ok()?;
    unix_ms_from_utc_parts(year, month, day, 0, 0, 0, 0)
}

fn month_name_to_number(value: &str) -> Option<u32> {
    match value {
        "jan" | "january" => Some(1),
        "feb" | "february" => Some(2),
        "mar" | "march" => Some(3),
        "apr" | "april" => Some(4),
        "may" => Some(5),
        "jun" | "june" => Some(6),
        "jul" | "july" => Some(7),
        "aug" | "august" => Some(8),
        "sep" | "sept" | "september" => Some(9),
        "oct" | "october" => Some(10),
        "nov" | "november" => Some(11),
        "dec" | "december" => Some(12),
        _ => None,
    }
}

fn parse_human_relative_timeref_ms(value: &str) -> Option<u64> {
    let normalized = value
        .split_whitespace()
        .map(|token| token.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if normalized.as_slice() == ["yesterday"] {
        return now_ms()?.checked_sub(86_400_000);
    }
    let [amount_token, unit_token, ago_token] = normalized.as_slice() else {
        return None;
    };
    if ago_token != "ago" {
        return None;
    }
    let amount: u64 = amount_token.parse().ok()?;
    let unit_ms = match unit_token.as_str() {
        "second" | "seconds" => 1_000,
        "minute" | "minutes" => 60_000,
        "hour" | "hours" => 3_600_000,
        "day" | "days" => 86_400_000,
        "week" | "weeks" => 604_800_000,
        "month" | "months" => 2_592_000_000,
        "year" | "years" => 31_536_000_000,
        _ => return None,
    };
    now_ms()?.checked_sub(amount.checked_mul(unit_ms)?)
}

fn parse_duration_timeref_ms(value: &str) -> Option<u64> {
    let split_at = value.as_bytes().iter().position(|b| !b.is_ascii_digit())?;
    let (digits, unit) = value.split_at(split_at);
    if digits.is_empty() {
        return None;
    }
    let amount: u64 = digits.parse().ok()?;
    let unit_ms = match unit {
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        "w" => 604_800_000,
        "mo" => 2_592_000_000,
        "y" => 31_536_000_000,
        _ => return None,
    };
    let duration_ms = amount.checked_mul(unit_ms)?;
    now_ms()?.checked_sub(duration_ms)
}

fn now_ms() -> Option<u64> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    u64::try_from(now_ms).ok()
}

fn unix_ms_from_utc_parts(
    year: u32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    fraction_ms: u32,
) -> Option<u64> {
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let days = days_from_civil(year, month, day)?;
    let seconds = u64::from(days)
        .saturating_mul(86_400)
        .saturating_add(u64::from(hour) * 3_600)
        .saturating_add(u64::from(minute) * 60)
        .saturating_add(u64::from(second));
    seconds
        .checked_mul(1_000)?
        .checked_add(u64::from(fraction_ms))
}

fn days_from_civil(year: u32, month: u32, day: u32) -> Option<u32> {
    let mut y = i64::from(year);
    let m = i64::from(month);
    y -= i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2).div_euclid(5) + i64::from(day) - 1;
    let doe = yoe * 365 + yoe.div_euclid(4) - yoe.div_euclid(100) + doy;
    u32::try_from(era * 146_097 + doe - 719_468).ok()
}

#[cfg(test)]
mod tests {
    use super::{is_rev_at_time_spec, parse_rev_at_time_spec, parse_search_timeref_ms};

    #[test]
    fn parses_rfc3339_and_date_only() {
        assert_eq!(
            parse_search_timeref_ms("2024-06-01"),
            Some(1_717_200_000_000)
        );
        assert_eq!(
            parse_search_timeref_ms("2024-06-01T12:34:56Z"),
            Some(1_717_245_296_000)
        );
    }

    #[test]
    fn parses_named_month_dates() {
        assert_eq!(
            parse_search_timeref_ms("june 25 2017"),
            Some(1_498_348_800_000)
        );
        assert_eq!(
            parse_search_timeref_ms("Jun 25, 2017"),
            Some(1_498_348_800_000)
        );
    }

    #[test]
    fn parses_relative_human_phrases() {
        assert!(parse_search_timeref_ms("yesterday").is_some());
        assert!(parse_search_timeref_ms("1 year ago").is_some());
        assert!(parse_search_timeref_ms("3 days ago").is_some());
    }

    #[test]
    fn parses_duration_short_hands() {
        assert!(parse_search_timeref_ms("7d").is_some());
        assert!(parse_search_timeref_ms("1y").is_some());
        assert!(parse_search_timeref_ms("12mo").is_some());
    }

    #[test]
    fn detects_rev_at_time_shape() {
        assert_eq!(
            parse_rev_at_time_spec("at.time(2024-06-01T12:34:56Z)"),
            Some("2024-06-01T12:34:56Z")
        );
        assert_eq!(
            parse_rev_at_time_spec(" at.time( June 25 2017 ) "),
            Some("June 25 2017")
        );
        assert!(is_rev_at_time_spec("at.time(1 year ago)"));
        assert!(!is_rev_at_time_spec("deadbeef"));
        assert!(!is_rev_at_time_spec("at.time()"));
    }
}
