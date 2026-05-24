//! `since.time:` and `since.commit:` filter primitives.
//!
//! [`parse_since_filter`] accepts:
//!
//! - `since.time:<millis>` where `<millis>` is a base-10 `u64` UNIX time in
//!   milliseconds. v1 deliberately rejects RFC3339; RFC3339 ingestion is
//!   deferred until a hand-rolled parser lands (no `chrono` etc). Document:
//!   the millis form is the canonical wire shape for trace records, so v1
//!   never has to disambiguate.
//! - `since.commit:<40-hex-sha>` for an explicit commit anchor.
//!
//! [`since_time`] returns every commit with `applied_at_ms >= threshold`,
//! sorted ascending by sha. Wall-clock `now()` is never consulted — the
//! threshold is whatever the caller supplies (typically pulled from a write-
//! packet trace).
//!
//! Failure: malformed prefix or invalid numeric/sha payload ->
//! [`HistoryErrorCode::HistoryRefNotFound`]. (Reusing `HistoryRefNotFound`
//! mirrors the spec's reuse for "input does not resolve to a known anchor".)

use crate::commit_graph::CommitGraph;
use crate::errors::{HistoryError, HistoryErrorCode};
use crate::types::{AppliedAtMs, CommitSha};

/// Parsed form of a `since.*:` filter input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParsedSince {
    /// `since.time:<millis>` parsed.
    Time(AppliedAtMs),
    /// `since.commit:<sha>` parsed.
    Commit(CommitSha),
}

/// Parse the right-hand side of a `since.*:` filter.
///
/// Accepts:
/// - `since.time:<u64-millis>` -> [`ParsedSince::Time`]
/// - `since.commit:<40-hex>` -> [`ParsedSince::Commit`]
pub fn parse_since_filter(raw: &str) -> Result<ParsedSince, HistoryError> {
    if let Some(rest) = raw.strip_prefix("since.time:") {
        let millis = parse_u64(rest)?;
        return Ok(ParsedSince::Time(AppliedAtMs::new(millis)));
    }
    if let Some(rest) = raw.strip_prefix("since.commit:") {
        let sha = CommitSha::parse_hex(rest)?;
        return Ok(ParsedSince::Commit(sha));
    }
    Err(HistoryError::new(
        HistoryErrorCode::HistoryRefNotFound,
        format!("since: input `{raw}` did not start with `since.time:` or `since.commit:`"),
    ))
}

/// Hand-rolled non-negative base-10 `u64` parser.
fn parse_u64(s: &str) -> Result<u64, HistoryError> {
    if s.is_empty() {
        return Err(HistoryError::new(
            HistoryErrorCode::HistoryRefNotFound,
            "since: empty millis payload",
        ));
    }
    let mut acc: u64 = 0;
    for b in s.bytes() {
        if !b.is_ascii_digit() {
            return Err(HistoryError::new(
                HistoryErrorCode::HistoryRefNotFound,
                format!("since: non-digit byte 0x{b:02x} in millis payload"),
            ));
        }
        let digit = u64::from(b.saturating_sub(b'0'));
        acc = acc.checked_mul(10).ok_or_else(|| {
            HistoryError::new(
                HistoryErrorCode::HistoryRefNotFound,
                "since: millis payload overflowed u64",
            )
        })?;
        acc = acc.checked_add(digit).ok_or_else(|| {
            HistoryError::new(
                HistoryErrorCode::HistoryRefNotFound,
                "since: millis payload overflowed u64",
            )
        })?;
    }
    Ok(acc)
}

/// Every commit with `applied_at_ms >= threshold`, sorted ascending by sha.
#[must_use]
pub fn since_time(graph: &CommitGraph, threshold: AppliedAtMs) -> Vec<CommitSha> {
    graph
        .nodes()
        .filter(|n| n.applied_at_ms() >= threshold)
        .map(|n| *n.sha())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{ParsedSince, parse_since_filter, parse_u64, since_time};
    use crate::commit_graph::{CommitGraph, CommitNode};
    use crate::errors::HistoryErrorCode;
    use crate::types::{AppliedAtMs, CommitSha};

    fn sha(byte: u8) -> CommitSha {
        CommitSha::from_bytes([byte; 20])
    }

    #[test]
    fn parse_u64_accepts_digits() {
        match parse_u64("12345") {
            Ok(v) => assert_eq!(v, 12345),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn parse_u64_rejects_empty() {
        match parse_u64("") {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, HistoryErrorCode::HistoryRefNotFound),
        }
    }

    #[test]
    fn parse_u64_rejects_non_digit() {
        match parse_u64("12a3") {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, HistoryErrorCode::HistoryRefNotFound),
        }
    }

    #[test]
    fn parse_u64_rejects_overflow() {
        // 2^64 = 18446744073709551616
        match parse_u64("18446744073709551616") {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, HistoryErrorCode::HistoryRefNotFound),
        }
    }

    #[test]
    fn parse_since_time_form() {
        match parse_since_filter("since.time:42") {
            Ok(ParsedSince::Time(t)) => assert_eq!(t, AppliedAtMs::new(42)),
            Ok(ParsedSince::Commit(_)) => assert!(false, "wrong variant"),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn parse_since_commit_form() {
        let hex = "0123456789abcdef0123456789abcdef01234567";
        match parse_since_filter(&format!("since.commit:{hex}")) {
            Ok(ParsedSince::Commit(c)) => assert_eq!(format!("{c}"), hex),
            Ok(ParsedSince::Time(_)) => assert!(false, "wrong variant"),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn parse_since_bad_prefix_fails() {
        match parse_since_filter("until.time:42") {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, HistoryErrorCode::HistoryRefNotFound),
        }
    }

    #[test]
    fn since_time_filters_threshold_inclusive() {
        let mut g = CommitGraph::new();
        g.add_commit(CommitNode::new(sha(0), Vec::new(), AppliedAtMs::new(10)));
        g.add_commit(CommitNode::new(sha(1), Vec::new(), AppliedAtMs::new(20)));
        g.add_commit(CommitNode::new(sha(2), Vec::new(), AppliedAtMs::new(30)));
        let got = since_time(&g, AppliedAtMs::new(20));
        assert_eq!(got, vec![sha(1), sha(2)]);
    }

    #[test]
    fn since_time_threshold_above_all() {
        let mut g = CommitGraph::new();
        g.add_commit(CommitNode::new(sha(0), Vec::new(), AppliedAtMs::new(10)));
        assert!(since_time(&g, AppliedAtMs::new(100)).is_empty());
    }
}
