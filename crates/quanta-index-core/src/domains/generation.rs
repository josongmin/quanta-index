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
pub const UNKNOWN_GENERATION_CODE: quanta_index_contract::SearchPlaneErrorCodeV2 =
    quanta_index_contract::SearchPlaneErrorCodeV2::UnknownGeneration;

/// The typed refusal for a pin the durable authority does not retain.
#[must_use]
pub fn unknown_generation_error(
    plane: &str,
    pin: &GenerationPin,
    track: SearchPlaneTrackKind,
) -> CoreError {
    CoreError::Typed {
        code: UNKNOWN_GENERATION_CODE,
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
/// Each reason is something the inventory can see without reading content:
/// the sealed identity, its format, and a quarantine receipt left behind.
/// The inventory never hashes a dataset itself; a content defect enters
/// here only once it was proved and a durable quarantine receipt recorded
/// beside the generation — by the integrity scrub (QI-BB-017), or by the
/// owning adapter re-proving what a door found at activation or rollback —
/// and every door that serves or mutates a generation still verifies what
/// it opens for itself.
/// [`Self::Orphaned`] is the search plane's: it compares the adapter's
/// inventory with the durable search-corpus authority and sets aside every
/// sealed directory the authority does not retain.
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
    /// The sealed identity decodes but was written in a format this adapter
    /// no longer serves; the generation must be rebuilt from its producer.
    FormatUnsupported,
    /// A committed file no longer matches the seal (missing, resized,
    /// rewritten, or a file the seal never listed), proved by the integrity
    /// scrub or by the adapter re-proving a door's finding at activation or
    /// rollback, and the receipt was left; nothing serves the generation,
    /// and no activation or rollback picks it, until it is discarded or
    /// rebuilt.
    ContentCorrupt,
}

impl GenerationQuarantineReasonV1 {
    /// Every reason, in declaration order.
    pub const ALL: [Self; 7] = [
        Self::NonCanonicalLayout,
        Self::IdentityUnreadable,
        Self::ScopeMismatch,
        Self::IdentityDigestMismatch,
        Self::Orphaned,
        Self::FormatUnsupported,
        Self::ContentCorrupt,
    ];

    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::NonCanonicalLayout => "GENERATION_QUARANTINE_NON_CANONICAL_LAYOUT",
            Self::IdentityUnreadable => "GENERATION_QUARANTINE_IDENTITY_UNREADABLE",
            Self::ScopeMismatch => "GENERATION_QUARANTINE_SCOPE_MISMATCH",
            Self::IdentityDigestMismatch => "GENERATION_QUARANTINE_IDENTITY_DIGEST_MISMATCH",
            Self::Orphaned => "GENERATION_QUARANTINE_ORPHANED",
            Self::FormatUnsupported => "GENERATION_QUARANTINE_FORMAT_UNSUPPORTED",
            Self::ContentCorrupt => "GENERATION_QUARANTINE_CONTENT_CORRUPT",
        }
    }

    /// Inverse of [`Self::as_code_str`], for a reason that crossed a wire.
    #[must_use]
    pub fn from_code_str(code: &str) -> Option<Self> {
        Self::ALL
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

    /// The name `generation` of this family takes in the reclaim area of
    /// its track while its removal is in progress
    /// ([`crate::domains::reclaim_area::reclaim_directory`]): unique per family and
    /// generation, and never a canonical family or generation name.
    #[must_use]
    pub fn reclaim_entry_name(&self, generation: ManifestGeneration) -> String {
        format!("{}.g{}", self.as_str(), generation.get())
    }

    /// The generation a directory named by [`Self::generation_dir`]
    /// denotes: `g<N>` in its one canonical spelling. `None` for any other
    /// name, including another spelling of a number (`g01`, `g+1`).
    #[must_use]
    pub fn generation_of_dir_name(name: &str) -> Option<ManifestGeneration> {
        let digits = name.strip_prefix('g')?;
        digits
            .parse::<u64>()
            .into_iter()
            .find(|generation| format!("g{generation}") == name)
            .map(ManifestGeneration::new)
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
pub const QUARANTINE_TARGET_NOT_QUARANTINED_CODE: quanta_index_contract::SearchPlaneErrorCodeV2 =
    quanta_index_contract::SearchPlaneErrorCodeV2::QuarantineTargetNotQuarantined;

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
/// quarantined under [`GenerationQuarantineReasonV1::Orphaned`]. The
/// reclaim itself is crash-atomic ([`crate::domains::reclaim_area`]): the directory leaves
/// the generation namespace by one durable rename before it is removed, so
/// a crash or a failed removal leaves an entry in the track's reclaim area
/// that [`Self::finish_interrupted_reclaims`] removes, never a half-removed
/// generation.
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

    /// Remove what interrupted reclaims left in this track's reclaim area:
    /// generations already out of the generation namespace whose removal a
    /// crash or a failed removal cut short. Idempotent.
    fn finish_interrupted_reclaims(&self) -> Result<FinishedReclaims, CoreError>;
}

/// What finishing the interrupted reclaims of a track did
/// ([`SealedGenerationReclaimPort::finish_interrupted_reclaims`]).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FinishedReclaims {
    /// Entries removed from the reclaim area.
    pub entries: u64,
    /// Bytes those entries occupied, each inode counted once.
    pub bytes: u64,
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
            &RepoId::new("repo--with--delimiter")
                .expect("static fixture ID satisfies canonical policy"),
            &RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
        );
        let second = GenerationStorageKeyV1::for_repo_revision(
            &RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
            &RevisionId::new("with--delimiter--rev")
                .expect("static fixture ID satisfies canonical policy"),
        );
        assert_ne!(first, second);
        assert_eq!(first.as_str().len(), "generation-v1-".len() + 64);
    }

    #[test]
    fn storage_key_has_a_stable_versioned_golden_vector() {
        let key = GenerationStorageKeyV1::for_repo_revision(
            &RepoId::new("repo-alpha").expect("static fixture ID satisfies canonical policy"),
            &RevisionId::new("rev-alpha").expect("static fixture ID satisfies canonical policy"),
        );
        assert_eq!(
            key.as_str(),
            "generation-v1-8b61e07455ecac08e19bd730eea2a4bf87abe9eaab7c7386238e957eadbc0d08"
        );
        assert!(GenerationStorageKeyV1::is_canonical_name(key.as_str()));
        assert!(!GenerationStorageKeyV1::is_canonical_name("repo-alpha"));
        assert!(!GenerationStorageKeyV1::is_canonical_name("generation-v1-ABCDEF"));
    }

    #[test]
    fn storage_key_contains_traversal_absolute_and_long_ids_under_root() {
        let root = std::path::Path::new("/state/indexes/lexical");
        for (repo, revision) in [
            ("../../outside".to_string(), "/absolute".to_string()),
            ("repo/child".to_string(), "rev\\child".to_string()),
            ("r".repeat(512), "v".repeat(512)),
        ] {
            let key = GenerationStorageKeyV1::for_repo_revision(
                &RepoId::new(repo).expect("test fixture ID satisfies canonical policy"),
                &RevisionId::new(revision).expect("test fixture ID satisfies canonical policy"),
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
        assert!(RepoId::new("r".repeat(16_384)).is_err());
        assert!(RevisionId::new("v".repeat(16_384)).is_err());
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
            entries.push((format!("{name}/{file_name}"), entry.path(), entry.file_type()?));
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
                write!(formatter, "{name}: present although the seal did not commit to it")
            }
            Self::Length {
                name,
                on_disk,
                committed,
            } => write!(formatter, "{name}: {on_disk} bytes on disk, {committed} committed"),
            Self::Digest { name } => {
                write!(formatter, "{name}: content digest differs from the committed digest")
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

/// One regular file found under a tree: its committed name and its length,
/// from directory metadata alone.
struct TreeEntryV1 {
    name: String,
    bytes: u64,
    path: PathBuf,
}

/// Walk `root` and list every regular file with its length, refusing
/// symlinks, without opening any file.
fn list_tree_v1(root: &Path, prefix: &str) -> std::io::Result<Vec<TreeEntryV1>> {
    let mut entries = Vec::new();
    let mut pending = vec![(root.to_path_buf(), prefix.to_string())];
    while let Some((directory, name)) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let file_name = entry.file_name();
            let file_name = file_name.to_str().ok_or_else(|| {
                std::io::Error::other(format!("non-UTF-8 file name under {}", directory.display()))
            })?;
            let entry_name = format!("{name}/{file_name}");
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                return Err(std::io::Error::other(format!(
                    "refusing to measure symlink {entry_name}"
                )));
            }
            if file_type.is_dir() {
                pending.push((entry.path(), entry_name));
            } else if file_type.is_file() {
                entries.push(TreeEntryV1 {
                    name: entry_name,
                    bytes: entry.metadata()?.len(),
                    path: entry.path(),
                });
            }
        }
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(entries)
}

/// Compare the file set and lengths under `root` with `committed`, reading
/// directory metadata only (QI-BB-017).
///
/// This is the cheap door check: `Ok(Ok(bytes))` is the committed total,
/// `Ok(Err(..))` the first missing, extra or resized file in path order. It
/// never opens a file, so its cost is the number of files, not their bytes;
/// a same-length rewrite passes here by design and is the scrub's to find
/// ([`scrub_tree_commitment_v1`]).
pub fn verify_tree_layout_v1(
    root: &Path,
    prefix: &str,
    committed: &[SealedArtifactCommitmentV1],
) -> std::io::Result<Result<u64, TreeCommitmentMismatchV1>> {
    let on_disk = list_tree_v1(root, prefix)?;
    let by_name: BTreeMap<&str, u64> = on_disk
        .iter()
        .map(|entry| (entry.name.as_str(), entry.bytes))
        .collect();
    let mut total = 0_u64;
    for artifact in committed {
        let Some(bytes) = by_name.get(artifact.name.as_str()) else {
            return Ok(Err(TreeCommitmentMismatchV1::Missing {
                name: artifact.name.clone(),
            }));
        };
        if *bytes != artifact.bytes {
            return Ok(Err(TreeCommitmentMismatchV1::Length {
                name: artifact.name.clone(),
                on_disk: *bytes,
                committed: artifact.bytes,
            }));
        }
        total = total.saturating_add(artifact.bytes);
    }
    let committed_names: BTreeSet<&str> = committed
        .iter()
        .map(|artifact| artifact.name.as_str())
        .collect();
    for entry in &on_disk {
        if !committed_names.contains(entry.name.as_str()) {
            return Ok(Err(TreeCommitmentMismatchV1::Extra {
                name: entry.name.clone(),
            }));
        }
    }
    Ok(Ok(total))
}

/// How one bounded scrub step ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TreeScrubVerdictV1 {
    /// Every committed file has now been hashed and matched.
    Completed,
    /// The byte budget ran out; the next step starts at this index into
    /// the committed list.
    Paused { next_artifact: u64 },
    /// The first way the tree differs from its commitment.
    Mismatch(TreeCommitmentMismatchV1),
}

/// What one bounded scrub step did and how it ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeScrubStepV1 {
    /// Committed files whose bytes were hashed and matched in this step.
    pub files_verified: u64,
    /// Bytes read and hashed in this step, including a file that turned
    /// out not to match.
    pub bytes_read: u64,
    pub verdict: TreeScrubVerdictV1,
}

/// Hash committed files from `start_artifact` until `max_bytes` is spent,
/// after a layout check of the whole tree (QI-BB-017).
///
/// The layout check makes a missing, extra or resized file visible on every
/// step, whatever the cursor; the hashing is [`hash_committed_step_v1`].
pub fn scrub_tree_commitment_v1(
    root: &Path,
    prefix: &str,
    committed: &[SealedArtifactCommitmentV1],
    start_artifact: u64,
    max_bytes: u64,
) -> std::io::Result<TreeScrubStepV1> {
    if let Err(mismatch) = verify_tree_layout_v1(root, prefix, committed)? {
        return Ok(TreeScrubStepV1 {
            files_verified: 0,
            bytes_read: 0,
            verdict: TreeScrubVerdictV1::Mismatch(mismatch),
        });
    }
    hash_committed_step_v1(
        &|name| committed_path_v1(root, prefix, name),
        committed,
        start_artifact,
        max_bytes,
    )
}

/// Hash committed files from `start_artifact` until `max_bytes` is spent
/// (QI-BB-017): the scrub's content proof, shared by every adapter.
///
/// `resolve` maps a committed name to the file to read; the caller owns the
/// layout. At least one file is hashed per step so a file larger than the
/// budget still completes; a step whose budget runs out mid-list pauses
/// with the index to resume from. The verdict is the first mismatch found;
/// `Err` is an I/O failure that proves nothing.
pub fn hash_committed_step_v1(
    resolve: &dyn Fn(&str) -> std::io::Result<PathBuf>,
    committed: &[SealedArtifactCommitmentV1],
    start_artifact: u64,
    max_bytes: u64,
) -> std::io::Result<TreeScrubStepV1> {
    let mut step = TreeScrubStepV1 {
        files_verified: 0,
        bytes_read: 0,
        verdict: TreeScrubVerdictV1::Completed,
    };
    let start = usize::try_from(start_artifact).map_err(|error| {
        std::io::Error::other(format!(
            "scrub cursor {start_artifact} does not fit this platform: {error}"
        ))
    })?;
    let Some(remaining) = committed.get(start..) else {
        return Err(std::io::Error::other(format!(
            "scrub cursor {start_artifact} is beyond the {} committed files",
            committed.len()
        )));
    };
    for (offset, artifact) in remaining.iter().enumerate() {
        if step.files_verified > 0 && step.bytes_read >= max_bytes {
            let next = start
                .checked_add(offset)
                .ok_or_else(|| std::io::Error::other("scrub cursor overflows"))?;
            step.verdict = TreeScrubVerdictV1::Paused {
                next_artifact: u64::try_from(next).map_err(|error| {
                    std::io::Error::other(format!("scrub cursor does not fit u64: {error}"))
                })?,
            };
            return Ok(step);
        }
        let path = resolve(&artifact.name)?;
        let (bytes, sha256) = match sha256_of_file(&path) {
            Ok(measured) => measured,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                step.verdict = TreeScrubVerdictV1::Mismatch(TreeCommitmentMismatchV1::Missing {
                    name: artifact.name.clone(),
                });
                return Ok(step);
            }
            Err(error) => return Err(error),
        };
        step.bytes_read = step.bytes_read.saturating_add(bytes);
        if bytes != artifact.bytes {
            step.verdict = TreeScrubVerdictV1::Mismatch(TreeCommitmentMismatchV1::Length {
                name: artifact.name.clone(),
                on_disk: bytes,
                committed: artifact.bytes,
            });
            return Ok(step);
        }
        if sha256 != artifact.sha256 {
            step.verdict = TreeScrubVerdictV1::Mismatch(TreeCommitmentMismatchV1::Digest {
                name: artifact.name.clone(),
            });
            return Ok(step);
        }
        step.files_verified = step.files_verified.saturating_add(1);
    }
    Ok(step)
}

/// A tree commitment together with how it was measured (QI-BB-006).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeCommitmentV1 {
    pub artifacts: Vec<SealedArtifactCommitmentV1>,
    /// Bytes this seal read and hashed itself.
    pub hashed_bytes: u64,
    /// Bytes whose digest was carried over from the base commitment because
    /// the file is the base's own inode.
    pub inherited_bytes: u64,
    /// Files whose digest was carried over.
    pub inherited_files: u64,
}

/// The filesystem identity of a regular file: `(device, inode)`.
///
/// Two paths with the same identity are one file, so a hard link inherited
/// from an immutable base carries the base's bytes exactly.
#[cfg(unix)]
fn file_identity_v1(path: &Path) -> std::io::Result<Option<(u64, u64)>> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Ok(None);
    }
    Ok(Some((metadata.dev(), metadata.ino())))
}

/// No file identity is available on this platform; every file is hashed.
#[cfg(not(unix))]
fn file_identity_v1(_path: &Path) -> std::io::Result<Option<(u64, u64)>> {
    Ok(None)
}

/// Whether `path` is the very file at `base_path`: the same inode on the
/// same device. A base file that is gone is simply not the same file; any
/// other failure to inspect either side is an error.
fn is_same_file_v1(path: &Path, base_path: &Path) -> std::io::Result<bool> {
    let Some(mine) = file_identity_v1(path)? else {
        return Ok(false);
    };
    let theirs = match std::fs::symlink_metadata(base_path) {
        Ok(_) => file_identity_v1(base_path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    Ok(theirs == Some(mine))
}

/// The on-disk path of a committed name (`prefix/rest`) under `root`.
fn committed_path_v1(root: &Path, prefix: &str, name: &str) -> std::io::Result<PathBuf> {
    name.strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('/'))
        .map(|rest| root.join(rest))
        .ok_or_else(|| {
            std::io::Error::other(format!("committed name {name} is not under {prefix}/"))
        })
}

/// Commit every regular file under `root`, carrying over the digest of any
/// file that is the same inode as the file `base` committed under the same
/// name with the same length (QI-BB-006 #4, QI-BB-017).
///
/// A delta generation inherits its base's immutable files by hard link, so
/// a file that is still the base's inode is the bytes the base's seal
/// hashed; only files new to this generation are read. When the base's
/// file has since been replaced or the platform reports no identity, the
/// file is hashed like any other, and the report says so.
pub fn commit_tree_inheriting_v1(
    root: &Path,
    prefix: &str,
    base: Option<(&Path, &[SealedArtifactCommitmentV1])>,
) -> std::io::Result<TreeCommitmentV1> {
    let entries = list_tree_v1(root, prefix)?;
    let base_by_name: BTreeMap<&str, &SealedArtifactCommitmentV1> = base
        .map(|(_, artifacts)| {
            artifacts
                .iter()
                .map(|artifact| (artifact.name.as_str(), artifact))
                .collect()
        })
        .unwrap_or_default();
    let mut commitment = TreeCommitmentV1 {
        artifacts: Vec::with_capacity(entries.len()),
        hashed_bytes: 0,
        inherited_bytes: 0,
        inherited_files: 0,
    };
    for entry in entries {
        let mut inherited = None;
        if let (Some((base_root, _)), Some(committed)) =
            (base, base_by_name.get(entry.name.as_str()))
            && committed.bytes == entry.bytes
        {
            let base_path = committed_path_v1(base_root, prefix, &entry.name)?;
            if is_same_file_v1(&entry.path, &base_path)? {
                inherited = Some(committed.sha256);
            }
        }
        let sha256 = if let Some(sha256) = inherited {
            commitment.inherited_bytes = commitment.inherited_bytes.saturating_add(entry.bytes);
            commitment.inherited_files = commitment.inherited_files.saturating_add(1);
            sha256
        } else {
            let (bytes, sha256) = sha256_of_file(&entry.path)?;
            if bytes != entry.bytes {
                return Err(std::io::Error::other(format!(
                    "{} changed length while being committed ({} then {bytes} bytes)",
                    entry.name, entry.bytes
                )));
            }
            commitment.hashed_bytes = commitment.hashed_bytes.saturating_add(bytes);
            sha256
        };
        commitment.artifacts.push(SealedArtifactCommitmentV1 {
            name: entry.name,
            bytes: entry.bytes,
            sha256,
        });
    }
    Ok(commitment)
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
        TreeCommitmentMismatchV1, TreeScrubStepV1, TreeScrubVerdictV1, commit_tree_inheriting_v1,
        commit_tree_v1, scrub_tree_commitment_v1, sha256_of_file, verify_tree_commitment_v1,
        verify_tree_layout_v1,
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

    /// The layout check sees every shape defect but a same-length rewrite,
    /// which only the scrub finds; the scrub pauses on its byte budget and
    /// resumes from its cursor without re-reading what it verified.
    #[test]
    fn the_layout_check_is_cheap_and_the_scrub_is_bounded_and_resumable() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("dataset");
        write(&root, "a.lance", b"alpha");
        write(&root, "sub/b.manifest", b"bravo!");
        write(&root, "sub/c.idx", b"charlie");
        let committed = commit_tree_v1(&root, "dataset").expect("commit");

        assert_eq!(verify_tree_layout_v1(&root, "dataset", &committed).expect("io"), Ok(18));
        // A same-length rewrite passes the layout check by design…
        std::fs::write(root.join("a.lance"), b"alphA").expect("flip");
        assert_eq!(verify_tree_layout_v1(&root, "dataset", &committed).expect("io"), Ok(18));
        // …and is the scrub's to find, whatever the cursor; the bytes it
        // read to find it are reported, not hidden.
        assert_eq!(
            scrub_tree_commitment_v1(&root, "dataset", &committed, 0, u64::MAX).expect("io"),
            TreeScrubStepV1 {
                files_verified: 0,
                bytes_read: 5,
                verdict: TreeScrubVerdictV1::Mismatch(TreeCommitmentMismatchV1::Digest {
                    name: "dataset/a.lance".to_string()
                })
            }
        );
        std::fs::write(root.join("a.lance"), b"alpha").expect("restore");
        // Shape defects are visible to the layout check on every step.
        std::fs::write(root.join("a.lance"), b"alph").expect("truncate");
        assert_eq!(
            verify_tree_layout_v1(&root, "dataset", &committed).expect("io"),
            Err(TreeCommitmentMismatchV1::Length {
                name: "dataset/a.lance".to_string(),
                on_disk: 4,
                committed: 5
            })
        );
        assert_eq!(
            scrub_tree_commitment_v1(&root, "dataset", &committed, 2, u64::MAX).expect("io"),
            TreeScrubStepV1 {
                files_verified: 0,
                bytes_read: 0,
                verdict: TreeScrubVerdictV1::Mismatch(TreeCommitmentMismatchV1::Length {
                    name: "dataset/a.lance".to_string(),
                    on_disk: 4,
                    committed: 5
                })
            }
        );
        std::fs::write(root.join("a.lance"), b"alpha").expect("restore");

        // A budget of one byte still hashes one file per step, then pauses.
        let first = scrub_tree_commitment_v1(&root, "dataset", &committed, 0, 1).expect("io");
        assert_eq!(
            first,
            TreeScrubStepV1 {
                files_verified: 1,
                bytes_read: 5,
                verdict: TreeScrubVerdictV1::Paused { next_artifact: 1 }
            }
        );
        let second = scrub_tree_commitment_v1(&root, "dataset", &committed, 1, 6).expect("io");
        assert_eq!(
            second,
            TreeScrubStepV1 {
                files_verified: 1,
                bytes_read: 6,
                verdict: TreeScrubVerdictV1::Paused { next_artifact: 2 }
            }
        );
        let third = scrub_tree_commitment_v1(&root, "dataset", &committed, 2, 6).expect("io");
        assert_eq!(
            third,
            TreeScrubStepV1 {
                files_verified: 1,
                bytes_read: 7,
                verdict: TreeScrubVerdictV1::Completed
            }
        );
        // One unbounded step reads exactly the committed bytes, once.
        let whole =
            scrub_tree_commitment_v1(&root, "dataset", &committed, 0, u64::MAX).expect("io");
        assert_eq!(
            whole,
            TreeScrubStepV1 {
                files_verified: 3,
                bytes_read: 18,
                verdict: TreeScrubVerdictV1::Completed
            }
        );
        // A cursor beyond the list is a caller defect, not a completed scrub.
        assert!(scrub_tree_commitment_v1(&root, "dataset", &committed, 4, u64::MAX).is_err());
    }

    /// A delta tree inherits the digest of every file that is still the
    /// base's inode and hashes only what is new or replaced.
    #[test]
    fn a_delta_commitment_hashes_only_the_files_that_are_not_the_base_inode() {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path().join("g1").join("dataset");
        write(&base, "data/1.lance", b"alpha");
        write(&base, "_versions/1.manifest", b"bravo!");
        let base_commitment = commit_tree_v1(&base, "dataset").expect("commit base");

        let delta = temp.path().join("g2").join("dataset");
        std::fs::create_dir_all(delta.join("data")).expect("mkdir");
        std::fs::create_dir_all(delta.join("_versions")).expect("mkdir");
        std::fs::hard_link(base.join("data/1.lance"), delta.join("data/1.lance")).expect("link");
        // Same name and length as the base's, but a copy, not its inode.
        std::fs::write(delta.join("_versions/1.manifest"), b"bravo!").expect("copy");
        write(&delta, "data/2.lance", b"charlie");

        let commitment =
            commit_tree_inheriting_v1(&delta, "dataset", Some((&base, &base_commitment)))
                .expect("commit delta");
        assert_eq!(commitment.inherited_files, 1);
        assert_eq!(commitment.inherited_bytes, 5);
        assert_eq!(commitment.hashed_bytes, 6 + 7);
        // The inherited digest is the base's, and every digest is the truth:
        // the same tree committed from scratch agrees byte for byte.
        assert_eq!(
            commitment.artifacts,
            commit_tree_v1(&delta, "dataset").expect("commit from scratch")
        );
        assert_eq!(
            commitment
                .artifacts
                .iter()
                .find(|artifact| artifact.name == "dataset/data/1.lance")
                .map(|artifact| artifact.sha256),
            base_commitment
                .iter()
                .find(|artifact| artifact.name == "dataset/data/1.lance")
                .map(|artifact| artifact.sha256)
        );
        // Without a base, everything is hashed and nothing is inherited.
        let fresh = commit_tree_inheriting_v1(&delta, "dataset", None).expect("commit fresh");
        assert_eq!(fresh.inherited_files, 0);
        assert_eq!(fresh.hashed_bytes, 18);
        assert_eq!(fresh.artifacts, commitment.artifacts);
        // A base file that vanished since is not inherited either.
        std::fs::remove_file(base.join("data/1.lance")).expect("remove base file");
        let orphaned =
            commit_tree_inheriting_v1(&delta, "dataset", Some((&base, &base_commitment)))
                .expect("commit with a vanished base file");
        assert_eq!(orphaned.inherited_files, 0);
        assert_eq!(orphaned.artifacts, commitment.artifacts);
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
