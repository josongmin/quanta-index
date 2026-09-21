//! Metavariable binding carrier and the authoritative match envelope.
//!
//! Per STR-01 §4.9, [`StructuralBinding`] is the canonical metavariable
//! carrier on the wire and closes GAP-03. Bindings are a `BTreeMap` so
//! iteration order is total (`canonical CBOR` per RFC § Migration policy).
//!
//! [`StructuralAuthorityCandidate`] is the per-match result envelope inside
//! the structural crate. Downstream adapters own projection into public
//! query-plane candidate carriers.
//!
//! D18 — hand-rolled serde; no proc-macro derives.

use core::fmt;
use std::collections::BTreeMap;

use crate::types::{ByteSpan, MetaVar};

/// Metavariable -> span map captured by one structural match.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct StructuralBinding {
    /// The captured spans, keyed by metavariable name.
    pub bindings: BTreeMap<MetaVar, ByteSpan>,
}

impl StructuralBinding {
    /// Construct an empty binding map.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            bindings: BTreeMap::new(),
        }
    }

    /// Construct from an explicit map.
    #[must_use]
    pub const fn from_map(bindings: BTreeMap<MetaVar, ByteSpan>) -> Self {
        Self { bindings }
    }

    /// `true` if no metavariables are bound.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// Number of bound metavariables.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    /// Borrow the span for `name` if present.
    #[must_use]
    pub fn get(&self, name: &MetaVar) -> Option<ByteSpan> {
        self.bindings.get(name).copied()
    }

    /// Insert / overwrite a binding. Returns the prior span if any.
    pub fn insert(&mut self, name: MetaVar, span: ByteSpan) -> Option<ByteSpan> {
        self.bindings.insert(name, span)
    }
}

impl serde::Serialize for StructuralBinding {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(self.bindings.len()))?;
        for (k, v) in &self.bindings {
            m.serialize_entry(k.as_str(), v)?;
        }
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for StructuralBinding {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = StructuralBinding;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("StructuralBinding map (metavar -> ByteSpan)")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<StructuralBinding, M::Error> {
                let mut out: BTreeMap<MetaVar, ByteSpan> = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    let mv = MetaVar::new(&key).map_err(|e| {
                        serde::de::Error::custom(format!("invalid metavar key {key:?}: {e}"))
                    })?;
                    let span: ByteSpan = map.next_value()?;
                    if out.insert(mv, span).is_some() {
                        return Err(serde::de::Error::custom(format!(
                            "duplicate metavar key in binding: {key}"
                        )));
                    }
                }
                Ok(StructuralBinding { bindings: out })
            }
        }
        de.deserialize_map(V)
    }
}

/// Per-match structural result envelope produced from authoritative parse-tree
/// + chunk-text inputs.
///
/// Deliberately omits `doc_id`; live runtime adapters own projection into
/// their public candidate carriers.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StructuralAuthorityCandidate {
    /// Byte span covered by the overall pattern match (root group span).
    pub pattern_span: ByteSpan,
    /// Captured metavariable bindings.
    pub binding: StructuralBinding,
}

impl StructuralAuthorityCandidate {
    /// Construct a [`StructuralAuthorityCandidate`].
    #[must_use]
    pub const fn new(pattern_span: ByteSpan, binding: StructuralBinding) -> Self {
        Self {
            pattern_span,
            binding,
        }
    }
}

impl serde::Serialize for StructuralAuthorityCandidate {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        m.serialize_entry("pattern_span", &self.pattern_span)?;
        m.serialize_entry("binding", &self.binding)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for StructuralAuthorityCandidate {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = StructuralAuthorityCandidate;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("StructuralAuthorityCandidate map (pattern_span, binding)")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<StructuralAuthorityCandidate, M::Error> {
                let mut pattern_span: Option<ByteSpan> = None;
                let mut binding: Option<StructuralBinding> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "pattern_span" => {
                            if pattern_span.is_some() {
                                return Err(serde::de::Error::duplicate_field("pattern_span"));
                            }
                            pattern_span = Some(map.next_value()?);
                        }
                        "binding" => {
                            if binding.is_some() {
                                return Err(serde::de::Error::duplicate_field("binding"));
                            }
                            binding = Some(map.next_value()?);
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(
                                other,
                                &["pattern_span", "binding"],
                            ));
                        }
                    }
                }
                let pattern_span =
                    pattern_span.ok_or_else(|| serde::de::Error::missing_field("pattern_span"))?;
                let binding = binding.ok_or_else(|| serde::de::Error::missing_field("binding"))?;
                Ok(StructuralAuthorityCandidate {
                    pattern_span,
                    binding,
                })
            }
        }
        de.deserialize_map(V)
    }
}

#[cfg(test)]
mod tests {
    use super::{StructuralAuthorityCandidate, StructuralBinding};
    use crate::types::{ByteSpan, MetaVar};

    fn mv(s: &str) -> MetaVar {
        match MetaVar::new(s) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                std::process::abort();
            }
        }
    }

    fn span(a: u32, b: u32) -> ByteSpan {
        match ByteSpan::new(a, b) {
            Ok(s) => s,
            Err(e) => {
                assert!(false, "{e}");
                std::process::abort();
            }
        }
    }

    #[test]
    fn empty_binding_is_empty() {
        let b = StructuralBinding::empty();
        assert!(b.is_empty());
        assert_eq!(b.len(), 0);
    }

    #[test]
    fn binding_insert_get() {
        let mut b = StructuralBinding::empty();
        let prior = b.insert(mv("X"), span(0, 3));
        assert!(prior.is_none());
        let got = b.get(&mv("X"));
        assert_eq!(got, Some(span(0, 3)));
        assert_eq!(b.len(), 1);
        assert!(!b.is_empty());
    }

    #[test]
    fn binding_serde_roundtrip() {
        let mut b = StructuralBinding::empty();
        let _prior: Option<ByteSpan> = b.insert(mv("X"), span(0, 3));
        let _prior: Option<ByteSpan> = b.insert(mv("Y"), span(4, 9));
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&b, &mut buf) {
            assert!(false, "{e}");
        }
        let got: Result<StructuralBinding, _> = ciborium::de::from_reader(buf.as_slice());
        match got {
            Ok(v) => assert_eq!(v, b),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn authority_candidate_serde_roundtrip() {
        let mut b = StructuralBinding::empty();
        let _prior: Option<ByteSpan> = b.insert(mv("X"), span(0, 3));
        let c = StructuralAuthorityCandidate::new(span(0, 10), b);
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&c, &mut buf) {
            assert!(false, "{e}");
        }
        let got: Result<StructuralAuthorityCandidate, _> =
            ciborium::de::from_reader(buf.as_slice());
        match got {
            Ok(v) => assert_eq!(v, c),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn authority_candidate_byte_identical_across_builds() {
        let mut b1 = StructuralBinding::empty();
        let _a: Option<ByteSpan> = b1.insert(mv("A"), span(0, 5));
        let _b: Option<ByteSpan> = b1.insert(mv("B"), span(6, 10));
        let mut b2 = StructuralBinding::empty();
        let _b: Option<ByteSpan> = b2.insert(mv("B"), span(6, 10));
        let _a: Option<ByteSpan> = b2.insert(mv("A"), span(0, 5));
        let c1 = StructuralAuthorityCandidate::new(span(0, 20), b1);
        let c2 = StructuralAuthorityCandidate::new(span(0, 20), b2);
        let mut buf1: Vec<u8> = Vec::new();
        let mut buf2: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&c1, &mut buf1) {
            assert!(false, "{e}");
        }
        if let Err(e) = ciborium::ser::into_writer(&c2, &mut buf2) {
            assert!(false, "{e}");
        }
        assert_eq!(buf1, buf2, "BTreeMap ordering must canonicalize output");
    }

    #[test]
    fn binding_rejects_duplicate_key_on_deserialize() {
        // Manually construct a CBOR map with the same metavar twice.
        // Use the underlying ciborium types to write a tampered map.
        use ciborium::value::Value;
        let span_val =
            Value::Array(vec![Value::Integer(0_u32.into()), Value::Integer(3_u32.into())]);
        let map = Value::Map(vec![
            (Value::Text("X".to_owned()), span_val.clone()),
            (Value::Text("X".to_owned()), span_val),
        ]);
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&map, &mut buf) {
            assert!(false, "{e}");
        }
        let got: Result<StructuralBinding, _> = ciborium::de::from_reader(buf.as_slice());
        assert!(got.is_err(), "duplicate metavar key must be rejected");
    }
}
