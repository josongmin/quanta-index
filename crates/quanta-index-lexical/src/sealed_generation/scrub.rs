//! The integrity scrub over sealed lexical generations, and the receipts it
//! leaves beside them (QI-BB-017).
//!
//! A door proves the index segment files by presence and length only; the
//! seal proved their content once, and between seals only the scrub does.
//! The scrub is bounded and resumable: each step re-checks every committed
//! file's presence and length, then hashes committed files from a cursor
//! until a byte budget is spent. Two receipts live beside a generation,
//! written by atomic durable rename and never part of the sealed manifest
//! (so writing one never makes the generation look tampered with):
//!
//! - the scrub receipt, when a pass over every committed file completed
//!   and matched — the manifest digest it proved and when;
//! - the quarantine receipt, when a committed file did not match. While it
//!   exists the inventory quarantines the generation as
//!   `GENERATION_QUARANTINE_CONTENT_CORRUPT` and every door refuses it
//!   typed `GENERATION_QUARANTINED`; the quarantine discard surface is the
//!   one way it leaves the disk.

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use super::receipts::{
    LEXICAL_SCRUB_RECEIPT_FILE_NAME, LEXICAL_SCRUB_RECEIPT_FORMAT_VERSION, MAX_RECEIPT_BYTES,
    ScrubReceiptRow, now_unix, quarantine_content_corrupt_at, quarantine_seal_failure_at,
    refuse_if_quarantined_at, seal_failure_quarantine_reason,
};
#[cfg(test)]
use super::receipts::{MAX_QUARANTINE_DETAIL_BYTES, bounded_quarantine_detail, read_receipt_value};
use crate::file_authority::paged::{AuthorityRoot, FileScrubCursor, verify_scrub_step};
use crate::lexical_store::{ObjectRef, ObjectStore};
use crate::sealed_generation::manifest::read_bound_manifest_at;
use crate::text_authority::{NativeRoot, NativeScrubCursor};
use quanta_index_contract::GenerationSnapshot;
use quanta_index_core::publication_admission::PublicationObjectLifetimePort;
use quanta_index_core::{
    CoreError, IntegrityScrubBudgetV1, IntegrityScrubCursorV1, IntegrityScrubOutcomeV1,
    IntegrityScrubReportV1, QuarantinedGenerationV1, SealedArtifactCommitmentV1,
    TreeCommitmentMismatchV1, TreeScrubVerdictV1, hash_committed_step_opened_v1,
};

const MAX_SHARED_FRONTIER: usize = 32 * 1024;
const MAX_SHARED_DEPTH: u16 = 128;

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum SharedScrubPhase {
    #[default]
    Outer,
    Objects,
    File,
    Native,
    Done,
}

/// In-process continuation for the pool and semantic proof. The core cursor
/// remains a token issued by this adapter; restart starts at zero.
#[derive(Default)]
pub(crate) struct SharedScrubProgress {
    generation: Option<GenerationSnapshot>,
    token: u64,
    phase: SharedScrubPhase,
    pending: Vec<(ObjectRef, u16)>,
    store: Option<ObjectStore>,
    file_root: Option<AuthorityRoot>,
    native_root: Option<NativeRoot>,
    file_cursor: FileScrubCursor,
    native_cursor: Option<NativeScrubCursor>,
    object_count: u64,
    shared_bytes_read: u64,
}

impl SharedScrubProgress {
    pub(crate) fn fault_reference(&self) -> Result<Option<ObjectRef>, CoreError> {
        self.store
            .as_ref()
            .map_or(Ok(None), ObjectStore::fault_reference)
    }

    fn bind(&mut self, generation: &GenerationSnapshot, start: u64) -> Result<(), CoreError> {
        if start == 0 {
            *self = Self::default();
            self.generation = Some(generation.clone());
            return Ok(());
        }
        if self.generation.as_ref() != Some(generation) || self.token != start {
            return Err(CoreError::InvalidContract(
                "lexical: shared scrub cursor differs from issued generation and position".into(),
            ));
        }
        Ok(())
    }

    fn advance_token(&mut self) -> Result<(), CoreError> {
        self.token = self
            .token
            .checked_add(1)
            .ok_or_else(|| CoreError::InvalidContract("lexical scrub cursor exhausted".into()))?;
        Ok(())
    }

    fn begin_objects(
        &mut self,
        state_root: &Path,
        generation_dir: &Path,
        object_lifetime: Option<Arc<dyn PublicationObjectLifetimePort>>,
    ) -> Result<(), CoreError> {
        let (file_root, native_root) = crate::lexical_gc::protected_authorities(generation_dir)?;
        let mut roots = file_root.object_roots();
        roots.extend(native_root.root_objects());
        self.pending.try_reserve(roots.len()).map_err(|error| {
            CoreError::Storage(format!("lexical: reserve scrub roots: {error}"))
        })?;
        self.pending
            .extend(roots.into_iter().rev().map(|root| (root, 0)));
        self.store = Some(
            ObjectStore::open_existing_with_lifetime(state_root, object_lifetime)?.metered_clone(),
        );
        self.file_root = Some(file_root);
        self.native_root = Some(native_root);
        self.phase = SharedScrubPhase::Objects;
        Ok(())
    }

    fn advance_shared(&mut self, max_bytes: u64) -> Result<(u64, u64, bool), CoreError> {
        self.advance_shared_with(max_bytes, &mut |store, reference| {
            store.object_dependencies(reference, None)
        })
    }

    fn advance_shared_with<F>(
        &mut self,
        max_bytes: u64,
        read_dependencies: &mut F,
    ) -> Result<(u64, u64, bool), CoreError>
    where
        F: FnMut(&ObjectStore, &ObjectRef) -> Result<Vec<ObjectRef>, CoreError>,
    {
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| CoreError::InvalidContract("lexical scrub has no shared store".into()))?
            .clone();
        if !store.has_read_meter() {
            return Err(CoreError::InvalidContract(
                "lexical scrub shared store is not metered".into(),
            ));
        }
        let mut bytes = 0_u64;
        let mut reads = 0_u64;
        // Each call is capped independently of a caller's large byte budget.
        for _ in 0..64 {
            if self.phase == SharedScrubPhase::Done {
                return Ok((bytes, reads, true));
            }
            if reads > 0 && bytes >= max_bytes {
                return Ok((bytes, reads, false));
            }
            match self.phase {
                SharedScrubPhase::Objects => {
                    let Some((reference, depth)) = self.pending.last().cloned() else {
                        self.phase = SharedScrubPhase::File;
                        continue;
                    };
                    if depth >= MAX_SHARED_DEPTH {
                        return Err(CoreError::InvalidContract(
                            "lexical scrub shared closure depth exceeded".into(),
                        ));
                    }
                    let before = store.read_meter_snapshot();
                    let children = read_dependencies(&store, &reference)?;
                    let after = store.read_meter_snapshot();
                    let next_bytes = bytes
                        .checked_add(after.0.checked_sub(before.0).ok_or_else(|| {
                            CoreError::InvalidContract("lexical scrub byte meter regressed".into())
                        })?)
                        .ok_or_else(|| {
                            CoreError::InvalidContract("lexical scrub bytes overflow".into())
                        })?;
                    let next_reads = reads
                        .checked_add(after.1.checked_sub(before.1).ok_or_else(|| {
                            CoreError::InvalidContract("lexical scrub read meter regressed".into())
                        })?)
                        .ok_or_else(|| {
                            CoreError::InvalidContract("lexical scrub reads overflow".into())
                        })?;
                    let next_object_count = self.object_count.checked_add(1).ok_or_else(|| {
                        CoreError::InvalidContract("lexical scrub object count overflow".into())
                    })?;
                    if self
                        .pending
                        .len()
                        .checked_sub(1)
                        .and_then(|count| count.checked_add(children.len()))
                        .is_none_or(|count| count > MAX_SHARED_FRONTIER)
                    {
                        return Err(CoreError::InvalidContract(
                            "lexical scrub shared closure frontier exceeded".into(),
                        ));
                    }
                    self.pending.try_reserve(children.len()).map_err(|error| {
                        CoreError::Storage(format!("lexical: reserve scrub closure: {error}"))
                    })?;
                    let next_depth = depth.checked_add(1).ok_or_else(|| {
                        CoreError::InvalidContract("lexical scrub depth overflow".into())
                    })?;
                    let next_token = self.token.checked_add(1).ok_or_else(|| {
                        CoreError::InvalidContract("lexical scrub cursor exhausted".into())
                    })?;
                    // Commit the row only after its authenticated read and all
                    // bounds/allocation checks succeed. A retry after an I/O
                    // refusal must still encounter this exact reference.
                    let _removed = self.pending.pop();
                    self.pending
                        .extend(children.into_iter().rev().map(|child| (child, next_depth)));
                    bytes = next_bytes;
                    reads = next_reads;
                    self.object_count = next_object_count;
                    self.token = next_token;
                }
                SharedScrubPhase::File => {
                    let root = self.file_root.as_ref().ok_or_else(|| {
                        CoreError::InvalidContract("lexical scrub File root absent".into())
                    })?;
                    let step = verify_scrub_step(&store, root, &mut self.file_cursor, None)?;
                    bytes = bytes.checked_add(step.bytes_read).ok_or_else(|| {
                        CoreError::InvalidContract("lexical scrub bytes overflow".into())
                    })?;
                    reads = reads.checked_add(step.read_count).ok_or_else(|| {
                        CoreError::InvalidContract("lexical scrub reads overflow".into())
                    })?;
                    self.advance_token()?;
                    if step.done {
                        let native = self.native_root.as_ref().ok_or_else(|| {
                            CoreError::InvalidContract("lexical scrub Native root absent".into())
                        })?;
                        self.native_cursor = Some(NativeScrubCursor::new(native)?);
                        self.phase = SharedScrubPhase::Native;
                    }
                    return Ok((bytes, reads, false));
                }
                SharedScrubPhase::Native => {
                    let cursor = self.native_cursor.as_mut().ok_or_else(|| {
                        CoreError::InvalidContract("lexical scrub Native cursor absent".into())
                    })?;
                    let remaining = max_bytes.saturating_sub(bytes).max(1);
                    let step = cursor.step(&store, remaining, None)?;
                    bytes = bytes.checked_add(step.charged_bytes).ok_or_else(|| {
                        CoreError::InvalidContract("lexical scrub bytes overflow".into())
                    })?;
                    reads = reads.checked_add(step.read_count).ok_or_else(|| {
                        CoreError::InvalidContract("lexical scrub reads overflow".into())
                    })?;
                    self.advance_token()?;
                    if step.done {
                        self.phase = SharedScrubPhase::Done;
                    }
                    return Ok((bytes, reads, step.done));
                }
                SharedScrubPhase::Outer => {
                    return Err(CoreError::InvalidContract(
                        "lexical scrub entered shared phase before outer files".into(),
                    ));
                }
                SharedScrubPhase::Done => return Ok((bytes, reads, true)),
            }
        }
        Ok((bytes, reads, self.phase == SharedScrubPhase::Done))
    }
}

/// Every committed file present at its committed length, through an anchored
/// descriptor. A symlink is corruption even when its target has valid bytes.
fn verify_committed_lengths(
    root: &File,
    generation_dir: &Path,
    committed: &[SealedArtifactCommitmentV1],
) -> Result<Option<TreeCommitmentMismatchV1>, CoreError> {
    for artifact in committed {
        let path = generation_dir.join(&artifact.name);
        match super::open_regular_below(root, Path::new(&artifact.name))
            .and_then(|file| file.metadata())
        {
            Ok(metadata) if metadata.len() == artifact.bytes => {}
            Ok(metadata) => {
                return Ok(Some(TreeCommitmentMismatchV1::Length {
                    name: artifact.name.clone(),
                    on_disk: metadata.len(),
                    committed: artifact.bytes,
                }));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Some(TreeCommitmentMismatchV1::Missing {
                    name: artifact.name.clone(),
                }));
            }
            Err(error) if super::is_unsafe_artifact_path(&error) => {
                return Ok(Some(TreeCommitmentMismatchV1::Digest {
                    name: artifact.name.clone(),
                }));
            }
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "lexical: scrub: stat {}: {error}",
                    path.display()
                )));
            }
        }
    }
    Ok(None)
}

/// Write the quarantine receipt the inventory and every door will read.
fn quarantine(
    root: &File,
    generation_dir: &Path,
    mismatch: &TreeCommitmentMismatchV1,
) -> Result<QuarantinedGenerationV1, CoreError> {
    quarantine_content_corrupt_at(root, generation_dir, &format!("scrub found {mismatch}"))
}

/// Defer a shared-object fault until every known referencing generation is fenced.
///
/// The adapter publishes quarantine receipts after completing that ordering.
pub(crate) enum ScrubStepResult {
    Report(IntegrityScrubReportV1),
    DeferredSharedFault {
        files_verified: u64,
        bytes_read: u64,
    },
}

impl From<IntegrityScrubReportV1> for ScrubStepResult {
    fn from(report: IntegrityScrubReportV1) -> Self {
        Self::Report(report)
    }
}

/// One bounded scrub step over the generation sealed for `identity`.
///
/// The manifest must be the one the identity binds; a quarantined
/// generation is refused. Every committed file is checked for presence
/// and length, then files are hashed from `cursor` until `budget` is
/// spent. A shared-object mismatch is deferred until the adapter has fenced
/// every generation reaching that object. Other mismatches write their receipt
/// here; a complete pass writes its completed-pass receipt.
pub(crate) fn scrub_step(
    state_root: &Path,
    generation_dir: &Path,
    identity: &GenerationSnapshot,
    start_artifact: u64,
    budget: IntegrityScrubBudgetV1,
    object_lifetime: Option<&Arc<dyn PublicationObjectLifetimePort>>,
    before_quarantine: &dyn Fn(&GenerationSnapshot) -> Result<(), CoreError>,
    shared: &mut SharedScrubProgress,
) -> Result<ScrubStepResult, CoreError> {
    shared.bind(identity, start_artifact)?;
    let root = super::open_generation_dir_nofollow(generation_dir).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: open generation for scrub {}: {error}",
            generation_dir.display()
        ))
    })?;
    let observed = crate::index_store::read_lexical_sealed_identity_at(generation_dir, &root)?;
    crate::index_store::validate_lexical_sealed_identity(&observed, identity)?;
    refuse_if_quarantined_at(&root, generation_dir)?;
    let manifest = match read_bound_manifest_at(generation_dir, &root, &identity.manifest_digest) {
        Ok(manifest) => manifest,
        Err(error) => {
            if let Some(reason) = seal_failure_quarantine_reason(&error) {
                before_quarantine(identity)?;
                return Ok(IntegrityScrubReportV1 {
                    generation: identity.clone(),
                    files_verified: 0,
                    bytes_read: 0,
                    outcome: IntegrityScrubOutcomeV1::Corrupt {
                        quarantined: quarantine_seal_failure_at(
                            &root,
                            generation_dir,
                            reason,
                            &format!("scrub cannot authenticate sealed manifest: {error}"),
                        )?,
                    },
                }
                .into());
            }
            if matches!(
                &error,
                CoreError::Typed {
                    code:
                        quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestMissing
                        | quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
                    ..
                }
            ) {
                before_quarantine(identity)?;
            }
            return Err(error);
        }
    };
    let mut committed: Vec<SealedArtifactCommitmentV1> =
        manifest.all_commitments().cloned().collect();
    if let Some(coverage_root) = &manifest.source_coverage {
        let pages = crate::sealed_generation::coverage::root_page_commitments_at(
            &root,
            generation_dir,
            coverage_root,
            identity,
        );
        match pages {
            Ok(pages) => committed.extend(pages),
            Err(
                error @ CoreError::Typed {
                    code:
                        quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt
                        | quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                    ..
                },
            ) => {
                before_quarantine(identity)?;
                return Ok(IntegrityScrubReportV1 {
                    generation: identity.clone(),
                    files_verified: 0,
                    bytes_read: 0,
                    outcome: IntegrityScrubOutcomeV1::Corrupt {
                        quarantined: quarantine_content_corrupt_at(
                            &root,
                            generation_dir,
                            &format!("scrub cannot authenticate committed coverage root: {error}"),
                        )?,
                    },
                }
                .into());
            }
            Err(error) => return Err(error),
        }
    }
    if let Some(mismatch) = verify_committed_lengths(&root, generation_dir, &committed)? {
        before_quarantine(identity)?;
        return Ok(IntegrityScrubReportV1 {
            generation: identity.clone(),
            files_verified: 0,
            bytes_read: 0,
            outcome: IntegrityScrubOutcomeV1::Corrupt {
                quarantined: quarantine(&root, generation_dir, &mismatch)?,
            },
        }
        .into());
    }
    let mut files_verified = 0_u64;
    let mut bytes_read = 0_u64;
    if shared.phase == SharedScrubPhase::Outer {
        let step = hash_committed_step_opened_v1(
            &|name| super::open_regular_below(&root, Path::new(name)),
            &committed,
            start_artifact,
            budget.max_bytes,
        )
        .map_err(|error| {
            CoreError::Storage(format!(
                "lexical: scrub {}: {error}",
                generation_dir.display()
            ))
        })?;
        files_verified = step.files_verified;
        bytes_read = step.bytes_read;
        match step.verdict {
            TreeScrubVerdictV1::Paused { next_artifact } => {
                shared.token = next_artifact;
                return Ok(IntegrityScrubReportV1 {
                    generation: identity.clone(),
                    files_verified,
                    bytes_read,
                    outcome: IntegrityScrubOutcomeV1::Paused {
                        cursor: IntegrityScrubCursorV1 { next_artifact },
                    },
                }
                .into());
            }
            TreeScrubVerdictV1::Mismatch(mismatch) => {
                before_quarantine(identity)?;
                return Ok(IntegrityScrubReportV1 {
                    generation: identity.clone(),
                    files_verified,
                    bytes_read,
                    outcome: IntegrityScrubOutcomeV1::Corrupt {
                        quarantined: quarantine(&root, generation_dir, &mismatch)?,
                    },
                }
                .into());
            }
            TreeScrubVerdictV1::Completed => {
                shared.token = crate::channel_payloads::count_from_len(committed.len())?;
                if let Err(error) =
                    shared.begin_objects(state_root, generation_dir, object_lifetime.cloned())
                {
                    if let Some(reason) = seal_failure_quarantine_reason(&error) {
                        before_quarantine(identity)?;
                        return Ok(IntegrityScrubReportV1 {
                            generation: identity.clone(),
                            files_verified,
                            bytes_read,
                            outcome: IntegrityScrubOutcomeV1::Corrupt {
                                quarantined: quarantine_seal_failure_at(
                                    &root,
                                    generation_dir,
                                    reason,
                                    &format!("scrub cannot authenticate shared roots: {error}"),
                                )?,
                            },
                        }
                        .into());
                    }
                    return Err(error);
                }
            }
        }
    }
    if bytes_read >= budget.max_bytes && bytes_read > 0 {
        return Ok(IntegrityScrubReportV1 {
            generation: identity.clone(),
            files_verified,
            bytes_read,
            outcome: IntegrityScrubOutcomeV1::Paused {
                cursor: IntegrityScrubCursorV1 {
                    next_artifact: shared.token,
                },
            },
        }
        .into());
    }
    let remaining = budget.max_bytes.saturating_sub(bytes_read);
    let (shared_bytes, shared_reads, done) = match shared.advance_shared(remaining) {
        Ok(step) => step,
        Err(error) => {
            if let Some(reason) = seal_failure_quarantine_reason(&error) {
                if shared.fault_reference()?.is_some() {
                    return Ok(ScrubStepResult::DeferredSharedFault {
                        files_verified,
                        bytes_read,
                    });
                }
                before_quarantine(identity)?;
                return Ok(IntegrityScrubReportV1 {
                    generation: identity.clone(),
                    files_verified,
                    bytes_read,
                    outcome: IntegrityScrubOutcomeV1::Corrupt {
                        quarantined: quarantine_seal_failure_at(
                            &root,
                            generation_dir,
                            reason,
                            &format!(
                                "scrub cannot authenticate shared object or semantics: {error}"
                            ),
                        )?,
                    },
                }
                .into());
            }
            return Err(error);
        }
    };
    bytes_read = bytes_read.checked_add(shared_bytes).ok_or_else(|| {
        CoreError::InvalidContract("lexical scrub step byte count overflow".into())
    })?;
    files_verified = files_verified.checked_add(shared_reads).ok_or_else(|| {
        CoreError::InvalidContract("lexical scrub step read count overflow".into())
    })?;
    shared.shared_bytes_read = shared
        .shared_bytes_read
        .checked_add(shared_bytes)
        .ok_or_else(|| {
            CoreError::InvalidContract("lexical scrub pass byte count overflow".into())
        })?;
    let outcome = if done {
        let outer_files = crate::channel_payloads::count_from_len(committed.len())?;
        let outer_bytes = committed.iter().try_fold(0_u64, |total, artifact| {
            total.checked_add(artifact.bytes).ok_or_else(|| {
                CoreError::InvalidContract("lexical scrub committed byte sum overflow".into())
            })
        })?;
        let files = outer_files
            .checked_add(shared.object_count)
            .ok_or_else(|| {
                CoreError::InvalidContract("lexical scrub receipt file count overflow".into())
            })?;
        let bytes = outer_bytes
            .checked_add(shared.shared_bytes_read)
            .ok_or_else(|| {
                CoreError::InvalidContract("lexical scrub receipt byte count overflow".into())
            })?;
        let row: ScrubReceiptRow = (
            LEXICAL_SCRUB_RECEIPT_FORMAT_VERSION,
            identity.manifest_digest.clone(),
            now_unix()?,
            files,
            bytes,
        );
        let receipt = crate::channel_payloads::encode_cbor(&row, "scrub receipt")?;
        if receipt.len() > MAX_RECEIPT_BYTES {
            return Err(CoreError::Storage(format!(
                "lexical: scrub receipt exceeds {MAX_RECEIPT_BYTES} encoded bytes"
            )));
        }
        crate::index_store::write_atomic_durable_at(
            &root,
            generation_dir,
            LEXICAL_SCRUB_RECEIPT_FILE_NAME,
            &receipt,
            "scrub receipt",
        )?;
        IntegrityScrubOutcomeV1::Completed
    } else {
        IntegrityScrubOutcomeV1::Paused {
            cursor: IntegrityScrubCursorV1 {
                next_artifact: shared.token,
            },
        }
    };
    Ok(IntegrityScrubReportV1 {
        generation: identity.clone(),
        files_verified,
        bytes_read,
        outcome,
    }
    .into())
}

#[cfg(test)]
mod tests {
    use quanta_index_contract::{
        GenerationSnapshot, ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
    };
    use quanta_index_core::domains::integrity::IntegrityScrubProgressV1;
    use quanta_index_core::{CoreError, IntegrityScrubCursorV1, IntegrityScrubOutcomeV1};
    use sha2::Digest as _;

    use crate::lexical_store::{AttemptBinding, ObjectKind, ObjectStore};

    #[test]
    fn paused_shared_scrub_retries_the_same_object_after_transient_read_error()
    -> Result<(), Box<dyn std::error::Error>> {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition))).into()); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(CoreError::Storage(format!($($message)+)).into()); }
            }};
        }
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!())).into()); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+))).into()); }
            }};
        }

        let directory = tempfile::tempdir()?;
        let store = ObjectStore::open(directory.path())?;
        let lease = store.begin_attempt(
            &AttemptBinding {
                root_incarnation: [1; 16],
                repository_sha256: [2; 32],
                revision_sha256: [3; 32],
                source_event_sha256: [4; 32],
                target_generation: 1,
                base_root_sha256: None,
                operations_sha256: [5; 32],
                format: 1,
            },
            &[],
        )?;
        let first = store.put_blob(&lease, ObjectKind::Record, &[], b"first")?;
        let second = store.put_blob(&lease, ObjectKind::Record, &[], b"second")?;
        let generation = GenerationSnapshot {
            repo_id: RepoId::new("repo")?,
            revision_id: RevisionId::new("revision")?,
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: ManifestGeneration::new(1),
            manifest_digest: "fixed-manifest".into(),
        };
        let mut shared = super::SharedScrubProgress {
            generation: Some(generation.clone()),
            phase: super::SharedScrubPhase::Objects,
            // The first successful step pops `first`, leaving `second` for
            // the exact issued continuation cursor.
            pending: vec![(second.root.clone(), 0), (first.root, 0)],
            store: Some(store.metered_clone()),
            ..super::SharedScrubProgress::default()
        };
        let (bytes, reads, done) = shared.advance_shared(1)?;
        check!(!done && bytes > 0 && reads > 0);
        let cursor = IntegrityScrubCursorV1 {
            next_artifact: shared.token,
        };
        check_eq!(cursor.next_artifact, 1);
        let mut progress = IntegrityScrubProgressV1::default();
        progress.record(&generation, &IntegrityScrubOutcomeV1::Paused { cursor })?;
        check_eq!(
            progress.start_or_resume(&generation, Some(cursor))?,
            cursor.next_artifact
        );
        shared.bind(&generation, cursor.next_artifact)?;

        let mut failed_once = false;
        let failed = shared.advance_shared_with(1, &mut |store, reference| {
            if !failed_once {
                failed_once = true;
                return Err(CoreError::Storage("transient object read refusal".into()));
            }
            store.object_dependencies(reference, None)
        });
        check!(matches!(failed, Err(CoreError::Storage(_))));
        check!(failed_once);
        check_eq!(shared.token, cursor.next_artifact);
        check_eq!(shared.object_count, 1);
        check_eq!(shared.pending, vec![(second.root.clone(), 0)]);

        // Damage the still-unread object. A retry of the issued cursor must
        // re-read this exact reference and refuse it, never complete a pass.
        let mut name = String::new();
        for byte in &second.root.sha256 {
            std::fmt::Write::write_fmt(&mut name, format_args!("{byte:02x}"))?;
        }
        let digest_prefix = name.get(..2).ok_or("object digest prefix absent")?;
        let object = directory
            .path()
            .join("lexical-object-pool")
            .join("objects")
            .join(digest_prefix)
            .join(name);
        std::fs::write(object, b"tampered")?;
        check_eq!(
            progress.start_or_resume(&generation, Some(cursor))?,
            cursor.next_artifact
        );
        shared.bind(&generation, cursor.next_artifact)?;
        check!(matches!(
            shared.advance_shared(1),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ));
        check_eq!(shared.pending, vec![(second.root, 0)]);
        Ok(())
    }

    #[test]
    fn completed_receipt_rejects_trailing_cbor() -> Result<(), Box<dyn std::error::Error>> {
        let generation = tempfile::tempdir()?;
        let path = generation
            .path()
            .join(super::LEXICAL_SCRUB_RECEIPT_FILE_NAME);
        let row: super::ScrubReceiptRow = (
            super::LEXICAL_SCRUB_RECEIPT_FORMAT_VERSION,
            "digest".into(),
            1,
            1,
            1,
        );
        let mut bytes = crate::channel_payloads::encode_cbor(&row, "scrub test")?;
        bytes.push(0xff);
        std::fs::write(&path, bytes)?;
        match super::read_receipt_value(
            &path,
            "scrub receipt",
            super::LEXICAL_SCRUB_RECEIPT_FORMAT_VERSION,
        ) {
            Err(quanta_index_core::CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationScrubReceiptInvalid,
                ..
            }) => Ok(()),
            other => Err(format!("trailing receipt bytes were admitted: {other:?}").into()),
        }
    }

    #[test]
    fn receipt_write_stays_in_opened_generation_after_path_replacement()
    -> Result<(), Box<dyn std::error::Error>> {
        let parent = tempfile::tempdir()?;
        let generation = parent.path().join("generation");
        std::fs::create_dir(&generation)?;
        let opened = super::super::open_generation_dir_nofollow(&generation)?;
        let old = parent.path().join("old-generation");
        std::fs::rename(&generation, &old)?;
        std::fs::create_dir(&generation)?;
        crate::index_store::write_atomic_durable_at(
            &opened,
            &generation,
            super::LEXICAL_SCRUB_RECEIPT_FILE_NAME,
            b"pinned",
            "scrub receipt test",
        )?;
        if std::fs::read(old.join(super::LEXICAL_SCRUB_RECEIPT_FILE_NAME))? != b"pinned"
            || generation
                .join(super::LEXICAL_SCRUB_RECEIPT_FILE_NAME)
                .exists()
        {
            return Err("receipt write followed a replacement generation".into());
        }
        Ok(())
    }

    #[test]
    fn long_multibyte_quarantine_detail_stays_bounded_and_identifiable() {
        let original = "가".repeat(2_000);
        let fingerprint = format!("{:x}", sha2::Sha256::digest(original.as_bytes()));
        let bounded = super::bounded_quarantine_detail(&original);
        assert!(bounded.len() <= super::MAX_QUARANTINE_DETAIL_BYTES);
        assert!(bounded.contains(&format!("original_sha256={fingerprint}")));
        assert!(
            bounded
                .split("... [truncated;")
                .next()
                .is_some_and(|prefix| !prefix.is_empty() && original.starts_with(prefix))
        );
    }

    #[test]
    fn oversized_receipt_refuses_before_decode() -> Result<(), Box<dyn std::error::Error>> {
        let generation = tempfile::tempdir()?;
        let path = generation
            .path()
            .join(super::LEXICAL_SCRUB_RECEIPT_FILE_NAME);
        let file = std::fs::File::create(&path)?;
        file.set_len(u64::try_from(super::MAX_RECEIPT_BYTES)? + 1)?;
        let result = super::read_receipt_value(
            &path,
            "scrub receipt",
            super::LEXICAL_SCRUB_RECEIPT_FORMAT_VERSION,
        );
        if !matches!(
            result,
            Err(quanta_index_core::CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationScrubReceiptInvalid,
                ..
            })
        ) {
            return Err("oversized receipt did not refuse typed".into());
        }
        Ok(())
    }
}
