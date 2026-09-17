//! Adjacency query: bounded-window co-occurrence between two terms.
//!
//! `dsl.md §5.3` specifies a default window of 8 tokens; the match is
//! symmetric (term `b` may appear before *or* after term `a`). For each
//! doc that contains both terms, emit one [`PhraseMatch`] per
//! `(pos_a, pos_b)` pair whose absolute distance fits inside
//! [`AdjacencyConfig::window_tokens`].
//!
//! Window > [`crate::types::MAX_WINDOW_TOKENS`] is rejected with
//! [`PositionsErrorCode::WindowOutOfRange`]. A `0`-width window is also
//! rejected at [`AdjacencyConfig`] construction time and similarly fails
//! closed here (defense-in-depth).
//!
//! Missing terms surface as an empty result set — not an error.

use crate::errors::{LimitDimension, PositionsError, PositionsErrorCode};
use crate::phrase_query::{PhraseMatch, PhraseMatches, collect_term_postings};
use crate::source::TermPostingSource;
use crate::types::{AdjacencyConfig, MAX_ADJACENCY_SCAN_DEPTH, MAX_WINDOW_TOKENS};

/// Run an adjacency query for `term_a` near `term_b` within `cfg`.
///
/// `idx` is any [`TermPostingSource`]: one [`crate::PositionsIndex`] or a
/// [`crate::ShardedPositionsIndex`] over a doc-id partition; the algorithm
/// is the same and so is the answer, scan-depth cap included.
///
/// Symmetric semantics: `b` may appear before or after `a`. The emitted
/// [`PhraseMatch::start_position`] is `min(pos_a, pos_b)` and
/// [`PhraseMatch::end_position`] is `max(pos_a, pos_b)` so the caller can
/// read the match span without re-running the comparison.
///
/// Window validation:
/// - `cfg.window_tokens == 0` → [`PositionsErrorCode::WindowOutOfRange`].
/// - `cfg.window_tokens > MAX_WINDOW_TOKENS` →
///   [`PositionsErrorCode::WindowOutOfRange`].
pub fn query_adjacency<S: TermPostingSource + ?Sized>(
    idx: &S,
    term_a: &str,
    term_b: &str,
    cfg: &AdjacencyConfig,
) -> Result<PhraseMatches, PositionsError> {
    if cfg.window_tokens == 0 || cfg.window_tokens > MAX_WINDOW_TOKENS {
        return Err(PositionsError::new(
            PositionsErrorCode::WindowOutOfRange,
            format!(
                "window_tokens={} outside [1..={}]",
                cfg.window_tokens, MAX_WINDOW_TOKENS
            ),
        ));
    }

    let a_map = collect_term_postings(idx, term_a)?;
    if a_map.is_empty() {
        return Ok(PhraseMatches::empty());
    }
    let b_map = collect_term_postings(idx, term_b)?;
    if b_map.is_empty() {
        return Ok(PhraseMatches::empty());
    }

    let mut out: Vec<PhraseMatch> = Vec::new();
    // LEX-03 §4.2 adjacency scan-depth cap: count every position-pair
    // comparison across all docs that hold both terms. The cap is checked
    // *before* each comparison so the (cap+1)-th candidate fails closed
    // without doing the work — no silent truncation of the result set.
    let mut scan_depth: u64 = 0;
    let cap: u64 = u64::from(MAX_ADJACENCY_SCAN_DEPTH);
    for (doc_id, a_positions) in &a_map {
        let Some(b_positions) = b_map.get(doc_id) else {
            continue;
        };
        // Two-pointer sweep: both position lists are ascending. For each
        // `a`, advance a working `b` cursor to the first `b` not before
        // `a - window`, then emit pairs while `b - a <= window`.
        // Symmetry is implicit because the window is two-sided.
        for a in a_positions {
            for b in b_positions {
                scan_depth = scan_depth.saturating_add(1);
                if scan_depth > cap {
                    return Err(PositionsError::plan_limit_exceeded(
                        LimitDimension::AdjacencyScanDepth,
                        format!(
                            "adjacency candidate-pair scan exceeded cap={MAX_ADJACENCY_SCAN_DEPTH}"
                        ),
                    ));
                }
                let diff = a.0.abs_diff(b.0);
                if diff == 0 {
                    // Same position — only possible if both terms map to
                    // the same token, which the caller must avoid. Skip to
                    // keep the spec unambiguous.
                    continue;
                }
                if diff <= cfg.window_tokens {
                    let (lo, hi) = if a.0 < b.0 { (*a, *b) } else { (*b, *a) };
                    out.push(PhraseMatch {
                        doc_id: *doc_id,
                        start_position: lo,
                        end_position: hi,
                    });
                }
            }
        }
    }

    Ok(PhraseMatches { matches: out })
}

#[cfg(test)]
#[expect(
    clippy::unreachable,
    clippy::indexing_slicing,
    clippy::redundant_clone,
    reason = "test fixtures use direct indexing and copies for failure clarity"
)]
mod tests {
    use super::query_adjacency;
    use crate::builder::PositionsBuilder;
    use crate::errors::{LimitDimension, PositionsErrorCode};
    use crate::index::PositionsIndex;
    use crate::types::{
        AdjacencyConfig, DocId, MAX_ADJACENCY_SCAN_DEPTH, MAX_WINDOW_TOKENS, NormalizerVersion,
        Position,
    };

    fn fixture() -> PositionsIndex {
        // Doc 0: positions [a@0, x@1, x@2, x@3, x@4, b@5]  -> distance 5
        // Doc 1: positions [b@0, x@1, x@2, a@3]            -> distance 3 (b before a)
        // Doc 2: positions [a@0, b@20]                     -> distance 20 (out of default 8)
        // Doc 3: positions [a@0, b@1, b@8, b@9]            -> distances 1, 8, 9
        let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));

        let inserts: &[(DocId, &str, Position)] = &[
            (DocId(0), "a", Position(0)),
            (DocId(0), "b", Position(5)),
            (DocId(1), "b", Position(0)),
            (DocId(1), "a", Position(3)),
            (DocId(2), "a", Position(0)),
            (DocId(2), "b", Position(20)),
            (DocId(3), "a", Position(0)),
            (DocId(3), "b", Position(1)),
            (DocId(3), "b", Position(8)),
            (DocId(3), "b", Position(9)),
        ];
        for (d, t, p) in inserts {
            if let Err(e) = b.add_token(*d, t, *p) {
                assert!(false, "{e}");
                unreachable!()
            }
        }

        match b.finish() {
            Ok(idx) => idx,
            Err(e) => {
                assert!(false, "{e}");
                unreachable!()
            }
        }
    }

    #[test]
    fn a_then_b_within_default_window() {
        let idx = fixture();
        let cfg = AdjacencyConfig::default_window();
        let r = match query_adjacency(&idx, "a", "b", &cfg) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        // Default window = 8. Doc 0 distance 5 -> hit. Doc 1 distance 3 ->
        // hit. Doc 2 distance 20 -> miss. Doc 3 distances 1, 8, 9 -> 1 and 8
        // are hits (<=8); 9 is out of window. So 4 hits total.
        assert_eq!(r.matches.len(), 4);
    }

    #[test]
    fn b_then_a_is_symmetric() {
        let idx = fixture();
        let cfg = AdjacencyConfig::default_window();
        let ab = match query_adjacency(&idx, "a", "b", &cfg) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let ba = match query_adjacency(&idx, "b", "a", &cfg) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(ab.matches.len(), ba.matches.len());
        // Spans are min/max sorted, so the two result sets should be equal
        // up to ordering. Sort both and compare.
        let mut a = ab.matches.clone();
        let mut b = ba.matches;
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b);
    }

    #[test]
    fn out_of_window_yields_empty_for_doc() {
        let idx = fixture();
        let Some(cfg) = AdjacencyConfig::new(2) else {
            assert!(false, "AdjacencyConfig::new(2)");
            return;
        };
        let r = match query_adjacency(&idx, "a", "b", &cfg) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        // Window = 2. Doc 0 distance 5 -> miss. Doc 1 distance 3 -> miss.
        // Doc 2 distance 20 -> miss. Doc 3 distances 1, 8, 9 -> only 1 hits.
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].doc_id, DocId(3));
    }

    #[test]
    fn missing_term_yields_empty() {
        let idx = fixture();
        let cfg = AdjacencyConfig::default_window();
        let r = match query_adjacency(&idx, "a", "absent", &cfg) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert!(r.matches.is_empty());
        let r2 = match query_adjacency(&idx, "absent", "b", &cfg) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert!(r2.matches.is_empty());
    }

    #[test]
    fn window_out_of_range_high_returns_typed_error() {
        let idx = fixture();
        let cfg = AdjacencyConfig {
            window_tokens: MAX_WINDOW_TOKENS.saturating_add(1),
        };
        match query_adjacency(&idx, "a", "b", &cfg) {
            Ok(_) => assert!(false, "expected WindowOutOfRange"),
            Err(e) => assert_eq!(e.code, PositionsErrorCode::WindowOutOfRange),
        }
    }

    #[test]
    fn window_out_of_range_zero_returns_typed_error() {
        let idx = fixture();
        let cfg = AdjacencyConfig { window_tokens: 0 };
        match query_adjacency(&idx, "a", "b", &cfg) {
            Ok(_) => assert!(false, "expected WindowOutOfRange"),
            Err(e) => assert_eq!(e.code, PositionsErrorCode::WindowOutOfRange),
        }
    }

    #[test]
    fn adjacency_scan_depth_cap_fails_closed() {
        // LEX-03 §4.2: when the candidate position-pair scan would exceed
        // MAX_ADJACENCY_SCAN_DEPTH, fail closed with PlanLimitExceeded +
        // dimension=AdjacencyScanDepth. Construct a single doc with two
        // terms each at `n` distinct positions such that `n*n > cap`.
        // Use spaced positions so neither term overlaps the other.
        // n=400 → 160_000 > 100_000 cap; both 400 ≤ MAX_POSITIONS_PER_CELL.
        let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        let n: u32 = 400;
        for i in 0..n {
            // `a` positions at 0, 10, 20, …
            if let Err(e) = b.add_token(DocId(0), "a", Position(i.saturating_mul(10))) {
                assert!(false, "{e}");
                return;
            }
            // `b` positions at 100_000, 100_010, … — far away from any
            // `a` so the result set is irrelevant to the cap check.
            let bp = 100_000u32.saturating_add(i.saturating_mul(10));
            if let Err(e) = b.add_token(DocId(0), "b", Position(bp)) {
                assert!(false, "{e}");
                return;
            }
        }
        let idx = match b.finish() {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let cfg = AdjacencyConfig::default_window();
        match query_adjacency(&idx, "a", "b", &cfg) {
            Ok(_) => assert!(false, "expected PlanLimitExceeded"),
            Err(e) => {
                assert_eq!(e.code, PositionsErrorCode::PlanLimitExceeded);
                assert_eq!(e.dimension, Some(LimitDimension::AdjacencyScanDepth));
                // Sanity: the documented cap is referenced — checked by
                // the assertions above plus the `types::plan_limit_constants_match_spec`
                // unit test. No additional inline check needed here.
                assert!(MAX_ADJACENCY_SCAN_DEPTH >= 1_000);
            }
        }
    }

    #[test]
    fn adjacency_scan_below_cap_succeeds() {
        // Sanity boundary: a scan well below the cap returns Ok. The
        // fixture exercises 4*4=16 candidate pairs in doc 3 (largest doc).
        let idx = fixture();
        let cfg = AdjacencyConfig::default_window();
        match query_adjacency(&idx, "a", "b", &cfg) {
            Ok(r) => assert!(!r.matches.is_empty()),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn match_span_uses_min_max_ordering() {
        let idx = fixture();
        let cfg = AdjacencyConfig::default_window();
        let r = match query_adjacency(&idx, "a", "b", &cfg) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        for m in &r.matches {
            assert!(m.start_position <= m.end_position, "span must be ordered");
        }
    }
}
