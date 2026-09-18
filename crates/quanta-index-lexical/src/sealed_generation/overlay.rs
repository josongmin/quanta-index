//! The repo-metadata overlay sidecars: one file per family beside the index.
//!
//! Seven families, each a whole-snapshot CBOR file a query decodes at open:
//! the `FullBundle` repo-metadata payload and the six source-repo keyed
//! authorities the aux ingest routes publish. They are written into the
//! generation before its seal, by atomic durable rename, and the seal lists
//! every family present with its length and digest; after the seal no
//! family may change (QI-BB-030). A family the seal did not list is one the
//! generation does not carry, and a query asking for it is refused typed —
//! never one that was lost.

use std::fs::File;
use std::path::{Path, PathBuf};

use quanta_index_core::CoreError;

/// One overlay family, by the file it lives in.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum OverlayFamily {
    /// The `FullBundle` repo-metadata payload (fork, archived, visibility,
    /// contexts).
    RepoMetadata,
    /// Latest committer time per source repo.
    CommitRecency,
    /// `key:value` metadata per source repo.
    Meta,
    /// Topics per source repo.
    Topic,
    /// Description per source repo.
    Description,
    /// Owners per file.
    FileOwnership,
    /// Contributors per file.
    Contributor,
}

impl OverlayFamily {
    /// Every family, in manifest order.
    pub(crate) const ALL: [Self; 7] = [
        Self::RepoMetadata,
        Self::CommitRecency,
        Self::Meta,
        Self::Topic,
        Self::Description,
        Self::FileOwnership,
        Self::Contributor,
    ];

    /// The family's file name inside the generation directory.
    pub(crate) const fn file_name(self) -> &'static str {
        match self {
            Self::RepoMetadata => "repo-metadata.cbor",
            Self::CommitRecency => "repo-commit-recency.cbor",
            Self::Meta => "repo-meta.cbor",
            Self::Topic => "repo-topic.cbor",
            Self::Description => "repo-description.cbor",
            Self::FileOwnership => "file-ownership.cbor",
            Self::Contributor => "file-contributor.cbor",
        }
    }

    /// The family a manifest entry names, if any.
    pub(crate) fn from_file_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|family| family.file_name() == name)
    }

    /// The family's path inside `generation_dir`.
    pub(crate) fn path(self, generation_dir: &Path) -> PathBuf {
        generation_dir.join(self.file_name())
    }
}

/// Write one family's snapshot durably: temporary file, fsync, rename,
/// directory fsync. Creates the generation directory when this is the first
/// thing published into it.
pub(crate) fn persist_overlay(
    generation_dir: &Path,
    family: OverlayFamily,
    bytes: &[u8],
) -> Result<(), CoreError> {
    std::fs::create_dir_all(generation_dir).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: create generation directory {} for {}: {err}",
            generation_dir.display(),
            family.file_name()
        ))
    })?;
    crate::write_atomic_durable(&family.path(generation_dir), bytes, family.file_name())
}

/// Remove one family's snapshot durably; an absent file is already removed.
pub(crate) fn remove_overlay(
    generation_dir: &Path,
    family: OverlayFamily,
) -> Result<(), CoreError> {
    let path = family.path(generation_dir);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(CoreError::Storage(format!(
                "lexical: remove {} snapshot {}: {err}",
                family.file_name(),
                path.display()
            )));
        }
    }
    File::open(generation_dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|err| {
            CoreError::Storage(format!(
                "lexical: fsync {} after removing {}: {err}",
                generation_dir.display(),
                family.file_name()
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::OverlayFamily;

    #[test]
    fn every_family_round_trips_through_its_file_name() {
        for family in OverlayFamily::ALL {
            assert_eq!(
                OverlayFamily::from_file_name(family.file_name()),
                Some(family)
            );
        }
        assert_eq!(OverlayFamily::from_file_name("meta.json"), None);
    }
}
