use std::time::{SystemTime, UNIX_EPOCH};

/// Parse a search time reference against an explicit `now`.
///
/// The deterministic owner (TOPT-01 / PO-1). Absolute shapes ignore
/// `now_ms`; relative shapes subtract from it with checked arithmetic,
/// so zero yields `now_ms` and underflow yields `None`.
#[must_use]
pub fn parse_search_timeref_ms_at(value: &str, now_ms: u64) -> Option<u64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(ms) = parse_absolute_timeref_ms(trimmed) {
        return Some(ms);
    }
    if let Some(ms) = parse_human_relative_timeref_ms_at(trimmed, now_ms) {
        return Some(ms);
    }
    parse_duration_timeref_ms_at(trimmed, now_ms)
}

#[must_use]
pub fn parse_search_timeref_ms(value: &str) -> Option<u64> {
    // The composition edge: absolute shapes parse without any clock, and
    // relative shapes sample the wall clock exactly once. A clock that
    // reads before the epoch still parses absolute shapes, exactly as
    // before; only relative shapes need `now`.
    if let Some(now_ms) = now_ms() {
        return parse_search_timeref_ms_at(value, now_ms);
    }
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    parse_absolute_timeref_ms(trimmed)
}

/// Absolute shapes: RFC-3339 and named-month dates. Clock-free.
fn parse_absolute_timeref_ms(trimmed: &str) -> Option<u64> {
    if let Some(ms) = parse_rfc3339_timeref_ms(trimmed) {
        return Some(ms);
    }
    parse_named_month_date_ms(trimmed)
}

#[must_use]
pub fn parse_rev_at_time_spec(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    let payload = trimmed.strip_prefix("at.time(")?.strip_suffix(')')?;
    let payload = payload.trim();
    (!payload.is_empty()).then_some(payload)
}

#[must_use]
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
    let rest = rest.strip_prefix('T')?;
    let (hour, minute, second, fraction_ms, rest) = parse_time_of_day(rest)?;
    if rest != "Z" {
        return None;
    }
    unix_ms_from_utc_parts(year, month, day, hour, minute, second, fraction_ms)
}

fn parse_year_prefix(value: &str) -> Option<(u32, &str)> {
    if value.as_bytes().get(4) != Some(&b'-') {
        return None;
    }
    let Ok(year) = value.get(..4)?.parse::<u32>() else {
        return None;
    };
    Some((year, value.get(5..)?))
}

fn parse_month_day(rest: &str) -> Option<(u32, u32, &str)> {
    if rest.as_bytes().get(2) != Some(&b'-') {
        return None;
    }
    let Ok(month) = rest.get(..2)?.parse::<u32>() else {
        return None;
    };
    let Ok(day) = rest.get(3..5)?.parse::<u32>() else {
        return None;
    };
    Some((month, day, rest.get(5..)?))
}

fn parse_time_of_day(rest: &str) -> Option<(u32, u32, u32, u32, &str)> {
    if rest.as_bytes().get(2) != Some(&b':') {
        return None;
    }
    let Ok(hour) = rest.get(..2)?.parse::<u32>() else {
        return None;
    };
    if rest.as_bytes().get(5) != Some(&b':') {
        return None;
    }
    let Ok(minute) = rest.get(3..5)?.parse::<u32>() else {
        return None;
    };
    let mut second_end = 6;
    while rest
        .as_bytes()
        .get(second_end)
        .is_some_and(u8::is_ascii_digit)
    {
        second_end = second_end.saturating_add(1);
    }
    let Ok(second) = rest.get(6..second_end)?.parse::<u32>() else {
        return None;
    };
    let mut fraction_ms = 0u32;
    let mut tail = rest.get(second_end..)?;
    if let Some(after_dot) = tail.strip_prefix('.') {
        tail = after_dot;
        let mut digits = 0u32;
        let mut places = 0u32;
        for ch in tail.chars() {
            let Some(digit) = ch.to_digit(10) else {
                break;
            };
            digits = digits.saturating_mul(10).saturating_add(digit);
            places = places.saturating_add(1);
            tail = tail.get(ch.len_utf8()..)?;
        }
        if places == 0 {
            return None;
        }
        while places < 3 {
            digits = digits.saturating_mul(10);
            places = places.saturating_add(1);
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
    let (Ok(day), Ok(year)) = (day_token.parse::<u32>(), year_token.parse::<u32>()) else {
        return None;
    };
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

fn parse_human_relative_timeref_ms_at(value: &str, now_ms: u64) -> Option<u64> {
    let normalized = value
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    if normalized.as_slice() == ["yesterday"] {
        return now_ms.checked_sub(86_400_000);
    }
    let [amount_token, unit_token, ago_token] = normalized.as_slice() else {
        return None;
    };
    if ago_token != "ago" {
        return None;
    }
    let Ok(amount) = amount_token.parse::<u64>() else {
        return None;
    };
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
    now_ms.checked_sub(amount.checked_mul(unit_ms)?)
}

fn parse_duration_timeref_ms_at(value: &str, now_ms: u64) -> Option<u64> {
    let split_at = value.as_bytes().iter().position(|b| !b.is_ascii_digit())?;
    let (digits, unit) = value.split_at(split_at);
    if digits.is_empty() {
        return None;
    }
    let Ok(amount) = digits.parse::<u64>() else {
        return None;
    };
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
    now_ms.checked_sub(duration_ms)
}

fn now_ms() -> Option<u64> {
    let Ok(elapsed) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return None;
    };
    let Ok(ms) = u64::try_from(elapsed.as_millis()) else {
        return None;
    };
    Some(ms)
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
        .saturating_add(u64::from(hour).saturating_mul(3_600))
        .saturating_add(u64::from(minute).saturating_mul(60))
        .saturating_add(u64::from(second));
    seconds
        .checked_mul(1_000)?
        .checked_add(u64::from(fraction_ms))
}

fn days_from_civil(year: u32, month: u32, day: u32) -> Option<u32> {
    let m = i64::from(month);
    let y = i64::from(year).checked_sub(i64::from(m <= 2))?;
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    // Howard Hinnant's days_from_civil, with every step checked so an
    // out-of-range date yields `None` rather than overflowing.
    let month_offset = if m > 2 {
        m.checked_sub(3)?
    } else {
        m.checked_add(9)?
    };
    let doy = 153i64
        .checked_mul(month_offset)?
        .checked_add(2)?
        .div_euclid(5)
        .checked_add(i64::from(day))?
        .checked_sub(1)?;
    let doe = yoe
        .checked_mul(365)?
        .checked_add(yoe.div_euclid(4))?
        .checked_sub(yoe.div_euclid(100))?
        .checked_add(doy)?;
    let days = era
        .checked_mul(146_097)?
        .checked_add(doe)?
        .checked_sub(719_468)?;
    let Ok(days) = u32::try_from(days) else {
        return None;
    };
    Some(days)
}

#[cfg(test)]
mod tests {
    use super::{
        is_rev_at_time_spec, parse_rev_at_time_spec, parse_search_timeref_ms,
        parse_search_timeref_ms_at,
    };

    /// Fixed `now` for the deterministic matrix: 2023-11-14T22:13:20Z.
    const FIXED_NOW_MS: u64 = 1_700_000_000_000;

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
    fn relative_units_subtract_exactly_at_fixed_time() {
        // Every human unit, singular and plural, at fixed `now`.
        let cases = [
            ("1 second ago", 1_000),
            ("2 seconds ago", 2_000),
            ("1 minute ago", 60_000),
            ("2 minutes ago", 120_000),
            ("1 hour ago", 3_600_000),
            ("3 hours ago", 10_800_000),
            ("1 day ago", 86_400_000),
            ("3 days ago", 259_200_000),
            ("1 week ago", 604_800_000),
            ("2 weeks ago", 1_209_600_000),
            ("1 month ago", 2_592_000_000),
            ("12 months ago", 31_104_000_000),
            ("1 year ago", 31_536_000_000),
            ("2 years ago", 63_072_000_000),
            ("yesterday", 86_400_000),
        ];
        for (text, delta_ms) in cases {
            assert_eq!(
                parse_search_timeref_ms_at(text, FIXED_NOW_MS),
                Some(FIXED_NOW_MS.saturating_sub(delta_ms)),
                "human phrase {text:?} subtracts exactly"
            );
        }
        // Every duration shorthand at fixed `now`.
        let shorthands = [
            ("1s", 1_000),
            ("30s", 30_000),
            ("1m", 60_000),
            ("5m", 300_000),
            ("1h", 3_600_000),
            ("7d", 604_800_000),
            ("2w", 1_209_600_000),
            ("1mo", 2_592_000_000),
            ("12mo", 31_104_000_000),
            ("1y", 31_536_000_000),
        ];
        for (text, delta_ms) in shorthands {
            assert_eq!(
                parse_search_timeref_ms_at(text, FIXED_NOW_MS),
                Some(FIXED_NOW_MS.saturating_sub(delta_ms)),
                "duration {text:?} subtracts exactly"
            );
        }
    }

    #[test]
    fn zero_durations_yield_now_and_underflow_yields_none() {
        assert_eq!(
            parse_search_timeref_ms_at("0s", FIXED_NOW_MS),
            Some(FIXED_NOW_MS),
            "a zero duration is exactly now"
        );
        assert_eq!(
            parse_search_timeref_ms_at("0 seconds ago", FIXED_NOW_MS),
            Some(FIXED_NOW_MS),
            "a zero human amount is exactly now"
        );
        // Underflow past the epoch is `None`, never a wrap.
        assert_eq!(parse_search_timeref_ms_at("7d", 0), None);
        assert_eq!(parse_search_timeref_ms_at("1 second ago", 999), None);
        assert_eq!(parse_search_timeref_ms_at("yesterday", 86_399_999), None);
        // Amount/unit multiplication overflow is `None`, never a wrap.
        assert_eq!(
            parse_search_timeref_ms_at("18446744073709551615y", FIXED_NOW_MS),
            None
        );
        assert_eq!(
            parse_search_timeref_ms_at("18446744073709551615 years ago", FIXED_NOW_MS),
            None
        );
        // Exact-boundary subtraction still yields `Some(0)`.
        assert_eq!(parse_search_timeref_ms_at("1000s", 1_000_000), Some(0));
    }

    #[test]
    fn absolute_shapes_ignore_the_clock() {
        assert_eq!(
            parse_search_timeref_ms_at("2024-06-01", 0),
            Some(1_717_200_000_000)
        );
        assert_eq!(
            parse_search_timeref_ms_at("june 25 2017", u64::MAX),
            Some(1_498_348_800_000)
        );
        assert_eq!(parse_search_timeref_ms_at("", FIXED_NOW_MS), None);
        assert_eq!(parse_search_timeref_ms_at("   ", FIXED_NOW_MS), None);
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
