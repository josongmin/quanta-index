//! QI-BB-020 — the auxiliary authority row tables, against the real engine.
//!
//! 1. A batch of upserts, deletes and family clears is applied in order and
//!    read back verified, in key order, with a receipt that counts exactly
//!    what the engine wrote and removed.
//! 2. A batch is one transaction: a mutation that cannot be written leaves
//!    every earlier mutation of the same batch unwritten.
//! 3. A one-row mutation over a large generation writes one row and leaves
//!    every other row byte-identical.
//! 4. A row whose stored bytes no longer match its digest is a typed
//!    `CATALOG_ROW_CORRUPT` on scan, never served.
//! 5. A generation forget removes exactly that generation's rows, across
//!    domains, in the same transaction as the batch's other rows, leaves
//!    track rows alone, and in a batch that cannot be written removes
//!    nothing.
//! 6. Track rows round-trip and are verified.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::time::Duration;

use quanta_index_catalog::{CATALOG_FILE_NAME, SqliteCatalog, catalog_dir};
use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind};
use quanta_index_core::{
    AuxiliaryAuthorityCatalogPort, AuxiliaryDomainV1, AuxiliaryGenerationKeyV1,
    AuxiliaryMutationBatchV1, AuxiliaryMutationReceiptV1, AuxiliaryRowFamilyV1, AuxiliaryRowKeyV1,
    AuxiliaryRowMutationV1, AuxiliaryRowV1, AuxiliaryTrackRowV1, CATALOG_ROW_CORRUPT_CODE,
    CoreError,
};

type TestResult = Result<(), Box<dyn Error>>;

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn generation(generation: u64) -> AuxiliaryGenerationKeyV1 {
    AuxiliaryGenerationKeyV1 {
        repo_id: RepoId::new("repo-aux").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev-aux")
            .expect("static fixture ID satisfies canonical policy"),
        generation: ManifestGeneration::new(generation),
    }
}

fn key(
    domain: AuxiliaryDomainV1,
    generation_number: u64,
    family: AuxiliaryRowFamilyV1,
    row_key: &str,
) -> AuxiliaryRowKeyV1 {
    AuxiliaryRowKeyV1 {
        domain,
        generation: generation(generation_number),
        family,
        row_key: row_key.as_bytes().to_vec(),
    }
}

fn row(
    domain: AuxiliaryDomainV1,
    generation_number: u64,
    family: AuxiliaryRowFamilyV1,
    row_key: &str,
    value: &str,
) -> AuxiliaryRowV1 {
    AuxiliaryRowV1 {
        key: key(domain, generation_number, family, row_key),
        value: value.as_bytes().to_vec(),
    }
}

/// A row under a generation past the engine's integer column: any batch
/// carrying it is refused.
fn unwritable_row() -> AuxiliaryRowV1 {
    AuxiliaryRowV1 {
        key: AuxiliaryRowKeyV1 {
            domain: AuxiliaryDomainV1::History,
            generation: generation(u64::MAX),
            family: AuxiliaryRowFamilyV1::Commit,
            row_key: b"z".to_vec(),
        },
        value: b"never".to_vec(),
    }
}

fn open(root: &std::path::Path) -> Result<SqliteCatalog, CoreError> {
    SqliteCatalog::open(root, Duration::from_millis(100))
}

fn all_rows(catalog: &SqliteCatalog) -> Result<Vec<AuxiliaryRowV1>, CoreError> {
    let mut rows = Vec::new();
    catalog.for_each_row(&mut |row| {
        rows.push(row);
        Ok(())
    })?;
    Ok(rows)
}

fn typed_code(error: &CoreError) -> Option<quanta_index_contract::SearchPlaneErrorCodeV2> {
    match error {
        CoreError::Typed { code, .. } => Some(*code),
        CoreError::InvalidContract(_)
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_) => None,
    }
}

#[test]
fn a_batch_applies_in_order_and_reads_back_verified_in_key_order() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = open(temp.path())?;
    let first = catalog.apply(&AuxiliaryMutationBatchV1 {
        rows: vec![
            AuxiliaryRowMutationV1::Upsert(row(
                AuxiliaryDomainV1::History,
                1,
                AuxiliaryRowFamilyV1::Commit,
                "b",
                "commit-b",
            )),
            AuxiliaryRowMutationV1::Upsert(row(
                AuxiliaryDomainV1::History,
                1,
                AuxiliaryRowFamilyV1::Commit,
                "a",
                "commit-a",
            )),
            AuxiliaryRowMutationV1::Upsert(row(
                AuxiliaryDomainV1::Runtime,
                1,
                AuxiliaryRowFamilyV1::DirtyDoc,
                "doc-1",
                "dirty-1",
            )),
            AuxiliaryRowMutationV1::Upsert(row(
                AuxiliaryDomainV1::Runtime,
                1,
                AuxiliaryRowFamilyV1::DirtyDoc,
                "doc-2",
                "dirty-2",
            )),
        ],
        tracks: Vec::new(),
    })?;
    if (first.rows_written, first.rows_deleted) != (4, 0) {
        return Err(format!("first receipt drifted: {first:?}").into());
    }
    // A replace of one commit, a delete of one dirty doc, and a clear of the
    // dirty family (which removes the remaining one) in the same batch.
    let second = catalog.apply(&AuxiliaryMutationBatchV1 {
        rows: vec![
            AuxiliaryRowMutationV1::Upsert(row(
                AuxiliaryDomainV1::History,
                1,
                AuxiliaryRowFamilyV1::Commit,
                "a",
                "commit-a-v2",
            )),
            AuxiliaryRowMutationV1::Delete(key(
                AuxiliaryDomainV1::Runtime,
                1,
                AuxiliaryRowFamilyV1::DirtyDoc,
                "doc-1",
            )),
            AuxiliaryRowMutationV1::Delete(key(
                AuxiliaryDomainV1::Runtime,
                1,
                AuxiliaryRowFamilyV1::DirtyDoc,
                "absent",
            )),
            AuxiliaryRowMutationV1::ClearFamily {
                domain: AuxiliaryDomainV1::Runtime,
                generation: generation(1),
                family: AuxiliaryRowFamilyV1::DirtyDoc,
            },
        ],
        tracks: Vec::new(),
    })?;
    if (second.rows_written, second.rows_deleted) != (1, 2) {
        return Err(format!("second receipt drifted: {second:?}").into());
    }
    let rows = all_rows(&catalog)?;
    let expected = vec![
        row(
            AuxiliaryDomainV1::History,
            1,
            AuxiliaryRowFamilyV1::Commit,
            "a",
            "commit-a-v2",
        ),
        row(
            AuxiliaryDomainV1::History,
            1,
            AuxiliaryRowFamilyV1::Commit,
            "b",
            "commit-b",
        ),
    ];
    if rows != expected {
        return Err(format!("stored rows drifted: {rows:?}").into());
    }
    Ok(())
}

/// A generation past the engine's integer column cannot be written; the
/// upserts before it in the same batch must not survive.
#[test]
fn a_batch_that_cannot_be_written_writes_nothing() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = open(temp.path())?;
    let refused = catalog
        .apply(&AuxiliaryMutationBatchV1 {
            rows: vec![
                AuxiliaryRowMutationV1::Upsert(row(
                    AuxiliaryDomainV1::History,
                    1,
                    AuxiliaryRowFamilyV1::Commit,
                    "a",
                    "commit-a",
                )),
                AuxiliaryRowMutationV1::Upsert(unwritable_row()),
            ],
            tracks: Vec::new(),
        })
        .expect_err("a generation past the column must be refused");
    if !matches!(refused, CoreError::InvalidContract(_)) {
        return Err(format!("expected an invalid-contract refusal, got {refused:?}").into());
    }
    if !all_rows(&catalog)?.is_empty() {
        return Err("a refused batch must leave no row behind".into());
    }
    // The catalog is still usable afterwards.
    let _receipt = catalog.apply(&AuxiliaryMutationBatchV1 {
        rows: vec![AuxiliaryRowMutationV1::Upsert(row(
            AuxiliaryDomainV1::History,
            1,
            AuxiliaryRowFamilyV1::Commit,
            "a",
            "commit-a",
        ))],
        tracks: Vec::new(),
    })?;
    if all_rows(&catalog)?.len() != 1 {
        return Err("the next batch must apply".into());
    }
    Ok(())
}

#[test]
fn a_one_row_mutation_over_a_large_generation_writes_one_row() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = open(temp.path())?;
    let bulk = (0..1_000)
        .map(|index| {
            AuxiliaryRowMutationV1::Upsert(row(
                AuxiliaryDomainV1::Runtime,
                3,
                AuxiliaryRowFamilyV1::DirtyDoc,
                &format!("doc-{index:04}"),
                &format!("payload-{index}"),
            ))
        })
        .collect();
    let seeded = catalog.apply(&AuxiliaryMutationBatchV1 {
        rows: bulk,
        tracks: Vec::new(),
    })?;
    if seeded.rows_written != 1_000 {
        return Err(format!("seed receipt drifted: {seeded:?}").into());
    }
    let before: BTreeMap<Vec<u8>, Vec<u8>> = all_rows(&catalog)?
        .into_iter()
        .map(|row| (row.key.row_key, row.value))
        .collect();

    let one = catalog.apply(&AuxiliaryMutationBatchV1 {
        rows: vec![AuxiliaryRowMutationV1::Upsert(row(
            AuxiliaryDomainV1::Runtime,
            3,
            AuxiliaryRowFamilyV1::DirtyDoc,
            "doc-0500",
            "payload-500-v2",
        ))],
        tracks: Vec::new(),
    })?;
    if (one.rows_written, one.rows_deleted) != (1, 0) {
        return Err(format!("a one-row mutation must write one row: {one:?}").into());
    }
    let after: BTreeMap<Vec<u8>, Vec<u8>> = all_rows(&catalog)?
        .into_iter()
        .map(|row| (row.key.row_key, row.value))
        .collect();
    if after.len() != 1_000 {
        return Err(format!("row count drifted to {}", after.len()).into());
    }
    for (row_key, value) in &before {
        let expected: &[u8] = if row_key.as_slice() == b"doc-0500" {
            b"payload-500-v2"
        } else {
            value
        };
        if after.get(row_key).map(Vec::as_slice) != Some(expected) {
            return Err(format!(
                "row {} drifted after an unrelated mutation",
                String::from_utf8_lossy(row_key)
            )
            .into());
        }
    }
    Ok(())
}

#[test]
fn a_row_that_does_not_match_its_digest_is_refused_typed() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = open(temp.path())?;
    let _receipt = catalog.apply(&AuxiliaryMutationBatchV1 {
        rows: vec![AuxiliaryRowMutationV1::Upsert(row(
            AuxiliaryDomainV1::Structural,
            2,
            AuxiliaryRowFamilyV1::Chunk,
            "chunk-1",
            "chunk-bytes",
        ))],
        tracks: vec![AuxiliaryTrackRowV1 {
            repo_id: RepoId::new("repo-aux").expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev-aux")
                .expect("static fixture ID satisfies canonical policy"),
            track: SearchPlaneTrackKind::Structural,
            value: b"track-bytes".to_vec(),
        }],
    })?;
    drop(catalog);

    // Flip one byte of a stored value behind the catalog's back, as bit-rot
    // would; the engine's own integrity check does not notice.
    let path = catalog_dir(temp.path()).join(CATALOG_FILE_NAME);
    let connection = rusqlite::Connection::open(&path)?;
    let changed = connection.execute(
        "UPDATE auxiliary_rows_v1 SET value = CAST('chunk-bytez' AS BLOB) WHERE row_key = CAST('chunk-1' AS BLOB)",
        [],
    )?;
    let changed_tracks = connection.execute(
        "UPDATE auxiliary_tracks_v1 SET value = CAST('track-bytez' AS BLOB)",
        [],
    )?;
    if changed != 1 || changed_tracks != 1 {
        return Err(format!(
            "expected to corrupt one row each, changed {changed}/{changed_tracks}"
        )
        .into());
    }
    drop(connection);

    let catalog = open(temp.path())?;
    let refused = all_rows(&catalog).expect_err("a corrupt row must not be served");
    if typed_code(&refused) != Some(CATALOG_ROW_CORRUPT_CODE) {
        return Err(format!("expected CATALOG_ROW_CORRUPT, got {refused:?}").into());
    }
    let refused_track = catalog
        .track_rows()
        .expect_err("a corrupt track row must not be served");
    if typed_code(&refused_track) != Some(CATALOG_ROW_CORRUPT_CODE) {
        return Err(format!(
            "expected CATALOG_ROW_CORRUPT for the track row, got {refused_track:?}"
        )
        .into());
    }
    Ok(())
}

/// A generation forget is one more mutation of the batch it rides in: it
/// lands with the batch's other rows or not at all.
#[test]
fn forgetting_a_generation_drops_exactly_its_rows_across_domains() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = open(temp.path())?;
    let _receipt = catalog.apply(&AuxiliaryMutationBatchV1 {
        rows: vec![
            AuxiliaryRowMutationV1::Upsert(row(
                AuxiliaryDomainV1::History,
                1,
                AuxiliaryRowFamilyV1::Commit,
                "a",
                "g1",
            )),
            AuxiliaryRowMutationV1::Upsert(row(
                AuxiliaryDomainV1::Runtime,
                1,
                AuxiliaryRowFamilyV1::DirtyDoc,
                "d",
                "g1",
            )),
            AuxiliaryRowMutationV1::Upsert(row(
                AuxiliaryDomainV1::Structural,
                1,
                AuxiliaryRowFamilyV1::Chunk,
                "c",
                "g1",
            )),
            AuxiliaryRowMutationV1::Upsert(row(
                AuxiliaryDomainV1::History,
                2,
                AuxiliaryRowFamilyV1::Commit,
                "a",
                "g2",
            )),
        ],
        tracks: vec![AuxiliaryTrackRowV1 {
            repo_id: RepoId::new("repo-aux").expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev-aux")
                .expect("static fixture ID satisfies canonical policy"),
            track: SearchPlaneTrackKind::Structural,
            value: b"track".to_vec(),
        }],
    })?;
    let refused = catalog.apply(&AuxiliaryMutationBatchV1 {
        rows: vec![
            AuxiliaryRowMutationV1::ForgetGeneration(generation(1)),
            AuxiliaryRowMutationV1::Upsert(unwritable_row()),
        ],
        tracks: Vec::new(),
    });
    if !matches!(refused, Err(CoreError::InvalidContract(_))) {
        return Err(format!("the unwritable batch must be refused, got {refused:?}").into());
    }
    if all_rows(&catalog)?.len() != 4 {
        return Err("a refused batch must forget nothing".into());
    }
    let receipt = catalog.apply(&AuxiliaryMutationBatchV1 {
        rows: vec![
            AuxiliaryRowMutationV1::Upsert(row(
                AuxiliaryDomainV1::Structural,
                3,
                AuxiliaryRowFamilyV1::Chunk,
                "c",
                "g3",
            )),
            AuxiliaryRowMutationV1::ForgetGeneration(generation(1)),
        ],
        tracks: Vec::new(),
    })?;
    let expected = AuxiliaryMutationReceiptV1 {
        rows_written: 1,
        rows_deleted: 3,
    };
    if receipt != expected {
        return Err(format!(
            "the receipt counts one write and generation 1's three rows, got {receipt:?}"
        )
        .into());
    }
    let rows = all_rows(&catalog)?;
    let survivors = vec![
        row(
            AuxiliaryDomainV1::History,
            2,
            AuxiliaryRowFamilyV1::Commit,
            "a",
            "g2",
        ),
        row(
            AuxiliaryDomainV1::Structural,
            3,
            AuxiliaryRowFamilyV1::Chunk,
            "c",
            "g3",
        ),
    ];
    if rows != survivors {
        return Err(format!("only generation 1's rows may go: {rows:?}").into());
    }
    let tracks = catalog.track_rows()?;
    if tracks.len() != 1 || tracks.first().map(|track| track.value.as_slice()) != Some(b"track") {
        return Err(
            format!("track rows are not generation-scoped and must survive: {tracks:?}").into(),
        );
    }
    Ok(())
}

#[test]
fn track_rows_replace_by_key_and_round_trip() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = open(temp.path())?;
    let track = |value: &str| AuxiliaryTrackRowV1 {
        repo_id: RepoId::new("repo-aux").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev-aux")
            .expect("static fixture ID satisfies canonical policy"),
        track: SearchPlaneTrackKind::Structural,
        value: value.as_bytes().to_vec(),
    };
    let _first = catalog.apply(&AuxiliaryMutationBatchV1 {
        rows: Vec::new(),
        tracks: vec![track("v1")],
    })?;
    let second = catalog.apply(&AuxiliaryMutationBatchV1 {
        rows: Vec::new(),
        tracks: vec![track("v2")],
    })?;
    if second.rows_written != 1 {
        return Err(format!("track receipt drifted: {second:?}").into());
    }
    drop(catalog);
    let reopened = open(temp.path())?;
    if reopened.track_rows()? != vec![track("v2")] {
        return Err("the latest track row must be the one stored".into());
    }
    Ok(())
}
