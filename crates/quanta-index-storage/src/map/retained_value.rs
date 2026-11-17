//! Attempt-local value storage for decoded and newly built radix nodes.
//! The public value and the object wire keep their five-child contract.

use std::sync::Arc;

use quanta_index_core::index::{
    IndexError, InlineMapValue, MapValue, MemoryLease, MemoryLifetime, ObjectRef, WorkBudget,
    WorkUsage,
};

pub(super) struct ManyChildren {
    value: InlineMapValue,
    // Shared clones retain the original allocation grant until the last owner.
    _memory: Option<MemoryLease>,
}

#[derive(Clone)]
pub(super) enum RetainedMapValue {
    Object(ObjectRef),
    Inline {
        data: [u8; 32],
        len: u8,
        child: Option<ObjectRef>,
    },
    Many(Arc<ManyChildren>),
}

impl From<ObjectRef> for RetainedMapValue {
    fn from(reference: ObjectRef) -> Self {
        Self::Object(reference)
    }
}

impl PartialEq for RetainedMapValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Object(left), Self::Object(right)) => left == right,
            (
                Self::Inline {
                    data: left,
                    len: left_len,
                    child: left_child,
                },
                Self::Inline {
                    data: right,
                    len: right_len,
                    child: right_child,
                },
            ) => left_len == right_len && left_child == right_child && left == right,
            (Self::Many(left), Self::Many(right)) => left.value == right.value,
            _ => false,
        }
    }
}
impl Eq for RetainedMapValue {}

impl RetainedMapValue {
    pub(super) fn from_public(
        value: MapValue,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        match value {
            MapValue::Object(reference) => Ok(Self::Object(reference)),
            MapValue::Inline(value) => Self::from_inline(value, budget),
        }
    }

    pub(super) fn from_parts(
        data: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        if data.len() > 32 || children.len() > 5 {
            return Err(IndexError::InvalidInput(
                "retained inline map value exceeds bound".into(),
            ));
        }
        if children.len() <= 1 {
            return Self::small(data, children.first().copied(), budget);
        }
        let MapValue::Inline(value) = MapValue::inline(data, children.iter().copied())? else {
            return Err(IndexError::BackendViolation(
                "inline map constructor returned object".into(),
            ));
        };
        Self::from_inline(value, budget)
    }

    fn from_inline(value: InlineMapValue, budget: &mut WorkBudget) -> Result<Self, IndexError> {
        let data = value.data()?;
        let children = value.children()?;
        if children.len() <= 1 {
            return Self::small(data, children.first().copied(), budget);
        }
        let bytes = MemoryLease::shared_allocation_backing_layout(std::alloc::Layout::new::<
            ManyChildren,
        >())?
        .size();
        let bytes = u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        let memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
        Ok(Self::Many(Arc::new(ManyChildren {
            value,
            _memory: memory,
        })))
    }

    fn small(
        data: &[u8],
        child: Option<ObjectRef>,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        // Moving into the compact node slot copies only live data and one edge.
        let bytes = data
            .len()
            .checked_add(if child.is_some() { 32 } else { 0 })
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let mut stored = [0; 32];
        stored
            .get_mut(..data.len())
            .ok_or(IndexError::ResourceLimited)?
            .copy_from_slice(data);
        Ok(Self::Inline {
            data: stored,
            len: u8::try_from(data.len()).map_err(|_error| IndexError::ResourceLimited)?,
            child,
        })
    }

    pub(super) fn children(&self) -> Result<&[ObjectRef], IndexError> {
        match self {
            Self::Object(reference) => Ok(std::slice::from_ref(reference)),
            Self::Inline {
                child: Some(reference),
                ..
            } => Ok(std::slice::from_ref(reference)),
            Self::Inline { child: None, .. } => Ok(&[]),
            Self::Many(value) => value.value.children(),
        }
    }

    pub(super) fn inline_data(&self) -> Result<Option<&[u8]>, IndexError> {
        match self {
            Self::Object(_) => Ok(None),
            Self::Inline { data, len, .. } => {
                data.get(..usize::from(*len)).map(Some).ok_or_else(|| {
                    IndexError::CorruptObject("retained map length exceeds bound".into())
                })
            }
            Self::Many(value) => value.value.data().map(Some),
        }
    }

    pub(super) fn to_public(&self, budget: &mut WorkBudget) -> Result<MapValue, IndexError> {
        match self {
            Self::Object(reference) => Ok(MapValue::Object(*reference)),
            Self::Many(value) => {
                budget.charge(WorkUsage {
                    bytes: u64::try_from(std::mem::size_of::<MapValue>())
                        .map_err(|_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                Ok(MapValue::Inline(value.value))
            }
            Self::Inline { .. } => {
                budget.charge(WorkUsage {
                    bytes: u64::try_from(std::mem::size_of::<MapValue>())
                        .map_err(|_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                MapValue::inline(
                    self.inline_data()?.ok_or(IndexError::ResourceLimited)?,
                    self.children()?.iter().copied(),
                )
            }
        }
    }

    pub(super) fn equals_public(&self, other: MapValue) -> Result<bool, IndexError> {
        match (self, other) {
            (Self::Object(left), MapValue::Object(right)) => Ok(*left == right),
            (Self::Object(_), MapValue::Inline(_))
            | (Self::Inline { .. } | Self::Many(_), MapValue::Object(_)) => Ok(false),
            (value, MapValue::Inline(other)) => Ok(value.inline_data()? == Some(other.data()?)
                && value.children()? == other.children()?),
        }
    }
}
