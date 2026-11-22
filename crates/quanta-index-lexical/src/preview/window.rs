//! Bounded, UTF-8 aligned excerpt selection for a verified source focus.

use core::ops::Range;

use crate::{SNIPPET_LEAD_BYTES, SNIPPET_WINDOW_BYTES};
use quanta_index_contract::HighlightSpan;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PositiveWitness {
    pub(crate) original: Range<usize>,
    pub(crate) normalized: Range<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WindowError {
    FocusExceedsBudget,
    Integrity(&'static str),
}

/// A selected content excerpt in the original and indexed byte coordinates.
/// The caller authenticates the source and funds output before copying it.
pub(crate) struct ContentWindow<'a, 'w> {
    pub(crate) text: &'a str,
    pub(crate) context: Range<usize>,
    pub(crate) focus: &'w PositiveWitness,
    witnesses: &'w [PositiveWitness],
}

impl ContentWindow<'_, '_> {
    /// Materialize the in-window, unique highlight spans after output admission.
    pub(crate) fn highlights(&self) -> Result<Vec<HighlightSpan>, WindowError> {
        let start = self.context.start;
        let end = self.context.end;
        let mut spans = self
            .witnesses
            .iter()
            .filter(|witness| witness.original.start >= start && witness.original.end <= end)
            .map(|witness| {
                Ok(HighlightSpan {
                    start: u32::try_from(witness.original.start.saturating_sub(start))
                        .map_err(|_error| WindowError::Integrity("highlight offset overflow"))?,
                    len: u32::try_from(witness.original.len())
                        .map_err(|_error| WindowError::Integrity("highlight length overflow"))?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        spans.sort_unstable_by_key(|span| (span.start, span.len));
        spans.dedup();
        Ok(spans)
    }
}

pub(crate) fn selected_content_window<'a, 'w>(
    raw: &'a str,
    indexed: &str,
    witnesses: &'w [PositiveWitness],
) -> Result<ContentWindow<'a, 'w>, WindowError> {
    // Even an out-of-window witness belongs to this authenticated source.
    // Validate all ranges before an optional focus cap can suppress the result.
    if witnesses.iter().any(|witness| {
        witness.original.is_empty()
            || witness.normalized.is_empty()
            || raw.get(witness.original.clone()).is_none()
            || indexed.get(witness.normalized.clone()).is_none()
    }) {
        return Err(WindowError::Integrity(
            "positive witness differs from original or indexed coordinates",
        ));
    }
    let focus = witnesses
        .iter()
        .min_by_key(|witness| {
            (
                witness.original.start,
                witness.original.end,
                witness.normalized.start,
                witness.normalized.end,
            )
        })
        .ok_or(WindowError::Integrity("lost positive witness"))?;
    let context = selected_window(raw, focus.original.clone())?;
    let text = raw
        .get(context.clone())
        .ok_or(WindowError::Integrity("window is not UTF-8 aligned"))?;
    Ok(ContentWindow {
        text,
        context,
        focus,
        witnesses,
    })
}

/// Select context around a focus in original UTF-8 byte coordinates.
///
/// The caller owns source authentication and preview admission. This function
/// neither changes matching truth nor allocates a snippet.
pub(crate) fn selected_window(raw: &str, focus: Range<usize>) -> Result<Range<usize>, WindowError> {
    if focus.start > focus.end
        || focus.end > raw.len()
        || !raw.is_char_boundary(focus.start)
        || !raw.is_char_boundary(focus.end)
    {
        return Err(WindowError::Integrity(
            "focus is not an original UTF-8 byte range",
        ));
    }
    let focus_len = focus.end.saturating_sub(focus.start);
    if focus_len > SNIPPET_WINDOW_BYTES {
        return Err(WindowError::FocusExceedsBudget);
    }
    let lead = SNIPPET_LEAD_BYTES.min(SNIPPET_WINDOW_BYTES.saturating_sub(focus_len));
    let mut start = if raw.len() <= SNIPPET_WINDOW_BYTES {
        0
    } else {
        focus.start.saturating_sub(lead)
    };
    while !raw.is_char_boundary(start) {
        start = start.saturating_add(1);
    }
    let end = floor_char_boundary(raw, start.saturating_add(SNIPPET_WINDOW_BYTES));
    if start > focus.start || end < focus.end {
        return Err(WindowError::Integrity("window clipped a fitting focus"));
    }
    Ok(start..end)
}

/// Stable equivalent of `str::floor_char_boundary`.
fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut i = index.min(text.len());
    while i > 0 && !text.is_char_boundary(i) {
        i = i.saturating_sub(1);
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_of_240_bytes_fits_and_241_is_unavailable() {
        let raw = "x".repeat(300);
        assert_eq!(selected_window(&raw, 10..250), Ok(10..250));
        assert_eq!(
            selected_window(&raw, 10..251),
            Err(WindowError::FocusExceedsBudget)
        );
    }

    #[test]
    fn multibyte_lead_rounds_up_without_clipping_focus() {
        let raw = format!("{}é{}", "a".repeat(99), "b".repeat(300));
        assert_eq!(selected_window(&raw, 220..223), Ok(101..341));
    }

    #[test]
    fn beginning_and_end_keep_fitting_focus() {
        let raw = "x".repeat(300);
        assert_eq!(selected_window(&raw, 0..3), Ok(0..240));
        assert_eq!(selected_window(&raw, 297..300), Ok(177..300));
    }

    #[test]
    fn invalid_original_coordinates_are_integrity_errors() {
        let raw = format!("é{}", "x".repeat(300));
        assert_eq!(
            selected_window(&raw, 1..2),
            Err(WindowError::Integrity(
                "focus is not an original UTF-8 byte range"
            ))
        );
        assert_eq!(
            selected_window(&raw, 400..401),
            Err(WindowError::Integrity(
                "focus is not an original UTF-8 byte range"
            ))
        );
        assert_eq!(
            selected_window(&raw, 400..700),
            Err(WindowError::Integrity(
                "focus is not an original UTF-8 byte range"
            )),
            "an optional preview cap must not hide invalid source coordinates"
        );
    }

    #[test]
    fn selected_content_keeps_overlapping_spans_without_duplicate_ranges() {
        let raw = "cafe\u{301} and café";
        let indexed = "café and café";
        let witnesses = [
            PositiveWitness {
                original: 11..16,
                normalized: 10..15,
            },
            PositiveWitness {
                original: 0..6,
                normalized: 0..5,
            },
            PositiveWitness {
                original: 3..6,
                normalized: 3..5,
            },
            PositiveWitness {
                original: 0..6,
                normalized: 0..5,
            },
        ];
        let observed = selected_content_window(raw, indexed, &witnesses).and_then(|window| {
            let highlights = window.highlights()?;
            Ok((window.text, window.context, window.focus, highlights))
        });
        assert_eq!(
            observed,
            Ok((
                raw,
                0..16,
                &PositiveWitness {
                    original: 0..6,
                    normalized: 0..5,
                },
                vec![
                    HighlightSpan { start: 0, len: 6 },
                    HighlightSpan { start: 3, len: 3 },
                    HighlightSpan { start: 11, len: 5 }
                ],
            ))
        );
    }

    #[test]
    fn selected_content_refuses_invalid_non_focus_before_the_optional_cap() {
        let raw = "a".repeat(300);
        let witnesses = [
            PositiveWitness {
                original: 0..241,
                normalized: 0..241,
            },
            PositiveWitness {
                original: 280..285,
                normalized: 280..301,
            },
        ];
        assert!(matches!(
            selected_content_window(&raw, &raw, &witnesses),
            Err(WindowError::Integrity(_))
        ));
    }
}
