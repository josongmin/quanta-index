//! Wire and memory admission for a bounded term position vector.

use quanta_index_core::index::{IndexError, WorkBudget};
use serde::{Deserialize, Serialize};

pub(in crate::native) const MAX_POSITIONS_PER_TERM: u64 = 8_192;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::native) struct Occurrence {
    pub(in crate::native) positions: Positions,
}
quanta_index_contract::native_serde_struct!(Occurrence {
    positions: Positions,
});

/// Same sequence wire shape; enforce its leaf envelope while decoding, before
/// an attacker-controlled sequence length can reserve a corpus-sized vector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::native) struct Positions(pub(in crate::native) Vec<u32>);

pub(in crate::native) struct PreparedPositions {
    occurrence: Occurrence,
    maximum: usize,
    memory: Option<quanta_index_core::index::MemoryLease>,
}

impl PreparedPositions {
    pub(in crate::native) fn new(maximum: usize) -> Result<Self, IndexError> {
        if maximum == 0
            || u64::try_from(maximum).map_err(|_error| IndexError::ResourceLimited)?
                > MAX_POSITIONS_PER_TERM
        {
            return Err(IndexError::ResourceLimited);
        }
        Ok(Self {
            occurrence: Occurrence {
                positions: Positions(Vec::new()),
            },
            maximum,
            memory: None,
        })
    }

    pub(in crate::native) fn as_slice(&self) -> &[u32] {
        &self.occurrence.positions
    }

    pub(in crate::native) fn as_occurrence(&self) -> &Occurrence {
        &self.occurrence
    }

    pub(in crate::native) fn push(
        &mut self,
        position: u32,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        let needed = self
            .as_slice()
            .len()
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if needed > self.maximum {
            return Err(IndexError::ResourceLimited);
        }
        if needed > self.occurrence.positions.0.capacity() {
            let capacity = self
                .occurrence
                .positions
                .0
                .capacity()
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?
                .max(needed)
                .min(self.maximum);
            let bytes = capacity
                .checked_mul(std::mem::size_of::<u32>())
                .ok_or(IndexError::ResourceLimited)?;
            let memory = budget.reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                quanta_index_core::index::MemoryLifetime::Temporary,
            )?;
            let mut positions = Vec::new();
            positions
                .try_reserve_exact(capacity)
                .map_err(|_error| IndexError::ResourceLimited)?;
            if positions.capacity() > capacity {
                return Err(IndexError::ResourceLimited);
            }
            positions.append(&mut self.occurrence.positions.0);
            self.occurrence.positions.0 = positions;
            self.memory = memory;
        } else {
            budget.checkpoint()?;
        }
        self.occurrence.positions.0.push(position);
        Ok(())
    }
}

impl std::ops::Deref for Positions {
    type Target = [u32];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Serialize for Positions {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let count = u64::try_from(self.len()).map_err(serde::ser::Error::custom)?;
        if count == 0 || count > MAX_POSITIONS_PER_TERM {
            return Err(serde::ser::Error::custom(
                "text positions exceed their leaf envelope",
            ));
        }
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Positions {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Positions;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("1..=8192 text positions")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let limit =
                    usize::try_from(MAX_POSITIONS_PER_TERM).map_err(serde::de::Error::custom)?;
                if sequence.size_hint().is_some_and(|count| count > limit) {
                    return Err(serde::de::Error::custom(
                        "text position sequence exceeds its leaf envelope",
                    ));
                }
                let mut positions = Vec::new();
                while let Some(position) = sequence.next_element::<u32>()? {
                    if positions.len() == limit {
                        return Err(serde::de::Error::custom(
                            "text position sequence exceeds its leaf envelope",
                        ));
                    }
                    positions.push(position);
                }
                if positions.is_empty() {
                    return Err(serde::de::Error::custom("text position sequence is empty"));
                }
                Ok(Positions(positions))
            }
        }
        deserializer.deserialize_seq(Visitor)
    }
}
