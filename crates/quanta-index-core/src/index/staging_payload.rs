//! Admit bounded staging JSON before allocating its backing.

use std::io::{self, Write};

use super::{IndexError, MemoryLease, MemoryLifetime, ObjectBatchInput, ObjectRef, WorkBudget};
use serde::Serialize;

pub struct StagedPayload {
    pub bytes: Vec<u8>,
    // Serialized data dies before its reservation on every error path.
    pub memory: Option<MemoryLease>,
}

fn reserve_backing(bytes: usize, budget: &WorkBudget) -> Result<Option<MemoryLease>, IndexError> {
    if bytes == 0 {
        budget.checkpoint()?;
        Ok(None)
    } else {
        budget.reserve_memory(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )
    }
}

impl StagedPayload {
    pub fn copy(bytes: &[u8], budget: &WorkBudget) -> Result<Self, IndexError> {
        let memory = reserve_backing(bytes.len(), budget)?;
        let mut backing = Vec::new();
        backing
            .try_reserve_exact(bytes.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        backing.extend_from_slice(bytes);
        Ok(Self {
            bytes: backing,
            memory,
        })
    }
}

const MAX_BATCH_OBJECTS: usize = 64;

struct InputMemory {
    _payload: Option<MemoryLease>,
    _children: Option<MemoryLease>,
}

/// Owns the existing immutable-store input IR together with its preparation
/// capacity. It does not install objects or acquire publication authority.
pub struct StagedObjectBatch {
    inputs: Vec<ObjectBatchInput>,
    input_memory: Vec<InputMemory>,
    limit: usize,
    // Input backing and its per-row guards die before the array reservation.
    _memory: Option<MemoryLease>,
}

impl StagedObjectBatch {
    pub fn new(limit: usize, budget: &WorkBudget) -> Result<Self, IndexError> {
        if limit > MAX_BATCH_OBJECTS {
            return Err(IndexError::ResourceLimited);
        }
        let bytes = limit
            .checked_mul(
                std::mem::size_of::<ObjectBatchInput>()
                    .checked_add(std::mem::size_of::<InputMemory>())
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)?;
        let memory = reserve_backing(bytes, budget)?;
        let mut inputs = Vec::new();
        inputs
            .try_reserve_exact(limit)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut input_memory = Vec::new();
        input_memory
            .try_reserve_exact(limit)
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(Self {
            inputs,
            input_memory,
            limit,
            _memory: memory,
        })
    }

    pub fn push(
        &mut self,
        payload: StagedPayload,
        children: &[ObjectRef],
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        if self.inputs.len() == self.limit {
            return Err(IndexError::ResourceLimited);
        }
        let bytes = children
            .len()
            .checked_mul(std::mem::size_of::<ObjectRef>())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = reserve_backing(bytes, budget)?;
        let mut copied = Vec::new();
        copied
            .try_reserve_exact(children.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        copied.extend_from_slice(children);
        budget.checkpoint()?;
        self.inputs.push(ObjectBatchInput {
            payload: payload.bytes,
            children: copied,
        });
        self.input_memory.push(InputMemory {
            _payload: payload.memory,
            _children: memory,
        });
        Ok(())
    }

    #[must_use]
    pub fn inputs(&self) -> &[ObjectBatchInput] {
        &self.inputs
    }
}

struct JsonWriter<'a> {
    bytes: Option<&'a mut Vec<u8>>,
    len: usize,
    limit: usize,
    budget: &'a WorkBudget,
    error: Option<IndexError>,
}

impl Write for JsonWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let next = self.budget.checkpoint().and_then(|()| {
            self.len
                .checked_add(bytes.len())
                .filter(|len| *len <= self.limit)
                .ok_or(IndexError::ResourceLimited)
        });
        match next {
            Ok(next) => {
                if let Some(data) = &mut self.bytes {
                    data.extend_from_slice(bytes);
                }
                self.len = next;
                Ok(bytes.len())
            }
            Err(error) => {
                self.error = Some(error);
                Err(io::Error::other("staging JSON admission refused"))
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn encode_staged_json<T: Serialize>(
    value: &T,
    limit: usize,
    budget: &WorkBudget,
) -> Result<StagedPayload, IndexError> {
    let mut census = JsonWriter {
        bytes: None,
        len: 0,
        limit,
        budget,
        error: None,
    };
    if let Err(error) = serde_json::to_writer(&mut census, value) {
        return Err(census.error.take().unwrap_or_else(|| {
            IndexError::InvalidInput(format!("staging JSON encoding: {error}"))
        }));
    }
    let len = census.len;
    drop(census);
    let memory = reserve_backing(len, budget)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(len)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut writer = JsonWriter {
        bytes: Some(&mut bytes),
        len: 0,
        limit: len,
        budget,
        error: None,
    };
    if let Err(error) = serde_json::to_writer(&mut writer, value) {
        return Err(writer.error.take().unwrap_or_else(|| {
            IndexError::InvalidInput(format!("staging JSON encoding: {error}"))
        }));
    }
    if writer.len != len {
        return Err(IndexError::BackendViolation(
            "staging JSON census changed".into(),
        ));
    }
    drop(writer);
    Ok(StagedPayload { bytes, memory })
}
