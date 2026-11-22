//! Bind the exact edit stream to the stamp shared by planning and replay.

use crate::native::text_inventory_descriptor::decode;
use quanta_index_core::index::{
    Digest, IndexError, MapValue, ObjectRef, OrdinalMapEdit, WorkBudget, WorkUsage,
};
use sha2::{Digest as _, Sha256};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Stamp {
    pub(super) count: u64,
    pub(super) digest: Digest,
}
pub(super) struct Edits {
    hash: Sha256,
    count: u64,
}
impl Edits {
    pub(super) fn new() -> Self {
        let mut hash = Sha256::new();
        hash.update(b"qidx.native.text.entry-edits.v1\0");
        Self { hash, count: 0 }
    }
    pub(super) fn push(
        &mut self,
        edit: &OrdinalMapEdit,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let _references =
            self.push_object_wires(edit.key(), [edit.expected_wire(), edit.next_wire()], budget)?;
        Ok(())
    }

    pub(super) fn push_object_wires(
        &mut self,
        key: &[u8],
        wires: [Option<&[u8]>; 2],
        budget: &mut WorkBudget,
    ) -> Result<[Option<ObjectRef>; 2], IndexError> {
        let len = u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: len.checked_add(74).ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        self.hash.update(len.to_be_bytes());
        self.hash.update(key);
        let object = |wire: Option<&[u8]>| {
            wire.map(|wire| MapValue::decode(wire)?.object_ref())
                .transpose()
        };
        let [expected, next] = wires;
        let references = [object(expected)?, object(next)?];
        for value in references {
            self.hash.update([u8::from(value.is_some())]);
            if let Some(value) = value {
                self.hash.update(value.0.0);
            }
        }
        self.count = self
            .count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(references)
    }
    /// The complete descriptor wire is DATA in the original scratch stream.
    /// Binding only its child would miss kind/count substitutions on replay.
    pub(super) fn push_descriptor(
        &mut self,
        edit: &OrdinalMapEdit,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.push_descriptor_wire(edit.key(), edit.expected_wire(), edit.next_wire(), budget)
    }

    pub(super) fn push_descriptor_wire(
        &mut self,
        key: &[u8],
        expected: Option<&[u8]>,
        next: Option<&[u8]>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if expected.is_some() {
            return Err(IndexError::CorruptObject(
                "directory descriptor CAS differs".into(),
            ));
        }
        let wire = next.ok_or_else(|| {
            IndexError::CorruptObject("directory descriptor next is absent".into())
        })?;
        // The cursor already owns the canonical value wire. Check the
        // descriptor before sealing those exact bytes.
        let _descriptor = decode(MapValue::decode(wire)?)?;
        let len = u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?;
        let wire_len = u64::try_from(wire.len()).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: len
                .checked_add(74)
                .and_then(|bytes| bytes.checked_add(wire_len))
                .and_then(|bytes| bytes.checked_add(8))
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        self.hash.update(b"directory-descriptor\0");
        self.hash.update(len.to_be_bytes());
        self.hash.update(key);
        self.hash.update([0, 1]);
        self.hash.update(wire_len.to_be_bytes());
        self.hash.update(wire);
        self.count = self
            .count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(())
    }
    pub(super) fn finish(&self) -> Stamp {
        Stamp {
            count: self.count,
            digest: Digest(self.hash.clone().finalize().into()),
        }
    }
}
