//! Request-local original-byte provenance for one immutable indexed chunk.
//!
//! NFC and case semantics remain owned by the existing normalizer. Canonical
//! decompositions are annotated only to recover which original intervals
//! contributed to an already-verified normalized scalar. No query matching or
//! whole-file normalization is performed here.

use core::ops::Range;
use std::cell::Cell;

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::{canonical_combining_class, decompose_canonical};

use crate::{CaseMode, apply_case};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MappingError {
    ByteLimit,
    EntryLimit,
    AllocationRefused,
    Interrupted,
    SourceMismatch,
    InvalidSpan,
}

impl core::fmt::Display for MappingError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::ByteLimit => "normalization preview byte limit exceeded",
            Self::EntryLimit => "normalization preview provenance entry limit exceeded",
            Self::AllocationRefused => "normalization preview allocation refused",
            Self::Interrupted => "normalization preview interrupted",
            Self::SourceMismatch => "immutable source does not match indexed NFC authority",
            Self::InvalidSpan => "invalid transformed or original preview span",
        })
    }
}

impl std::error::Error for MappingError {}

#[derive(Debug)]
struct ScalarOrigin {
    transformed: Range<usize>,
    original: Range<usize>,
    normalized: Range<usize>,
}

#[derive(Debug)]
struct Decomposed {
    scalar: char,
    original: Range<usize>,
    owner: usize,
    ordinal: usize,
}

/// A transformed chunk bound to the original immutable string it maps.
///
/// Fields are private and there is no deserializer or caller-supplied map. A map
/// cannot be paired with another source or forged by a consumer. Returned
/// intervals cover normalization equivalence, not necessarily byte equality.
#[derive(Debug)]
pub struct MappedText<'a> {
    source: &'a str,
    transformed: String,
    origins: Vec<ScalarOrigin>,
    normalized_len: usize,
    case: CaseMode,
}

impl<'a> MappedText<'a> {
    /// Verify and map exactly one raw/indexed chunk pair.
    ///
    /// `max_bytes` bounds each input and the transformed output separately.
    /// `max_entries` bounds the sum of capacities of raw decomposition, NFC
    /// decomposition, NFC scalar origins and folded scalar origins, including
    /// temporaries. [`Self::allocation_bound`] supplies a conservative bound for
    /// caller reservations, including the library's normalization buffers.
    /// Owner canonical-order sorts are in-place; the library's stable-sort
    /// scratch is included in the reservation. Cancellation is checked around
    /// each bounded normalization segment. One library iterator step/sort is
    /// not preemptible; raw input bytes and entries limit that work.
    pub fn new(
        raw: &'a str,
        indexed_nfc: &str,
        case: CaseMode,
        max_bytes: usize,
        max_entries: usize,
        interrupted: &dyn Fn() -> bool,
    ) -> Result<Self, MappingError> {
        checkpoint(interrupted)?;
        if raw.len() > max_bytes || indexed_nfc.len() > max_bytes {
            return Err(MappingError::ByteLimit);
        }
        let mut remaining = max_entries;
        let original = decompose(raw, &mut remaining, interrupted)?;
        // Use the actual library iterator behind crate::nfc. The guarded source
        // iterator also polls inside a single long combining sequence. If it
        // stops, even an equal prefix is discarded as interrupted.
        let stopped = Cell::new(false);
        let source_chars = raw.chars().take_while(|_| {
            let stop = interrupted();
            stopped.set(stopped.get() || stop);
            !stop
        });
        let equal = source_chars.nfc().eq(indexed_nfc.chars());
        if stopped.get() {
            return Err(MappingError::Interrupted);
        }
        checkpoint(interrupted)?;
        if !equal {
            return Err(MappingError::SourceMismatch);
        }

        let normalized = decompose(indexed_nfc, &mut remaining, interrupted)?;
        if original.len() != normalized.len() {
            return Err(MappingError::SourceMismatch);
        }
        let mut nfc_origins = Vec::new();
        for (offset, scalar) in indexed_nfc.char_indices() {
            checkpoint(interrupted)?;
            reserve_entry(&mut nfc_origins, &mut remaining)?;
            nfc_origins.push(ScalarOrigin {
                transformed: offset..offset.saturating_add(scalar.len_utf8()),
                original: raw.len()..0,
                normalized: offset..offset.saturating_add(scalar.len_utf8()),
            });
        }
        for (source, indexed) in original.iter().zip(&normalized) {
            checkpoint(interrupted)?;
            if source.scalar != indexed.scalar {
                return Err(MappingError::SourceMismatch);
            }
            let origin = nfc_origins
                .get_mut(indexed.owner)
                .ok_or(MappingError::InvalidSpan)?;
            origin.original.start = origin.original.start.min(source.original.start);
            origin.original.end = origin.original.end.max(source.original.end);
        }

        let mut transformed = String::new();
        let mut origins = Vec::new();
        for origin in &nfc_origins {
            checkpoint(interrupted)?;
            let scalar_text = indexed_nfc
                .get(origin.transformed.clone())
                .ok_or(MappingError::InvalidSpan)?;
            if raw.get(origin.original.clone()).is_none() {
                return Err(MappingError::InvalidSpan);
            }
            // Calling the existing case owner per scalar preserves its exact
            // context-free fold, including expanding U+0130 and non-final sigma.
            let folded = apply_case(scalar_text, case);
            for scalar in folded.chars() {
                let end = transformed.len().saturating_add(scalar.len_utf8());
                if end > max_bytes {
                    return Err(MappingError::ByteLimit);
                }
                reserve_entry(&mut origins, &mut remaining)?;
                reserve_text(&mut transformed, scalar.len_utf8(), max_bytes)?;
                origins.push(ScalarOrigin {
                    transformed: transformed.len()..end,
                    original: origin.original.clone(),
                    normalized: origin.normalized.clone(),
                });
                transformed.push(scalar);
            }
        }
        checkpoint(interrupted)?;
        Ok(Self {
            source: raw,
            transformed,
            origins,
            normalized_len: indexed_nfc.len(),
            case,
        })
    }

    /// Conservative requested-heap-capacity bound, not an RSS measurement.
    ///
    /// Canonical decompositions are bounded before NFC iteration. Each of the
    /// library's two normalization buffers holds at most that many scalars,
    /// with capacity growth below twice the scalar count. Decomposition stores
    /// `(u8, char)` entries; recomposition stores `char` entries.
    /// The pinned Rust stable sort also requests at most one decomposition
    /// entry per scalar as heap scratch (small sorts use stack storage).
    /// Entry capacities across all owner vectors sum to `max_entries`; output
    /// string capacity is at most `max_bytes`. Extra space covers small inline
    /// buffers and per-scalar case temporaries. Overflow refuses reservation.
    #[must_use]
    pub fn allocation_bound(max_bytes: usize, max_entries: usize) -> Option<usize> {
        let normalization_bytes = core::mem::size_of::<(u8, char)>()
            .checked_add(core::mem::size_of::<char>())?
            .checked_mul(2)?
            .checked_add(core::mem::size_of::<(u8, char)>())?;
        let entry_bytes = core::mem::size_of::<Decomposed>()
            .max(core::mem::size_of::<ScalarOrigin>())
            .checked_add(normalization_bytes)?;
        max_entries
            .checked_mul(entry_bytes)?
            .checked_add(max_bytes)?
            .checked_add(1024)
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.transformed
    }

    /// First raw-substring witness under this map's bound case mode.
    /// Uses the same needle transformation and comparison as `contains_substring`.
    #[must_use]
    pub fn find_substring(&self, needle: &str) -> Option<Range<usize>> {
        self.find_substrings(needle).next()
    }

    /// Every raw-substring witness, including overlapping occurrences, with
    /// the same transformation and first-match semantics as [`Self::find_substring`].
    pub fn find_substrings(&self, needle: &str) -> impl Iterator<Item = Range<usize>> + '_ {
        crate::tokens::substring_ranges_in_case_text(&self.transformed, needle, self.case)
    }

    /// Map a nonempty, UTF-8-aligned transformed span to its covering raw span.
    /// Reordering may make this cover intervening source scalars as well.
    pub fn source_range(&self, range: Range<usize>) -> Result<Range<usize>, MappingError> {
        self.map_ranges(range).map(|(original, _)| original)
    }

    /// NFC coordinates before case folding, even when the fold expands a scalar.
    pub fn normalized_range(&self, range: Range<usize>) -> Result<Range<usize>, MappingError> {
        self.map_ranges(range).map(|(_, normalized)| normalized)
    }

    fn map_ranges(
        &self,
        range: Range<usize>,
    ) -> Result<(Range<usize>, Range<usize>), MappingError> {
        if range.start >= range.end || self.transformed.get(range.clone()).is_none() {
            return Err(MappingError::InvalidSpan);
        }
        let mut original = self.source.len()..0;
        let mut normalized = self.normalized_len..0;
        let start = self
            .origins
            .partition_point(|origin| origin.transformed.end <= range.start);
        for origin in self
            .origins
            .iter()
            .skip(start)
            .take_while(|origin| origin.transformed.start < range.end)
        {
            original.start = original.start.min(origin.original.start);
            original.end = original.end.max(origin.original.end);
            normalized.start = normalized.start.min(origin.normalized.start);
            normalized.end = normalized.end.max(origin.normalized.end);
        }
        if original.start >= original.end || self.source.get(original.clone()).is_none() {
            return Err(MappingError::InvalidSpan);
        }
        if normalized.start >= normalized.end {
            return Err(MappingError::InvalidSpan);
        }
        if normalized.end > self.normalized_len {
            return Err(MappingError::InvalidSpan);
        }
        Ok((original, normalized))
    }
}

fn checkpoint(interrupted: &dyn Fn() -> bool) -> Result<(), MappingError> {
    if interrupted() {
        Err(MappingError::Interrupted)
    } else {
        Ok(())
    }
}

fn reserve_entry<T>(entries: &mut Vec<T>, remaining: &mut usize) -> Result<(), MappingError> {
    if entries.len() < entries.capacity() {
        return Ok(());
    }
    let extra = entries.capacity().max(1).min(*remaining);
    if extra == 0 {
        return Err(MappingError::EntryLimit);
    }
    let before = entries.capacity();
    entries
        .try_reserve_exact(extra)
        .map_err(|_allocation_error| MappingError::AllocationRefused)?;
    *remaining = remaining
        .checked_sub(entries.capacity().saturating_sub(before))
        .ok_or(MappingError::EntryLimit)?;
    Ok(())
}

fn reserve_text(text: &mut String, extra: usize, limit: usize) -> Result<(), MappingError> {
    let needed = text
        .len()
        .checked_add(extra)
        .ok_or(MappingError::ByteLimit)?;
    if needed > limit {
        return Err(MappingError::ByteLimit);
    }
    if needed > text.capacity() {
        let capacity = text.capacity().saturating_mul(2).max(needed).min(limit);
        text.try_reserve_exact(capacity.saturating_sub(text.len()))
            .map_err(|_allocation_error| MappingError::AllocationRefused)?;
        if text.capacity() > limit {
            return Err(MappingError::ByteLimit);
        }
    }
    Ok(())
}

fn decompose(
    text: &str,
    remaining: &mut usize,
    interrupted: &dyn Fn() -> bool,
) -> Result<Vec<Decomposed>, MappingError> {
    let mut out = Vec::new();
    for (owner, (start, scalar)) in text.char_indices().enumerate() {
        checkpoint(interrupted)?;
        let mut failure = None;
        decompose_canonical(scalar, |part| {
            if failure.is_some() {
                return;
            }
            if let Err(error) = reserve_entry(&mut out, remaining) {
                failure = Some(error);
                return;
            }
            out.push(Decomposed {
                scalar: part,
                original: start..start.saturating_add(scalar.len_utf8()),
                owner,
                ordinal: out.len(),
            });
        });
        if let Some(error) = failure {
            return Err(error);
        }
    }
    // Stable canonical order without allocating a sorting scratch vector.
    // Ordinal breaks equal-class ties; this is provenance ordering only, and
    // both streams are checked against the library's actual NFC output above.
    let mut start = 0_usize;
    while start < out.len() {
        checkpoint(interrupted)?;
        if out
            .get(start)
            .is_some_and(|item| canonical_combining_class(item.scalar) == 0)
        {
            start = start.saturating_add(1);
        }
        let mut end = start;
        while out
            .get(end)
            .is_some_and(|item| canonical_combining_class(item.scalar) != 0)
        {
            end = end.saturating_add(1);
        }
        out.get_mut(start..end)
            .ok_or(MappingError::InvalidSpan)?
            .sort_unstable_by_key(|item| (canonical_combining_class(item.scalar), item.ordinal));
        checkpoint(interrupted)?;
        start = end;
    }
    Ok(out)
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed byte-range oracles are assertions; setup errors propagate"
)]
mod tests {
    use super::{MappedText, MappingError};
    use crate::{CaseMode, apply_case, nfc};

    fn mapped<'a>(
        raw: &'a str,
        indexed: &str,
        case: CaseMode,
    ) -> Result<MappedText<'a>, MappingError> {
        MappedText::new(raw, indexed, case, 4096, 16384, &|| false)
    }

    #[test]
    fn l4_canonical_order_and_fold_matrix_preserves_source_ranges() -> Result<(), MappingError> {
        // A fixed small alphabet exercises leading/reordered marks, Hangul
        // composition, singleton decomposition, and an expanding lowercase.
        // Compare the mapped output to the public normalization/case contract,
        // then require every emitted scalar to carry valid source/NFC bytes.
        let alphabet = [
            "a", "\u{301}", "\u{323}", "\u{1100}", "\u{1161}", "\u{212a}", "\u{130}",
        ];
        for first in alphabet {
            for second in alphabet {
                for third in alphabet {
                    let raw = format!("{first}{second}{third}");
                    let indexed = nfc(&raw);
                    for case in [CaseMode::Sensitive, CaseMode::Folded] {
                        let map = mapped(&raw, indexed.as_ref(), case)?;
                        assert_eq!(map.text(), apply_case(indexed.as_ref(), case));
                        for (start, scalar) in map.text().char_indices() {
                            let end = start + scalar.len_utf8();
                            let source = map.source_range(start..end)?;
                            let normalized = map.normalized_range(start..end)?;
                            assert!(raw.get(source).is_some(), "{raw:?} {case:?}");
                            assert!(indexed.get(normalized).is_some(), "{raw:?} {case:?}");
                        }
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn l4_substring_truth_and_range_share_case_and_nfc_owner() -> Result<(), MappingError> {
        let map = mapped("İ CAFE\u{301}", "İ CAFÉ", CaseMode::Folded)?;
        assert_eq!(map.find_substring("i\u{307}"), Some(0..3));
        assert_eq!(map.find_substring("cafe\u{301}"), Some(4..9));
        assert!(crate::contains_substring(
            "İ CAFÉ",
            "cafe\u{301}",
            CaseMode::Folded
        ));
        assert_eq!(map.find_substring(""), None);
        assert_eq!(
            mapped("İ", "İ", CaseMode::Sensitive)?.find_substring("i\u{307}"),
            None
        );
        Ok(())
    }

    #[test]
    fn l4_substring_ranges_keep_folded_coordinates_and_overlaps() -> Result<(), MappingError> {
        let folded = mapped("İ İ", "İ İ", CaseMode::Folded)?;
        assert_eq!(
            folded.find_substrings("i\u{307}").collect::<Vec<_>>(),
            vec![0..3, 4..7]
        );
        assert_eq!(folded.find_substrings("").count(), 0);
        let sensitive = mapped("aaa", "aaa", CaseMode::Sensitive)?;
        assert_eq!(
            sensitive.find_substrings("aa").collect::<Vec<_>>(),
            vec![0..2, 1..3]
        );
        let utf8 = mapped("ééé", "ééé", CaseMode::Sensitive)?;
        assert_eq!(
            utf8.find_substrings("éé").collect::<Vec<_>>(),
            vec![0..4, 2..6]
        );
        Ok(())
    }

    #[test]
    fn l4_decomposed_source_maps_composed_focus() -> Result<(), MappingError> {
        let mapped = mapped("cafe\u{301}!", "café!", CaseMode::Sensitive)?;
        assert_eq!(mapped.text(), "café!");
        assert_eq!(mapped.source_range(3..5)?, 3..6);
        assert_eq!(mapped.source_range(0..5)?, 0..6);
        assert_eq!(mapped.source_range(5..6)?, 6..7);
        Ok(())
    }

    #[test]
    fn l4_reordered_marks_require_non_monotonic_intervals() -> Result<(), MappingError> {
        let reordered = mapped("a\u{301}\u{323}", "ạ\u{301}", CaseMode::Sensitive)?;
        assert_eq!(reordered.source_range(0..3)?, 0..5);
        assert_eq!(reordered.source_range(3..5)?, 1..3);
        assert_eq!(reordered.source_range(0..5)?, 0..5);
        let leading = mapped("\u{301}\u{323}a", "\u{323}\u{301}a", CaseMode::Sensitive)?;
        assert_eq!(leading.source_range(0..2)?, 2..4);
        assert_eq!(leading.source_range(2..4)?, 0..2);
        Ok(())
    }

    #[test]
    fn l4_expanding_case_retains_one_source_scalar() -> Result<(), MappingError> {
        let mapped = mapped("İ!", "İ!", CaseMode::Folded)?;
        assert_eq!(mapped.text(), "i\u{307}!");
        assert_eq!(mapped.source_range(0..1)?, 0..2);
        assert_eq!(mapped.source_range(1..3)?, 0..2);
        assert_eq!(mapped.normalized_range(1..3)?, 0..2);
        assert_eq!(mapped.source_range(0..3)?, 0..2);
        assert_eq!(mapped.source_range(3..4)?, 2..3);
        assert_eq!(mapped.source_range(2..3), Err(MappingError::InvalidSpan));
        Ok(())
    }

    #[test]
    fn l4_hangul_and_singleton_use_library_normalization() -> Result<(), MappingError> {
        let hangul = mapped("\u{1100}\u{1161}\u{11a8}", "각", CaseMode::Sensitive)?;
        assert_eq!(hangul.source_range(0..3)?, 0..9);
        let singleton = mapped("\u{212a}elvin", "Kelvin", CaseMode::Folded)?;
        assert_eq!(singleton.text(), "kelvin");
        assert_eq!(singleton.source_range(0..1)?, 0..3);
        assert_eq!(singleton.source_range(0..6)?, 0..8);
        Ok(())
    }

    #[test]
    fn l4_chunk_normalization_cannot_borrow_adjacent_chunk() -> Result<(), MappingError> {
        assert_eq!(
            mapped("e", "e", CaseMode::Sensitive)?.source_range(0..1)?,
            0..1
        );
        assert_eq!(
            mapped("\u{301}", "\u{301}", CaseMode::Sensitive)?.source_range(0..2)?,
            0..2
        );
        assert!(matches!(
            mapped("e", "é", CaseMode::Sensitive),
            Err(MappingError::SourceMismatch)
        ));
        Ok(())
    }

    #[test]
    fn l4_wrong_authority_and_invalid_spans_fail_closed() -> Result<(), MappingError> {
        assert!(matches!(
            mapped("old", "new", CaseMode::Sensitive),
            Err(MappingError::SourceMismatch)
        ));
        assert!(matches!(
            mapped("cafe\u{301}", "cafe\u{301}", CaseMode::Sensitive),
            Err(MappingError::SourceMismatch)
        ));
        let mapped = mapped("é", "é", CaseMode::Sensitive)?;
        for span in [0..0, 1..2, 0..3, core::ops::Range { start: 2, end: 1 }] {
            assert_eq!(mapped.source_range(span), Err(MappingError::InvalidSpan));
        }
        Ok(())
    }

    #[test]
    fn l4_preview_byte_entry_and_cancellation_limits_are_distinct() -> Result<(), MappingError> {
        assert!(matches!(
            MappedText::new("ab", "ab", CaseMode::Sensitive, 1, 64, &|| false),
            Err(MappingError::ByteLimit)
        ));
        assert!(matches!(
            MappedText::new("İ", "İ", CaseMode::Folded, 2, 64, &|| false),
            Err(MappingError::ByteLimit)
        ));
        assert!(matches!(
            MappedText::new("a", "a", CaseMode::Sensitive, 1, 3, &|| false),
            Err(MappingError::EntryLimit)
        ));
        assert_eq!(
            MappedText::new("a", "a", CaseMode::Sensitive, 1, 4, &|| false)?.text(),
            "a"
        );
        assert!(matches!(
            MappedText::new("a", "a", CaseMode::Sensitive, 1, 4, &|| true),
            Err(MappingError::Interrupted)
        ));
        let checks = std::cell::Cell::new(0_usize);
        let interrupted = || {
            checks.set(checks.get().saturating_add(1));
            checks.get() >= 20
        };
        let raw = "a".repeat(100);
        assert!(matches!(
            MappedText::new(&raw, &raw, CaseMode::Sensitive, 100, 1000, &interrupted),
            Err(MappingError::Interrupted)
        ));
        assert!(MappedText::allocation_bound(usize::MAX, 1).is_none());
        Ok(())
    }

    #[test]
    fn l4_long_combining_segment_preserves_fixed_reordering_intervals() -> Result<(), MappingError>
    {
        // 8,193 scalars exercise the library's heap-backed stable sort. The
        // golden is independent: grave (CCC 230) precedes comma-above-right
        // (CCC 232), and the first grave composes with the leading ASCII a.
        let raw = format!("a{}", "\u{315}\u{300}".repeat(4096));
        let indexed = format!("à{}{}", "\u{300}".repeat(4095), "\u{315}".repeat(4096));
        let mapped = MappedText::new(&raw, &indexed, CaseMode::Sensitive, 32_768, 65_536, &|| {
            false
        })?;
        assert_eq!(mapped.text(), indexed);
        assert_eq!(mapped.source_range(0..2)?, 0..5);
        assert_eq!(mapped.source_range(8192..8194)?, 1..3);
        assert_eq!(mapped.source_range(0..indexed.len())?, 0..raw.len());
        Ok(())
    }

    #[test]
    fn l4_empty_source_has_no_invented_positive_span() -> Result<(), MappingError> {
        let mapped = MappedText::new("", "", CaseMode::Sensitive, 0, 0, &|| false)?;
        assert_eq!(mapped.text(), "");
        assert_eq!(mapped.source_range(0..0), Err(MappingError::InvalidSpan));
        Ok(())
    }
}
