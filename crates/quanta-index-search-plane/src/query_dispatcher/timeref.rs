//! Timeref parsing (RFC 3339, relative durations) and runtime scope parsing.

use quanta_index_core::CoreError;

use crate::query_dispatcher::errors::{history_invalid_timeref, runtime_invalid_scope};

pub(super) fn parse_runtime_changed_scope_ms(scope: &str) -> Result<u64, CoreError> {
    let Some(timeref) = scope.strip_prefix("since=") else {
        return Err(runtime_invalid_scope(format!(
            "runtime metadata: changed scope `{scope}` must use since=<timeref>"
        )));
    };
    parse_history_timeref_ms(timeref).map_err(|err| {
        runtime_invalid_scope(format!(
            "runtime metadata: changed scope timeref `{timeref}` is not a valid RFC3339 timestamp or duration: {err}"
        ))
    })
}

pub(super) fn parse_runtime_stale_scope_ms(scope: &str) -> Result<u64, CoreError> {
    let Some(timeref) = scope.strip_prefix("before=") else {
        return Err(runtime_invalid_scope(format!(
            "runtime metadata: stale scope `{scope}` must use before=<timeref>"
        )));
    };
    parse_history_timeref_ms(timeref).map_err(|err| {
        runtime_invalid_scope(format!(
            "runtime metadata: stale scope timeref `{timeref}` is not a valid RFC3339 timestamp or duration: {err}"
        ))
    })
}

pub(super) fn unix_seconds_from_ms(ms: u64) -> i64 {
    i64::try_from(ms.div_euclid(1_000)).map_or(i64::MAX, core::convert::identity)
}

pub(super) fn parse_history_timeref_ms(value: &str) -> Result<u64, CoreError> {
    if let Some(ms) = parse_rfc3339_timeref_ms(value) {
        return Ok(ms);
    }
    if let Some(ms) = parse_duration_timeref_ms(value) {
        return Ok(ms);
    }
    Err(history_invalid_timeref(format!(
        "history: timeref `{value}` is not a valid RFC3339 timestamp or duration"
    )))
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
    if value.len() < 5 || value.as_bytes().get(4) != Some(&b'-') {
        return None;
    }
    let Ok(year) = value.get(..4)?.parse::<u32>() else {
        return None;
    };
    Some((year, value.get(5..)?))
}

fn parse_month_day(rest: &str) -> Option<(u32, u32, &str)> {
    if rest.len() < 5 || rest.as_bytes().get(2) != Some(&b'-') {
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
    if rest.len() < 8 || rest.as_bytes().get(2) != Some(&b':') {
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
    let mut second_end = 6usize;
    while rest
        .as_bytes()
        .get(second_end)
        .is_some_and(u8::is_ascii_digit)
    {
        second_end = second_end.checked_add(1)?;
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
            places = places.checked_add(1)?;
            tail = tail.get(ch.len_utf8()..)?;
        }
        if places == 0 {
            return None;
        }
        while places < 3 {
            digits = digits.saturating_mul(10);
            places = places.checked_add(1)?;
        }
        fraction_ms = digits;
    }
    Some((hour, minute, second, fraction_ms, tail))
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
    let mut y = i64::from(year);
    let m = i64::from(month);
    y = y.checked_sub(i64::from(m <= 2))?;
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let month_shift = if m > 2 { -3 } else { 9 };
    let doy = m
        .checked_add(month_shift)?
        .checked_mul(153)?
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
    let Ok(value) = u32::try_from(days) else {
        return None;
    };
    Some(value)
}

fn parse_duration_timeref_ms(value: &str) -> Option<u64> {
    let split_at = value.as_bytes().iter().position(|b| !b.is_ascii_digit())?;
    let (digits, unit) = value.split_at(split_at);
    if digits.is_empty() {
        return None;
    }
    let Ok(amount) = digits.parse::<u64>() else {
        return None;
    };
    let unit_ms: u64 = match unit {
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
    let Ok(elapsed) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else {
        return None;
    };
    let Ok(now_ms) = u64::try_from(elapsed.as_millis()) else {
        return None;
    };
    now_ms.checked_sub(duration_ms)
}
