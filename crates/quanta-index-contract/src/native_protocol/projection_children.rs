//! Bounded, canonical direct children for one immutable projection output.

use super::MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN;
use crate::index::ObjectRef;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{Error as _, SeqAccess, Visitor},
};
use std::fmt;

/// The wire remains one contiguous array; callers cannot bypass the decoder's
/// child bound or canonical order by constructing the field directly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeProjectionOutputChildren(Vec<ObjectRef>);

impl NativeProjectionOutputChildren {
    pub fn new(children: Vec<ObjectRef>) -> Result<Self, &'static str> {
        if children.len() > MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN {
            return Err("projection output child count exceeds 4096");
        }
        if children
            .windows(2)
            .any(|pair| matches!(pair, [left, right] if left >= right))
        {
            return Err("projection output children must be sorted and unique");
        }
        Ok(Self(children))
    }

    #[must_use]
    pub fn as_slice(&self) -> &[ObjectRef] {
        &self.0
    }
}

impl Serialize for NativeProjectionOutputChildren {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

struct RejectExtra;

impl<'de> serde::de::DeserializeSeed<'de> for RejectExtra {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, _deserializer: D) -> Result<Self::Value, D::Error> {
        Err(D::Error::custom(
            "projection output child count exceeds 4096",
        ))
    }
}

impl<'de> Deserialize<'de> for NativeProjectionOutputChildren {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Children;

        impl<'de> Visitor<'de> for Children {
            type Value = NativeProjectionOutputChildren;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("at most 4096 sorted unique projection output children")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                if seq
                    .size_hint()
                    .is_some_and(|hint| hint > MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN)
                {
                    return Err(A::Error::custom(
                        "projection output declared child count exceeds 4096",
                    ));
                }
                let mut children = Vec::new();
                if let Some(hint) = seq.size_hint() {
                    children.try_reserve_exact(hint).map_err(A::Error::custom)?;
                }
                let mut previous = None;
                loop {
                    if children.len() == MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN {
                        if seq.next_element_seed(RejectExtra)?.is_some() {
                            return Err(A::Error::custom(
                                "projection output child count exceeds 4096",
                            ));
                        }
                        break;
                    }
                    let Some(child) = seq.next_element::<ObjectRef>()? else {
                        break;
                    };
                    if previous.is_some_and(|last| last >= child) {
                        return Err(A::Error::custom(
                            "projection output children must be sorted and unique",
                        ));
                    }
                    children.try_reserve(1).map_err(A::Error::custom)?;
                    children.push(child);
                    previous = Some(child);
                }
                Ok(NativeProjectionOutputChildren(children))
            }
        }

        deserializer.deserialize_seq(Children)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Digest;

    fn reference(value: u64) -> ObjectRef {
        let mut digest = [0_u8; 32];
        for (slot, byte) in digest.iter_mut().zip(value.to_be_bytes()) {
            *slot = byte;
        }
        ObjectRef(Digest(digest))
    }

    #[test]
    fn canonical_wire_roundtrip_and_order_refusals() -> Result<(), Box<dyn std::error::Error>> {
        let expected = [reference(1), reference(2)];
        let [first, second] = expected;
        let children = NativeProjectionOutputChildren::new(expected.to_vec())?;
        let wire = serde_json::to_vec(&children)?;
        if wire != serde_json::to_vec(&expected)?
            || serde_json::from_slice::<NativeProjectionOutputChildren>(&wire)? != children
            || children.as_slice() != expected
            || NativeProjectionOutputChildren::new(vec![second, first]).is_ok()
            || NativeProjectionOutputChildren::new(vec![first, first]).is_ok()
        {
            return Err("projection child canonical wire or constructor changed".into());
        }
        for invalid in [[second, first], [first, first]] {
            if serde_json::from_slice::<NativeProjectionOutputChildren>(&serde_json::to_vec(
                &invalid,
            )?)
            .is_ok()
            {
                return Err("projection child decoder accepted bad order or duplicate".into());
            }
        }
        Ok(())
    }

    struct DeclaredOversize;

    impl<'de> serde::de::SeqAccess<'de> for DeclaredOversize {
        type Error = serde::de::value::Error;

        fn next_element_seed<T: serde::de::DeserializeSeed<'de>>(
            &mut self,
            _seed: T,
        ) -> Result<Option<T::Value>, Self::Error> {
            Err(Self::Error::custom(
                "element decoded before declared-size refusal",
            ))
        }

        fn size_hint(&self) -> Option<usize> {
            Some(MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN.saturating_add(1))
        }
    }

    #[test]
    fn declared_size_and_unknown_length_refuse_before_extra_child_decode()
    -> Result<(), Box<dyn std::error::Error>> {
        let declared = NativeProjectionOutputChildren::deserialize(
            serde::de::value::SeqAccessDeserializer::new(DeclaredOversize),
        );
        let Err(declared) = declared else {
            return Err("declared oversize projection child list was accepted".into());
        };
        if !declared
            .to_string()
            .contains("declared child count exceeds 4096")
        {
            return Err("declared-size refusal decoded an element".into());
        }

        let mut overbound = Vec::new();
        overbound.try_reserve_exact(MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN.saturating_add(1))?;
        for ordinal in 0..=MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN {
            overbound.push(reference(u64::try_from(ordinal)?));
        }
        if NativeProjectionOutputChildren::new(overbound.clone()).is_ok() {
            return Err("constructor accepted oversized child list".into());
        }
        let wire = serde_json::to_vec(&overbound)?;
        let Err(decoded) = serde_json::from_slice::<NativeProjectionOutputChildren>(&wire) else {
            return Err("unknown-length oversized child list was accepted".into());
        };
        if !decoded.to_string().contains("child count exceeds 4096") {
            return Err("unknown-length decoder passed the child bound".into());
        }
        Ok(())
    }
}
