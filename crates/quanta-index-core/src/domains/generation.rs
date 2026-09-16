use quanta_index_contract::GenerationSnapshot;
use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind};
use sha2::{Digest, Sha256};
use std::fmt;
use std::path::{Path, PathBuf};

use crate::CoreError;

/// Non-mutating physical validation of one sealed generation identity.
///
/// Implementations must bypass query caches, must not create missing storage,
/// and must validate the durable manifest digest before returning success.
pub trait GenerationIdentityValidatePort: Send + Sync {
    fn validate_generation_identity(&self, candidate: &GenerationSnapshot)
    -> Result<(), CoreError>;
}

/// Why boot set a persisted generation aside instead of seeding it
/// (QI-BB-026).
///
/// Each reason is something the inventory can see from the sealed identity
/// alone. Content defects (a corrupt sidecar, a row root that no longer
/// matches) are deliberately not here: the inventory does not look for them,
/// and every door that serves or mutates a generation verifies content for
/// itself.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum GenerationQuarantineReasonV1 {
    /// A directory under the track root that is neither a canonical
    /// generation family nor a `g<N>` generation directory.
    NonCanonicalLayout,
    /// The sealed identity (or manifest) could not be read or decoded.
    IdentityUnreadable,
    /// The identity names a different `(repo, revision, generation)` than the
    /// directory it sits in.
    ScopeMismatch,
    /// The sealed marker and the manifest disagree on the digest.
    IdentityDigestMismatch,
}

impl GenerationQuarantineReasonV1 {
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::NonCanonicalLayout => "GENERATION_QUARANTINE_NON_CANONICAL_LAYOUT",
            Self::IdentityUnreadable => "GENERATION_QUARANTINE_IDENTITY_UNREADABLE",
            Self::ScopeMismatch => "GENERATION_QUARANTINE_SCOPE_MISMATCH",
            Self::IdentityDigestMismatch => "GENERATION_QUARANTINE_IDENTITY_DIGEST_MISMATCH",
        }
    }
}

impl fmt::Display for GenerationQuarantineReasonV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_code_str())
    }
}

/// One persisted generation the inventory refused to seed.
///
/// Carries where it is and why, so an operator can repair or remove it. It
/// is excluded from readiness: nothing can serve, activate, or build on it
/// until it is fixed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuarantinedGenerationV1 {
    pub track: SearchPlaneTrackKind,
    /// The adapter-owned directory (family or generation) that was set aside.
    pub path: PathBuf,
    pub reason: GenerationQuarantineReasonV1,
    pub detail: String,
}

/// What one adapter found under its track root at boot.
///
/// `sealed` carries every generation whose sealed identity was readable and
/// owns its path; nothing about their content has been verified. Callers
/// decide which of them boot must prove (the active set) and prove exactly
/// those, once, through [`GenerationIdentityValidatePort`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SealedGenerationInventoryV1 {
    pub sealed: Vec<GenerationSnapshot>,
    pub quarantined: Vec<QuarantinedGenerationV1>,
}

/// Inventories the sealed generations owned by one adapter (QI-BB-026).
///
/// This is the cheap boot step: it reads only each generation's sealed
/// identity, never its content, so its cost is proportional to the number of
/// generations on disk rather than their bytes. A generation whose identity
/// cannot be read, or does not own its directory, is reported as quarantined
/// instead of failing the whole inventory; only an unreadable track root is
/// an error. The adapter owns its storage layout and sealed-identity
/// encoding: callers receive contract identities and must not walk adapter
/// directories or decode adapter sidecars.
pub trait SealedGenerationScanPort: Send + Sync {
    fn inventory_sealed_generations(&self) -> Result<SealedGenerationInventoryV1, CoreError>;
}

/// Bounded filesystem key for one logical `(repo, revision)` generation family.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GenerationStorageKeyV1(String);

impl GenerationStorageKeyV1 {
    const DOMAIN: &'static [u8] = b"quanta-index-generation-storage-key-v1\0";
    const PREFIX: &'static str = "generation-v1-";

    #[must_use]
    pub fn for_repo_revision(repo_id: &RepoId, revision_id: &RevisionId) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(Self::DOMAIN);
        for value in [repo_id.as_str(), revision_id.as_str()] {
            hasher.update(value.len().to_string().as_bytes());
            hasher.update([0]);
            hasher.update(value.as_bytes());
        }
        let digest = hasher.finalize();
        Self(format!("{}{digest:x}", Self::PREFIX))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    #[must_use]
    pub fn is_canonical_name(value: &str) -> bool {
        let Some(digest) = value.strip_prefix(Self::PREFIX) else {
            return false;
        };
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }

    #[must_use]
    pub fn generation_dir(&self, root: &Path, generation: ManifestGeneration) -> PathBuf {
        root.join(self.as_str())
            .join(format!("g{}", generation.get()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IncompleteGenerationDiscardOutcomeV1 {
    Absent,
    Discarded,
}

/// Destructive recovery port restricted to an unsealed/incomplete generation.
///
/// Implementations must refuse a sealed exact identity and any observed digest
/// conflict. Missing storage is an idempotent `Absent` outcome.
pub trait IncompleteGenerationDiscardPort: Send + Sync {
    fn discard_incomplete_generation(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<IncompleteGenerationDiscardOutcomeV1, CoreError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SealedGenerationReclaimOutcomeV1 {
    /// No durable state for the identity; nothing to do.
    Absent,
    /// The generation's bytes are gone; `bytes` is what was measured on disk
    /// before deletion.
    Reclaimed { bytes: u64 },
}

/// Destructive port for a sealed generation the search plane has retired
/// from its authority (physical GC, QI-BB-003).
///
/// The counterpart of [`IncompleteGenerationDiscardPort`]: that one refuses
/// sealed identities, this one refuses everything else. Implementations must
/// verify the durable identity (scope and digest) against `retired` before
/// deleting, so an authority record that was reaped can never delete a
/// generation that was re-sealed under a different digest or that belongs to
/// another scope. Missing storage is an idempotent `Absent`. A generation
/// directory without a sealed identity is not this port's to remove.
///
/// The owner sequences the protocol around this port: authority reaped and
/// the in-memory ledger reconciled first (so no query can pin the generation
/// any more), the snapshot registry fenced (so no resident handle maps its
/// files), then reclaim. A crash between reap and reclaim leaves an orphan
/// that the next sweep finds through
/// [`Self::sealed_generations_for_pair`].
pub trait SealedGenerationReclaimPort: Send + Sync {
    fn reclaim_sealed_generation(
        &self,
        retired: &GenerationSnapshot,
    ) -> Result<SealedGenerationReclaimOutcomeV1, CoreError>;

    /// Every sealed generation present on disk for the pair, with its durable
    /// digest, in ascending generation order. Unsealed directories are not
    /// listed: they belong to the incomplete-generation protocol.
    fn sealed_generations_for_pair(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Vec<GenerationSnapshot>, CoreError>;
}

#[cfg(test)]
mod tests {
    use std::path::Component;

    use super::GenerationStorageKeyV1;
    use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};

    #[test]
    fn storage_key_is_bounded_and_distinguishes_delimiter_ambiguous_pairs() {
        let first = GenerationStorageKeyV1::for_repo_revision(
            &RepoId::new("repo--with--delimiter"),
            &RevisionId::new("rev"),
        );
        let second = GenerationStorageKeyV1::for_repo_revision(
            &RepoId::new("repo"),
            &RevisionId::new("with--delimiter--rev"),
        );
        assert_ne!(first, second);
        assert_eq!(first.as_str().len(), "generation-v1-".len() + 64);
    }

    #[test]
    fn storage_key_has_a_stable_versioned_golden_vector() {
        let key = GenerationStorageKeyV1::for_repo_revision(
            &RepoId::new("repo-alpha"),
            &RevisionId::new("rev-alpha"),
        );
        assert_eq!(
            key.as_str(),
            "generation-v1-8b61e07455ecac08e19bd730eea2a4bf87abe9eaab7c7386238e957eadbc0d08"
        );
        assert!(GenerationStorageKeyV1::is_canonical_name(key.as_str()));
        assert!(!GenerationStorageKeyV1::is_canonical_name("repo-alpha"));
        assert!(!GenerationStorageKeyV1::is_canonical_name(
            "generation-v1-ABCDEF"
        ));
    }

    #[test]
    fn storage_key_contains_traversal_absolute_and_long_ids_under_root() {
        let root = std::path::Path::new("/state/indexes/lexical");
        for (repo, revision) in [
            ("../../outside".to_string(), "/absolute".to_string()),
            ("repo/child".to_string(), "rev\\child".to_string()),
            ("r".repeat(16_384), "v".repeat(16_384)),
        ] {
            let key = GenerationStorageKeyV1::for_repo_revision(
                &RepoId::new(repo),
                &RevisionId::new(revision),
            );
            let path = key.generation_dir(root, ManifestGeneration::new(17));
            let relative = path
                .strip_prefix(root)
                .expect("path must remain under root");
            assert_eq!(relative.components().count(), 2);
            assert!(
                relative
                    .components()
                    .all(|component| matches!(component, Component::Normal(_)))
            );
            assert!(key.as_str().len() < 96);
        }
    }
}
