//! Raw DTO-wire output admitted before allocation, without transport framing.
//!
//! The caller retains the returned lease alongside the bytes. This funds the
//! declared output capacity, not the input DTO or serializer-private backing.

use sha2::{Digest, Sha256};
use std::io::{self, Write};

use crate::{IndexError, IpcError, MemoryLease, MemoryLifetime, MemoryScope};

#[derive(Debug)]
pub enum CborPayloadAdmissionErrorV1<E> {
    Codec(IpcError),
    Admission(IndexError),
    Progress(E),
}

struct PayloadSink<'a, E, F> {
    output: Option<&'a mut Vec<u8>>,
    position: usize,
    maximum: usize,
    progress: &'a mut F,
    interrupted: Option<E>,
    digest: Sha256,
}

impl<E, F: FnMut() -> Result<(), E>> PayloadSink<'_, E, F> {
    fn checkpoint(&mut self) -> io::Result<()> {
        if self.interrupted.is_some() {
            return Err(io::Error::other("CBOR payload execution interrupted"));
        }
        if let Err(error) = (self.progress)() {
            self.interrupted = Some(error);
            // Write::write_all retries Interrupted; this refusal is permanent.
            return Err(io::Error::other("CBOR payload execution interrupted"));
        }
        Ok(())
    }
}

impl<E, F: FnMut() -> Result<(), E>> Write for PayloadSink<'_, E, F> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        const WINDOW: usize = 64 * 1024;
        for part in bytes.chunks(WINDOW) {
            self.checkpoint()?;
            let next = self
                .position
                .checked_add(part.len())
                .filter(|next| *next <= self.maximum)
                .ok_or_else(|| io::Error::other("CBOR payload exceeds admitted capacity"))?;
            if let Some(output) = self.output.as_mut() {
                if next > output.capacity() {
                    return Err(io::Error::other("CBOR payload lost admitted backing"));
                }
                output.extend_from_slice(part);
            }
            self.digest.update(part);
            self.position = next;
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.checkpoint()
    }
}

/// Count exact raw CBOR, reserve its capacity, then encode without growth.
///
/// Both passes use the ordinary IPC Ciborium serialization, preserving its DTO
/// wire order. The output cap is supplied by the artifact owner, independently
/// of frame caps. A changed serializer, capacity drift or observer refusal
/// cannot return a payload. Checkpoints cover at most 64KiB per sink write;
/// custom serializers must cooperate before they emit bytes.
pub fn encode_cbor_payload_admitted_v1<T, E, F>(
    value: &T,
    memory: &MemoryScope,
    max_payload_bytes: u64,
    progress: &mut F,
) -> Result<(Vec<u8>, MemoryLease), CborPayloadAdmissionErrorV1<E>>
where
    T: serde::Serialize,
    F: FnMut() -> Result<(), E>,
{
    let mut output = Vec::new();
    let mut lease = None;
    let result = encode_cbor_payload_into_slots_v1(
        value,
        memory,
        max_payload_bytes,
        progress,
        &mut output,
        &mut lease,
    );
    if let Err(cause) = result {
        // The ordinary facade owns no exterior DATA. Retire partial bytes
        // before refunding the reservation on every failure.
        drop(output);
        drop(lease);
        return Err(cause);
    }
    let Some(lease) = lease else {
        drop(output);
        return Err(CborPayloadAdmissionErrorV1::Codec(IpcError::Encode(
            "successful CBOR payload lost its admitted lease".into(),
        )));
    };
    Ok((output, lease))
}

/// Encode into caller-owned exterior DATA.
///
/// Once the retained reservation is
/// acquired, all partial bytes and its lease remain in the supplied slots on
/// success and failure. The caller must keep both slots alive through its
/// highest Source finisher, then drop the bytes before the lease.
///
/// Both slots must be fresh. This prevents replacing a previous partial or
/// releasing its charge while an enclosing Original still owns the attempt.
pub fn encode_cbor_payload_into_slots_v1<T, E, F>(
    value: &T,
    memory: &MemoryScope,
    max_payload_bytes: u64,
    progress: &mut F,
    output: &mut Vec<u8>,
    lease: &mut Option<MemoryLease>,
) -> Result<(), CborPayloadAdmissionErrorV1<E>>
where
    T: serde::Serialize,
    F: FnMut() -> Result<(), E>,
{
    if !output.is_empty() || output.capacity() != 0 || lease.is_some() {
        return Err(CborPayloadAdmissionErrorV1::Codec(IpcError::Encode(
            "CBOR payload output slots are not fresh".into(),
        )));
    }
    progress().map_err(CborPayloadAdmissionErrorV1::Progress)?;
    let maximum = usize::try_from(max_payload_bytes).map_err(|_width_error| {
        CborPayloadAdmissionErrorV1::Codec(IpcError::Oversized(max_payload_bytes))
    })?;
    let (counted, counted_digest) = {
        let mut sink = PayloadSink {
            output: None,
            position: 0,
            maximum,
            progress,
            interrupted: None,
            digest: Sha256::new(),
        };
        let encoded = ciborium::into_writer(value, &mut sink);
        if let Some(cause) = sink.interrupted {
            return Err(CborPayloadAdmissionErrorV1::Progress(cause));
        }
        encoded.map_err(|error| {
            CborPayloadAdmissionErrorV1::Codec(IpcError::Encode(error.to_string()))
        })?;
        (sink.position, sink.digest.finalize())
    };
    progress().map_err(CborPayloadAdmissionErrorV1::Progress)?;
    let capacity = u64::try_from(counted).map_err(|_width_error| {
        CborPayloadAdmissionErrorV1::Admission(IndexError::ResourceLimited)
    })?;
    *lease = Some(
        memory
            .reserve(capacity, MemoryLifetime::Retained)
            .map_err(CborPayloadAdmissionErrorV1::Admission)?,
    );
    // The caller owns output and lease in this order. The reservation stays
    // live even if capacity allocation or the second encode fails.
    output
        .try_reserve_exact(counted)
        .map_err(|_allocation_error| {
            CborPayloadAdmissionErrorV1::Admission(IndexError::ResourceLimited)
        })?;
    if output.capacity() != counted {
        return Err(CborPayloadAdmissionErrorV1::Codec(IpcError::Encode(
            "CBOR payload capacity differs from prepaid demand".into(),
        )));
    }
    {
        let mut sink = PayloadSink {
            output: Some(output),
            position: 0,
            maximum: counted,
            progress,
            interrupted: None,
            digest: Sha256::new(),
        };
        let encoded = ciborium::into_writer(value, &mut sink);
        if let Some(cause) = sink.interrupted {
            return Err(CborPayloadAdmissionErrorV1::Progress(cause));
        }
        encoded.map_err(|error| {
            CborPayloadAdmissionErrorV1::Codec(IpcError::Encode(error.to_string()))
        })?;
        if sink.position != counted || sink.digest.finalize() != counted_digest {
            return Err(CborPayloadAdmissionErrorV1::Codec(IpcError::Encode(
                "CBOR payload serialization differs from counted capacity".into(),
            )));
        }
    }
    progress().map_err(CborPayloadAdmissionErrorV1::Progress)?;
    Ok(())
}

#[cfg(test)]
mod tests;
