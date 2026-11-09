//! Comparison data for a posting Tree, never an object-loading or custody authority.
//!
//! The selected owner Contribution inventory owns the actual occurrence reference.
//! A query compares that reference with this commitment before loading it. Small
//! postings retain their bounded, explicit object-child representation.

use quanta_index_core::index::{Digest, IndexError, MapValue, ObjectRef};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct OccurrenceDigestWitness(Digest);

impl OccurrenceDigestWitness {
    pub(super) fn from_reference(reference: ObjectRef) -> Self {
        Self(reference.0)
    }

    pub(super) fn matches(self, reference: ObjectRef) -> bool {
        self.0 == reference.0
    }

    pub(super) fn bytes(self) -> [u8; 32] {
        self.0.0
    }

    pub(super) fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(Digest(bytes))
    }

    pub(super) fn into_value(self) -> Result<MapValue, IndexError> {
        MapValue::inline(&self.0.0, [])
    }

    pub(super) fn from_value(value: MapValue) -> Result<Self, IndexError> {
        let MapValue::Inline(value) = value else {
            return Err(IndexError::CorruptObject(
                "posting Tree requires an occurrence digest witness".into(),
            ));
        };
        if !value.children()?.is_empty() {
            return Err(IndexError::CorruptObject(
                "posting digest witness carries physical children".into(),
            ));
        }
        let bytes = value.data()?.try_into().map_err(|_width_error| {
            IndexError::CorruptObject("posting digest witness must contain 32 bytes".into())
        })?;
        Ok(Self(Digest(bytes)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(value: bool) -> Result<(), IndexError> {
        if value {
            Ok(())
        } else {
            Err(IndexError::BackendViolation(
                "posting witness independent wire differs".into(),
            ))
        }
    }

    #[test]
    fn exact_childless_commitment_preserves_reference_equality() -> Result<(), IndexError> {
        let reference = ObjectRef(Digest([0x93; 32]));
        let witness = OccurrenceDigestWitness::from_reference(reference);
        let value = witness.into_value()?;
        let mut expected = vec![1, 32];
        expected.extend_from_slice(&[0x93; 32]);
        expected.push(0);
        check(value.encoded()?.as_slice()? == expected)?;
        check(value.children()?.is_empty())?;
        check(OccurrenceDigestWitness::from_value(value)? == witness)?;
        check(witness.matches(reference))?;
        check(!witness.matches(ObjectRef(Digest([0x94; 32]))))
    }

    #[test]
    fn reference_values_wrong_width_and_children_are_not_witnesses() -> Result<(), IndexError> {
        let reference = ObjectRef(Digest([0x93; 32]));
        for value in [
            MapValue::Object(reference),
            MapValue::inline(&[], [])?,
            MapValue::inline(&[0x93; 31], [])?,
            MapValue::inline(&[0x93; 32], [reference])?,
        ] {
            check(matches!(
                OccurrenceDigestWitness::from_value(value),
                Err(IndexError::CorruptObject(_))
            ))?;
        }
        Ok(())
    }
}
