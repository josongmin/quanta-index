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
//! ## Idempotent upsert / delete / `from_prior`
//!
//! [`SymbolIndexBuilder`] supports replay-safe mutations driven by the
//! producer channel subscriber pattern. A symbol's **identity** is the
//! tuple `(doc_id, name, kind, span.byte_start)`; two `Symbol` values with
//! the same identity are duplicates. [`SymbolIndexBuilder::upsert_symbol`]
//! replaces by identity, [`SymbolIndexBuilder::remove_symbol`] removes by
//! identity, and [`SymbolIndexBuilder::remove_doc`] cascades removal across
//! every symbol belonging to a doc (used by `DeleteChunk` cascade).
//! [`SymbolIndexBuilder::from_prior`] seeds a fresh builder for a new
//! generation from the prior generation's finished [`SymbolIndex`].
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use std::collections::BTreeMap;
use std::io::{Read, Write};

use crate::errors::{SymbolError, SymbolErrorCode};
use crate::symbol_kind::SymbolKind;
use crate::types::{DocId, Symbol};

/// Identity tuple for a [`Symbol`]: `(doc_id, name, kind, span.byte_start)`.
///
/// Two `Symbol` values with the same identity are duplicates; producer-emitted
/// re-upsert replaces the prior copy.
type SymbolIdentity = (DocId, Box<str>, SymbolKind, u32);

fn identity_of(sym: &Symbol) -> SymbolIdentity {
    (sym.doc_id, sym.name.clone(), sym.kind, sym.span.start())
}

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
///
/// Supports idempotent upsert and delete by symbol identity
/// `(doc_id, name, kind, span.byte_start)`. The builder keeps an internal
/// identity-to-position map so duplicate upserts run in `O(log n)` and
/// removals stay consistent across the `by_name`, `by_kind`, and `by_doc`
/// posting tables that [`SymbolIndex`] rebuilds at finish-time.
pub struct SymbolIndexBuilder {
    generation: u64,
    symbols: Vec<Symbol>,
    by_identity: BTreeMap<SymbolIdentity, usize>,
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
            by_identity: BTreeMap::new(),
        })
    }

    /// Seed a fresh builder for `new_generation` from a prior finished
    /// [`SymbolIndex`].
    ///
    /// The builder's `generation` becomes `new_generation` (must be
    /// non-zero); the symbol list and identity map are cloned from
    /// `prior.symbols()` in order. The resulting builder supports the same
    /// upsert / remove operations as a fresh builder; finishing produces
    /// a [`SymbolIndex`] tagged with `new_generation`.
    pub fn from_prior(prior: &SymbolIndex, new_generation: u64) -> Result<Self, SymbolError> {
        if new_generation == 0 {
            return Err(SymbolError::new(
                SymbolErrorCode::InvalidDocument,
                "new_generation must be non-zero",
            ));
        }
        let mut by_identity: BTreeMap<SymbolIdentity, usize> = BTreeMap::new();
        let mut symbols: Vec<Symbol> = Vec::with_capacity(prior.symbols.len());
        for s in &prior.symbols {
            let id = identity_of(s);
            if by_identity.contains_key(&id) {
                // Prior index had a duplicate identity — corruption.
                return Err(SymbolError::new(
                    SymbolErrorCode::IndexCorrupted,
                    format!(
                        "prior index contained duplicate symbol identity: doc={} name={} kind={} start={}",
                        id.0, id.1, id.2, id.3,
                    ),
                ));
            }
            let pos = symbols.len();
            symbols.push(s.clone());
            let _prior = by_identity.insert(id, pos);
        }
        Ok(Self {
            generation: new_generation,
            symbols,
            by_identity,
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

    /// Add a symbol to the staged list (append-only).
    ///
    /// This is the historical append API and remains append-only for
    /// backwards compatibility with producers that pre-deduplicate. **For
    /// replay-safe pipelines use [`Self::upsert_symbol`]**, which replaces
    /// by identity rather than letting duplicates accumulate.
    pub fn add_symbol(&mut self, sym: Symbol) {
        let id = identity_of(&sym);
        let pos = self.symbols.len();
        self.symbols.push(sym);
        // Keep the identity map consistent so later upserts/removes still
        // resolve. If the caller appends two symbols with the same
        // identity, the identity map points at the most recent one; the
        // older copy stays in the symbols list (matching the append
        // contract) but is no longer reachable via identity-keyed
        // mutators. Producers that need delta safety should call
        // `upsert_symbol` instead.
        let _prior = self.by_identity.insert(id, pos);
    }

    /// Bulk-add `symbols`. Equivalent to repeated
    /// [`Self::add_symbol`].
    pub fn extend(&mut self, symbols: impl IntoIterator<Item = Symbol>) {
        for s in symbols {
            self.add_symbol(s);
        }
    }

    /// Replace any prior symbol with the same identity. If no prior symbol
    /// matches, appends. Idempotent: re-applying the same `Symbol` leaves
    /// state unchanged.
    ///
    /// Returns `Result<(), SymbolError>` to match the rest of the mutator
    /// surface (`remove_symbol`, `remove_doc`, `from_prior`). The current
    /// implementation always succeeds; reserving the failure channel keeps
    /// the API stable when future validation (e.g. identity-domain checks)
    /// lands.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "stable mutator surface — sibling methods may surface SymbolError; reserve the channel"
    )]
    pub fn upsert_symbol(&mut self, sym: Symbol) -> Result<(), SymbolError> {
        self.upsert_in_place(sym);
        Ok(())
    }

    fn upsert_in_place(&mut self, sym: Symbol) {
        let id = identity_of(&sym);
        if let Some(&pos) = self.by_identity.get(&id) {
            if let Some(slot) = self.symbols.get_mut(pos) {
                *slot = sym;
                return;
            }
            // Identity map points past the symbol list — drop the stale
            // entry and append. Self-healing because the identity map is
            // an internal authority and not a contract surface.
            let _stale = self.by_identity.remove(&id);
        }
        let pos = self.symbols.len();
        self.symbols.push(sym);
        let _prior = self.by_identity.insert(id, pos);
    }

    /// Remove the symbol matching `identity`. Returns `true` if a symbol
    /// was removed, `false` if no symbol with that identity was present.
    /// Idempotent: repeated calls with the same identity return `false`
    /// after the first successful removal.
    pub fn remove_symbol(
        &mut self,
        identity: (&DocId, &str, SymbolKind, u32),
    ) -> Result<bool, SymbolError> {
        let key: SymbolIdentity = (*identity.0, identity.1.into(), identity.2, identity.3);
        let Some(pos) = self.by_identity.remove(&key) else {
            return Ok(false);
        };
        self.remove_at(pos)?;
        Ok(true)
    }

    /// Remove every symbol belonging to `doc_id`. Returns the number of
    /// symbols removed. Idempotent: calling on an empty doc returns `0`.
    pub fn remove_doc(&mut self, doc_id: DocId) -> Result<usize, SymbolError> {
        // Collect identities first so we don't mutate while iterating the
        // identity map. Range over `(doc_id, ..)` would require a
        // synthetic upper bound — explicit collect is simpler and the
        // cost is bounded by symbol count.
        let victims: Vec<SymbolIdentity> = self
            .by_identity
            .iter()
            .filter(|(id, _)| id.0 == doc_id)
            .map(|(id, _)| id.clone())
            .collect();
        let n = victims.len();
        for id in victims {
            let Some(pos) = self.by_identity.remove(&id) else {
                continue;
            };
            self.remove_at(pos)?;
        }
        Ok(n)
    }

    fn remove_at(&mut self, pos: usize) -> Result<(), SymbolError> {
        if pos >= self.symbols.len() {
            return Err(SymbolError::new(
                SymbolErrorCode::IndexCorrupted,
                format!(
                    "remove_at out of range: pos={pos} len={}",
                    self.symbols.len()
                ),
            ));
        }
        let last = self.symbols.len().saturating_sub(1);
        if pos == last {
            let _dropped = self.symbols.pop();
            return Ok(());
        }
        // swap_remove keeps the cost O(1) but moves the tail symbol into
        // `pos`. Repoint that symbol's identity to its new position.
        let _dropped = self.symbols.swap_remove(pos);
        let moved_id = match self.symbols.get(pos) {
            Some(s) => identity_of(s),
            None => {
                return Err(SymbolError::new(
                    SymbolErrorCode::IndexCorrupted,
                    "swap_remove left empty slot",
                ));
            }
        };
        let _prior = self.by_identity.insert(moved_id, pos);
        Ok(())
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

    fn sym_at(name: &str, kind: SymbolKind, doc: u64, start: u32, end: u32) -> Symbol {
        let Ok(span) = ByteSpan::new(start, end) else {
            std::process::abort();
        };
        Symbol::new(name, kind, DocId(doc), span, LangId::Rust, None)
    }

    fn fixture() -> SymbolIndex {
        let Ok(mut b) = SymbolIndexBuilder::new(1) else {
            std::process::abort();
        };
        b.add_symbol(sym_at("foo", SymbolKind::Function, 1, 0, 3));
        b.add_symbol(sym_at("bar", SymbolKind::Function, 1, 10, 13));
        b.add_symbol(sym_at("Baz", SymbolKind::Struct, 2, 0, 3));
        b.add_symbol(sym_at("foo", SymbolKind::Method, 3, 0, 3));
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

    // ── delta-handling: upsert / remove / from_prior ──────────────────

    #[test]
    fn upsert_replaces_by_identity() {
        let Ok(mut b) = SymbolIndexBuilder::new(1) else {
            std::process::abort();
        };
        // Insert a Symbol, then re-upsert one with the same identity but a
        // longer span. The replacement wins; the identity map keeps len=1.
        let Ok(span_a) = ByteSpan::new(0, 3) else {
            std::process::abort();
        };
        let Ok(span_b) = ByteSpan::new(0, 9) else {
            std::process::abort();
        };
        let s_a = Symbol::new(
            "foo",
            SymbolKind::Function,
            DocId(1),
            span_a,
            LangId::Rust,
            None,
        );
        let s_b = Symbol::new(
            "foo",
            SymbolKind::Function,
            DocId(1),
            span_b,
            LangId::Rust,
            None,
        );
        if let Err(e) = b.upsert_symbol(s_a) {
            assert!(false, "{e}");
            return;
        }
        if let Err(e) = b.upsert_symbol(s_b) {
            assert!(false, "{e}");
            return;
        }
        assert_eq!(b.len(), 1, "upsert must not grow when identity matches");

        // Confirm the replacement (different content) wins.
        let Ok(idx) = b.finish() else {
            std::process::abort();
        };
        let hits = idx.lookup_by_name("foo");
        assert_eq!(hits.len(), 1);
        let Some(got) = hits.first() else {
            assert!(false, "no hit");
            return;
        };
        assert_eq!(got.span, span_b);
    }

    #[test]
    fn upsert_dup_no_growth() {
        let Ok(mut b) = SymbolIndexBuilder::new(1) else {
            std::process::abort();
        };
        let s = sym("foo", SymbolKind::Function, 1);
        for _ in 0..5 {
            if let Err(e) = b.upsert_symbol(s.clone()) {
                assert!(false, "{e}");
                return;
            }
        }
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn upsert_different_identity_appends() {
        let Ok(mut b) = SymbolIndexBuilder::new(1) else {
            std::process::abort();
        };
        if let Err(e) = b.upsert_symbol(sym_at("foo", SymbolKind::Function, 1, 0, 3)) {
            assert!(false, "{e}");
            return;
        }
        if let Err(e) = b.upsert_symbol(sym_at("foo", SymbolKind::Function, 1, 10, 13)) {
            assert!(false, "{e}");
            return;
        }
        assert_eq!(b.len(), 2);
    }

    #[test]
    fn remove_symbol_idempotent() {
        let Ok(mut b) = SymbolIndexBuilder::new(1) else {
            std::process::abort();
        };
        b.add_symbol(sym_at("foo", SymbolKind::Function, 1, 0, 3));
        b.add_symbol(sym_at("bar", SymbolKind::Function, 2, 5, 8));
        let id = (&DocId(1), "foo", SymbolKind::Function, 0u32);
        match b.remove_symbol(id) {
            Ok(true) => {}
            Ok(false) => {
                assert!(false, "first remove must return true");
                return;
            }
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        }
        // Idempotency: repeat returns false.
        match b.remove_symbol(id) {
            Ok(false) => {}
            Ok(true) => {
                assert!(false, "second remove must return false");
                return;
            }
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        }
        assert_eq!(b.len(), 1);
        let Ok(idx) = b.finish() else {
            std::process::abort();
        };
        assert!(idx.lookup_by_name("foo").is_empty());
        assert_eq!(idx.lookup_by_name("bar").len(), 1);
    }

    #[test]
    fn remove_doc_cascades_to_all_indices() {
        let Ok(mut b) = SymbolIndexBuilder::new(1) else {
            std::process::abort();
        };
        b.add_symbol(sym_at("foo", SymbolKind::Function, 1, 0, 3));
        b.add_symbol(sym_at("bar", SymbolKind::Method, 1, 10, 13));
        b.add_symbol(sym_at("Baz", SymbolKind::Struct, 1, 20, 23));
        b.add_symbol(sym_at("survivor", SymbolKind::Function, 2, 0, 8));
        let n = match b.remove_doc(DocId(1)) {
            Ok(n) => n,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(n, 3);
        let Ok(idx) = b.finish() else {
            std::process::abort();
        };
        // by_name: only "survivor" should resolve.
        assert!(idx.lookup_by_name("foo").is_empty());
        assert!(idx.lookup_by_name("bar").is_empty());
        assert!(idx.lookup_by_name("Baz").is_empty());
        assert_eq!(idx.lookup_by_name("survivor").len(), 1);
        // by_kind: Method and Struct should be gone, Function only has survivor.
        assert_eq!(idx.lookup_by_kind(SymbolKind::Function).len(), 1);
        assert!(idx.lookup_by_kind(SymbolKind::Method).is_empty());
        assert!(idx.lookup_by_kind(SymbolKind::Struct).is_empty());
        // by_doc: doc 1 has nothing, doc 2 has 1.
        assert!(idx.lookup_by_doc(DocId(1)).is_empty());
        assert_eq!(idx.lookup_by_doc(DocId(2)).len(), 1);
    }

    #[test]
    fn remove_doc_idempotent_on_unknown_doc() {
        let Ok(mut b) = SymbolIndexBuilder::new(1) else {
            std::process::abort();
        };
        b.add_symbol(sym_at("foo", SymbolKind::Function, 1, 0, 3));
        let n = match b.remove_doc(DocId(99)) {
            Ok(n) => n,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(n, 0);
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn from_prior_preserves_state() {
        let i = fixture();
        let g1 = i.generation();
        let total = i.len();
        let new_gen = g1.saturating_add(1);
        let Ok(b) = SymbolIndexBuilder::from_prior(&i, new_gen) else {
            std::process::abort();
        };
        assert_eq!(b.generation(), new_gen);
        assert_eq!(b.len(), total);
        let Ok(rebuilt) = b.finish() else {
            std::process::abort();
        };
        assert_eq!(rebuilt.generation(), new_gen);
        assert_eq!(rebuilt.len(), total);
        // Every lookup the original satisfies, the rebuilt index satisfies.
        for s in i.symbols() {
            let hits = rebuilt.lookup_by_name(s.name.as_ref());
            assert!(hits.contains(&s));
        }
    }

    #[test]
    fn from_prior_then_remove_drops_only_target() {
        let i = fixture();
        let total = i.len();
        let Ok(mut b) = SymbolIndexBuilder::from_prior(&i, 2) else {
            std::process::abort();
        };
        // Drop the (doc=2, "Baz", Struct, 0) symbol; that's exactly 1.
        let id = (&DocId(2), "Baz", SymbolKind::Struct, 0u32);
        match b.remove_symbol(id) {
            Ok(true) => {}
            Ok(false) => {
                assert!(false, "must have removed");
                return;
            }
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        }
        assert_eq!(b.len(), total.saturating_sub(1));
        let Ok(idx) = b.finish() else {
            std::process::abort();
        };
        assert!(idx.lookup_by_name("Baz").is_empty());
        // Unrelated symbols survive.
        assert!(!idx.lookup_by_name("foo").is_empty());
        assert!(!idx.lookup_by_name("bar").is_empty());
    }

    #[test]
    fn from_prior_rejects_zero_new_generation() {
        let i = fixture();
        match SymbolIndexBuilder::from_prior(&i, 0) {
            Ok(_) => assert!(false, "must reject new_generation=0"),
            Err(e) => assert_eq!(e.code, SymbolErrorCode::InvalidDocument),
        }
    }
}
