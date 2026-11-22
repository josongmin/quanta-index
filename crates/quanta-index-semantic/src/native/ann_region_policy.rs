//! Persisted region envelope shared by the format 4 graph reader and format 5
//! Lance reader/writer. Changing these limits requires a format compatibility
//! decision for both stored formats.

pub(super) const MAX_REGION_ROWS: usize = 8192;
pub(super) const MAX_REGION_DIMENSION: usize = 4096;
pub(super) const MAX_REGION_CELLS: usize = 1_048_576;
