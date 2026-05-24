//! `since.time:` and `since.commit:` filter primitives for the
//! semantic adapter.
//!
//! Accepted forms:
//!
//! - `since.time:<u64-millis>` — base-10 UNIX time in milliseconds.
//! - `since.time:<rfc3339>` — wall-clock timestamp in the canonical
//!   subset of RFC3339 that the producer emits:
//!     - `YYYY-MM-DDTHH:MM:SSZ`
//!     - `YYYY-MM-DDTHH:MM:SS.fffZ` (1-9 fractional-second digits)
//!     - `YYYY-MM-DDTHH:MM:SS±HH:MM`
//!     - `YYYY-MM-DDTHH:MM:SS.fff±HH:MM`
//! - `since.commit:<sha>` — opaque commit anchor; this crate does not
//!   resolve git, only forwards the SHA payload.
//!
//! No external `chrono` dep. The RFC3339 parser is hand-rolled in
//! pure integer arithmetic so the resulting millis value is
//! bit-exact across compilers and targets.
//!
//! Failure: any malformed input surfaces as a typed
//! [`SemanticErrorCode::SemInvalidVector`] — this crate reuses that
//! code for the SEM-01 "input did not pass an authoritative gate"
//! shape (the planner/AST surface in higher layers re-wraps it into
//! the DSL-side `PARSE_INVALID_FILTER_VALUE` code).
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use crate::errors::{SemanticError, SemanticErrorCode};

/// Wall-clock milliseconds since the UNIX epoch (UTC). Mirrors the
/// `AppliedAtMs` newtype in `quanta-index-lq-history` so cross-crate
/// values compare without re-implementing the wrapper.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AppliedAtMs(pub u64);

impl AppliedAtMs {
    /// Construct from a raw `u64`.
    #[must_use]
    pub const fn new(v: u64) -> Self {
        Self(v)
    }

    /// Borrow the wrapped `u64`.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Parsed form of a `since.*:` filter input.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ParsedSince {
    /// `since.time:<...>` parsed to a UNIX-epoch millis value.
    Time(AppliedAtMs),
    /// `since.commit:<sha>` parsed. The SHA payload is *not* validated
    /// for hex correctness here — that is the git resolver's job in
    /// higher layers. This crate keeps the input intact for forwarding.
    Commit(Box<str>),
}

/// Parse the right-hand side of a `since.*:` filter.
///
/// Accepts (in order of dispatch):
///
/// 1. `since.time:<u64-millis>` (digits-only payload).
/// 2. `since.time:<rfc3339>` (`YYYY-MM-DD…`).
/// 3. `since.commit:<sha>`.
pub fn parse_since_filter(raw: &str) -> Result<ParsedSince, SemanticError> {
    if let Some(rest) = raw.strip_prefix("since.time:") {
        return parse_time_payload(rest);
    }
    if let Some(rest) = raw.strip_prefix("since.commit:") {
        if rest.is_empty() {
            return Err(SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                "since.commit: empty SHA payload",
            ));
        }
        return Ok(ParsedSince::Commit(rest.into()));
    }
    Err(SemanticError::new(
        SemanticErrorCode::SemInvalidVector,
        format!("since: input `{raw}` did not start with a recognised prefix"),
    ))
}

/// Parse the `since.time:` payload. Tries the digit-only millis form
/// first; if the first byte is not an ASCII digit followed by digits,
/// falls through to the RFC3339 form.
fn parse_time_payload(s: &str) -> Result<ParsedSince, SemanticError> {
    if s.is_empty() {
        return Err(SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            "since.time: empty payload",
        ));
    }
    // Pure-digit fast path (`since.time:1700000000000`).
    if s.bytes().all(|b| b.is_ascii_digit()) {
        let millis = parse_u64(s)?;
        return Ok(ParsedSince::Time(AppliedAtMs(millis)));
    }
    // RFC3339 path.
    let millis = parse_rfc3339_to_millis(s)?;
    Ok(ParsedSince::Time(AppliedAtMs(millis)))
}

/// Hand-rolled non-negative base-10 `u64` parser.
fn parse_u64(s: &str) -> Result<u64, SemanticError> {
    if s.is_empty() {
        return Err(SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            "since: empty u64 payload",
        ));
    }
    let mut acc: u64 = 0;
    for b in s.bytes() {
        if !b.is_ascii_digit() {
            return Err(SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                format!("since: non-digit byte 0x{b:02x} in u64 payload"),
            ));
        }
        let digit = u64::from(b.saturating_sub(b'0'));
        acc = acc.checked_mul(10).ok_or_else(|| {
            SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                "since: u64 payload overflowed",
            )
        })?;
        acc = acc.checked_add(digit).ok_or_else(|| {
            SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                "since: u64 payload overflowed",
            )
        })?;
    }
    Ok(acc)
}

/// Parse exactly `n` ASCII digits from `bytes[offset..]`, returning
/// `(value, new_offset)`. Fails closed if fewer than `n` digits are
/// present or if any byte is not an ASCII digit.
fn take_digits(bytes: &[u8], offset: usize, n: usize) -> Result<(u64, usize), SemanticError> {
    if n == 0 {
        return Ok((0, offset));
    }
    let end = offset.checked_add(n).ok_or_else(|| {
        SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            "since: offset overflow",
        )
    })?;
    if end > bytes.len() {
        return Err(SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            format!("since.time: need {n} digits at offset {offset}, payload too short"),
        ));
    }
    let mut acc: u64 = 0;
    let mut i = offset;
    while i < end {
        let Some(b) = bytes.get(i) else {
            return Err(SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                "since.time: digit read past end",
            ));
        };
        if !b.is_ascii_digit() {
            return Err(SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                format!("since.time: non-digit 0x{b:02x} at offset {i}"),
            ));
        }
        let digit = u64::from(b.saturating_sub(b'0'));
        acc = acc.checked_mul(10).ok_or_else(|| {
            SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                "since.time: digit accumulator overflow",
            )
        })?;
        acc = acc.checked_add(digit).ok_or_else(|| {
            SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                "since.time: digit accumulator overflow",
            )
        })?;
        i = i.checked_add(1).ok_or_else(|| {
            SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                "since.time: offset overflow",
            )
        })?;
    }
    Ok((acc, end))
}

/// Require `bytes[offset]` to equal `expected`. Returns the next
/// offset on success.
fn take_byte(bytes: &[u8], offset: usize, expected: u8) -> Result<usize, SemanticError> {
    match bytes.get(offset) {
        Some(b) if *b == expected => offset.checked_add(1).ok_or_else(|| {
            SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                "since.time: offset overflow",
            )
        }),
        Some(b) => Err(SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            format!("since.time: expected 0x{expected:02x} at offset {offset}, got 0x{b:02x}"),
        )),
        None => Err(SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            format!("since.time: expected 0x{expected:02x} at offset {offset}, past end"),
        )),
    }
}

/// Convert a parsed `(year, month, day, hour, minute, second)` tuple
/// to UNIX-epoch seconds. Implements the proleptic Gregorian
/// calendar in pure integer arithmetic.
///
/// Range: `1970-01-01T00:00:00` .. `9999-12-31T23:59:59`. Below 1970
/// fails closed; values above the upper bound are accepted as long as
/// they fit in `u64` seconds.
fn ymdhms_to_unix_seconds(
    year: u64,
    month: u64,
    day: u64,
    hour: u64,
    minute: u64,
    second: u64,
) -> Result<u64, SemanticError> {
    if year < 1970 {
        return Err(SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            format!("since.time: year {year} before 1970"),
        ));
    }
    if !(1..=12).contains(&month) {
        return Err(SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            format!("since.time: month {month} out of range"),
        ));
    }
    if hour > 23 {
        return Err(SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            format!("since.time: hour {hour} out of range"),
        ));
    }
    if minute > 59 {
        return Err(SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            format!("since.time: minute {minute} out of range"),
        ));
    }
    // RFC3339 accepts leap second :60 in `time-secfrac`, but we are
    // conservative: require [0,59].
    if second > 59 {
        return Err(SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            format!("since.time: second {second} out of range"),
        ));
    }
    // Days-in-month with leap-year handling.
    let dim = days_in_month(year, month)?;
    if day < 1 || day > dim {
        return Err(SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            format!("since.time: day {day} out of range for {year}-{month:02}"),
        ));
    }

    // Days since 1970-01-01 via summing whole years + months.
    let mut days: u64 = 0;
    let mut y: u64 = 1970;
    while y < year {
        let yd = if is_leap_year(y) { 366u64 } else { 365u64 };
        days = days.checked_add(yd).ok_or_else(|| {
            SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                "since.time: days accumulator overflow",
            )
        })?;
        y = y.checked_add(1).ok_or_else(|| {
            SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                "since.time: year overflow",
            )
        })?;
    }
    let mut m: u64 = 1;
    while m < month {
        let md = days_in_month(year, m)?;
        days = days.checked_add(md).ok_or_else(|| {
            SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                "since.time: days accumulator overflow",
            )
        })?;
        m = m.checked_add(1).ok_or_else(|| {
            SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                "since.time: month overflow",
            )
        })?;
    }
    days = days.checked_add(day.saturating_sub(1)).ok_or_else(|| {
        SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            "since.time: days accumulator overflow",
        )
    })?;

    // seconds = (((days * 24) + hour) * 60 + minute) * 60 + second.
    let h_total = days.checked_mul(24).ok_or_else(|| {
        SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            "since.time: hour accumulator overflow",
        )
    })?;
    let h_total = h_total.checked_add(hour).ok_or_else(|| {
        SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            "since.time: hour accumulator overflow",
        )
    })?;
    let m_total = h_total.checked_mul(60).ok_or_else(|| {
        SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            "since.time: minute accumulator overflow",
        )
    })?;
    let m_total = m_total.checked_add(minute).ok_or_else(|| {
        SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            "since.time: minute accumulator overflow",
        )
    })?;
    let s_total = m_total.checked_mul(60).ok_or_else(|| {
        SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            "since.time: second accumulator overflow",
        )
    })?;
    let s_total = s_total.checked_add(second).ok_or_else(|| {
        SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            "since.time: second accumulator overflow",
        )
    })?;
    Ok(s_total)
}

const fn is_leap_year(year: u64) -> bool {
    let by4 = year.wrapping_rem(4) == 0;
    let by100 = year.wrapping_rem(100) == 0;
    let by400 = year.wrapping_rem(400) == 0;
    by4 && (!by100 || by400)
}

fn days_in_month(year: u64, month: u64) -> Result<u64, SemanticError> {
    let v: u64 = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        other => {
            return Err(SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                format!("since.time: month {other} out of range"),
            ));
        }
    };
    Ok(v)
}

/// Parse an RFC3339 timestamp (the bounded subset documented above)
/// to UNIX-epoch milliseconds.
fn parse_rfc3339_to_millis(s: &str) -> Result<u64, SemanticError> {
    let bytes = s.as_bytes();
    let mut off: usize = 0;
    let (year, n) = take_digits(bytes, off, 4)?;
    off = n;
    off = take_byte(bytes, off, b'-')?;
    let (month, n) = take_digits(bytes, off, 2)?;
    off = n;
    off = take_byte(bytes, off, b'-')?;
    let (day, n) = take_digits(bytes, off, 2)?;
    off = n;
    off = take_byte(bytes, off, b'T')?;
    let (hour, n) = take_digits(bytes, off, 2)?;
    off = n;
    off = take_byte(bytes, off, b':')?;
    let (minute, n) = take_digits(bytes, off, 2)?;
    off = n;
    off = take_byte(bytes, off, b':')?;
    let (second, n) = take_digits(bytes, off, 2)?;
    off = n;

    // Optional fractional seconds (1..=9 digits).
    #[expect(
        clippy::useless_let_if_seq,
        reason = "the if-block also mutates `off`; rewriting as an expression would either lose that mutation or duplicate the offset arithmetic"
    )]
    let mut frac_ms: u64 = 0;
    if let Some(b) = bytes.get(off)
        && *b == b'.'
    {
        off = off.checked_add(1).ok_or_else(|| {
            SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                "since.time: offset overflow",
            )
        })?;
        // Count up to 9 fractional digits; we only need the first
        // three for millisecond precision but accept more.
        let start = off;
        let mut count: usize = 0;
        while count < 9 {
            let Some(c) = bytes.get(off) else { break };
            if !c.is_ascii_digit() {
                break;
            }
            off = off.checked_add(1).ok_or_else(|| {
                SemanticError::new(
                    SemanticErrorCode::SemInvalidVector,
                    "since.time: offset overflow",
                )
            })?;
            count = count.checked_add(1).ok_or_else(|| {
                SemanticError::new(
                    SemanticErrorCode::SemInvalidVector,
                    "since.time: count overflow",
                )
            })?;
        }
        if count == 0 {
            return Err(SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                "since.time: trailing '.' with no fractional digits",
            ));
        }
        // Take only the first three digits for millisecond
        // precision; pad with zeros on the right if fewer.
        let take = core::cmp::min(count, 3);
        // Validate the slice still fits without binding the
        // resulting offset (we re-read via `take_digits` below).
        let _frac_end_check: usize = start.checked_add(take).ok_or_else(|| {
            SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                "since.time: frac offset overflow",
            )
        })?;
        let (acc, _next_off) = take_digits(bytes, start, take)?;
        // Pad: if take=3, multiplier=1; take=2 -> 10; take=1 -> 100.
        // `take` is `min(count, 3)` with `count >= 1` (validated above),
        // so the wildcard arm catches the impossible `take == 0` case
        // and any future ceiling raise; we default it to 1 (no padding).
        let pad: u64 = match take {
            1 => 100,
            2 => 10,
            _ => 1,
        };
        frac_ms = acc.checked_mul(pad).ok_or_else(|| {
            SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                "since.time: fractional overflow",
            )
        })?;
    }

    // Timezone: Z | ±HH:MM.
    let tz_offset_seconds: i64 = match bytes.get(off) {
        Some(b'Z') => {
            off = off.checked_add(1).ok_or_else(|| {
                SemanticError::new(
                    SemanticErrorCode::SemInvalidVector,
                    "since.time: offset overflow",
                )
            })?;
            0
        }
        Some(b'+' | b'-') => {
            let Some(sign_byte) = bytes.get(off) else {
                return Err(SemanticError::new(
                    SemanticErrorCode::SemInvalidVector,
                    "since.time: sign read past end",
                ));
            };
            let sign: i64 = if *sign_byte == b'-' { -1 } else { 1 };
            off = off.checked_add(1).ok_or_else(|| {
                SemanticError::new(
                    SemanticErrorCode::SemInvalidVector,
                    "since.time: offset overflow",
                )
            })?;
            let (offset_hours, n) = take_digits(bytes, off, 2)?;
            off = n;
            off = take_byte(bytes, off, b':')?;
            let (offset_minutes, n) = take_digits(bytes, off, 2)?;
            off = n;
            if offset_hours > 23 || offset_minutes > 59 {
                return Err(SemanticError::new(
                    SemanticErrorCode::SemInvalidVector,
                    format!(
                        "since.time: offset {offset_hours:02}:{offset_minutes:02} out of range"
                    ),
                ));
            }
            let hours_i64 = i64::try_from(offset_hours).map_err(|e| {
                SemanticError::new(
                    SemanticErrorCode::SemInvalidVector,
                    format!("since.time: offset hour cast: {e}"),
                )
            })?;
            let minutes_i64 = i64::try_from(offset_minutes).map_err(|e| {
                SemanticError::new(
                    SemanticErrorCode::SemInvalidVector,
                    format!("since.time: offset minute cast: {e}"),
                )
            })?;
            let total_min = hours_i64.checked_mul(60).ok_or_else(|| {
                SemanticError::new(
                    SemanticErrorCode::SemInvalidVector,
                    "since.time: offset overflow",
                )
            })?;
            let total_min = total_min.checked_add(minutes_i64).ok_or_else(|| {
                SemanticError::new(
                    SemanticErrorCode::SemInvalidVector,
                    "since.time: offset overflow",
                )
            })?;
            let total_sec = total_min.checked_mul(60).ok_or_else(|| {
                SemanticError::new(
                    SemanticErrorCode::SemInvalidVector,
                    "since.time: offset overflow",
                )
            })?;
            sign.checked_mul(total_sec).ok_or_else(|| {
                SemanticError::new(
                    SemanticErrorCode::SemInvalidVector,
                    "since.time: offset overflow",
                )
            })?
        }
        Some(b) => {
            return Err(SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                format!("since.time: expected 'Z' or '±HH:MM', got 0x{b:02x}"),
            ));
        }
        None => {
            return Err(SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                "since.time: missing timezone designator",
            ));
        }
    };
    if off != bytes.len() {
        return Err(SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            format!("since.time: trailing bytes at offset {off}"),
        ));
    }

    let local_secs = ymdhms_to_unix_seconds(year, month, day, hour, minute, second)?;
    // wall_clock_utc_secs = local_secs - tz_offset_secs
    let local_i64 = i64::try_from(local_secs).map_err(|e| {
        SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            format!("since.time: local secs cast: {e}"),
        )
    })?;
    let utc_i64 = local_i64.checked_sub(tz_offset_seconds).ok_or_else(|| {
        SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            "since.time: UTC subtract overflow",
        )
    })?;
    if utc_i64 < 0 {
        return Err(SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            "since.time: timezone offset pushed result before 1970",
        ));
    }
    let utc_secs = u64::try_from(utc_i64).map_err(|e| {
        SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            format!("since.time: utc cast: {e}"),
        )
    })?;
    let millis = utc_secs.checked_mul(1000).ok_or_else(|| {
        SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            "since.time: millis multiply overflow",
        )
    })?;
    millis.checked_add(frac_ms).ok_or_else(|| {
        SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            "since.time: millis add overflow",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{
        AppliedAtMs, ParsedSince, is_leap_year, parse_rfc3339_to_millis, parse_since_filter,
        parse_u64, ymdhms_to_unix_seconds,
    };
    use crate::errors::SemanticErrorCode;

    #[test]
    fn millis_form_round_trip() {
        match parse_since_filter("since.time:1700000000000") {
            Ok(ParsedSince::Time(t)) => assert_eq!(t, AppliedAtMs(1_700_000_000_000)),
            Ok(ParsedSince::Commit(_)) => assert!(false, "wrong variant"),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn rfc3339_utc_no_fraction() {
        // 2024-01-15T12:34:56Z = 1705322096 sec (verified via POSIX `date -u`)
        match parse_since_filter("since.time:2024-01-15T12:34:56Z") {
            Ok(ParsedSince::Time(t)) => assert_eq!(t, AppliedAtMs(1_705_322_096_000)),
            Ok(ParsedSince::Commit(_)) => assert!(false, "wrong variant"),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn rfc3339_utc_with_milliseconds() {
        // 2024-01-15T12:34:56.789Z = 1705322096789 ms
        match parse_since_filter("since.time:2024-01-15T12:34:56.789Z") {
            Ok(ParsedSince::Time(t)) => assert_eq!(t, AppliedAtMs(1_705_322_096_789)),
            Ok(ParsedSince::Commit(_)) => assert!(false, "wrong variant"),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn rfc3339_positive_offset() {
        // 2024-01-15T12:34:56+09:00 = 1705289696 sec UTC
        match parse_since_filter("since.time:2024-01-15T12:34:56+09:00") {
            Ok(ParsedSince::Time(t)) => assert_eq!(t, AppliedAtMs(1_705_289_696_000)),
            Ok(ParsedSince::Commit(_)) => assert!(false, "wrong variant"),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn rfc3339_negative_offset() {
        // 2024-01-15T12:34:56-05:00 = 1705340096 sec UTC
        match parse_since_filter("since.time:2024-01-15T12:34:56-05:00") {
            Ok(ParsedSince::Time(t)) => assert_eq!(t, AppliedAtMs(1_705_340_096_000)),
            Ok(ParsedSince::Commit(_)) => assert!(false, "wrong variant"),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn rfc3339_one_fractional_digit_padded() {
        // .5 -> 500 ms
        match parse_since_filter("since.time:2024-01-15T12:34:56.5Z") {
            Ok(ParsedSince::Time(t)) => assert_eq!(t, AppliedAtMs(1_705_322_096_500)),
            Ok(ParsedSince::Commit(_)) => assert!(false, "wrong variant"),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn rfc3339_six_fractional_digits_truncated_to_ms() {
        // .123456 -> 123 ms (truncate, do not round)
        match parse_since_filter("since.time:2024-01-15T12:34:56.123456Z") {
            Ok(ParsedSince::Time(t)) => assert_eq!(t, AppliedAtMs(1_705_322_096_123)),
            Ok(ParsedSince::Commit(_)) => assert!(false, "wrong variant"),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn rfc3339_rejects_missing_tz() {
        match parse_since_filter("since.time:2024-01-15T12:34:56") {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn rfc3339_rejects_garbage_trailing() {
        match parse_since_filter("since.time:2024-01-15T12:34:56Zxxx") {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn rfc3339_rejects_bad_month() {
        match parse_since_filter("since.time:2024-13-15T12:34:56Z") {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn rfc3339_rejects_bad_day_for_feb_non_leap() {
        match parse_since_filter("since.time:2023-02-29T00:00:00Z") {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn rfc3339_accepts_feb_29_on_leap_year() {
        match parse_since_filter("since.time:2024-02-29T00:00:00Z") {
            Ok(ParsedSince::Time(_)) => {}
            Ok(ParsedSince::Commit(_)) => assert!(false, "wrong variant"),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn rfc3339_rejects_bad_hour() {
        match parse_since_filter("since.time:2024-01-15T24:00:00Z") {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn rfc3339_rejects_year_before_1970() {
        match parse_since_filter("since.time:1969-12-31T23:59:59Z") {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn rfc3339_rejects_dot_without_digits() {
        match parse_since_filter("since.time:2024-01-15T12:34:56.Z") {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn rfc3339_rejects_bad_offset_format() {
        match parse_since_filter("since.time:2024-01-15T12:34:56+0900") {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn epoch_zero() {
        match parse_since_filter("since.time:1970-01-01T00:00:00Z") {
            Ok(ParsedSince::Time(t)) => assert_eq!(t, AppliedAtMs(0)),
            Ok(ParsedSince::Commit(_)) => assert!(false, "wrong variant"),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn commit_form_parsed() {
        match parse_since_filter("since.commit:abcdef0123456789") {
            Ok(ParsedSince::Commit(s)) => assert_eq!(s.as_ref(), "abcdef0123456789"),
            Ok(ParsedSince::Time(_)) => assert!(false, "wrong variant"),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn commit_form_rejects_empty() {
        match parse_since_filter("since.commit:") {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn unknown_prefix_fails() {
        match parse_since_filter("until.time:42") {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn parse_u64_rejects_overflow() {
        match parse_u64("18446744073709551616") {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn leap_year_helper() {
        assert!(is_leap_year(2000));
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(2023));
        assert!(!is_leap_year(1900));
        assert!(is_leap_year(2400));
    }

    #[test]
    fn ymdhms_epoch_zero() {
        let v = match ymdhms_to_unix_seconds(1970, 1, 1, 0, 0, 0) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(v, 0);
    }

    #[test]
    fn ymdhms_known_value() {
        // 2000-01-01T00:00:00 UTC = 946684800 seconds.
        let v = match ymdhms_to_unix_seconds(2000, 1, 1, 0, 0, 0) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(v, 946_684_800);
    }

    #[test]
    fn rfc3339_known_unix_value_through_helper() {
        // Direct call to bypass the parse layer.
        let ms = match parse_rfc3339_to_millis("2024-06-15T08:30:00Z") {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        // 2024-06-15T08:30:00 UTC = 1718440200 sec.
        assert_eq!(ms, 1_718_440_200_000);
    }
}
