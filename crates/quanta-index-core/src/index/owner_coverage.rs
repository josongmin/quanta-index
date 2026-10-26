//! One owner-coverage IR with bounded pending source data in the existing map.
use super::{
    Digest, IndexError, MapValue, ObjectChildren, OwnerCoverage, OwnerInputBinding, WorkBudget,
    owner_from_map_key, ports::ImmutableObjectPort,
};

/// Source-only pending coverage has one real source child and a policy digest.
/// Covered, searchable, projected or contextual records keep their object form.
pub fn pending_owner_coverage_value(value: &OwnerCoverage) -> Result<Option<MapValue>, IndexError> {
    if value.covered.is_some()
        || value.searchable.is_some()
        || value.desired.projection.is_some()
        || value.desired.producer_context != Digest([0; 32])
    {
        return Ok(None);
    }
    let Some(source) = value.desired.source else {
        return Ok(None);
    };
    MapValue::inline(&value.desired.derivation_policy.0, Some(source)).map(Some)
}

/// Authenticate the stored representation and canonical owner key together.
/// Inline fields are fixed data; their sole source remains a real physical edge.
pub fn read_owner_coverage(
    value: MapValue,
    key: &[u8],
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<OwnerCoverage, IndexError> {
    let owner = owner_from_map_key(key)?;
    budget.checkpoint()?;
    match value {
        MapValue::Object(reference) => {
            let stored = objects.get(reference, budget)?;
            let binding = super::decode_owner_coverage(&stored.payload, budget)?;
            let children = ObjectChildren::from_references([
                binding.desired.source,
                binding.desired.projection,
                binding.searchable,
                None,
            ]);
            if stored.registration.reference != reference
                || binding.owner != owner
                || stored.registration.children.as_slice() != children.as_slice()
            {
                return Err(IndexError::CorruptObject(
                    "owner coverage identity/edges mismatch".into(),
                ));
            }
            Ok(binding)
        }
        MapValue::Inline(value) => {
            let policy = Digest(value.data()?.try_into().map_err(|_error| {
                IndexError::CorruptObject("pending owner policy width differs".into())
            })?);
            let source = value.single_child()?.ok_or_else(|| {
                IndexError::CorruptObject("pending owner source child missing".into())
            })?;
            Ok(OwnerCoverage {
                owner,
                desired: OwnerInputBinding {
                    source: Some(source),
                    projection: None,
                    derivation_policy: policy,
                    producer_context: Digest([0; 32]),
                },
                covered: None,
                searchable: None,
            })
        }
    }
}
