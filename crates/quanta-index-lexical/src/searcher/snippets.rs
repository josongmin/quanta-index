//! Snippet windows and highlights, and reading stored fields back.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::{SNIPPET_LEAD_BYTES, SNIPPET_WINDOW_BYTES};
use quanta_index_contract::{HighlightSpan, LqExpr, LqLeaf, LqQuery};

/// Collect the literal substrings a snippet may be centered on, in query order.
///
/// Only literal-bearing leaves contribute (`Keyword` / `Phrase` / `RawString`);
/// regex, structural, and predicate leaves carry no single literal to center on.
pub(crate) fn collect_snippet_center_terms(expr: &LqExpr, out: &mut Vec<String>) {
    match expr {
        LqExpr::Leaf(LqLeaf::Keyword(text) | LqLeaf::Phrase(text) | LqLeaf::RawString(text)) => {
            if !text.is_empty() {
                out.push(text.clone());
            }
        }
        LqExpr::Leaf(LqLeaf::Regex(_) | LqLeaf::StructuralBlock(_) | LqLeaf::Predicate { .. })
        | LqExpr::Empty => {}
        LqExpr::Not(inner) => collect_snippet_center_terms(inner, out),
        LqExpr::All(children) | LqExpr::Any(children) => {
            for child in children {
                collect_snippet_center_terms(child, out);
            }
        }
    }
}

/// The center terms for a query, in query order.
pub(crate) fn snippet_center_terms(query: &LqQuery) -> Vec<String> {
    let mut terms = Vec::new();
    collect_snippet_center_terms(&query.expr, &mut terms);
    terms
}

/// Step `index` down to the nearest UTF-8 char boundary at or below it.
///
/// `str::floor_char_boundary` is unstable, so this is a stable hand-rolled
/// equivalent. `index` is always clamped into `0..=len` by callers.
pub(crate) fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut i = index.min(text.len());
    while i > 0 && !text.is_char_boundary(i) {
        i = i.saturating_sub(1);
    }
    i
}

/// Narrow a within-snippet byte offset to `u32` for the candidate field.
///
/// The offset is always bounded by [`SNIPPET_WINDOW_BYTES`] (≤ 240) in the
/// truncated case, or by the short snippet's own length otherwise, so it is far
/// below `u32::MAX` and the narrowing is exact.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "the offset is always into the emitted text, which is at most SNIPPET_WINDOW_BYTES (240) bytes — the full stored snippet when it is <= 240 bytes, otherwise a windowed excerpt — so the usize->u32 narrowing is exact"
)]
pub(crate) fn snippet_offset_u32(within: usize) -> u32 {
    within as u32
}

/// Collect every center-term occurrence within `emitted` as a highlight span,
/// in ascending start order with exact duplicates removed.
///
/// Spans are computed over the *emitted* text (post-windowing), so their offsets
/// are valid against the snippet a consumer actually receives (J7Q-07).
pub(crate) fn collect_highlights(emitted: &str, center_terms: &[String]) -> Vec<HighlightSpan> {
    let mut spans: Vec<HighlightSpan> = Vec::new();
    for term in center_terms {
        if term.is_empty() {
            continue;
        }
        for (pos, matched) in emitted.match_indices(term.as_str()) {
            spans.push(HighlightSpan {
                start: snippet_offset_u32(pos),
                len: snippet_offset_u32(matched.len()),
            });
        }
    }
    spans.sort_by_key(|span| span.start);
    spans.dedup();
    spans
}

/// Produce the emitted snippet for a stored chunk, plus the primary hit offset
/// and every matched-hit span within it (for UI highlight anchoring, J7Q-02 +
/// J7Q-07).
///
/// The whole text is returned when it already fits [`SNIPPET_WINDOW_BYTES`].
/// Otherwise a window of at most that many bytes is taken, centered on the first
/// present center term (so the hit keeps leading and trailing context) and
/// clamped to UTF-8 char boundaries. When no center term is present in an
/// over-long snippet, the leading window is kept so the result is still bounded —
/// never an unbounded blob. Highlight spans and the primary offset are computed
/// over the emitted text, so the primary offset equals the first span's `start`.
/// Fully determined by `(stored, center_terms)`, so two runs over identical
/// inputs emit byte-identical windows, offsets, and spans.
pub(crate) fn window_snippet(
    stored: &str,
    center_terms: &[String],
) -> (String, Option<u32>, Vec<HighlightSpan>) {
    let text = if stored.len() <= SNIPPET_WINDOW_BYTES {
        stored.to_string()
    } else {
        let first_hit = center_terms
            .iter()
            .filter_map(|term| stored.find(term.as_str()))
            .min();
        let start = first_hit
            .map_or(0, |hit| floor_char_boundary(stored, hit.saturating_sub(SNIPPET_LEAD_BYTES)));
        let raw_end = start.saturating_add(SNIPPET_WINDOW_BYTES).min(stored.len());
        let end = floor_char_boundary(stored, raw_end);
        // start <= end <= stored.len(), both floor_char_boundary results, so this is unreachable; "" keeps the fallback bounded.
        stored.get(start..end).unwrap_or("").to_string()
    };
    let highlights = collect_highlights(&text, center_terms);
    let primary = highlights.first().map(|span| span.start);
    (text, primary, highlights)
}

/// Filter the planner's typed-unavailable list against adapter state.
///
/// The planner is stateless — it does not know which producers this
/// particular `TantivySearcher` actually has wired. The
/// repo-metadata-dependent codes (FORK/ARCHIVED/VISIBILITY/CONTEXT) drop
/// out of the typed-unavailable surface when the adapter has loaded a
/// repo metadata from the bundle payload, because the live
/// `repo_filter_matches` path then handles those filters correctly.
///
/// The `HISTORY_PRODUCER_UNAVAILABLE` and `REV_UNAVAILABLE` codes are
/// never suppressed: no commit/diff/repo producer or history producer is
/// wired on any current configuration of the lexical rail.
pub(crate) fn is_unavailable_suppressed_by_metadata(
    code: quanta_index_contract::SearchPlaneErrorCodeV2,
    has_repo_metadata: bool,
) -> bool {
    if !has_repo_metadata {
        return false;
    }
    matches!(
        code,
        crate::filters::codes::FORK_UNAVAILABLE
            | crate::filters::codes::ARCHIVED_UNAVAILABLE
            | crate::filters::codes::VISIBILITY_UNAVAILABLE
            | crate::filters::codes::CONTEXT_UNAVAILABLE
    )
}
