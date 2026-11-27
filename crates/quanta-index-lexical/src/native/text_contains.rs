//! Shared bounded literal containment for authenticated source bodies and paths.

use crate::plan::file_contains_raw_literal_supported;
use quanta_index_contract::MAX_NATIVE_FILE_CONTAINS_LEAVES as MAX_FILE_CONTAINS_LEAVES;
use quanta_index_core::index::{IndexError, MemoryLifetime, WorkBudget, WorkUsage};

pub(super) fn file_contains_mask(
    source_text: &str,
    needles: &[&str],
    case: super::text::CaseMode,
    budget: &mut WorkBudget,
) -> Result<u8, IndexError> {
    if needles.len() > MAX_FILE_CONTAINS_LEAVES
        || needles
            .iter()
            .any(|needle| !file_contains_raw_literal_supported(needle))
    {
        return Err(IndexError::InvalidInput(
            "native file.contains requires bounded literal raw substrings".into(),
        ));
    }
    let text_bytes =
        u64::try_from(source_text.len()).map_err(|_width| IndexError::ResourceLimited)?;
    let needle_bytes = needles.iter().try_fold(0u64, |sum, needle| {
        let len = u64::try_from(needle.len()).map_err(|_width| IndexError::ResourceLimited)?;
        sum.checked_add(len).ok_or(IndexError::ResourceLimited)
    })?;
    let backing = text_bytes
        .checked_add(needle_bytes)
        .and_then(|bytes| bytes.checked_mul(16))
        .ok_or(IndexError::ResourceLimited)?;
    let _memory = budget.reserve_memory(backing, MemoryLifetime::Temporary)?;
    budget.charge(WorkUsage {
        bytes: text_bytes,
        ..WorkUsage::default()
    })?;
    budget.checkpoint()?;
    let normalized = crate::normalize::nfc(source_text);
    let folded = crate::normalize::apply_case(normalized.as_ref(), case);
    let normalized_bytes =
        u64::try_from(normalized.len()).map_err(|_width| IndexError::ResourceLimited)?;
    let folded_bytes = u64::try_from(folded.len()).map_err(|_width| IndexError::ResourceLimited)?;
    let source_prepare_bytes = normalized_bytes
        .checked_add(folded_bytes)
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes: source_prepare_bytes,
        ..WorkUsage::default()
    })?;
    let mut mask = 0u8;
    for (ordinal, needle) in needles.iter().enumerate() {
        budget.checkpoint()?;
        let needle_len =
            u64::try_from(needle.len()).map_err(|_width| IndexError::ResourceLimited)?;
        let scan_bytes = folded_bytes
            .checked_add(needle_len)
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: scan_bytes,
            ..WorkUsage::default()
        })?;
        let normalized_needle = crate::normalize::nfc(needle);
        let folded_needle = crate::normalize::apply_case(normalized_needle.as_ref(), case);
        let normalized_needle_bytes =
            u64::try_from(normalized_needle.len()).map_err(|_width| IndexError::ResourceLimited)?;
        let folded_needle_bytes =
            u64::try_from(folded_needle.len()).map_err(|_width| IndexError::ResourceLimited)?;
        let literal_prepare_bytes = normalized_needle_bytes
            .checked_add(folded_needle_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: literal_prepare_bytes,
            ..WorkUsage::default()
        })?;
        if folded.contains(folded_needle.as_ref()) {
            mask |= 1u8
                .checked_shl(u32::try_from(ordinal).map_err(|_width| IndexError::ResourceLimited)?)
                .ok_or(IndexError::ResourceLimited)?;
        }
    }
    budget.checkpoint()?;
    Ok(mask)
}
