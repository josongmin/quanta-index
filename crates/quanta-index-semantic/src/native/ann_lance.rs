//! Bounded immutable Lance regions for the Native semantic lane.
//!
//! The caller owns the admitted staging session, the selected-view member
//! directory, and physical-file custody. This module cannot publish a region.

use std::sync::Arc;

use arrow_array::{
    Array, BinaryArray, FixedSizeBinaryArray, FixedSizeListArray, Float32Array, RecordBatch,
    RecordBatchIterator,
    builder::{BinaryBuilder, FixedSizeBinaryBuilder},
};
use arrow_schema::{ArrowError, DataType, Field, Schema};
use futures::{Stream, StreamExt as _, TryStreamExt as _};
use lance::{Dataset, dataset::write::WriteParams, session::Session};
use lance_linalg::distance::MetricType;
use quanta_index_contract::native_protocol::{
    MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES, NativeSemanticTraversal,
};
use quanta_index_core::domains::semantic::DenseIndexV1;
use quanta_index_core::index::{
    Digest, IndexError, MemoryLease, MemoryLifetime, OwnerInputBinding, OwnerKey, ProducerId,
    StableOwnerId, WorkBudget, WorkUsage,
};
use sha2::{Digest as _, Sha256};

use super::{
    ann_region_policy::{MAX_REGION_CELLS, MAX_REGION_DIMENSION, MAX_REGION_ROWS},
    ann_store::AnnMemoryStore,
};
use crate::{
    COLUMN_VECTOR,
    manifest::VectorIndexSealV1,
    vector_index::{
        LoadedVectorIndexV1, seal_fresh_vector_index_dataset_v1, verify_vector_index_dataset_v1,
    },
};

const ROW_DOMAIN: &[u8] = b"quanta-index-native-ann-region-rows-v2\0";
const INPUT_DOMAIN: &[u8] = b"quanta-index-native-ann-owner-input-v1\0";
/// Region creation and selected reopen use the same private session path.
pub(super) const ANN_DATASET_URI: &str = "memory://native-ann/region/region.lance";
const COLUMN_PRODUCER: &str = "producer";
const COLUMN_OWNER: &str = "owner";
const COLUMN_MEMBER: &str = "member";
const COLUMN_VECTOR_ID: &str = "vector_id";
const COLUMN_INPUT: &str = "input_stamp";
const COLUMN_DISTANCE: &str = "_distance";

mod custody;
#[cfg(test)]
mod physical_query_tests;
mod row_attestation;
#[cfg(test)]
mod trained_tests;
mod write_settle;

/// A region can contain multiple owners; the cap is independent of corpus size.
/// Regions smaller than this project's index floor seal as bounded exact.
pub(super) const MAX_REGION_CANDIDATES: usize = 1024;

#[derive(Clone, Copy, Debug)]
pub(super) struct RegionRow<'a> {
    pub owner: OwnerKey,
    pub member: Digest,
    pub vector_id: &'a [u8],
    pub input: OwnerInputBinding,
    pub vector: &'a [f32],
}

/// The parent binds this record to a Catalog member artifact and registers
/// every Lance dataset child in the immutable object graph before publication.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RegionSeal {
    pub generation: u64,
    pub recipe: Digest,
    pub dimension: u32,
    pub rows: u32,
    pub rows_digest: Digest,
    pub vector_index: VectorIndexSealV1,
}
quanta_index_contract::native_serde_struct!(RegionSeal {
    generation: u64,
    recipe: Digest,
    dimension: u32,
    rows: u32,
    rows_digest: Digest,
    vector_index: VectorIndexSealV1,
});

#[derive(Clone, Debug, PartialEq)]
pub(super) struct RegionCandidate {
    pub owner: OwnerKey,
    pub member: Digest,
    pub vector_id: Vec<u8>,
    pub input_stamp: Digest,
}

/// Traversal describes only this immutable region, independently of durable
/// source coverage. Approximate traversal never certifies exhaustive recall.
#[derive(Debug)]
pub(super) struct RegionPage {
    pub inspected: u32,
    pub traversal: NativeSemanticTraversal,
    pub requested: usize,
    pub last_prefix: usize,
}

struct PrefixResult {
    page: RegionPage,
    seen: Vec<RegionCandidate>,
    seen_memory: Option<MemoryLease>,
}

struct PrefixIdentity {
    candidates: Vec<RegionCandidate>,
    _memory: Option<MemoryLease>,
}

// A widening step smaller than doubling is terminal: it reaches the row
// count or consumes the remaining cumulative budget.
const MAX_REFILL_PREFIXES: usize = {
    let mut width = MAX_REGION_CANDIDATES;
    let mut count = 1usize;
    while width > 1 {
        width >>= 1;
        count = count.saturating_add(1);
    }
    count
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RefillProgress {
    Vector,
    SourceFile(OwnerKey),
    /// Drain the current batch, but do not spend work on another prefix.
    OwnerLimitReached,
}

/// One region query's verified candidates across widening backend prefixes.
/// Identity vectors remain live only while another prefix needs them.
struct RefillDecisions {
    prior_prefixes: [Option<PrefixIdentity>; MAX_REFILL_PREFIXES],
    file_owners: Vec<OwnerKey>,
    _file_owners_memory: Option<MemoryLease>,
    accepted_count: usize,
    rescored: usize,
    skipped_after_rescore_cap: bool,
    owner_limit_reached: bool,
}

impl RefillDecisions {
    fn new() -> Self {
        Self {
            prior_prefixes: std::array::from_fn(|_| None),
            file_owners: Vec::new(),
            _file_owners_memory: None,
            accepted_count: 0,
            rescored: 0,
            skipped_after_rescore_cap: false,
            owner_limit_reached: false,
        }
    }

    fn remember_prefix(
        &mut self,
        seen: Vec<RegionCandidate>,
        memory: Option<MemoryLease>,
    ) -> Result<(), IndexError> {
        let slot = self
            .prior_prefixes
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or_else(|| {
                IndexError::BackendViolation("Native ANN refill exceeded prefix capacity".into())
            })?;
        *slot = Some(PrefixIdentity {
            candidates: seen,
            _memory: memory,
        });
        Ok(())
    }

    fn admit(
        &mut self,
        candidate: &RegionCandidate,
        budget: &mut WorkBudget,
        top_k: usize,
        rescore_cap: usize,
        accept: &mut impl FnMut(
            &RegionCandidate,
            &mut WorkBudget,
        ) -> Result<Option<RefillProgress>, IndexError>,
    ) -> Result<(), IndexError> {
        if let Some(prior) = self
            .prior_prefixes
            .iter()
            .flatten()
            .flat_map(|prefix| &prefix.candidates)
            .find(|prior| {
                prior.owner == candidate.owner
                    && prior.member == candidate.member
                    && prior.vector_id == candidate.vector_id
            })
        {
            if prior.input_stamp != candidate.input_stamp {
                return Err(corrupt(
                    "Native ANN refill changed immutable candidate input",
                ));
            }
            return Ok(());
        }
        // A rejected row does not spend the original-vector window. Once
        // enough distinct results exist, do not rank further new rows in an
        // already started backend batch. The physical batch is still drained.
        if self.rescored >= rescore_cap && self.accepted_count >= top_k {
            self.skipped_after_rescore_cap = true;
            return Ok(());
        }
        let decision = accept(candidate, budget)?;
        if decision == Some(RefillProgress::OwnerLimitReached) {
            self.owner_limit_reached = true;
        } else if decision.is_some() {
            self.rescored = self
                .rescored
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
        }
        let new_result = match decision {
            Some(RefillProgress::Vector) if self.accepted_count < top_k => true,
            Some(RefillProgress::SourceFile(owner))
                if self.accepted_count < top_k && !self.file_owners.contains(&owner) =>
            {
                if self.file_owners.is_empty() {
                    let bytes = top_k
                        .checked_mul(std::mem::size_of::<OwnerKey>())
                        .ok_or(IndexError::ResourceLimited)?;
                    let memory = budget.reserve_memory(
                        u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                        MemoryLifetime::Temporary,
                    )?;
                    self.file_owners
                        .try_reserve_exact(top_k)
                        .map_err(|_error| IndexError::ResourceLimited)?;
                    if self.file_owners.capacity() > top_k {
                        return Err(IndexError::ResourceLimited);
                    }
                    self._file_owners_memory = memory;
                }
                self.file_owners.push(owner);
                true
            }
            _ => false,
        };
        if new_result {
            self.accepted_count = self
                .accepted_count
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
        }
        Ok(())
    }
}

pub(super) struct OpenRegion {
    dataset: Arc<lance::Dataset>,
    index: LoadedVectorIndexV1,
    seal: RegionSeal,
    read_store: Option<Arc<AnnMemoryStore>>,
}

impl OpenRegion {
    pub(super) fn with_read_store(mut self, store: Arc<AnnMemoryStore>) -> Self {
        store.enable_read_scope();
        self.read_store = Some(store);
        self
    }

    fn stop_reads(&self) {
        if let Some(store) = &self.read_store {
            store.stop_reads();
        }
    }

    fn resolve_read_error(&self, error: IndexError) -> IndexError {
        if matches!(error, IndexError::Storage(_))
            && let Some(report) = self
                .read_store
                .as_ref()
                .and_then(|store| store.take_denial())
        {
            return report;
        }
        error
    }
}

fn invalid(message: impl Into<String>) -> IndexError {
    IndexError::InvalidInput(message.into())
}

fn corrupt(message: impl Into<String>) -> IndexError {
    IndexError::CorruptObject(message.into())
}

fn backend(error: impl std::fmt::Display) -> IndexError {
    IndexError::Storage(format!("native ANN Lance: {error}"))
}

fn update_optional_ref(hash: &mut Sha256, value: Option<quanta_index_core::index::ObjectRef>) {
    match value {
        Some(reference) => {
            hash.update([1]);
            hash.update(reference.0.0);
        }
        None => hash.update([0]),
    }
}

/// The selected member must match this exact source/projection/context stamp;
/// a matching logical member ID alone is not sufficient across view changes.
pub(super) fn owner_input_stamp(input: OwnerInputBinding) -> Digest {
    let mut hash = Sha256::new();
    hash.update(INPUT_DOMAIN);
    update_optional_ref(&mut hash, input.source);
    update_optional_ref(&mut hash, input.projection);
    hash.update(input.producer_context.0);
    hash.update(input.derivation_policy.0);
    Digest(hash.finalize().into())
}

/// Use after resolving a member in the pinned selected-view directory.
pub(super) fn selected_input_matches(
    candidate: &RegionCandidate,
    selected_owner: OwnerKey,
    selected_member: Digest,
    selected_vector_id: &[u8],
    selected_input: OwnerInputBinding,
) -> bool {
    candidate.owner == selected_owner
        && candidate.member == selected_member
        && candidate.vector_id == selected_vector_id
        && candidate.input_stamp == owner_input_stamp(selected_input)
}

fn schema(dimension: usize) -> Result<Arc<Schema>, IndexError> {
    let dimension = i32::try_from(dimension).map_err(|_error| IndexError::ResourceLimited)?;
    // Lance 7 persists primitive FixedSizeList's type/width but reconstructs
    // its item Field as nullable. Use that exact physical form on both writes
    // and selected opens; rows are checked for finite, nonzero values first.
    Ok(Arc::new(Schema::new(vec![
        Field::new(COLUMN_PRODUCER, DataType::FixedSizeBinary(16), false),
        Field::new(COLUMN_OWNER, DataType::FixedSizeBinary(16), false),
        Field::new(COLUMN_MEMBER, DataType::FixedSizeBinary(32), false),
        Field::new(COLUMN_VECTOR_ID, DataType::Binary, false),
        Field::new(COLUMN_INPUT, DataType::FixedSizeBinary(32), false),
        Field::new(
            COLUMN_VECTOR,
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                dimension,
            ),
            false,
        ),
    ])))
}

fn validate_rows(
    rows: &[RegionRow<'_>],
    recipe: Digest,
    dimension: usize,
) -> Result<Digest, IndexError> {
    if rows.is_empty()
        || rows.len() > MAX_REGION_ROWS
        || dimension == 0
        || dimension > MAX_REGION_DIMENSION
        || rows.len().saturating_mul(dimension) > MAX_REGION_CELLS
    {
        return Err(invalid(
            "Native ANN region exceeds bounded row/dimension policy",
        ));
    }
    let mut hash = Sha256::new();
    hash.update(ROW_DOMAIN);
    hash.update(recipe.0);
    hash.update(
        u32::try_from(dimension)
            .map_err(|_error| IndexError::ResourceLimited)?
            .to_be_bytes(),
    );
    hash.update(
        u32::try_from(rows.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .to_be_bytes(),
    );
    let mut last: Option<(OwnerKey, Digest, &[u8])> = None;
    for row in rows {
        let key = (row.owner, row.member, row.vector_id);
        if last.is_some_and(|previous| previous >= key) {
            return Err(invalid(
                "Native ANN rows must be sorted and unique by owner/member/vector ID",
            ));
        }
        last = Some(key);
        if row.vector_id.is_empty() || row.vector_id.len() > MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES {
            return Err(invalid("Native ANN vector ID exceeds bounded policy"));
        }
        if row.input.derivation_policy != recipe || row.vector.len() != dimension {
            return Err(invalid("Native ANN row recipe/dimension mismatch"));
        }
        let norm = row.vector.iter().try_fold(0.0f64, |acc, value| {
            let coordinate = f64::from(*value);
            coordinate
                .is_finite()
                .then_some(coordinate.mul_add(coordinate, acc))
        });
        if !norm.is_some_and(|value| value.is_finite() && value > 0.0) {
            return Err(invalid("Native ANN vector must be finite and nonzero"));
        }
        hash.update(row.owner.producer.0);
        hash.update(row.owner.stable_id.0);
        hash.update(row.member.0);
        hash.update(
            u32::try_from(row.vector_id.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .to_be_bytes(),
        );
        hash.update(row.vector_id);
        hash.update(owner_input_stamp(row.input).0);
        for value in row.vector {
            hash.update(value.to_bits().to_be_bytes());
        }
    }
    Ok(Digest(hash.finalize().into()))
}

fn arrow_batch(rows: &[RegionRow<'_>], dimension: usize) -> Result<RecordBatch, IndexError> {
    let mut producer = FixedSizeBinaryBuilder::with_capacity(rows.len(), 16);
    let mut owner = FixedSizeBinaryBuilder::with_capacity(rows.len(), 16);
    let mut member = FixedSizeBinaryBuilder::with_capacity(rows.len(), 32);
    let vector_id_bytes = rows.iter().try_fold(0usize, |total, row| {
        total
            .checked_add(row.vector_id.len())
            .ok_or(IndexError::ResourceLimited)
    })?;
    let mut vector_id = BinaryBuilder::with_capacity(rows.len(), vector_id_bytes);
    let mut input = FixedSizeBinaryBuilder::with_capacity(rows.len(), 32);
    let cells = rows
        .len()
        .checked_mul(dimension)
        .ok_or(IndexError::ResourceLimited)?;
    let mut flat = Vec::new();
    flat.try_reserve_exact(cells)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if flat.capacity() > cells {
        return Err(IndexError::ResourceLimited);
    }
    for row in rows {
        producer
            .append_value(row.owner.producer.0)
            .map_err(backend)?;
        owner.append_value(row.owner.stable_id.0).map_err(backend)?;
        member.append_value(row.member.0).map_err(backend)?;
        vector_id.append_value(row.vector_id);
        input
            .append_value(owner_input_stamp(row.input).0)
            .map_err(backend)?;
        flat.extend_from_slice(row.vector);
    }
    let vectors = FixedSizeListArray::try_new(
        Arc::new(Field::new("item", DataType::Float32, true)),
        i32::try_from(dimension).map_err(|_error| IndexError::ResourceLimited)?,
        Arc::new(Float32Array::from(flat)),
        None,
    )
    .map_err(backend)?;
    RecordBatch::try_new(
        schema(dimension)?,
        vec![
            Arc::new(producer.finish()),
            Arc::new(owner.finish()),
            Arc::new(member.finish()),
            Arc::new(vector_id.finish()),
            Arc::new(input.finish()),
            Arc::new(vectors),
        ],
    )
    .map_err(backend)
}

fn admitted_region_batch(
    rows: &[RegionRow<'_>],
    recipe: Digest,
    dimension: usize,
    budget: &mut WorkBudget,
) -> Result<(RecordBatch, Digest, u64), IndexError> {
    budget.checkpoint()?;
    let rows_digest = validate_rows(rows, recipe, dimension)?;
    let row_count = u64::try_from(rows.len()).map_err(|_error| IndexError::ResourceLimited)?;
    let dimension_u64 = u64::try_from(dimension).map_err(|_error| IndexError::ResourceLimited)?;
    let vector_id_bytes = rows.iter().try_fold(0u64, |total, row| {
        total
            .checked_add(
                u64::try_from(row.vector_id.len()).map_err(|_error| IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)
    })?;
    let bytes = dimension_u64
        .checked_mul(4)
        .and_then(|vector| vector.checked_add(256))
        .and_then(|per_row| per_row.checked_mul(row_count))
        .and_then(|total| total.checked_add(vector_id_bytes))
        .and_then(|total| total.checked_add(8192))
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        writes: 1,
        bytes,
        mutations: 1,
        ..WorkUsage::default()
    })?;
    // Lance can move this batch into a task which outlives an aborted caller.
    // Admit escape capacity before allocating, using the original scope.
    let input_memory = budget.reserve_memory(bytes, MemoryLifetime::Retained)?;
    let batch = custody::retain_batch(arrow_batch(rows, dimension)?, input_memory.clone())?;
    Ok((batch, rows_digest, row_count))
}

/// Create one fresh Native region through the bounded session and seal the
/// exact Lance dataset returned by the writer before it can be published.
pub(super) async fn build_region_direct(
    session: Arc<Session>,
    rows: &[RegionRow<'_>],
    recipe: Digest,
    generation: u64,
    dimension: usize,
    budget: &mut WorkBudget,
) -> Result<(Arc<Dataset>, RegionSeal), IndexError> {
    build_region_at(
        ANN_DATASET_URI,
        session,
        rows,
        recipe,
        generation,
        dimension,
        budget,
    )
    .await
}

async fn build_region_at(
    uri: &str,
    session: Arc<Session>,
    rows: &[RegionRow<'_>],
    recipe: Digest,
    generation: u64,
    dimension: usize,
    budget: &mut WorkBudget,
) -> Result<(Arc<Dataset>, RegionSeal), IndexError> {
    let (batch, rows_digest, row_count) = admitted_region_batch(rows, recipe, dimension, budget)?;
    let schema = batch.schema();
    let reader = RecordBatchIterator::new(std::iter::once(Ok::<_, ArrowError>(batch)), schema);
    let params = WriteParams {
        session: Some(session),
        ..Default::default()
    };
    let request = budget.request().clone();
    let mut dataset = write_settle::settle_write(&request, async {
        Dataset::write(reader, uri, Some(params))
            .await
            .map_err(backend)
    })
    .await?;
    let vector_index = write_settle::settle_write(&request, async {
        seal_fresh_vector_index_dataset_v1(&mut dataset, generation, row_count)
            .await
            .map_err(backend)
    })
    .await?;
    vector_index
        .validate(row_count, generation)
        .map_err(backend)?;
    let seal = RegionSeal {
        generation,
        recipe,
        dimension: u32::try_from(dimension).map_err(|_error| IndexError::ResourceLimited)?,
        rows: u32::try_from(rows.len()).map_err(|_error| IndexError::ResourceLimited)?,
        rows_digest,
        vector_index,
    };
    row_attestation::attest(&dataset, &seal, budget).await?;
    budget.checkpoint()?;
    Ok((Arc::new(dataset), seal))
}

#[cfg(test)]
fn local_fixture_uri(root: &std::path::Path, name: &str) -> Result<String, IndexError> {
    let path = root.join(format!("{name}.lance"));
    url::Url::from_file_path(&path)
        .map(|uri| uri.to_string())
        .map_err(|()| invalid("Native ANN fixture requires an absolute file path"))
}

#[cfg(test)]
async fn write_fixture_rows(uri: &str, batch: RecordBatch) -> lance::Result<Dataset> {
    let schema = batch.schema();
    let reader = RecordBatchIterator::new(std::iter::once(Ok::<_, ArrowError>(batch)), schema);
    Dataset::write(reader, uri, None).await
}

/// Verify a bounded region already selected through Catalog. Caller must
/// attest the physical dataset bytes and the row digest before this call.
pub(super) async fn open_region(
    dataset: Arc<lance::Dataset>,
    seal: RegionSeal,
    budget: &mut WorkBudget,
) -> Result<OpenRegion, IndexError> {
    budget.checkpoint()?;
    let rows = usize::try_from(seal.rows).map_err(|_error| IndexError::ResourceLimited)?;
    let dimension =
        usize::try_from(seal.dimension).map_err(|_error| IndexError::ResourceLimited)?;
    if seal.rows == 0
        || rows > MAX_REGION_ROWS
        || seal.dimension == 0
        || dimension > MAX_REGION_DIMENSION
        || rows.saturating_mul(dimension) > MAX_REGION_CELLS
    {
        return Err(corrupt("Native ANN region seal exceeds bounded policy"));
    }
    seal.vector_index
        .validate(u64::from(seal.rows), seal.generation)
        .map_err(backend)?;
    let actual = arrow_schema::Schema::from(dataset.schema());
    budget.checkpoint()?;
    if &actual != schema(dimension)?.as_ref() {
        return Err(corrupt("Native ANN physical schema differs from seal"));
    }
    let index = verify_vector_index_dataset_v1(&dataset, &seal.vector_index, u64::from(seal.rows))
        .await
        .map_err(backend);
    budget.checkpoint()?;
    let index = index?;
    budget.charge(OPEN_REGION_WORK)?;
    Ok(OpenRegion {
        dataset,
        index,
        seal,
        read_store: None,
    })
}

pub(super) const OPEN_REGION_WORK: WorkUsage = WorkUsage {
    reads: 1,
    writes: 0,
    bytes: 0,
    mutations: 0,
};

fn binary<'a>(
    batch: &'a RecordBatch,
    name: &str,
    size: i32,
) -> Result<&'a FixedSizeBinaryArray, IndexError> {
    let column = batch
        .column_by_name(name)
        .ok_or_else(|| corrupt(format!("Native ANN missing {name}")))?;
    let array = column
        .as_any()
        .downcast_ref::<FixedSizeBinaryArray>()
        .ok_or_else(|| corrupt(format!("Native ANN malformed {name}")))?;
    if array.value_length() != size || array.null_count() != 0 {
        return Err(corrupt(format!("Native ANN invalid {name} width/null")));
    }
    Ok(array)
}

fn vector_ids(batch: &RecordBatch) -> Result<&BinaryArray, IndexError> {
    let column = batch
        .column_by_name(COLUMN_VECTOR_ID)
        .ok_or_else(|| corrupt("Native ANN missing vector ID"))?;
    let array = column
        .as_any()
        .downcast_ref::<BinaryArray>()
        .ok_or_else(|| corrupt("Native ANN malformed vector ID"))?;
    if array.null_count() != 0 {
        return Err(corrupt("Native ANN null vector ID"));
    }
    Ok(array)
}

fn digest(bytes: &[u8]) -> Digest {
    let mut value = [0; 32];
    value.copy_from_slice(bytes);
    Digest(value)
}
fn id(bytes: &[u8]) -> [u8; 16] {
    let mut value = [0; 16];
    value.copy_from_slice(bytes);
    value
}

/// Refill a selected region within a cumulative materialized-row ceiling.
///
/// `initial_candidate_cap == candidate_cap` preserves the one-prefix policy.
/// A smaller initial prefix enables explicitly funded refill.
/// Repeated materialized rows count again against `candidate_cap`; every query
/// charges the same original work budget. Backend probes/distance evaluations
/// are separate costs. A capped or approximate prefix never proves exhaustion.
/// `accept` returns vector progress or the authenticated source-file owner
/// after validating the pinned selected-view owner/member/input binding.
/// Multiple vectors of one file count once in file search.
/// An owner limit stops subsequent prefixes while the current prefix settles.
pub(super) async fn query_region(
    region: &OpenRegion,
    vector: &[f32],
    top_k: usize,
    initial_candidate_cap: usize,
    candidate_cap: usize,
    budget: &mut WorkBudget,
    accept: impl FnMut(&RegionCandidate, &mut WorkBudget) -> Result<Option<RefillProgress>, IndexError>,
) -> Result<RegionPage, IndexError> {
    let request = budget.request().clone();
    let query = query_region_bounded(
        region,
        vector,
        top_k,
        initial_candidate_cap,
        candidate_cap,
        budget,
        accept,
    );
    let Some(store) = &region.read_store else {
        return query.await;
    };
    tokio::pin!(query);
    let result = tokio::select! {
        biased;
        result = &mut query => result,
        _interruption = crate::budget::wait_for_request_interruption(&request, "native ANN read") => {
            if store.reads_stopped() {
                // The consumer already recorded a failure before this wake.
                query.await
            } else {
                let interruption = if request.is_cancelled() {
                    IndexError::Cancelled
                } else {
                    IndexError::ResourceLimited
                };
                // Signal the private store while its read is pending. Keep the
                // Lance future and its memory alive until the vendor settles.
                store.stop_reads();
                let _settled = query.await;
                Err(interruption)
            }
        }
    };
    if result.is_err() {
        store.stop_reads();
        store.wait_for_stopped_reads().await;
    }
    let _denial = store.take_denial();
    result
}

async fn query_region_bounded(
    region: &OpenRegion,
    vector: &[f32],
    top_k: usize,
    initial_candidate_cap: usize,
    candidate_cap: usize,
    budget: &mut WorkBudget,
    mut accept: impl FnMut(
        &RegionCandidate,
        &mut WorkBudget,
    ) -> Result<Option<RefillProgress>, IndexError>,
) -> Result<RegionPage, IndexError> {
    budget.checkpoint()?;
    validate_region_query(region, vector, top_k, candidate_cap)?;
    if initial_candidate_cap < top_k || initial_candidate_cap > candidate_cap {
        return Err(invalid(
            "Native ANN initial prefix exceeds candidate work policy",
        ));
    }
    let rows = usize::try_from(region.seal.rows).map_err(|_error| IndexError::ResourceLimited)?;
    let first = initial_candidate_cap.min(rows);
    let remaining = candidate_cap
        .checked_sub(first)
        .ok_or(IndexError::ResourceLimited)?;
    // A single-prefix policy needs no extra retained merge buffer.
    if next_prefix(first, rows, remaining).is_none() {
        let prefix = query_prefix(region, vector, first, budget, |candidate, budget| {
            let _progress = accept(candidate, budget)?;
            Ok(())
        })
        .await?;
        return Ok(prefix.page);
    }
    let mut decisions = RefillDecisions::new();
    let mut requested = 0usize;
    let mut inspected = 0u32;
    let mut fetch = first;
    let mut traversal;
    loop {
        let PrefixResult {
            page,
            seen,
            seen_memory,
        } = query_prefix(region, vector, fetch, budget, |candidate, budget| {
            decisions.admit(candidate, budget, top_k, initial_candidate_cap, &mut accept)
        })
        .await?;
        requested = requested
            .checked_add(fetch)
            .ok_or(IndexError::ResourceLimited)?;
        inspected = inspected
            .checked_add(page.inspected)
            .ok_or(IndexError::ResourceLimited)?;
        traversal = if decisions.skipped_after_rescore_cap {
            match page.traversal {
                NativeSemanticTraversal::ExactExhausted => NativeSemanticTraversal::ExactCapped,
                NativeSemanticTraversal::Approximate { .. } => {
                    NativeSemanticTraversal::Approximate { capped: true }
                }
                other => other,
            }
        } else {
            page.traversal
        };
        if decisions.owner_limit_reached
            || decisions.accepted_count >= top_k
            || traversal == NativeSemanticTraversal::ExactExhausted
        {
            break;
        }
        let remaining = candidate_cap
            .checked_sub(requested)
            .ok_or(IndexError::ResourceLimited)?;
        let Some(next) = next_prefix(fetch, rows, remaining) else {
            break;
        };
        // Transfer the verified prefix and its exact identity lease only when
        // another backend prefix will need cross-prefix duplicate checks.
        decisions.remember_prefix(seen, seen_memory)?;
        fetch = next;
    }
    budget.checkpoint()?;
    Ok(RegionPage {
        inspected,
        traversal,
        requested,
        last_prefix: fetch,
    })
}

fn next_prefix(fetch: usize, rows: usize, remaining: usize) -> Option<usize> {
    let next = fetch.saturating_mul(2).min(rows).min(remaining);
    (next > fetch).then_some(next)
}

fn validate_region_query(
    region: &OpenRegion,
    vector: &[f32],
    top_k: usize,
    candidate_cap: usize,
) -> Result<(), IndexError> {
    let dimension =
        usize::try_from(region.seal.dimension).map_err(|_error| IndexError::ResourceLimited)?;
    if vector.len() != dimension
        || vector.iter().any(|v| !v.is_finite())
        || vector.iter().all(|v| *v == 0.0)
    {
        return Err(invalid("Native ANN query dimension/vector invalid"));
    }
    if top_k == 0
        || top_k > MAX_REGION_CANDIDATES
        || candidate_cap < top_k
        || candidate_cap > MAX_REGION_CANDIDATES
    {
        return Err(invalid("Native ANN candidate cap/top_k out of range"));
    }
    Ok(())
}

// A consumer refusal can arrive with a vendor batch already being prepared.
// Drive the stream to completion under the existing execution grant, keeping
// the first error and admitting no further candidates.
async fn settle_query_batches(
    mut batches: impl Stream<Item = Result<RecordBatch, IndexError>> + Unpin,
    mut accept: impl FnMut(Result<RecordBatch, IndexError>) -> Result<(), IndexError>,
    stop_reads: impl FnOnce(),
) -> Result<(), IndexError> {
    while let Some(batch) = batches.next().await {
        if let Err(error) = accept(batch) {
            stop_reads();
            while batches.next().await.is_some() {}
            return Err(error);
        }
    }
    Ok(())
}

pub(super) fn prefix_work(fetch: usize) -> Result<WorkUsage, IndexError> {
    let fetch = u64::try_from(fetch).map_err(|_error| IndexError::ResourceLimited)?;
    let vector_id_max = u64::try_from(MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let bytes = 128u64
        .checked_add(vector_id_max)
        .and_then(|bytes| fetch.checked_mul(bytes))
        .ok_or(IndexError::ResourceLimited)?;
    Ok(WorkUsage {
        reads: 1,
        bytes,
        ..WorkUsage::default()
    })
}

/// One immutable prefix. The caller owns the cumulative refill ceiling.
async fn query_prefix(
    region: &OpenRegion,
    vector: &[f32],
    candidate_cap: usize,
    budget: &mut WorkBudget,
    mut accept: impl FnMut(&RegionCandidate, &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<PrefixResult, IndexError> {
    let rows = usize::try_from(region.seal.rows).map_err(|_error| IndexError::ResourceLimited)?;
    let fetch = candidate_cap.min(rows);
    let fetch_u64 = u64::try_from(fetch).map_err(|_error| IndexError::ResourceLimited)?;
    let vector_bytes = u64::try_from(vector.len())
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_mul(
            u64::try_from(std::mem::size_of::<f32>())
                .map_err(|_error| IndexError::ResourceLimited)?,
        )
        .ok_or(IndexError::ResourceLimited)?;
    let vector_id_max = u64::try_from(MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let candidate_bytes = u64::try_from(std::mem::size_of::<RegionCandidate>())
        .map_err(|_error| IndexError::ResourceLimited)?;
    let seen_bytes = fetch_u64
        .checked_mul(
            candidate_bytes
                .checked_add(vector_id_max)
                .ok_or(IndexError::ResourceLimited)?,
        )
        .ok_or(IndexError::ResourceLimited)?;
    let scratch_bytes = fetch_u64
        .checked_mul(512)
        .and_then(|bytes| bytes.checked_add(vector_bytes))
        .and_then(|bytes| bytes.checked_add(4096))
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(prefix_work(fetch)?)?;
    let _scratch_memory = budget.reserve_memory(scratch_bytes, MemoryLifetime::Temporary)?;
    let seen_memory = budget.reserve_memory(seen_bytes, MemoryLifetime::Temporary)?;
    let mut query_vector = Vec::new();
    query_vector
        .try_reserve_exact(vector.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    if query_vector.capacity() > vector.len() {
        return Err(IndexError::ResourceLimited);
    }
    query_vector.extend_from_slice(vector);
    let mut seen: Vec<RegionCandidate> = Vec::new();
    seen.try_reserve_exact(fetch)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if seen.capacity() > fetch {
        return Err(IndexError::ResourceLimited);
    }
    let query_vector = Float32Array::from(query_vector);
    let mut scan = region.dataset.scan();
    let _ = scan
        .nearest(COLUMN_VECTOR, &query_vector, fetch)
        .map_err(backend)?;
    let _ = scan.distance_metric(MetricType::Cosine);
    let _ = scan
        .limit(
            Some(i64::try_from(fetch).map_err(|_error| IndexError::ResourceLimited)?),
            None,
        )
        .map_err(backend)?;
    let _ = scan
        .project(&[
            COLUMN_PRODUCER,
            COLUMN_OWNER,
            COLUMN_MEMBER,
            COLUMN_VECTOR_ID,
            COLUMN_INPUT,
        ])
        .map_err(backend)?;
    let _ = scan.batch_size(1024);
    region
        .index
        .apply_scanner(&mut scan, fetch)
        .map_err(backend)?;
    budget.checkpoint()?;
    let batches = match scan.try_into_stream().await.map_err(backend) {
        Ok(batches) => batches,
        Err(error) => {
            budget.checkpoint()?;
            return Err(region.resolve_read_error(error));
        }
    };
    let mut inspected = 0usize;
    let accept_batch = |batch: Result<RecordBatch, IndexError>| {
        budget.checkpoint()?;
        let batch = batch.map_err(|error| region.resolve_read_error(error))?;
        if batch.num_rows() > fetch.saturating_sub(inspected) {
            return Err(IndexError::BackendViolation(
                "Native ANN batch exceeded remaining candidate cap".into(),
            ));
        }
        let producer = binary(&batch, COLUMN_PRODUCER, 16)?;
        let owner = binary(&batch, COLUMN_OWNER, 16)?;
        let member = binary(&batch, COLUMN_MEMBER, 32)?;
        let vector_ids = vector_ids(&batch)?;
        let input = binary(&batch, COLUMN_INPUT, 32)?;
        let distances = batch
            .column_by_name(COLUMN_DISTANCE)
            .and_then(|column| column.as_any().downcast_ref::<Float32Array>())
            .ok_or_else(|| corrupt("Native ANN distance column absent or malformed"))?;
        if distances.null_count() != 0 {
            return Err(corrupt("Native ANN null distance"));
        }
        for row in 0..batch.num_rows() {
            budget.checkpoint()?;
            if inspected == fetch {
                return Err(IndexError::BackendViolation(
                    "Native ANN exceeded candidate cap".into(),
                ));
            }
            inspected = inspected
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            let distance = distances.value(row);
            if !distance.is_finite() {
                return Err(corrupt("Native ANN nonfinite distance"));
            }
            let raw_vector_id = vector_ids.value(row);
            if raw_vector_id.is_empty() || raw_vector_id.len() > MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES
            {
                return Err(corrupt("Native ANN invalid vector ID length"));
            }
            let mut vector_id = Vec::new();
            vector_id
                .try_reserve_exact(raw_vector_id.len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            if vector_id.capacity() > MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES {
                return Err(IndexError::ResourceLimited);
            }
            vector_id.extend_from_slice(raw_vector_id);
            let candidate = RegionCandidate {
                owner: OwnerKey {
                    producer: ProducerId(id(producer.value(row))),
                    stable_id: StableOwnerId(id(owner.value(row))),
                },
                member: digest(member.value(row)),
                vector_id,
                input_stamp: digest(input.value(row)),
            };
            if seen.iter().any(|prior| {
                prior.owner == candidate.owner
                    && prior.member == candidate.member
                    && prior.vector_id == candidate.vector_id
            }) {
                return Err(corrupt("Native ANN duplicate logical candidate"));
            }
            accept(&candidate, budget)?;
            seen.push(candidate);
        }
        Ok(())
    };
    settle_query_batches(batches.map_err(backend), accept_batch, || {
        region.stop_reads();
    })
    .await?;
    budget.checkpoint()?;
    let approximate = matches!(
        region.index.contract().index,
        DenseIndexV1::Approximate { .. }
    );
    let capped = fetch < rows || inspected < fetch;
    let traversal = if approximate {
        NativeSemanticTraversal::Approximate { capped }
    } else if capped {
        NativeSemanticTraversal::ExactCapped
    } else {
        NativeSemanticTraversal::ExactExhausted
    };
    Ok(PrefixResult {
        page: RegionPage {
            inspected: u32::try_from(inspected).map_err(|_error| IndexError::ResourceLimited)?,
            traversal,
            requested: fetch,
            last_prefix: fetch,
        },
        seen,
        seen_memory,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::index::{
        MemoryLeaseOwner, MemoryScope, MemoryScopePort, ObjectRef, WorkLimits,
    };
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, Instant},
    };

    #[derive(Default)]
    struct MemoryCount {
        live: AtomicU64,
        retained_requests: AtomicU64,
    }

    impl MemoryLeaseOwner for MemoryCount {
        fn release(&self, bytes: u64) {
            let previous = self.live.fetch_sub(bytes, Ordering::SeqCst);
            assert!(previous >= bytes);
        }
    }

    struct CountedScope(Arc<MemoryCount>);

    impl MemoryScopePort for CountedScope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            let _previous = self.0.live.fetch_add(bytes, Ordering::SeqCst);
            if lifetime == MemoryLifetime::Retained {
                let _previous = self.0.retained_requests.fetch_add(1, Ordering::SeqCst);
            }
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }

    struct LimitedScope(Arc<MemoryCount>, u64);

    impl MemoryScopePort for LimitedScope {
        fn reserve(
            &self,
            bytes: u64,
            _lifetime: MemoryLifetime,
        ) -> Result<MemoryLease, IndexError> {
            let _prior = self
                .0
                .live
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                    live.checked_add(bytes).filter(|next| *next <= self.1)
                })
                .map_err(|_current| IndexError::ResourceLimited)?;
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }

    fn verify(condition: bool, message: &'static str) -> Result<(), Box<dyn std::error::Error>> {
        if condition {
            Ok(())
        } else {
            Err(Box::new(std::io::Error::other(message)))
        }
    }

    fn budget() -> Result<WorkBudget, IndexError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(60))
            .ok_or(IndexError::ResourceLimited)?;
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 100_000_000,
                mutations: 100,
            },
            deadline,
        ))
    }

    #[test]
    fn cancelled_ann_query_settles_started_batch_before_returning()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        crate::run_blocking(&runtime, async {
            let work = budget()?;
            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = tokio::sync::oneshot::channel();
            let batch = RecordBatch::new_empty(Arc::new(Schema::empty()));
            let running_batch = batch.clone();
            let child = tokio::task::spawn_blocking(move || {
                started_tx.send(()).map_err(|_closed| {
                    IndexError::BackendViolation("ANN batch start receiver closed".into())
                })?;
                release_rx.blocking_recv().map_err(|_closed| {
                    IndexError::BackendViolation("ANN batch release sender closed".into())
                })?;
                Ok(running_batch)
            });
            started_rx.await?;

            let remaining = futures::stream::once(async move {
                child
                    .await
                    .map_err(|error| IndexError::Storage(format!("ANN batch task: {error}")))?
            });
            let batches = Box::pin(futures::StreamExt::chain(
                futures::stream::iter([Ok(batch)]),
                remaining,
            ));
            work.request().cancel_handle().cancel();
            let mut callbacks = 0;
            let mut settle = Box::pin(settle_query_batches(
                batches,
                |_batch| {
                    callbacks += 1;
                    work.checkpoint()
                },
                || {},
            ));
            verify(
                futures::poll!(settle.as_mut()).is_pending(),
                "ANN cancellation returned while the next batch was still running",
            )?;
            release_tx
                .send(())
                .map_err(|_closed| "ANN batch ended early")?;
            verify(
                settle.await == Err(IndexError::Cancelled),
                "ANN drain discarded the original cancellation",
            )?;
            verify(
                callbacks == 1,
                "ANN drain admitted more batches after refusal",
            )
        })
    }

    #[test]
    fn first_batch_error_signals_cooperative_tail_and_wins_settlement()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        crate::run_blocking(&runtime, async {
            let (stop, stopped) = tokio::sync::oneshot::channel();
            let batch = RecordBatch::new_empty(Arc::new(Schema::empty()));
            let tail = futures::stream::once(async move {
                stopped.await.map_err(|error| {
                    IndexError::Storage(format!("ANN stop signal closed: {error}"))
                })?;
                Err(IndexError::Cancelled)
            });
            let batches = Box::pin(futures::stream::iter([Ok(batch)]).chain(tail));
            let first = IndexError::Storage("ANN first consumer error".into());
            let mut callbacks = 0usize;
            let result = tokio::time::timeout(
                Duration::from_secs(2),
                settle_query_batches(
                    batches,
                    |_batch| {
                        callbacks = callbacks.saturating_add(1);
                        Err(first.clone())
                    },
                    || {
                        let _sent = stop.send(());
                    },
                ),
            )
            .await?;
            verify(result == Err(first), "tail replaced the first ANN error")?;
            verify(
                callbacks == 1,
                "tail invoked the ANN consumer after refusal",
            )
        })
    }

    fn owner() -> OwnerKey {
        OwnerKey {
            producer: ProducerId([1; 16]),
            stable_id: StableOwnerId([2; 16]),
        }
    }

    fn input(recipe: Digest) -> OwnerInputBinding {
        OwnerInputBinding {
            source: Some(ObjectRef(Digest([3; 32]))),
            projection: None,
            producer_context: Digest([4; 32]),
            derivation_policy: recipe,
        }
    }

    fn row<'a>(vector_id: &'a [u8], vector: &'a [f32], recipe: Digest) -> RegionRow<'a> {
        RegionRow {
            owner: owner(),
            member: Digest([8; 32]),
            vector_id,
            input: input(recipe),
            vector,
        }
    }

    #[test]
    fn refill_skips_repeated_candidate_and_refuses_changed_stamp() -> Result<(), IndexError> {
        let mut budget = budget()?;
        let mut decisions = RefillDecisions::new();
        let candidate = RegionCandidate {
            owner: owner(),
            member: Digest([7; 32]),
            vector_id: b"chunk-a".to_vec(),
            input_stamp: Digest([8; 32]),
        };
        let mut calls = 0;
        let mut accept = |_: &RegionCandidate, _: &mut WorkBudget| {
            calls += 1;
            Ok(Some(RefillProgress::SourceFile(owner())))
        };
        decisions.admit(&candidate, &mut budget, 2, 2, &mut accept)?;
        decisions.remember_prefix(vec![candidate.clone()], None)?;
        decisions.admit(&candidate, &mut budget, 2, 2, &mut accept)?;
        let mut same_file = candidate.clone();
        same_file.vector_id = b"chunk-b".to_vec();
        decisions.admit(&same_file, &mut budget, 2, 2, &mut accept)?;
        decisions.remember_prefix(vec![candidate.clone(), same_file.clone()], None)?;
        let mut changed = candidate.clone();
        changed.input_stamp = Digest([9; 32]);
        assert!(matches!(
            decisions.admit(&changed, &mut budget, 2, 2, &mut accept),
            Err(IndexError::CorruptObject(_))
        ));
        assert_eq!(calls, 2);
        assert_eq!(decisions.prior_prefixes.iter().flatten().count(), 2);
        assert_eq!(
            decisions.prior_prefixes[1]
                .as_ref()
                .map(|prefix| &prefix.candidates),
            Some(&vec![candidate, same_file])
        );
        assert_eq!(decisions.file_owners, vec![owner()]);
        assert_eq!(decisions.accepted_count, 1);
        Ok(())
    }

    #[test]
    fn vector_refill_counts_rows_without_file_state() -> Result<(), IndexError> {
        let mut budget = budget()?;
        let mut decisions = RefillDecisions::new();
        let mut candidate = RegionCandidate {
            owner: owner(),
            member: Digest([7; 32]),
            vector_id: b"chunk-a".to_vec(),
            input_stamp: Digest([8; 32]),
        };
        let mut calls = 0;
        let mut accept = |_: &RegionCandidate, _: &mut WorkBudget| {
            calls += 1;
            Ok(Some(RefillProgress::Vector))
        };
        decisions.admit(&candidate, &mut budget, 2, 2, &mut accept)?;
        candidate.vector_id = b"chunk-b".to_vec();
        decisions.admit(&candidate, &mut budget, 2, 2, &mut accept)?;
        assert_eq!(calls, 2);
        assert_eq!(decisions.accepted_count, 2);
        assert_eq!(decisions.file_owners.capacity(), 0);
        assert!(decisions._file_owners_memory.is_none());
        Ok(())
    }

    #[test]
    fn refill_prefix_schedule_matches_cumulative_work_oracle() {
        // Enumerate small policies and the production ceiling edges. The
        // independent oracle considers every legal next physical prefix;
        // repeated rows still spend work on every request.
        let ceiling = [
            MAX_REGION_CANDIDATES.saturating_sub(1),
            MAX_REGION_CANDIDATES,
        ];
        for cap in (1..=64).chain(ceiling) {
            for rows in (0..=64).chain(ceiling) {
                for initial in 1..=cap {
                    let mut prefix = initial.min(rows);
                    let mut spent = prefix;
                    let mut remembered = 0usize;
                    loop {
                        let legal = (prefix.saturating_add(1)..=rows)
                            .filter(|next| *next <= prefix.saturating_mul(2))
                            .filter(|next| spent.checked_add(*next).is_some_and(|sum| sum <= cap))
                            .next_back();
                        assert!(spent <= cap);
                        let actual = next_prefix(prefix, rows, cap.saturating_sub(spent));
                        assert_eq!(actual, legal, "cap={cap} rows={rows} initial={initial}");
                        let Some(next) = actual else { break };
                        remembered = remembered.saturating_add(1);
                        assert!(remembered <= MAX_REFILL_PREFIXES);
                        spent = spent.saturating_add(next);
                        prefix = next;
                    }
                }
            }
        }
    }

    #[test]
    fn rescore_window_still_checks_repeated_candidate_stamp() -> Result<(), IndexError> {
        let mut budget = budget()?;
        let mut decisions = RefillDecisions::new();
        let candidate = RegionCandidate {
            owner: owner(),
            member: Digest([7; 32]),
            vector_id: b"chunk-a".to_vec(),
            input_stamp: Digest([8; 32]),
        };
        let mut calls = 0;
        let mut accept = |_: &RegionCandidate, _: &mut WorkBudget| {
            calls += 1;
            Ok(Some(RefillProgress::Vector))
        };
        decisions.admit(&candidate, &mut budget, 1, 1, &mut accept)?;
        decisions.remember_prefix(vec![candidate.clone()], None)?;
        let mut changed = candidate.clone();
        changed.input_stamp = Digest([9; 32]);
        assert!(matches!(
            decisions.admit(&changed, &mut budget, 1, 1, &mut accept),
            Err(IndexError::CorruptObject(_))
        ));
        let mut next = candidate;
        next.vector_id = b"chunk-b".to_vec();
        decisions.admit(&next, &mut budget, 1, 1, &mut accept)?;
        assert_eq!(calls, 1);
        assert!(decisions.skipped_after_rescore_cap);
        Ok(())
    }

    #[test]
    fn input_stamp_refuses_selected_view_aba() {
        let recipe = Digest([9; 32]);
        let original = input(recipe);
        let candidate = RegionCandidate {
            owner: owner(),
            member: Digest([8; 32]),
            vector_id: b"whole-file".to_vec(),
            input_stamp: owner_input_stamp(original),
        };
        assert!(selected_input_matches(
            &candidate,
            owner(),
            candidate.member,
            b"whole-file",
            original
        ));
        let changed = OwnerInputBinding {
            source: Some(ObjectRef(Digest([5; 32]))),
            ..original
        };
        assert!(!selected_input_matches(
            &candidate,
            owner(),
            candidate.member,
            b"whole-file",
            changed
        ));
        assert!(!selected_input_matches(
            &candidate,
            owner(),
            candidate.member,
            b"different",
            original
        ));
    }

    #[test]
    fn row_identity_accepts_multiple_vectors_and_rejects_bad_ids()
    -> Result<(), Box<dyn std::error::Error>> {
        let recipe = Digest([9; 32]);
        let vector = [1.0, 0.0, 0.0];
        let rows = [
            row(b"chunk-a", &vector, recipe),
            row(b"chunk-b", &vector, recipe),
        ];
        let digest = validate_rows(&rows, recipe, 3)?;
        verify(
            digest
                != validate_rows(
                    &[
                        row(b"chunk-a", &vector, recipe),
                        row(b"chunk-c", &vector, recipe),
                    ],
                    recipe,
                    3,
                )?,
            "changed vector ID retained the same seal",
        )?;
        verify(
            matches!(
                validate_rows(
                    &[
                        row(b"chunk-a", &vector, recipe),
                        row(b"chunk-a", &vector, recipe),
                    ],
                    recipe,
                    3,
                ),
                Err(IndexError::InvalidInput(_))
            ),
            "duplicate vector ID was accepted",
        )?;
        verify(
            matches!(
                validate_rows(&[row(b"", &vector, recipe)], recipe, 3),
                Err(IndexError::InvalidInput(_))
            ),
            "empty vector ID was accepted",
        )?;
        let oversized = vec![
            42;
            MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?
        ];
        verify(
            matches!(
                validate_rows(&[row(&oversized, &vector, recipe)], recipe, 3),
                Err(IndexError::InvalidInput(_))
            ),
            "oversized vector ID was accepted",
        )?;
        Ok(())
    }

    #[test]
    fn near_full_initial_prefix_does_not_reserve_unneeded_refill_memory()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        crate::run_blocking(&runtime, async {
            let root = tempfile::tempdir()?;
            let uri = local_fixture_uri(root.path(), "near_full_refill")?;
            let recipe = Digest([9; 32]);
            let vector = [1.0, 0.0, 0.0];
            let ids: Vec<[u8; 2]> = (0..64u8).map(|ordinal| [b'v', ordinal]).collect();
            let rows: Vec<_> = ids
                .iter()
                .map(|vector_id| row(vector_id, &vector, recipe))
                .collect();
            let mut build = budget()?;
            let (dataset, seal) = build_region_at(
                &uri,
                Arc::new(Session::default()),
                &rows,
                recipe,
                1,
                3,
                &mut build,
            )
            .await?;
            let opened = open_region(dataset, seal, &mut build).await?;
            // Each first prefix fits by itself. Both refill policies already
            // have a result, so neither must retain cross-prefix identities.
            for (initial, progress) in [
                (64, RefillProgress::Vector),
                (63, RefillProgress::Vector),
                (16, RefillProgress::Vector),
                (63, RefillProgress::SourceFile(owner())),
            ] {
                let per_row = u64::try_from(std::mem::size_of::<RegionCandidate>())?
                    .checked_add(u64::try_from(MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES)?)
                    .and_then(|bytes| bytes.checked_add(512))
                    .ok_or(IndexError::ResourceLimited)?;
                let scratch_limit = u64::try_from(initial)?
                    .checked_mul(per_row)
                    .and_then(|bytes| bytes.checked_add(3 * 4 + 4096))
                    .ok_or(IndexError::ResourceLimited)?;
                let memory = Arc::new(MemoryCount::default());
                let mut work = budget()?;
                work.bind_memory_scope(Some(MemoryScope::new(Arc::new(LimitedScope(
                    memory.clone(),
                    scratch_limit,
                )))));
                let mut accepted = false;
                let page = query_region(&opened, &vector, 1, initial, 64, &mut work, |_hit, _| {
                    accepted = true;
                    Ok(Some(progress))
                })
                .await?;
                verify(accepted, "query did not accept its first candidate")?;
                verify(
                    page.inspected == u32::try_from(initial)? && page.requested == initial,
                    "query consumed an unnecessary refill prefix",
                )?;
                verify(
                    memory.live.load(Ordering::SeqCst) == 0,
                    "near-full query retained scoped scratch memory",
                )?;
            }
            Ok(())
        })
    }

    #[test]
    fn exact_region_obeys_selected_filter_and_releases_query_memory()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        crate::run_blocking(&runtime, exact_region_body())
    }

    async fn exact_region_body() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let uri = local_fixture_uri(root.path(), "region")?;
        let recipe = Digest([9; 32]);
        let vectors = [
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let ids: [&[u8]; 4] = [b"chunk-a", b"chunk-b", b"chunk-c", b"chunk-d"];
        let rows: Vec<_> = vectors
            .iter()
            .zip(ids)
            .enumerate()
            .map(|(position, (vector, vector_id))| RegionRow {
                owner: owner(),
                member: Digest([u8::from(position == 3); 32]),
                vector_id,
                input: input(recipe),
                vector,
            })
            .collect();
        let mut work = budget()?;
        let memory = Arc::new(MemoryCount::default());
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(CountedScope(
            memory.clone(),
        )))));
        let (dataset, seal) = build_region_at(
            &uri,
            Arc::new(Session::default()),
            &rows,
            recipe,
            1,
            3,
            &mut work,
        )
        .await?;
        verify(
            memory.retained_requests.load(Ordering::SeqCst) == 1,
            "spawned Arrow input was not admitted to escape execution",
        )?;
        let changed_vector = [0.5, 0.5, 0.0];
        let mut changed_rows = rows.clone();
        changed_rows
            .first_mut()
            .ok_or("region fixture has no row")?
            .vector = &changed_vector;
        let tampered_vector_dataset = write_fixture_rows(
            &local_fixture_uri(root.path(), "tampered_vector")?,
            arrow_batch(&changed_rows, 3)?,
        )
        .await?;
        verify(
            matches!(
                row_attestation::attest(&tampered_vector_dataset, &seal, &mut work).await,
                Err(IndexError::CorruptObject(_))
            ),
            "physical vector tamper matched the original row seal",
        )?;
        let mut changed_rows = rows.clone();
        changed_rows
            .first_mut()
            .ok_or("region fixture has no row")?
            .input
            .producer_context = Digest([7; 32]);
        let tampered_stamp_dataset = write_fixture_rows(
            &local_fixture_uri(root.path(), "tampered_stamp")?,
            arrow_batch(&changed_rows, 3)?,
        )
        .await?;
        verify(
            matches!(
                row_attestation::attest(&tampered_stamp_dataset, &seal, &mut work).await,
                Err(IndexError::CorruptObject(_))
            ),
            "physical input stamp tamper matched the original row seal",
        )?;
        verify(
            memory.live.load(Ordering::SeqCst) == 0,
            "completed build retained input memory",
        )?;
        verify(seal.vector_index.ann.is_none(), "tiny region is not exact")?;
        verify(seal.recipe == recipe, "region seal changed recipe")?;
        verify(
            seal.rows_digest == validate_rows(&rows, recipe, 3)?,
            "region seal changed vector row identities",
        )?;
        let opened = open_region(dataset, seal, &mut work).await?;
        let mut accepted = Vec::new();
        let page = query_region(
            &opened,
            &[1.0, 0.0, 0.0],
            2,
            4,
            4,
            &mut work,
            |candidate, _| {
                verify(
                    memory.live.load(Ordering::SeqCst) > 0,
                    "query callback ran without scratch admission",
                )
                .map_err(|error| IndexError::Storage(error.to_string()))?;
                if candidate.vector_id != b"chunk-a" {
                    accepted.push(candidate.clone());
                    Ok(Some(RefillProgress::Vector))
                } else {
                    Ok(None)
                }
            },
        )
        .await?;
        verify(
            accepted
                .iter()
                .map(|hit| hit.vector_id.as_slice())
                .collect::<std::collections::BTreeSet<_>>()
                == std::collections::BTreeSet::from([
                    b"chunk-b".as_slice(),
                    b"chunk-c".as_slice(),
                    b"chunk-d".as_slice(),
                ]),
            "selected callback accepted wrong vector IDs",
        )?;
        verify(
            accepted.iter().all(|hit| {
                selected_input_matches(hit, owner(), hit.member, &hit.vector_id, input(recipe))
            }),
            "selected vector binding mismatch",
        )?;
        verify(
            !matches!(page.traversal, NativeSemanticTraversal::Approximate { .. }),
            "tiny region reported approximate",
        )?;
        verify(
            page.traversal == NativeSemanticTraversal::ExactExhausted,
            "complete tiny region reported capped",
        )?;
        verify(page.inspected == 4, "query did not inspect bounded rows")?;
        verify(
            memory.live.load(Ordering::SeqCst) == 0,
            "completed query retained scratch memory",
        )?;
        let reopened_dataset = Arc::new(Dataset::open(&uri).await?);
        let reopened = open_region(reopened_dataset, opened.seal.clone(), &mut work).await?;
        let mut reopened_accepted = Vec::new();
        let reopened_page = query_region(
            &reopened,
            &[1.0, 0.0, 0.0],
            2,
            4,
            4,
            &mut work,
            |candidate, _| {
                if candidate.vector_id != b"chunk-a" {
                    reopened_accepted.push(candidate.clone());
                    Ok(Some(RefillProgress::Vector))
                } else {
                    Ok(None)
                }
            },
        )
        .await?;
        verify(
            reopened_accepted
                .iter()
                .map(|hit| hit.vector_id.as_slice())
                .collect::<std::collections::BTreeSet<_>>()
                == accepted
                    .iter()
                    .map(|hit| hit.vector_id.as_slice())
                    .collect::<std::collections::BTreeSet<_>>(),
            "reopen changed vector identity mapping",
        )?;
        verify(
            reopened_page.inspected == 4,
            "reopen changed inspected rows",
        )?;
        verify(
            memory.live.load(Ordering::SeqCst) == 0,
            "reopened query retained scratch memory",
        )?;
        Ok(())
    }

    #[test]
    fn filtered_refill_preserves_cumulative_cap_and_original_work()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        crate::run_blocking(&runtime, filtered_refill_body())
    }

    async fn filtered_refill_body() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let uri = local_fixture_uri(root.path(), "refill")?;
        let recipe = Digest([9; 32]);
        // Strictly decreasing cosine to [1,0,0]; only the eighth row is eligible.
        let vectors = [
            [8.0, 1.0, 0.0],
            [7.0, 1.0, 0.0],
            [6.0, 1.0, 0.0],
            [5.0, 1.0, 0.0],
            [4.0, 1.0, 0.0],
            [3.0, 1.0, 0.0],
            [2.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
        ];
        let ids: [&[u8]; 8] = [b"a", b"b", b"c", b"d", b"e", b"f", b"g", b"h"];
        let rows: Vec<_> = vectors
            .iter()
            .zip(ids)
            .map(|(vector, id)| row(id, vector, recipe))
            .collect();
        let mut build = budget()?;
        let (dataset, seal) = build_region_at(
            &uri,
            Arc::new(Session::default()),
            &rows,
            recipe,
            1,
            3,
            &mut build,
        )
        .await?;
        let opened = open_region(dataset, seal, &mut build).await?;
        let memory = Arc::new(MemoryCount::default());
        let mut work = budget()?;
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(CountedScope(
            memory.clone(),
        )))));
        let mut calls = 0u32;
        let mut accepted = Vec::new();
        let page = query_region(&opened, &[1.0, 0.0, 0.0], 1, 2, 14, &mut work, |hit, _| {
            calls = calls.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            if memory.live.load(Ordering::SeqCst) == 0 {
                return Err(IndexError::ResourceLimited);
            }
            if hit.vector_id == b"h" {
                accepted.push(hit.vector_id.clone());
                Ok(Some(RefillProgress::Vector))
            } else {
                Ok(None)
            }
        })
        .await?;
        verify(accepted == [b"h".to_vec()], "refill missed eligible tail")?;
        verify(
            page.inspected == 14 && calls == 8,
            "repeated prefixes revalidated an already verified candidate",
        )?;
        verify(
            work.usage().reads == 3,
            "refill did not charge each backend prefix",
        )?;
        verify(
            page.traversal == NativeSemanticTraversal::ExactExhausted,
            "exhaustive exact prefix was mislabeled",
        )?;
        verify(
            memory.live.load(Ordering::SeqCst) == 0,
            "completed refill retained scratch memory",
        )?;

        let mut second_file = owner();
        second_file.stable_id.0[0] ^= 1;
        let mut reached_tail = false;
        let page = query_region(&opened, &[1.0, 0.0, 0.0], 2, 4, 12, &mut work, |hit, _| {
            if hit.vector_id == b"h" {
                reached_tail = true;
                Ok(Some(RefillProgress::SourceFile(second_file)))
            } else {
                // Seven distinct rows belong to the same source file.
                Ok(Some(RefillProgress::SourceFile(owner())))
            }
        })
        .await?;
        verify(
            reached_tail
                && page.inspected == 12
                && page.traversal == NativeSemanticTraversal::ExactExhausted,
            "same-file rows stopped refill before a second file",
        )?;
        verify(
            memory.live.load(Ordering::SeqCst) == 0,
            "same-file refill retained scratch memory",
        )?;

        let mut vector_work = WorkBudget::new(
            WorkLimits {
                reads: 1,
                writes: 0,
                bytes: 100_000_000,
                mutations: 0,
            },
            Instant::now()
                .checked_add(Duration::from_secs(60))
                .ok_or(IndexError::ResourceLimited)?,
        );
        vector_work.bind_memory_scope(Some(MemoryScope::new(Arc::new(CountedScope(
            memory.clone(),
        )))));
        let mut vector_callbacks = 0usize;
        let page = query_region(
            &opened,
            &[1.0, 0.0, 0.0],
            2,
            4,
            12,
            &mut vector_work,
            |_, _| {
                vector_callbacks += 1;
                Ok(Some(RefillProgress::Vector))
            },
        )
        .await?;
        verify(
            page.inspected == 4
                && page.traversal == NativeSemanticTraversal::ExactCapped
                && vector_callbacks == 4
                && vector_work.usage().reads == 1,
            "distinct vectors of one owner widened beyond the funded first prefix",
        )?;
        verify(
            memory.live.load(Ordering::SeqCst) == 0,
            "vector refill retained scratch memory",
        )?;

        let mut calls = 0u32;
        let mut accepted = Vec::new();
        let page = query_region(&opened, &[1.0, 0.0, 0.0], 1, 2, 8, &mut work, |hit, _| {
            calls = calls.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            if hit.vector_id == b"h" {
                accepted.push(hit.vector_id.clone());
                Ok(Some(RefillProgress::Vector))
            } else {
                Ok(None)
            }
        })
        .await?;
        verify(
            accepted.is_empty() && page.traversal == NativeSemanticTraversal::ExactCapped,
            "cap exhaustion claimed empty corpus",
        )?;
        verify(
            page.inspected == 6 && calls == 4,
            "refill exceeded cumulative candidate cap or repeated callbacks",
        )?;
        verify(
            memory.live.load(Ordering::SeqCst) == 0,
            "capped refill retained memory",
        )?;

        let before = work.usage().reads;
        let page = query_region(&opened, &[1.0, 0.0, 0.0], 2, 2, 14, &mut work, |_, _| {
            Ok(Some(RefillProgress::OwnerLimitReached))
        })
        .await?;
        verify(
            page.inspected == 2
                && page.traversal == NativeSemanticTraversal::ExactCapped
                && work.usage().reads.checked_sub(before) == Some(1),
            "owner limit admitted a refill prefix",
        )?;

        let mut accepted = Vec::new();
        let page = query_region(&opened, &[1.0, 0.0, 0.0], 2, 4, 12, &mut work, |hit, _| {
            if hit.vector_id == b"a" || hit.vector_id == b"h" {
                accepted.push(hit.vector_id.clone());
                Ok(Some(RefillProgress::Vector))
            } else {
                Ok(None)
            }
        })
        .await?;
        verify(
            accepted == [b"a".to_vec(), b"h".to_vec()],
            "refill duplicated a logical hit across prefixes",
        )?;
        verify(
            page.inspected == 12 && page.traversal == NativeSemanticTraversal::ExactExhausted,
            "refill did not exhaust the immutable exact region",
        )?;
        verify(
            memory.live.load(Ordering::SeqCst) == 0,
            "deduplicated refill leaked memory",
        )?;

        let mut accepted = Vec::new();
        let page = query_region(&opened, &[1.0, 0.0, 0.0], 1, 8, 8, &mut work, |hit, _| {
            if hit.vector_id == b"h" {
                accepted.push(hit.vector_id.clone());
                Ok(Some(RefillProgress::Vector))
            } else {
                Ok(None)
            }
        })
        .await?;
        verify(
            accepted == [b"h".to_vec()]
                && page.inspected == 8
                && page.traversal == NativeSemanticTraversal::ExactExhausted,
            "explicit one-prefix policy lost an eligible tail within its original cap",
        )?;
        verify(
            memory.live.load(Ordering::SeqCst) == 0,
            "one-prefix query leaked memory",
        )?;

        // Refill can inspect a full physical prefix after the selected
        // original-vector window has been funded. The remaining rows must be
        // drained without another rank callback or an exactness claim.
        let reads_before = work.usage().reads;
        let mut rescored_ids = Vec::new();
        let mut callback_ids = Vec::new();
        let page = query_region(&opened, &[1.0, 0.0, 0.0], 2, 2, 14, &mut work, |hit, _| {
            callback_ids.push(hit.vector_id.clone());
            if hit.vector_id == b"c" || hit.vector_id == b"e" {
                rescored_ids.push(hit.vector_id.clone());
                Ok(Some(RefillProgress::Vector))
            } else {
                Ok(None)
            }
        })
        .await?;
        verify(
            rescored_ids == [b"c".to_vec(), b"e".to_vec()]
                && callback_ids
                    == [
                        b"a".to_vec(),
                        b"b".to_vec(),
                        b"c".to_vec(),
                        b"d".to_vec(),
                        b"e".to_vec(),
                    ]
                && page.inspected == 14
                && page.traversal == NativeSemanticTraversal::ExactCapped
                && work.usage().reads == reads_before + 3,
            "refill ranked past the selected window or failed to drain its physical prefix",
        )?;

        // Two vectors of the same source file cannot satisfy two file slots.
        // Refill may cross the rescore window until a second file is found.
        let mut file_ids = Vec::new();
        let mut second_file = owner();
        second_file.stable_id.0[0] ^= 1;
        let page = query_region(&opened, &[1.0, 0.0, 0.0], 2, 2, 14, &mut work, |hit, _| {
            file_ids.push(hit.vector_id.clone());
            let progress =
                if hit.vector_id == b"c" || hit.vector_id == b"d" || hit.vector_id == b"e" {
                    Some(RefillProgress::SourceFile(owner()))
                } else if hit.vector_id == b"f" {
                    Some(RefillProgress::SourceFile(second_file))
                } else {
                    None
                };
            Ok(progress)
        })
        .await?;
        verify(
            file_ids
                == [
                    b"a".to_vec(),
                    b"b".to_vec(),
                    b"c".to_vec(),
                    b"d".to_vec(),
                    b"e".to_vec(),
                    b"f".to_vec(),
                ]
                && page.inspected == 14
                && page.traversal == NativeSemanticTraversal::ExactCapped,
            "same-file duplicates stopped refill or ranked past its complete file result",
        )?;
        verify(
            memory.live.load(Ordering::SeqCst) == 0,
            "window-limited refill retained scratch memory",
        )?;

        let mut denied = WorkBudget::new(
            WorkLimits {
                reads: 1,
                writes: 0,
                bytes: 100_000_000,
                mutations: 0,
            },
            Instant::now()
                .checked_add(Duration::from_secs(60))
                .ok_or(IndexError::ResourceLimited)?,
        );
        denied.bind_memory_scope(Some(MemoryScope::new(Arc::new(CountedScope(
            memory.clone(),
        )))));
        let mut calls = 0u32;
        let refused = query_region(&opened, &[1.0, 0.0, 0.0], 1, 2, 14, &mut denied, |_, _| {
            calls = calls.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            Ok(None)
        })
        .await;
        verify(
            matches!(refused, Err(IndexError::ResourceLimited)),
            "refill reset the original work limit",
        )?;
        verify(
            calls == 2 && denied.usage().reads == 1,
            "denied second prefix executed",
        )?;
        verify(
            memory.live.load(Ordering::SeqCst) == 0,
            "denied refill leaked memory",
        )?;

        let failed = query_region(&opened, &[1.0, 0.0, 0.0], 1, 2, 14, &mut work, |_, _| {
            Err(IndexError::Cancelled)
        })
        .await;
        verify(
            matches!(failed, Err(IndexError::Cancelled)),
            "filter failure returned a success prefix",
        )?;
        verify(
            memory.live.load(Ordering::SeqCst) == 0,
            "failed filter leaked memory",
        )?;
        Ok(())
    }
}
