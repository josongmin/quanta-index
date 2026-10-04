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
//! - **Policy**: every builder parameter, every query-effort parameter and
//!   the append budget is a named constant here, passed explicitly to the
//!   library; none is left to a default.
//! - **Seal**: a fresh seal builds the policy's index when the row count is
//!   at or above the floor. A delta seal appends its new rows to the index
//!   it inherited when that index is the policy's own recipe under the same
//!   library version and the rows assigned since its centroids were trained
//!   stay inside the append budget; otherwise it drops what it inherited
//!   and trains again. Either way the seal reads back what the library
//!   reports — coverage, segment count, and the graph parameters every
//!   segment was actually built with — refuses an index that does not
//!   cover every row, and records the recipe, the report, the effort, the
//!   training lineage and the per-segment build in the scope manifest. An
//!   appended segment is built by the library's incremental builder under
//!   its own parameters; the seal records those verbatim rather than
//!   claiming the recipe for it.
//! - **Open**: the open lists the dataset's indices, their statistics and
//!   their per-segment build parameters and refuses a generation whose
//!   dataset disagrees with its seal (`ANN_INDEX_MISSING`,
//!   `ANN_INDEX_INCOMPATIBLE`). A generation without a seal is not served.
//! - **Query**: the lane runs with exactly the sealed effort, and an exact
//!   lane bypasses any index by construction.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use lance::index::DatasetIndexExt as _;
use lancedb::DistanceType;
use lancedb::index::vector::IvfHnswSqIndexBuilder;
use lancedb::index::{Index, IndexConfig, IndexStatistics, IndexType};
use lancedb::query::VectorQuery;
use lancedb::table::{OptimizeAction, OptimizeOptions};
use quanta_index_core::CoreError;
use quanta_index_core::domains::semantic::{
    DenseIndexBuildV1, DenseIndexEffortV1, DenseIndexSegmentBuildV1, DenseIndexTrainingV1,
    DenseIndexV1, DenseLaneAttestationV1, DenseLaneContractV1,
};

use crate::budget::DenseLaneKindV1;
use crate::errors::lancedb_err;
use crate::layout::COLUMN_VECTOR;
use crate::manifest::{
    AnnIndexLineageV1, AnnIndexSealV1, AnnIndexSegmentSealV1, VECTOR_INDEX_MODE_EXACT,
    VECTOR_INDEX_MODE_IVF_HNSW_SQ, VectorIndexSealV1,
};
use crate::semantic_row_integrity_v1::SemanticRowFingerprintV1;

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

/// Rows a delta may assign to inherited centroids without retraining, per
/// thousand rows those centroids were trained on: one quarter.
///
/// An append assigns each new row to the nearest existing centroid and
/// builds it into a segment of its own; the centroids do not move. While
/// [`plan_for_rows_v1`] yields one partition (fewer than two
/// [`TARGET_PARTITION_ROWS`]) there is one centroid and nothing to
/// misassign, so the budget bounds two other costs: the share of served
/// rows the k-means never saw (its centroids were placed by
/// [`IVF_SAMPLE_RATE`] samples per partition of the trained population,
/// and a quarter more of the same distribution does not move them), and
/// the rows living in segments whose graphs the library's incremental
/// builder shaped under its own parameters rather than the policy's
/// [`HNSW_EF_CONSTRUCTION`] — parameters the seal reads back and records
/// per segment. `vector_index::tests` measure the appended index against a
/// freshly trained one at this ratio.
pub(crate) const ANN_APPEND_RATIO_MAX_PER_MILLE: u32 = 250;

/// The absolute cap on appended rows, whatever the trained population.
///
/// The ratio alone would let a ten-million-row base accumulate millions of
/// rows in appended segments; this bounds that mass to a quarter of one
/// [`TARGET_PARTITION_ROWS`] partition, so the incremental builder never
/// owns more of the served graph than one partition's re-clustering would
/// have touched.
pub(crate) const ANN_APPEND_ROWS_MAX: u64 = 1 << 18;

/// The most segments appends may add beside the trained one.
///
/// Every appended segment is one more graph the query walks with the same
/// sealed effort, so the cost of a query grows with the segment count even
/// though its effort does not change; eight keeps that growth inside one
/// order of magnitude before a retrain folds the segments back into one.
pub(crate) const ANN_APPEND_SEGMENTS_MAX: u32 = 8;

/// What the seal knows about the generation it seals.
#[derive(Clone, Copy, Debug)]
pub(crate) struct VectorIndexSealInputV1<'a> {
    /// The generation being sealed; a train records it as the lineage's
    /// origin.
    pub(crate) generation: u64,
    /// The live rows of the dataset being sealed.
    pub(crate) row_count: u64,
    /// The sealed index contract of the base generation whose dataset this
    /// one inherited by hard link, if it inherited one.
    pub(crate) inherited: Option<&'a VectorIndexSealV1>,
}

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

/// One vector index as the dataset lists it, whatever its segment count.
#[derive(Clone, Debug, PartialEq)]
struct VectorIndexListingV1 {
    name: String,
    index_type: IndexType,
}

/// The distinct vector indices the dataset lists, in listing order.
///
/// The library lists one entry per segment, so an appended index appears
/// once per append under the same name; this folds those into one listing
/// and refuses a name whose segments disagree on their type.
fn vector_indices(configs: &[IndexConfig]) -> Result<Vec<VectorIndexListingV1>, CoreError> {
    let mut listings: Vec<VectorIndexListingV1> = Vec::new();
    for config in configs
        .iter()
        .filter(|config| config.columns.iter().any(|column| column == COLUMN_VECTOR))
    {
        match listings.iter().find(|listing| listing.name == config.name) {
            Some(listing) if listing.index_type == config.index_type => {}
            Some(listing) => {
                return Err(CoreError::Storage(format!(
                    "semantic: vector index `{}` lists segments of type {} and {}",
                    config.name,
                    index_type_token(&listing.index_type),
                    index_type_token(&config.index_type)
                )));
            }
            None => listings.push(VectorIndexListingV1 {
                name: config.name.clone(),
                index_type: config.index_type.clone(),
            }),
        }
    }
    Ok(listings)
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

fn seal_refused(detail: &str) -> CoreError {
    CoreError::Storage(format!(
        "semantic: refusing to seal the vector index: {detail}"
    ))
}

/// What the library reports about the policy's index, in the seal's units.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct IndexReportV1 {
    indexed_rows: u64,
    unindexed_rows: u64,
    segments: u32,
}

/// Read the policy index's statistics and refuse a report whose type or
/// distance is not the policy's.
async fn read_index_report_v1(
    table: &lancedb::Table,
    when: &str,
) -> Result<IndexReportV1, CoreError> {
    let statistics = table
        .index_stats(VECTOR_INDEX_NAME)
        .await
        .map_err(|err| lancedb_err(&format!("index_stats {when}"), err))?
        .ok_or_else(|| {
            seal_refused(&format!(
                "the vector index `{VECTOR_INDEX_NAME}` reports no statistics {when}"
            ))
        })?;
    if statistics.index_type != IndexType::IvfHnswSq
        || statistics.distance_type != Some(DistanceType::Cosine)
    {
        return Err(seal_refused(&format!(
            "the library reports index type {} with distance {:?} {when}, not ivf_hnsw_sq/cosine",
            index_type_token(&statistics.index_type),
            statistics.distance_type
        )));
    }
    Ok(IndexReportV1 {
        indexed_rows: rows_u64(statistics.num_indexed_rows, "indexed row count")?,
        unindexed_rows: rows_u64(statistics.num_unindexed_rows, "unindexed row count")?,
        segments: index_segments_u32(&statistics)?,
    })
}

/// The graph parameters of every segment of `index_name`, as the library
/// reports them, in the library's segment order.
///
/// Read from the dataset's index statistics, where each segment carries
/// the `HnswBuildParams` it was built with. This is the only place the
/// adapter learns what an appended segment was really built with, so a
/// report without a segment identity or a graph parameter is a refusal,
/// never a guess.
async fn read_index_segments_v1(
    table: &lancedb::Table,
    index_name: &str,
    when: &str,
) -> Result<Vec<AnnIndexSegmentSealV1>, CoreError> {
    let dataset = table
        .dataset()
        .ok_or_else(|| {
            CoreError::Storage(format!(
                "semantic: the table is not a local dataset; its index segments cannot be read {when}"
            ))
        })?
        .get()
        .await
        .map_err(|err| lancedb_err(&format!("open dataset for index segments {when}"), err))?;
    let statistics = dataset.index_statistics(index_name).await.map_err(|err| {
        CoreError::Storage(format!(
            "semantic: read index segment statistics {when}: {err}"
        ))
    })?;
    let statistics: serde_json::Value = serde_json::from_str(&statistics).map_err(|err| {
        CoreError::Storage(format!(
            "semantic: decode index segment statistics {when}: {err}"
        ))
    })?;
    let malformed = |detail: &str| {
        CoreError::Storage(format!(
            "semantic: index segment statistics {when} are malformed: {detail}"
        ))
    };
    let segments = statistics
        .get("indices")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| malformed("no `indices` list"))?;
    let mut out = Vec::with_capacity(segments.len());
    for (position, segment) in segments.iter().enumerate() {
        let uuid = segment
            .get("uuid")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| malformed(&format!("segment {position} names no `uuid`")))?;
        let params = segment
            .get("sub_index")
            .and_then(|sub_index| sub_index.get("params"))
            .ok_or_else(|| {
                malformed(&format!("segment {position} reports no `sub_index.params`"))
            })?;
        let parameter = |name: &str| -> Result<u32, CoreError> {
            let value = params
                .get(name)
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| malformed(&format!("segment {position} reports no `{name}`")))?;
            u32::try_from(value).map_err(|error| {
                malformed(&format!(
                    "segment {position} `{name}` {value} overflows: {error}"
                ))
            })
        };
        out.push(AnnIndexSegmentSealV1 {
            uuid: uuid.to_string(),
            hnsw_m: parameter("m")?,
            hnsw_ef_construction: parameter("ef_construction")?,
        });
    }
    Ok(out)
}

/// Count physical row removals and appends relative to a sealed base.
/// Byte-identical scope replacement still replaces Lance native row IDs.
/// An in-place payload change under one physical ID is a custody violation.
fn row_change_counts_v1(
    base: &[SemanticRowFingerprintV1],
    current: &[SemanticRowFingerprintV1],
) -> Result<(u64, u64), CoreError> {
    let ordered = |rows: &[SemanticRowFingerprintV1]| {
        rows.windows(2).all(|pair| {
            pair.first()
                .zip(pair.get(1))
                .is_some_and(|(left, right)| left.record_id < right.record_id)
        })
    };
    if !ordered(base) || !ordered(current) {
        return Err(seal_refused(
            "semantic row fingerprints are not in unique record-ID order",
        ));
    }
    // A physical row carried forward from the base must retain its logical
    // identity and canonical payload, even when the record-ID merge below
    // would otherwise count the change as one removal and one insertion.
    let mut base_by_native_id = std::collections::BTreeMap::new();
    for row in base {
        if base_by_native_id.insert(row.native_row_id, row).is_some() {
            return Err(seal_refused(
                "the sealed base repeats a Lance native row ID",
            ));
        }
    }
    let mut current_native_ids = std::collections::BTreeSet::new();
    for row in current {
        if !current_native_ids.insert(row.native_row_id) {
            return Err(seal_refused("the successor repeats a Lance native row ID"));
        }
        if let Some(base_row) = base_by_native_id.get(&row.native_row_id) {
            if base_row.record_id != row.record_id || base_row.leaf_digest != row.leaf_digest {
                return Err(seal_refused(
                    "an inherited physical row changed identity or payload without a new Lance row ID",
                ));
            }
        }
    }
    let mut base_rows = base.iter().peekable();
    let mut current_rows = current.iter().peekable();
    let mut deleted = 0_u64;
    let mut added = 0_u64;
    let mut count = |is_deleted: bool| -> Result<(), CoreError> {
        let slot = if is_deleted { &mut deleted } else { &mut added };
        *slot = slot
            .checked_add(1)
            .ok_or_else(|| seal_refused("row-change count overflow"))?;
        Ok(())
    };
    loop {
        match (base_rows.peek(), current_rows.peek()) {
            (None, None) => break,
            (Some(_), None) => {
                count(true)?;
                let _removed = base_rows.next();
            }
            (None, Some(_)) => {
                count(false)?;
                let _added = current_rows.next();
            }
            (Some(base_row), Some(current_row)) => {
                match base_row.record_id.cmp(&current_row.record_id) {
                    std::cmp::Ordering::Less => {
                        count(true)?;
                        let _removed = base_rows.next();
                    }
                    std::cmp::Ordering::Greater => {
                        count(false)?;
                        let _added = current_rows.next();
                    }
                    std::cmp::Ordering::Equal => {
                        if base_row.native_row_id == current_row.native_row_id {
                            if base_row.leaf_digest != current_row.leaf_digest {
                                return Err(seal_refused(
                                    "an inherited physical row changed payload without a new Lance row ID",
                                ));
                            }
                        } else {
                            count(true)?;
                            count(false)?;
                        }
                        let _base = base_rows.next();
                        let _current = current_rows.next();
                    }
                }
            }
        }
    }
    Ok((deleted, added))
}

/// Bind the index's lost coverage to actual canonical row changes.
/// Index loss with unchanged rows cannot masquerade as a delete, and newly
/// inserted or replaced rows must be exactly the rows the index calls pending.
fn verify_contraction_row_changes_v1(
    ann: &AnnIndexSealV1,
    report: IndexReportV1,
    row_count: u64,
    base_rows: &[SemanticRowFingerprintV1],
    current_rows: &[SemanticRowFingerprintV1],
) -> Result<(), CoreError> {
    if report.indexed_rows.checked_add(report.unindexed_rows) != Some(row_count) {
        return Err(seal_refused(&format!(
            "the contracted index reports {} indexed and {} unindexed rows, the dataset holds {row_count}",
            report.indexed_rows, report.unindexed_rows
        )));
    }
    let Some(indexed_decrease) = ann.indexed_rows.checked_sub(report.indexed_rows) else {
        return Err(seal_refused(
            "the contracted index covers more indexed rows than the sealed base",
        ));
    };
    if indexed_decrease == 0 {
        return Err(seal_refused(
            "the inherited segment count contracted without a positive indexed-row deletion",
        ));
    }
    if u64::try_from(base_rows.len()) != Ok(ann.indexed_rows)
        || u64::try_from(current_rows.len()) != Ok(row_count)
    {
        return Err(seal_refused(
            "the canonical row fingerprint counts disagree with the base or current row count",
        ));
    }
    let (deleted_rows, added_rows) = row_change_counts_v1(base_rows, current_rows)?;
    if deleted_rows != indexed_decrease || added_rows != report.unindexed_rows {
        return Err(seal_refused(&format!(
            "the contracted index reports {indexed_decrease} removed indexed and {} unindexed rows, but canonical row changes prove {deleted_rows} removed and {added_rows} new rows",
            report.unindexed_rows
        )));
    }
    Ok(())
}

/// A deletion can retire a complete inherited segment. A contracted index
/// cannot retain the old lineage: a caller must verify the sealed base and
/// retrain the current live rows before publishing a successor seal.
///
/// The remaining segment identities and parameters must be an ordered subset
/// of the base's exact record. A changed or newly introduced segment is a
/// disagreement with the base, not a deletion-only contraction.
pub(crate) async fn inherited_index_contracted_v1(
    table: &lancedb::Table,
    inherited: &VectorIndexSealV1,
    row_count: u64,
    base_rows: &[SemanticRowFingerprintV1],
    current_rows: &[SemanticRowFingerprintV1],
) -> Result<bool, CoreError> {
    let Some(ann) = inherited.ann.as_ref() else {
        return Ok(false);
    };
    if !matches!(
        plan_for_rows_v1(row_count),
        VectorIndexPlanV1::IvfHnswSq { .. }
    ) {
        return Ok(false);
    }
    let report = read_index_report_v1(table, "before contracted-index seal").await?;
    if report.segments >= ann.index_segments {
        return Ok(false);
    }
    verify_contraction_row_changes_v1(ann, report, row_count, base_rows, current_rows)?;
    let present = table
        .list_indices()
        .await
        .map_err(|err| lancedb_err("list_indices before contracted-index seal", err))?;
    let present = vector_indices(&present)?;
    if present.len() != 1
        || present.first().is_none_or(|index| {
            index.name != ann.index_name || index.index_type != IndexType::IvfHnswSq
        })
    {
        return Err(seal_refused(
            "the contracted dataset does not carry exactly the inherited vector index",
        ));
    }
    let segments =
        read_index_segments_v1(table, VECTOR_INDEX_NAME, "before contracted-index seal").await?;
    if u32::try_from(segments.len()).ok() != Some(report.segments) {
        return Err(seal_refused(
            "the contracted index report and segment identity list disagree",
        ));
    }
    let mut inherited_segments = ann.segments.iter();
    for segment in &segments {
        if !inherited_segments.any(|sealed| sealed == segment) {
            return Err(seal_refused(&format!(
                "the contracted index carries a segment absent from the sealed base: {segment:?}"
            )));
        }
    }
    Ok(true)
}

/// The policy's seal record for an index the library reports as covering
/// `report` rows in `segments`, with `lineage` behind it.
fn ann_seal_record_v1(
    num_partitions: u32,
    report: IndexReportV1,
    lineage: AnnIndexLineageV1,
    segments: Vec<AnnIndexSegmentSealV1>,
) -> VectorIndexSealV1 {
    VectorIndexSealV1 {
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
            indexed_rows: report.indexed_rows,
            index_segments: report.segments,
            nprobes: NPROBES.min(num_partitions),
            ef_floor: EF_FLOOR,
            ef_per_candidate: EF_PER_CANDIDATE,
            refine_factor: REFINE_FACTOR,
            lineage,
            segments,
        }),
    }
}

/// The lineage a fresh train at `generation` over `rows` rows records.
const fn trained_lineage_v1(generation: u64, rows: u64) -> AnnIndexLineageV1 {
    AnnIndexLineageV1 {
        trained_at_generation: generation,
        trained_rows: rows,
        appended_rows: 0,
        deleted_rows: 0,
        append_ratio_max_per_mille: ANN_APPEND_RATIO_MAX_PER_MILLE,
        append_rows_max: ANN_APPEND_ROWS_MAX,
        append_segments_max: ANN_APPEND_SEGMENTS_MAX,
    }
}

/// Whether an inherited index is the one this policy would have built for
/// `num_partitions` partitions, by the library this policy runs.
fn inherited_matches_policy_v1(
    inherited: &VectorIndexSealV1,
    ann: &AnnIndexSealV1,
    num_partitions: u32,
) -> bool {
    inherited.library == ANN_LIBRARY
        && inherited.library_version == ANN_LIBRARY_VERSION
        && ann.index_name == VECTOR_INDEX_NAME
        && ann.distance == "cosine"
        && ann.num_partitions == num_partitions
        && ann.sample_rate == IVF_SAMPLE_RATE
        && ann.max_iterations == IVF_MAX_ITERATIONS
        && ann.hnsw_m == HNSW_M
        && ann.hnsw_ef_construction == HNSW_EF_CONSTRUCTION
}

/// An append the policy admits: what to add and what the record becomes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AppendPlanV1 {
    num_partitions: u32,
    /// The live rows the appended index must cover.
    row_count: u64,
    /// Rows in fragments the inherited index does not cover yet.
    pending_rows: u64,
    /// The segment count the library must report once they are added.
    expected_segments: u32,
    lineage: AnnIndexLineageV1,
}

/// Decide whether this seal appends to the index it inherited.
///
/// Returns `None` when the policy says to train instead: no inherited
/// contract, an exact one, a recipe or library version other than this
/// policy's, or an append budget the new rows would exceed. Refuses
/// outright when the dataset disagrees with the inherited contract,
/// because the seal would then be recording a lineage for an index it
/// cannot vouch for.
async fn append_plan_v1(
    table: &lancedb::Table,
    input: VectorIndexSealInputV1<'_>,
    present: &[VectorIndexListingV1],
    num_partitions: u32,
) -> Result<Option<AppendPlanV1>, CoreError> {
    let Some(inherited) = input.inherited else {
        return Ok(None);
    };
    let Some(ann) = inherited.ann.as_ref() else {
        if let Some(unsealed) = present.first() {
            return Err(seal_refused(&format!(
                "the inherited dataset carries vector index `{}` but the base generation sealed an exact lane",
                unsealed.name
            )));
        }
        return Ok(None);
    };
    let base_lineage = ann.lineage;
    let names: Vec<&str> = present
        .iter()
        .map(|listing| listing.name.as_str())
        .collect();
    if names != [ann.index_name.as_str()]
        || present
            .iter()
            .any(|listing| listing.index_type != IndexType::IvfHnswSq)
    {
        return Err(seal_refused(&format!(
            "the base generation sealed vector index `{}` but the inherited dataset lists {names:?}",
            ann.index_name
        )));
    }
    let before = read_index_report_v1(table, "before the append").await?;
    if before.segments != ann.index_segments {
        return Err(seal_refused(&format!(
            "the inherited index has {} segments, the base generation sealed {}",
            before.segments, ann.index_segments
        )));
    }
    let inherited_segments =
        read_index_segments_v1(table, VECTOR_INDEX_NAME, "before the append").await?;
    if inherited_segments != ann.segments {
        return Err(seal_refused(&format!(
            "the inherited index segments {inherited_segments:?} differ from the base seal {:?}",
            ann.segments
        )));
    }
    if before.indexed_rows.saturating_add(before.unindexed_rows) != input.row_count {
        return Err(seal_refused(&format!(
            "the inherited index reports {} indexed and {} unindexed rows, the dataset holds {}",
            before.indexed_rows, before.unindexed_rows, input.row_count
        )));
    }
    // A policy or library change may retrain, but it cannot legitimize a
    // physical index that no longer matches the sealed base.
    if !inherited_matches_policy_v1(inherited, ann, num_partitions) {
        return Ok(None);
    }
    let Some(deleted_now) = ann.indexed_rows.checked_sub(before.indexed_rows) else {
        return Err(seal_refused(&format!(
            "the inherited index covers {} live rows, more than the {} the base generation sealed",
            before.indexed_rows, ann.indexed_rows
        )));
    };
    let (Some(appended_rows), Some(deleted_rows)) = (
        base_lineage
            .appended_rows
            .checked_add(before.unindexed_rows),
        base_lineage.deleted_rows.checked_add(deleted_now),
    ) else {
        return Err(seal_refused("the lineage counters overflow"));
    };
    let lineage = AnnIndexLineageV1 {
        appended_rows,
        deleted_rows,
        ..trained_lineage_v1(
            base_lineage.trained_at_generation,
            base_lineage.trained_rows,
        )
    };
    let expected_segments = if before.unindexed_rows == 0 {
        before.segments
    } else {
        before.segments.saturating_add(1)
    };
    let Some(within_row_budget) = lineage.within_append_budget() else {
        return Err(seal_refused("the append budget arithmetic overflows"));
    };
    let within_budget =
        within_row_budget && expected_segments <= ANN_APPEND_SEGMENTS_MAX.saturating_add(1);
    Ok(within_budget.then_some(AppendPlanV1 {
        num_partitions,
        row_count: input.row_count,
        pending_rows: before.unindexed_rows,
        expected_segments,
        lineage,
    }))
}

/// Assign the pending rows to the inherited centroids as one more segment
/// and record the widened lineage.
///
/// The library builds the new segment's graph with its own incremental
/// builder, whose construction parameters are not settable through this
/// version's optimize surface; what it built with is read back from the
/// dataset and recorded per segment, and the seal pins the library
/// version so a bump retrains under this policy's parameters rather than
/// appending under changed ones. A seal with nothing pending changes
/// nothing in the dataset and only advances the deletion counter.
async fn append_to_inherited_v1(
    table: &lancedb::Table,
    plan: AppendPlanV1,
) -> Result<VectorIndexSealV1, CoreError> {
    if plan.pending_rows > 0 {
        let _stats = table
            .optimize(OptimizeAction::Index(
                OptimizeOptions::append().index_names(vec![VECTOR_INDEX_NAME.to_string()]),
            ))
            .await
            .map_err(|err| lancedb_err("optimize Index(append) IvfHnswSq(cosine)", err))?;
    }
    let after = read_index_report_v1(table, "after the append").await?;
    if after.indexed_rows != plan.row_count || after.unindexed_rows != 0 {
        return Err(seal_refused(&format!(
            "the appended index covers {} of {} rows ({} unindexed)",
            after.indexed_rows, plan.row_count, after.unindexed_rows
        )));
    }
    if plan.lineage.accounted_rows() != Some(plan.row_count) {
        return Err(seal_refused(&format!(
            "the lineage accounts for {:?} rows, the appended index covers {}",
            plan.lineage.accounted_rows(),
            plan.row_count
        )));
    }
    if after.segments != plan.expected_segments {
        return Err(seal_refused(&format!(
            "the appended index has {} segments, expected {}",
            after.segments, plan.expected_segments
        )));
    }
    let segments = read_index_segments_v1(table, VECTOR_INDEX_NAME, "after the append").await?;
    Ok(ann_seal_record_v1(
        plan.num_partitions,
        after,
        plan.lineage,
        segments,
    ))
}

/// Drop every vector index the dataset carries and train the policy's.
async fn train_v1(
    table: &lancedb::Table,
    present: &[VectorIndexListingV1],
    generation: u64,
    row_count: u64,
    num_partitions: u32,
) -> Result<VectorIndexSealV1, CoreError> {
    drop_vector_indices_v1(table, present).await?;
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
    let report = read_index_report_v1(table, "after the train").await?;
    if report.indexed_rows != row_count || report.unindexed_rows != 0 {
        return Err(seal_refused(&format!(
            "the trained index covers {} of {row_count} rows ({} unindexed)",
            report.indexed_rows, report.unindexed_rows
        )));
    }
    let segments = read_index_segments_v1(table, VECTOR_INDEX_NAME, "after the train").await?;
    let trained_as_built = segments.first().is_some_and(|segment| {
        segment.hnsw_m == HNSW_M && segment.hnsw_ef_construction == HNSW_EF_CONSTRUCTION
    });
    if segments.len() != 1 || !trained_as_built {
        return Err(seal_refused(&format!(
            "the library reports the trained index as {segments:?}, not one segment built with m={HNSW_M} ef_construction={HNSW_EF_CONSTRUCTION}"
        )));
    }
    Ok(ann_seal_record_v1(
        num_partitions,
        report,
        trained_lineage_v1(generation, row_count),
        segments,
    ))
}

/// Drop every listed vector index; the library drops all of a name's
/// segments at once.
async fn drop_vector_indices_v1(
    table: &lancedb::Table,
    present: &[VectorIndexListingV1],
) -> Result<(), CoreError> {
    for listing in present {
        table
            .drop_index(&listing.name)
            .await
            .map_err(|err| lancedb_err("drop inherited vector index", err))?;
    }
    Ok(())
}

/// Seal the dense lane of `table`.
///
/// Below the row floor the lane is exact and every inherited vector index
/// is dropped. At or above it, a delta appends to the inherited index when
/// [`append_plan_v1`] admits it and trains otherwise; a fresh seal trains.
pub(crate) async fn seal_vector_index_v1(
    table: &lancedb::Table,
    input: VectorIndexSealInputV1<'_>,
) -> Result<VectorIndexSealV1, CoreError> {
    let configs = table
        .list_indices()
        .await
        .map_err(|err| lancedb_err("list_indices before seal", err))?;
    let present = vector_indices(&configs)?;
    let VectorIndexPlanV1::IvfHnswSq { num_partitions } = plan_for_rows_v1(input.row_count) else {
        drop_vector_indices_v1(table, &present).await?;
        return Ok(VectorIndexSealV1 {
            mode: VECTOR_INDEX_MODE_EXACT.to_string(),
            library: ANN_LIBRARY.to_string(),
            library_version: ANN_LIBRARY_VERSION.to_string(),
            index_min_rows: VECTOR_INDEX_MIN_ROWS,
            ann: None,
        });
    };
    match append_plan_v1(table, input, &present, num_partitions).await? {
        Some(plan) => append_to_inherited_v1(table, plan).await,
        None => {
            train_v1(
                table,
                &present,
                input.generation,
                input.row_count,
                num_partitions,
            )
            .await
        }
    }
}

/// How an opened generation runs its dense lane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LoadedVectorIndexV1 {
    /// The approximate index the lane runs through; `None` for an exact
    /// lane.
    approximate: Option<LoadedApproximateIndexV1>,
    attestation: DenseLaneAttestationV1,
}

/// The approximate index an opened generation serves: the effort a query
/// spends in it, where its centroids came from, and how its segments were
/// built.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LoadedApproximateIndexV1 {
    effort: QueryEffortV1,
    lineage: DenseIndexTrainingV1,
    build: DenseIndexBuildV1,
}

impl LoadedApproximateIndexV1 {
    fn from_seal(ann: &AnnIndexSealV1) -> Self {
        Self {
            effort: QueryEffortV1::from_seal(ann),
            lineage: DenseIndexTrainingV1 {
                trained_at_generation: ann.lineage.trained_at_generation,
                trained_rows: ann.lineage.trained_rows,
                appended_rows: ann.lineage.appended_rows,
                deleted_rows: ann.lineage.deleted_rows,
            },
            build: DenseIndexBuildV1 {
                hnsw_m: ann.hnsw_m,
                hnsw_ef_construction: ann.hnsw_ef_construction,
                appended_segments: ann
                    .appended_segments()
                    .iter()
                    .map(|segment| DenseIndexSegmentBuildV1 {
                        hnsw_m: segment.hnsw_m,
                        hnsw_ef_construction: segment.hnsw_ef_construction,
                    })
                    .collect(),
            },
        }
    }
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
        match self.approximate.as_ref() {
            Some(approximate) => approximate.effort.apply(query, top_k),
            None => Ok(query.bypass_vector_index()),
        }
    }

    /// Which lane a query runs through, as the budget checkpoint names it.
    pub(crate) const fn lane_kind(&self) -> DenseLaneKindV1 {
        if self.approximate.is_some() {
            DenseLaneKindV1::Approximate
        } else {
            DenseLaneKindV1::Exact
        }
    }

    pub(crate) fn contract(&self) -> DenseLaneContractV1 {
        DenseLaneContractV1 {
            index: self
                .approximate
                .as_ref()
                .map_or(DenseIndexV1::Exact, |approximate| {
                    DenseIndexV1::Approximate {
                        effort: approximate.effort.contract(),
                        lineage: approximate.lineage,
                        build: approximate.build.clone(),
                    }
                }),
            attestation: self.attestation,
        }
    }
}

fn ann_missing(detail: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::AnnIndexMissing,
        message: format!("semantic: {detail}"),
    }
}

fn ann_incompatible(detail: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::AnnIndexIncompatible,
        message: format!("semantic: {detail}"),
    }
}

/// Verify the dataset's vector indices against the seal.
///
/// A sealed exact lane must have no vector index (an index the seal did not
/// record would silently change the served mode); a sealed approximate lane
/// must have exactly the recorded index, reported by the library with the
/// recorded type, distance, coverage, segment count and per-segment build
/// parameters. The lineage the seal recorded (already checked against that
/// coverage when the manifest was validated) is carried into the served
/// contract as it stands.
pub(crate) async fn verify_vector_index_v1(
    table: &lancedb::Table,
    seal: &VectorIndexSealV1,
    row_count: u64,
) -> Result<LoadedVectorIndexV1, CoreError> {
    let configs = table
        .list_indices()
        .await
        .map_err(|err| lancedb_err("list_indices at open", err))?;
    let present = vector_indices(&configs)?;
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
            approximate: None,
            attestation,
        });
    };
    let Some(listing) = present
        .iter()
        .find(|listing| listing.name == ann.index_name)
    else {
        let names: Vec<&str> = present
            .iter()
            .map(|listing| listing.name.as_str())
            .collect();
        return Err(ann_missing(&format!(
            "the seal recorded vector index `{}` but the dataset lists {names:?}",
            ann.index_name
        )));
    };
    if present.len() != 1 {
        let names: Vec<&str> = present
            .iter()
            .map(|listing| listing.name.as_str())
            .collect();
        return Err(ann_incompatible(&format!(
            "the seal recorded one vector index but the dataset carries {names:?}"
        )));
    }
    if listing.index_type != IndexType::IvfHnswSq {
        return Err(ann_incompatible(&format!(
            "vector index `{}` is {} in the dataset, ivf_hnsw_sq in the seal",
            ann.index_name,
            index_type_token(&listing.index_type)
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
    let built = read_index_segments_v1(table, &ann.index_name, "at open").await?;
    if built != ann.segments {
        return Err(ann_incompatible(&format!(
            "vector index `{}` segments were built as {built:?}, the seal recorded {:?}",
            ann.index_name, ann.segments
        )));
    }
    Ok(LoadedVectorIndexV1 {
        approximate: Some(LoadedApproximateIndexV1::from_seal(ann)),
        attestation,
    })
}
#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning tests assert with `assert!` on fixture invariants; a violated fixture invariant is not a propagatable error"
)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use arrow_array::{Array, FixedSizeListArray, Float32Array, Int64Array, RecordBatch};
    use arrow_schema::{DataType, Field, Schema};
    use futures::TryStreamExt as _;
    use lancedb::query::{ExecutableQuery as _, QueryBase as _};
    use quanta_index_core::CoreError;
    use quanta_index_core::domains::semantic::{
        DenseIndexBuildV1, DenseIndexSegmentBuildV1, DenseIndexTrainingV1, DenseIndexV1,
        DenseLaneAttestationV1,
    };

    use super::{
        ANN_APPEND_RATIO_MAX_PER_MILLE, ANN_APPEND_ROWS_MAX, ANN_APPEND_SEGMENTS_MAX,
        ANN_LIBRARY_VERSION, HNSW_EF_CONSTRUCTION, HNSW_M, IndexReportV1, LoadedVectorIndexV1,
        MAX_PARTITIONS, SemanticRowFingerprintV1, TARGET_PARTITION_ROWS, VECTOR_INDEX_MIN_ROWS,
        VECTOR_INDEX_NAME, VectorIndexPlanV1, VectorIndexSealInputV1,
        inherited_index_contracted_v1, plan_for_rows_v1, read_index_segments_v1,
        seal_vector_index_v1, verify_contraction_row_changes_v1, verify_vector_index_v1,
    };

    /// The graph parameters the library's incremental builder uses for a
    /// segment appended by `optimize(Index(append))`.
    ///
    /// They are its own defaults, not this policy's recipe. Pinned here so a library bump that
    /// changes them fails loudly, and read back from the dataset in the
    /// tests below so the pin is checked against the library, not assumed.
    const LIBRARY_INCREMENTAL_HNSW_M: u32 = 20;
    const LIBRARY_INCREMENTAL_HNSW_EF_CONSTRUCTION: u32 = 150;
    use crate::layout::COLUMN_VECTOR;
    use crate::manifest::{
        AnnIndexLineageV1, AnnIndexSegmentSealV1, VECTOR_INDEX_MODE_EXACT, VectorIndexSealV1,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const DIMENSION: usize = 8;
    const COLUMN_ID: &str = "id";

    /// A deterministic unit direction for `seed`.
    ///
    /// An xorshift stream mapped to `[-1, 1)` and normalized. Random
    /// directions are well separated, so a row's own vector is
    /// unambiguously its nearest neighbour even through a scalar-quantized
    /// index, and the exact ranking this module computes is a fair oracle
    /// for the library's.
    fn unit_vector(seed: u64, dimension: usize) -> Vec<f32> {
        let mut state = seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(0xD1B5_4A32_D192_ED03);
        let mut vector = Vec::with_capacity(dimension);
        for _ in 0..dimension {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let lane = u16::try_from(state & 0xFFFF).map_or(0.0_f32, f32::from);
            vector.push(lane / 32_768.0_f32 - 1.0);
        }
        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        vector.iter().map(|value| value / norm).collect()
    }

    fn schema(dimension: usize) -> Result<Arc<Schema>, Box<dyn std::error::Error>> {
        Ok(Arc::new(Schema::new(vec![
            Field::new(COLUMN_ID, DataType::Int64, false),
            Field::new(
                COLUMN_VECTOR,
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    i32::try_from(dimension)?,
                ),
                false,
            ),
        ])))
    }

    /// Rows `ids`, each carrying [`unit_vector`] of its id.
    fn rows_batch(
        ids: std::ops::Range<u64>,
        dimension: usize,
    ) -> Result<RecordBatch, Box<dyn std::error::Error>> {
        let mut flat = Vec::with_capacity(
            usize::try_from(ids.end.saturating_sub(ids.start))?.saturating_mul(dimension),
        );
        let mut id_values = Vec::new();
        for id in ids {
            flat.extend(unit_vector(id, dimension));
            id_values.push(i64::try_from(id)?);
        }
        let values: Arc<dyn Array> = Arc::new(Float32Array::from(flat));
        let vectors = FixedSizeListArray::try_new(
            Arc::new(Field::new("item", DataType::Float32, true)),
            i32::try_from(dimension)?,
            values,
            None,
        )?;
        Ok(RecordBatch::try_new(
            schema(dimension)?,
            vec![Arc::new(Int64Array::from(id_values)), Arc::new(vectors)],
        )?)
    }

    /// A table holding rows `0..rows`: the smallest dataset the seal and
    /// the verifier can be exercised on.
    async fn vector_table(
        root: &std::path::Path,
        rows: u64,
        dimension: usize,
    ) -> Result<lancedb::Table, Box<dyn std::error::Error>> {
        let batch = rows_batch(0..rows, dimension)?;
        let connection = lancedb::connect(&root.to_string_lossy()).execute().await?;
        let table = connection.create_table("semantic", batch).execute().await?;
        Ok(table)
    }

    async fn append_rows(
        table: &lancedb::Table,
        ids: std::ops::Range<u64>,
        dimension: usize,
    ) -> TestResult {
        let batch = rows_batch(ids, dimension)?;
        let _added = table.add(batch).execute().await?;
        Ok(())
    }

    async fn delete_ids(table: &lancedb::Table, ids: &[u64]) -> TestResult {
        let list = ids
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let _deleted = table.delete(&format!("{COLUMN_ID} IN ({list})")).await?;
        Ok(())
    }

    async fn row_count(table: &lancedb::Table) -> Result<u64, Box<dyn std::error::Error>> {
        Ok(u64::try_from(table.count_rows(None).await?)?)
    }

    /// Top-`k` ids the lane returns for `query` under `loaded`'s effort.
    async fn search_ids(
        table: &lancedb::Table,
        loaded: &LoadedVectorIndexV1,
        query: &[f32],
        top_k: usize,
    ) -> Result<Vec<u64>, Box<dyn std::error::Error>> {
        let vector_query = loaded.apply(
            table
                .vector_search(query.to_vec())?
                .distance_type(lancedb::DistanceType::Cosine)
                .limit(top_k),
            top_k,
        )?;
        let batches: Vec<RecordBatch> = vector_query.execute().await?.try_collect().await?;
        let mut ids = Vec::with_capacity(top_k);
        for batch in batches {
            let column = batch
                .column_by_name(COLUMN_ID)
                .and_then(|column| column.as_any().downcast_ref::<Int64Array>())
                .ok_or("id column missing from the search result")?;
            for row in 0..column.len() {
                ids.push(u64::try_from(column.value(row))?);
            }
        }
        ids.truncate(top_k);
        Ok(ids)
    }

    /// Exact top-`k` ids by cosine over `ids`, computed here without the
    /// library.
    fn exact_top_k(ids: &BTreeSet<u64>, dimension: usize, query: &[f32], k: usize) -> Vec<u64> {
        let mut scored: Vec<(u64, f32)> = ids
            .iter()
            .map(|id| {
                let score = unit_vector(*id, dimension)
                    .iter()
                    .zip(query.iter())
                    .map(|(a, b)| a * b)
                    .sum::<f32>();
                (*id, score)
            })
            .collect();
        scored.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| left.0.cmp(&right.0))
        });
        scored.truncate(k);
        scored.into_iter().map(|(id, _)| id).collect()
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

    fn typed_code(
        result: &Result<LoadedVectorIndexV1, CoreError>,
    ) -> Option<quanta_index_contract::SearchPlaneErrorCodeV2> {
        match result {
            Err(CoreError::Typed { code, .. }) => Some(*code),
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

    /// Seal `table` as generation `generation`, a fresh one without a base.
    async fn seal_fresh(
        table: &lancedb::Table,
        generation: u64,
    ) -> Result<VectorIndexSealV1, CoreError> {
        let rows = row_count(table)
            .await
            .map_err(|err| CoreError::Storage(err.to_string()))?;
        seal_vector_index_v1(
            table,
            VectorIndexSealInputV1 {
                generation,
                row_count: rows,
                inherited: None,
            },
        )
        .await
    }

    /// Seal `table` as generation `generation`, a delta over `base`.
    async fn seal_delta(
        table: &lancedb::Table,
        generation: u64,
        base: &VectorIndexSealV1,
    ) -> Result<VectorIndexSealV1, CoreError> {
        let rows = row_count(table)
            .await
            .map_err(|err| CoreError::Storage(err.to_string()))?;
        seal_vector_index_v1(
            table,
            VectorIndexSealInputV1 {
                generation,
                row_count: rows,
                inherited: Some(base),
            },
        )
        .await
    }

    fn lineage_of(
        seal: &VectorIndexSealV1,
    ) -> Result<AnnIndexLineageV1, Box<dyn std::error::Error>> {
        seal.ann
            .as_ref()
            .map(|ann| ann.lineage)
            .ok_or_else(|| "the seal carries no lineage".into())
    }

    fn report_of(seal: &VectorIndexSealV1) -> Option<(u64, u32)> {
        seal.ann
            .as_ref()
            .map(|ann| (ann.indexed_rows, ann.index_segments))
    }

    /// What the library itself reports for the policy index: indexed
    /// rows, unindexed rows, segments.
    async fn library_report(
        table: &lancedb::Table,
    ) -> Result<(usize, usize, Option<u32>), Box<dyn std::error::Error>> {
        let statistics = table
            .index_stats(VECTOR_INDEX_NAME)
            .await?
            .ok_or("the policy index reports no statistics")?;
        Ok((
            statistics.num_indexed_rows,
            statistics.num_unindexed_rows,
            statistics.num_indices,
        ))
    }

    #[test]
    fn the_verifier_refuses_every_disagreement_between_seal_and_dataset() -> TestResult {
        run(async {
            let temp = tempfile::tempdir()?;
            let table = vector_table(temp.path(), VECTOR_INDEX_MIN_ROWS, DIMENSION).await?;
            let sealed = seal_fresh(&table, 1).await?;
            let ann = sealed.ann.clone().ok_or("the floor seals an index")?;

            // The dataset agrees with its own seal.
            let loaded = verify_vector_index_v1(&table, &sealed, VECTOR_INDEX_MIN_ROWS).await?;
            assert!(loaded.approximate.is_some());
            assert_eq!(loaded.attestation, DenseLaneAttestationV1::Sealed);

            // A seal from another version of the same library is served, and
            // says so; one from another library is not served at all.
            let mut other_version = sealed.clone();
            other_version.library_version = "0.0.1".to_string();
            let served =
                verify_vector_index_v1(&table, &other_version, VECTOR_INDEX_MIN_ROWS).await?;
            assert_eq!(
                served.attestation,
                DenseLaneAttestationV1::SealedByAnotherLibraryVersion
            );
            let mut other_library = sealed.clone();
            other_library.library = "faiss".to_string();
            let refused =
                verify_vector_index_v1(&table, &other_library, VECTOR_INDEX_MIN_ROWS).await;
            assert_eq!(
                typed_code(&refused),
                Some(quanta_index_contract::SearchPlaneErrorCodeV2::AnnIndexIncompatible)
            );

            // An exact seal over an indexed dataset: the served mode would not
            // be the sealed one.
            let indexed_but_exact =
                verify_vector_index_v1(&table, &exact_seal(), VECTOR_INDEX_MIN_ROWS).await;
            assert_eq!(
                typed_code(&indexed_but_exact),
                Some(quanta_index_contract::SearchPlaneErrorCodeV2::AnnIndexIncompatible)
            );

            // A seal whose coverage or segment count the library contradicts.
            let mut fewer_rows = sealed.clone();
            if let Some(record) = fewer_rows.ann.as_mut() {
                record.indexed_rows = record.indexed_rows.saturating_sub(1);
            }
            let coverage = verify_vector_index_v1(&table, &fewer_rows, VECTOR_INDEX_MIN_ROWS).await;
            assert_eq!(
                typed_code(&coverage),
                Some(quanta_index_contract::SearchPlaneErrorCodeV2::AnnIndexIncompatible)
            );
            let mut more_segments = sealed.clone();
            if let Some(record) = more_segments.ann.as_mut() {
                record.index_segments = record.index_segments.saturating_add(1);
            }
            let segments =
                verify_vector_index_v1(&table, &more_segments, VECTOR_INDEX_MIN_ROWS).await;
            assert_eq!(
                typed_code(&segments),
                Some(quanta_index_contract::SearchPlaneErrorCodeV2::AnnIndexIncompatible)
            );

            // A seal naming an index the dataset does not list.
            let mut renamed = sealed.clone();
            if let Some(record) = renamed.ann.as_mut() {
                record.index_name = "vector_idx".to_string();
            }
            let missing_name =
                verify_vector_index_v1(&table, &renamed, VECTOR_INDEX_MIN_ROWS).await;
            assert_eq!(
                typed_code(&missing_name),
                Some(quanta_index_contract::SearchPlaneErrorCodeV2::AnnIndexMissing)
            );

            // The index gone from the dataset while the seal still records it.
            table.drop_index(&ann.index_name).await?;
            let dropped = verify_vector_index_v1(&table, &sealed, VECTOR_INDEX_MIN_ROWS).await;
            assert_eq!(
                typed_code(&dropped),
                Some(quanta_index_contract::SearchPlaneErrorCodeV2::AnnIndexMissing)
            );
            // ... and the same dataset agrees with an exact seal again.
            let exact =
                verify_vector_index_v1(&table, &exact_seal(), VECTOR_INDEX_MIN_ROWS).await?;
            assert_eq!(
                exact,
                LoadedVectorIndexV1 {
                    approximate: None,
                    attestation: DenseLaneAttestationV1::Sealed,
                }
            );
            Ok(())
        })
    }

    #[test]
    fn the_seal_replaces_an_inherited_index_of_any_name() -> TestResult {
        run(async {
            let temp = tempfile::tempdir()?;
            let table = vector_table(temp.path(), VECTOR_INDEX_MIN_ROWS, DIMENSION).await?;
            // An index a base generation built under a different name and
            // policy, as a delta inherits it from a base that sealed nothing.
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
            let sealed = seal_fresh(&table, 1).await?;
            let names: Vec<String> = table
                .list_indices()
                .await?
                .into_iter()
                .map(|config| config.name)
                .collect();
            assert_eq!(names, vec![VECTOR_INDEX_NAME.to_string()]);
            assert_eq!(report_of(&sealed), Some((VECTOR_INDEX_MIN_ROWS, 1)));
            // Below the floor the seal drops what it inherited and seals exact.
            let small_root = tempfile::tempdir()?;
            let below = VECTOR_INDEX_MIN_ROWS.saturating_sub(1);
            let small = vector_table(small_root.path(), below, DIMENSION).await?;
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
            let exact = seal_fresh(&small, 1).await?;
            assert_eq!(exact.mode, VECTOR_INDEX_MODE_EXACT);
            assert!(small.list_indices().await?.is_empty());
            Ok(())
        })
    }

    /// (i) A fresh seal trains and records itself as the lineage's origin.
    #[test]
    fn a_fresh_seal_trains_and_is_its_own_lineage() -> TestResult {
        run(async {
            let temp = tempfile::tempdir()?;
            let table = vector_table(temp.path(), 300, DIMENSION).await?;
            let sealed = seal_fresh(&table, 4).await?;
            assert_eq!(report_of(&sealed), Some((300, 1)));
            assert_eq!(
                lineage_of(&sealed)?,
                AnnIndexLineageV1 {
                    trained_at_generation: 4,
                    trained_rows: 300,
                    appended_rows: 0,
                    deleted_rows: 0,
                    append_ratio_max_per_mille: ANN_APPEND_RATIO_MAX_PER_MILLE,
                    append_rows_max: ANN_APPEND_ROWS_MAX,
                    append_segments_max: ANN_APPEND_SEGMENTS_MAX,
                }
            );
            assert_eq!(library_report(&table).await?, (300, 0, Some(1)));
            Ok(())
        })
    }

    /// (ii) A delta inside the budget appends.
    ///
    /// The record names the base's train, counts the new rows, and the
    /// library covers every row in one more segment. (vi) Rows tombstoned
    /// by the delta are counted, absent from the results, and never block
    /// the append.
    #[test]
    fn a_delta_inside_the_budget_appends_to_the_inherited_index() -> TestResult {
        run(async {
            let temp = tempfile::tempdir()?;
            let table = vector_table(temp.path(), 400, DIMENSION).await?;
            let base = seal_fresh(&table, 1).await?;

            // The delta tombstones four base rows and adds sixty: 60 of 400 is
            // 150 per mille, inside the quarter.
            let deleted = [3_u64, 77, 150, 399];
            delete_ids(&table, &deleted).await?;
            append_rows(&table, 1_000..1_060, DIMENSION).await?;
            let delta = seal_delta(&table, 2, &base).await?;

            assert_eq!(report_of(&delta), Some((456, 2)));
            assert_eq!(
                lineage_of(&delta)?,
                AnnIndexLineageV1 {
                    trained_at_generation: 1,
                    trained_rows: 400,
                    appended_rows: 60,
                    deleted_rows: 4,
                    ..lineage_of(&base)?
                }
            );
            assert_eq!(library_report(&table).await?, (456, 0, Some(2)));

            // The open serves the appended index with its lineage.
            let loaded = verify_vector_index_v1(&table, &delta, 456).await?;
            let DenseIndexV1::Approximate { lineage, build, .. } = loaded.contract().index else {
                return Err("the appended index is approximate".into());
            };
            assert_eq!(
                lineage,
                DenseIndexTrainingV1 {
                    trained_at_generation: 1,
                    trained_rows: 400,
                    appended_rows: 60,
                    deleted_rows: 4,
                }
            );
            // The appended segment was built by the library's incremental
            // builder under its own parameters; the attestation says so
            // instead of claiming the trained recipe for it.
            assert_eq!(
                build,
                DenseIndexBuildV1 {
                    hnsw_m: HNSW_M,
                    hnsw_ef_construction: HNSW_EF_CONSTRUCTION,
                    appended_segments: vec![DenseIndexSegmentBuildV1 {
                        hnsw_m: LIBRARY_INCREMENTAL_HNSW_M,
                        hnsw_ef_construction: LIBRARY_INCREMENTAL_HNSW_EF_CONSTRUCTION,
                    }],
                }
            );

            // An appended row is served through the index, at the top for
            // its own vector; a tombstoned row is not served at all.
            let appended = search_ids(&table, &loaded, &unit_vector(1_042, DIMENSION), 3).await?;
            assert_eq!(appended.first(), Some(&1_042));
            for id in deleted {
                let hits = search_ids(&table, &loaded, &unit_vector(id, DIMENSION), 5).await?;
                assert!(!hits.contains(&id), "tombstoned row {id} served: {hits:?}");
                assert_eq!(hits.len(), 5);
            }

            // A further deletion-only delta appends nothing, changes no
            // segment, and only advances the deletion counter.
            delete_ids(&table, &[1_000, 1_001]).await?;
            let deletion_only = seal_delta(&table, 3, &delta).await?;
            assert_eq!(report_of(&deletion_only), Some((454, 2)));
            assert_eq!(
                lineage_of(&deletion_only)?,
                AnnIndexLineageV1 {
                    appended_rows: 60,
                    deleted_rows: 6,
                    ..lineage_of(&delta)?
                }
            );
            assert_eq!(library_report(&table).await?, (454, 0, Some(2)));
            let loaded = verify_vector_index_v1(&table, &deletion_only, 454).await?;
            for id in [1_000_u64, 1_001] {
                let hits = search_ids(&table, &loaded, &unit_vector(id, DIMENSION), 5).await?;
                assert!(
                    !hits.contains(&id),
                    "row {id} tombstoned after its append is still served: {hits:?}"
                );
                assert_eq!(hits.len(), 5);
            }
            Ok(())
        })
    }

    /// (iii) A delta whose new rows would exceed the ratio retrains: the
    /// record starts over at this generation.
    #[test]
    fn a_delta_beyond_the_ratio_retrains() -> TestResult {
        run(async {
            let temp = tempfile::tempdir()?;
            let table = vector_table(temp.path(), 400, DIMENSION).await?;
            let base = seal_fresh(&table, 1).await?;
            // 101 of 400 is 252.5 per mille, one row past the quarter.
            append_rows(&table, 1_000..1_101, DIMENSION).await?;
            let delta = seal_delta(&table, 2, &base).await?;
            assert_eq!(report_of(&delta), Some((501, 1)));
            assert_eq!(
                lineage_of(&delta)?,
                AnnIndexLineageV1 {
                    trained_at_generation: 2,
                    trained_rows: 501,
                    appended_rows: 0,
                    deleted_rows: 0,
                    ..lineage_of(&base)?
                }
            );
            assert_eq!(library_report(&table).await?, (501, 0, Some(1)));

            // The budget is cumulative: appends that each fit still retrain
            // once their sum would not.
            let second = {
                append_rows(&table, 2_000..2_100, DIMENSION).await?;
                seal_delta(&table, 3, &delta).await?
            };
            assert_eq!(lineage_of(&second)?.appended_rows, 100);
            assert_eq!(lineage_of(&second)?.trained_at_generation, 2);
            append_rows(&table, 3_000..3_030, DIMENSION).await?;
            let third = seal_delta(&table, 4, &second).await?;
            // 130 of 501 is 259 per mille: over.
            assert_eq!(lineage_of(&third)?.trained_at_generation, 4);
            assert_eq!(lineage_of(&third)?.appended_rows, 0);
            assert_eq!(report_of(&third), Some((631, 1)));
            Ok(())
        })
    }

    /// (iv) A base whose index is not this policy's own recipe, name or
    /// library version retrains.
    #[test]
    fn a_base_that_is_not_the_policy_retrains() -> TestResult {
        run(async {
            for label in ["another library version", "another graph recipe"] {
                // A train replaces the index. Each variant needs its own
                // physical base so the next one cannot inherit an old seal
                // over the previous variant's newly trained index.
                let temp = tempfile::tempdir()?;
                let table = vector_table(temp.path(), 400, DIMENSION).await?;
                let base = seal_fresh(&table, 1).await?;
                let mut inherited = base.clone();
                match label {
                    "another library version" => {
                        inherited.library_version = "0.29.0".to_string();
                    }
                    "another graph recipe" => {
                        let ann = inherited.ann.as_mut().ok_or("fixture has no index")?;
                        ann.hnsw_ef_construction = ann.hnsw_ef_construction.saturating_add(1);
                    }
                    _ => return Err("unexpected policy variant".into()),
                }
                append_rows(&table, 1_000..1_010, DIMENSION).await?;
                let rows = row_count(&table).await?;
                let sealed = seal_delta(&table, 2, &inherited).await?;
                assert_eq!(
                    lineage_of(&sealed)?.trained_at_generation,
                    2,
                    "{label} must retrain"
                );
                assert_eq!(report_of(&sealed), Some((rows, 1)), "{label}");
            }
            Ok(())
        })
    }

    /// A base whose contract names an index the inherited dataset does not
    /// carry as sealed is refused, not silently retrained: the seal cannot
    /// vouch for a lineage it cannot see.
    #[test]
    fn a_dataset_that_disagrees_with_the_inherited_contract_is_refused() -> TestResult {
        run(async {
            let temp = tempfile::tempdir()?;
            let table = vector_table(temp.path(), 400, DIMENSION).await?;
            let base = seal_fresh(&table, 1).await?;
            append_rows(&table, 1_000..1_010, DIMENSION).await?;

            let mut more_segments = base.clone();
            if let Some(ann) = more_segments.ann.as_mut() {
                ann.index_segments = 2;
            }
            let refused = seal_delta(&table, 2, &more_segments).await;
            assert!(
                matches!(&refused, Err(CoreError::Storage(message)) if message.contains("segments")),
                "{refused:?}"
            );

            let mut changed_segment = base.clone();
            let segment = changed_segment
                .ann
                .as_mut()
                .and_then(|ann| ann.segments.first_mut())
                .ok_or("fixture must seal a trained segment")?;
            segment.uuid = "forged-segment-uuid".to_string();
            let refused = seal_delta(&table, 2, &changed_segment).await;
            assert!(
                matches!(&refused, Err(CoreError::Storage(message)) if message.contains("segment")),
                "a changed inherited segment identity must be refused: {refused:?}"
            );

            let mut exact_base = base.clone();
            exact_base.mode = VECTOR_INDEX_MODE_EXACT.to_string();
            exact_base.ann = None;
            let refused = seal_delta(&table, 2, &exact_base).await;
            assert!(
                matches!(&refused, Err(CoreError::Storage(message)) if message.contains("sealed an exact lane")),
                "{refused:?}"
            );

            table.drop_index(VECTOR_INDEX_NAME).await?;
            let refused = seal_delta(&table, 2, &base).await;
            assert!(
                matches!(&refused, Err(CoreError::Storage(message)) if message.contains("inherited dataset lists")),
                "{refused:?}"
            );
            Ok(())
        })
    }

    /// A fully deleted index segment is a strict subset of the base record.
    /// The contraction gate rejects missing rows, a forged surviving UUID,
    /// and a count decrease without any deleted indexed rows.
    #[test]
    fn a_contracted_index_requires_exact_survivor_identity_and_deleted_rows() -> TestResult {
        run(async {
            let temp = tempfile::tempdir()?;
            let table = vector_table(temp.path(), 400, DIMENSION).await?;
            let base = seal_fresh(&table, 1).await?;
            append_rows(&table, 1_000..1_060, DIMENSION).await?;
            let appended = seal_delta(&table, 2, &base).await?;
            assert_eq!(report_of(&appended), Some((460, 2)));
            delete_ids(&table, &(1_000..1_060).collect::<Vec<_>>()).await?;
            assert_eq!(library_report(&table).await?, (400, 0, Some(1)));
            let fingerprint = |id: u64| SemanticRowFingerprintV1 {
                record_id: format!("{id:04}"),
                leaf_digest: [0; 32],
                native_row_id: id,
            };
            let mut base_rows = (0..400)
                .chain(1_000..1_060)
                .map(fingerprint)
                .collect::<Vec<_>>();
            base_rows.sort_unstable_by(|left, right| left.record_id.cmp(&right.record_id));
            let current_rows = (0..400).map(fingerprint).collect::<Vec<_>>();
            assert!(
                inherited_index_contracted_v1(&table, &appended, 400, &base_rows, &current_rows)
                    .await?
            );

            let ann = appended
                .ann
                .as_ref()
                .ok_or("fixture has no appended index")?;
            let missing_index_segment_without_row_deletion = verify_contraction_row_changes_v1(
                ann,
                IndexReportV1 {
                    indexed_rows: 400,
                    unindexed_rows: 60,
                    segments: 1,
                },
                460,
                &base_rows,
                &base_rows,
            );
            assert!(
                matches!(&missing_index_segment_without_row_deletion, Err(CoreError::Storage(message)) if message.contains("canonical row changes prove")),
                "{missing_index_segment_without_row_deletion:?}"
            );

            // Identical payload replacement has no logical row delta, but
            // Lance allocates new physical row IDs after delete plus append.
            let mut replacement_rows = base_rows.clone();
            for row in replacement_rows
                .iter_mut()
                .filter(|row| row.record_id.as_str() >= "1000")
            {
                row.native_row_id = row
                    .native_row_id
                    .checked_add(10_000)
                    .ok_or("fixture row ID overflow")?;
            }
            verify_contraction_row_changes_v1(
                ann,
                IndexReportV1 {
                    indexed_rows: 400,
                    unindexed_rows: 60,
                    segments: 1,
                },
                460,
                &base_rows,
                &replacement_rows,
            )?;
            let mut changed_payload = base_rows.clone();
            let row = changed_payload.first_mut().ok_or("fixture has no rows")?;
            row.leaf_digest = [1; 32];
            let same_physical_id_changed_payload = verify_contraction_row_changes_v1(
                ann,
                IndexReportV1 {
                    indexed_rows: 400,
                    unindexed_rows: 60,
                    segments: 1,
                },
                460,
                &base_rows,
                &changed_payload,
            );
            assert!(
                matches!(&same_physical_id_changed_payload, Err(CoreError::Storage(message)) if message.contains("inherited physical row changed identity or payload")),
                "{same_physical_id_changed_payload:?}"
            );

            let mut changed_uuid = appended.clone();
            let survivor = changed_uuid
                .ann
                .as_mut()
                .and_then(|ann| ann.segments.first_mut())
                .ok_or("fixture has no trained segment")?;
            survivor.uuid = "forged-survivor-uuid".to_string();
            let mismatch = inherited_index_contracted_v1(
                &table,
                &changed_uuid,
                400,
                &base_rows,
                &current_rows,
            )
            .await;
            assert!(
                matches!(&mismatch, Err(CoreError::Storage(message)) if message.contains("segment")),
                "{mismatch:?}"
            );

            let wrong_row_count =
                inherited_index_contracted_v1(&table, &appended, 399, &base_rows, &current_rows)
                    .await;
            assert!(
                matches!(&wrong_row_count, Err(CoreError::Storage(message)) if message.contains("dataset holds")),
                "{wrong_row_count:?}"
            );

            let mut no_deleted_rows = appended.clone();
            let ann = no_deleted_rows.ann.as_mut().ok_or("fixture has no index")?;
            ann.indexed_rows = 400;
            let no_deletion = inherited_index_contracted_v1(
                &table,
                &no_deleted_rows,
                400,
                &current_rows,
                &current_rows,
            )
            .await;
            assert!(
                matches!(&no_deletion, Err(CoreError::Storage(message)) if message.contains("positive indexed-row deletion")),
                "{no_deletion:?}"
            );
            Ok(())
        })
    }

    /// The seal records what the library built each segment with.
    ///
    /// Read back from the dataset: the trained segment carries the policy's
    /// recipe and an appended segment carries the incremental builder's own
    /// parameters. The open verifies the record against the dataset and
    /// refuses a seal that claims the recipe for a segment not built with
    /// it.
    #[test]
    #[expect(
        clippy::indexing_slicing,
        reason = "the test reads the segments it just asserted the count of; a short list fails the test"
    )]
    fn the_seal_records_every_segment_as_built_and_the_open_refuses_a_false_claim() -> TestResult {
        run(async {
            let temp = tempfile::tempdir()?;
            let table = vector_table(temp.path(), 400, DIMENSION).await?;
            let base = seal_fresh(&table, 1).await?;
            let built = read_index_segments_v1(&table, VECTOR_INDEX_NAME, "in the test").await?;
            let trained = base.ann.as_ref().ok_or("trained seal")?;
            assert_eq!(trained.segments, built);
            assert_eq!(built.len(), 1);
            assert_eq!(
                (built[0].hnsw_m, built[0].hnsw_ef_construction),
                (HNSW_M, HNSW_EF_CONSTRUCTION),
                "the trained segment is the recipe"
            );

            append_rows(&table, 1_000..1_060, DIMENSION).await?;
            let delta = seal_delta(&table, 2, &base).await?;
            let built = read_index_segments_v1(&table, VECTOR_INDEX_NAME, "in the test").await?;
            let appended = delta.ann.as_ref().ok_or("appended seal")?;
            assert_eq!(appended.segments, built);
            assert_eq!(built.len(), 2);
            assert_eq!(
                built[0], trained.segments[0],
                "the trained segment is unchanged"
            );
            assert_eq!(
                (built[1].hnsw_m, built[1].hnsw_ef_construction),
                (
                    LIBRARY_INCREMENTAL_HNSW_M,
                    LIBRARY_INCREMENTAL_HNSW_EF_CONSTRUCTION
                ),
                "the appended segment was built by the library's incremental builder"
            );
            assert_ne!(
                built[1].hnsw_ef_construction, HNSW_EF_CONSTRUCTION,
                "the appended segment does not carry the recipe, so the record must not claim it"
            );

            // A seal that claims the recipe for the appended segment is
            // refused by the manifest validator …
            let mut claimed = delta.clone();
            if let Some(ann) = claimed.ann.as_mut() {
                ann.segments = vec![
                    AnnIndexSegmentSealV1 {
                        uuid: built[0].uuid.clone(),
                        hnsw_m: HNSW_M,
                        hnsw_ef_construction: HNSW_EF_CONSTRUCTION,
                    },
                    AnnIndexSegmentSealV1 {
                        uuid: built[1].uuid.clone(),
                        hnsw_m: HNSW_M,
                        hnsw_ef_construction: HNSW_EF_CONSTRUCTION,
                    },
                ];
            }
            claimed.validate(460, 2)?;
            // … it is self-consistent, so it is the open that catches it
            // against the dataset.
            let refused = verify_vector_index_v1(&table, &claimed, 460).await;
            assert_eq!(
                typed_code(&refused),
                Some(quanta_index_contract::SearchPlaneErrorCodeV2::AnnIndexIncompatible)
            );
            // A record with a segment the dataset does not have is refused too.
            let mut foreign = delta.clone();
            if let Some(ann) = foreign.ann.as_mut()
                && let Some(segment) = ann.segments.last_mut()
            {
                segment.uuid = "not-a-segment".to_string();
            }
            let refused = verify_vector_index_v1(&table, &foreign, 460).await;
            assert_eq!(
                typed_code(&refused),
                Some(quanta_index_contract::SearchPlaneErrorCodeV2::AnnIndexIncompatible)
            );
            // The honest record opens, and the attestation names the appended
            // segment's actual construction beam width.
            let loaded = verify_vector_index_v1(&table, &delta, 460).await?;
            let trace = loaded.contract().trace_detail();
            assert!(
                trace.contains(&format!(
                    "ann.appended_segments=1; ann.appended_segments_m/ef_construction={LIBRARY_INCREMENTAL_HNSW_M}/{LIBRARY_INCREMENTAL_HNSW_EF_CONSTRUCTION}"
                )),
                "{trace}"
            );
            Ok(())
        })
    }

    /// (v) Recall oracle for an appended index.
    ///
    /// Over one corpus, the appended index and a freshly trained index are
    /// each measured against an exhaustive exact ranking computed here; the
    /// appended one may trail the fresh one by at most two points of
    /// recall@10.
    #[test]
    #[expect(
        clippy::print_stdout,
        reason = "the QI-BB-027-APPEND-RECALL line is the measurement the ledger cites; it must land in the run log"
    )]
    fn an_appended_index_keeps_recall_against_a_freshly_trained_one() -> TestResult {
        const WIDE: usize = 32;
        const BASE_ROWS: u64 = 1_024;
        const DELTA_ROWS: u64 = 200;
        const QUERIES: u64 = 50;
        const K: usize = 10;
        const TOLERANCE: f64 = 0.02;
        run(async {
            let appended_root = tempfile::tempdir()?;
            let appended_table = vector_table(appended_root.path(), BASE_ROWS, WIDE).await?;
            let base = seal_fresh(&appended_table, 1).await?;
            append_rows(
                &appended_table,
                BASE_ROWS..BASE_ROWS.saturating_add(DELTA_ROWS),
                WIDE,
            )
            .await?;
            let appended = seal_delta(&appended_table, 2, &base).await?;
            assert_eq!(lineage_of(&appended)?.appended_rows, DELTA_ROWS);
            let appended_loaded = verify_vector_index_v1(
                &appended_table,
                &appended,
                BASE_ROWS.saturating_add(DELTA_ROWS),
            )
            .await?;

            let fresh_root = tempfile::tempdir()?;
            let fresh_table = vector_table(
                fresh_root.path(),
                BASE_ROWS.saturating_add(DELTA_ROWS),
                WIDE,
            )
            .await?;
            let fresh = seal_fresh(&fresh_table, 1).await?;
            let fresh_loaded =
                verify_vector_index_v1(&fresh_table, &fresh, BASE_ROWS.saturating_add(DELTA_ROWS))
                    .await?;

            let corpus: BTreeSet<u64> = (0..BASE_ROWS.saturating_add(DELTA_ROWS)).collect();
            let mut appended_found = 0_u64;
            let mut fresh_found = 0_u64;
            for query_seed in 0..QUERIES {
                // Fresh directions, unrelated to any row.
                let query = unit_vector(5_000_000_u64.saturating_add(query_seed), WIDE);
                let expected: BTreeSet<u64> =
                    exact_top_k(&corpus, WIDE, &query, K).into_iter().collect();
                let appended_hits =
                    search_ids(&appended_table, &appended_loaded, &query, K).await?;
                let fresh_hits = search_ids(&fresh_table, &fresh_loaded, &query, K).await?;
                assert_eq!(
                    appended_hits.len(),
                    K,
                    "query {query_seed} through the appended index"
                );
                assert_eq!(
                    fresh_hits.len(),
                    K,
                    "query {query_seed} through the fresh index"
                );
                appended_found = appended_found.saturating_add(u64::try_from(
                    appended_hits
                        .iter()
                        .filter(|id| expected.contains(id))
                        .count(),
                )?);
                fresh_found = fresh_found.saturating_add(u64::try_from(
                    fresh_hits.iter().filter(|id| expected.contains(id)).count(),
                )?);
            }
            let total = f64::from(u32::try_from(QUERIES.saturating_mul(u64::try_from(K)?))?);
            let appended_recall = f64::from(u32::try_from(appended_found)?) / total;
            let fresh_recall = f64::from(u32::try_from(fresh_found)?) / total;
            println!(
                "QI-BB-027-APPEND-RECALL rows={BASE_ROWS}+{DELTA_ROWS} dim={WIDE} queries={QUERIES} k={K} appended_recall_at_k={appended_recall:.4} fresh_recall_at_k={fresh_recall:.4}"
            );
            assert!(
                appended_recall >= fresh_recall - TOLERANCE,
                "appended recall@{K} {appended_recall:.4} trails fresh {fresh_recall:.4} by more than {TOLERANCE}"
            );
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
