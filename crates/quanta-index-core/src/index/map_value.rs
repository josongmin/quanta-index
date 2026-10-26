//! Bounded map data, distinct from registered object identity.
use super::{Digest, IndexError, ObjectRef};

pub const MAX_MAP_INLINE_BYTES: usize = 32;
// Five children keep the complete Copy value below256 bytes without heap indirection.
// Larger logical records retain the current object representation.
pub const MAX_MAP_INLINE_CHILDREN: usize = 5;
pub const MAX_MAP_VALUE_WIRE_BYTES: usize =
    1 + 1 + MAX_MAP_INLINE_BYTES + 1 + 32 * MAX_MAP_INLINE_CHILDREN;

/// Fixed data and a bounded explicit physical child list. Unused slots are
/// zeroed and never encoded, returned or registered. No heap backing escapes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InlineMapValue {
    bytes: [u8; MAX_MAP_INLINE_BYTES],
    len: u8,
    children: [ObjectRef; MAX_MAP_INLINE_CHILDREN],
    child_count: u8,
}
impl InlineMapValue {
    pub fn data(&self) -> Result<&[u8], IndexError> {
        self.bytes
            .get(..usize::from(self.len))
            .ok_or_else(|| IndexError::CorruptObject("inline map length exceeds bound".into()))
    }
    pub fn children(&self) -> Result<&[ObjectRef], IndexError> {
        self.children
            .get(..usize::from(self.child_count))
            .ok_or_else(|| IndexError::CorruptObject("inline map child count exceeds bound".into()))
    }
    pub fn single_child(&self) -> Result<Option<ObjectRef>, IndexError> {
        match self.children()? {
            [] => Ok(None),
            [child] => Ok(Some(*child)),
            _ => Err(IndexError::CorruptObject(
                "expected at most one map child".into(),
            )),
        }
    }
}

/// One current value contract. Inline data never claims a separate `ObjectRef`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MapValue {
    Object(ObjectRef),
    Inline(InlineMapValue),
}
impl From<ObjectRef> for MapValue {
    fn from(reference: ObjectRef) -> Self {
        Self::Object(reference)
    }
}
impl MapValue {
    pub fn inline(
        data: &[u8],
        children: impl IntoIterator<Item = ObjectRef>,
    ) -> Result<Self, IndexError> {
        if data.len() > MAX_MAP_INLINE_BYTES {
            return Err(IndexError::InvalidInput(
                "inline map data exceeds 32 bytes".into(),
            ));
        }
        let mut bytes = [0; MAX_MAP_INLINE_BYTES];
        bytes
            .get_mut(..data.len())
            .ok_or(IndexError::ResourceLimited)?
            .copy_from_slice(data);
        let mut stored = [ObjectRef(Digest([0; 32])); MAX_MAP_INLINE_CHILDREN];
        let mut child_count = 0usize;
        for child in children {
            *stored.get_mut(child_count).ok_or_else(|| {
                IndexError::InvalidInput("inline map children exceed bound".into())
            })? = child;
            child_count = child_count
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
        }
        Ok(Self::Inline(InlineMapValue {
            bytes,
            len: u8::try_from(data.len()).map_err(|_error| IndexError::ResourceLimited)?,
            children: stored,
            child_count: u8::try_from(child_count).map_err(|_error| IndexError::ResourceLimited)?,
        }))
    }
    pub fn object_ref(self) -> Result<ObjectRef, IndexError> {
        match self {
            Self::Object(reference) => Ok(reference),
            Self::Inline(_) => Err(IndexError::CorruptObject(
                "expected object map value".into(),
            )),
        }
    }
    pub fn as_object_ref(&self) -> Result<&ObjectRef, IndexError> {
        match self {
            Self::Object(reference) => Ok(reference),
            Self::Inline(_) => Err(IndexError::CorruptObject(
                "expected object map value".into(),
            )),
        }
    }
    pub fn children(&self) -> Result<&[ObjectRef], IndexError> {
        match self {
            Self::Object(reference) => Ok(std::slice::from_ref(reference)),
            Self::Inline(value) => value.children(),
        }
    }
    pub fn single_child(&self) -> Result<Option<ObjectRef>, IndexError> {
        match self {
            Self::Object(reference) => Ok(Some(*reference)),
            Self::Inline(value) => value.single_child(),
        }
    }
    pub fn encoded(self) -> Result<EncodedMapValue, IndexError> {
        let mut encoded = EncodedMapValue {
            bytes: [0; MAX_MAP_VALUE_WIRE_BYTES],
            len: 0,
        };
        match self {
            Self::Object(reference) => {
                encoded.extend(&[0])?;
                encoded.extend(&reference.0.0)?;
            }
            Self::Inline(value) => {
                encoded.extend(&[1, value.len])?;
                encoded.extend(value.data()?)?;
                encoded.extend(&[value.child_count])?;
                for child in value.children()? {
                    encoded.extend(&child.0.0)?;
                }
            }
        }
        Ok(encoded)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, IndexError> {
        let invalid = || IndexError::CorruptObject("invalid encoded map value".into());
        let reference = |bytes: &[u8]| -> Result<ObjectRef, IndexError> {
            Ok(ObjectRef(Digest(
                bytes.try_into().map_err(|_error| invalid())?,
            )))
        };
        match bytes.first() {
            Some(0) if bytes.len() == 33 => Ok(Self::Object(reference(
                bytes.get(1..).ok_or_else(invalid)?,
            )?)),
            Some(1) => {
                let len = usize::from(*bytes.get(1).ok_or_else(invalid)?);
                if len > MAX_MAP_INLINE_BYTES {
                    return Err(invalid());
                }
                let end = 2usize.checked_add(len).ok_or_else(invalid)?;
                let data = bytes.get(2..end).ok_or_else(invalid)?;
                let (count, child_bytes) = bytes
                    .get(end..)
                    .and_then(|bytes| bytes.split_first())
                    .ok_or_else(invalid)?;
                let count = usize::from(*count);
                let expected = count.checked_mul(32).ok_or_else(invalid)?;
                if count > MAX_MAP_INLINE_CHILDREN || child_bytes.len() != expected {
                    return Err(invalid());
                }
                let mut children = [ObjectRef(Digest([0; 32])); MAX_MAP_INLINE_CHILDREN];
                for (target, raw) in children.iter_mut().zip(child_bytes.chunks_exact(32)) {
                    *target = reference(raw)?;
                }
                Self::inline(
                    data,
                    children.get(..count).ok_or_else(invalid)?.iter().copied(),
                )
            }
            _ => Err(invalid()),
        }
    }
}

/// Bounded scratch/wire bytes; no allocation is needed before SQL binding.
#[derive(Clone, Copy, Debug)]
pub struct EncodedMapValue {
    bytes: [u8; MAX_MAP_VALUE_WIRE_BYTES],
    len: usize,
}
impl EncodedMapValue {
    fn extend(&mut self, bytes: &[u8]) -> Result<(), IndexError> {
        let end = self
            .len
            .checked_add(bytes.len())
            .ok_or(IndexError::ResourceLimited)?;
        self.bytes
            .get_mut(self.len..end)
            .ok_or(IndexError::ResourceLimited)?
            .copy_from_slice(bytes);
        self.len = end;
        Ok(())
    }
    pub fn as_slice(&self) -> Result<&[u8], IndexError> {
        self.bytes
            .get(..self.len)
            .ok_or_else(|| IndexError::CorruptObject("encoded map length exceeds bound".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn five_children_preserve_order_duplicates_and_exact_wire_bound() -> Result<(), IndexError> {
        let first = ObjectRef(Digest([3; 32]));
        let second = ObjectRef(Digest([4; 32]));
        let children = [first, second, first, second, first];
        let value = MapValue::inline(&[9; 32], children)?;
        let mut gold = vec![1, 32];
        gold.extend_from_slice(&[9; 32]);
        gold.push(5);
        for byte in [3, 4, 3, 4, 3] {
            gold.extend_from_slice(&[byte; 32]);
        }
        if gold.len() != 195
            || value.encoded()?.as_slice()? != gold
            || MapValue::decode(&gold)? != value
            || value.children()? != children
            || value.single_child().is_ok()
            || MapValue::inline(&[], [first; 6]).is_ok()
        {
            return Err(IndexError::BackendViolation(
                "multi-child contract changed".into(),
            ));
        }
        for bad in [
            gold.get(..194).ok_or(IndexError::ResourceLimited)?.to_vec(),
            [gold.as_slice(), &[0]].concat(),
            {
                let mut bytes = gold.clone();
                *bytes.get_mut(34).ok_or(IndexError::ResourceLimited)? = 6;
                bytes
            },
        ] {
            if MapValue::decode(&bad).is_ok() {
                return Err(IndexError::BackendViolation(
                    "invalid child count accepted".into(),
                ));
            }
        }
        Ok(())
    }
    #[test]
    fn value_kinds_children_and_exact_wire_are_independent() -> Result<(), IndexError> {
        let reference = ObjectRef(Digest([7; 32]));
        let object = MapValue::Object(reference);
        let scalar = MapValue::inline(&[7; 32], None)?;
        let compound = MapValue::inline(&[7; 32], Some(reference))?;
        if object == scalar
            || scalar == compound
            || scalar.single_child()?.is_some()
            || compound.single_child()? != Some(reference)
        {
            return Err(IndexError::BackendViolation(
                "value kind or explicit child changed".into(),
            ));
        }
        let mut object_gold = vec![0];
        object_gold.extend_from_slice(&[7; 32]);
        let mut scalar_gold = vec![1, 32];
        scalar_gold.extend_from_slice(&[7; 32]);
        scalar_gold.push(0);
        let mut compound_gold = vec![1, 32];
        compound_gold.extend_from_slice(&[7; 32]);
        compound_gold.push(1);
        compound_gold.extend_from_slice(&[7; 32]);
        for (value, gold) in [
            (object, object_gold),
            (scalar, scalar_gold),
            (compound, compound_gold),
            (MapValue::inline(&[], None)?, vec![1, 0, 0]),
        ] {
            if value.encoded()?.as_slice()? != gold || MapValue::decode(&gold)? != value {
                return Err(IndexError::BackendViolation(
                    "fixed map value wire changed".into(),
                ));
            }
            let mut extra = gold.clone();
            extra.push(0);
            if MapValue::decode(&extra).is_ok()
                || MapValue::decode(
                    gold.get(
                        ..gold
                            .len()
                            .checked_sub(1)
                            .ok_or(IndexError::ResourceLimited)?,
                    )
                    .ok_or(IndexError::ResourceLimited)?,
                )
                .is_ok()
            {
                return Err(IndexError::BackendViolation(
                    "partial or trailing value accepted".into(),
                ));
            }
        }
        if MapValue::inline(&[0; 33], None).is_ok()
            || scalar.object_ref().is_ok()
            || MapValue::decode(&[1, 33, 0]).is_ok()
            || MapValue::decode(&[2]).is_ok()
        {
            return Err(IndexError::BackendViolation(
                "wrong value kind/bound accepted".into(),
            ));
        }
        Ok(())
    }
}
