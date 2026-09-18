use quanta_index_contract::{
    GenerationPin, GenerationSnapshot, ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::CoreError;

/// Wire code for a pin the durable authority does not retain.
///
/// Reaped by retention, orphaned by a crash between seal and record, or
/// never sealed. The refusal is typed so a caller can tell "this
/// generation is gone" from "this generation is not ready yet"
/// (`NOT_READY`); neither is ever answered from another generation.
pub const UNKNOWN_GENERATION_CODE: &str = "UNKNOWN_GENERATION";

/// The typed refusal for a pin the durable authority does not retain.
#[must_use]
pub fn unknown_generation_error(
    plane: &str,
    pin: &GenerationPin,
    track: SearchPlaneTrackKind,
) -> CoreError {
    CoreError::Typed {
        code: UNKNOWN_GENERATION_CODE.to_string(),
        message: format!(
            "{plane}: generation {} of repo={} revision={} track={track:?} is not a sealed generation the durable search-corpus authority retains (reaped, orphaned or never sealed)",
            pin.manifest_generation.get(),
            pin.repo_id.as_str(),
            pin.revision_id.as_str(),
        ),
    }
}

/// What the readiness authority knows about one track of a pair when a
/// request pins a generation on it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PinnedGenerationReadinessV1 {
    /// The durable search-corpus authority retains the pinned generation
    /// as a sealed identity (recorded at seal, restored at boot, pruned on
    /// reap).
    pub retained_by_authority: bool,
    /// The highest generation the track has materialized, sealed or not.
    pub materialized_head: Option<ManifestGeneration>,
    /// The highest generation the track has sealed.
    pub sealed_head: Option<ManifestGeneration>,
}

/// The serving boundary of one pinned track generation (QI-BB-003).
///
/// The one policy every query route resolves a pin through, for both
/// tracks. A pin is serveable only when the durable authority retains that
/// exact sealed generation; nothing else is served, not from a resident
/// handle and not from a cold open. Of what is not serveable:
///
/// - a pin past the track's materialized head, or the head itself while
///   it is still being built, is `NOT_READY` — it may become serveable;
/// - everything else — reaped, a crash orphan whose directory is still
///   there, never sealed — is [`UNKNOWN_GENERATION_CODE`]; it will not.
///
/// Neither is ever answered from another generation.
pub fn validate_pinned_generation_v1(
    plane: &str,
    pin: &GenerationPin,
    track: SearchPlaneTrackKind,
    readiness: PinnedGenerationReadinessV1,
) -> Result<(), CoreError> {
    if readiness.retained_by_authority {
        return Ok(());
    }
    let generation = pin.manifest_generation;
    match readiness.materialized_head {
        None => Err(CoreError::NotReady(format!(
            "{plane}: no materialized {track:?} generation yet for repo={} revision={}",
            pin.repo_id.as_str(),
            pin.revision_id.as_str(),
        ))),
        Some(head) if generation.get() > head.get() => Err(CoreError::NotReady(format!(
            "{plane}: requested {track:?} generation {} but materialized only up to {} for repo={} revision={}",
            generation.get(),
            head.get(),
            pin.repo_id.as_str(),
            pin.revision_id.as_str(),
        ))),
        Some(head)
            if generation == head
                && readiness
                    .sealed_head
                    .is_none_or(|sealed| sealed.get() < head.get()) =>
        {
            Err(CoreError::NotReady(format!(
                "{plane}: {track:?} generation {} is materialized but not sealed yet for repo={} revision={}",
                generation.get(),
                pin.repo_id.as_str(),
                pin.revision_id.as_str(),
            )))
        }
        Some(_) => Err(unknown_generation_error(plane, pin, track)),
    }
}

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
/// All but one reason are things the adapter's inventory can see from the
/// sealed identity alone. [`Self::Orphaned`] is the search plane's: it
/// compares the adapter's inventory with the durable search-corpus
/// authority and sets aside every sealed directory the authority does not
/// retain. Content defects (a corrupt sidecar, a row root that no longer
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
    /// A sealed directory whose exact `(generation, digest)` the durable
    /// search-corpus authority does not retain: reaped and left behind by
    /// a crash before reclaim, or sealed on disk before its authority
    /// record was ever written. It is never seeded as sealed and nothing
    /// serves it; discarding it goes through the sealed-generation reclaim
    /// port, which re-proves the identity before deleting.
    Orphaned,
}

impl GenerationQuarantineReasonV1 {
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::NonCanonicalLayout => "GENERATION_QUARANTINE_NON_CANONICAL_LAYOUT",
            Self::IdentityUnreadable => "GENERATION_QUARANTINE_IDENTITY_UNREADABLE",
            Self::ScopeMismatch => "GENERATION_QUARANTINE_SCOPE_MISMATCH",
            Self::IdentityDigestMismatch => "GENERATION_QUARANTINE_IDENTITY_DIGEST_MISMATCH",
            Self::Orphaned => "GENERATION_QUARANTINE_ORPHANED",
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
            Self::Orphaned,
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

/// One sealed generation the inventory found: its identity and the
/// adapter-owned directory it was read from.
///
/// The path is what lets the search plane name the directory back to the
/// operator (as an orphan) without knowing the adapter's layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoriedSealedGenerationV1 {
    pub identity: GenerationSnapshot,
    pub path: PathBuf,
}

/// What one adapter found under its track root at boot.
///
/// `sealed` carries every generation whose sealed identity was readable and
/// owns its path; nothing about their content has been verified. Callers
/// decide which of them boot must prove (the active set) and prove exactly
/// those, once, through [`GenerationIdentityValidatePort`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SealedGenerationInventoryV1 {
    pub sealed: Vec<InventoriedSealedGenerationV1>,
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

/// What measuring a set of generation directories found (QI-BB-003).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SealedGenerationBytesV1 {
    /// Bytes the listed directories occupy together: every regular file
    /// counted once by inode, so a segment two generations hard-link is
    /// not counted twice. This is what `du` reports for the same set.
    pub bytes: u64,
    /// Generations in the request that have no directory on disk; they
    /// occupy nothing and are reported so the caller can see a retained
    /// record whose bytes are gone.
    pub absent: BTreeSet<ManifestGeneration>,
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
/// files and no open in flight can admit one), then reclaim. A crash between
/// reap and reclaim leaves an orphan that the next sweep finds through
/// [`Self::sealed_generations_for_pair`] and that boot reports as
/// quarantined under [`GenerationQuarantineReasonV1::Orphaned`].
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

    /// The bytes `generations` of the pair occupy on disk together, by
    /// unique inode (see [`SealedGenerationBytesV1`]). This is the measure
    /// retention caps and reports: the actual index bytes, never the size
    /// of an authority record. Measured from metadata only; nothing is
    /// opened or hashed.
    fn measure_sealed_generations(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generations: &BTreeSet<ManifestGeneration>,
    ) -> Result<SealedGenerationBytesV1, CoreError>;
}

/// Bytes of every regular file under `roots`, each inode counted once.
///
/// Symlinks are not followed and not counted; a sealed tree that points
/// outside itself is refused by the seal, not measured here. Entries
/// `skip` names (a live writer's lock file, for instance) are ignored.
/// A root that does not exist is an error: the caller decides what an
/// absent root means. Below the roots the tree may be live — a seal
/// renaming its temporaries, the index engine deleting merged segments, a
/// reclaim removing a generation — so an entry that vanishes between the
/// listing and its stat is not on disk any more and is not counted; every
/// other I/O error is returned.
pub fn unique_inode_tree_bytes(
    roots: &[PathBuf],
    skip: &dyn Fn(&str) -> bool,
) -> std::io::Result<u64> {
    let mut seen: BTreeSet<(u64, u64)> = BTreeSet::new();
    let mut total = 0_u64;
    let mut pending: Vec<(PathBuf, bool)> = roots.iter().map(|root| (root.clone(), true)).collect();
    while let Some((directory, is_root)) = pending.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if !is_root && error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for entry in entries {
            let Some(entry) = vanished_is_none(entry)? else {
                continue;
            };
            let file_name = entry.file_name();
            if file_name.to_str().is_some_and(skip) {
                continue;
            }
            let Some(file_type) = vanished_is_none(entry.file_type())? else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                pending.push((entry.path(), false));
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let Some(metadata) = vanished_is_none(entry.metadata())? else {
                continue;
            };
            if seen.insert((metadata.dev(), metadata.ino())) {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    Ok(total)
}

/// `None` when the entry vanished (`NotFound`) while the tree was walked.
fn vanished_is_none<T>(result: std::io::Result<T>) -> std::io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
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
mod unique_inode_bytes_tests {
    use super::unique_inode_tree_bytes;

    /// Two generation directories that hard-link a segment occupy the
    /// segment once; a symlink and a skipped entry occupy nothing.
    #[test]
    fn hard_linked_files_are_counted_once_across_roots() {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path().join("g1");
        let delta = temp.path().join("g2");
        std::fs::create_dir_all(base.join("sub")).expect("mkdir");
        std::fs::create_dir_all(&delta).expect("mkdir");
        std::fs::write(base.join("segment"), [0_u8; 1000]).expect("write");
        std::fs::write(base.join("sub").join("meta"), [0_u8; 10]).expect("write");
        std::fs::hard_link(base.join("segment"), delta.join("segment")).expect("link");
        std::fs::write(delta.join("delta-only"), [0_u8; 5]).expect("write");
        std::fs::write(delta.join(".lock"), [0_u8; 99]).expect("write");
        std::os::unix::fs::symlink(base.join("segment"), delta.join("alias")).expect("symlink");

        let skip = |name: &str| name.starts_with(".lock");
        let both = unique_inode_tree_bytes(&[base.clone(), delta.clone()], &skip).expect("io");
        assert_eq!(both, 1000 + 10 + 5);
        let delta_alone = unique_inode_tree_bytes(&[delta], &skip).expect("io");
        assert_eq!(delta_alone, 1000 + 5);
        let base_alone = unique_inode_tree_bytes(&[base], &skip).expect("io");
        assert_eq!(base_alone, 1000 + 10);
    }

    #[test]
    fn a_missing_root_is_an_error_not_zero() {
        let temp = tempfile::tempdir().expect("tempdir");
        let missing = temp.path().join("absent");
        assert!(unique_inode_tree_bytes(&[missing], &|_name| false).is_err());
    }
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
