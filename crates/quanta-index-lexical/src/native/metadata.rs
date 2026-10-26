//! Original-funded parsing for private fixed-field native lexical metadata.

use quanta_index_core::index::{IndexError, MemoryLifetime, WorkBudget};
use serde::de::DeserializeOwned;

/// Fixed identities/scalars only; no owned path/position data escapes.
pub(super) fn decode_fixed<T: Copy + DeserializeOwned>(
    bytes: &[u8],
    budget: &WorkBudget,
) -> Result<T, IndexError> {
    budget.checkpoint()?;
    if bytes.len() > quanta_index_core::index::MAX_BINDING_METADATA_BYTES {
        return Err(IndexError::CorruptObject(
            "native lexical fixed metadata exceeds its byte bound".into(),
        ));
    }
    let allowance = bytes
        .len()
        .checked_mul(8)
        .and_then(|size| size.checked_add(512))
        .ok_or(IndexError::ResourceLimited)?;
    let _memory = budget.reserve_memory(
        u64::try_from(allowance).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let result =
        serde_json::from_slice(bytes).map_err(|error| IndexError::CorruptObject(error.to_string()));
    budget.checkpoint()?;
    result
}
