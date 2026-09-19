//! The one normalization every repo-metadata value goes through before it is stored or matched.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::predicate_registry::{MetaPattern, RepoMetaArgError};
use quanta_index_contract::{FileContributorIdentityEntry, LqOptions, LqPatternType};

/// Build an `LqOptions` snapshot pinned to the standard pattern type.
///
/// Used by internal scope-discovery compiles (predicate path lowering) where
/// the caller's regex options should NOT bleed into the discovery query —
/// those are user-facing leaves planned separately.
pub(crate) fn standard_pattern_options() -> LqOptions {
    let mut opts = LqOptions::defaults();
    opts.pattern_type = LqPatternType::Standard;
    opts
}

pub(crate) fn normalize_language(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_lowercase())
    }
}

pub(crate) fn normalize_repo_meta_key(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_lowercase())
    }
}

pub(crate) fn normalize_repo_meta_pattern(
    pattern: MetaPattern,
) -> Result<MetaPattern, RepoMetaArgError> {
    match pattern {
        MetaPattern::Exact(value) => normalize_repo_meta_key(&value)
            .map(MetaPattern::Exact)
            .ok_or(RepoMetaArgError::EmptyKey),
        MetaPattern::Regex(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Err(RepoMetaArgError::EmptyKey)
            } else {
                Ok(MetaPattern::Regex(trimmed.to_string()))
            }
        }
    }
}

pub(crate) fn normalize_repo_topic_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_lowercase())
    }
}

pub(crate) fn normalize_owner_identity(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_lowercase())
    }
}

pub(crate) fn normalize_contributor_identity(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_lowercase())
    }
}

pub(crate) fn normalize_contributor_identity_entry(
    entry: &FileContributorIdentityEntry,
) -> Option<FileContributorIdentityEntry> {
    let canonical = normalize_contributor_identity(&entry.canonical)?;
    let name = entry
        .name
        .as_deref()
        .and_then(normalize_contributor_identity);
    let email = entry
        .email
        .as_deref()
        .and_then(normalize_contributor_identity);
    Some(FileContributorIdentityEntry {
        canonical,
        name,
        email,
    })
}
