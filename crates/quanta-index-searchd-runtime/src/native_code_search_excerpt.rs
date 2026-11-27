//! `CodeSearch` match positions on retained path pages.
//!
//! Each ranked row arrives with its file's representative match, computed by
//! the scan (or carried by a cached ranking); this step never re-scores or
//! re-chooses it. A path match goes on the wire as is. A content match gains
//! the Legacy excerpt, read from the row's verified Source bytes: up to
//! [`NATIVE_CODE_SEARCH_CONTEXT_BYTES`] on each side of the match, inside the
//! Source, with both edges moved inward to UTF-8 boundaries. A Source over
//! [`MAX_NATIVE_CODE_SEARCH_EXCERPT_SOURCE_BYTES`] keeps its match lines and
//! carries no excerpt. Every read and every retained byte is charged to the
//! page's request.

use std::ops::Range;

use quanta_index_contract::PreviewByteRange;
use quanta_index_contract::index::ObjectRef;
use quanta_index_contract::native_protocol::{
    MAX_NATIVE_CODE_SEARCH_EXCERPT_SOURCE_BYTES, NATIVE_CODE_SEARCH_CONTEXT_BYTES,
    NativeCodeSearchExcerpt, NativeCodeSearchMatch, NativeTextPathPageV1,
};
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, SourceRecordOwner, WorkBudget,
    ports::ImmutableObjectPort,
};
use quanta_index_lexical::native::CodeSearchMatch;

use crate::native_result::ResultValue;

/// Attach each row's `CodeSearch` match to `page`, in row order. A page whose
/// rows carry no match, such as a non-`CodeSearch` query, is left unchanged.
pub(super) fn attach_code_search_matches(
    page: &mut ResultValue<NativeTextPathPageV1>,
    witnesses: &[Option<CodeSearchMatch>],
    objects: &dyn ImmutableObjectPort,
    work: &mut WorkBudget,
) -> Result<(), IndexError> {
    if witnesses.iter().all(Option::is_none) {
        return Ok(());
    }
    let hits = &page.result.ranked.ranking.hits;
    if witnesses.len() != hits.len() {
        return Err(IndexError::BackendViolation(
            "CodeSearch witnesses differ from the page rows".into(),
        ));
    }
    let list_bytes = hits
        .len()
        .checked_mul(std::mem::size_of::<Option<NativeCodeSearchMatch>>())
        .ok_or(IndexError::ResourceLimited)
        .and_then(|bytes| u64::try_from(bytes).map_err(|_width| IndexError::ResourceLimited))?;
    let mut memory = Vec::new();
    memory
        .try_reserve_exact(
            hits.len()
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?,
        )
        .map_err(|_error| IndexError::ResourceLimited)?;
    memory.push(work.reserve_memory(list_bytes, MemoryLifetime::Retained)?);
    let mut matches = Vec::new();
    matches
        .try_reserve_exact(hits.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    for (hit, witness) in hits.iter().zip(witnesses) {
        work.checkpoint()?;
        matches.push(match *witness {
            None => None,
            Some(CodeSearchMatch::Path { start, end }) => {
                Some(NativeCodeSearchMatch::Path { start, end })
            }
            Some(CodeSearchMatch::Content {
                start,
                end,
                start_line,
                end_line,
            }) => {
                let (excerpt, grant) =
                    content_excerpt(hit.source, &hit.path, start, end, objects, work)?;
                memory.push(grant);
                Some(NativeCodeSearchMatch::Content {
                    start,
                    end,
                    start_line,
                    end_line,
                    excerpt,
                })
            }
        });
    }
    page.result.code_search = matches;
    page.result.validate_code_search().map_err(|message| {
        IndexError::BackendViolation(format!("CodeSearch page matches: {message}"))
    })?;
    page.payload_memory
        .try_reserve(memory.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    page.payload_memory.extend(memory);
    work.checkpoint()
}

/// The Legacy excerpt of one content match and the grant for its text.
fn content_excerpt(
    source: ObjectRef,
    path: &str,
    start: u64,
    end: u64,
    objects: &dyn ImmutableObjectPort,
    work: &mut WorkBudget,
) -> Result<(Option<NativeCodeSearchExcerpt>, Option<MemoryLease>), IndexError> {
    let stored = objects.get(source, work)?;
    let record = SourceRecordOwner::decode_json(&stored.payload, work, MemoryLifetime::Temporary)?;
    if stored.registration.reference != source
        || stored.registration.children.as_slice() != [record.bytes]
        || record.path != path
    {
        return Err(IndexError::CorruptObject(
            "CodeSearch row Source record differs from its ranked row".into(),
        ));
    }
    if record.byte_len > MAX_NATIVE_CODE_SEARCH_EXCERPT_SOURCE_BYTES {
        return Ok((None, None));
    }
    let mut excerpt = None;
    objects.visit_utf8_leaf(record.bytes, record.byte_len, work, &mut |raw, work| {
        let window = legacy_context_window(raw, start, end)?;
        let text = raw.get(window.clone()).ok_or_else(|| {
            IndexError::BackendViolation("CodeSearch excerpt window is not UTF-8 aligned".into())
        })?;
        let grant = work.reserve_memory(
            u64::try_from(text.len()).map_err(|_width| IndexError::ResourceLimited)?,
            MemoryLifetime::Retained,
        )?;
        let context = PreviewByteRange {
            start: u64::try_from(window.start).map_err(|_width| IndexError::ResourceLimited)?,
            end: u64::try_from(window.end).map_err(|_width| IndexError::ResourceLimited)?,
        };
        let mut owned = String::new();
        owned
            .try_reserve_exact(text.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        owned.push_str(text);
        excerpt = Some((
            NativeCodeSearchExcerpt {
                context,
                text: owned,
            },
            grant,
        ));
        Ok(())
    })?;
    let (excerpt, grant) = excerpt.ok_or_else(|| {
        IndexError::BackendViolation("CodeSearch Source body was not visited".into())
    })?;
    Ok((Some(excerpt), grant))
}

/// The Legacy excerpt window of `start..end` in `raw`: up to
/// [`NATIVE_CODE_SEARCH_CONTEXT_BYTES`] before and after, clipped to the
/// Source, then each edge moved toward the match to a UTF-8 boundary.
pub(super) fn legacy_context_window(
    raw: &str,
    start: u64,
    end: u64,
) -> Result<Range<usize>, IndexError> {
    let misaligned =
        || IndexError::CorruptObject("CodeSearch match is not a UTF-8 range of its Source".into());
    let start = usize::try_from(start).map_err(|_width| misaligned())?;
    let end = usize::try_from(end).map_err(|_width| misaligned())?;
    if start >= end || raw.get(start..end).is_none() {
        return Err(misaligned());
    }
    let reach = usize::try_from(NATIVE_CODE_SEARCH_CONTEXT_BYTES)
        .map_err(|_width| IndexError::ResourceLimited)?;
    let mut context_start = start.saturating_sub(reach);
    while !raw.is_char_boundary(context_start) {
        context_start = context_start.checked_add(1).ok_or_else(misaligned)?;
    }
    let mut context_end = end.saturating_add(reach).min(raw.len());
    while !raw.is_char_boundary(context_end) {
        context_end = context_end.checked_sub(1).ok_or_else(misaligned)?;
    }
    Ok(context_start..context_end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(raw: &str, start: u64, end: u64) -> Option<(usize, usize)> {
        match legacy_context_window(raw, start, end) {
            Ok(range) => Some((range.start, range.end)),
            Err(_refused) => None,
        }
    }

    #[test]
    fn small_sources_show_the_whole_file() {
        // "todo!()\nfn main() {}\n" is 21 bytes; `main` is 11..15.
        assert_eq!(window("todo!()\nfn main() {}\n", 11, 15), Some((0, 21)));
        // A match at the first or last byte still clips to the file.
        assert_eq!(window("abc", 0, 1), Some((0, 3)));
        assert_eq!(window("abc", 2, 3), Some((0, 3)));
    }

    #[test]
    fn long_sources_show_120_bytes_on_each_side() {
        let raw = format!("{}needle{}", "a".repeat(300), "b".repeat(300));
        // `needle` is 300..306; the window is 180..426.
        assert_eq!(window(&raw, 300, 306), Some((180, 426)));
        // Near either edge only the in-file side is clipped.
        assert_eq!(window(&raw, 10, 16), Some((0, 136)));
        assert_eq!(window(&raw, 590, 596), Some((470, 606)));
    }

    #[test]
    fn edges_move_inward_to_utf8_boundaries() {
        // `é` is two bytes. Here 120 bytes before `x` (at 121) is byte 1, the
        // middle of the leading `é`, so the window starts at 2.
        let raw = format!("\u{e9}{}x{}\u{e9}", "a".repeat(119), "b".repeat(119));
        assert_eq!(raw.len(), 2 + 119 + 1 + 119 + 2);
        // 120 bytes after `x` (ends at 122) is byte 242, the middle of the
        // trailing `é` at 241..243, so the window ends at 241.
        assert_eq!(window(&raw, 121, 122), Some((2, 241)));
    }

    #[test]
    fn misaligned_or_empty_matches_are_refused() {
        let raw = "\u{e9}t\u{e9}";
        assert_eq!(window(raw, 1, 2), None, "split inside `é`");
        assert_eq!(window(raw, 2, 2), None, "empty");
        assert_eq!(window(raw, 3, 2), None, "reversed");
        assert_eq!(window(raw, 2, 9), None, "past the end");
        assert_eq!(window(raw, 2, 3), Some((0, 5)));
    }

    /// Over generated mixed-width texts and matches, the window holds the
    /// match, both edges are UTF-8 boundaries within 120 bytes of the match
    /// and inside the text, and no boundary further out is left unused.
    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "generated window properties are asserted against independent bounds"
    )]
    fn generated_windows_are_the_widest_aligned_ones_in_reach() -> Result<(), String> {
        const UNITS: [&str; 5] = ["a", "\n", "\u{e9}", "\u{20ac}", "\u{1f600}"];
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut draw = |bound: usize| -> Result<usize, String> {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let value = state.wrapping_mul(0x2545_f491_4f6c_dd1d);
            let bound = u64::try_from(bound).map_err(|error| error.to_string())?;
            let index = value.checked_rem(bound).ok_or("empty draw")?;
            usize::try_from(index).map_err(|error| error.to_string())
        };
        let reach = 120_usize;
        for _ in 0..400 {
            let mut raw = String::new();
            for _ in 0..draw(160)? {
                raw.push_str(UNITS.get(draw(UNITS.len())?).ok_or("unit")?);
            }
            let boundaries = (0..=raw.len())
                .filter(|index| raw.is_char_boundary(*index))
                .collect::<Vec<_>>();
            if boundaries.len() < 2 {
                continue;
            }
            let first = draw(boundaries.len().saturating_sub(1))?;
            let last = first.saturating_add(1).saturating_add(draw(4)?);
            let (Some(&start), Some(&end)) = (
                boundaries.get(first),
                boundaries.get(last.min(boundaries.len().saturating_sub(1))),
            ) else {
                return Err("generated match lost its boundaries".into());
            };
            let wire = |value: usize| u64::try_from(value).map_err(|error| error.to_string());
            let range = legacy_context_window(&raw, wire(start)?, wire(end)?)
                .map_err(|error| format!("{raw:?} {start}..{end}: {error}"))?;
            let low = start.saturating_sub(reach);
            let high = end.saturating_add(reach).min(raw.len());
            let case = format!("{raw:?} {start}..{end} -> {range:?}");
            assert!(range.start <= start && end <= range.end, "{case}");
            assert!(low <= range.start && range.end <= high, "{case}");
            assert!(raw.get(range.clone()).is_some(), "{case}");
            assert!(
                !boundaries
                    .iter()
                    .any(|edge| (low..range.start).contains(edge)
                        || (range.end.saturating_add(1)..=high).contains(edge)),
                "{case}"
            );
        }
        Ok(())
    }
}
