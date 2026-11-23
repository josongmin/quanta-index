//! Compact encoding of the exact retained replay, with no extra child objects.

use std::marker::PhantomData;

use quanta_index_cbor::{
    Error as CborError, FiniteParserAllocationAdmissionV3, FiniteParserAllocationRefusalV3,
    FiniteReadErrorV3, from_slice_with_finite_seed_and_allocation_v3,
};
use quanta_index_contract::{DecodeAttemptStateV1, index::HistorySourceInputV1};
use quanta_index_core::{
    CoreError,
    index::{IndexError, MemoryLifetime, WorkBudget, WorkUsage},
};
use quanta_index_ipc::{CborPayloadMatchProgressError, cbor_payload_matches_with_work_v1};
use serde::de::{DeserializeOwned, DeserializeSeed, Deserializer};

use super::{SizeOfShape, SourceShape, decode_backing, history_work_error, invalid};
use crate::native_history::SourceInputData;

struct ValueSeed<T>(PhantomData<T>);

impl<'de, T: DeserializeOwned> DeserializeSeed<'de> for ValueSeed<T> {
    type Value = T;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<T, D::Error> {
        T::deserialize(decoder)
    }
}

/// Both passes reserve their backing before entering the parser. The shape pass
/// lends one scalar at a time. The typed pass additionally pays for the retained
/// carrier and its typed conversions. Exact reservation prevents geometric
/// parser growth from consuming either grant.
struct ParserBacking {
    attempt: DecodeAttemptStateV1,
    encoded_bytes: usize,
    scalar_bytes: usize,
    allocation_failed: bool,
    borrowed_text: Vec<u8>,
}

type ParserRefusal = FiniteParserAllocationRefusalV3;

impl ParserBacking {
    fn admitted_length(&self, declared: Option<usize>) -> Result<usize, ParserRefusal> {
        declared
            .filter(|&bytes| bytes <= self.encoded_bytes)
            .ok_or(FiniteParserAllocationRefusalV3)
    }

    fn reserve_scalar(
        &mut self,
        len: usize,
        additional: usize,
        reserve: impl FnOnce(usize) -> bool,
    ) -> Result<(), ParserRefusal> {
        if len.saturating_add(additional) > self.scalar_bytes {
            return Err(FiniteParserAllocationRefusalV3);
        }
        if !reserve(self.scalar_bytes - len) {
            self.allocation_failed = true;
            return Err(FiniteParserAllocationRefusalV3);
        }
        Ok(())
    }
}

impl FiniteParserAllocationAdmissionV3 for ParserBacking {
    fn begin_preflight_v3(&mut self) -> Result<(), ParserRefusal> {
        self.attempt
            .begin_preflight_v1()
            .map_err(|_attempt_refusal| FiniteParserAllocationRefusalV3)
    }

    fn complete_preflight_v3(&mut self) -> Result<(), ParserRefusal> {
        self.attempt
            .complete_preflight_v1()
            .map_err(|_attempt_refusal| FiniteParserAllocationRefusalV3)
    }

    fn begin_decode_v3(&mut self) -> Result<(), ParserRefusal> {
        self.attempt
            .begin_decode_v1()
            .map_err(|_attempt_refusal| FiniteParserAllocationRefusalV3)
    }

    fn complete_decode_v3(&mut self) -> Result<(), ParserRefusal> {
        self.attempt
            .complete_decode_v1()
            .map_err(|_attempt_refusal| FiniteParserAllocationRefusalV3)
    }

    fn abort_decode_v3(&mut self) {
        self.attempt.abort_v1();
    }

    fn check_text_length_v3(&mut self, length: Option<usize>) -> Result<(), ParserRefusal> {
        self.scalar_bytes = self.admitted_length(length)?;
        Ok(())
    }

    fn check_bytes_length_v3(&mut self, length: Option<usize>) -> Result<(), ParserRefusal> {
        self.scalar_bytes = self.admitted_length(length)?;
        Ok(())
    }

    fn check_sequence_length_v3(&mut self, length: Option<usize>) -> Result<(), ParserRefusal> {
        self.admitted_length(length).map(|_| ())
    }

    fn reserve_text_v3(
        &mut self,
        target: &mut String,
        additional: usize,
    ) -> Result<(), ParserRefusal> {
        self.reserve_scalar(target.len(), additional, |remaining| {
            target.try_reserve_exact(remaining).is_ok()
        })
    }

    fn reserve_bytes_v3(
        &mut self,
        target: &mut Vec<u8>,
        additional: usize,
    ) -> Result<(), ParserRefusal> {
        self.reserve_scalar(target.len(), additional, |remaining| {
            target.try_reserve_exact(remaining).is_ok()
        })
    }

    fn with_borrowed_text_scratch_v3(
        &mut self,
        length: usize,
        visit: &mut dyn FnMut(&mut [u8]),
    ) -> Result<(), ParserRefusal> {
        if length != self.scalar_bytes {
            return Err(FiniteParserAllocationRefusalV3);
        }
        self.borrowed_text.clear();
        if self.borrowed_text.try_reserve_exact(length).is_err() {
            self.allocation_failed = true;
            return Err(FiniteParserAllocationRefusalV3);
        }
        self.borrowed_text.resize(length, 0);
        visit(&mut self.borrowed_text);
        Ok(())
    }
}

fn charge(budget: &mut WorkBudget, bytes: u64) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        bytes,
        ..WorkUsage::default()
    })
}

fn decode<T: DeserializeOwned>(bytes: &[u8], budget: &mut WorkBudget) -> Result<T, CoreError> {
    let mut backing = ParserBacking {
        attempt: DecodeAttemptStateV1::default(),
        encoded_bytes: bytes.len(),
        scalar_bytes: 0,
        allocation_failed: false,
        borrowed_text: Vec::new(),
    };
    let mut consumed = 0;
    let mut work = |length: usize| {
        charge(budget, length as u64)?;
        consumed += length;
        Ok(())
    };
    let value = from_slice_with_finite_seed_and_allocation_v3(
        bytes,
        &mut [0; 4096],
        ValueSeed(PhantomData),
        &mut work,
        &mut backing,
    )
    .map_err(|error| match error {
        CborError::Io(FiniteReadErrorV3::Work(cause)) => history_work_error(budget, cause),
        _ if backing.allocation_failed => history_work_error(budget, IndexError::ResourceLimited),
        _ => invalid(&format!("History compact replay decode failed: {error}")),
    })?;
    if consumed != bytes.len() {
        return Err(invalid("History compact replay contains trailing bytes"));
    }
    Ok(value)
}

pub(in crate::native_history::build) fn decode_cbor_replay(
    bytes: &[u8],
    budget: &mut WorkBudget,
) -> Result<SourceInputData<HistorySourceInputV1>, CoreError> {
    let scalar_scratch = bytes.len() as u64;
    let shape_scratch = budget
        .reserve_memory(scalar_scratch, MemoryLifetime::Temporary)
        .map_err(|cause| history_work_error(budget, cause))?;
    let shape: SourceShape = decode(bytes, budget)?;
    let size = shape
        .size()
        .ok_or_else(|| invalid("History compact replay shape overflow"))?;
    drop(shape_scratch);
    let (memory, decode_scratch) = decode_backing(size, scalar_scratch)?.reserve_decode(budget)?;
    let value: HistorySourceInputV1 = decode(bytes, budget)?;
    drop(decode_scratch);
    let canonical =
        cbor_payload_matches_with_work_v1(&value, bytes, &mut |count| charge(budget, count))
            .map_err(|error| match error {
                CborPayloadMatchProgressError::Progress(cause) => history_work_error(budget, cause),
                CborPayloadMatchProgressError::Encode(cause) => invalid(&cause.to_string()),
            })?;
    if !canonical {
        return Err(invalid("History compact replay bytes are not canonical"));
    }
    Ok(SourceInputData {
        value,
        _memory: memory,
    })
}
