//! Native repository metadata normalization.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; its item is used across modules"
)]

pub(crate) fn normalize_language(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_lowercase())
    }
}
