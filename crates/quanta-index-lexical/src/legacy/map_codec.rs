//! Canonical persisted map codec shared by write and existing-only readers.
use super::object_binary::{corrupt, storage};
use super::object_codec::{BlobRef, ObjectKind, ObjectRef, PageRef};
use quanta_index_core::CoreError;
fn invalid(message: &str) -> CoreError {
    CoreError::InvalidContract(format!("lexical object store: {message}"))
}
pub(crate) const PAGE_MAGIC: &[u8; 8] = b"QLMAP003";
pub(crate) const LEAF_INLINE: u8 = 0;
pub(crate) const LEAF_DICTIONARY: u8 = 1;
// Only the Native gram-route bootstrap writes this bounded fixed-key format.
pub(crate) const LEAF_WIDE_GRAM_DICTIONARY: u8 = 2;
pub(crate) const BLOB_REF_WIRE_BYTES: usize = 81;
pub(crate) const MAX_KEY_BYTES: usize = 256;
pub(crate) const LEAF_FANOUT: usize = 256;
pub(crate) const WIDE_GRAM_FANOUT: usize = 1024;
pub(crate) const GRAM_ROUTE_KEY_BYTES: usize = 12;
// Keep a leaf within one 32 KiB payload even when all values use inline wire.
// Dictionary mode can only reduce the encoded width from this bound.
pub(crate) const LEAF_INLINE_BYTES: usize = 32 * 1024;
pub(crate) const LEAF_DEPENDENCIES: usize = 128;
pub(crate) const BRANCH_FANOUT: usize = 64;
pub(crate) use crate::legacy::names::MAX_MAP_HEIGHT;

type PageExtent = (Vec<u8>, Vec<u8>, u64, u8, Vec<ObjectRef>);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Page {
    Leaf(Vec<(Vec<u8>, BlobRef)>),
    Branch(Vec<PageRef>),
}

pub(crate) fn distinct_leaf_values(rows: &[(Vec<u8>, BlobRef)]) -> Result<Vec<BlobRef>, CoreError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(rows.len())
        .map_err(|error| storage("reserve leaf value dictionary", error))?;
    values.extend(rows.iter().map(|(_, value)| value.clone()));
    values.sort_unstable();
    values.dedup();
    Ok(values)
}

pub(crate) fn dictionary_is_smaller(rows: usize, unique_values: usize) -> bool {
    // Both V2 modes carry the one-byte tag. Fanout bounds make these
    // saturating operations exact; equal-sized encodings use inline mode.
    2_usize
        .saturating_add(BLOB_REF_WIRE_BYTES.saturating_mul(unique_values))
        .saturating_add(2_usize.saturating_mul(rows))
        < BLOB_REF_WIRE_BYTES.saturating_mul(rows)
}

pub(crate) fn page_extent(page: &Page) -> Result<PageExtent, CoreError> {
    match page {
        Page::Leaf(entries) => {
            if entries.is_empty()
                || entries.len() > WIDE_GRAM_FANOUT
                || (entries.len() > LEAF_FANOUT
                    && entries
                        .iter()
                        .any(|(key, _)| key.len() != GRAM_ROUTE_KEY_BYTES))
            {
                return Err(invalid("leaf page fanout"));
            }
            let first = entries
                .first()
                .ok_or_else(|| invalid("empty leaf"))?
                .0
                .clone();
            let last = entries
                .last()
                .ok_or_else(|| invalid("empty leaf"))?
                .0
                .clone();
            let count =
                u64::try_from(entries.len()).map_err(|error| storage("leaf count width", error))?;
            let mut dependencies = Vec::new();
            dependencies
                .try_reserve_exact(entries.len())
                .map_err(|error| storage("reserve leaf dependencies", error))?;
            dependencies.extend(entries.iter().map(|(_, blob)| blob.root.clone()));
            dependencies.sort_unstable();
            dependencies.dedup();
            if dependencies.len() > LEAF_DEPENDENCIES {
                return Err(invalid("leaf dependency bound"));
            }
            Ok((first, last, count, 0, dependencies))
        }
        Page::Branch(children) => {
            if children.is_empty() || children.len() > BRANCH_FANOUT {
                return Err(invalid("branch page fanout"));
            }
            let first = children
                .first()
                .ok_or_else(|| invalid("empty branch"))?
                .first
                .clone();
            let last = children
                .last()
                .ok_or_else(|| invalid("empty branch"))?
                .last
                .clone();
            let height = children
                .first()
                .ok_or_else(|| invalid("empty branch"))?
                .height
                .checked_add(1)
                .ok_or_else(|| invalid("map height overflow"))?;
            if height > MAX_MAP_HEIGHT {
                return Err(invalid("map height exceeds bound"));
            }
            let count = children
                .iter()
                .try_fold(0_u64, |sum, child| sum.checked_add(child.entries))
                .ok_or_else(|| invalid("map subtree count overflow"))?;
            let mut dependencies = Vec::new();
            dependencies
                .try_reserve_exact(children.len())
                .map_err(|error| storage("reserve branch dependencies", error))?;
            dependencies.extend(children.iter().map(|child| child.object.clone()));
            Ok((first, last, count, height, dependencies))
        }
    }
}

pub(crate) fn page_map_encoded_bytes(page: &Page, own_bytes: u64) -> Result<u64, CoreError> {
    if own_bytes == 0 {
        return Err(corrupt("map page encoded width is zero"));
    }
    match page {
        Page::Leaf(_) => Ok(own_bytes),
        Page::Branch(children) => children.iter().try_fold(own_bytes, |sum, child| {
            if child.object.bytes == 0 || child.map_encoded_bytes < child.object.bytes {
                return Err(corrupt("map child encoded summary is invalid"));
            }
            sum.checked_add(child.map_encoded_bytes)
                .ok_or_else(|| corrupt("map encoded subtree width overflows"))
        }),
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], CoreError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| corrupt("map cursor overflow"))?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| corrupt("truncated map page"))?;
        self.offset = end;
        Ok(value)
    }
    fn u8(&mut self) -> Result<u8, CoreError> {
        Ok(*self
            .take(1)?
            .first()
            .ok_or_else(|| corrupt("map u8 absent"))?)
    }
    fn u16(&mut self) -> Result<u16, CoreError> {
        let raw = self.take(2)?;
        Ok(u16::from_be_bytes(
            raw.try_into()
                .map_err(|_conversion_error| corrupt("map u16 width"))?,
        ))
    }
    fn u64(&mut self) -> Result<u64, CoreError> {
        let mut raw = [0_u8; 8];
        raw.copy_from_slice(self.take(8)?);
        Ok(u64::from_be_bytes(raw))
    }
    fn digest(&mut self) -> Result<[u8; 32], CoreError> {
        let mut raw = [0_u8; 32];
        raw.copy_from_slice(self.take(32)?);
        Ok(raw)
    }
    fn key(&mut self) -> Result<Vec<u8>, CoreError> {
        let len = usize::from(self.u16()?);
        if len > MAX_KEY_BYTES {
            return Err(corrupt("map key exceeds bound"));
        }
        Ok(self.take(len)?.to_vec())
    }
    fn object_ref(&mut self) -> Result<ObjectRef, CoreError> {
        let kind =
            ObjectKind::from_code(self.u8()?).ok_or_else(|| corrupt("unknown map object kind"))?;
        let bytes = self.u64()?;
        let sha256 = self.digest()?;
        Ok(ObjectRef {
            kind,
            bytes,
            sha256,
        })
    }
    fn blob_ref(&mut self) -> Result<BlobRef, CoreError> {
        Ok(BlobRef {
            root: self.object_ref()?,
            bytes: self.u64()?,
            sha256: self.digest()?,
        })
    }
}

pub(crate) fn decode_page(
    payload: &[u8],
    reference: &PageRef,
    object_deps: &[ObjectRef],
) -> Result<Page, CoreError> {
    let mut cursor = Cursor {
        bytes: payload,
        offset: 0,
    };
    if cursor.take(8)? != PAGE_MAGIC {
        return Err(corrupt("map page magic differs"));
    }
    let height = cursor.u8()?;
    let count = usize::from(cursor.u16()?);
    let entries = cursor.u64()?;
    let first = cursor.key()?;
    let last = cursor.key()?;
    if height != reference.height
        || entries != reference.entries
        || first != reference.first
        || last != reference.last
        || height > MAX_MAP_HEIGHT
        || count == 0
    {
        return Err(corrupt("map page descriptor differs"));
    }
    let page = if height == 0 {
        if count > WIDE_GRAM_FANOUT
            || entries
                != u64::try_from(count).map_err(|_conversion_error| corrupt("leaf count width"))?
        {
            return Err(corrupt("leaf count differs"));
        }
        let mode = cursor.u8()?;
        let mut dictionary = Vec::new();
        let mut used = Vec::new();
        match mode {
            LEAF_INLINE if count <= LEAF_FANOUT => {}
            LEAF_DICTIONARY | LEAF_WIDE_GRAM_DICTIONARY => {
                let wide = mode == LEAF_WIDE_GRAM_DICTIONARY;
                if wide {
                    if count <= LEAF_FANOUT
                        || count > WIDE_GRAM_FANOUT
                        || payload.len() > LEAF_INLINE_BYTES
                        || first.len() != GRAM_ROUTE_KEY_BYTES
                        || last.len() != GRAM_ROUTE_KEY_BYTES
                        || object_deps.len() > LEAF_DEPENDENCIES
                    {
                        return Err(corrupt("wide gram leaf bounds differ"));
                    }
                } else if count > LEAF_FANOUT {
                    return Err(corrupt("ordinary leaf fanout differs"));
                }
                let unique_values = usize::from(cursor.u16()?);
                if unique_values == 0
                    || unique_values > count
                    || (wide && unique_values > LEAF_DEPENDENCIES)
                    || !dictionary_is_smaller(count, unique_values)
                {
                    return Err(corrupt("leaf dictionary count differs"));
                }
                dictionary
                    .try_reserve_exact(unique_values)
                    .map_err(|error| storage("reserve decoded leaf dictionary", error))?;
                used.try_reserve_exact(unique_values)
                    .map_err(|error| storage("reserve leaf dictionary usage", error))?;
                for _ in 0..unique_values {
                    let value = cursor.blob_ref()?;
                    if dictionary.last().is_some_and(|prior| prior >= &value) {
                        return Err(corrupt("leaf dictionary values repeat or are not sorted"));
                    }
                    dictionary.push(value);
                    used.push(false);
                }
            }
            _ => return Err(corrupt("leaf value mode differs")),
        }
        let mut rows = Vec::new();
        rows.try_reserve_exact(count)
            .map_err(|error| storage("reserve map leaf", error))?;
        for _ in 0..count {
            let key = cursor.key()?;
            let value = if mode == LEAF_INLINE {
                cursor.blob_ref()?
            } else {
                let index = usize::from(cursor.u16()?);
                let used_entry = used
                    .get_mut(index)
                    .ok_or_else(|| corrupt("leaf dictionary index is out of range"))?;
                *used_entry = true;
                dictionary
                    .get(index)
                    .ok_or_else(|| corrupt("leaf dictionary value is absent"))?
                    .clone()
            };
            if mode == LEAF_WIDE_GRAM_DICTIONARY && key.len() != GRAM_ROUTE_KEY_BYTES {
                return Err(corrupt("wide gram leaf key width differs"));
            }
            rows.push((key, value));
        }
        if (mode == LEAF_DICTIONARY || mode == LEAF_WIDE_GRAM_DICTIONARY) && used.contains(&false) {
            return Err(corrupt("leaf dictionary contains an unused value"));
        }
        if mode == LEAF_INLINE && dictionary_is_smaller(count, distinct_leaf_values(&rows)?.len()) {
            return Err(corrupt("leaf inline mode is not minimal"));
        }
        if rows
            .windows(2)
            .any(|pair| match (pair.first(), pair.get(1)) {
                (Some(a), Some(b)) => a.0 >= b.0,
                _ => true,
            })
        {
            return Err(corrupt("leaf keys are not strictly sorted"));
        }
        Page::Leaf(rows)
    } else {
        if count > BRANCH_FANOUT {
            return Err(corrupt("branch fanout exceeds bound"));
        }
        let mut children = Vec::new();
        children
            .try_reserve_exact(count)
            .map_err(|error| storage("reserve map branch", error))?;
        for _ in 0..count {
            children.push(PageRef {
                object: cursor.object_ref()?,
                entries: cursor.u64()?,
                height: cursor.u8()?,
                map_encoded_bytes: cursor.u64()?,
                first: cursor.key()?,
                last: cursor.key()?,
            });
        }
        if children.iter().any(|child| {
            child.height.checked_add(1) != Some(height)
                || child.entries == 0
                || child.first > child.last
                || child.object.kind != ObjectKind::MapPage
                || child.object.bytes == 0
                || child.map_encoded_bytes < child.object.bytes
        }) || children
            .windows(2)
            .any(|pair| match (pair.first(), pair.get(1)) {
                (Some(a), Some(b)) => a.last >= b.first,
                _ => true,
            })
            || children
                .iter()
                .try_fold(0_u64, |sum, child| sum.checked_add(child.entries))
                != Some(entries)
        {
            return Err(corrupt("branch child ordering, height or count differs"));
        }
        Page::Branch(children)
    };
    if cursor.offset != payload.len() {
        return Err(corrupt("map page has trailing bytes"));
    }
    let (derived_first, derived_last, derived_entries, derived_height, mut dependencies) =
        page_extent(&page).map_err(|_conversion_error| corrupt("map page extent is invalid"))?;
    dependencies.sort();
    dependencies.dedup();
    if derived_first != first
        || derived_last != last
        || derived_entries != entries
        || derived_height != height
        || dependencies != object_deps
    {
        return Err(corrupt("map page fences or dependency inventory differ"));
    }
    if page_map_encoded_bytes(&page, reference.object.bytes)? != reference.map_encoded_bytes {
        return Err(corrupt("map encoded subtree summary differs"));
    }
    Ok(page)
}
