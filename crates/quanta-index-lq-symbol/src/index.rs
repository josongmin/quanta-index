//! Canonical per-generation symbol index — serialized artifact.
//!
//! [`SymbolIndex`] holds every captured [`Symbol`] for a generation along
//! with three lookup tables: by name, by kind, and by document. Query-side
//! consumers (Tantivy bridge, planner pushdown) consult this artifact
//! directly; rebuilding posting lists on the fly is forbidden.
//!
//! Determinism: `BTreeMap` keys yield sorted iteration order; CBOR encoding
//! is byte-identical across runs with the same insertion sequence.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use std::collections::BTreeMap;
use std::io::{Read, Write};

use crate::errors::{SymbolError, SymbolErrorCode};
use crate::symbol_kind::SymbolKind;
use crate::types::{DocId, Symbol};

/// Authoritative per-generation symbol-index state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolIndex {
    generation: u64,
    symbols: Vec<Symbol>,
    by_name: BTreeMap<Box<str>, Vec<u32>>,
    by_kind: BTreeMap<SymbolKind, Vec<u32>>,
    by_doc: BTreeMap<DocId, Vec<u32>>,
}

impl SymbolIndex {
    /// Generation id pinned at build time.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Total number of symbols in the index.
    #[must_use]
    pub fn len(&self) -> usize {
        self.symbols.len()
    }

    /// `true` if the index has zero symbols.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }

    /// All symbols in insertion order.
    #[must_use]
    pub fn symbols(&self) -> &[Symbol] {
        &self.symbols
    }

    /// Lookup by exact symbol name.
    ///
    /// Returns an owned vector of references into [`Self::symbols`]; the
    /// vector is empty when the name is absent.
    #[must_use]
    pub fn lookup_by_name(&self, name: &str) -> Vec<&Symbol> {
        self.by_name
            .get(name)
            .map_or_else(Vec::new, |ixs| self.resolve_indices(ixs))
    }

    /// Lookup by symbol kind.
    #[must_use]
    pub fn lookup_by_kind(&self, kind: SymbolKind) -> Vec<&Symbol> {
        self.by_kind
            .get(&kind)
            .map_or_else(Vec::new, |ixs| self.resolve_indices(ixs))
    }

    /// Lookup by source document.
    #[must_use]
    pub fn lookup_by_doc(&self, doc_id: DocId) -> Vec<&Symbol> {
        self.by_doc
            .get(&doc_id)
            .map_or_else(Vec::new, |ixs| self.resolve_indices(ixs))
    }

    fn resolve_indices(&self, ixs: &[u32]) -> Vec<&Symbol> {
        let mut out: Vec<&Symbol> = Vec::with_capacity(ixs.len());
        for i in ixs {
            let u = match usize::try_from(*i) {
                Ok(v) => v,
                Err(_e) => continue,
            };
            if let Some(s) = self.symbols.get(u) {
                out.push(s);
            }
        }
        out
    }

    /// Serialize as CBOR.
    pub fn serialize_cbor<W: Write>(&self, writer: W) -> Result<(), SymbolError> {
        ciborium::ser::into_writer(self, writer).map_err(|e| {
            SymbolError::new(
                SymbolErrorCode::IndexDeserialize,
                format!("CBOR encode failed: {e}"),
            )
        })
    }

    /// Inverse of [`Self::serialize_cbor`]. Rebuilds the in-memory
    /// posting tables from the symbol list.
    pub fn deserialize_cbor<R: Read>(reader: R) -> Result<Self, SymbolError> {
        ciborium::de::from_reader(reader).map_err(|e| {
            SymbolError::new(
                SymbolErrorCode::IndexDeserialize,
                format!("CBOR decode failed: {e}"),
            )
        })
    }

    fn rebuild_indices(&mut self) -> Result<(), SymbolError> {
        let mut by_name: BTreeMap<Box<str>, Vec<u32>> = BTreeMap::new();
        let mut by_kind: BTreeMap<SymbolKind, Vec<u32>> = BTreeMap::new();
        let mut by_doc: BTreeMap<DocId, Vec<u32>> = BTreeMap::new();
        for (i, s) in self.symbols.iter().enumerate() {
            let idx32 = u32::try_from(i).map_err(|e| {
                SymbolError::new(
                    SymbolErrorCode::IndexCorrupted,
                    format!("symbol index overflows u32: {e}"),
                )
            })?;
            by_name.entry(s.name.clone()).or_default().push(idx32);
            by_kind.entry(s.kind).or_default().push(idx32);
            by_doc.entry(s.doc_id).or_default().push(idx32);
        }
        self.by_name = by_name;
        self.by_kind = by_kind;
        self.by_doc = by_doc;
        Ok(())
    }
}

impl serde::Serialize for SymbolIndex {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        // Only persist generation + symbols; indices rebuild on load.
        let mut m = ser.serialize_map(Some(2))?;
        m.serialize_entry("generation", &self.generation)?;
        m.serialize_entry("symbols", &SymbolSeq(&self.symbols))?;
        m.end()
    }
}

struct SymbolSeq<'a>(&'a [Symbol]);

impl serde::Serialize for SymbolSeq<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq as _;
        let mut s = ser.serialize_seq(Some(self.0.len()))?;
        for sym in self.0 {
            s.serialize_element(sym)?;
        }
        s.end()
    }
}

impl<'de> serde::Deserialize<'de> for SymbolIndex {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = SymbolIndex;
            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("SymbolIndex map with fields generation, symbols")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<SymbolIndex, M::Error> {
                let mut generation: Option<u64> = None;
                let mut symbols: Option<Vec<Symbol>> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "generation" => {
                            if generation.is_some() {
                                return Err(serde::de::Error::duplicate_field("generation"));
                            }
                            generation = Some(map.next_value()?);
                        }
                        "symbols" => {
                            if symbols.is_some() {
                                return Err(serde::de::Error::duplicate_field("symbols"));
                            }
                            symbols = Some(map.next_value()?);
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(
                                other,
                                &["generation", "symbols"],
                            ));
                        }
                    }
                }
                let g = generation.ok_or_else(|| serde::de::Error::missing_field("generation"))?;
                if g == 0 {
                    return Err(serde::de::Error::custom("generation must be non-zero"));
                }
                let syms = symbols.ok_or_else(|| serde::de::Error::missing_field("symbols"))?;
                let mut idx = SymbolIndex {
                    generation: g,
                    symbols: syms,
                    by_name: BTreeMap::new(),
                    by_kind: BTreeMap::new(),
                    by_doc: BTreeMap::new(),
                };
                if let Err(e) = idx.rebuild_indices() {
                    return Err(serde::de::Error::custom(format!(
                        "rebuild_indices failed: {e}"
                    )));
                }
                Ok(idx)
            }
        }
        de.deserialize_map(V)
    }
}

/// Deterministic per-generation symbol-index builder.
pub struct SymbolIndexBuilder {
    generation: u64,
    symbols: Vec<Symbol>,
}

impl SymbolIndexBuilder {
    /// Construct a fresh builder for `generation`.
    ///
    /// Returns [`SymbolErrorCode::InvalidDocument`] for `generation == 0`
    /// (mirrors the trigram/positions builders' zero-generation rule).
    pub fn new(generation: u64) -> Result<Self, SymbolError> {
        if generation == 0 {
            return Err(SymbolError::new(
                SymbolErrorCode::InvalidDocument,
                "generation must be non-zero",
            ));
        }
        Ok(Self {
            generation,
            symbols: Vec::new(),
        })
    }

    /// Generation id of this builder.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Number of symbols staged so far.
    #[must_use]
    pub fn len(&self) -> usize {
        self.symbols.len()
    }

    /// `true` if no symbols have been added.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }

    /// Add a symbol to the staged list. Caller is responsible for
    /// providing a valid `Symbol` (the public constructor enforces
    /// `ByteSpan` invariants).
    pub fn add_symbol(&mut self, sym: Symbol) {
        self.symbols.push(sym);
    }

    /// Bulk-add `symbols`. Equivalent to repeated
    /// [`Self::add_symbol`].
    pub fn extend(&mut self, symbols: impl IntoIterator<Item = Symbol>) {
        for s in symbols {
            self.symbols.push(s);
        }
    }

    /// Finalise the builder into a [`SymbolIndex`]. Rebuilds the per-name
    /// / per-kind / per-doc posting tables from the staged symbol list.
    pub fn finish(self) -> Result<SymbolIndex, SymbolError> {
        let mut idx = SymbolIndex {
            generation: self.generation,
            symbols: self.symbols,
            by_name: BTreeMap::new(),
            by_kind: BTreeMap::new(),
            by_doc: BTreeMap::new(),
        };
        idx.rebuild_indices()?;
        Ok(idx)
    }
}

#[cfg(test)]
mod tests {
    use super::{SymbolIndex, SymbolIndexBuilder};
    use crate::errors::SymbolErrorCode;
    use crate::symbol_kind::SymbolKind;
    use crate::types::{ByteSpan, DocId, LangId, Symbol};

    fn sym(name: &str, kind: SymbolKind, doc: u64) -> Symbol {
        let Ok(span) = ByteSpan::new(0, 3) else {
            std::process::abort();
        };
        Symbol::new(name, kind, DocId(doc), span, LangId::Rust, None)
    }

    fn fixture() -> SymbolIndex {
        let Ok(mut b) = SymbolIndexBuilder::new(1) else {
            std::process::abort();
        };
        b.add_symbol(sym("foo", SymbolKind::Function, 1));
        b.add_symbol(sym("bar", SymbolKind::Function, 1));
        b.add_symbol(sym("Baz", SymbolKind::Struct, 2));
        b.add_symbol(sym("foo", SymbolKind::Method, 3));
        let Ok(i) = b.finish() else {
            std::process::abort();
        };
        i
    }

    #[test]
    fn builder_rejects_zero_generation() {
        match SymbolIndexBuilder::new(0) {
            Ok(_) => assert!(false, "must reject generation=0"),
            Err(e) => assert_eq!(e.code, SymbolErrorCode::InvalidDocument),
        }
    }

    #[test]
    fn empty_builder_finishes_to_empty_index() {
        let Ok(b) = SymbolIndexBuilder::new(1) else {
            std::process::abort();
        };
        assert!(b.is_empty());
        let Ok(i) = b.finish() else {
            std::process::abort();
        };
        assert_eq!(i.generation(), 1);
        assert!(i.is_empty());
        assert_eq!(i.len(), 0);
    }

    #[test]
    fn lookup_by_name_returns_all_matches() {
        let i = fixture();
        let foos = i.lookup_by_name("foo");
        assert_eq!(foos.len(), 2);
        assert!(foos.iter().any(|s| s.kind == SymbolKind::Function));
        assert!(foos.iter().any(|s| s.kind == SymbolKind::Method));
    }

    #[test]
    fn lookup_by_name_missing_returns_empty() {
        let i = fixture();
        assert!(i.lookup_by_name("nope").is_empty());
    }

    #[test]
    fn lookup_by_kind_groups_correctly() {
        let i = fixture();
        let fns = i.lookup_by_kind(SymbolKind::Function);
        assert_eq!(fns.len(), 2);
        let structs = i.lookup_by_kind(SymbolKind::Struct);
        assert_eq!(structs.len(), 1);
        let traits = i.lookup_by_kind(SymbolKind::Trait);
        assert!(traits.is_empty());
    }

    #[test]
    fn lookup_by_doc_groups_correctly() {
        let i = fixture();
        let d1 = i.lookup_by_doc(DocId(1));
        assert_eq!(d1.len(), 2);
        let d2 = i.lookup_by_doc(DocId(2));
        assert_eq!(d2.len(), 1);
        let d99 = i.lookup_by_doc(DocId(99));
        assert!(d99.is_empty());
    }

    #[test]
    fn cbor_roundtrip_preserves_value() {
        let i = fixture();
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = i.serialize_cbor(&mut buf) {
            assert!(false, "{e}");
            return;
        }
        match SymbolIndex::deserialize_cbor(buf.as_slice()) {
            Ok(got) => assert_eq!(got, i),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn cbor_encoding_is_byte_identical_across_runs() {
        let i1 = fixture();
        let i2 = fixture();
        let mut b1: Vec<u8> = Vec::new();
        let mut b2: Vec<u8> = Vec::new();
        if let Err(e) = i1.serialize_cbor(&mut b1) {
            assert!(false, "{e}");
            return;
        }
        if let Err(e) = i2.serialize_cbor(&mut b2) {
            assert!(false, "{e}");
            return;
        }
        assert_eq!(b1, b2);
    }

    #[test]
    fn deserialize_rejects_zero_generation() {
        // Construct a payload by serializing a hand-crafted struct that
        // would yield generation=0 if accepted.
        struct ZeroGen;
        impl serde::Serialize for ZeroGen {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeMap as _;
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("generation", &0u64)?;
                m.serialize_entry::<str, [u8; 0]>("symbols", &[])?;
                m.end()
            }
        }
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&ZeroGen, &mut buf) {
            assert!(false, "{e}");
            return;
        }
        match SymbolIndex::deserialize_cbor(buf.as_slice()) {
            Ok(_) => assert!(false, "must reject generation=0"),
            Err(e) => assert_eq!(e.code, SymbolErrorCode::IndexDeserialize),
        }
    }

    #[test]
    fn deserialize_rejects_truncated() {
        let i = fixture();
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = i.serialize_cbor(&mut buf) {
            assert!(false, "{e}");
            return;
        }
        let last = buf.len().saturating_sub(3);
        let Some(t) = buf.get(..last) else {
            assert!(false, "slice");
            return;
        };
        match SymbolIndex::deserialize_cbor(t) {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, SymbolErrorCode::IndexDeserialize),
        }
    }
}
