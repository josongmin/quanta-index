//! Validate the relative Source path carried by native requests and replies.

use quanta_index_core::index::MAX_SOURCE_PATH_BYTES;

pub(super) fn valid_native_source_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_SOURCE_PATH_BYTES
        && !path.starts_with('/')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}
