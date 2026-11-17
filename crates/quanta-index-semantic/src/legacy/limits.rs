//! Existing owner wire bounds; no filesystem or engine dependency.

use quanta_index_core::CoreError;

pub const MAX_SEALED_MARKER_BYTES: usize = 4 * 1024;
pub const MAX_GENERATION_CONTRACT_BYTES: usize = 1024 * 1024;
pub const MAX_SCOPE_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_SEALED_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_RECEIPT_BYTES: usize = 64 * 1024;

pub fn ensure_bounded(bytes: &[u8], max_bytes: usize, what: &str) -> Result<(), CoreError> {
    if bytes.len() > max_bytes {
        return Err(CoreError::Storage(format!(
            "semantic: {what} exceeds {max_bytes} encoded bytes"
        )));
    }
    Ok(())
}
