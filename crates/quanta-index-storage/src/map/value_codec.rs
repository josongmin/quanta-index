//! Node-local value fields; physical references remain in the exact child list.
use quanta_index_core::index::{IndexError, MapValue, ObjectRef};

pub(super) const MAX_VALUE_PAYLOAD: usize = 35;
pub(super) fn payload_len(value: MapValue) -> Result<usize, IndexError> {
    match value {
        MapValue::Object(_) => Ok(1),
        MapValue::Inline(value) => value
            .data()?
            .len()
            .checked_add(3)
            .ok_or(IndexError::ResourceLimited),
    }
}
pub(super) fn write(value: MapValue, payload: &mut Vec<u8>) -> Result<(), IndexError> {
    match value {
        MapValue::Object(_) => payload.push(0),
        MapValue::Inline(value) => {
            let data = value.data()?;
            payload.push(1);
            payload.push(u8::try_from(data.len()).map_err(|_error| IndexError::ResourceLimited)?);
            payload.extend_from_slice(data);
            payload.push(
                u8::try_from(value.children()?.len())
                    .map_err(|_error| IndexError::ResourceLimited)?,
            );
        }
    }
    Ok(())
}
pub(super) fn read(
    payload: &[u8],
    at: &mut usize,
    children: &mut impl Iterator<Item = ObjectRef>,
) -> Result<MapValue, IndexError> {
    let invalid = || IndexError::CorruptObject("invalid map value field".into());
    let tag = *payload.get(*at).ok_or_else(invalid)?;
    *at = at.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    match tag {
        0 => Ok(MapValue::Object(children.next().ok_or_else(invalid)?)),
        1 => {
            let len = usize::from(*payload.get(*at).ok_or_else(invalid)?);
            if len > quanta_index_core::index::MAX_MAP_INLINE_BYTES {
                return Err(invalid());
            }
            *at = at.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            let end = at.checked_add(len).ok_or(IndexError::ResourceLimited)?;
            let data = payload.get(*at..end).ok_or_else(invalid)?;
            let count = usize::from(*payload.get(end).ok_or_else(invalid)?);
            if count > quanta_index_core::index::MAX_MAP_INLINE_CHILDREN {
                return Err(invalid());
            }
            let mut refs = [ObjectRef(quanta_index_core::index::Digest([0; 32]));
                quanta_index_core::index::MAX_MAP_INLINE_CHILDREN];
            let selected = refs.get_mut(..count).ok_or_else(invalid)?;
            for target in selected.iter_mut() {
                *target = children.next().ok_or_else(invalid)?;
            }
            *at = end.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            MapValue::inline(data, selected.iter().copied())
        }
        _ => Err(invalid()),
    }
}
