//! Read back every physical row before a region can become an immutable file set.

use arrow_array::{Array as _, FixedSizeListArray, Float32Array};
use futures::TryStreamExt as _;
use lancedb::query::{ExecutableQuery as _, QueryBase as _, Select};
use quanta_index_contract::native_protocol::MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES;
use quanta_index_core::index::{
    Digest, IndexError, MemoryLifetime, OwnerKey, ProducerId, StableOwnerId, WorkBudget, WorkUsage,
};
use sha2::{Digest as _, Sha256};

use super::{
    COLUMN_INPUT, COLUMN_MEMBER, COLUMN_OWNER, COLUMN_PRODUCER, COLUMN_VECTOR, COLUMN_VECTOR_ID,
    MAX_REGION_CELLS, MAX_REGION_DIMENSION, MAX_REGION_ROWS, ROW_DOMAIN, RegionSeal, backend,
    binary, corrupt, digest, id, interruptible, schema, vector_ids,
};

/// The scan cap includes one surplus row so a forged count cannot be accepted.
/// The original budget funds the complete bounded Arrow result before execution.
pub(super) async fn attest(
    table: &lancedb::Table,
    seal: &RegionSeal,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    budget.checkpoint()?;
    let rows = usize::try_from(seal.rows).map_err(|_error| IndexError::ResourceLimited)?;
    let dimension =
        usize::try_from(seal.dimension).map_err(|_error| IndexError::ResourceLimited)?;
    if rows == 0
        || rows > MAX_REGION_ROWS
        || dimension == 0
        || dimension > MAX_REGION_DIMENSION
        || rows
            .checked_mul(dimension)
            .is_none_or(|cells| cells > MAX_REGION_CELLS)
    {
        return Err(corrupt(
            "Native ANN attestation seal exceeds bounded policy",
        ));
    }
    let scan_cap = rows.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    let max_id = MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES;
    let per_row = dimension
        .checked_mul(std::mem::size_of::<f32>())
        .and_then(|bytes| bytes.checked_add(max_id))
        .and_then(|bytes| bytes.checked_add(512))
        .ok_or(IndexError::ResourceLimited)?;
    let scan_bytes = scan_cap
        .checked_mul(per_row)
        .and_then(|bytes| bytes.checked_add(8192))
        .ok_or(IndexError::ResourceLimited)?;
    let admitted = u64::try_from(scan_bytes).map_err(|_error| IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        reads: 1,
        bytes: admitted,
        ..WorkUsage::default()
    })?;
    let _memory = budget.reserve_memory(admitted, MemoryLifetime::Temporary)?;
    let actual_schema = interruptible(budget.request(), async {
        table.schema().await.map_err(backend)
    })
    .await?;
    if actual_schema.as_ref() != schema(dimension)?.as_ref() {
        return Err(corrupt("Native ANN attestation physical schema differs"));
    }
    let mut hash = Sha256::new();
    hash.update(ROW_DOMAIN);
    hash.update(seal.recipe.0);
    hash.update(seal.dimension.to_be_bytes());
    hash.update(seal.rows.to_be_bytes());
    let request = budget.request().clone();
    // `build_region` inserted canonical rows. Do not ask Lance to sort a
    // potentially forged, overlarge table before the bounded scan limit.
    let query = table.query().limit(scan_cap).select(Select::columns(&[
        COLUMN_PRODUCER,
        COLUMN_OWNER,
        COLUMN_MEMBER,
        COLUMN_VECTOR_ID,
        COLUMN_INPUT,
        COLUMN_VECTOR,
    ]));
    let mut batches =
        interruptible(&request, async { query.execute().await.map_err(backend) }).await?;
    let mut count = 0usize;
    let mut prior_key: Option<(OwnerKey, Digest)> = None;
    let mut prior_vector_id = Vec::new();
    prior_vector_id
        .try_reserve_exact(max_id)
        .map_err(|_error| IndexError::ResourceLimited)?;
    while let Some(batch) = interruptible(&request, async {
        batches.try_next().await.map_err(backend)
    })
    .await?
    {
        budget.checkpoint()?;
        if batch.num_rows() > scan_cap.saturating_sub(count) {
            return Err(corrupt("Native ANN physical rows exceed seal"));
        }
        let producer = binary(&batch, COLUMN_PRODUCER, 16)?;
        let owner = binary(&batch, COLUMN_OWNER, 16)?;
        let member = binary(&batch, COLUMN_MEMBER, 32)?;
        let vector_ids = vector_ids(&batch)?;
        let input = binary(&batch, COLUMN_INPUT, 32)?;
        let vectors = batch
            .column_by_name(COLUMN_VECTOR)
            .and_then(|column| column.as_any().downcast_ref::<FixedSizeListArray>())
            .ok_or_else(|| corrupt("Native ANN attestation vector column malformed"))?;
        if vectors.null_count() != 0
            || vectors.value_length()
                != i32::try_from(dimension).map_err(|_error| IndexError::ResourceLimited)?
        {
            return Err(corrupt("Native ANN attestation vector width/null differs"));
        }
        for row in 0..batch.num_rows() {
            budget.checkpoint()?;
            count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            if count > rows {
                return Err(corrupt("Native ANN physical rows exceed seal"));
            }
            let vector_id = vector_ids.value(row);
            if vector_id.is_empty() || vector_id.len() > max_id {
                return Err(corrupt("Native ANN attestation vector ID invalid"));
            }
            let owner_key = OwnerKey {
                producer: ProducerId(id(producer.value(row))),
                stable_id: StableOwnerId(id(owner.value(row))),
            };
            let member_digest = digest(member.value(row));
            if prior_key.is_some_and(|(previous_owner, previous_member)| {
                (previous_owner, previous_member, prior_vector_id.as_slice())
                    >= (owner_key, member_digest, vector_id)
            }) {
                return Err(corrupt(
                    "Native ANN physical rows are not canonical and unique",
                ));
            }
            let vector = vectors.value(row);
            let values = vector
                .as_any()
                .downcast_ref::<Float32Array>()
                .ok_or_else(|| corrupt("Native ANN attestation vector values malformed"))?;
            if values.null_count() != 0 || values.len() != dimension {
                return Err(corrupt("Native ANN attestation vector values differ"));
            }
            let mut norm = 0.0f64;
            for value in values.values() {
                let coordinate = f64::from(*value);
                if !coordinate.is_finite() {
                    return Err(corrupt("Native ANN attestation vector nonfinite"));
                }
                norm = coordinate.mul_add(coordinate, norm);
            }
            if !norm.is_finite() || norm <= 0.0 {
                return Err(corrupt("Native ANN attestation vector zero/overflow"));
            }
            hash.update(owner_key.producer.0);
            hash.update(owner_key.stable_id.0);
            hash.update(member_digest.0);
            hash.update(
                u32::try_from(vector_id.len())
                    .map_err(|_error| IndexError::ResourceLimited)?
                    .to_be_bytes(),
            );
            hash.update(vector_id);
            hash.update(input.value(row));
            for value in values.values() {
                hash.update(value.to_bits().to_be_bytes());
            }
            prior_key = Some((owner_key, member_digest));
            prior_vector_id.clear();
            prior_vector_id.extend_from_slice(vector_id);
        }
    }
    if count != rows || Digest(hash.finalize().into()) != seal.rows_digest {
        return Err(corrupt(
            "Native ANN physical row count/digest differs from seal",
        ));
    }
    budget.checkpoint()
}
