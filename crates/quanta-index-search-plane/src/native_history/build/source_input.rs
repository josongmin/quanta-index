//! Decode source carriers under the original History attempt's budget.

use quanta_index_contract::{
    HistoryIngestBatch,
    index::{HistorySourceInputV1, MAX_HISTORY_SOURCE_INPUT_BYTES_V1},
};
use quanta_index_core::{
    CoreError,
    index::{AttemptContext, IndexError, ObjectRef, StoredObject, WorkBudget, WorkUsage},
};

use super::batch_budget;
use crate::native_history::{SourceInputData, history_work_error, invalid, row_budget};

pub(super) fn is_compact_replay(payload: &[u8]) -> bool {
    // A CBOR map is major type 5. JSON starts with ASCII whitespace, a
    // struct object, or a struct sequence. The decoder checks canonical bytes.
    payload.first().is_some_and(|byte| byte >> 5 == 5)
}

pub(in crate::native_history) fn load_history_object<T: serde::de::DeserializeOwned>(
    reference: ObjectRef,
    context: &mut AttemptContext<'_>,
) -> Result<SourceInputData<T>, CoreError> {
    let stored = context
        .objects
        .get(reference, context.budget)
        .map_err(|error| history_work_error(context.budget, error))?;
    decode_history_object(reference, &stored, context.budget)
}

pub(in crate::native_history) fn load_history_batch_object(
    reference: ObjectRef,
    context: &mut AttemptContext<'_>,
) -> Result<SourceInputData<HistoryIngestBatch>, CoreError> {
    let stored = context
        .objects
        .get(reference, context.budget)
        .map_err(|error| history_work_error(context.budget, error))?;
    decode_history_batch_object(reference, &stored, context.budget)
}

pub(in crate::native_history) fn load_history_contributor_object(
    reference: ObjectRef,
    context: &mut AttemptContext<'_>,
) -> Result<SourceInputData<quanta_index_contract::FileContributorIngestBatch>, CoreError> {
    let stored = context
        .objects
        .get(reference, context.budget)
        .map_err(|error| history_work_error(context.budget, error))?;
    decode_history_contributor_object(reference, &stored, context.budget)
}

pub(super) fn decode_history_contributor_object(
    reference: ObjectRef,
    stored: &StoredObject,
    budget: &mut WorkBudget,
) -> Result<SourceInputData<quanta_index_contract::FileContributorIngestBatch>, CoreError> {
    validate_source_object(reference, stored, false)?;
    let backing = batch_budget::contributor_batch_backing(&stored.payload, budget)?;
    decode_history_object_inner(reference, stored, budget, false, Some(backing))
}

pub(super) fn decode_history_batch_object(
    reference: ObjectRef,
    stored: &StoredObject,
    budget: &mut WorkBudget,
) -> Result<SourceInputData<HistoryIngestBatch>, CoreError> {
    validate_source_object(reference, stored, false)?;
    let backing = batch_budget::history_batch_backing(&stored.payload, budget)?;
    decode_history_object_inner(reference, stored, budget, false, Some(backing))
}

pub(super) fn decode_history_source_object(
    reference: ObjectRef,
    stored: &StoredObject,
    budget: &mut WorkBudget,
) -> Result<SourceInputData<HistorySourceInputV1>, CoreError> {
    validate_source_object(reference, stored, false)?;
    if is_compact_replay(&stored.payload) {
        return batch_budget::decode_cbor_replay(&stored.payload, budget);
    }
    let backing = batch_budget::history_source_backing(&stored.payload, budget)?;
    decode_history_object_inner(reference, stored, budget, false, Some(backing))
}

fn decode_history_object<T: serde::de::DeserializeOwned>(
    reference: ObjectRef,
    stored: &StoredObject,
    budget: &mut WorkBudget,
) -> Result<SourceInputData<T>, CoreError> {
    decode_history_object_inner(reference, stored, budget, false, None)
}

pub(in crate::native_history) fn decode_history_object_allow_children<
    T: serde::de::DeserializeOwned,
>(
    reference: ObjectRef,
    stored: &StoredObject,
    budget: &mut WorkBudget,
) -> Result<SourceInputData<T>, CoreError> {
    decode_history_object_inner(reference, stored, budget, true, None)
}

fn validate_source_object(
    reference: ObjectRef,
    stored: &StoredObject,
    allow_children: bool,
) -> Result<(), CoreError> {
    if stored.registration.reference != reference
        || (!allow_children && !stored.registration.children.is_empty())
        || stored.payload.len() > MAX_HISTORY_SOURCE_INPUT_BYTES_V1
    {
        return Err(invalid(
            "History source payload identity, child set or encoded length is invalid",
        ));
    }
    Ok(())
}

fn decode_history_object_inner<T: serde::de::DeserializeOwned>(
    reference: ObjectRef,
    stored: &StoredObject,
    budget: &mut WorkBudget,
    allow_children: bool,
    backing: Option<row_budget::RowBacking>,
) -> Result<SourceInputData<T>, CoreError> {
    validate_source_object(reference, stored, allow_children)?;
    let conservative = u64::try_from(stored.payload.len())
        .ok()
        .and_then(|bytes| bytes.checked_mul(16))
        .and_then(|bytes| bytes.checked_add(4096))
        .ok_or_else(|| invalid("History source decode bound overflow"))?;
    let backing = backing.unwrap_or(row_budget::RowBacking::new(conservative, conservative)?);
    let (memory, decode_scratch) = backing.reserve_decode(budget)?;
    // Keep decode work on the original attempt and observe refusal between
    // bounded reads. The backing buffer is covered by the scratch grant above.
    struct SourceReader<'a> {
        bytes: &'a [u8],
        budget: &'a mut WorkBudget,
        refusal: Option<IndexError>,
    }
    impl std::io::Read for SourceReader<'_> {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            let count = output.len().min(self.bytes.len());
            if let Err(error) = self.budget.charge(WorkUsage {
                bytes: count as u64,
                ..WorkUsage::default()
            }) {
                self.refusal = Some(error);
                return Err(std::io::Error::other("History source decode work refused"));
            }
            output[..count].copy_from_slice(&self.bytes[..count]);
            self.bytes = &self.bytes[count..];
            Ok(count)
        }
    }
    let mut reader = std::io::BufReader::with_capacity(
        row_budget::SOURCE_JSON_IO_BUFFER_BYTES,
        SourceReader {
            bytes: &stored.payload,
            budget,
            refusal: None,
        },
    );
    let decoded = serde_json::from_reader(&mut reader);
    let refusal = reader.get_mut().refusal.take();
    drop(reader);
    if let Some(error) = refusal {
        return Err(history_work_error(budget, error));
    }
    let value = decoded.map_err(|error| invalid(&error.to_string()))?;
    budget
        .checkpoint()
        .map_err(|error| history_work_error(budget, error))?;
    drop(decode_scratch);
    Ok(SourceInputData {
        value,
        _memory: memory,
    })
}
