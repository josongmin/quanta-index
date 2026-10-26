//! Compact representation of the existing logical contribution; no separate
//! object identity is claimed for inline data. Every source/occurrence remains
//! an explicit physical child of the owning map node.
use super::{
    Contribution, EntryMap, ImmutableObjectPort, IndexError, WorkBudget, check_term_key,
    contribution_children, entry_map,
};
use quanta_index_core::index::{MAX_MAP_INLINE_BYTES, MAX_MAP_INLINE_CHILDREN, MapValue};

pub(super) fn inline(
    value: &Contribution,
    budget: &WorkBudget,
) -> Result<Option<MapValue>, IndexError> {
    budget.checkpoint()?;
    let Some(small) = value.inventory.inline_map() else {
        return Ok(None);
    };
    let children = contribution_children(value)?;
    if children.as_slice().len() > MAX_MAP_INLINE_CHILDREN {
        return Ok(None);
    }
    let mut length = 9usize;
    for slot in 0..small.len() {
        let (key, _) = small.entry(slot)?.ok_or_else(|| {
            IndexError::CorruptObject("missing contribution inventory entry".into())
        })?;
        check_term_key(key)?;
        length = length
            .checked_add(1)
            .and_then(|bytes| bytes.checked_add(key.len()))
            .ok_or(IndexError::ResourceLimited)?;
    }
    if length > MAX_MAP_INLINE_BYTES {
        return Ok(None);
    }
    let mut data = [0; MAX_MAP_INLINE_BYTES];
    *data.first_mut().ok_or(IndexError::ResourceLimited)? =
        u8::try_from(small.len()).map_err(|_error| IndexError::ResourceLimited)?;
    data.get_mut(1..9)
        .ok_or(IndexError::ResourceLimited)?
        .copy_from_slice(&value.indexed_tokens.to_be_bytes());
    let mut at = 9usize;
    for slot in 0..small.len() {
        let (key, _) = small.entry(slot)?.ok_or_else(|| {
            IndexError::CorruptObject("missing contribution inventory entry".into())
        })?;
        *data.get_mut(at).ok_or(IndexError::ResourceLimited)? =
            u8::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?;
        at = at.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        let end = at
            .checked_add(key.len())
            .ok_or(IndexError::ResourceLimited)?;
        data.get_mut(at..end)
            .ok_or(IndexError::ResourceLimited)?
            .copy_from_slice(key);
        at = end;
    }
    MapValue::inline(
        data.get(..at).ok_or(IndexError::ResourceLimited)?,
        children.as_slice().iter().copied(),
    )
    .map(Some)
}

pub(super) fn read(
    stored_value: MapValue,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<Contribution, IndexError> {
    budget.checkpoint()?;
    let value = match stored_value {
        MapValue::Object(reference) => {
            let stored = objects.get(reference, budget)?;
            let value =
                super::super::metadata::decode_fixed::<Contribution>(&stored.payload, budget)?;
            if stored.registration.reference != reference
                || stored.registration.children.as_slice()
                    != contribution_children(&value)?.as_slice()
            {
                return Err(IndexError::CorruptObject(
                    "native text contribution edges mismatch".into(),
                ));
            }
            value
        }
        MapValue::Inline(inline) => {
            let data = inline.data()?;
            let count =
                usize::from(*data.first().ok_or_else(|| {
                    IndexError::CorruptObject("contribution count missing".into())
                })?);
            let children = inline.children()?;
            if count > entry_map::INLINE_ENTRIES
                || children.len() != count.checked_add(1).ok_or(IndexError::ResourceLimited)?
            {
                return Err(IndexError::CorruptObject(
                    "contribution child count differs".into(),
                ));
            }
            let tokens = u64::from_be_bytes(
                data.get(1..9)
                    .ok_or_else(|| {
                        IndexError::CorruptObject("contribution token count missing".into())
                    })?
                    .try_into()
                    .map_err(|_error| {
                        IndexError::CorruptObject("contribution token count width".into())
                    })?,
            );
            let record = *children.first().ok_or_else(|| {
                IndexError::CorruptObject("contribution source child missing".into())
            })?;
            let mut at = 9usize;
            let mut small = entry_map::SmallMap::default();
            for child in children.get(1..).ok_or(IndexError::ResourceLimited)? {
                let length = usize::from(*data.get(at).ok_or_else(|| {
                    IndexError::CorruptObject("contribution key length missing".into())
                })?);
                at = at.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                let end = at.checked_add(length).ok_or(IndexError::ResourceLimited)?;
                let key = data.get(at..end).ok_or_else(|| {
                    IndexError::CorruptObject("contribution key truncated".into())
                })?;
                check_term_key(key)?;
                if !small.push(key, *child)? {
                    return Err(IndexError::CorruptObject(
                        "contribution inventory exceeds bound".into(),
                    ));
                }
                at = end;
            }
            if at != data.len() {
                return Err(IndexError::CorruptObject(
                    "contribution trailing data".into(),
                ));
            }
            Contribution {
                record,
                inventory: EntryMap::inline(&small),
                indexed_tokens: tokens,
            }
        }
    };
    if let Some(small) = value.inventory.inline_map() {
        for slot in 0..small.len() {
            check_term_key(
                small
                    .entry(slot)?
                    .ok_or_else(|| IndexError::CorruptObject("missing inventory entry".into()))?
                    .0,
            )?;
        }
    }
    Ok(value)
}
