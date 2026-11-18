//! Ordered immutable edges used when copying an authenticated object closure.

use super::MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN;
use crate::index::ObjectRef;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{Error as _, SeqAccess, Visitor},
};
use std::fmt;

/// Child order participates in the immutable hash. A restored object must
/// preserve that order, including an edge repeated by its original encoding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeImmutableObjectChildrenV1(Vec<ObjectRef>);

impl NativeImmutableObjectChildrenV1 {
    pub fn new(children: Vec<ObjectRef>) -> Result<Self, &'static str> {
        if children.len() > MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN {
            return Err("immutable object child count exceeds 4096");
        }
        Ok(Self(children))
    }

    #[must_use]
    pub fn as_slice(&self) -> &[ObjectRef] {
        &self.0
    }
}

impl Serialize for NativeImmutableObjectChildrenV1 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for NativeImmutableObjectChildrenV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Children;
        struct RejectExtra;
        impl<'de> serde::de::DeserializeSeed<'de> for RejectExtra {
            type Value = ();

            fn deserialize<D: Deserializer<'de>>(self, _: D) -> Result<(), D::Error> {
                Err(D::Error::custom(
                    "immutable object child count exceeds 4096",
                ))
            }
        }
        impl<'de> Visitor<'de> for Children {
            type Value = NativeImmutableObjectChildrenV1;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("at most 4096 ordered immutable object references")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut children = Vec::new();
                if let Some(hint) = seq.size_hint() {
                    if hint > MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN {
                        return Err(A::Error::custom(
                            "immutable object child count exceeds 4096",
                        ));
                    }
                    children.try_reserve_exact(hint).map_err(A::Error::custom)?;
                }
                while children.len() < MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN {
                    let Some(child) = seq.next_element()? else {
                        return Ok(NativeImmutableObjectChildrenV1(children));
                    };
                    children.try_reserve(1).map_err(A::Error::custom)?;
                    children.push(child);
                }
                if seq.next_element_seed(RejectExtra)?.is_some() {
                    return Err(A::Error::custom(
                        "immutable object child count exceeds 4096",
                    ));
                }
                Ok(NativeImmutableObjectChildrenV1(children))
            }
        }
        deserializer.deserialize_seq(Children)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{Digest, native_immutable_object_ref_v1};

    #[test]
    fn restored_edges_preserve_order_and_repeated_children() {
        let edges = vec![
            ObjectRef(Digest([2; 32])),
            ObjectRef(Digest([1; 32])),
            ObjectRef(Digest([2; 32])),
        ];
        let children = NativeImmutableObjectChildrenV1::new(edges.clone()).expect("bounded edges");
        let encoded = serde_json::to_vec(&children).expect("encode");
        let decoded: NativeImmutableObjectChildrenV1 =
            serde_json::from_slice(&encoded).expect("decode");
        assert_eq!(decoded.as_slice(), edges);
        assert_eq!(
            native_immutable_object_ref_v1(b"original", decoded.as_slice()),
            native_immutable_object_ref_v1(b"original", &edges),
        );
        let mut sorted = edges;
        sorted.sort_unstable();
        assert_ne!(
            native_immutable_object_ref_v1(b"original", decoded.as_slice()),
            native_immutable_object_ref_v1(b"original", &sorted),
        );
    }

    #[test]
    fn restored_edges_refuse_excess_children() {
        let edges = vec![ObjectRef(Digest([1; 32])); MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN + 1];
        assert!(NativeImmutableObjectChildrenV1::new(edges.clone()).is_err());
        let encoded = serde_json::to_vec(&edges).expect("encode excess");
        assert!(serde_json::from_slice::<NativeImmutableObjectChildrenV1>(&encoded).is_err());
    }
}
