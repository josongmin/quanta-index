use quanta_index_contract::GenerationSnapshot;
use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Read;
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

    /// Inverse of [`Self::as_code_str`], for a reason that crossed a wire.
    #[must_use]
    pub fn from_code_str(code: &str) -> Option<Self> {
        [
            Self::NonCanonicalLayout,
            Self::IdentityUnreadable,
            Self::ScopeMismatch,
            Self::IdentityDigestMismatch,
        ]
        .into_iter()
        .find(|reason| reason.as_code_str() == code)
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

/// Wire code for a discard whose target is not quarantined right now.
pub const QUARANTINE_TARGET_NOT_QUARANTINED_CODE: &str = "QUARANTINE_TARGET_NOT_QUARANTINED";

/// What discarding a quarantined entry did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuarantineDiscardOutcomeV1 {
    /// The entry's bytes are gone; `bytes` is what was on disk before.
    Discarded { bytes: u64 },
    /// Nothing was at the path any more.
    Absent,
}

/// Destructive port for a directory the inventory set aside (QI-BB-026).
///
/// The only way a quarantined directory leaves the disk. Implementations
/// re-run their own inventory and remove `entry.path` only if that
/// inventory reports it quarantined at that moment, under the same reason:
/// a sealed generation, an in-progress build, a directory repaired since
/// the caller listed it, or any path the inventory does not name is refused
/// typed as [`QUARANTINE_TARGET_NOT_QUARANTINED_CODE`], never removed. A
/// path that is already gone is an idempotent `Absent`.
pub trait QuarantinedGenerationDiscardPort: Send + Sync {
    fn discard_quarantined_generation(
        &self,
        entry: &QuarantinedGenerationV1,
    ) -> Result<QuarantineDiscardOutcomeV1, CoreError>;
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

/// One file a sealed generation commits to: its `/`-joined path relative to
/// the generation directory, its length and its SHA-256.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SealedArtifactCommitmentV1 {
    pub name: String,
    pub bytes: u64,
    pub sha256: [u8; 32],
}

/// Length and SHA-256 of one file, streamed through a fixed buffer so the
/// cost is I/O and hashing, never the file's size in memory.
pub fn sha256_of_file(path: &Path) -> std::io::Result<(u64, [u8; 32])> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 16];
    let mut length = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let chunk = buffer.get(..read).ok_or_else(|| {
            std::io::Error::other("read returned more bytes than the buffer holds")
        })?;
        hasher.update(chunk);
        length = length
            .checked_add(u64::try_from(read).map_err(std::io::Error::other)?)
            .ok_or_else(|| std::io::Error::other("file length overflows u64"))?;
    }
    Ok((length, hasher.finalize().into()))
}

/// Commit every regular file under `root`, recursively, in sorted path order.
///
/// Names are `/`-joined paths relative to `root`'s parent as `prefix/…`, so a
/// commitment over `generation_dir/dataset` names `dataset/a/b`. Symlinks
/// are refused: a sealed tree that points outside itself cannot be committed
/// to. Directory entries themselves are not committed; a tree is its files.
pub fn commit_tree_v1(
    root: &Path,
    prefix: &str,
) -> std::io::Result<Vec<SealedArtifactCommitmentV1>> {
    let mut artifacts = Vec::new();
    let mut pending = vec![(root.to_path_buf(), prefix.to_string())];
    while let Some((directory, name)) = pending.pop() {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let file_name = entry.file_name();
            let file_name = file_name.to_str().ok_or_else(|| {
                std::io::Error::other(format!("non-UTF-8 file name under {}", directory.display()))
            })?;
            entries.push((
                format!("{name}/{file_name}"),
                entry.path(),
                entry.file_type()?,
            ));
        }
        for (entry_name, path, file_type) in entries {
            if file_type.is_symlink() {
                return Err(std::io::Error::other(format!(
                    "refusing to commit symlink {entry_name}"
                )));
            }
            if file_type.is_dir() {
                pending.push((path, entry_name));
            } else if file_type.is_file() {
                let (bytes, sha256) = sha256_of_file(&path)?;
                artifacts.push(SealedArtifactCommitmentV1 {
                    name: entry_name,
                    bytes,
                    sha256,
                });
            }
        }
    }
    artifacts.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(artifacts)
}

/// The first way a tree differed from its commitment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TreeCommitmentMismatchV1 {
    /// A committed file is gone.
    Missing { name: String },
    /// A file exists that the seal did not commit to; for a versioned
    /// dataset an extra file can change what opens, so it is a defect.
    Extra { name: String },
    /// A committed file has a different length.
    Length {
        name: String,
        on_disk: u64,
        committed: u64,
    },
    /// A committed file has different content.
    Digest { name: String },
}

impl fmt::Display for TreeCommitmentMismatchV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { name } => write!(formatter, "{name}: missing"),
            Self::Extra { name } => {
                write!(
                    formatter,
                    "{name}: present although the seal did not commit to it"
                )
            }
            Self::Length {
                name,
                on_disk,
                committed,
            } => write!(
                formatter,
                "{name}: {on_disk} bytes on disk, {committed} committed"
            ),
            Self::Digest { name } => {
                write!(
                    formatter,
                    "{name}: content digest differs from the committed digest"
                )
            }
        }
    }
}

impl std::error::Error for TreeCommitmentMismatchV1 {}

/// Re-measure `root` and compare it with `committed`, file set included.
///
/// `Ok(Ok(bytes))` is the committed tree's total size; `Ok(Err(..))` is the
/// first mismatch in path order; `Err` is an I/O failure that says nothing
/// about the commitment. This is the check an activation validator and a
/// cold open share: it reads and hashes every committed file once, which is
/// I/O proportional to the tree's bytes with bounded memory, and never
/// decodes a row.
pub fn verify_tree_commitment_v1(
    root: &Path,
    prefix: &str,
    committed: &[SealedArtifactCommitmentV1],
) -> std::io::Result<Result<u64, TreeCommitmentMismatchV1>> {
    let on_disk = commit_tree_v1(root, prefix)?;
    let by_name: BTreeMap<&str, &SealedArtifactCommitmentV1> = on_disk
        .iter()
        .map(|artifact| (artifact.name.as_str(), artifact))
        .collect();
    let mut total = 0_u64;
    for artifact in committed {
        let Some(found) = by_name.get(artifact.name.as_str()) else {
            return Ok(Err(TreeCommitmentMismatchV1::Missing {
                name: artifact.name.clone(),
            }));
        };
        if found.bytes != artifact.bytes {
            return Ok(Err(TreeCommitmentMismatchV1::Length {
                name: artifact.name.clone(),
                on_disk: found.bytes,
                committed: artifact.bytes,
            }));
        }
        if found.sha256 != artifact.sha256 {
            return Ok(Err(TreeCommitmentMismatchV1::Digest {
                name: artifact.name.clone(),
            }));
        }
        total = total.saturating_add(artifact.bytes);
    }
    let committed_names: BTreeSet<&str> = committed
        .iter()
        .map(|artifact| artifact.name.as_str())
        .collect();
    for artifact in &on_disk {
        if !committed_names.contains(artifact.name.as_str()) {
            return Ok(Err(TreeCommitmentMismatchV1::Extra {
                name: artifact.name.clone(),
            }));
        }
    }
    Ok(Ok(total))
}

#[cfg(test)]
mod tree_commitment_tests {
    use super::{
        TreeCommitmentMismatchV1, commit_tree_v1, sha256_of_file, verify_tree_commitment_v1,
    };

    fn write(root: &std::path::Path, name: &str, bytes: &[u8]) {
        let path = root.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(path, bytes).expect("write file");
    }

    #[test]
    fn a_tree_verifies_against_its_own_commitment_and_reports_its_bytes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("dataset");
        write(&root, "a.lance", b"alpha");
        write(&root, "sub/b.manifest", b"bravo!");
        let committed = commit_tree_v1(&root, "dataset").expect("commit");
        assert_eq!(
            committed
                .iter()
                .map(|artifact| artifact.name.as_str())
                .collect::<Vec<_>>(),
            vec!["dataset/a.lance", "dataset/sub/b.manifest"]
        );
        let verified = verify_tree_commitment_v1(&root, "dataset", &committed).expect("io");
        assert_eq!(verified, Ok(11));
    }

    #[test]
    fn every_way_a_tree_can_drift_is_named_in_path_order() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("dataset");
        write(&root, "a.lance", b"alpha");
        write(&root, "sub/b.manifest", b"bravo!");
        let committed = commit_tree_v1(&root, "dataset").expect("commit");

        std::fs::write(root.join("a.lance"), b"alphA").expect("flip");
        assert_eq!(
            verify_tree_commitment_v1(&root, "dataset", &committed).expect("io"),
            Err(TreeCommitmentMismatchV1::Digest {
                name: "dataset/a.lance".to_string()
            })
        );
        std::fs::write(root.join("a.lance"), b"alph").expect("truncate");
        assert_eq!(
            verify_tree_commitment_v1(&root, "dataset", &committed).expect("io"),
            Err(TreeCommitmentMismatchV1::Length {
                name: "dataset/a.lance".to_string(),
                on_disk: 4,
                committed: 5
            })
        );
        std::fs::remove_file(root.join("a.lance")).expect("remove");
        assert_eq!(
            verify_tree_commitment_v1(&root, "dataset", &committed).expect("io"),
            Err(TreeCommitmentMismatchV1::Missing {
                name: "dataset/a.lance".to_string()
            })
        );
        write(&root, "a.lance", b"alpha");
        write(&root, "sub/999.manifest", b"foreign");
        assert_eq!(
            verify_tree_commitment_v1(&root, "dataset", &committed).expect("io"),
            Err(TreeCommitmentMismatchV1::Extra {
                name: "dataset/sub/999.manifest".to_string()
            })
        );
    }

    #[test]
    fn a_streamed_digest_matches_a_one_shot_digest() {
        use sha2::Digest as _;
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("big");
        let content = vec![0xAB_u8; (1 << 16) * 3 + 17];
        std::fs::write(&path, &content).expect("write");
        let (length, digest) = sha256_of_file(&path).expect("hash");
        assert_eq!(length, u64::try_from(content.len()).expect("len"));
        let expected: [u8; 32] = sha2::Sha256::digest(&content).into();
        assert_eq!(digest, expected);
    }
}
