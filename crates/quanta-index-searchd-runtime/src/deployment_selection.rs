//! Canonical selection-file transaction for resident Native recovery.
//!
//! The caller closes actual producer admission before entering this scope,
//! then performs Q verification inside the callback. This mechanical publisher
//! issues no producer, Catalog, replay, or rollback authority.

mod codec;
mod completion_witness;
#[cfg(test)]
mod tests;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use anyhow::Context as _;
use anyhow::{Result, anyhow, ensure};
use quanta_index_contract::{
    RepoId, RepoMapMutationAck, RepoMapTerminalReceiptV2, RevisionId, SearchCorpusActiveHeadV1,
    index::Digest,
    native_cutover_evidence_v1::{
        NativeCutoverControlBeginV1, NativeCutoverControlTerminalV1,
        NativeCutoverEvidenceBeforeSelectionV1, NativeCutoverEvidenceLegacyPublicationV1,
        NativeCutoverEvidenceLegacyRepoMapV1, NativeCutoverInstalledHistoryV1,
        NativeCutoverRecoveryModeV1,
    },
};
use quanta_index_core::index::{MemoryLease, MemoryLifetime, WorkBudget, WorkUsage};
use quanta_index_core::publication_capacity::InodeKey;
use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use sha2::{Digest as _, Sha256};
use std::{
    ffi::OsStr,
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub(crate) use codec::DeploymentSelectionDocumentV1;
const MAX_BYTES: usize = 64 * 1024;
const LOCK_NAME: &str = ".quanta-index-deployment.lock";
#[cfg(target_os = "linux")]
const PUBLICATION_ATTRIBUTE: &str = "user.quanta-index.recovery-publication";
#[cfg(target_os = "macos")]
const PUBLICATION_ATTRIBUTE: &str = "com.quanta-index.recovery-publication";
// Three bounded JSON trees, normalized payloads, config trees and stream buffer.
// Admission precedes opening/decoding; its backing remains until all data drops.
const MEMORY_BYTES: u64 = 3 * (MAX_BYTES as u64) * 64 + 128 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DeploymentRecoveryModeV1 {
    RollbackLegacy,
    RestoreForward,
}

/// The journal's original bytes are data, never publication authority.
pub(crate) struct DeploymentSelectionBeforeDocumentV1<'a> {
    pub operation_id_v1: &'a str,
    pub bytes_v1: &'a [u8],
    pub raw_sha256_v1: Digest,
    pub file_version_v1: Option<Digest>,
    /// A previous response only restricts this attempt to reconciliation.
    /// It never authorizes publication or attributes the current state.
    pub retained_terminal_v1: Option<&'a NativeCutoverControlTerminalV1>,
}

pub(crate) struct DeploymentSelectionRequestV1<'a> {
    pub selection_path_v1: &'a Path,
    pub expected_current_sha256_v1: Digest,
    pub candidate_path_v1: &'a Path,
    /// Fingerprint of the canonical schema-2 payload, including its newline.
    pub expected_candidate_sha256_v1: Digest,
    pub mode_v1: DeploymentRecoveryModeV1,
}

#[derive(Debug)]
pub(crate) enum DeploymentSelectionErrorV1 {
    RefusedBeforeRename(anyhow::Error),
    PersistenceUncertain(anyhow::Error),
}
impl std::fmt::Display for DeploymentSelectionErrorV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RefusedBeforeRename(e) => write!(f, "selection was not changed: {e}"),
            Self::PersistenceUncertain(e) => write!(
                f,
                "selection rename may have occurred; inspect durable selection: {e}"
            ),
        }
    }
}
impl std::error::Error for DeploymentSelectionErrorV1 {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::RefusedBeforeRename(e) | Self::PersistenceUncertain(e) => Some(e.as_ref()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DeploymentSelectionResolutionV1 {
    Published,
    Reconciled,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct DeploymentSelectionPublishedV1 {
    resolution_v1: DeploymentSelectionResolutionV1,
    before_sha256_v1: Digest,
    after_sha256_v1: Digest,
}
impl DeploymentSelectionPublishedV1 {
    pub(crate) fn resolution_v1(&self) -> DeploymentSelectionResolutionV1 {
        self.resolution_v1
    }
    pub(crate) fn before_sha256_v1(&self) -> Digest {
        self.before_sha256_v1
    }
    pub(crate) fn after_sha256_v1(&self) -> Digest {
        self.after_sha256_v1
    }
}

/// Non-Clone deployment custody. No detached approval/permission token exists.
/// Keep this owner outside the Q verification callback, through its return.
pub(crate) struct LockedDeploymentSelectionV1 {
    current: DeploymentSelectionDocumentV1,
    observed: Option<DeploymentSelectionDocumentV1>,
    active_file_version: Digest,
    recovery_publication: Option<Digest>,
    witness_binding: Option<RecoveryWitnessBindingV1>,
    recovery_witness: Option<RecoveryWitnessV1>,
    candidate: DeploymentSelectionDocumentV1,
    path: PathBuf,
    candidate_path: PathBuf,
    directory: File,
    lock: File,
    phase: PublicationPhase,
    _memory: MemoryLease,
}

struct RecoveryWitnessFieldSeed(&'static [&'static str]);

impl<'de> serde::de::DeserializeSeed<'de> for RecoveryWitnessFieldSeed {
    type Value = &'static str;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_identifier(self)
    }
}

impl serde::de::Visitor<'_> for RecoveryWitnessFieldSeed {
    type Value = &'static str;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("field identifier")
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        self.0
            .iter()
            .copied()
            .find(|field| *field == value)
            .ok_or_else(|| E::unknown_field(value, self.0))
    }

    fn visit_bytes<E: serde::de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
        self.0
            .iter()
            .copied()
            .find(|field| field.as_bytes() == value)
            .ok_or_else(|| E::unknown_field(&String::from_utf8_lossy(value), self.0))
    }
}

// These records accept omitted nullable map fields but require every position in
// sequence readers. Keep the declaration order for durable witness bytes.
macro_rules! recovery_witness_codec {
    ($name:ident $(<$($param:ident),+>)?, $shape:literal, {
        $($field:ident : $ty:ty $(= $map_default:ident)?),+ $(,)?
    }) => {
        impl $(<$($param: serde::Serialize),+>)? serde::Serialize for $name $(<$($param),+>)? {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                const FIELDS: &[&str] = &[$(stringify!($field)),+];
                let mut record = serializer.serialize_struct(stringify!($name), FIELDS.len())?;
                $(serde::ser::SerializeStruct::serialize_field(&mut record, stringify!($field), &self.$field)?;)+
                serde::ser::SerializeStruct::end(record)
            }
        }

        impl<'de $(, $($param: serde::Deserialize<'de>),+)?> serde::Deserialize<'de>
            for $name $(<$($param),+>)?
        {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                const FIELDS: &[&str] = &[$(stringify!($field)),+];
                struct WireVisitor<T>(std::marker::PhantomData<fn() -> T>);

                impl<'de $(, $($param: serde::Deserialize<'de>),+)?> serde::de::Visitor<'de>
                    for WireVisitor<$name $(<$($param),+>)?>
                {
                    type Value = $name $(<$($param),+>)?;

                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        formatter.write_str(concat!("struct ", stringify!($name)))
                    }

                    fn visit_map<A: serde::de::MapAccess<'de>>(
                        self,
                        mut map: A,
                    ) -> Result<Self::Value, A::Error> {
                        $(let mut $field: Option<$ty> = None;)+
                        while let Some(key) = map.next_key_seed(
                            RecoveryWitnessFieldSeed(FIELDS)
                        )? {
                            match key {
                                $(stringify!($field) => {
                                    if $field.is_some() {
                                        return Err(serde::de::Error::duplicate_field(stringify!($field)));
                                    }
                                    $field = Some(map.next_value()?);
                                },)+
                                _ => unreachable!("closed field seed only returns witness keys"),
                            }
                        }
                        Ok($name {
                            $($field: recovery_witness_codec!(@map_value $field $(= $map_default)?),)+
                        })
                    }

                    fn visit_seq<A: serde::de::SeqAccess<'de>>(
                        self,
                        mut seq: A,
                    ) -> Result<Self::Value, A::Error> {
                        let mut indices = 0..FIELDS.len();
                        $(let $field = {
                            let index = indices.next().unwrap_or(FIELDS.len());
                            seq.next_element()?.ok_or_else(|| {
                                serde::de::Error::invalid_length(
                                    index, &concat!("struct ", stringify!($name), " with ", $shape, " elements"),
                                )
                            })?
                        };)+
                        Ok($name { $($field,)+ })
                    }
                }

                deserializer.deserialize_struct(
                    stringify!($name),
                    FIELDS,
                    WireVisitor::<Self>(std::marker::PhantomData),
                )
            }
        }
    };
    (@map_value $field:ident) => {
        $field.ok_or_else(|| serde::de::Error::missing_field(stringify!($field)))?
    };
    (@map_value $field:ident = default) => {
        $field.unwrap_or_default()
    };
}

struct RecoveryWitnessBindingV1 {
    mode: NativeCutoverRecoveryModeV1,
    nonce: Digest,
    config_sha256: Digest,
    helper_sha256: Digest,
    evidence_length: u64,
    evidence_sha256: Digest,
}

recovery_witness_codec!(RecoveryWitnessBindingV1, "6", {
    mode: NativeCutoverRecoveryModeV1,
    nonce: Digest,
    config_sha256: Digest,
    helper_sha256: Digest,
    evidence_length: u64,
    evidence_sha256: Digest,
});

struct RecoveryWitnessWireV1<B, H, L, R> {
    format_version: u8,
    binding: B,
    root_device: u64,
    root_inode: u64,
    history: Option<H>,
    legacy: Option<L>,
    repomap: Option<R>,
}

recovery_witness_codec!(RecoveryWitnessWireV1<B, H, L, R>, "7", {
    format_version: u8,
    binding: B,
    root_device: u64,
    root_inode: u64,
    history: Option<H> = default,
    legacy: Option<L> = default,
    repomap: Option<R> = default,
});

pub(crate) struct LegacyPublishedRecoveryWitnessV1<R = RepoId, V = RevisionId> {
    pub(crate) repo_id: R,
    pub(crate) revision_id: V,
    pub(crate) durable_sequence: u64,
    pub(crate) activation: Digest,
}

recovery_witness_codec!(LegacyPublishedRecoveryWitnessV1<R, V>, "4", {
    repo_id: R,
    revision_id: V,
    durable_sequence: u64,
    activation: Digest,
});

impl<'a> LegacyPublishedRecoveryWitnessV1<&'a RepoId, &'a RevisionId> {
    pub(crate) fn from_publication_v1(
        publication: &'a NativeCutoverEvidenceLegacyPublicationV1,
        budget: &mut WorkBudget,
    ) -> Result<Self> {
        let lexical = &publication.activation.active.generation.lexical;
        Ok(Self {
            repo_id: &lexical.repo_id,
            revision_id: &lexical.revision_id,
            durable_sequence: publication.receipt.durable_sequence,
            activation: legacy_recovery_activation_digest_v1(
                &publication.activation.active,
                budget,
            )?,
        })
    }
}

pub(crate) fn legacy_recovery_activation_digest_v1(
    head: &SearchCorpusActiveHeadV1,
    budget: &mut WorkBudget,
) -> Result<Digest> {
    let mut bytes = 4096_u64;
    for track in [&head.generation.lexical, &head.generation.semantic] {
        for identity in [track.repo_id.as_str(), track.revision_id.as_str()] {
            bytes = bytes
                .checked_add(u64::try_from(identity.len())?)
                .ok_or_else(|| anyhow!("Legacy activation recovery witness exceeds its budget"))?;
        }
    }
    charge(budget, 0, 0, bytes, 0)?;
    let mut digest = Sha256::new();
    digest.update(b"quanta.native-cutover.legacy-activation.v1\0");
    ciborium::ser::into_writer(head, &mut digest)?;
    Ok(Digest(digest.finalize().into()))
}

/// A replay reports the same durable RepoMap receipt with a different reply flag.
pub(crate) fn same_durable_repomap_v1(
    left: &NativeCutoverEvidenceLegacyRepoMapV1,
    right: &NativeCutoverEvidenceLegacyRepoMapV1,
) -> bool {
    use NativeCutoverEvidenceLegacyRepoMapV1::Required;
    match (left, right) {
        (
            Required {
                state_root,
                locator_digest,
                publish,
                activation,
            },
            Required {
                state_root: other_root,
                locator_digest: other_locator,
                publish: other_publish,
                activation: other_activation,
            },
        ) => {
            state_root == other_root
                && locator_digest == other_locator
                && same_durable_repomap_receipt_v1(publish, other_publish)
                && same_durable_repomap_receipt_v1(activation, other_activation)
        }
        _ => left == right,
    }
}

fn same_durable_repomap_receipt_v1(
    left: &RepoMapTerminalReceiptV2,
    right: &RepoMapTerminalReceiptV2,
) -> bool {
    let RepoMapTerminalReceiptV2 {
        phase,
        mutation,
        manifest_digest,
        snapshot_id,
        projection_version,
        authority_digest,
        source_bundle_digest,
    } = left;
    let RepoMapMutationAck {
        repo_id,
        revision_id,
        manifest_generation,
        prior_candidate_commitment,
        new_candidate_commitment,
        activation_epoch,
        terminal_sequence,
        replayed: _,
    } = mutation;
    *phase == right.phase
        && repo_id == &right.mutation.repo_id
        && revision_id == &right.mutation.revision_id
        && *manifest_generation == right.mutation.manifest_generation
        && prior_candidate_commitment == &right.mutation.prior_candidate_commitment
        && new_candidate_commitment == &right.mutation.new_candidate_commitment
        && *activation_epoch == right.mutation.activation_epoch
        && *terminal_sequence == right.mutation.terminal_sequence
        && manifest_digest == &right.manifest_digest
        && snapshot_id == &right.snapshot_id
        && *projection_version == right.projection_version
        && authority_digest == &right.authority_digest
        && source_bundle_digest == &right.source_bundle_digest
}

enum RecoveryWitnessV1 {
    Prepared {
        bytes: Vec<u8>,
        _memory: MemoryLease,
    },
    Selected(Digest),
}

type OwnedRecoveryWitnessV1 = RecoveryWitnessWireV1<
    RecoveryWitnessBindingV1,
    NativeCutoverInstalledHistoryV1,
    LegacyPublishedRecoveryWitnessV1,
    NativeCutoverEvidenceLegacyRepoMapV1,
>;

pub(crate) struct PublishedRecoveryRootWitnessV1<'a> {
    pub(crate) root: InodeKey,
    pub(crate) history: Option<&'a NativeCutoverInstalledHistoryV1>,
    pub(crate) legacy: Option<&'a LegacyPublishedRecoveryWitnessV1>,
    pub(crate) repomap: &'a NativeCutoverEvidenceLegacyRepoMapV1,
}

pub(crate) struct SettledRecoveryWitnessV1 {
    pub(crate) publication: DeploymentSelectionPublishedV1,
    pub(crate) history: Option<NativeCutoverInstalledHistoryV1>,
    pub(crate) memory: MemoryLease,
}

pub(crate) type SyncPublishedRecoveryV1<'a> = dyn FnMut(
        &mut WorkBudget,
        &mut dyn FnMut(&mut WorkBudget) -> Result<()>,
    ) -> Result<DeploymentSelectionPublishedV1>
    + 'a;

/// Read the exact selected deployment while holding its existing lock.
pub(crate) fn with_locked_export_selection_v1<T>(
    path: &Path,
    expected: Digest,
    budget: &mut WorkBudget,
    read: impl FnOnce(&DeploymentSelectionDocumentV1, &mut WorkBudget) -> Result<T>,
) -> Result<T> {
    let _memory = budget
        .reserve_memory(MEMORY_BYTES, MemoryLifetime::Temporary)?
        .ok_or_else(|| anyhow!("History export requires admitted Control memory"))?;
    let parent = selection_parent(path, budget)?;
    charge(budget, 1, 0, 0, 0)?;
    let directory = File::from(rustix::fs::open(
        &parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    charge(budget, 1, 0, 0, 0)?;
    let lock = File::from(rustix::fs::openat(
        &directory,
        LOCK_NAME,
        OFlags::RDWR | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    control_metadata(&lock.metadata()?)?;
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .map_err(|error| anyhow!("cannot acquire deployment lock: {error}"))?;
    let (bytes, version, _file) = read_active_control(&directory, path, budget)?;
    let selected = codec::decode(&bytes, budget)?;
    ensure!(
        selected.fingerprint_v1() == expected,
        "active selection fingerprint changed"
    );
    let result = read(&selected, budget)?;
    let (after_bytes, after_version, _after_file) = read_active_control(&directory, path, budget)?;
    ensure!(
        after_version == version && after_bytes == bytes,
        "active selection changed during History export"
    );
    Ok(result)
}

/// Ordering: actual producer closure -> this deployment lock -> actual Q root
/// verification -> publication/file+directory fsync -> Q retirement -> unlock.
/// A successful arbitrary callback does not authorize a transition; the resident
/// coordinator must join canonical source/ACK evidence and real Q verification.
#[cfg(test)]
pub(crate) fn with_locked_deployment_selection_v1(
    request: &DeploymentSelectionRequestV1<'_>,
    budget: &mut WorkBudget,
    coordinate: impl FnOnce(
        &mut LockedDeploymentSelectionV1,
        &mut WorkBudget,
    ) -> std::result::Result<
        DeploymentSelectionPublishedV1,
        DeploymentSelectionErrorV1,
    >,
) -> std::result::Result<DeploymentSelectionPublishedV1, DeploymentSelectionErrorV1> {
    with_locked_selection_inner(request, None, budget, coordinate)
}

/// Reuses the canonical publisher and codec while preserving the journal's
/// first before document. Reconciliation requires the original publication
/// attribute, full domain validation, file+directory sync and post-verification.
pub(crate) fn with_locked_deployment_selection_reconciliation_v1(
    request: &DeploymentSelectionRequestV1<'_>,
    before: &DeploymentSelectionBeforeDocumentV1<'_>,
    budget: &mut WorkBudget,
    coordinate: impl FnOnce(
        &mut LockedDeploymentSelectionV1,
        &mut WorkBudget,
    ) -> std::result::Result<
        DeploymentSelectionPublishedV1,
        DeploymentSelectionErrorV1,
    >,
) -> std::result::Result<DeploymentSelectionPublishedV1, DeploymentSelectionErrorV1> {
    with_locked_selection_inner(request, Some(before), budget, coordinate)
}

/// Reopen an already-selected recovery under the standard deployment lock.
/// The callback can only re-fsync the observed inode while holding verified
/// root custody. There is no capability to create a selection or replace it.
pub(crate) fn with_locked_published_recovery_v1(
    request: &DeploymentSelectionRequestV1<'_>,
    before: &DeploymentSelectionBeforeDocumentV1<'_>,
    config_sha256: Digest,
    budget: &mut WorkBudget,
    verify: impl FnOnce(
        &LockedDeploymentSelectionV1,
        Digest,
        PublishedRecoveryRootWitnessV1<'_>,
        &mut WorkBudget,
        &mut SyncPublishedRecoveryV1<'_>,
    ) -> Result<DeploymentSelectionPublishedV1>,
) -> std::result::Result<SettledRecoveryWitnessV1, DeploymentSelectionErrorV1> {
    let mut phase = PublicationPhase::ReconciliationPending;
    let locked =
        LockedDeploymentSelectionV1::acquire(request, Some(before), budget, &mut phase, true)
            .map_err(DeploymentSelectionErrorV1::PersistenceUncertain)?;
    let mode = match request.mode_v1 {
        DeploymentRecoveryModeV1::RollbackLegacy => NativeCutoverRecoveryModeV1::RollbackLegacy,
        DeploymentRecoveryModeV1::RestoreForward => NativeCutoverRecoveryModeV1::RestoreForward,
    };
    let helper_sha256 = match mode {
        NativeCutoverRecoveryModeV1::RollbackLegacy => locked.current.binary_sha256_v1(),
        NativeCutoverRecoveryModeV1::RestoreForward => locked.candidate.binary_sha256_v1(),
    };
    locked
        .with_observed_witness_v1(budget, |wire, _digest, memory, budget| {
            ensure!(
                wire.binding.mode == mode
                    && wire.binding.config_sha256 == config_sha256
                    && wire.binding.helper_sha256 == helper_sha256,
                "published recovery witness differs from the requested mode or installed image"
            );
            let repomap = wire
                .repomap
                .as_ref()
                .ok_or_else(|| anyhow!("published recovery has no retained RepoMap disposition"))?;
            let mut synced = false;
            let publication = verify(
                &locked,
                helper_sha256,
                PublishedRecoveryRootWitnessV1 {
                    root: InodeKey {
                        device: wire.root_device,
                        inode: wire.root_inode,
                    },
                    history: wire.history.as_ref(),
                    legacy: wire.legacy.as_ref(),
                    repomap,
                },
                budget,
                &mut |budget, checkpoint| {
                    ensure!(!synced, "published recovery was already synced");
                    let result =
                        locked.sync_observed_recovery_v1(budget, checkpoint, &mut |_| Ok(()))?;
                    synced = true;
                    Ok(result)
                },
            )?;
            ensure!(
                synced
                    && publication.resolution_v1 == DeploymentSelectionResolutionV1::Reconciled
                    && publication.before_sha256_v1 == request.expected_current_sha256_v1
                    && publication.after_sha256_v1 == request.expected_candidate_sha256_v1,
                "published recovery did not sync the exact selection"
            );
            Ok(SettledRecoveryWitnessV1 {
                publication,
                history: wire.history,
                memory,
            })
        })
        .and_then(|settled| settled.ok_or_else(|| anyhow!("selected recovery witness is absent")))
        .map_err(DeploymentSelectionErrorV1::PersistenceUncertain)
}

fn with_locked_selection_inner(
    request: &DeploymentSelectionRequestV1<'_>,
    before: Option<&DeploymentSelectionBeforeDocumentV1<'_>>,
    budget: &mut WorkBudget,
    coordinate: impl FnOnce(
        &mut LockedDeploymentSelectionV1,
        &mut WorkBudget,
    ) -> std::result::Result<
        DeploymentSelectionPublishedV1,
        DeploymentSelectionErrorV1,
    >,
) -> std::result::Result<DeploymentSelectionPublishedV1, DeploymentSelectionErrorV1> {
    // A journal-backed attempt may already have published. Even a failure
    // before opening the active control cannot establish its outcome.
    let mut phase = if before.is_some() {
        PublicationPhase::ReconciliationPending
    } else {
        PublicationPhase::BeforeRename
    };
    let mut locked =
        LockedDeploymentSelectionV1::acquire(request, before, budget, &mut phase, false)
            .map_err(|error| phase.classify(error))?;
    match coordinate(&mut locked, budget) {
        Ok(_)
            if matches!(
                locked.phase,
                PublicationPhase::RenameAttempted | PublicationPhase::ReconciliationPending
            ) =>
        {
            Err(DeploymentSelectionErrorV1::PersistenceUncertain(anyhow!(
                "selection publication or reconciliation has incomplete durability"
            )))
        }
        Ok(_) if locked.phase == PublicationPhase::BeforeRename => {
            Err(DeploymentSelectionErrorV1::RefusedBeforeRename(anyhow!(
                "selection publisher did not run"
            )))
        }
        Ok(value) => {
            let resolution = if locked.observed.is_some() {
                DeploymentSelectionResolutionV1::Reconciled
            } else {
                DeploymentSelectionResolutionV1::Published
            };
            if locked.phase != PublicationPhase::Durable
                || value.resolution_v1 != resolution
                || value.before_sha256_v1 != locked.current.fingerprint_v1()
                || value.after_sha256_v1 != locked.candidate.fingerprint_v1()
            {
                return Err(DeploymentSelectionErrorV1::PersistenceUncertain(anyhow!(
                    "selection publisher returned a foreign or incomplete outcome"
                )));
            }
            budget
                .checkpoint()
                .map(|()| value)
                .map_err(|error| locked.classify(error.into()))
        }
        Err(error) => Err(locked.normalize(error)),
    }
}

impl LockedDeploymentSelectionV1 {
    pub(crate) fn current_v1(&self) -> &DeploymentSelectionDocumentV1 {
        &self.current
    }
    pub(crate) fn candidate_v1(&self) -> &DeploymentSelectionDocumentV1 {
        &self.candidate
    }

    pub(crate) fn bind_recovery_witness_v1(
        &mut self,
        begin: &NativeCutoverControlBeginV1,
        config_sha256: Digest,
        helper_sha256: Digest,
    ) -> Result<()> {
        ensure!(
            self.witness_binding.is_none(),
            "recovery witness was already bound"
        );
        ensure!(
            self.recovery_publication.is_some(),
            "recovery witness requires an exact intent"
        );
        self.witness_binding = Some(RecoveryWitnessBindingV1 {
            mode: begin.mode,
            nonce: begin.nonce,
            config_sha256,
            helper_sha256,
            evidence_length: begin.evidence_length,
            evidence_sha256: begin.evidence_sha256,
        });
        Ok(())
    }

    /// The root verifier supplies the exact installed outcome while still
    /// holding physical custody. It is provisional until the selection CAS.
    pub(crate) fn prepare_recovery_witness_v1(
        &mut self,
        history: Option<&NativeCutoverInstalledHistoryV1>,
        root: InodeKey,
        legacy: Option<LegacyPublishedRecoveryWitnessV1<&RepoId, &RevisionId>>,
        repomap: &NativeCutoverEvidenceLegacyRepoMapV1,
        budget: &mut WorkBudget,
    ) -> Result<()> {
        let Some(binding) = self.witness_binding.as_ref() else {
            return Ok(());
        };
        ensure!(
            match binding.mode {
                NativeCutoverRecoveryModeV1::RollbackLegacy =>
                    legacy.is_some() && history.is_none(),
                NativeCutoverRecoveryModeV1::RestoreForward =>
                    legacy.is_some() == self.current.native_command_v1().is_none(),
            },
            "recovery physical witness differs from its mode"
        );
        if self.phase == PublicationPhase::ReconciliationPending {
            let observed =
                self.with_observed_witness_v1(budget, |wire, digest, _memory, _budget| {
                    let same_legacy = match (wire.legacy.as_ref(), legacy.as_ref()) {
                        (Some(retained), Some(actual)) => {
                            &retained.repo_id == actual.repo_id
                                && &retained.revision_id == actual.revision_id
                                && retained.durable_sequence == actual.durable_sequence
                                && retained.activation == actual.activation
                        }
                        (None, None) => true,
                        (None, Some(_)) => {
                            wire.repomap.is_none()
                                && binding.mode == NativeCutoverRecoveryModeV1::RestoreForward
                        }
                        _ => false,
                    };
                    // The fresh Original may supply new FD6 bytes and a new nonce.
                    ensure!(
                        wire.binding.mode == binding.mode
                            && wire.binding.config_sha256 == binding.config_sha256
                            && wire.binding.helper_sha256 == binding.helper_sha256
                            && wire.root_device == root.device
                            && wire.root_inode == root.inode
                            && wire.history.as_ref() == history
                            && wire.repomap.as_ref().is_none_or(|retained| {
                                same_durable_repomap_v1(retained, repomap)
                            })
                            && same_legacy,
                        "recovery witness differs from the verified outcome"
                    );
                    Ok(digest)
                })?;
            let digest = observed.ok_or_else(|| anyhow!("selected recovery witness is absent"))?;
            self.recovery_witness = Some(RecoveryWitnessV1::Selected(digest));
            return Ok(());
        }
        ensure!(
            self.phase == PublicationPhase::BeforeRename && self.recovery_witness.is_none(),
            "recovery witness requires an unused selection transaction"
        );
        if let Some(history) = history {
            history.validate_v1()?;
        }
        let maximum = completion_witness::MAX_WITNESS_BYTES;
        let maximum_charged = u64::try_from(maximum)?;
        let memory = budget
            .reserve_memory(maximum_charged, MemoryLifetime::Temporary)?
            .ok_or_else(|| anyhow!("recovery witness requires admitted Control memory"))?;
        charge(budget, 0, 0, maximum_charged, 0)?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(maximum)?;
        bytes.resize(maximum, 0);
        let mut output = std::io::Cursor::new(bytes.as_mut_slice());
        ciborium::ser::into_writer(
            &RecoveryWitnessWireV1 {
                format_version: 1,
                binding,
                root_device: root.device,
                root_inode: root.inode,
                history,
                legacy,
                repomap: Some(repomap),
            },
            &mut output,
        )?;
        let used = usize::try_from(output.position())?;
        bytes.truncate(used);
        self.recovery_witness = Some(RecoveryWitnessV1::Prepared {
            bytes,
            _memory: memory,
        });
        Ok(())
    }

    fn with_selected_recovery_witness_v1<T>(
        &self,
        budget: &mut WorkBudget,
        read: impl FnOnce(&[u8], Digest, &mut WorkBudget) -> Result<T>,
    ) -> Result<Option<T>> {
        ensure!(
            self.phase == PublicationPhase::ReconciliationPending,
            "recovery witness requires an already selected publication"
        );
        self.recheck_custody(budget)?;
        let (_, version, selected) = read_active_control(&self.directory, &self.path, budget)?;
        ensure!(
            version == self.active_file_version,
            "recovery witness selection changed"
        );
        let publication = self
            .recovery_publication
            .ok_or_else(|| anyhow!("reconciliation has no publication binding"))?;
        require_recovery_publication(&selected, publication, budget)?;
        let name = self
            .path
            .file_name()
            .ok_or_else(|| anyhow!("selection basename missing"))?;
        completion_witness::with_read(&self.directory, name, &selected, budget, read)
    }

    fn with_observed_witness_v1<T>(
        &self,
        budget: &mut WorkBudget,
        read: impl FnOnce(OwnedRecoveryWitnessV1, Digest, MemoryLease, &mut WorkBudget) -> Result<T>,
    ) -> Result<Option<T>> {
        self.with_selected_recovery_witness_v1(budget, |bytes, digest, budget| {
            let bytes_len = u64::try_from(bytes.len())?;
            let memory = budget
                .reserve_memory(64 * 1024 + 4 * bytes_len, MemoryLifetime::Temporary)?
                .ok_or_else(|| {
                    anyhow!("recovery witness decode requires admitted Control memory")
                })?;
            charge(budget, 0, 0, bytes_len, 0)?;
            let mut remaining = bytes;
            let wire: OwnedRecoveryWitnessV1 =
                ciborium::de::from_reader_with_recursion_limit(&mut remaining, 32)?;
            ensure!(
                    remaining.is_empty()
                        && wire.format_version == 1
                        && wire.root_inode > 0
                        && wire.binding.evidence_length > 0
                        && wire.binding.evidence_length
                            <= quanta_index_contract::native_cutover_evidence_v1::MAX_NATIVE_CUTOVER_EVIDENCE_BYTES_V1 as u64,
                    "invalid published recovery witness"
                );
            if let Some(history) = wire.history.as_ref() {
                history.validate_v1()?;
            }
            ensure!(
                match wire.binding.mode {
                    NativeCutoverRecoveryModeV1::RollbackLegacy =>
                        wire.history.is_none()
                            && wire
                                .legacy
                                .as_ref()
                                .is_some_and(|legacy| legacy.durable_sequence > 0),
                    NativeCutoverRecoveryModeV1::RestoreForward => wire.legacy.as_ref()
                        .is_none_or(|legacy| legacy.durable_sequence > 0),
                },
                "published recovery witness has the wrong physical scope"
            );
            read(wire, digest, memory, budget)
        })
    }

    pub(crate) fn read_legacy_repomap_selection_v1(
        &self,
        path: &Path,
        expected: Digest,
        root: &Path,
        budget: &mut WorkBudget,
    ) -> Result<DeploymentSelectionDocumentV1> {
        self.recheck_custody(budget)?;
        let parent = selection_parent(path, budget)?;
        ensure!(
            Some(parent.as_path()) == self.path.parent()
                && path != self.path
                && path != self.candidate_path,
            "saved RepoMap selection must be a distinct control in the locked deployment directory"
        );
        let saved = codec::read_document(path, budget)?;
        ensure!(
            saved.fingerprint_v1() == expected
                && saved.native_command_v1().is_none()
                && saved.state_root_v1() == root,
            "saved RepoMap selection differs from its configured binding or producer root"
        );
        Ok(saved)
    }

    pub(crate) fn require_legacy_repomap_selection_v1(
        path: &Path,
        expected: Digest,
        budget: &mut WorkBudget,
    ) -> Result<()> {
        let saved = codec::read_document(path, budget)?;
        ensure!(
            saved.fingerprint_v1() == expected,
            "saved RepoMap selection changed during publication"
        );
        Ok(())
    }

    // An observed candidate may have been opened by a restarted service.
    // Its logical domain checks still run; its installation-time file bytes
    // no longer describe the same kind of observation as an uninstalled root.
    pub(crate) fn requires_candidate_install_attestation_v1(&self) -> bool {
        self.phase == PublicationPhase::BeforeRename
    }

    fn acquire(
        request: &DeploymentSelectionRequestV1<'_>,
        before: Option<&DeploymentSelectionBeforeDocumentV1<'_>>,
        budget: &mut WorkBudget,
        phase: &mut PublicationPhase,
        published_only: bool,
    ) -> Result<Self> {
        let memory = budget
            .reserve_memory(MEMORY_BYTES, MemoryLifetime::Temporary)?
            .ok_or_else(|| {
                anyhow!("deployment selection requires an admitted live memory scope")
            })?;
        let parent = selection_parent(request.selection_path_v1, budget)?;
        let candidate_parent = selection_parent(request.candidate_path_v1, budget)?;
        ensure!(
            request.selection_path_v1 != request.candidate_path_v1,
            "candidate equals active selection"
        );
        if request.mode_v1 == DeploymentRecoveryModeV1::RollbackLegacy {
            ensure!(
                parent == candidate_parent,
                "saved Legacy selection must share deployment directory"
            );
        }
        charge(budget, 1, 0, 0, 0)?;
        let directory = File::from(rustix::fs::open(
            &parent,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        let create = if published_only {
            OFlags::empty()
        } else {
            OFlags::CREATE
        };
        charge(
            budget,
            1,
            u64::from(!published_only),
            0,
            u64::from(!published_only),
        )?;
        let lock = File::from(rustix::fs::openat(
            &directory,
            LOCK_NAME,
            create | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?);
        control_metadata(&lock.metadata()?)?;
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)
            .map_err(|e| anyhow!("cannot acquire deployment lock: {e}"))?;
        let (active_bytes, active_file_version, active_file) =
            read_active_control(&directory, request.selection_path_v1, budget)?;
        let (active_wire, active_payload) = codec::canonical_document(&active_bytes)?;
        drop(active_bytes);
        let recovery_publication = before.and_then(|before| {
            before.file_version_v1.map(|version| {
                // The request ID is stable across attempts, even when a later
                // publication has recreated the original selection bytes.
                let mut digest = Sha256::new();
                digest.update(b"quanta.native-cutover.selection-publication.v1\0");
                digest.update(version.0);
                digest.update(request.expected_current_sha256_v1.0);
                digest.update(request.expected_candidate_sha256_v1.0);
                digest.update(before.operation_id_v1.as_bytes());
                Digest(digest.finalize().into())
            })
        });
        let (current, observed) = if let Some(before) = before {
            ensure!(
                before.bytes_v1.len() <= MAX_BYTES,
                "journal before document exceeds byte bound"
            );
            charge(budget, 0, 0, u64::try_from(before.bytes_v1.len())?, 0)?;
            ensure!(
                fingerprint(before.bytes_v1) == before.raw_sha256_v1,
                "journal before raw SHA-256 changed"
            );
            let (original_wire, original_payload) = codec::canonical_document(before.bytes_v1)?;
            ensure!(
                fingerprint(&original_payload) == request.expected_current_sha256_v1,
                "journal before canonical fingerprint differs"
            );
            if active_payload == original_payload {
                ensure!(!published_only, "original selection is still active");
                if before.retained_terminal_v1.is_some_and(|terminal| {
                    matches!(
                        terminal,
                        NativeCutoverControlTerminalV1::Applied { .. }
                            | NativeCutoverControlTerminalV1::Reconciled { .. }
                            | NativeCutoverControlTerminalV1::AppliedHistory { .. }
                            | NativeCutoverControlTerminalV1::ReconciledHistory { .. }
                    )
                }) {
                    *phase = PublicationPhase::BeforeRename;
                    return Err(anyhow!(
                        "completed retained terminal requires the exact current candidate for reconciliation"
                    ));
                }
                // CAS by bytes identifies the selected configuration. Only
                // the first physical file identifies the pre-rename state.
                ensure!(
                    before.file_version_v1 == Some(active_file_version),
                    "current selection is not the retained original file version"
                );
                *phase = PublicationPhase::BeforeRename;
                (
                    codec::decode_document(&active_wire, active_payload, budget)?,
                    None,
                )
            } else {
                ensure!(
                    fingerprint(&active_payload) == request.expected_candidate_sha256_v1,
                    "current selection is neither the original nor exact candidate"
                );
                let expected = recovery_publication.ok_or_else(|| {
                    anyhow!("retained intent has no original selection file version")
                })?;
                require_recovery_publication(&active_file, expected, budget)?;
                // Preserve ReconciliationPending through candidate validation.
                let current = codec::decode_document(&original_wire, original_payload, budget)?;
                let observed = codec::decode_document(&active_wire, active_payload, budget)?;
                (current, Some(observed))
            }
        } else {
            ensure!(
                !published_only,
                "published recovery requires an original intent"
            );
            ensure!(
                fingerprint(&active_payload) == request.expected_current_sha256_v1,
                "current selection changed"
            );
            (
                codec::decode_document(&active_wire, active_payload, budget)?,
                None,
            )
        };
        drop(active_wire);
        let candidate = codec::read_document(request.candidate_path_v1, budget)?;
        ensure!(
            candidate.fingerprint_v1() == request.expected_candidate_sha256_v1
                && observed
                    .as_ref()
                    .is_none_or(|active| active.payload_v1() == candidate.payload_v1()),
            "candidate selection changed"
        );
        let old = current.state_root_v1();
        let new = candidate.state_root_v1();
        ensure!(
            old != new && !old.starts_with(new) && !new.starts_with(old),
            "switch roots must be distinct and non-nested"
        );
        charge(budget, 2, 0, 0, 0)?;
        let old_meta = fs::metadata(old)?;
        let new_meta = fs::metadata(new)?;
        ensure!(
            (old_meta.dev(), old_meta.ino()) != (new_meta.dev(), new_meta.ino()),
            "switch roots alias the same physical directory"
        );
        match request.mode_v1 {
            DeploymentRecoveryModeV1::RollbackLegacy => ensure!(
                current.native_command_v1().is_some() && candidate.native_command_v1().is_none(),
                "rollback requires Native current and saved Legacy candidate"
            ),
            DeploymentRecoveryModeV1::RestoreForward => {
                let new = candidate
                    .native_command_v1()
                    .and_then(|c| c.identity)
                    .ok_or_else(|| anyhow!("restore-forward candidate is not Native"))?;
                if let Some(old) = current.native_command_v1().and_then(|c| c.identity) {
                    ensure!(
                        old.root_nonce != new.root_nonce,
                        "switch roots must have independent Native incarnations"
                    );
                    ensure!(
                        old.manifest_operation.workspace == new.manifest_operation.workspace,
                        "switch candidate belongs to another workspace"
                    );
                }
            }
        }
        let this = Self {
            current,
            observed,
            active_file_version,
            recovery_publication,
            witness_binding: None,
            recovery_witness: None,
            candidate,
            path: request.selection_path_v1.to_owned(),
            candidate_path: request.candidate_path_v1.to_owned(),
            directory,
            lock,
            phase: *phase,
            _memory: memory,
        };
        drop(Self::verify_binaries(
            &this.current,
            &this.candidate,
            budget,
        )?);
        this.recheck_custody(budget)?;
        Ok(this)
    }

    fn recheck_custody(&self, budget: &mut WorkBudget) -> Result<()> {
        let parent = selection_parent(&self.path, budget)?;
        charge(budget, 3, 0, 0, 0)?;
        let held = self.directory.metadata()?;
        let actual = fs::symlink_metadata(parent)?;
        ensure!(
            (held.dev(), held.ino()) == (actual.dev(), actual.ino()),
            "deployment directory changed"
        );
        let held = self.lock.metadata()?;
        let actual = fs::symlink_metadata(
            self.path
                .parent()
                .ok_or_else(|| anyhow!("deployment parent missing"))?
                .join(LOCK_NAME),
        )?;
        control_metadata(&actual)?;
        ensure!(
            (held.dev(), held.ino()) == (actual.dev(), actual.ino()),
            "deployment lock inode changed"
        );
        Ok(())
    }

    fn recheck_selection_pair(&self, budget: &mut WorkBudget) -> Result<()> {
        self.recheck_custody(budget)?;
        let (bytes, file_version, file) = read_active_control(&self.directory, &self.path, budget)?;
        ensure!(
            file_version == self.active_file_version,
            "current selection file version changed"
        );
        let current = codec::decode(&bytes, budget)?;
        ensure!(
            current.payload_v1() == self.observed.as_ref().unwrap_or(&self.current).payload_v1(),
            "current selection changed"
        );
        if self.observed.is_some() {
            let expected = self
                .recovery_publication
                .ok_or_else(|| anyhow!("reconciliation has no publication binding"))?;
            require_recovery_publication(&file, expected, budget)?;
        }
        let candidate = codec::read_document(&self.candidate_path, budget)?;
        ensure!(
            candidate.payload_v1() == self.candidate.payload_v1(),
            "candidate selection changed"
        );
        Ok(())
    }

    fn recheck_documents(&self, budget: &mut WorkBudget) -> Result<codec::VerifiedBinary> {
        self.recheck_selection_pair(budget)?;
        Self::verify_binaries(&self.current, &self.candidate, budget)
    }

    fn recheck_selection_file(&self, file: &File, budget: &mut WorkBudget) -> Result<()> {
        self.recheck_custody(budget)?;
        charge(budget, 2, 0, 0, 0)?;
        let name = self
            .path
            .file_name()
            .ok_or_else(|| anyhow!("selection basename missing"))?;
        require_named_control_file(&self.directory, Path::new(name), file)
    }

    fn verify_binaries(
        current: &DeploymentSelectionDocumentV1,
        candidate: &DeploymentSelectionDocumentV1,
        budget: &mut WorkBudget,
    ) -> Result<codec::VerifiedBinary> {
        // Reauthenticate at every phase. Reuse only the exact same binding
        // within this pair; never cache a digest across verification phases.
        let current_binary = current.verify_binary_v1(budget)?;
        if current.binary_v1() == candidate.binary_v1()
            && current.binary_sha256_v1() == candidate.binary_sha256_v1()
        {
            return Ok(current_binary);
        }
        candidate.verify_binary_v1(budget)
    }

    fn classify(&self, cause: anyhow::Error) -> DeploymentSelectionErrorV1 {
        self.phase.classify(cause)
    }

    fn normalize(&self, error: DeploymentSelectionErrorV1) -> DeploymentSelectionErrorV1 {
        match error {
            DeploymentSelectionErrorV1::RefusedBeforeRename(cause) => self.classify(cause),
            uncertain @ DeploymentSelectionErrorV1::PersistenceUncertain(_) => uncertain,
        }
    }

    /// Exercise only the selection transaction in the publisher's unit tests.
    #[cfg(test)]
    fn publish_v1(
        &mut self,
        budget: &mut WorkBudget,
    ) -> std::result::Result<DeploymentSelectionPublishedV1, DeploymentSelectionErrorV1> {
        self.publish_inner(budget, &mut |_| Ok(()))
    }

    /// Revalidate live domain custody after all prepublication IO and after
    /// directory fsync, inside the same publisher's phase classification.
    pub(crate) fn publish_with_custody_v1(
        &mut self,
        budget: &mut WorkBudget,
        checkpoint: &mut dyn FnMut(&mut WorkBudget) -> Result<()>,
    ) -> std::result::Result<DeploymentSelectionPublishedV1, DeploymentSelectionErrorV1> {
        self.publish_inner_with_custody(budget, checkpoint, &mut |_| Ok(()))
    }

    #[cfg(test)]
    fn publish_inner(
        &mut self,
        budget: &mut WorkBudget,
        hook: &mut impl FnMut(PublishStage) -> Result<()>,
    ) -> std::result::Result<DeploymentSelectionPublishedV1, DeploymentSelectionErrorV1> {
        self.publish_inner_with_custody(budget, &mut |_| Ok(()), hook)
    }

    fn publish_inner_with_custody(
        &mut self,
        budget: &mut WorkBudget,
        checkpoint: &mut dyn FnMut(&mut WorkBudget) -> Result<()>,
        hook: &mut impl FnMut(PublishStage) -> Result<()>,
    ) -> std::result::Result<DeploymentSelectionPublishedV1, DeploymentSelectionErrorV1> {
        if self.phase == PublicationPhase::ReconciliationPending {
            return self.reconcile_inner(budget, checkpoint, hook);
        }
        let mut temporary = None;
        let result = (|| -> Result<DeploymentSelectionPublishedV1> {
            ensure!(
                self.phase == PublicationPhase::BeforeRename,
                "deployment transaction already attempted publication"
            );
            ensure!(
                self.witness_binding.is_none()
                    || matches!(
                        self.recovery_witness.as_ref(),
                        Some(RecoveryWitnessV1::Prepared { .. })
                    ),
                "verified recovery outcome is missing its publication witness"
            );
            drop(self.recheck_documents(budget)?);
            let name = self
                .path
                .file_name()
                .ok_or_else(|| anyhow!("selection basename missing"))?
                .to_str()
                .ok_or_else(|| anyhow!("selection basename is not UTF-8"))?;
            for _ in 0..16 {
                let serial = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
                let temp = format!(".{name}.tmp-{}-{serial:016x}", std::process::id());
                charge(budget, 0, 2, 0, 2)?;
                match rustix::fs::openat(
                    &self.directory,
                    temp.as_str(),
                    OFlags::CREATE | OFlags::EXCL | OFlags::RDWR | OFlags::CLOEXEC,
                    Mode::RUSR | Mode::WUSR,
                ) {
                    Ok(fd) => {
                        temporary = Some((temp, File::from(fd)));
                        break;
                    }
                    Err(rustix::io::Errno::EXIST) => continue,
                    Err(e) => return Err(e.into()),
                }
            }
            let (temp, file) = temporary
                .as_mut()
                .ok_or_else(|| anyhow!("selection temporary collision limit"))?;
            charge(
                budget,
                0,
                3,
                u64::try_from(self.candidate.payload_v1().len())?,
                1,
            )?;
            rustix::fs::fchmod(&*file, Mode::RUSR | Mode::WUSR)?;
            file.write_all(self.candidate.payload_v1())?;
            if let Some(publication) = self.recovery_publication {
                write_recovery_publication(file, publication, budget)?;
            }
            if let Some(RecoveryWitnessV1::Prepared { bytes, .. }) = &self.recovery_witness {
                let (_, version, active) =
                    read_active_control(&self.directory, &self.path, budget)?;
                ensure!(
                    version == self.active_file_version,
                    "current selection file version changed"
                );
                completion_witness::stage(
                    &self.directory,
                    OsStr::new(name),
                    &active,
                    file,
                    bytes,
                    budget,
                )?;
            }
            file.sync_all()?;
            hook(PublishStage::AfterFileFsync)?;
            let candidate_binary = self.recheck_documents(budget)?;
            checkpoint(budget)?;
            self.recheck_selection_pair(budget)?;
            drop(require_control_bytes(
                file,
                self.candidate.payload_v1(),
                budget,
            )?);
            if let Some(publication) = self.recovery_publication {
                require_recovery_publication(file, publication, budget)?;
            }
            self.require_prepared_witness_v1(
                OsStr::new(name),
                OsStr::new(temp.as_str()),
                file,
                budget,
            )?;
            candidate_binary.require_current(self.candidate.binary_v1(), budget)?;
            charge(budget, 2, 0, 0, 0)?;
            require_named_control_file(&self.directory, Path::new(temp.as_str()), file)?;
            // Admit rename+directory sync before the irreversible boundary.
            charge(budget, 0, 2, 0, 1)?;
            self.phase = PublicationPhase::RenameAttempted;
            rustix::fs::renameat(&self.directory, temp.as_str(), &self.directory, name)?;
            hook(PublishStage::AfterRename)?;
            self.directory.sync_all()?;
            hook(PublishStage::AfterDirectoryFsync)?;
            checkpoint(budget)?;
            let selected = require_control_bytes(file, self.candidate.payload_v1(), budget)?;
            if let Some(publication) = self.recovery_publication {
                require_recovery_publication(file, publication, budget)?;
            }
            self.require_prepared_witness_v1(OsStr::new(name), OsStr::new(name), file, budget)?;
            // Decode the observed document again to authenticate its config.
            let _selected = codec::decode(&selected, budget)?;
            candidate_binary.require_current(self.candidate.binary_v1(), budget)?;
            self.recheck_selection_file(file, budget)?;
            budget.checkpoint()?;
            self.phase = PublicationPhase::Durable;
            Ok(DeploymentSelectionPublishedV1 {
                resolution_v1: DeploymentSelectionResolutionV1::Published,
                before_sha256_v1: self.current.fingerprint_v1(),
                after_sha256_v1: self.candidate.fingerprint_v1(),
            })
        })();
        if self.phase == PublicationPhase::BeforeRename
            && let Some((name, file)) = temporary
            && require_named_control_file(&self.directory, Path::new(&name), &file).is_ok()
        {
            // Leave a replacement belonging to another writer untouched.
            let _cleanup =
                rustix::fs::unlinkat(&self.directory, name.as_str(), rustix::fs::AtFlags::empty());
        }
        result.map_err(|error| self.classify(error))
    }

    fn require_prepared_witness_v1(
        &self,
        name: &OsStr,
        file_name: &OsStr,
        file: &File,
        budget: &mut WorkBudget,
    ) -> Result<()> {
        if let Some(RecoveryWitnessV1::Prepared { bytes, .. }) = &self.recovery_witness {
            completion_witness::require(&self.directory, name, file_name, file, bytes, budget)?;
        }
        Ok(())
    }

    fn reconcile_inner(
        &mut self,
        budget: &mut WorkBudget,
        checkpoint: &mut dyn FnMut(&mut WorkBudget) -> Result<()>,
        hook: &mut impl FnMut(PublishStage) -> Result<()>,
    ) -> std::result::Result<DeploymentSelectionPublishedV1, DeploymentSelectionErrorV1> {
        let result = match (&self.recovery_witness, &self.witness_binding) {
            (Some(RecoveryWitnessV1::Selected(expected_digest)), _) => self
                .with_selected_recovery_witness_v1(budget, |_, digest, budget| {
                    ensure!(
                        digest == *expected_digest,
                        "selected recovery witness changed"
                    );
                    self.sync_observed_recovery_v1(budget, checkpoint, hook)
                })
                .and_then(|observed| {
                    observed.ok_or_else(|| anyhow!("selected recovery witness is absent"))
                }),
            (None, None) => self.sync_observed_recovery_v1(budget, checkpoint, hook),
            _ => Err(anyhow!(
                "verified recovery outcome is missing its publication witness"
            )),
        };
        if result.is_ok() {
            self.phase = PublicationPhase::Durable;
        }
        result.map_err(|error| self.classify(error))
    }

    fn sync_observed_recovery_v1(
        &self,
        budget: &mut WorkBudget,
        checkpoint: &mut dyn FnMut(&mut WorkBudget) -> Result<()>,
        hook: &mut impl FnMut(PublishStage) -> Result<()>,
    ) -> Result<DeploymentSelectionPublishedV1> {
        ensure!(
            self.phase == PublicationPhase::ReconciliationPending,
            "recovery candidate is not selected"
        );
        drop(self.recheck_documents(budget)?);
        let (bytes, version, mut file) = read_active_control(&self.directory, &self.path, budget)?;
        ensure!(
            version == self.active_file_version,
            "current selection file version changed"
        );
        let actual = codec::decode(&bytes, budget)?;
        ensure!(
            actual.payload_v1() == self.candidate.payload_v1(),
            "reconciliation file is not the exact candidate"
        );
        let expected = self
            .recovery_publication
            .ok_or_else(|| anyhow!("reconciliation has no publication binding"))?;
        require_recovery_publication(&file, expected, budget)?;
        charge(budget, 0, 2, 0, 0)?;
        checkpoint(budget)?;
        drop(require_control_bytes(&mut file, &bytes, budget)?);
        file.sync_all()?;
        hook(PublishStage::AfterFileFsync)?;
        self.directory.sync_all()?;
        hook(PublishStage::AfterDirectoryFsync)?;
        checkpoint(budget)?;
        let candidate_binary = self.recheck_documents(budget)?;
        // The canonical digest can stay equal after a raw rewrite. The
        // bytes we report as durable must still be the ones we synced.
        drop(require_control_bytes(&mut file, &bytes, budget)?);
        require_recovery_publication(&file, expected, budget)?;
        self.recheck_selection_file(&file, budget)?;
        candidate_binary.require_current(self.candidate.binary_v1(), budget)?;
        budget.checkpoint()?;
        Ok(DeploymentSelectionPublishedV1 {
            resolution_v1: DeploymentSelectionResolutionV1::Reconciled,
            before_sha256_v1: self.current.fingerprint_v1(),
            after_sha256_v1: self.candidate.fingerprint_v1(),
        })
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum PublicationPhase {
    BeforeRename,
    ReconciliationPending,
    RenameAttempted,
    Durable,
}

impl PublicationPhase {
    fn classify(self, cause: anyhow::Error) -> DeploymentSelectionErrorV1 {
        if self == Self::BeforeRename {
            DeploymentSelectionErrorV1::RefusedBeforeRename(cause)
        } else {
            DeploymentSelectionErrorV1::PersistenceUncertain(cause)
        }
    }
}

#[derive(Clone, Copy)]
enum PublishStage {
    AfterFileFsync,
    AfterRename,
    AfterDirectoryFsync,
}

fn charge(
    budget: &mut WorkBudget,
    reads: u64,
    writes: u64,
    bytes: u64,
    mutations: u64,
) -> Result<()> {
    budget.charge(WorkUsage {
        reads,
        writes,
        bytes,
        mutations,
    })?;
    Ok(())
}
fn control_metadata(m: &fs::Metadata) -> Result<()> {
    ensure!(
        m.is_file()
            && m.uid() == rustix::process::geteuid().as_raw()
            && m.nlink() == 1
            && m.mode() & 0o077 == 0,
        "control file must be private, singly linked, and owned by this user"
    );
    Ok(())
}
fn require_named_control_file(directory: &File, name: &Path, file: &File) -> Result<()> {
    let held = rustix::fs::fstat(file)?;
    let named = rustix::fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)?;
    ensure!(
        (held.st_dev, held.st_ino) == (named.st_dev, named.st_ino),
        "control file inode changed"
    );
    ensure!(
        FileType::from_raw_mode(named.st_mode) == FileType::RegularFile
            && named.st_uid == rustix::process::geteuid().as_raw()
            && named.st_nlink == 1
            && named.st_mode & 0o077 == 0,
        "control file must be private, singly linked, and owned by this user"
    );
    Ok(())
}

fn require_control_bytes(
    file: &mut File,
    expected: &[u8],
    budget: &mut WorkBudget,
) -> Result<Vec<u8>> {
    charge(budget, 1, 0, 0, 0)?;
    let _start = file.seek(SeekFrom::Start(0))?;
    let bytes = read_control_file(file, budget)?;
    ensure!(bytes == expected, "selection bytes changed");
    Ok(bytes)
}

fn selection_parent(path: &Path, budget: &mut WorkBudget) -> Result<PathBuf> {
    ensure!(
        path.is_absolute()
            && path
                .to_str()
                .is_some_and(|s| s.len() <= 4096 && !s.contains('\0'))
            && path.file_name().is_some(),
        "selection path must be a bounded absolute file path"
    );
    ensure!(
        path.file_name().is_some_and(|name| name != LOCK_NAME),
        "selection path aliases deployment lock"
    );
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("selection parent missing"))?;
    charge(budget, 2, 0, 0, 0)?;
    let canonical = fs::canonicalize(parent)?;
    ensure!(
        canonical.as_os_str() == parent.as_os_str(),
        "selection parent is not canonical"
    );
    let m = fs::metadata(&canonical)?;
    ensure!(
        m.is_dir() && m.uid() == rustix::process::geteuid().as_raw() && m.mode() & 0o022 == 0,
        "selection parent must be owned by this user and not group/world writable"
    );
    Ok(canonical)
}
fn read_control(path: &Path, budget: &mut WorkBudget) -> Result<Vec<u8>> {
    charge(budget, 2, 0, 0, 0)?;
    let mut file = File::from(rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    read_control_file(&mut file, budget)
}

fn read_active_control(
    directory: &File,
    path: &Path,
    budget: &mut WorkBudget,
) -> Result<(Vec<u8>, Digest, File)> {
    let name = Path::new(
        path.file_name()
            .ok_or_else(|| anyhow!("selection basename missing"))?,
    );
    charge(budget, 5, 0, 0, 0)?;
    let mut file = File::from(rustix::fs::openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let version = NativeCutoverEvidenceBeforeSelectionV1::file_version_v1(&file.metadata()?);
    let bytes = read_control_file(&mut file, budget)?;
    require_named_control_file(directory, name, &file)?;
    ensure!(
        version == NativeCutoverEvidenceBeforeSelectionV1::file_version_v1(&file.metadata()?),
        "active selection changed while reading"
    );
    Ok((bytes, version, file))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn recovery_publication_on_file(
    file: &File,
    publication: Digest,
    budget: &mut WorkBudget,
) -> Result<Digest> {
    charge(budget, 1, 0, 0, 0)?;
    let metadata = file.metadata()?;
    let mut digest = Sha256::new();
    digest.update(b"quanta.native-cutover.publication-inode.v1\0");
    digest.update(publication.0);
    digest.update(metadata.dev().to_be_bytes());
    digest.update(metadata.ino().to_be_bytes());
    Ok(Digest(digest.finalize().into()))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn write_recovery_publication(
    file: &File,
    publication: Digest,
    budget: &mut WorkBudget,
) -> Result<()> {
    let digest = recovery_publication_on_file(file, publication, budget)?;
    charge(budget, 0, 1, 32, 1)?;
    rustix::fs::fsetxattr(
        file,
        PUBLICATION_ATTRIBUTE,
        &digest.0,
        rustix::fs::XattrFlags::CREATE,
    )
    .context("record recovery publication on selection file")
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn require_recovery_publication(
    file: &File,
    publication: Digest,
    budget: &mut WorkBudget,
) -> Result<()> {
    let expected = recovery_publication_on_file(file, publication, budget)?;
    charge(budget, 1, 0, 32, 0)?;
    let mut bytes = [0_u8; 32];
    let length = rustix::fs::fgetxattr(file, PUBLICATION_ATTRIBUTE, &mut bytes)
        .context("read recovery publication from selection file")?;
    ensure!(
        length == bytes.len() && bytes == expected.0,
        "selection was published by another recovery operation"
    );
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn write_recovery_publication(_: &File, _: Digest, _: &mut WorkBudget) -> Result<()> {
    Err(anyhow!(
        "recovery selection publication requires file extended attributes"
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn require_recovery_publication(_: &File, _: Digest, _: &mut WorkBudget) -> Result<()> {
    Err(anyhow!(
        "recovery selection reconciliation requires file extended attributes"
    ))
}

fn read_control_file(file: &mut File, budget: &mut WorkBudget) -> Result<Vec<u8>> {
    charge(budget, 1, 0, u64::try_from(MAX_BYTES + 1)?, 0)?;
    control_metadata(&file.metadata()?)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(MAX_BYTES + 1)?;
    let _read_bytes = file
        .take(u64::try_from(MAX_BYTES + 1)?)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_BYTES, "control file exceeds byte bound");
    Ok(bytes)
}
fn fingerprint(bytes: &[u8]) -> Digest {
    Digest(Sha256::digest(bytes).into())
}
