use core::fmt;
use core::marker::PhantomData;

use serde::de::{self, Deserialize, Deserializer, SeqAccess, Visitor};

use crate::SymbolId;

pub(crate) struct BoundedVecV1<T, const MAXIMUM: usize>(Vec<T>);

impl<T, const MAXIMUM: usize> BoundedVecV1<T, MAXIMUM> {
    pub(crate) fn into_inner(self) -> Vec<T> {
        self.0
    }
}

struct BoundedVecV1Visitor<T, const MAXIMUM: usize>(PhantomData<T>);

impl<'de, T, const MAXIMUM: usize> Visitor<'de> for BoundedVecV1Visitor<T, MAXIMUM>
where
    T: Deserialize<'de>,
{
    type Value = BoundedVecV1<T, MAXIMUM>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "at most {MAXIMUM} bounded sequence elements")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let size_hint = sequence.size_hint();
        if size_hint.is_some_and(|hint| hint > MAXIMUM) {
            return Err(de::Error::invalid_length(
                size_hint.unwrap_or(MAXIMUM.saturating_add(1)),
                &self,
            ));
        }
        let mut values = Vec::with_capacity(size_hint.unwrap_or(0).min(MAXIMUM));
        while let Some(value) = sequence.next_element::<T>()? {
            if values.len() == MAXIMUM {
                return Err(de::Error::invalid_length(MAXIMUM.saturating_add(1), &self));
            }
            values.push(value);
        }
        Ok(BoundedVecV1(values))
    }
}

impl<'de, T, const MAXIMUM: usize> Deserialize<'de> for BoundedVecV1<T, MAXIMUM>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(BoundedVecV1Visitor(PhantomData))
    }
}

/// [`MAX_CLUSTER_MEMBERSHIP_READ_V1`] in the slice-length domain.
///
/// Declared from the same literal rather than narrowed with a silent cast; the
/// two are pinned together by `bounded_capacity_matches_the_wire_bound_v1`.
const CLUSTER_MEMBERS_CAPACITY_V1: usize = 4_096;

pub(crate) type BoundedClusterMembersV1 = BoundedVecV1<SymbolId, CLUSTER_MEMBERS_CAPACITY_V1>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MAX_CLUSTER_MEMBERSHIP_READ_V1;

    #[test]
    fn bounded_capacity_matches_the_wire_bound_v1() {
        assert_eq!(u32::try_from(CLUSTER_MEMBERS_CAPACITY_V1), Ok(MAX_CLUSTER_MEMBERSHIP_READ_V1));
    }

    #[test]
    fn bounded_member_decoder_rejects_the_first_over_limit_element_v1() {
        let encoded = serde_json::to_string(
            &(0..=MAX_CLUSTER_MEMBERSHIP_READ_V1)
                .map(|index| format!("symbol:{index:04}"))
                .collect::<Vec<_>>(),
        )
        .expect("oversized fixture");
        assert!(serde_json::from_str::<BoundedClusterMembersV1>(&encoded).is_err());
    }
}
