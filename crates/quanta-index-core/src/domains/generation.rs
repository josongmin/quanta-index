use quanta_index_contract::GenerationSnapshot;
use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
use sha2::{Digest, Sha256};
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

/// Enumerates physically valid sealed generations owned by one adapter.
///
/// The adapter owns its storage layout and sealed-identity encoding. Callers
/// receive only contract identities and must still apply their own readiness
/// policy; they must not walk adapter directories or decode adapter sidecars.
pub trait SealedGenerationScanPort: Send + Sync {
    fn scan_sealed_generations(&self) -> Result<Vec<GenerationSnapshot>, CoreError>;
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
