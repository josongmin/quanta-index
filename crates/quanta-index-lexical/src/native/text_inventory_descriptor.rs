//! Validate the exact inline wire for one cold inventory descriptor.

use super::text_occurrence::block::MAX_BLOCK_TERMS;
use quanta_index_core::index::{IndexError, MapValue, ObjectRef};

pub(super) const PACKED: u8 = 1;
pub(super) const LEAF: u8 = 2;
pub(super) fn corrupt(reason: &str) -> IndexError {
    IndexError::CorruptObject(format!("inventory directory: {reason}"))
}

pub(super) fn descriptor(
    kind: u8,
    terms: usize,
    reference: ObjectRef,
) -> Result<MapValue, IndexError> {
    if !((kind == PACKED && (2..=MAX_BLOCK_TERMS).contains(&terms)) || (kind == LEAF && terms == 1))
    {
        return Err(corrupt("descriptor shape invalid"));
    }
    let mut data = [0; 3];
    data[0] = kind;
    data[1..].copy_from_slice(
        &u16::try_from(terms)
            .map_err(|_error| IndexError::ResourceLimited)?
            .to_be_bytes(),
    );
    MapValue::inline(&data, Some(reference))
}
pub(super) fn decode(value: MapValue) -> Result<(u8, usize, ObjectRef), IndexError> {
    let MapValue::Inline(inline) = value else {
        return Err(corrupt("descriptor not inline"));
    };
    let [kind, high, low] = inline.data()? else {
        return Err(corrupt("descriptor width differs"));
    };
    let reference = inline
        .single_child()?
        .ok_or_else(|| corrupt("child absent"))?;
    let terms = usize::from(u16::from_be_bytes([*high, *low]));
    if !((*kind == PACKED && (2..=MAX_BLOCK_TERMS).contains(&terms))
        || (*kind == LEAF && terms == 1))
    {
        return Err(corrupt("descriptor kind or count invalid"));
    }
    Ok((*kind, terms, reference))
}

/// A disposable inventory value carries either one occurrence or one exact
/// directory descriptor. Its child is DATA until the install receipt closes.
pub(super) fn inventory_reference(value: MapValue) -> Result<ObjectRef, IndexError> {
    match value {
        MapValue::Object(reference) => Ok(reference),
        MapValue::Inline(_) => decode(value).map(|(_, _, reference)| reference),
    }
}
