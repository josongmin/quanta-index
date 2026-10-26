//! One controlled CBOR value decoded under a caller-retained backing owner.

use std::fmt;
use std::io::{self, Read};

use ciborium::de::{
    Error as CborDecodeError, FiniteParserAllocationAdmissionV3, FiniteReadErrorV3,
    FiniteSemanticReasonV3, from_slice_with_finite_seed_and_allocation_v3,
};
use serde::de::DeserializeSeed;

use crate::IpcError;
use crate::cbor_preflight::retained_text_budget_reader;

/// A framing refusal, authentic read-work refusal, or finite parser failure.
///
/// Wire bytes, diagnostic metadata and typed-seed visitor allocations outside
/// the shared owner remain the caller's responsibility.
#[derive(Debug)]
pub enum AdmittedCborDecodeErrorV1<E> {
    InputTooLarge { actual: usize, maximum: usize },
    ScratchTooSmall { actual: usize },
    Frame(IpcError),
    Work(E),
    Parser(CborDecodeError<FiniteReadErrorV3<E>, FiniteSemanticReasonV3>),
}

/// The decoded value is dropped before its retained parser and visitor owner.
/// A caller may borrow either field but cannot detach a bare DTO from custody.
pub struct AdmittedDecodedCborV1<T, O> {
    value: T,
    owner: O,
}

/// Preflight can invoke caller work before the parser's own attempt guard
/// exists. Retain the same owner through both phases so an unwind poisons all
/// of its clones without issuing a second begin.
struct AbortIncompleteDecodeV1<'a, O: FiniteParserAllocationAdmissionV3> {
    owner: &'a mut O,
    armed: bool,
}

impl<O: FiniteParserAllocationAdmissionV3> Drop for AbortIncompleteDecodeV1<'_, O> {
    fn drop(&mut self) {
        if self.armed {
            self.owner.abort_decode_v3();
        }
    }
}

impl<T, O> AdmittedDecodedCborV1<T, O> {
    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn owner(&self) -> &O {
        &self.owner
    }
}

struct PreflightWorkReader<'input, 'work, E, F> {
    remaining: &'input [u8],
    work: &'work mut F,
    first_refusal: Option<E>,
}

impl<E, F: FnMut(usize) -> Result<(), E>> Read for PreflightWorkReader<'_, '_, E, F> {
    fn read(&mut self, destination: &mut [u8]) -> io::Result<usize> {
        if self.first_refusal.is_some() {
            return Err(io::Error::from(io::ErrorKind::Other));
        }
        let read_len = self.remaining.len().min(destination.len());
        if let Err(cause) = (self.work)(read_len) {
            self.first_refusal = Some(cause);
            // read_exact retries Interrupted, which would overwrite the first
            // refusal or loop after cancellation. Other is terminal here.
            return Err(io::Error::from(io::ErrorKind::Other));
        }
        let (read, remaining) = self.remaining.split_at(read_len);
        let destination = destination
            .get_mut(..read_len)
            .ok_or_else(|| io::Error::from(io::ErrorKind::Other))?;
        destination.copy_from_slice(read);
        self.remaining = remaining;
        Ok(read_len)
    }
}

/// Preflight exactly one CBOR value, then run the existing finite Ciborium
/// parser with a typed seed and the same retained allocation owner.
///
/// `seed` must admit its visitor backings against this owner's shared grant;
/// `parser_owner` is the matching parser port, commonly a clone of that same
/// owner. The owner is aborted on every error and must not be reused after an
/// ignored failure. `scratch` must also hold every admitted map identifier;
/// the parser rejects identifiers longer than scratch before String birth.
/// This function does not fund the input slice or the seed's external data.
pub fn decode_cbor_seed_admitted_v1<'de, S, E, F, O>(
    bytes: &'de [u8],
    scratch: &mut [u8],
    seed: S,
    work: &mut F,
    max_input_bytes: usize,
    mut parser_owner: O,
) -> Result<AdmittedDecodedCborV1<S::Value, O>, AdmittedCborDecodeErrorV1<E>>
where
    S: DeserializeSeed<'de>,
    E: fmt::Debug,
    F: FnMut(usize) -> Result<(), E>,
    O: FiniteParserAllocationAdmissionV3,
{
    let mut attempt = AbortIncompleteDecodeV1 {
        owner: &mut parser_owner,
        armed: true,
    };
    if attempt.owner.begin_preflight_v3().is_err() {
        return Err(AdmittedCborDecodeErrorV1::Parser(
            CborDecodeError::semantic(None, "controlled CBOR preflight attempt refused"),
        ));
    }
    if bytes.len() > max_input_bytes {
        return Err(AdmittedCborDecodeErrorV1::InputTooLarge {
            actual: bytes.len(),
            maximum: max_input_bytes,
        });
    }
    if scratch.len() < 4 {
        return Err(AdmittedCborDecodeErrorV1::ScratchTooSmall {
            actual: scratch.len(),
        });
    }
    if let Err(cause) = work(0) {
        return Err(AdmittedCborDecodeErrorV1::Work(cause));
    }

    let mut reader = PreflightWorkReader {
        remaining: bytes,
        work,
        first_refusal: None,
    };
    // The legacy storage heuristic is deliberately not used as an allocation
    // grant. The scanner only proves framing, length and depth constraints.
    let preflight = retained_text_budget_reader(&mut reader, bytes.len());
    if let Some(cause) = reader.first_refusal.take() {
        return Err(AdmittedCborDecodeErrorV1::Work(cause));
    }
    if let Err(error) = preflight {
        return Err(AdmittedCborDecodeErrorV1::Frame(error));
    }
    drop(reader);
    if attempt.owner.complete_preflight_v3().is_err() {
        return Err(AdmittedCborDecodeErrorV1::Parser(
            CborDecodeError::semantic(None, "controlled CBOR preflight completion refused"),
        ));
    }

    let value = match from_slice_with_finite_seed_and_allocation_v3(
        bytes,
        scratch,
        seed,
        work,
        &mut *attempt.owner,
    ) {
        Ok(value) => value,
        Err(CborDecodeError::Io(FiniteReadErrorV3::Work(cause))) => {
            return Err(AdmittedCborDecodeErrorV1::Work(cause));
        }
        Err(error) => return Err(AdmittedCborDecodeErrorV1::Parser(error)),
    };
    attempt.armed = false;
    drop(attempt);
    Ok(AdmittedDecodedCborV1 {
        value,
        owner: parser_owner,
    })
}

#[cfg(test)]
mod tests;
