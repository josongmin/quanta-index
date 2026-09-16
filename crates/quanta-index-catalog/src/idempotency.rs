//! The ingest idempotency table (QI-BB-032).
//!
//! Schema (`WITHOUT ROWID`, keyed by the idempotency key):
//!
//! ```text
//! idempotency_v1(kind, repo_id, revision_id, generation, batch_digest,
//!                body_sha256 BLOB, applied INTEGER, receipt_cbor BLOB NULL,
//!                durable_sequence INTEGER NULL, row_sha256 BLOB)
//! catalog_sequence_v1(id = 1, next INTEGER)
//! ```
//!
//! `row_sha256` commits to every other column of the row and is verified on
//! every read. `durable_sequence` is allocated from `catalog_sequence_v1`
//! inside the finalizing transaction, so it is unique and monotonic across
//! every key in the catalog.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use quanta_index_contract::{BatchPublishReceipt, ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::{
    BATCH_DIGEST_CONFLICT_CODE, CATALOG_BUSY_CODE, CATALOG_ROW_CORRUPT_CODE, CoreError,
    IdempotencyBeginV1, IdempotencyCatalogPort, IdempotencyKeyV1,
};
use quanta_index_ipc::{decode_cbor_payload, encode_cbor_payload};
use rusqlite::{Connection, OpenFlags, OptionalExtension as _, TransactionBehavior, params};
use sha2::{Digest, Sha256};

/// The catalog database file, under `state_root/catalog/`.
pub const IDEMPOTENCY_CATALOG_FILE_NAME: &str = "catalog-v1.sqlite";
const ROW_DIGEST_DOMAIN: &[u8] = b"quanta-index:catalog:idempotency-row:v1\0";
const FIELD_SEPARATOR: &[u8] = b"\x1f";

/// The catalog directory under a state root.
#[must_use]
pub fn catalog_dir(state_root: &Path) -> PathBuf {
    state_root.join("catalog")
}

/// The idempotency catalog over one `SQLite` file.
///
/// One connection, serialized by a mutex: the ingest socket dispatches
/// serially, so a second connection would only add lock contention.
pub struct SqliteIdempotencyCatalog {
    connection: Mutex<Connection>,
    path: PathBuf,
}

impl std::fmt::Debug for SqliteIdempotencyCatalog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteIdempotencyCatalog")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

fn storage(action: &str, path: &Path, error: &dyn std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("catalog: {action} {}: {error}", path.display()))
}

/// Map an engine error to the typed codes the port promises.
fn engine_error(action: &str, path: &Path, error: &rusqlite::Error) -> CoreError {
    if let rusqlite::Error::SqliteFailure(failure, _) = error
        && matches!(
            failure.code,
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
        )
    {
        return CoreError::Typed {
            code: CATALOG_BUSY_CODE.to_string(),
            message: format!(
                "catalog: {action} {} met a held lock past the busy budget: {error}",
                path.display()
            ),
        };
    }
    storage(action, path, error)
}

impl SqliteIdempotencyCatalog {
    /// Open (creating if needed) the catalog under `state_root/catalog/`.
    ///
    /// `busy_timeout` is how long a write waits on a held lock before it is
    /// answered typed; the caller maps it from its own deadline.
    pub fn open(state_root: &Path, busy_timeout: Duration) -> Result<Self, CoreError> {
        let directory = catalog_dir(state_root);
        std::fs::create_dir_all(&directory)
            .map_err(|error| storage("create directory", &directory, &error))?;
        let path = directory.join(IDEMPOTENCY_CATALOG_FILE_NAME);
        let connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )
        .map_err(|error| engine_error("open", &path, &error))?;
        // Each pragma is read back rather than assumed: `journal_mode` is a
        // request the engine may decline, and a catalog that silently ran
        // under `NORMAL` would ack work the engine can forget (G0-C).
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|error| engine_error("set journal_mode", &path, &error))?;
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(|error| engine_error("set synchronous", &path, &error))?;
        connection
            .pragma_update(None, "fullfsync", "ON")
            .map_err(|error| engine_error("set fullfsync", &path, &error))?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(|error| engine_error("set foreign_keys", &path, &error))?;
        connection
            .busy_timeout(busy_timeout)
            .map_err(|error| engine_error("set busy_timeout", &path, &error))?;
        let journal_mode: String = connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .map_err(|error| engine_error("read journal_mode", &path, &error))?;
        let synchronous: i64 = connection
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .map_err(|error| engine_error("read synchronous", &path, &error))?;
        if !journal_mode.eq_ignore_ascii_case("wal") || synchronous != 2 {
            return Err(CoreError::Storage(format!(
                "catalog: {} runs journal_mode={journal_mode} synchronous={synchronous}; the catalog requires wal/FULL",
                path.display()
            )));
        }
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS idempotency_v1 (
                     kind TEXT NOT NULL,
                     repo_id TEXT NOT NULL,
                     revision_id TEXT NOT NULL,
                     generation INTEGER NOT NULL,
                     batch_digest TEXT NOT NULL,
                     body_sha256 BLOB NOT NULL,
                     applied INTEGER NOT NULL,
                     receipt_cbor BLOB,
                     durable_sequence INTEGER,
                     row_sha256 BLOB NOT NULL,
                     PRIMARY KEY (kind, repo_id, revision_id, generation, batch_digest)
                 ) WITHOUT ROWID;
                 CREATE INDEX IF NOT EXISTS idempotency_v1_by_generation
                     ON idempotency_v1 (repo_id, revision_id, generation);
                 CREATE TABLE IF NOT EXISTS catalog_sequence_v1 (
                     id INTEGER PRIMARY KEY CHECK (id = 1),
                     next INTEGER NOT NULL
                 );
                 INSERT OR IGNORE INTO catalog_sequence_v1 (id, next) VALUES (1, 1);",
            )
            .map_err(|error| engine_error("create schema", &path, &error))?;
        Ok(Self {
            connection: Mutex::new(connection),
            path,
        })
    }

    /// The database file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, CoreError> {
        self.connection.lock().map_err(|error| {
            CoreError::Storage(format!(
                "catalog: connection to {} poisoned: {error}",
                self.path.display()
            ))
        })
    }
}

/// One stored row, as read back and verified.
struct StoredRow {
    body_sha256: [u8; 32],
    applied: bool,
    receipt_cbor: Option<Vec<u8>>,
    durable_sequence: Option<u64>,
}

fn generation_i64(generation: ManifestGeneration) -> Result<i64, CoreError> {
    i64::try_from(generation.get()).map_err(|error| {
        CoreError::InvalidContract(format!(
            "catalog: generation {} does not fit the catalog's integer column: {error}",
            generation.get()
        ))
    })
}

fn sequence_u64(sequence: i64) -> Result<u64, CoreError> {
    u64::try_from(sequence).map_err(|error| {
        CoreError::Storage(format!(
            "catalog: durable sequence {sequence} is negative: {error}"
        ))
    })
}

/// The digest every row commits to: the key, the body hash, the state, the
/// receipt bytes and the sequence, in a fixed order with separators.
fn row_digest(
    key: &IdempotencyKeyV1,
    body_sha256: &[u8; 32],
    applied: bool,
    receipt_cbor: Option<&[u8]>,
    durable_sequence: Option<u64>,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(ROW_DIGEST_DOMAIN);
    hasher.update(key.kind.as_code_str().as_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(key.repo_id.as_str().as_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(key.revision_id.as_str().as_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(key.generation.get().to_le_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(key.batch_digest.as_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(body_sha256);
    hasher.update(FIELD_SEPARATOR);
    hasher.update([u8::from(applied)]);
    hasher.update(FIELD_SEPARATOR);
    match receipt_cbor {
        Some(bytes) => {
            hasher.update([1_u8]);
            hasher.update(bytes);
        }
        None => hasher.update([0_u8]),
    }
    hasher.update(FIELD_SEPARATOR);
    match durable_sequence {
        Some(sequence) => {
            hasher.update([1_u8]);
            hasher.update(sequence.to_le_bytes());
        }
        None => hasher.update([0_u8]),
    }
    hasher.finalize().into()
}

fn blob32(label: &str, bytes: &[u8]) -> Result<[u8; 32], CoreError> {
    <[u8; 32]>::try_from(bytes).map_err(|_wrong_length| CoreError::Typed {
        code: CATALOG_ROW_CORRUPT_CODE.to_string(),
        message: format!("catalog: {label} is {} bytes, expected 32", bytes.len()),
    })
}

/// Read one row inside `connection`'s current transaction and verify its
/// digest.
fn read_row(
    connection: &Connection,
    path: &Path,
    key: &IdempotencyKeyV1,
) -> Result<Option<StoredRow>, CoreError> {
    let generation = generation_i64(key.generation)?;
    let row = connection
        .query_row(
            "SELECT body_sha256, applied, receipt_cbor, durable_sequence, row_sha256
             FROM idempotency_v1
             WHERE kind = ?1 AND repo_id = ?2 AND revision_id = ?3
               AND generation = ?4 AND batch_digest = ?5",
            params![
                key.kind.as_code_str(),
                key.repo_id.as_str(),
                key.revision_id.as_str(),
                generation,
                key.batch_digest.as_str(),
            ],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(|error| engine_error("read record", path, &error))?;
    let Some((body, applied, receipt_cbor, durable_sequence, row_sha256)) = row else {
        return Ok(None);
    };
    let body_sha256 = blob32("body digest", &body)?;
    let stored_digest = blob32("row digest", &row_sha256)?;
    let applied = match applied {
        0 => false,
        1 => true,
        other => {
            return Err(CoreError::Typed {
                code: CATALOG_ROW_CORRUPT_CODE.to_string(),
                message: format!("catalog: applied flag is {other}, expected 0 or 1"),
            });
        }
    };
    let durable_sequence = durable_sequence.map(sequence_u64).transpose()?;
    let expected = row_digest(
        key,
        &body_sha256,
        applied,
        receipt_cbor.as_deref(),
        durable_sequence,
    );
    if expected != stored_digest {
        return Err(CoreError::Typed {
            code: CATALOG_ROW_CORRUPT_CODE.to_string(),
            message: format!(
                "catalog: idempotency row for {} repo={} revision={} generation={} batch_digest={} does not match its own digest",
                key.kind,
                key.repo_id.as_str(),
                key.revision_id.as_str(),
                key.generation.get(),
                key.batch_digest
            ),
        });
    }
    Ok(Some(StoredRow {
        body_sha256,
        applied,
        receipt_cbor,
        durable_sequence,
    }))
}

fn conflict(key: &IdempotencyKeyV1) -> CoreError {
    CoreError::Typed {
        code: BATCH_DIGEST_CONFLICT_CODE.to_string(),
        message: format!(
            "{} batch_digest={} for repo={} revision={} generation={} was already published with a different body; a batch digest names one immutable body",
            key.kind,
            key.batch_digest,
            key.repo_id.as_str(),
            key.revision_id.as_str(),
            key.generation.get()
        ),
    }
}

impl IdempotencyCatalogPort for SqliteIdempotencyCatalog {
    fn begin(
        &self,
        key: &IdempotencyKeyV1,
        body_sha256: &[u8; 32],
    ) -> Result<IdempotencyBeginV1, CoreError> {
        let mut connection = self.lock()?;
        let path = self.path.clone();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| engine_error("begin transaction", &path, &error))?;
        let outcome = match read_row(&transaction, &path, key)? {
            None => {
                let digest = row_digest(key, body_sha256, false, None, None);
                let _inserted = transaction
                    .execute(
                        "INSERT INTO idempotency_v1
                             (kind, repo_id, revision_id, generation, batch_digest,
                              body_sha256, applied, receipt_cbor, durable_sequence, row_sha256)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, NULL, NULL, ?7)",
                        params![
                            key.kind.as_code_str(),
                            key.repo_id.as_str(),
                            key.revision_id.as_str(),
                            generation_i64(key.generation)?,
                            key.batch_digest.as_str(),
                            body_sha256.as_slice(),
                            digest.as_slice(),
                        ],
                    )
                    .map_err(|error| engine_error("insert record", &path, &error))?;
                IdempotencyBeginV1::Fresh
            }
            Some(stored) if stored.body_sha256 != *body_sha256 => return Err(conflict(key)),
            Some(StoredRow {
                applied: true,
                receipt_cbor: Some(receipt_cbor),
                durable_sequence: Some(durable_sequence),
                ..
            }) => {
                let receipt: BatchPublishReceipt =
                    decode_cbor_payload(&receipt_cbor).map_err(|error| CoreError::Typed {
                        code: CATALOG_ROW_CORRUPT_CODE.to_string(),
                        message: format!("catalog: stored receipt does not decode: {error}"),
                    })?;
                IdempotencyBeginV1::Replay {
                    receipt,
                    durable_sequence,
                }
            }
            Some(StoredRow { applied: false, .. }) => IdempotencyBeginV1::Resume,
            Some(StoredRow { applied: true, .. }) => {
                return Err(CoreError::Typed {
                    code: CATALOG_ROW_CORRUPT_CODE.to_string(),
                    message: "catalog: an applied record has no receipt or sequence".to_string(),
                });
            }
        };
        transaction
            .commit()
            .map_err(|error| engine_error("commit begin", &path, &error))?;
        drop(connection);
        Ok(outcome)
    }

    fn finalize(
        &self,
        key: &IdempotencyKeyV1,
        body_sha256: &[u8; 32],
        receipt: &BatchPublishReceipt,
    ) -> Result<u64, CoreError> {
        let mut connection = self.lock()?;
        let path = self.path.clone();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| engine_error("begin transaction", &path, &error))?;
        let Some(stored) = read_row(&transaction, &path, key)? else {
            return Err(CoreError::InvalidContract(format!(
                "catalog: finalize for {} batch_digest={} was never begun",
                key.kind, key.batch_digest
            )));
        };
        if stored.body_sha256 != *body_sha256 {
            return Err(conflict(key));
        }
        if let (true, Some(sequence)) = (stored.applied, stored.durable_sequence) {
            // Finalizing an applied record again is a caller defect: the
            // record is immutable once applied.
            return Err(CoreError::InvalidContract(format!(
                "catalog: {} batch_digest={} was already finalized at sequence {sequence}",
                key.kind, key.batch_digest
            )));
        }
        let sequence: i64 = transaction
            .query_row(
                "UPDATE catalog_sequence_v1 SET next = next + 1 WHERE id = 1 RETURNING next - 1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| engine_error("allocate sequence", &path, &error))?;
        let durable_sequence = sequence_u64(sequence)?;
        let receipt_cbor = encode_cbor_payload(receipt)
            .map_err(|error| CoreError::Storage(format!("catalog: encode receipt: {error}")))?;
        let digest = row_digest(
            key,
            body_sha256,
            true,
            Some(&receipt_cbor),
            Some(durable_sequence),
        );
        let updated = transaction
            .execute(
                "UPDATE idempotency_v1
                 SET applied = 1, receipt_cbor = ?6, durable_sequence = ?7, row_sha256 = ?8
                 WHERE kind = ?1 AND repo_id = ?2 AND revision_id = ?3
                   AND generation = ?4 AND batch_digest = ?5",
                params![
                    key.kind.as_code_str(),
                    key.repo_id.as_str(),
                    key.revision_id.as_str(),
                    generation_i64(key.generation)?,
                    key.batch_digest.as_str(),
                    receipt_cbor.as_slice(),
                    sequence,
                    digest.as_slice(),
                ],
            )
            .map_err(|error| engine_error("finalize record", &path, &error))?;
        if updated != 1 {
            return Err(CoreError::Storage(format!(
                "catalog: finalize updated {updated} rows, expected exactly one"
            )));
        }
        transaction
            .commit()
            .map_err(|error| engine_error("commit finalize", &path, &error))?;
        drop(connection);
        Ok(durable_sequence)
    }

    fn forget_generation(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<u64, CoreError> {
        let connection = self.lock()?;
        let removed = connection
            .execute(
                "DELETE FROM idempotency_v1
                 WHERE repo_id = ?1 AND revision_id = ?2 AND generation = ?3",
                params![
                    repo_id.as_str(),
                    revision_id.as_str(),
                    generation_i64(generation)?
                ],
            )
            .map_err(|error| engine_error("forget generation", &self.path, &error))?;
        drop(connection);
        u64::try_from(removed).map_err(|error| {
            CoreError::Storage(format!("catalog: removed-row count overflow: {error}"))
        })
    }
}
