//! The dense lane's index contract: sealed, verified, and pinned at query
//! time (QI-BB-027).
//!
//! Before this, a seal built `IvfHnswSq` with the library's defaults above a
//! row floor, the manifest said nothing about it, and a query ran with the
//! library's default effort. Three things could then drift under one
//! generation identity: the index topology (a library default changing),
//! the index's presence (a build that silently skipped it, or an inherited
//! index the seal never rebuilt), and the recall/latency point (a default
//! effort changing). This module owns all three.
//!
//! - **Policy**: every builder parameter and every query-effort parameter
//!   is a named constant here, passed explicitly to the library; none is
//!   left to a default.
//! - **Seal**: the seal drops every vector index the working dataset
//!   inherited, builds the policy's index when the row count is at or above
//!   the floor, reads back what the library reports, and refuses to seal
//!   an index that does not cover every row. The record goes into the scope
//!   manifest.
//! - **Open**: the open lists the dataset's indices and their statistics
//!   and refuses a generation whose dataset disagrees with its seal
//!   (`ANN_INDEX_MISSING`, `ANN_INDEX_INCOMPATIBLE`).
//! - **Query**: the lane runs with exactly the sealed effort, and an exact
//!   lane bypasses any index by construction.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use lancedb::DistanceType;
use lancedb::index::vector::IvfHnswSqIndexBuilder;
use lancedb::index::{Index, IndexConfig, IndexStatistics, IndexType};
use lancedb::query::VectorQuery;
use quanta_index_core::CoreError;
use quanta_index_core::domains::semantic::{
    DenseIndexEffortV1, DenseIndexV1, DenseLaneAttestationV1, DenseLaneContractV1,
};

use crate::errors::lancedb_err;
use crate::layout::COLUMN_VECTOR;
use crate::manifest::{
    AnnIndexSealV1, VECTOR_INDEX_MODE_EXACT, VECTOR_INDEX_MODE_IVF_HNSW_SQ, VectorIndexSealV1,
};

/// The library that builds and serves the index, as recorded in every seal.
pub(crate) const ANN_LIBRARY: &str = "lancedb";
/// Its version; `tests` pin this to the workspace lock file.
pub(crate) const ANN_LIBRARY_VERSION: &str = "0.30.0";

/// Row-count floor below which the lane is exact.
///
/// This is our policy, not a library minimum: below it a flat scan is
/// cheaper than a partition probe plus a graph walk, and the seal skips the
/// index build. At or above it the seal builds [`VECTOR_INDEX_NAME`].
pub(crate) const VECTOR_INDEX_MIN_ROWS: u64 = 256;

/// The one vector index a sealed generation carries.
pub(crate) const VECTOR_INDEX_NAME: &str = "vector_ivf_hnsw_sq";

/// Rows per IVF partition.
///
/// Partitions are `rows / TARGET_PARTITION_ROWS`, clamped to
/// `1..=MAX_PARTITIONS`. The HNSW graph inside a partition is what makes a
/// search sub-linear, so partitions are only worth their probe cost at this
/// scale.
const TARGET_PARTITION_ROWS: u64 = 1 << 20;
const MAX_PARTITIONS: u32 = 4096;
/// Training vectors per partition for the k-means that places partitions.
const IVF_SAMPLE_RATE: u32 = 256;
const IVF_MAX_ITERATIONS: u32 = 50;
/// HNSW neighbours per node and construction beam width.
const HNSW_M: u32 = 20;
const HNSW_EF_CONSTRUCTION: u32 = 300;

/// Partitions probed per query, clamped to the partition count.
const NPROBES: u32 = 20;
/// Graph beam width floor.
///
/// The beam is `max(EF_FLOOR, EF_PER_CANDIDATE * candidates)` where
/// `candidates = top_k * REFINE_FACTOR`; the floor keeps small queries from
/// starving the walk, the factor keeps large ones ahead of the candidate
/// list the graph must produce.
const EF_FLOOR: u32 = 64;
const EF_PER_CANDIDATE: u32 = 2;
/// The index scores 8-bit-quantized vectors; the nearest
/// `top_k * REFINE_FACTOR` candidates are re-ranked by exact distance so
/// the returned scores are true cosine similarities.
const REFINE_FACTOR: u32 = 2;

/// The index build the seal policy prescribes for a row count.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum VectorIndexPlanV1 {
    Exact,
    IvfHnswSq { num_partitions: u32 },
}

/// The policy's plan for `row_count` rows.
#[must_use]
pub(crate) fn plan_for_rows_v1(row_count: u64) -> VectorIndexPlanV1 {
    if row_count < VECTOR_INDEX_MIN_ROWS {
        return VectorIndexPlanV1::Exact;
    }
    let partitions = row_count.checked_div(TARGET_PARTITION_ROWS).unwrap_or(0);
    let clamped =
        u32::try_from(partitions).map_or(MAX_PARTITIONS, |value| value.clamp(1, MAX_PARTITIONS));
    VectorIndexPlanV1::IvfHnswSq {
        num_partitions: clamped,
    }
}

fn vector_indices(configs: &[IndexConfig]) -> Vec<&IndexConfig> {
    configs
        .iter()
        .filter(|config| config.columns.iter().any(|column| column == COLUMN_VECTOR))
        .collect()
}

fn index_type_token(index_type: &IndexType) -> &'static str {
    match index_type {
        IndexType::IvfHnswSq => VECTOR_INDEX_MODE_IVF_HNSW_SQ,
        IndexType::IvfFlat => "ivf_flat",
        IndexType::IvfSq => "ivf_sq",
        IndexType::IvfPq => "ivf_pq",
        IndexType::IvfRq => "ivf_rq",
        IndexType::IvfHnswPq => "ivf_hnsw_pq",
        IndexType::IvfHnswFlat => "ivf_hnsw_flat",
        IndexType::BTree => "btree",
        IndexType::Bitmap => "bitmap",
        IndexType::LabelList => "label_list",
        IndexType::FTS => "fts",
    }
}

fn index_segments_u32(statistics: &IndexStatistics) -> Result<u32, CoreError> {
    statistics.num_indices.ok_or_else(|| {
        CoreError::Storage(
            "semantic: index statistics report no segment count for the vector index".to_string(),
        )
    })
}

fn rows_u64(rows: usize, what: &str) -> Result<u64, CoreError> {
    u64::try_from(rows)
        .map_err(|err| CoreError::Storage(format!("semantic: {what} overflow: {err}")))
}

/// Seal the dense lane of `table`, which holds `row_count` rows.
///
/// Drops every inherited vector index first: a delta generation clones its
/// base dataset, and a base index neither covers the delta's rows nor
/// necessarily carries the current name. Then builds the policy's index or
/// leaves the lane exact, reads back the library's report and refuses to
/// seal an index that does not cover every row.
pub(crate) async fn seal_vector_index_v1(
    table: &lancedb::Table,
    row_count: u64,
) -> Result<VectorIndexSealV1, CoreError> {
    let inherited = table
        .list_indices()
        .await
        .map_err(|err| lancedb_err("list_indices before seal", err))?;
    for config in vector_indices(&inherited) {
        table
            .drop_index(&config.name)
            .await
            .map_err(|err| lancedb_err("drop inherited vector index", err))?;
    }
    let plan = plan_for_rows_v1(row_count);
    let VectorIndexPlanV1::IvfHnswSq { num_partitions } = plan else {
        return Ok(VectorIndexSealV1 {
            mode: VECTOR_INDEX_MODE_EXACT.to_string(),
            library: ANN_LIBRARY.to_string(),
            library_version: ANN_LIBRARY_VERSION.to_string(),
            index_min_rows: VECTOR_INDEX_MIN_ROWS,
            ann: None,
        });
    };
    table
        .create_index(
            &[COLUMN_VECTOR],
            Index::IvfHnswSq(
                IvfHnswSqIndexBuilder::default()
                    .distance_type(DistanceType::Cosine)
                    .num_partitions(num_partitions)
                    .sample_rate(IVF_SAMPLE_RATE)
                    .max_iterations(IVF_MAX_ITERATIONS)
                    .num_edges(HNSW_M)
                    .ef_construction(HNSW_EF_CONSTRUCTION),
            ),
        )
        .name(VECTOR_INDEX_NAME.to_string())
        .replace(false)
        .execute()
        .await
        .map_err(|err| lancedb_err("create_index IvfHnswSq(cosine)", err))?;
    let statistics = table
        .index_stats(VECTOR_INDEX_NAME)
        .await
        .map_err(|err| lancedb_err("index_stats after seal", err))?
        .ok_or_else(|| {
            CoreError::Storage(format!(
                "semantic: the vector index `{VECTOR_INDEX_NAME}` is absent right after its build"
            ))
        })?;
    let indexed_rows = rows_u64(statistics.num_indexed_rows, "indexed row count")?;
    let unindexed_rows = rows_u64(statistics.num_unindexed_rows, "unindexed row count")?;
    if indexed_rows != row_count || unindexed_rows != 0 {
        return Err(CoreError::Storage(format!(
            "semantic: refusing to seal a vector index covering {indexed_rows} of {row_count} rows ({unindexed_rows} unindexed)"
        )));
    }
    if statistics.index_type != IndexType::IvfHnswSq
        || statistics.distance_type != Some(DistanceType::Cosine)
    {
        return Err(CoreError::Storage(format!(
            "semantic: the library built index type {} with distance {:?}, not ivf_hnsw_sq/cosine",
            index_type_token(&statistics.index_type),
            statistics.distance_type
        )));
    }
    Ok(VectorIndexSealV1 {
        mode: VECTOR_INDEX_MODE_IVF_HNSW_SQ.to_string(),
        library: ANN_LIBRARY.to_string(),
        library_version: ANN_LIBRARY_VERSION.to_string(),
        index_min_rows: VECTOR_INDEX_MIN_ROWS,
        ann: Some(AnnIndexSealV1 {
            index_name: VECTOR_INDEX_NAME.to_string(),
            distance: "cosine".to_string(),
            num_partitions,
            sample_rate: IVF_SAMPLE_RATE,
            max_iterations: IVF_MAX_ITERATIONS,
            hnsw_m: HNSW_M,
            hnsw_ef_construction: HNSW_EF_CONSTRUCTION,
            indexed_rows,
            index_segments: index_segments_u32(&statistics)?,
            nprobes: NPROBES.min(num_partitions),
            ef_floor: EF_FLOOR,
            ef_per_candidate: EF_PER_CANDIDATE,
            refine_factor: REFINE_FACTOR,
        }),
    })
}

/// How an opened generation runs its dense lane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LoadedVectorIndexV1 {
    /// The effort of the approximate index the lane runs through; `None`
    /// for an exact lane.
    effort: Option<QueryEffortV1>,
    attestation: DenseLaneAttestationV1,
}

/// The effort the lane spends in an approximate index, per query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QueryEffortV1 {
    partitions: u32,
    nprobes: u32,
    ef_floor: u32,
    ef_per_candidate: u32,
    refine_factor: u32,
}

impl QueryEffortV1 {
    fn from_seal(ann: &AnnIndexSealV1) -> Self {
        Self {
            partitions: ann.num_partitions,
            nprobes: ann.nprobes,
            ef_floor: ann.ef_floor,
            ef_per_candidate: ann.ef_per_candidate,
            refine_factor: ann.refine_factor,
        }
    }

    /// The policy effort for a legacy index whose partition count no seal
    /// recorded; the library clamps probes to what exists.
    const fn legacy_policy() -> Self {
        Self {
            partitions: NPROBES,
            nprobes: NPROBES,
            ef_floor: EF_FLOOR,
            ef_per_candidate: EF_PER_CANDIDATE,
            refine_factor: REFINE_FACTOR,
        }
    }

    fn contract(&self) -> DenseIndexEffortV1 {
        DenseIndexEffortV1 {
            index_kind: VECTOR_INDEX_MODE_IVF_HNSW_SQ.to_string(),
            partitions: self.partitions,
            nprobes: self.nprobes,
            ef_floor: self.ef_floor,
            ef_per_candidate: self.ef_per_candidate,
            refine_factor: self.refine_factor,
        }
    }

    fn apply(&self, query: VectorQuery, top_k: usize) -> Result<VectorQuery, CoreError> {
        let refine_factor = usize::try_from(self.refine_factor).map_err(|err| {
            CoreError::Storage(format!("semantic: refine factor overflow: {err}"))
        })?;
        let candidates = top_k.checked_mul(refine_factor).ok_or_else(|| {
            CoreError::InvalidContract(format!(
                "semantic: top_k {top_k} times refine factor {refine_factor} overflows"
            ))
        })?;
        let ef_per_candidate = usize::try_from(self.ef_per_candidate)
            .map_err(|err| CoreError::Storage(format!("semantic: ef factor overflow: {err}")))?;
        let ef_floor = usize::try_from(self.ef_floor)
            .map_err(|err| CoreError::Storage(format!("semantic: ef floor overflow: {err}")))?;
        let ef = candidates
            .checked_mul(ef_per_candidate)
            .ok_or_else(|| {
                CoreError::InvalidContract(format!(
                    "semantic: {candidates} candidates times ef factor {ef_per_candidate} overflows"
                ))
            })?
            .max(ef_floor);
        let nprobes = usize::try_from(self.nprobes)
            .map_err(|err| CoreError::Storage(format!("semantic: nprobes overflow: {err}")))?;
        Ok(query
            .nprobes(nprobes)
            .ef(ef)
            .refine_factor(self.refine_factor))
    }
}

impl LoadedVectorIndexV1 {
    /// Pin the sealed effort on a query, or bypass every index for an exact
    /// lane.
    pub(crate) fn apply(&self, query: VectorQuery, top_k: usize) -> Result<VectorQuery, CoreError> {
        match self.effort.as_ref() {
            Some(effort) => effort.apply(query, top_k),
            None => Ok(query.bypass_vector_index()),
        }
    }

    pub(crate) fn contract(&self) -> DenseLaneContractV1 {
        DenseLaneContractV1 {
            index: self.effort.as_ref().map_or(DenseIndexV1::Exact, |effort| {
                DenseIndexV1::Approximate(effort.contract())
            }),
            attestation: self.attestation,
        }
    }
}

fn ann_missing(detail: &str) -> CoreError {
    CoreError::Typed {
        code: "ANN_INDEX_MISSING".to_string(),
        message: format!("semantic: {detail}"),
    }
}

fn ann_incompatible(detail: &str) -> CoreError {
    CoreError::Typed {
        code: "ANN_INDEX_INCOMPATIBLE".to_string(),
        message: format!("semantic: {detail}"),
    }
}

/// Verify the dataset's vector indices against the seal, or observe them
/// for a generation that sealed none.
///
/// A sealed exact lane must have no vector index (an index the seal did not
/// record would silently change the served mode); a sealed approximate lane
/// must have exactly the recorded index, reported by the library with the
/// recorded type, distance, coverage and segment count.
pub(crate) async fn verify_vector_index_v1(
    table: &lancedb::Table,
    seal: Option<&VectorIndexSealV1>,
    row_count: u64,
) -> Result<LoadedVectorIndexV1, CoreError> {
    let configs = table
        .list_indices()
        .await
        .map_err(|err| lancedb_err("list_indices at open", err))?;
    let present = vector_indices(&configs);
    let Some(seal) = seal else {
        let approximate = present
            .iter()
            .any(|config| config.index_type == IndexType::IvfHnswSq);
        return Ok(LoadedVectorIndexV1 {
            effort: approximate.then(QueryEffortV1::legacy_policy),
            attestation: DenseLaneAttestationV1::LegacyUnverified,
        });
    };
    if seal.library != ANN_LIBRARY {
        return Err(ann_incompatible(&format!(
            "the seal was built by `{}`, which is not the serving library `{ANN_LIBRARY}`",
            seal.library
        )));
    }
    let attestation = if seal.library_version == ANN_LIBRARY_VERSION {
        DenseLaneAttestationV1::Sealed
    } else {
        DenseLaneAttestationV1::SealedByAnotherLibraryVersion
    };
    let Some(ann) = seal.ann.as_ref() else {
        if let Some(unsealed) = present.first() {
            return Err(ann_incompatible(&format!(
                "the seal serves an exact lane but the dataset carries vector index `{}` ({})",
                unsealed.name,
                index_type_token(&unsealed.index_type)
            )));
        }
        return Ok(LoadedVectorIndexV1 {
            effort: None,
            attestation,
        });
    };
    let Some(config) = present.iter().find(|config| config.name == ann.index_name) else {
        let names: Vec<&str> = present.iter().map(|config| config.name.as_str()).collect();
        return Err(ann_missing(&format!(
            "the seal recorded vector index `{}` but the dataset lists {names:?}",
            ann.index_name
        )));
    };
    if present.len() != 1 {
        let names: Vec<&str> = present.iter().map(|config| config.name.as_str()).collect();
        return Err(ann_incompatible(&format!(
            "the seal recorded one vector index but the dataset carries {names:?}"
        )));
    }
    if config.index_type != IndexType::IvfHnswSq {
        return Err(ann_incompatible(&format!(
            "vector index `{}` is {} in the dataset, ivf_hnsw_sq in the seal",
            ann.index_name,
            index_type_token(&config.index_type)
        )));
    }
    let statistics = table
        .index_stats(&ann.index_name)
        .await
        .map_err(|err| lancedb_err("index_stats at open", err))?
        .ok_or_else(|| {
            ann_missing(&format!(
                "vector index `{}` is listed but reports no statistics",
                ann.index_name
            ))
        })?;
    if statistics.index_type != IndexType::IvfHnswSq {
        return Err(ann_incompatible(&format!(
            "vector index `{}` statistics report {}, the seal says ivf_hnsw_sq",
            ann.index_name,
            index_type_token(&statistics.index_type)
        )));
    }
    if statistics.distance_type != Some(DistanceType::Cosine) {
        return Err(ann_incompatible(&format!(
            "vector index `{}` distance is {:?}, the seal says cosine",
            ann.index_name, statistics.distance_type
        )));
    }
    let indexed_rows = rows_u64(statistics.num_indexed_rows, "indexed row count")?;
    let unindexed_rows = rows_u64(statistics.num_unindexed_rows, "unindexed row count")?;
    if indexed_rows != ann.indexed_rows || indexed_rows != row_count || unindexed_rows != 0 {
        return Err(ann_incompatible(&format!(
            "vector index `{}` covers {indexed_rows} rows with {unindexed_rows} unindexed; the seal covers {} of {row_count}",
            ann.index_name, ann.indexed_rows
        )));
    }
    let segments = index_segments_u32(&statistics)?;
    if segments != ann.index_segments {
        return Err(ann_incompatible(&format!(
            "vector index `{}` has {segments} segments, the seal recorded {}",
            ann.index_name, ann.index_segments
        )));
    }
    Ok(LoadedVectorIndexV1 {
        effort: Some(QueryEffortV1::from_seal(ann)),
        attestation,
    })
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning tests assert with `assert!` on fixture invariants; a violated fixture invariant is not a propagatable error"
)]
mod tests {
    use std::sync::Arc;

    use arrow_array::{Array, FixedSizeListArray, Float32Array, RecordBatch};
    use arrow_schema::{DataType, Field, Schema};
    use quanta_index_core::CoreError;
    use quanta_index_core::domains::semantic::DenseLaneAttestationV1;

    use super::{
        ANN_LIBRARY_VERSION, LoadedVectorIndexV1, MAX_PARTITIONS, TARGET_PARTITION_ROWS,
        VECTOR_INDEX_MIN_ROWS, VECTOR_INDEX_NAME, VectorIndexPlanV1, plan_for_rows_v1,
        seal_vector_index_v1, verify_vector_index_v1,
    };
    use crate::layout::COLUMN_VECTOR;
    use crate::manifest::{VECTOR_INDEX_MODE_EXACT, VectorIndexSealV1};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const DIMENSION: i32 = 8;

    /// A vector-only table with `rows` deterministic rows, the smallest
    /// dataset the seal and the verifier can be exercised on.
    async fn vector_table(
        root: &std::path::Path,
        rows: usize,
    ) -> Result<lancedb::Table, Box<dyn std::error::Error>> {
        let dimension = usize::try_from(DIMENSION)?;
        let mut flat = Vec::with_capacity(rows.saturating_mul(dimension));
        for row in 0..rows {
            for lane in 0..dimension {
                let value = ((row.wrapping_mul(31).wrapping_add(lane.wrapping_mul(7))) % 97)
                    .wrapping_add(1);
                flat.push(f32::from(u8::try_from(value)?));
            }
        }
        let values: Arc<dyn Array> = Arc::new(Float32Array::from(flat));
        let vectors = FixedSizeListArray::try_new(
            Arc::new(Field::new("item", DataType::Float32, true)),
            DIMENSION,
            values,
            None,
        )?;
        let schema = Arc::new(Schema::new(vec![Field::new(
            COLUMN_VECTOR,
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                DIMENSION,
            ),
            false,
        )]));
        let batch = RecordBatch::try_new(schema, vec![Arc::new(vectors)])?;
        let connection = lancedb::connect(&root.to_string_lossy()).execute().await?;
        let table = connection.create_table("semantic", batch).execute().await?;
        Ok(table)
    }

    /// Drive one async test body through the crate's single sync seam.
    fn run<F>(body: F) -> TestResult
    where
        F: core::future::Future<Output = TestResult>,
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        crate::run_blocking(&runtime, body)
    }

    fn typed_code(result: &Result<LoadedVectorIndexV1, CoreError>) -> Option<&str> {
        match result {
            Err(CoreError::Typed { code, .. }) => Some(code.as_str()),
            _ => None,
        }
    }

    fn exact_seal() -> VectorIndexSealV1 {
        VectorIndexSealV1 {
            mode: VECTOR_INDEX_MODE_EXACT.to_string(),
            library: "lancedb".to_string(),
            library_version: ANN_LIBRARY_VERSION.to_string(),
            index_min_rows: VECTOR_INDEX_MIN_ROWS,
            ann: None,
        }
    }

    #[test]
    fn the_verifier_refuses_every_disagreement_between_seal_and_dataset() -> TestResult {
        run(async {
            let temp = tempfile::tempdir()?;
            let rows = usize::try_from(VECTOR_INDEX_MIN_ROWS)?;
            let table = vector_table(temp.path(), rows).await?;
            let sealed = seal_vector_index_v1(&table, VECTOR_INDEX_MIN_ROWS).await?;
            let ann = sealed.ann.clone().ok_or("the floor seals an index")?;

            // The dataset agrees with its own seal.
            let loaded =
                verify_vector_index_v1(&table, Some(&sealed), VECTOR_INDEX_MIN_ROWS).await?;
            assert!(loaded.effort.is_some());
            assert_eq!(loaded.attestation, DenseLaneAttestationV1::Sealed);

            // A seal from another version of the same library is served, and
            // says so; one from another library is not served at all.
            let mut other_version = sealed.clone();
            other_version.library_version = "0.0.1".to_string();
            let served =
                verify_vector_index_v1(&table, Some(&other_version), VECTOR_INDEX_MIN_ROWS).await?;
            assert_eq!(
                served.attestation,
                DenseLaneAttestationV1::SealedByAnotherLibraryVersion
            );
            let mut other_library = sealed.clone();
            other_library.library = "faiss".to_string();
            let refused =
                verify_vector_index_v1(&table, Some(&other_library), VECTOR_INDEX_MIN_ROWS).await;
            assert_eq!(typed_code(&refused), Some("ANN_INDEX_INCOMPATIBLE"));

            // An exact seal over an indexed dataset: the served mode would not
            // be the sealed one.
            let indexed_but_exact =
                verify_vector_index_v1(&table, Some(&exact_seal()), VECTOR_INDEX_MIN_ROWS).await;
            assert_eq!(
                typed_code(&indexed_but_exact),
                Some("ANN_INDEX_INCOMPATIBLE")
            );

            // A seal whose coverage or segment count the library contradicts.
            let mut fewer_rows = sealed.clone();
            if let Some(record) = fewer_rows.ann.as_mut() {
                record.indexed_rows = record.indexed_rows.saturating_sub(1);
            }
            let coverage =
                verify_vector_index_v1(&table, Some(&fewer_rows), VECTOR_INDEX_MIN_ROWS).await;
            assert_eq!(typed_code(&coverage), Some("ANN_INDEX_INCOMPATIBLE"));
            let mut more_segments = sealed.clone();
            if let Some(record) = more_segments.ann.as_mut() {
                record.index_segments = record.index_segments.saturating_add(1);
            }
            let segments =
                verify_vector_index_v1(&table, Some(&more_segments), VECTOR_INDEX_MIN_ROWS).await;
            assert_eq!(typed_code(&segments), Some("ANN_INDEX_INCOMPATIBLE"));

            // A seal naming an index the dataset does not list.
            let mut renamed = sealed.clone();
            if let Some(record) = renamed.ann.as_mut() {
                record.index_name = "vector_idx".to_string();
            }
            let missing_name =
                verify_vector_index_v1(&table, Some(&renamed), VECTOR_INDEX_MIN_ROWS).await;
            assert_eq!(typed_code(&missing_name), Some("ANN_INDEX_MISSING"));

            // The index gone from the dataset while the seal still records it.
            table.drop_index(&ann.index_name).await?;
            let dropped =
                verify_vector_index_v1(&table, Some(&sealed), VECTOR_INDEX_MIN_ROWS).await;
            assert_eq!(typed_code(&dropped), Some("ANN_INDEX_MISSING"));
            // ... and the same dataset agrees with an exact seal again.
            let exact =
                verify_vector_index_v1(&table, Some(&exact_seal()), VECTOR_INDEX_MIN_ROWS).await?;
            assert_eq!(
                exact,
                LoadedVectorIndexV1 {
                    effort: None,
                    attestation: DenseLaneAttestationV1::Sealed,
                }
            );
            Ok(())
        })
    }

    #[test]
    fn a_legacy_generation_is_observed_not_verified() -> TestResult {
        run(async {
            let temp = tempfile::tempdir()?;
            let rows = usize::try_from(VECTOR_INDEX_MIN_ROWS)?;
            let table = vector_table(temp.path(), rows).await?;
            let none = verify_vector_index_v1(&table, None, VECTOR_INDEX_MIN_ROWS).await?;
            assert_eq!(
                none,
                LoadedVectorIndexV1 {
                    effort: None,
                    attestation: DenseLaneAttestationV1::LegacyUnverified,
                }
            );
            let _sealed = seal_vector_index_v1(&table, VECTOR_INDEX_MIN_ROWS).await?;
            let some = verify_vector_index_v1(&table, None, VECTOR_INDEX_MIN_ROWS).await?;
            assert!(some.effort.is_some());
            assert_eq!(some.attestation, DenseLaneAttestationV1::LegacyUnverified);
            Ok(())
        })
    }

    #[test]
    fn the_seal_replaces_an_inherited_index_of_any_name() -> TestResult {
        run(async {
            let temp = tempfile::tempdir()?;
            let rows = usize::try_from(VECTOR_INDEX_MIN_ROWS)?;
            let table = vector_table(temp.path(), rows).await?;
            // An index a base generation built under a different name and
            // policy, as a delta inherits it.
            table
                .create_index(
                    &[COLUMN_VECTOR],
                    lancedb::index::Index::IvfHnswSq(
                        lancedb::index::vector::IvfHnswSqIndexBuilder::default()
                            .distance_type(lancedb::DistanceType::Cosine),
                    ),
                )
                .name("vector_idx".to_string())
                .execute()
                .await?;
            let sealed = seal_vector_index_v1(&table, VECTOR_INDEX_MIN_ROWS).await?;
            let names: Vec<String> = table
                .list_indices()
                .await?
                .into_iter()
                .map(|config| config.name)
                .collect();
            assert_eq!(names, vec![VECTOR_INDEX_NAME.to_string()]);
            assert_eq!(
                sealed.ann.as_ref().map(|ann| ann.indexed_rows),
                Some(VECTOR_INDEX_MIN_ROWS)
            );
            // Below the floor the seal drops what it inherited and seals exact.
            let small_root = tempfile::tempdir()?;
            let below = VECTOR_INDEX_MIN_ROWS.saturating_sub(1);
            let small = vector_table(small_root.path(), usize::try_from(below)?).await?;
            small
                .create_index(
                    &[COLUMN_VECTOR],
                    lancedb::index::Index::IvfHnswSq(
                        lancedb::index::vector::IvfHnswSqIndexBuilder::default()
                            .distance_type(lancedb::DistanceType::Cosine),
                    ),
                )
                .name("vector_idx".to_string())
                .execute()
                .await?;
            let exact = seal_vector_index_v1(&small, below).await?;
            assert_eq!(exact.mode, VECTOR_INDEX_MODE_EXACT);
            assert!(small.list_indices().await?.is_empty());
            Ok(())
        })
    }

    #[test]
    fn the_recorded_library_version_is_the_locked_one() {
        // The seal records the library version as provenance; a dependency
        // bump that forgets this constant would record the wrong builder.
        let lock =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock"))
                .expect("workspace lock file");
        let mut lines = lock.lines();
        let mut locked = None;
        while let Some(line) = lines.next() {
            if line.trim() == "name = \"lancedb\"" {
                locked = lines.next().and_then(|version| {
                    version
                        .trim()
                        .strip_prefix("version = \"")
                        .and_then(|rest| rest.strip_suffix('"'))
                        .map(str::to_owned)
                });
                break;
            }
        }
        assert_eq!(locked.as_deref(), Some(ANN_LIBRARY_VERSION));
    }

    #[test]
    fn the_plan_is_exact_below_the_floor_and_partitions_by_scale_above_it() {
        assert_eq!(
            plan_for_rows_v1(VECTOR_INDEX_MIN_ROWS.saturating_sub(1)),
            VectorIndexPlanV1::Exact
        );
        assert_eq!(plan_for_rows_v1(0), VectorIndexPlanV1::Exact);
        assert_eq!(
            plan_for_rows_v1(VECTOR_INDEX_MIN_ROWS),
            VectorIndexPlanV1::IvfHnswSq { num_partitions: 1 }
        );
        assert_eq!(
            plan_for_rows_v1(TARGET_PARTITION_ROWS.saturating_mul(3)),
            VectorIndexPlanV1::IvfHnswSq { num_partitions: 3 }
        );
        assert_eq!(
            plan_for_rows_v1(u64::MAX),
            VectorIndexPlanV1::IvfHnswSq {
                num_partitions: MAX_PARTITIONS
            }
        );
    }
}
