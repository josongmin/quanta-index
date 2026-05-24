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

use crate::errors::{PositionsError, PositionsErrorCode};
use crate::index::PositionsIndex;
use crate::phrase_query::{PhraseMatch, PhraseMatches, collect_term_postings};
use crate::types::{AdjacencyConfig, MAX_WINDOW_TOKENS};

/// Run an adjacency query for `term_a` near `term_b` within `cfg`.
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
pub fn query_adjacency(
    idx: &PositionsIndex,
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
    use crate::errors::PositionsErrorCode;
    use crate::index::PositionsIndex;
    use crate::types::{AdjacencyConfig, DocId, MAX_WINDOW_TOKENS, NormalizerVersion, Position};

    fn fixture() -> PositionsIndex {
        // Doc 0: positions [a@0, x@1, x@2, x@3, x@4, b@5]  -> distance 5
        // Doc 1: positions [b@0, x@1, x@2, a@3]            -> distance 3 (b before a)
        // Doc 2: positions [a@0, b@20]                     -> distance 20 (out of default 8)
        // Doc 3: positions [a@0, b@1, b@8, b@9]            -> distances 1, 8, 9
        let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));

        b.add_token(DocId(0), "a", Position(0));
        b.add_token(DocId(0), "b", Position(5));

        b.add_token(DocId(1), "b", Position(0));
        b.add_token(DocId(1), "a", Position(3));

        b.add_token(DocId(2), "a", Position(0));
        b.add_token(DocId(2), "b", Position(20));

        b.add_token(DocId(3), "a", Position(0));
        b.add_token(DocId(3), "b", Position(1));
        b.add_token(DocId(3), "b", Position(8));
        b.add_token(DocId(3), "b", Position(9));

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
