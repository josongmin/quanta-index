//! One bounded entry-map representation for Text inventories and memberships.

use quanta_index_core::index::{
    IndexError, MapPage, MapRoot, MemoryLifetime, ObjectRef, WorkBudget, WorkUsage,
    ports::{ImmutableObjectPort, PersistentMapPort},
};

pub(super) mod block;
pub(super) use block::BlockDraft;

pub(super) const LAYOUT_POLICY: &[u8] =
    b"entry-map:inventory-small8-32/positional-QITXEM01-512-16384-1024;posting-small8-32/tree\0";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum EntryMapUse {
    Inventory,
    Posting,
}

pub(super) const INLINE_ENTRIES: usize = 8;
const INLINE_KEY_BYTES: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Entry {
    len: u8,
    key: [u8; INLINE_KEY_BYTES],
    value: ObjectRef,
}
quanta_index_contract::native_serde_struct!(Entry {
    len: u8,
    key: [u8; INLINE_KEY_BYTES],
    value: ObjectRef
});

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::native) struct SmallMap {
    len: u8,
    rows: [Option<Entry>; INLINE_ENTRIES],
}
quanta_index_contract::native_serde_struct!(SmallMap { len: u8, rows: [Option<Entry>; INLINE_ENTRIES] });

impl SmallMap {
    pub(super) fn len(&self) -> usize {
        usize::from(self.len)
    }

    pub(super) fn entry(&self, slot: usize) -> Result<Option<(&[u8], ObjectRef)>, IndexError> {
        if slot >= self.len() {
            return Ok(None);
        }
        let row = self
            .rows
            .get(slot)
            .and_then(Option::as_ref)
            .ok_or_else(|| IndexError::CorruptObject("inline entry map has a hole".into()))?;
        let key = row
            .key
            .get(..usize::from(row.len))
            .ok_or_else(|| IndexError::CorruptObject("inline entry key exceeds bound".into()))?;
        Ok(Some((key, row.value)))
    }

    pub(super) fn validate(&self) -> Result<(), IndexError> {
        if self.len() > INLINE_ENTRIES {
            return Err(IndexError::CorruptObject(
                "inline entry count exceeds bound".into(),
            ));
        }
        let mut previous: Option<&[u8]> = None;
        for (slot, row) in self.rows.iter().enumerate() {
            if slot >= self.len() {
                if row.is_some() {
                    return Err(IndexError::CorruptObject(
                        "inline entry has a trailing slot".into(),
                    ));
                }
                continue;
            }
            let (key, _) = self
                .entry(slot)?
                .ok_or_else(|| IndexError::CorruptObject("missing inline entry".into()))?;
            let row = row
                .as_ref()
                .ok_or_else(|| IndexError::CorruptObject("missing inline entry".into()))?;
            if previous.is_some_and(|prior| prior >= key)
                || row
                    .key
                    .get(usize::from(row.len)..)
                    .is_none_or(|tail| tail.iter().any(|byte| *byte != 0))
            {
                return Err(IndexError::CorruptObject(
                    "inline entry order or padding differs".into(),
                ));
            }
            previous = Some(key);
        }
        Ok(())
    }

    /// False means the current representation cannot hold this valid entry.
    pub(super) fn push(&mut self, key: &[u8], value: ObjectRef) -> Result<bool, IndexError> {
        if key.len() > INLINE_KEY_BYTES || self.len() == INLINE_ENTRIES {
            return Ok(false);
        }
        if self.len() != 0 {
            let prior = self
                .entry(
                    self.len()
                        .checked_sub(1)
                        .ok_or(IndexError::ResourceLimited)?,
                )?
                .ok_or_else(|| {
                    IndexError::CorruptObject("inline entry prefix is incomplete".into())
                })?;
            if prior.0 >= key {
                return Err(IndexError::CorruptObject(
                    "inline entries are not strictly ordered".into(),
                ));
            }
        }
        let mut fixed = [0; INLINE_KEY_BYTES];
        fixed
            .get_mut(..key.len())
            .ok_or(IndexError::ResourceLimited)?
            .copy_from_slice(key);
        let slot = self.len();
        *self.rows.get_mut(slot).ok_or(IndexError::ResourceLimited)? = Some(Entry {
            len: u8::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?,
            key: fixed,
            value,
        });
        self.len = self.len.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        Ok(true)
    }
}

/// One current persisted representation; empty maps are always Small0.
#[expect(
    clippy::large_enum_variant,
    reason = "bounded fixed metadata stays Copy for admission-controlled decoding without boxed owners"
)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::native) enum EntryMap {
    Small(SmallMap),
    Block(ObjectRef),
    Tree(ObjectRef),
}
quanta_index_contract::native_serde_enum!(EntryMap {
    units { }
    newtypes { Small = 0 => SmallMap, Block = 1 => ObjectRef, Tree = 2 => ObjectRef }
    records { }
});
impl Default for EntryMap {
    fn default() -> Self {
        Self::Small(SmallMap::default())
    }
}
impl From<MapRoot> for EntryMap {
    fn from(root: MapRoot) -> Self {
        root.0.map_or_else(Self::default, Self::Tree)
    }
}
impl From<&EntryMap> for EntryMap {
    fn from(map: &EntryMap) -> Self {
        *map
    }
}

impl EntryMap {
    pub(super) fn inline(small: &SmallMap) -> Self {
        Self::Small(*small)
    }
    pub(super) fn inline_map(&self) -> Option<SmallMap> {
        match self {
            Self::Small(small) => Some(*small),
            Self::Block(_) | Self::Tree(_) => None,
        }
    }
    pub(super) fn tree_base(&self) -> MapRoot {
        MapRoot(match self {
            Self::Tree(reference) => Some(*reference),
            Self::Small(_) | Self::Block(_) => None,
        })
    }
    pub(super) fn validate(&self) -> Result<(), IndexError> {
        match self {
            Self::Small(small) => small.validate(),
            Self::Block(_) | Self::Tree(_) => Ok(()),
        }
    }
    pub(super) fn validate_for(&self, usage: EntryMapUse) -> Result<(), IndexError> {
        self.validate()?;
        if usage == EntryMapUse::Posting && matches!(self, Self::Block(_)) {
            return Err(IndexError::CorruptObject(
                "block inventory is not a posting representation".into(),
            ));
        }
        Ok(())
    }
    pub(super) fn children(&self) -> Result<impl Iterator<Item = ObjectRef>, IndexError> {
        self.validate()?;
        let (root, small) = match self {
            Self::Small(small) => (None, *small),
            Self::Block(reference) | Self::Tree(reference) => {
                (Some(*reference), SmallMap::default())
            }
        };
        Ok(root
            .into_iter()
            .chain(small.rows.into_iter().flatten().map(|entry| entry.value)))
    }

    pub(in crate::native) fn lookup(
        &self,
        objects: &dyn ImmutableObjectPort,
        maps: &dyn PersistentMapPort,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<ObjectRef>, IndexError> {
        self.validate()?;
        let small = match self {
            Self::Small(small) => small,
            Self::Tree(reference) => {
                return maps.lookup_object(MapRoot(Some(*reference)), key, budget);
            }
            Self::Block(reference) => return block::lookup(objects, *reference, key, budget),
        };
        budget.checkpoint()?;
        small.validate()?;
        budget.charge(WorkUsage {
            bytes: u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        for slot in 0..small.len() {
            let (stored, value) = small
                .entry(slot)?
                .ok_or_else(|| IndexError::CorruptObject("missing inline lookup entry".into()))?;
            match stored.cmp(key) {
                std::cmp::Ordering::Equal => return Ok(Some(value)),
                std::cmp::Ordering::Greater => break,
                std::cmp::Ordering::Less => {}
            }
        }
        Ok(None)
    }

    pub(super) fn scan(
        &self,
        objects: &dyn ImmutableObjectPort,
        maps: &dyn PersistentMapPort,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        self.validate()?;
        let small = match self {
            Self::Small(small) => small,
            Self::Tree(reference) => {
                return maps.scan(MapRoot(Some(*reference)), after, limit, budget);
            }
            Self::Block(reference) => {
                return block::scan(objects, *reference, after, limit, budget);
            }
        };
        budget.checkpoint()?;
        if limit == 0 || limit > 4096 {
            return Err(IndexError::InvalidInput(
                "entry scan limit exceeds bound".into(),
            ));
        }
        small.validate()?;
        let capacity = small
            .len()
            .min(usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?);
        let allocation = budget.reserve_memory(
            MapPage::allocation_bytes(capacity)?,
            MemoryLifetime::Retained,
        )?;
        let mut leases = Vec::new();
        leases
            .try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut has_more = false;
        for slot in 0..small.len() {
            budget.checkpoint()?;
            let (key, value) = small
                .entry(slot)?
                .ok_or_else(|| IndexError::CorruptObject("missing inline scan entry".into()))?;
            if after.is_some_and(|after| key <= after) {
                continue;
            }
            if entries.len() == capacity {
                has_more = true;
                break;
            }
            let bytes = u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?;
            budget.charge(WorkUsage {
                bytes,
                ..WorkUsage::default()
            })?;
            let memory = if bytes == 0 {
                None
            } else {
                budget.reserve_memory(bytes, MemoryLifetime::Retained)?
            };
            let mut owned = Vec::new();
            owned
                .try_reserve_exact(key.len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            owned.extend_from_slice(key);
            entries.push((owned, value.into()));
            leases.extend(memory);
        }
        Ok(MapPage::new(entries, has_more, leases, allocation))
    }
}
