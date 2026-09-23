//! Private legacy journal migration authority.
//!
//! The source side is read-only: [`LegacySemanticJournalReaderV1`] opens the
//! legacy journal directory without creating it, without taking a lock file,
//! and without writing anything into it — a source-side `MIGRATED` receipt
//! is old-binary authority this binary cannot verify, so the journal is
//! refused immutable instead. The migration receipt and every durable
//! generation are written by the staging writer into the new root only.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use quanta_index_contract::{
    ManifestGeneration, OwnerDocKind, RepoId, RevisionId, SemanticCorpusKindV1, SemanticIngestBatch,
};
use quanta_index_core::{
    CoreError, SemanticScopeStreamBuildPort, SemanticStreamWindowPolicy,
    build_resident_semantic_batch_v1,
};
use quanta_index_ipc::{decode_cbor_payload, encode_cbor_payload};
use quanta_index_semantic::{
    ValidatedPersistedSemanticGenerationV2, inventory_persisted_generations,
    validate_persisted_generation_v2,
};
use sha2::{Digest as _, Sha256};

/// Outcome of the one-shot legacy semantic journal migration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticMigrationOutcome {
    /// No legacy `journal.cbor` present; nothing to migrate.
    NoLegacyJournal,
    /// Completion marker already present; migration skipped.
    AlreadyMigrated,
    /// Migration ran and applied `imported` batches into durable generations.
    Migrated { imported: usize },
}

pub(super) fn normalize_legacy_semantic_batch_v1(
    batch: &SemanticIngestBatch,
) -> SemanticIngestBatch {
    let mut normalized = batch.clone();
    for scope in &mut normalized.replace_scopes {
        let legacy_owner_id =
            format!("legacy-path:{}", scope.scope.repo_relative_path.as_str()).into_boxed_str();
        for embedding in &mut scope.embeddings {
            if embedding.owner_kind == OwnerDocKind::Chunk
                && embedding.corpus_kind == SemanticCorpusKindV1::RawCodeFallback
            {
                embedding.owner_id = legacy_owner_id.clone();
                embedding.parent_owner_id = Some(legacy_owner_id.clone());
            }
        }
    }
    normalized
}

const MAX_JOURNAL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RECEIPT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_JOURNAL_BATCHES: usize = 262_144;
const MAX_MIGRATED_GENERATIONS: usize = 262_144;
const RECEIPT_FORMAT: u32 = 2;
const FAIL_AFTER_TEMP_SYNC: &str = "after-temp-sync";
const FAIL_AFTER_RECEIPT_PUBLISH: &str = "after-receipt-publish";

#[cfg(test)]
mod failpoint {
    use std::cell::RefCell;

    thread_local! {
        static BOUNDARY: RefCell<Option<&'static str>> = const { RefCell::new(None) };
    }

    pub(super) fn set(boundary: Option<&'static str>) {
        BOUNDARY.with(|slot| *slot.borrow_mut() = boundary);
    }

    pub(super) fn should_fail(boundary: &str) -> bool {
        BOUNDARY.with(|slot| slot.borrow().is_some_and(|value| value == boundary))
    }
}

#[cfg(not(test))]
mod failpoint {
    pub(super) fn should_fail(_boundary: &str) -> bool {
        false
    }
}

#[derive(Default)]
struct Journal {
    batches: Vec<SemanticIngestBatch>,
}
type JournalWire = (Vec<SemanticIngestBatch>,);

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceRow {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    manifest_digest: String,
}
type SourceRowWire = (RepoId, RevisionId, ManifestGeneration, String);

#[derive(Clone, Debug, Eq, PartialEq)]
struct DurableRow {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    manifest_digest: String,
    semantic_row_root_digest: String,
    row_count: u64,
}
type DurableRowWire = (RepoId, RevisionId, ManifestGeneration, String, String, u64);

#[derive(Clone, Debug, Eq, PartialEq)]
struct Receipt {
    format_version: u32,
    journal_digest: String,
    batch_count: u64,
    sources: Vec<SourceRow>,
    durable: Vec<DurableRow>,
}
type ReceiptWire = (u32, String, u64, Vec<SourceRowWire>, Vec<DurableRowWire>);

fn decode_journal(bytes: &[u8]) -> Result<Journal, CoreError> {
    let (batches,): JournalWire = decode_cbor_payload(bytes).map_err(|error| CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticJournalCorrupt,
        message: format!("legacy semantic migration: decode journal: {error}"),
    })?;
    Ok(Journal { batches })
}

#[cfg(test)]
fn encode_journal(journal: &Journal) -> Result<Vec<u8>, CoreError> {
    encode_cbor_payload(&(journal.batches.clone(),))
        .map_err(|error| CoreError::Storage(format!("legacy journal encode: {error}")))
}

fn receipt_to_wire(receipt: &Receipt) -> ReceiptWire {
    (
        receipt.format_version,
        receipt.journal_digest.clone(),
        receipt.batch_count,
        receipt
            .sources
            .iter()
            .map(|row| {
                (
                    row.repo_id.clone(),
                    row.revision_id.clone(),
                    row.generation,
                    row.manifest_digest.clone(),
                )
            })
            .collect(),
        receipt
            .durable
            .iter()
            .map(|row| {
                (
                    row.repo_id.clone(),
                    row.revision_id.clone(),
                    row.generation,
                    row.manifest_digest.clone(),
                    row.semantic_row_root_digest.clone(),
                    row.row_count,
                )
            })
            .collect(),
    )
}

fn receipt_from_wire(wire: ReceiptWire) -> Receipt {
    Receipt {
        format_version: wire.0,
        journal_digest: wire.1,
        batch_count: wire.2,
        sources: wire
            .3
            .into_iter()
            .map(|row| SourceRow {
                repo_id: row.0,
                revision_id: row.1,
                generation: row.2,
                manifest_digest: row.3,
            })
            .collect(),
        durable: wire
            .4
            .into_iter()
            .map(|row| DurableRow {
                repo_id: row.0,
                revision_id: row.1,
                generation: row.2,
                manifest_digest: row.3,
                semantic_row_root_digest: row.4,
                row_count: row.5,
            })
            .collect(),
    }
}

fn decode_receipt(bytes: &[u8]) -> Result<Receipt, CoreError> {
    decode_cbor_payload::<ReceiptWire>(bytes)
        .map(receipt_from_wire)
        .map_err(|error| CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticMigrationReceiptCorrupt,
            message: error.to_string(),
        })
}

fn encode_receipt(receipt: &Receipt) -> Result<Vec<u8>, CoreError> {
    encode_cbor_payload(&receipt_to_wire(receipt))
        .map_err(|error| CoreError::Storage(format!("receipt encode: {error}")))
}

struct ValidatedMigrationV2(Receipt);

/// Opaque read-only journal snapshot. The source directory is only ever
/// read: receipt construction and persistence happen in the staging root.
#[derive(Debug)]
pub struct LegacySemanticJournalReaderV1 {
    journal_dir: PathBuf,
    journal_path: PathBuf,
    journal_digest: Option<String>,
    batches: Vec<SemanticIngestBatch>,
}

impl LegacySemanticJournalReaderV1 {
    /// Open the legacy journal directory read-only.
    ///
    /// Nothing is created and nothing is written: no directory, no lock
    /// file, no receipt, no cleanup of the source. A missing directory (or a
    /// directory without `journal.cbor`) simply means there is no legacy
    /// journal, and so does a journal that decodes to zero batches: there is
    /// nothing to migrate, revalidate, or receipt. A source-side `MIGRATED`
    /// receipt is old-binary authority this binary cannot verify against a
    /// fresh staging root, so the journal is refused immutable rather than
    /// re-migrated or ignored. A stale `MIGRATED.lock` beside it is inert
    /// residue and is ignored.
    pub fn open(journal_dir: impl AsRef<Path>) -> Result<Self, CoreError> {
        let requested_root = journal_dir.as_ref();
        match fs::symlink_metadata(requested_root) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::empty());
            }
            Err(error) => {
                return Err(storage("inspect journal root", requested_root)(error));
            }
        }
        validate_root_custody(requested_root)?;
        // Pin the resolved directory so ancestor aliases cannot redirect
        // later reads to a different custody tree.
        let journal_dir = fs::canonicalize(requested_root)
            .map_err(storage("canonicalize journal root", requested_root))?;
        validate_root_custody(&journal_dir)?;
        if journal_dir.join("MIGRATED").exists() {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticJournalImmutableAfterMigration,
                message: "legacy journal already has a migration receipt; a source-side receipt is old-binary authority this binary does not verify".to_string(),
            });
        }
        let journal_path = journal_dir.join("journal.cbor");
        let bytes = read_bounded(&journal_dir, &journal_path, MAX_JOURNAL_BYTES, "journal")?;
        let Some(bytes) = bytes else {
            return Ok(Self::empty());
        };
        let journal = decode_journal(&bytes)?;
        if journal.batches.is_empty() {
            return Ok(Self::empty());
        }
        if journal.batches.len() > MAX_JOURNAL_BATCHES {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticMigrationInputTooLarge,
                message: format!(
                    "journal has {} batches; maximum is {MAX_JOURNAL_BATCHES}",
                    journal.batches.len()
                ),
            });
        }
        Ok(Self {
            journal_dir,
            journal_path,
            journal_digest: Some(digest(&bytes)),
            batches: journal.batches,
        })
    }

    fn empty() -> Self {
        Self {
            journal_dir: PathBuf::new(),
            journal_path: PathBuf::new(),
            journal_digest: None,
            batches: Vec::new(),
        }
    }

    #[cfg(test)]
    pub fn stage_for_test(
        root: impl AsRef<Path>,
        batches: &[SemanticIngestBatch],
    ) -> Result<(), CoreError> {
        let root = root.as_ref();
        fs::create_dir_all(root).map_err(storage("create journal root", root))?;
        if root.join("MIGRATED").exists() {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticJournalImmutableAfterMigration,
                message: "legacy journal already has a migration receipt".to_string(),
            });
        }
        let bytes = encode_journal(&Journal {
            batches: batches.to_vec(),
        })?;
        atomic_replace_for_test(root, &root.join("journal.cbor"), &bytes)
    }
}

type Key = (RepoId, RevisionId, ManifestGeneration);

/// Migrate the decoded journal into the staging semantic root.
///
/// `reader` is a read-only snapshot of the source journal; every durable
/// write — the generations, the temporary receipt files, and the published
/// `MIGRATED` receipt — lands under `staging_semantic_root`, which the
/// offline operation owns. The source journal is re-validated before and
/// after the import so a mid-migration source change fails closed.
pub(super) fn migrate(
    reader: &LegacySemanticJournalReaderV1,
    builder: &(dyn SemanticScopeStreamBuildPort + Send + Sync),
    staging_semantic_root: &Path,
    window_policy: SemanticStreamWindowPolicy,
) -> Result<SemanticMigrationOutcome, CoreError> {
    let receipt_path = staging_semantic_root.join("MIGRATED");
    let Some(journal_digest) = reader.journal_digest.as_deref() else {
        if receipt_path.exists() {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticJournalMissing,
                message: "migration receipt exists without retained journal".to_string(),
            });
        }
        return Ok(SemanticMigrationOutcome::NoLegacyJournal);
    };
    revalidate_journal(reader, journal_digest)?;
    let batches: Vec<_> = reader
        .batches
        .iter()
        .map(normalize_legacy_semantic_batch_v1)
        .collect();
    let sources = source_rows(&batches)?;
    let existing = read_receipt(staging_semantic_root, &receipt_path)?;
    let persisted = inventory_persisted_generations(staging_semantic_root)?.sealed;
    if let Some(receipt) = existing {
        let durable = durable_rows(staging_semantic_root, &sources, &persisted)?;
        let expected =
            validated_migration(reader.batches.len(), journal_digest, sources, durable)?.0;
        if receipt != expected {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticMigrationReceiptConflict,
                message: "migration receipt no longer matches journal/durable authority"
                    .to_string(),
            });
        }
        return Ok(SemanticMigrationOutcome::AlreadyMigrated);
    }
    let before = witness_map_for_sources(staging_semantic_root, &persisted, &sources)?;
    let preexisting: BTreeSet<Key> = sources
        .iter()
        .filter_map(|source| {
            let key = (
                source.repo_id.clone(),
                source.revision_id.clone(),
                source.generation,
            );
            before
                .get(&key)
                .filter(|w| w.manifest_digest() == source.manifest_digest)
                .map(|_| key)
        })
        .collect();
    for source in &sources {
        let key = (
            source.repo_id.clone(),
            source.revision_id.clone(),
            source.generation,
        );
        if let Some(witness) = before.get(&key)
            && witness.manifest_digest() != source.manifest_digest
        {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticMigrationDigestConflict,
                message: "journal conflicts with sealed durable generation".to_string(),
            });
        }
    }
    let mut imported = 0usize;
    for batch in &batches {
        let key = (
            batch.repo_id.clone(),
            batch.revision_id.clone(),
            batch.generation,
        );
        if !preexisting.contains(&key) {
            // The journal's batches arrive decoded, so they stream through
            // the same build entry as a live batch, windowed by the policy
            // the builder admits against; see `ResidentScopeSource` for what
            // that does and does not bound.
            let _tally = build_resident_semantic_batch_v1(builder, batch, window_policy)?;
            imported = imported
                .checked_add(1)
                .ok_or_else(|| CoreError::Storage("migration count overflow".to_string()))?;
        }
    }
    let persisted = inventory_persisted_generations(staging_semantic_root)?.sealed;
    let durable = durable_rows(staging_semantic_root, &sources, &persisted)?;
    let receipt = validated_migration(reader.batches.len(), journal_digest, sources, durable)?;
    revalidate_journal(reader, journal_digest)?;
    // The receipt directory is staging-owned: create it only on the publish
    // path, converge its temporaries, then publish the receipt noreplace.
    fs::create_dir_all(staging_semantic_root).map_err(storage(
        "create staging semantic root",
        staging_semantic_root,
    ))?;
    cleanup_stale_temporaries(staging_semantic_root)?;
    write_receipt(staging_semantic_root, &receipt_path, &receipt)?;
    Ok(SemanticMigrationOutcome::Migrated { imported })
}

fn source_rows(batches: &[SemanticIngestBatch]) -> Result<Vec<SourceRow>, CoreError> {
    let mut map = BTreeMap::<Key, String>::new();
    for batch in batches {
        if batch.manifest_digest.is_empty() {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticJournalManifestDigestMissing,
                message: "journal generation has an empty manifest digest".to_string(),
            });
        }
        let key = (
            batch.repo_id.clone(),
            batch.revision_id.clone(),
            batch.generation,
        );
        if let Some(prior) = map.insert(key.clone(), batch.manifest_digest.clone())
            && prior != batch.manifest_digest
        {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticJournalGenerationConflict,
                message: "journal has multiple digests for one generation".to_string(),
            });
        }
    }
    Ok(map
        .into_iter()
        .map(
            |((repo_id, revision_id, generation), manifest_digest)| SourceRow {
                repo_id,
                revision_id,
                generation,
                manifest_digest,
            },
        )
        .collect())
}

/// Prove exactly the journal's generations durable (QI-BB-026): the
/// inventory lists what is sealed, and only the generations the journal
/// names are opened and verified here.
fn witness_map_for_sources(
    semantic_root: &Path,
    persisted: &[quanta_index_semantic::PersistedSemanticGeneration],
    sources: &[SourceRow],
) -> Result<BTreeMap<Key, ValidatedPersistedSemanticGenerationV2>, CoreError> {
    let required: BTreeSet<Key> = sources
        .iter()
        .map(|source| {
            (
                source.repo_id.clone(),
                source.revision_id.clone(),
                source.generation,
            )
        })
        .collect();
    let mut out = BTreeMap::new();
    for record in persisted {
        let key = (
            record.repo_id.clone(),
            record.revision_id.clone(),
            record.generation,
        );
        if required.contains(&key) {
            let witness = validate_persisted_generation_v2(semantic_root, record)?;
            if out.insert(key, witness).is_some() {
                return Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticMigrationDurableGenerationDuplicate,
                    message: "durable scan returned one generation more than once".to_string(),
                });
            }
        }
    }
    Ok(out)
}

fn durable_rows(
    semantic_root: &Path,
    sources: &[SourceRow],
    persisted: &[quanta_index_semantic::PersistedSemanticGeneration],
) -> Result<Vec<DurableRow>, CoreError> {
    let map = witness_map_for_sources(semantic_root, persisted, sources)?;
    sources
        .iter()
        .map(|source| {
            let key = (
                source.repo_id.clone(),
                source.revision_id.clone(),
                source.generation,
            );
            let witness = map.get(&key).ok_or_else(|| CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticMigrationDurableRootMissing,
                message: "journal generation has no validated v7 durable root".to_string(),
            })?;
            if witness.manifest_digest() != source.manifest_digest {
                return Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticMigrationDigestConflict,
                    message: "durable generation digest conflicts with journal".to_string(),
                });
            }
            Ok(DurableRow {
                repo_id: source.repo_id.clone(),
                revision_id: source.revision_id.clone(),
                generation: source.generation,
                manifest_digest: source.manifest_digest.clone(),
                semantic_row_root_digest: witness.semantic_row_root_digest().to_string(),
                row_count: witness.row_count(),
            })
        })
        .collect()
}

fn validated_migration(
    batch_count: usize,
    journal_digest: &str,
    sources: Vec<SourceRow>,
    durable: Vec<DurableRow>,
) -> Result<ValidatedMigrationV2, CoreError> {
    if sources.len() != durable.len()
        || !sources.iter().zip(&durable).all(|(source, durable)| {
            source.repo_id == durable.repo_id
                && source.revision_id == durable.revision_id
                && source.generation == durable.generation
                && source.manifest_digest == durable.manifest_digest
                && is_canonical_sha256(&durable.semantic_row_root_digest)
        })
    {
        return Err(CoreError::InvalidContract(
            "migration witness does not exactly cover source generations".to_string(),
        ));
    }
    Ok(ValidatedMigrationV2(Receipt {
        format_version: RECEIPT_FORMAT,
        journal_digest: journal_digest.to_string(),
        batch_count: u64::try_from(batch_count)
            .map_err(|err| CoreError::Storage(format!("batch count overflow: {err}")))?,
        sources,
        durable,
    }))
}

fn read_receipt(staging_root: &Path, receipt_path: &Path) -> Result<Option<Receipt>, CoreError> {
    let Some(bytes) = read_bounded(staging_root, receipt_path, MAX_RECEIPT_BYTES, "receipt")?
    else {
        return Ok(None);
    };
    if bytes == b"migrated" {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticMigrationReceiptUnsupported,
            message: "unvalidated v1 migration marker cannot authorize durable cutover".to_string(),
        });
    }
    let receipt = decode_receipt(&bytes)?;
    validate_receipt_shape(&receipt)?;
    Ok(Some(receipt))
}

fn validate_receipt_shape(receipt: &Receipt) -> Result<(), CoreError> {
    let batch_count = usize::try_from(receipt.batch_count).map_err(|error| CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticMigrationReceiptCorrupt,
        message: format!("receipt batch count overflow: {error}"),
    })?;
    let exact_rows = receipt.sources.len() == receipt.durable.len()
        && receipt
            .sources
            .iter()
            .zip(&receipt.durable)
            .all(|(source, durable)| {
                source.repo_id == durable.repo_id
                    && source.revision_id == durable.revision_id
                    && source.generation == durable.generation
                    && source.manifest_digest == durable.manifest_digest
                    && !source.manifest_digest.is_empty()
                    && is_canonical_sha256(&durable.semantic_row_root_digest)
            });
    let strictly_ordered = receipt.sources.is_sorted_by(|left, right| {
        (&left.repo_id, &left.revision_id, left.generation)
            < (&right.repo_id, &right.revision_id, right.generation)
    });
    if receipt.format_version != RECEIPT_FORMAT
        || !is_canonical_sha256(&receipt.journal_digest)
        || batch_count > MAX_JOURNAL_BATCHES
        || receipt.sources.len() > MAX_MIGRATED_GENERATIONS
        || !exact_rows
        || !strictly_ordered
    {
        return Err(CoreError::Typed {
            code:
                quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticMigrationReceiptCorrupt,
            message: "receipt shape, bounds, ordering, or content proof is invalid".to_string(),
        });
    }
    Ok(())
}

fn is_canonical_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn write_receipt(
    staging_root: &Path,
    receipt_path: &Path,
    witness: &ValidatedMigrationV2,
) -> Result<(), CoreError> {
    let receipt = &witness.0;
    if let Some(existing) = read_receipt(staging_root, receipt_path)? {
        if existing == *receipt {
            return Ok(());
        }
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticMigrationReceiptConflict,
            message: "refusing to replace a different migration receipt".to_string(),
        });
    }
    let bytes = encode_receipt(receipt)?;
    atomic_publish_noreplace(staging_root, receipt_path, &bytes)
}

fn revalidate_journal(
    reader: &LegacySemanticJournalReaderV1,
    expected: &str,
) -> Result<(), CoreError> {
    let bytes = read_bounded(
        &reader.journal_dir,
        &reader.journal_path,
        MAX_JOURNAL_BYTES,
        "journal",
    )?
    .ok_or_else(|| CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticJournalChangedDuringMigration,
        message: "journal disappeared".to_string(),
    })?;
    if digest(&bytes) != expected {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticJournalChangedDuringMigration,
            message: "journal changed".to_string(),
        });
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn storage<'a>(
    action: &'static str,
    path: &'a Path,
) -> impl FnOnce(std::io::Error) -> CoreError + 'a {
    move |error| {
        CoreError::Storage(format!(
            "legacy semantic migration: {action} {}: {error}",
            path.display()
        ))
    }
}

fn read_bounded(
    root: &Path,
    path: &Path,
    max: u64,
    label: &str,
) -> Result<Option<Vec<u8>>, CoreError> {
    let mut file = match open_read_nofollow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CoreError::Storage(format!(
                "{label} open {}: {error}",
                path.display()
            )));
        }
    };
    validate_opened_file_custody(root, &file, path, true)?;
    if !file
        .metadata()
        .map_err(storage("inspect input", path))?
        .is_file()
    {
        return Err(CoreError::Storage(format!("{label} is not a regular file")));
    }
    let mut bytes = Vec::new();
    let _read = Read::by_ref(&mut file)
        .take(max.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(storage("read bounded input", path))?;
    if u64::try_from(bytes.len()).is_ok_and(|len| len > max) {
        return Err(CoreError::Typed {
            code:
                quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticMigrationInputTooLarge,
            message: format!("{label} exceeds {max} bytes"),
        });
    }
    Ok(Some(bytes))
}

#[cfg(unix)]
fn validate_root_custody(root: &Path) -> Result<(), CoreError> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = fs::symlink_metadata(root).map_err(storage("inspect migration root", root))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() || metadata.mode() & 0o022 != 0 {
        return Err(CoreError::Typed {
            code:
                quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticMigrationCustodyInvalid,
            message: format!(
                "migration root must be a non-symlink directory without group/other write permission: {}",
                root.display()
            ),
        });
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_root_custody(root: &Path) -> Result<(), CoreError> {
    let metadata = fs::symlink_metadata(root).map_err(storage("inspect migration root", root))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(CoreError::Typed {
            code:
                quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticMigrationCustodyInvalid,
            message: format!(
                "migration root is not a non-symlink directory: {}",
                root.display()
            ),
        });
    }
    Ok(())
}

#[cfg(unix)]
fn validate_opened_file_custody(
    root: &Path,
    file: &File,
    path: &Path,
    require_single_link: bool,
) -> Result<(), CoreError> {
    use std::os::unix::fs::MetadataExt as _;

    let root_metadata =
        fs::symlink_metadata(root).map_err(storage("inspect migration root", root))?;
    let metadata = file
        .metadata()
        .map_err(storage("inspect migration input", path))?;
    if !metadata.is_file()
        || metadata.uid() != root_metadata.uid()
        || metadata.mode() & 0o022 != 0
        || (require_single_link && metadata.nlink() != 1)
    {
        return Err(CoreError::Typed {
            code:
                quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticMigrationCustodyInvalid,
            message: format!(
                "migration file must be regular, same-owner, non-shared, and not group/other writable: {}",
                path.display()
            ),
        });
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_opened_file_custody(
    _root: &Path,
    file: &File,
    path: &Path,
    _require_single_link: bool,
) -> Result<(), CoreError> {
    if !file
        .metadata()
        .map_err(storage("inspect migration input", path))?
        .is_file()
    {
        return Err(CoreError::Typed {
            code:
                quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticMigrationCustodyInvalid,
            message: format!("migration input is not a regular file: {}", path.display()),
        });
    }
    Ok(())
}

#[cfg(unix)]
fn open_read_nofollow(path: &Path) -> std::io::Result<File> {
    use rustix::fs::{Mode, OFlags, open};

    open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))
}

#[cfg(not(unix))]
fn open_read_nofollow(path: &Path) -> std::io::Result<File> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(std::io::Error::other(format!(
            "migration input is not a regular non-symlink file: {}",
            path.display()
        )));
    }
    File::open(path)
}

/// Remove interrupted receipt temporaries under a staging-owned directory.
/// Never called on a source root: the source side is read-only.
fn cleanup_stale_temporaries(root: &Path) -> Result<(), CoreError> {
    for entry in fs::read_dir(root).map_err(storage("list migration root", root))? {
        let entry = entry.map_err(storage("read migration entry", root))?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(".MIGRATED.tmp-"))
            && entry
                .file_type()
                .map_err(storage("inspect migration temp", root))?
                .is_file()
        {
            fs::remove_file(entry.path()).map_err(storage("remove stale migration temp", root))?;
        }
    }
    Ok(())
}

fn write_synced_temporary(root: &Path, bytes: &[u8]) -> Result<PathBuf, CoreError> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let temporary = root.join(format!(
        ".MIGRATED.tmp-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        #[cfg(unix)]
        use std::os::unix::fs::OpenOptionsExt as _;

        let mut options = OpenOptions::new();
        let _options = options.write(true).create_new(true);
        #[cfg(unix)]
        let _mode = options.mode(0o600);
        let mut file = options
            .open(&temporary)
            .map_err(storage("create migration temp", &temporary))?;
        validate_opened_file_custody(root, &file, &temporary, true)?;
        file.write_all(bytes)
            .map_err(storage("write migration temp", &temporary))?;
        file.sync_all()
            .map_err(storage("fsync migration temp", &temporary))?;
        drop(file);
        if failpoint::should_fail(FAIL_AFTER_TEMP_SYNC) {
            return Err(CoreError::Storage(
                "injected migration failure after temp fsync".to_string(),
            ));
        }
        Ok(temporary.clone())
    })();
    match result {
        Ok(path) => Ok(path),
        Err(primary) => match fs::remove_file(&temporary) {
            Ok(()) => Err(primary),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(primary),
            Err(cleanup) => Err(CoreError::Storage(format!(
                "{primary}; cleanup {} also failed: {cleanup}",
                temporary.display()
            ))),
        },
    }
}

fn sync_root(root: &Path) -> Result<(), CoreError> {
    File::open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(storage("fsync migration root", root))
}

#[cfg(test)]
fn atomic_replace_for_test(root: &Path, path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
    let temporary = write_synced_temporary(root, bytes)?;
    fs::rename(&temporary, path).map_err(storage("rename migration temp", path))?;
    sync_root(root)
}

fn atomic_publish_noreplace(root: &Path, path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
    let temporary = write_synced_temporary(root, bytes)?;
    let result = (|| {
        fs::hard_link(&temporary, path).map_err(storage("publish migration receipt", path))?;
        fs::remove_file(&temporary)
            .map_err(storage("unlink published migration temp", &temporary))?;
        if failpoint::should_fail(FAIL_AFTER_RECEIPT_PUBLISH) {
            return Err(CoreError::Storage(
                "injected migration failure after receipt publication".to_string(),
            ));
        }
        sync_root(root)
    })();
    match result {
        Ok(()) => Ok(()),
        Err(primary) => match fs::remove_file(&temporary) {
            Ok(()) => Err(primary),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(primary),
            Err(cleanup) => Err(CoreError::Storage(format!(
                "{primary}; cleanup {} also failed: {cleanup}",
                temporary.display()
            ))),
        },
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning migration tests assert with `assert!` on fixture invariants; a violated fixture invariant is not a propagatable error"
)]
mod tests {
    use super::*;

    /// Whether any `.MIGRATED.tmp-*` staging file survives under `root`.
    ///
    /// Directory-entry errors propagate rather than being filtered away: a
    /// test asserting that nothing leaked must not pass because it could not
    /// list the directory.
    fn leaked_migration_temp(root: &Path) -> Result<bool, Box<dyn std::error::Error>> {
        let entries = fs::read_dir(root)?.collect::<Result<Vec<_>, _>>()?;
        Ok(entries.iter().any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".MIGRATED.tmp-")
        }))
    }

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn receipt_publication_never_replaces_existing_destination() -> TestResult {
        let root = tempfile::tempdir()?;
        let receipt = root.path().join("MIGRATED");
        fs::write(&receipt, b"existing-authority")?;

        let error = atomic_publish_noreplace(root.path(), &receipt, b"new-authority")
            .expect_err("publication must fail when a destination appears");
        assert!(matches!(error, CoreError::Storage(_)));
        assert_eq!(fs::read(&receipt)?, b"existing-authority");
        assert!(!leaked_migration_temp(root.path())?);
        Ok(())
    }

    #[test]
    fn receipt_crash_boundaries_cleanup_or_recover_idempotently() -> TestResult {
        let staging = tempfile::tempdir()?;
        let receipt_path = staging.path().join("MIGRATED");
        let witness = ValidatedMigrationV2(Receipt {
            format_version: RECEIPT_FORMAT,
            journal_digest: format!("sha256:{}", "1".repeat(64)),
            batch_count: 0,
            sources: Vec::new(),
            durable: Vec::new(),
        });

        failpoint::set(Some(FAIL_AFTER_TEMP_SYNC));
        let before_publish = write_receipt(staging.path(), &receipt_path, &witness)
            .expect_err("temp-sync failure must surface before publication");
        failpoint::set(None);
        assert!(matches!(before_publish, CoreError::Storage(_)));
        assert!(!receipt_path.exists());
        assert!(!leaked_migration_temp(staging.path())?);

        failpoint::set(Some(FAIL_AFTER_RECEIPT_PUBLISH));
        let after_publish = write_receipt(staging.path(), &receipt_path, &witness)
            .expect_err("post-publication sync failure must surface");
        failpoint::set(None);
        assert!(matches!(after_publish, CoreError::Storage(_)));
        assert!(receipt_path.exists());
        write_receipt(staging.path(), &receipt_path, &witness)?;
        assert_eq!(
            read_receipt(staging.path(), &receipt_path)?,
            Some(witness.0)
        );
        Ok(())
    }

    #[test]
    fn receipt_shape_rejects_fabricated_legacy_or_invalid_root() -> TestResult {
        let staging = tempfile::tempdir()?;
        let receipt_path = staging.path().join("MIGRATED");
        fs::write(&receipt_path, b"migrated")?;
        let legacy = read_receipt(staging.path(), &receipt_path)
            .expect_err("v1 marker must not authorize migration");
        assert!(matches!(
            legacy,
            CoreError::Typed { ref code, .. }
                if *code == quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticMigrationReceiptUnsupported
        ));

        fs::write(
            &receipt_path,
            encode_receipt(&Receipt {
                format_version: RECEIPT_FORMAT,
                journal_digest: format!("sha256:{}", "1".repeat(64)),
                batch_count: 1,
                sources: vec![SourceRow {
                    repo_id: RepoId::new("repo")
                        .expect("static fixture ID satisfies canonical policy"),
                    revision_id: RevisionId::new("rev")
                        .expect("static fixture ID satisfies canonical policy"),
                    generation: ManifestGeneration::new(1),
                    manifest_digest: "manifest".to_string(),
                }],
                durable: vec![DurableRow {
                    repo_id: RepoId::new("repo")
                        .expect("static fixture ID satisfies canonical policy"),
                    revision_id: RevisionId::new("rev")
                        .expect("static fixture ID satisfies canonical policy"),
                    generation: ManifestGeneration::new(1),
                    manifest_digest: "manifest".to_string(),
                    semantic_row_root_digest: "sha256:not-a-root".to_string(),
                    row_count: 1,
                }],
            })?,
        )?;
        let invalid = read_receipt(staging.path(), &receipt_path)
            .expect_err("invalid durable root must fail closed");
        assert!(matches!(
            invalid,
            CoreError::Typed { ref code, .. }
                if *code == quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticMigrationReceiptCorrupt
        ));
        Ok(())
    }

    /// The read-only open creates nothing beside the journal it reads: no
    /// lock file, no receipt, no temporary, and the journal bytes are
    /// identical before and after.
    #[test]
    fn reader_open_creates_nothing_in_the_source() -> TestResult {
        let root = tempfile::tempdir()?;
        let journal_dir = root.path().join("semantic");
        LegacySemanticJournalReaderV1::stage_for_test(&journal_dir, &[])?;
        let before: Vec<(PathBuf, Vec<u8>)> = vec![(
            journal_dir.join("journal.cbor"),
            fs::read(journal_dir.join("journal.cbor"))?,
        )];

        let _reader = LegacySemanticJournalReaderV1::open(&journal_dir)?;
        assert!(
            !journal_dir.join("MIGRATED").exists(),
            "the reader must not write a receipt into the source"
        );
        assert!(
            !journal_dir.join("MIGRATED.lock").exists(),
            "the reader must not take a lock file in the source"
        );
        assert!(!leaked_migration_temp(&journal_dir)?);
        for (path, bytes) in &before {
            assert_eq!(
                &fs::read(path)?,
                bytes,
                "the source journal must read back byte-identical"
            );
        }
        Ok(())
    }

    /// Refuse a source-side `MIGRATED` receipt from an old binary.
    ///
    /// New binary, old root: this binary does not verify that authority,
    /// so the journal is refused
    /// immutable. A stale `MIGRATED.lock` beside it is inert residue and is
    /// ignored.
    #[test]
    fn reader_refuses_a_source_side_receipt_as_old_binary_authority() -> TestResult {
        let root = tempfile::tempdir()?;
        let journal_dir = root.path().join("semantic");
        LegacySemanticJournalReaderV1::stage_for_test(&journal_dir, &[])?;
        fs::write(journal_dir.join("MIGRATED.lock"), b"stale lock residue")?;

        let reader = LegacySemanticJournalReaderV1::open(&journal_dir)?;
        assert!(
            reader.journal_digest.is_none(),
            "an empty staged journal carries no digest"
        );

        fs::write(journal_dir.join("MIGRATED"), b"migrated")?;
        let error = LegacySemanticJournalReaderV1::open(&journal_dir)
            .expect_err("a source-side receipt must refuse the journal immutable");
        assert!(matches!(
            error,
            CoreError::Typed { ref code, .. }
                if *code == quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticJournalImmutableAfterMigration
        ));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn reader_rejects_symlinked_journal() -> TestResult {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir()?;
        let outside = tempfile::NamedTempFile::new()?;
        symlink(outside.path(), root.path().join("journal.cbor"))?;
        assert!(LegacySemanticJournalReaderV1::open(root.path()).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn reader_rejects_hard_linked_journal() -> TestResult {
        let root = tempfile::tempdir()?;
        let outside = tempfile::NamedTempFile::new()?;
        fs::hard_link(outside.path(), root.path().join("journal.cbor"))?;

        let error = LegacySemanticJournalReaderV1::open(root.path())
            .expect_err("multi-link journal must not cross the custody boundary");
        assert!(matches!(
            error,
            CoreError::Typed { ref code, .. }
                if *code == quanta_index_contract::SearchPlaneErrorCodeV2::LegacySemanticMigrationCustodyInvalid
        ));
        Ok(())
    }
}
