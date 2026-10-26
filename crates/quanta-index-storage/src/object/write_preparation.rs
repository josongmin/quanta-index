//! Canonical object identity and admitted temporary encoding backing.

use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, ObjectRef, RegisteredObject, WorkBudget, WorkUsage,
};
use sha2::{Digest as _, Sha256};

use super::{HASH_DOMAIN, HEADER_LEN, MAGIC, MAX_CHILDREN, MAX_PAYLOAD, digest_wire};

#[cfg(test)]
mod tests;

pub(crate) struct PreparedObject {
    pub(crate) registration: RegisteredObject,
    pub(crate) wire: Vec<u8>,
    // Wire storage dies before its temporary allocation grant on every exit.
    _wire_memory: Option<MemoryLease>,
}

pub(crate) struct AdmittedWire {
    pub(crate) bytes: Vec<u8>,
    memory: Option<MemoryLease>,
}

impl AdmittedWire {
    pub(crate) fn new(length: usize, budget: &WorkBudget) -> Result<Self, IndexError> {
        let bytes = u64::try_from(length).map_err(|_error| IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
        let mut backing = Vec::new();
        backing
            .try_reserve_exact(length)
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(Self {
            bytes: backing,
            memory,
        })
    }
}

impl PreparedObject {
    pub(crate) fn from_wire(registration: RegisteredObject, wire: AdmittedWire) -> Self {
        Self {
            registration,
            wire: wire.bytes,
            _wire_memory: wire.memory,
        }
    }
}

fn header(payload: &[u8], children: &[ObjectRef]) -> Result<([u8; HEADER_LEN], usize), IndexError> {
    if payload.len() > MAX_PAYLOAD || children.len() > MAX_CHILDREN {
        return Err(IndexError::InvalidInput(
            "object exceeds physical size limit".into(),
        ));
    }
    let wire_len = HEADER_LEN
        .checked_add(
            children
                .len()
                .checked_mul(32)
                .ok_or(IndexError::ResourceLimited)?,
        )
        .and_then(|length| length.checked_add(payload.len()))
        .ok_or(IndexError::ResourceLimited)?;
    let mut header = [0; HEADER_LEN];
    header[..8].copy_from_slice(MAGIC);
    header[8..12].copy_from_slice(
        &u32::try_from(payload.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .to_be_bytes(),
    );
    header[12..].copy_from_slice(
        &u32::try_from(children.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .to_be_bytes(),
    );
    Ok((header, wire_len))
}

fn charge(wire_len: usize, budget: &mut WorkBudget) -> Result<u64, IndexError> {
    let bytes = u64::try_from(wire_len).map_err(|_error| IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes,
        ..WorkUsage::default()
    })?;
    Ok(bytes)
}

fn registration(
    reference: ObjectRef,
    bytes: u64,
    children: &[ObjectRef],
    budget: &WorkBudget,
) -> Result<RegisteredObject, IndexError> {
    RegisteredObject::copy_children(reference, bytes, children, budget)
}

pub(super) fn identify(
    payload: &[u8],
    children: &[ObjectRef],
    budget: &mut WorkBudget,
) -> Result<RegisteredObject, IndexError> {
    let (header, wire_len) = header(payload, children)?;
    let bytes = charge(wire_len, budget)?;
    let mut digest = Sha256::new();
    digest.update(HASH_DOMAIN);
    digest.update(header);
    for child in children {
        digest.update((child.0).0);
    }
    digest.update(payload);
    budget.checkpoint()?;
    registration(
        ObjectRef(quanta_index_core::index::Digest(digest.finalize().into())),
        bytes,
        children,
        budget,
    )
}

pub(super) fn prepare(
    payload: &[u8],
    children: &[ObjectRef],
    budget: &mut WorkBudget,
) -> Result<PreparedObject, IndexError> {
    let (header, wire_len) = header(payload, children)?;
    let bytes = charge(wire_len, budget)?;
    let mut wire = AdmittedWire::new(wire_len, budget)?;
    wire.bytes.extend_from_slice(&header);
    for child in children {
        wire.bytes.extend_from_slice(&(child.0).0);
    }
    wire.bytes.extend_from_slice(payload);
    let registration = registration(digest_wire(&wire.bytes), bytes, children, budget)?;
    budget.checkpoint()?;
    Ok(PreparedObject::from_wire(registration, wire))
}
