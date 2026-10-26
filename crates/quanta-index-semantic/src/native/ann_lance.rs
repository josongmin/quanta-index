//! Bounded immutable Lance regions for the Native semantic lane.
//!
//! The caller owns the admitted staging connection, the selected-view member
//! directory, and physical-file custody. This module cannot publish a region.

use std::{future::Future, sync::Arc, time::Duration};

use arrow_array::{
    Array, BinaryArray, FixedSizeBinaryArray, FixedSizeListArray, Float32Array, RecordBatch,
    builder::{BinaryBuilder, FixedSizeBinaryBuilder},
};
use arrow_schema::{DataType, Field, Schema};
use futures::TryStreamExt as _;
use lancedb::{
    DistanceType,
    query::{ExecutableQuery as _, QueryBase as _, Select},
};
use quanta_index_contract::native_protocol::{
    MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES, NativeSemanticTraversal,
};
use quanta_index_core::RequestBudgetV1;
use quanta_index_core::domains::semantic::DenseIndexV1;
use quanta_index_core::index::{
    Digest, IndexError, MemoryLease, MemoryLifetime, OwnerInputBinding, OwnerKey, ProducerId,
    StableOwnerId, WorkBudget, WorkUsage,
};
use sha2::{Digest as _, Sha256};

use crate::{
    manifest::VectorIndexSealV1,
    vector_index::{
        LoadedVectorIndexV1, VectorIndexSealInputV1, seal_vector_index_v1, verify_vector_index_v1,
    },
};

const ROW_DOMAIN: &[u8] = b"quanta-index-native-ann-region-rows-v2\0";
const INPUT_DOMAIN: &[u8] = b"quanta-index-native-ann-owner-input-v1\0";
const COLUMN_VECTOR: &str = "vector";
const COLUMN_PRODUCER: &str = "producer";
const COLUMN_OWNER: &str = "owner";
const COLUMN_MEMBER: &str = "member";
const COLUMN_VECTOR_ID: &str = "vector_id";
const COLUMN_INPUT: &str = "input_stamp";
const COLUMN_DISTANCE: &str = "_distance";

mod custody;
mod row_attestation;
#[cfg(test)]
mod trained_tests;
mod write_settle;

/// A region can contain multiple owners; the cap is independent of corpus size.
/// Regions smaller than this project's index floor seal as bounded exact.
pub(super) const MAX_REGION_ROWS: usize = 8192;
pub(super) const MAX_REGION_DIMENSION: usize = 4096;
pub(super) const MAX_REGION_CELLS: usize = 1_048_576;
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
    pub cosine: f32,
}

/// Traversal describes only this immutable region, independently of durable
/// source coverage. Approximate traversal never certifies exhaustive recall.
#[derive(Debug)]
pub(super) struct RegionPage {
    hits: Vec<RegionCandidate>,
    pub inspected: u32,
    pub traversal: NativeSemanticTraversal,
    _memory: Option<MemoryLease>,
}

impl RegionPage {
    #[cfg(test)]
    #[must_use]
    pub(super) fn hits(&self) -> &[RegionCandidate] {
        &self.hits
    }
}

pub(super) struct OpenRegion {
    table: lancedb::Table,
    index: LoadedVectorIndexV1,
    seal: RegionSeal,
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

async fn interruptible<T>(
    request: &RequestBudgetV1,
    future: impl Future<Output = Result<T, IndexError>>,
) -> Result<T, IndexError> {
    let future = future;
    tokio::pin!(future);
    let mut poll = tokio::time::interval(Duration::from_millis(25));
    loop {
        if request.is_cancelled() {
            return Err(IndexError::Cancelled);
        }
        if request.remaining().is_zero() {
            return Err(IndexError::ResourceLimited);
        }
        tokio::select! {
            result = &mut future => return result,
            _ = poll.tick() => {}
        }
    }
}

/// Build in a caller-owned, admitted and unpublished staging connection.
///
/// The caller must persist `RegionSeal` and register the resulting Lance files
/// under the selected member artifact before exposing a view.
pub(super) async fn build_region(
    connection: &lancedb::Connection,
    table_name: &str,
    rows: &[RegionRow<'_>],
    recipe: Digest,
    generation: u64,
    dimension: usize,
    budget: &mut WorkBudget,
) -> Result<(lancedb::Table, RegionSeal), IndexError> {
    budget.checkpoint()?;
    if table_name.is_empty()
        || !table_name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(invalid(
            "Native ANN table name must be a nonempty ASCII identifier",
        ));
    }
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
    let request = budget.request().clone();
    let table = write_settle::settle_write(&request, async {
        connection
            .create_table(table_name, batch)
            .execute()
            .await
            .map_err(backend)
    })
    .await?;
    let vector_index = interruptible(&request, async {
        seal_vector_index_v1(
            &table,
            VectorIndexSealInputV1 {
                generation,
                row_count,
                inherited: None,
            },
        )
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
    row_attestation::attest(&table, &seal, budget).await?;
    budget.checkpoint()?;
    Ok((table, seal))
}

/// Verify a bounded region already selected through Catalog. Caller must
/// attest the physical dataset bytes and the row digest before this call.
pub(super) async fn open_region(
    table: lancedb::Table,
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
    let actual = interruptible(budget.request(), async {
        table.schema().await.map_err(backend)
    })
    .await?;
    if actual.as_ref() != schema(dimension)?.as_ref() {
        return Err(corrupt("Native ANN physical schema differs from seal"));
    }
    let index = interruptible(budget.request(), async {
        verify_vector_index_v1(&table, &seal.vector_index, u64::from(seal.rows))
            .await
            .map_err(backend)
    })
    .await?;
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    Ok(OpenRegion { table, index, seal })
}

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
/// `accept` must use the pinned selected-view owner/member/input binding.
pub(super) async fn query_region(
    region: &OpenRegion,
    vector: &[f32],
    top_k: usize,
    initial_candidate_cap: usize,
    candidate_cap: usize,
    budget: &mut WorkBudget,
    mut accept: impl FnMut(&RegionCandidate, &mut WorkBudget) -> Result<bool, IndexError>,
) -> Result<RegionPage, IndexError> {
    budget.checkpoint()?;
    validate_region_query(region, vector, top_k, candidate_cap)?;
    if initial_candidate_cap < top_k || initial_candidate_cap > candidate_cap {
        return Err(invalid(
            "Native ANN initial prefix exceeds candidate work policy",
        ));
    }
    let rows = usize::try_from(region.seal.rows).map_err(|_error| IndexError::ResourceLimited)?;
    // A single-prefix policy needs no extra retained merge buffer.
    if initial_candidate_cap == candidate_cap || rows <= initial_candidate_cap {
        return query_prefix(region, vector, top_k, initial_candidate_cap, budget, accept).await;
    }
    let capacity = candidate_cap.min(rows);
    let result_bytes = capacity
        .checked_mul(std::mem::size_of::<RegionCandidate>())
        .and_then(|bytes| {
            bytes.checked_add(capacity.checked_mul(MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES)?)
        })
        .ok_or(IndexError::ResourceLimited)?;
    let result_memory = budget.reserve_memory(
        u64::try_from(result_bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )?;
    let mut hits = Vec::<RegionCandidate>::new();
    hits.try_reserve_exact(capacity)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if hits.capacity() > capacity {
        return Err(IndexError::ResourceLimited);
    }
    let mut requested = 0usize;
    let mut inspected = 0u32;
    let mut fetch = initial_candidate_cap.min(capacity);
    let mut traversal;
    loop {
        let page = query_prefix(region, vector, fetch, fetch, budget, &mut accept).await?;
        requested = requested
            .checked_add(fetch)
            .ok_or(IndexError::ResourceLimited)?;
        inspected = inspected
            .checked_add(page.inspected)
            .ok_or(IndexError::ResourceLimited)?;
        traversal = page.traversal;
        for candidate in page.hits {
            budget.checkpoint()?;
            if let Some(prior) = hits.iter().find(|prior| {
                prior.owner == candidate.owner
                    && prior.member == candidate.member
                    && prior.vector_id == candidate.vector_id
            }) {
                if prior.input_stamp != candidate.input_stamp {
                    return Err(corrupt(
                        "Native ANN refill changed immutable candidate input",
                    ));
                }
            } else {
                if hits.len() == capacity {
                    return Err(IndexError::BackendViolation(
                        "Native ANN refill exceeded result capacity".into(),
                    ));
                }
                hits.push(candidate);
            }
        }
        if hits.len() >= top_k || traversal == NativeSemanticTraversal::ExactExhausted {
            break;
        }
        let remaining = candidate_cap
            .checked_sub(requested)
            .ok_or(IndexError::ResourceLimited)?;
        let next = fetch.saturating_mul(2).min(rows).min(remaining);
        // Another equal/smaller prefix cannot advance the selected region.
        if next <= fetch {
            break;
        }
        fetch = next;
    }
    sort_candidates(&mut hits);
    hits.truncate(top_k);
    budget.checkpoint()?;
    Ok(RegionPage {
        hits,
        inspected,
        traversal,
        _memory: result_memory,
    })
}

fn sort_candidates(hits: &mut [RegionCandidate]) {
    hits.sort_by(|left, right| {
        right
            .cosine
            .total_cmp(&left.cosine)
            .then_with(|| left.owner.cmp(&right.owner))
            .then_with(|| left.member.cmp(&right.member))
            .then_with(|| left.vector_id.cmp(&right.vector_id))
    });
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

/// One immutable prefix. The caller owns the cumulative refill ceiling.
async fn query_prefix(
    region: &OpenRegion,
    vector: &[f32],
    top_k: usize,
    candidate_cap: usize,
    budget: &mut WorkBudget,
    mut accept: impl FnMut(&RegionCandidate, &mut WorkBudget) -> Result<bool, IndexError>,
) -> Result<RegionPage, IndexError> {
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
    let scratch_bytes = fetch_u64
        .checked_mul(
            candidate_bytes
                .checked_add(vector_id_max)
                .and_then(|size| size.checked_add(512))
                .ok_or(IndexError::ResourceLimited)?,
        )
        .and_then(|bytes| bytes.checked_add(vector_bytes))
        .and_then(|bytes| bytes.checked_add(4096))
        .ok_or(IndexError::ResourceLimited)?;
    let result_bytes = fetch_u64
        .checked_mul(
            candidate_bytes
                .checked_add(vector_id_max)
                .ok_or(IndexError::ResourceLimited)?,
        )
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        reads: 1,
        bytes: fetch_u64
            .checked_mul(
                128u64
                    .checked_add(vector_id_max)
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let _scratch_memory = budget.reserve_memory(scratch_bytes, MemoryLifetime::Temporary)?;
    let result_memory = budget.reserve_memory(result_bytes, MemoryLifetime::Retained)?;
    let mut query_vector = Vec::new();
    query_vector
        .try_reserve_exact(vector.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    if query_vector.capacity() > vector.len() {
        return Err(IndexError::ResourceLimited);
    }
    query_vector.extend_from_slice(vector);
    let mut hits = Vec::new();
    hits.try_reserve_exact(fetch)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut seen: Vec<(RegionCandidate, bool)> = Vec::new();
    seen.try_reserve_exact(fetch)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if hits.capacity() > fetch || seen.capacity() > fetch {
        return Err(IndexError::ResourceLimited);
    }
    let query = region
        .table
        .vector_search(query_vector)
        .map_err(backend)?
        .distance_type(DistanceType::Cosine)
        .limit(fetch)
        .select(Select::columns(&[
            COLUMN_PRODUCER,
            COLUMN_OWNER,
            COLUMN_MEMBER,
            COLUMN_VECTOR_ID,
            COLUMN_INPUT,
        ]));
    let query = region.index.apply(query, fetch).map_err(backend)?;
    let mut batches = interruptible(budget.request(), async {
        query.execute().await.map_err(backend)
    })
    .await?;
    let mut inspected = 0usize;
    while let Some(batch) = interruptible(budget.request(), async {
        batches.try_next().await.map_err(backend)
    })
    .await?
    {
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
                cosine: distance.mul_add(-1.0, 1.0),
            };
            if seen.iter().any(|(prior, _)| {
                prior.owner == candidate.owner
                    && prior.member == candidate.member
                    && prior.vector_id == candidate.vector_id
            }) {
                return Err(corrupt("Native ANN duplicate logical candidate"));
            }
            let accepted = accept(&candidate, budget)?;
            seen.push((candidate, accepted));
        }
    }
    for (candidate, accepted) in seen {
        if accepted {
            hits.push(candidate);
        }
    }
    sort_candidates(&mut hits);
    hits.truncate(top_k);
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
    Ok(RegionPage {
        hits,
        inspected: u32::try_from(inspected).map_err(|_error| IndexError::ResourceLimited)?,
        traversal,
        _memory: result_memory,
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
        time::Instant,
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
    fn input_stamp_refuses_selected_view_aba() {
        let recipe = Digest([9; 32]);
        let original = input(recipe);
        let candidate = RegionCandidate {
            owner: owner(),
            member: Digest([8; 32]),
            vector_id: b"whole-file".to_vec(),
            input_stamp: owner_input_stamp(original),
            cosine: 1.0,
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
    fn exact_region_obeys_selected_filter_and_stable_ties() -> Result<(), Box<dyn std::error::Error>>
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        crate::run_blocking(&runtime, exact_region_body())
    }

    async fn exact_region_body() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let connection = lancedb::connect(&root.path().to_string_lossy())
            .execute()
            .await?;
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
        let (table, seal) =
            build_region(&connection, "region", &rows, recipe, 1, 3, &mut work).await?;
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
        let tampered_vector = connection
            .create_table("tampered_vector", arrow_batch(&changed_rows, 3)?)
            .execute()
            .await?;
        verify(
            matches!(
                row_attestation::attest(&tampered_vector, &seal, &mut work).await,
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
        let tampered_stamp = connection
            .create_table("tampered_stamp", arrow_batch(&changed_rows, 3)?)
            .execute()
            .await?;
        verify(
            matches!(
                row_attestation::attest(&tampered_stamp, &seal, &mut work).await,
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
        let opened = open_region(table, seal, &mut work).await?;
        let page = query_region(
            &opened,
            &[1.0, 0.0, 0.0],
            2,
            4,
            4,
            &mut work,
            |candidate, _| Ok(candidate.vector_id != b"chunk-a"),
        )
        .await?;
        verify(
            page.hits()
                .iter()
                .map(|hit| hit.vector_id.as_slice())
                .collect::<Vec<_>>()
                == vec![b"chunk-b".as_slice(), b"chunk-c".as_slice()],
            "selected filter/tie returned wrong vector IDs",
        )?;
        verify(
            page.hits().iter().all(|hit| {
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
            memory.live.load(Ordering::SeqCst) > 0,
            "query result escaped without retained charge",
        )?;
        drop(page);
        verify(
            memory.live.load(Ordering::SeqCst) == 0,
            "query result leaked retained charge",
        )?;
        let reopened_table = connection.open_table("region").execute().await?;
        let reopened = open_region(reopened_table, opened.seal.clone(), &mut work).await?;
        let reopened_page = query_region(
            &reopened,
            &[1.0, 0.0, 0.0],
            2,
            4,
            4,
            &mut work,
            |candidate, _| Ok(candidate.vector_id != b"chunk-a"),
        )
        .await?;
        verify(
            reopened_page
                .hits()
                .iter()
                .map(|hit| hit.vector_id.as_slice())
                .collect::<Vec<_>>()
                == vec![b"chunk-b".as_slice(), b"chunk-c".as_slice()],
            "reopen changed vector identity mapping",
        )?;
        verify(
            memory.live.load(Ordering::SeqCst) > 0,
            "reopened result escaped without retained charge",
        )?;
        drop(reopened_page);
        verify(
            memory.live.load(Ordering::SeqCst) == 0,
            "reopened result leaked retained charge",
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
        let connection = lancedb::connect(&root.path().to_string_lossy())
            .execute()
            .await?;
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
        let (table, seal) =
            build_region(&connection, "refill", &rows, recipe, 1, 3, &mut build).await?;
        let opened = open_region(table, seal, &mut build).await?;
        let memory = Arc::new(MemoryCount::default());
        let mut work = budget()?;
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(CountedScope(
            memory.clone(),
        )))));
        let mut calls = 0u32;
        let page = query_region(&opened, &[1.0, 0.0, 0.0], 1, 2, 14, &mut work, |hit, _| {
            calls = calls.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            Ok(hit.vector_id == b"h")
        })
        .await?;
        verify(
            page.hits().len() == 1 && page.hits().first().is_some_and(|hit| hit.vector_id == b"h"),
            "refill missed eligible tail",
        )?;
        verify(
            page.inspected == 14 && calls == 14,
            "repeated prefixes escaped cumulative accounting",
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
            memory.live.load(Ordering::SeqCst) > 0,
            "refill result lost its grant",
        )?;
        drop(page);
        verify(
            memory.live.load(Ordering::SeqCst) == 0,
            "refill result did not refund",
        )?;

        let mut calls = 0u32;
        let page = query_region(&opened, &[1.0, 0.0, 0.0], 1, 2, 8, &mut work, |hit, _| {
            calls = calls.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            Ok(hit.vector_id == b"h")
        })
        .await?;
        verify(
            page.hits().is_empty() && page.traversal == NativeSemanticTraversal::ExactCapped,
            "cap exhaustion claimed empty corpus",
        )?;
        verify(
            page.inspected == 6 && calls == 6,
            "refill exceeded cumulative candidate cap",
        )?;
        drop(page);
        verify(
            memory.live.load(Ordering::SeqCst) == 0,
            "capped refill retained memory",
        )?;

        let page = query_region(&opened, &[1.0, 0.0, 0.0], 2, 4, 12, &mut work, |hit, _| {
            Ok(hit.vector_id == b"a" || hit.vector_id == b"h")
        })
        .await?;
        verify(
            page.hits()
                .iter()
                .map(|hit| hit.vector_id.as_slice())
                .collect::<Vec<_>>()
                == vec![b"a".as_slice(), b"h".as_slice()],
            "refill duplicated a logical hit across prefixes",
        )?;
        verify(
            page.inspected == 12 && page.traversal == NativeSemanticTraversal::ExactExhausted,
            "refill did not exhaust the immutable exact region",
        )?;
        drop(page);
        verify(
            memory.live.load(Ordering::SeqCst) == 0,
            "deduplicated refill leaked memory",
        )?;

        let page = query_region(&opened, &[1.0, 0.0, 0.0], 1, 8, 8, &mut work, |hit, _| {
            Ok(hit.vector_id == b"h")
        })
        .await?;
        verify(
            page.hits().len() == 1
                && page.hits().first().is_some_and(|hit| hit.vector_id == b"h")
                && page.inspected == 8
                && page.traversal == NativeSemanticTraversal::ExactExhausted,
            "explicit one-prefix policy lost an eligible tail within its original cap",
        )?;
        drop(page);
        verify(
            memory.live.load(Ordering::SeqCst) == 0,
            "one-prefix query leaked memory",
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
            Ok(false)
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
