//! The auxiliary authority row tables (QI-BB-020).
//!
//! Schema (both `WITHOUT ROWID`):
//!
//! ```text
//! auxiliary_rows_v1(domain, repo_id, revision_id, generation, family,
//!                   row_key BLOB, value BLOB, row_sha256 BLOB)
//! auxiliary_tracks_v1(repo_id, revision_id, track, value BLOB, row_sha256 BLOB)
//! ```
//!
//! `row_sha256` commits to every other column and is verified on every
//! read. A mutation batch is applied inside one `IMMEDIATE` transaction:
//! every row mutation in order, then every track row, then commit under
//! `synchronous=FULL` — so a receipt means every row is durable and a
//! failure means none is.

use std::path::Path;

use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind};
use quanta_index_core::{
    AuxiliaryAuthorityCatalogPort, AuxiliaryDomainV1, AuxiliaryGenerationKeyV1,
    AuxiliaryMutationBatchV1, AuxiliaryMutationReceiptV1, AuxiliaryRowFamilyV1, AuxiliaryRowKeyV1,
    AuxiliaryRowMutationV1, AuxiliaryRowV1, AuxiliaryTrackRowV1, CoreError,
};
use rusqlite::{Connection, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use crate::connection::{SqliteCatalog, blob32, count_u64, engine_error, generation_i64};

const ROW_DIGEST_DOMAIN: &[u8] = b"quanta-index:catalog:auxiliary-row:v1\0";
const TRACK_DIGEST_DOMAIN: &[u8] = b"quanta-index:catalog:auxiliary-track:v1\0";
const FIELD_SEPARATOR: &[u8] = b"\x1f";

/// The tables, created at open.
pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS auxiliary_rows_v1 (
         domain TEXT NOT NULL,
         repo_id TEXT NOT NULL,
         revision_id TEXT NOT NULL,
         generation INTEGER NOT NULL,
         family TEXT NOT NULL,
         row_key BLOB NOT NULL,
         value BLOB NOT NULL,
         row_sha256 BLOB NOT NULL,
         PRIMARY KEY (domain, repo_id, revision_id, generation, family, row_key)
     ) WITHOUT ROWID;
     CREATE INDEX IF NOT EXISTS auxiliary_rows_v1_by_generation
         ON auxiliary_rows_v1 (repo_id, revision_id, generation);
     CREATE TABLE IF NOT EXISTS auxiliary_tracks_v1 (
         repo_id TEXT NOT NULL,
         revision_id TEXT NOT NULL,
         track TEXT NOT NULL,
         value BLOB NOT NULL,
         row_sha256 BLOB NOT NULL,
         PRIMARY KEY (repo_id, revision_id, track)
     ) WITHOUT ROWID;";

/// The digest every row commits to: its full address and its value.
fn row_digest(key: &AuxiliaryRowKeyV1, value: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(ROW_DIGEST_DOMAIN);
    hasher.update(key.domain.as_code_str().as_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(key.generation.repo_id.as_str().as_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(key.generation.revision_id.as_str().as_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(key.generation.generation.get().to_le_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(key.family.as_code_str().as_bytes());
    hasher.update(FIELD_SEPARATOR);
    // The key length is committed so a key/value boundary shift cannot
    // produce the same digest; a length past `u64` cannot occur in a row
    // the engine stored.
    let key_len = u64::try_from(key.row_key.len()).map_or(u64::MAX, |len| len);
    hasher.update(key_len.to_le_bytes());
    hasher.update(&key.row_key);
    hasher.update(FIELD_SEPARATOR);
    hasher.update(value);
    hasher.finalize().into()
}

fn track_digest(row: &AuxiliaryTrackRowV1) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(TRACK_DIGEST_DOMAIN);
    hasher.update(row.repo_id.as_str().as_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(row.revision_id.as_str().as_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(row.track.as_code_str().as_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(&row.value);
    hasher.finalize().into()
}

fn corrupt(message: String) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
        message,
    }
}

fn track_from_code(code: &str) -> Result<SearchPlaneTrackKind, CoreError> {
    match code {
        "Lexical" => Ok(SearchPlaneTrackKind::Lexical),
        "Semantic" => Ok(SearchPlaneTrackKind::Semantic),
        "Structural" => Ok(SearchPlaneTrackKind::Structural),
        other => {
            Err(corrupt(format!("catalog: auxiliary track row names unknown track {other:?}")))
        }
    }
}

fn upsert_row(connection: &Connection, path: &Path, row: &AuxiliaryRowV1) -> Result<(), CoreError> {
    let digest = row_digest(&row.key, &row.value);
    let _written = connection
        .execute(
            "INSERT OR REPLACE INTO auxiliary_rows_v1
                 (domain, repo_id, revision_id, generation, family, row_key, value, row_sha256)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                row.key.domain.as_code_str(),
                row.key.generation.repo_id.as_str(),
                row.key.generation.revision_id.as_str(),
                generation_i64(row.key.generation.generation)?,
                row.key.family.as_code_str(),
                row.key.row_key.as_slice(),
                row.value.as_slice(),
                digest.as_slice(),
            ],
        )
        .map_err(|error| engine_error("upsert auxiliary row", path, &error))?;
    Ok(())
}

fn delete_row(
    connection: &Connection,
    path: &Path,
    key: &AuxiliaryRowKeyV1,
) -> Result<usize, CoreError> {
    connection
        .execute(
            "DELETE FROM auxiliary_rows_v1
             WHERE domain = ?1 AND repo_id = ?2 AND revision_id = ?3
               AND generation = ?4 AND family = ?5 AND row_key = ?6",
            params![
                key.domain.as_code_str(),
                key.generation.repo_id.as_str(),
                key.generation.revision_id.as_str(),
                generation_i64(key.generation.generation)?,
                key.family.as_code_str(),
                key.row_key.as_slice(),
            ],
        )
        .map_err(|error| engine_error("delete auxiliary row", path, &error))
}

fn clear_family(
    connection: &Connection,
    path: &Path,
    domain: AuxiliaryDomainV1,
    generation: &AuxiliaryGenerationKeyV1,
    family: AuxiliaryRowFamilyV1,
) -> Result<usize, CoreError> {
    connection
        .execute(
            "DELETE FROM auxiliary_rows_v1
             WHERE domain = ?1 AND repo_id = ?2 AND revision_id = ?3
               AND generation = ?4 AND family = ?5",
            params![
                domain.as_code_str(),
                generation.repo_id.as_str(),
                generation.revision_id.as_str(),
                generation_i64(generation.generation)?,
                family.as_code_str(),
            ],
        )
        .map_err(|error| engine_error("clear auxiliary family", path, &error))
}

fn forget_generation(
    connection: &Connection,
    path: &Path,
    generation: &AuxiliaryGenerationKeyV1,
) -> Result<usize, CoreError> {
    connection
        .execute(
            "DELETE FROM auxiliary_rows_v1
             WHERE repo_id = ?1 AND revision_id = ?2 AND generation = ?3",
            params![
                generation.repo_id.as_str(),
                generation.revision_id.as_str(),
                generation_i64(generation.generation)?,
            ],
        )
        .map_err(|error| engine_error("forget auxiliary generation", path, &error))
}

fn upsert_track(
    connection: &Connection,
    path: &Path,
    row: &AuxiliaryTrackRowV1,
) -> Result<(), CoreError> {
    let digest = track_digest(row);
    let _written = connection
        .execute(
            "INSERT OR REPLACE INTO auxiliary_tracks_v1
                 (repo_id, revision_id, track, value, row_sha256)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                row.repo_id.as_str(),
                row.revision_id.as_str(),
                row.track.as_code_str(),
                row.value.as_slice(),
                digest.as_slice(),
            ],
        )
        .map_err(|error| engine_error("upsert auxiliary track row", path, &error))?;
    Ok(())
}

/// One raw row as the engine returns it, before its digest is verified.
struct RawRow {
    domain: String,
    repo_id: String,
    revision_id: String,
    generation: i64,
    family: String,
    row_key: Vec<u8>,
    value: Vec<u8>,
    row_sha256: Vec<u8>,
}

fn verified_row(raw: RawRow) -> Result<AuxiliaryRowV1, CoreError> {
    let domain = AuxiliaryDomainV1::from_code_str(&raw.domain).ok_or_else(|| {
        corrupt(format!("catalog: auxiliary row names unknown domain {:?}", raw.domain))
    })?;
    let family = AuxiliaryRowFamilyV1::from_code_str(&raw.family).ok_or_else(|| {
        corrupt(format!("catalog: auxiliary row names unknown family {:?}", raw.family))
    })?;
    let generation = u64::try_from(raw.generation).map_err(|error| {
        corrupt(format!("catalog: auxiliary row generation is negative: {error}"))
    })?;
    let key = AuxiliaryRowKeyV1 {
        domain,
        generation: AuxiliaryGenerationKeyV1 {
            repo_id: RepoId::new(raw.repo_id)
                .map_err(|error| corrupt(format!("catalog: auxiliary row repo ID: {error}")))?,
            revision_id: RevisionId::new(raw.revision_id)
                .map_err(|error| corrupt(format!("catalog: auxiliary row revision ID: {error}")))?,
            generation: ManifestGeneration::new(generation),
        },
        family,
        row_key: raw.row_key,
    };
    let stored = blob32("auxiliary row digest", &raw.row_sha256)?;
    if row_digest(&key, &raw.value) != stored {
        return Err(corrupt(format!(
            "catalog: auxiliary row {domain}/{family} for repo={} revision={} generation={} does not match its own digest",
            key.generation.repo_id.as_str(),
            key.generation.revision_id.as_str(),
            key.generation.generation.get()
        )));
    }
    Ok(AuxiliaryRowV1 {
        key,
        value: raw.value,
    })
}

/// Visit every row of the row table, verified, in key order.
fn scan_rows(
    connection: &Connection,
    path: &Path,
    visit: &mut dyn FnMut(AuxiliaryRowV1) -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    let mut statement = connection
        .prepare(
            "SELECT domain, repo_id, revision_id, generation, family, row_key, value, row_sha256
             FROM auxiliary_rows_v1
             ORDER BY domain, repo_id, revision_id, generation, family, row_key",
        )
        .map_err(|error| engine_error("prepare auxiliary scan", path, &error))?;
    let rows = statement
        .query_map([], |row| {
            Ok(RawRow {
                domain: row.get(0)?,
                repo_id: row.get(1)?,
                revision_id: row.get(2)?,
                generation: row.get(3)?,
                family: row.get(4)?,
                row_key: row.get(5)?,
                value: row.get(6)?,
                row_sha256: row.get(7)?,
            })
        })
        .map_err(|error| engine_error("scan auxiliary rows", path, &error))?;
    for raw in rows {
        let raw = raw.map_err(|error| engine_error("read auxiliary row", path, &error))?;
        visit(verified_row(raw)?)?;
    }
    Ok(())
}

/// Every track row, verified.
fn scan_tracks(
    connection: &Connection,
    path: &Path,
) -> Result<Vec<AuxiliaryTrackRowV1>, CoreError> {
    let mut statement = connection
        .prepare(
            "SELECT repo_id, revision_id, track, value, row_sha256
             FROM auxiliary_tracks_v1
             ORDER BY repo_id, revision_id, track",
        )
        .map_err(|error| engine_error("prepare auxiliary track scan", path, &error))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, Vec<u8>>(4)?,
            ))
        })
        .map_err(|error| engine_error("scan auxiliary track rows", path, &error))?;
    let mut out = Vec::new();
    for raw in rows {
        let (repo_id, revision_id, track, value, row_sha256) =
            raw.map_err(|error| engine_error("read auxiliary track row", path, &error))?;
        let row = AuxiliaryTrackRowV1 {
            repo_id: RepoId::new(repo_id)
                .map_err(|error| corrupt(format!("catalog: auxiliary track repo ID: {error}")))?,
            revision_id: RevisionId::new(revision_id).map_err(|error| {
                corrupt(format!("catalog: auxiliary track revision ID: {error}"))
            })?,
            track: track_from_code(&track)?,
            value,
        };
        let stored = blob32("auxiliary track row digest", &row_sha256)?;
        if track_digest(&row) != stored {
            return Err(corrupt(format!(
                "catalog: auxiliary track row for repo={} revision={} track={} does not match its own digest",
                row.repo_id.as_str(),
                row.revision_id.as_str(),
                row.track.as_code_str()
            )));
        }
        out.push(row);
    }
    Ok(out)
}

impl AuxiliaryAuthorityCatalogPort for SqliteCatalog {
    fn apply(
        &self,
        batch: &AuxiliaryMutationBatchV1,
    ) -> Result<AuxiliaryMutationReceiptV1, CoreError> {
        let mut connection = self.lock()?;
        let path = self.path.clone();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| engine_error("begin auxiliary transaction", &path, &error))?;
        let mut written = 0_usize;
        let mut deleted = 0_usize;
        for mutation in &batch.rows {
            match mutation {
                AuxiliaryRowMutationV1::Upsert(row) => {
                    upsert_row(&transaction, &path, row)?;
                    written = written.saturating_add(1);
                }
                AuxiliaryRowMutationV1::Delete(key) => {
                    deleted = deleted.saturating_add(delete_row(&transaction, &path, key)?);
                }
                AuxiliaryRowMutationV1::ClearFamily {
                    domain,
                    generation,
                    family,
                } => {
                    deleted = deleted.saturating_add(clear_family(
                        &transaction,
                        &path,
                        *domain,
                        generation,
                        *family,
                    )?);
                }
                AuxiliaryRowMutationV1::ForgetGeneration(generation) => {
                    deleted =
                        deleted.saturating_add(forget_generation(&transaction, &path, generation)?);
                }
            }
        }
        for track in &batch.tracks {
            upsert_track(&transaction, &path, track)?;
            written = written.saturating_add(1);
        }
        transaction
            .commit()
            .map_err(|error| engine_error("commit auxiliary transaction", &path, &error))?;
        drop(connection);
        Ok(AuxiliaryMutationReceiptV1 {
            rows_written: count_u64("written-row", written)?,
            rows_deleted: count_u64("deleted-row", deleted)?,
        })
    }

    fn for_each_row(
        &self,
        visit: &mut dyn FnMut(AuxiliaryRowV1) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        let connection = self.lock()?;
        let outcome = scan_rows(&connection, &self.path, visit);
        drop(connection);
        outcome
    }

    fn track_rows(&self) -> Result<Vec<AuxiliaryTrackRowV1>, CoreError> {
        let connection = self.lock()?;
        let outcome = scan_tracks(&connection, &self.path);
        drop(connection);
        outcome
    }
}
