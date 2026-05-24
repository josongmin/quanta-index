//! Canonical per-generation semantic vector index — serialized artifact.
//!
//! [`SemanticIndex`] holds every captured `(DocId, embedding)` pair for a
//! generation along with a sorted [`std::collections::BTreeMap`] keyed by
//! [`DocId`]. Query-side consumers ([`crate::query`]) iterate the corpus
//! in sorted [`DocId`] order so ties at the cosine score break
//! deterministically.
//!
//! Determinism: same insertion sequence yields a byte-identical CBOR
//! encoding because the persisted form sorts on [`DocId`] (the `BTreeMap`
//! iteration order).
//!
//! CBOR shape — top level is a map with three required keys:
//!
//! ```text
//! { "generation": u64, "dim": u32, "by_doc": [ [u64, [f32, f32, ...]], ... ] }
//! ```
//!
//! The `by_doc` value is a sequence of two-element tuples to preserve
//! sorted [`DocId`] order on the wire (CBOR maps are unordered by spec;
//! a sequence-of-tuples is the canonical ordered serialization). Each
//! inner vector serializes as a CBOR array of `f32` floats — ciborium
//! emits `f32` half/single-precision tags so the encoding is byte-stable
//! across compilers.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use std::collections::BTreeMap;
use std::io::{Read, Write};

use crate::errors::{LimitDimension, SemanticError, SemanticErrorCode};
use crate::hnsw::HnswParams;
use crate::types::{DocId, Embedding, MAX_EMBEDDING_DIM};

/// Authoritative per-generation semantic-vector index.
///
/// Two indices that share `(generation, dim, by_doc, hnsw_params)`
/// (in `BTreeMap` canonical iteration order) compare equal and
/// serialize byte-identical.
///
/// `hnsw_params` is the HNSW backend configuration the executor uses
/// when corpus size exceeds [`crate::types::EXACT_NN_CUTOFF`]. When
/// absent (`None`) and the corpus crosses the cutoff, the executor
/// falls back to [`HnswParams::DEFAULTS`] unless the caller opted out
/// of HNSW via the options entry point.
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticIndex {
    generation: u64,
    dim: u32,
    by_doc: BTreeMap<DocId, Vec<f32>>,
    hnsw_params: Option<HnswParams>,
}

impl SemanticIndex {
    /// Generation id pinned at build time.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Per-generation embedding dimension. Every corpus vector and every
    /// query vector must match this dimension.
    #[must_use]
    pub const fn dim(&self) -> u32 {
        self.dim
    }

    /// Number of documents in the corpus.
    #[must_use]
    pub fn corpus_size(&self) -> usize {
        self.by_doc.len()
    }

    /// `true` if the corpus has zero documents.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_doc.is_empty()
    }

    /// Borrow the corpus as a sorted (by [`DocId`]) iterator over
    /// `(DocId, &[f32])` pairs. The iteration order matches the wire
    /// order used by the executor; tied scores therefore break by
    /// ascending [`DocId`].
    pub fn iter(&self) -> impl Iterator<Item = (DocId, &[f32])> + '_ {
        self.by_doc.iter().map(|(d, v)| (*d, v.as_slice()))
    }

    /// Borrow the embedding for `doc_id`, if present.
    #[must_use]
    pub fn get(&self, doc_id: DocId) -> Option<&[f32]> {
        self.by_doc.get(&doc_id).map(Vec::as_slice)
    }

    /// Borrow the persisted HNSW configuration, if attached.
    #[must_use]
    pub const fn hnsw_params(&self) -> Option<&HnswParams> {
        self.hnsw_params.as_ref()
    }

    /// Builder-style: attach a validated [`HnswParams`] to this
    /// index. The params are persisted alongside the corpus so the
    /// executor can spin up an HNSW backend deterministically.
    /// Fails closed on invalid params.
    pub fn with_hnsw_params(mut self, params: HnswParams) -> Result<Self, SemanticError> {
        params.validate()?;
        self.hnsw_params = Some(params);
        Ok(self)
    }

    /// Builder-style: clear any attached HNSW params.
    #[must_use]
    pub fn without_hnsw_params(mut self) -> Self {
        self.hnsw_params = None;
        self
    }

    /// Serialize as canonical CBOR. See module-level docs for the wire
    /// shape. Failures route through [`SemanticErrorCode::IndexDeserialize`]
    /// (the encode side of the same wire-shape contract).
    pub fn serialize_cbor<W: Write>(&self, writer: W) -> Result<(), SemanticError> {
        ciborium::ser::into_writer(self, writer).map_err(|e| {
            SemanticError::new(
                SemanticErrorCode::IndexDeserialize,
                format!("CBOR encode failed: {e}"),
            )
        })
    }

    /// Inverse of [`Self::serialize_cbor`]. Validates the dim invariant
    /// against each persisted vector and returns
    /// [`SemanticErrorCode::IndexCorrupted`] on mismatch.
    pub fn deserialize_cbor<R: Read>(reader: R) -> Result<Self, SemanticError> {
        ciborium::de::from_reader(reader).map_err(|e| {
            SemanticError::new(
                SemanticErrorCode::IndexDeserialize,
                format!("CBOR decode failed: {e}"),
            )
        })
    }
}

// ─────────────────────────── Manual serde ───────────────────────────

impl serde::Serialize for SemanticIndex {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let n = if self.hnsw_params.is_some() { 4 } else { 3 };
        let mut m = ser.serialize_map(Some(n))?;
        m.serialize_entry("generation", &self.generation)?;
        m.serialize_entry("dim", &self.dim)?;
        m.serialize_entry("by_doc", &ByDocSeq(&self.by_doc))?;
        if let Some(p) = self.hnsw_params.as_ref() {
            m.serialize_entry("hnsw_params", p)?;
        }
        m.end()
    }
}

struct ByDocSeq<'a>(&'a BTreeMap<DocId, Vec<f32>>);

impl serde::Serialize for ByDocSeq<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq as _;
        let mut s = ser.serialize_seq(Some(self.0.len()))?;
        for (doc, vec) in self.0 {
            s.serialize_element(&ByDocEntry(*doc, vec))?;
        }
        s.end()
    }
}

struct ByDocEntry<'a>(DocId, &'a Vec<f32>);

impl serde::Serialize for ByDocEntry<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeTuple as _;
        let mut t = ser.serialize_tuple(2)?;
        t.serialize_element(&self.0)?;
        t.serialize_element(&F32Seq(self.1))?;
        t.end()
    }
}

struct F32Seq<'a>(&'a [f32]);

impl serde::Serialize for F32Seq<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq as _;
        let mut s = ser.serialize_seq(Some(self.0.len()))?;
        for f in self.0 {
            s.serialize_element(f)?;
        }
        s.end()
    }
}

impl<'de> serde::Deserialize<'de> for SemanticIndex {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = SemanticIndex;
            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("SemanticIndex map (generation, dim, by_doc)")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<SemanticIndex, M::Error> {
                let mut generation: Option<u64> = None;
                let mut dim: Option<u32> = None;
                let mut by_doc_seq: Option<Vec<(DocId, Vec<f32>)>> = None;
                let mut hnsw_params: Option<HnswParams> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "generation" => {
                            if generation.is_some() {
                                return Err(serde::de::Error::duplicate_field("generation"));
                            }
                            generation = Some(map.next_value()?);
                        }
                        "dim" => {
                            if dim.is_some() {
                                return Err(serde::de::Error::duplicate_field("dim"));
                            }
                            dim = Some(map.next_value()?);
                        }
                        "by_doc" => {
                            if by_doc_seq.is_some() {
                                return Err(serde::de::Error::duplicate_field("by_doc"));
                            }
                            by_doc_seq = Some(map.next_value()?);
                        }
                        "hnsw_params" => {
                            if hnsw_params.is_some() {
                                return Err(serde::de::Error::duplicate_field("hnsw_params"));
                            }
                            hnsw_params = Some(map.next_value()?);
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(
                                other,
                                &["generation", "dim", "by_doc", "hnsw_params"],
                            ));
                        }
                    }
                }
                let generation =
                    generation.ok_or_else(|| serde::de::Error::missing_field("generation"))?;
                if generation == 0 {
                    return Err(serde::de::Error::custom("generation must be non-zero"));
                }
                let dim = dim.ok_or_else(|| serde::de::Error::missing_field("dim"))?;
                if dim == 0 {
                    return Err(serde::de::Error::custom("dim must be non-zero"));
                }
                let max = u32::try_from(MAX_EMBEDDING_DIM).map_err(|e| {
                    serde::de::Error::custom(format!("MAX_EMBEDDING_DIM cast: {e}"))
                })?;
                if dim > max {
                    return Err(serde::de::Error::custom(format!(
                        "dim {dim} exceeds MAX_EMBEDDING_DIM {max}"
                    )));
                }
                let entries =
                    by_doc_seq.ok_or_else(|| serde::de::Error::missing_field("by_doc"))?;
                let dim_usize = usize::try_from(dim)
                    .map_err(|e| serde::de::Error::custom(format!("dim cast: {e}")))?;
                let mut by_doc: BTreeMap<DocId, Vec<f32>> = BTreeMap::new();
                for (id, v) in entries {
                    if v.len() != dim_usize {
                        return Err(serde::de::Error::custom(format!(
                            "doc {id} has dim {} but index dim is {dim}",
                            v.len()
                        )));
                    }
                    for (i, f) in v.iter().enumerate() {
                        if !f.is_finite() {
                            return Err(serde::de::Error::custom(format!(
                                "doc {id} component {i} non-finite: {f}"
                            )));
                        }
                    }
                    if by_doc.insert(id, v).is_some() {
                        return Err(serde::de::Error::custom(format!(
                            "duplicate doc {id} in by_doc sequence"
                        )));
                    }
                }
                Ok(SemanticIndex {
                    generation,
                    dim,
                    by_doc,
                    hnsw_params,
                })
            }
        }
        de.deserialize_map(V)
    }
}

// ─────────────────────────── Builder ───────────────────────────

/// Deterministic per-generation semantic-index builder.
pub struct SemanticIndexBuilder {
    generation: u64,
    dim: u32,
    by_doc: BTreeMap<DocId, Vec<f32>>,
}

impl SemanticIndexBuilder {
    /// Construct a fresh builder for `(generation, dim)`. Rejects
    /// `generation == 0`, `dim == 0`, and `dim > MAX_EMBEDDING_DIM`.
    pub fn new(generation: u64, dim: u32) -> Result<Self, SemanticError> {
        if generation == 0 {
            return Err(SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                "generation must be non-zero",
            ));
        }
        if dim == 0 {
            return Err(SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                "dim must be non-zero",
            ));
        }
        let max = u32::try_from(MAX_EMBEDDING_DIM).map_err(|e| {
            SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                format!("MAX_EMBEDDING_DIM cast: {e}"),
            )
        })?;
        if dim > max {
            return Err(SemanticError::plan_limit(
                LimitDimension::EmbeddingDim,
                format!("dim {dim} exceeds MAX_EMBEDDING_DIM {max}"),
            ));
        }
        Ok(Self {
            generation,
            dim,
            by_doc: BTreeMap::new(),
        })
    }

    /// Generation id of this builder.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Embedding dimension of this builder.
    #[must_use]
    pub const fn dim(&self) -> u32 {
        self.dim
    }

    /// Number of staged documents.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_doc.len()
    }

    /// `true` if no documents have been added.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_doc.is_empty()
    }

    /// Stage an `(doc_id, embedding)` pair. Rejects dim mismatch with a
    /// typed [`SemanticErrorCode::SemDimMismatch`] error and rejects
    /// duplicate `doc_id` insertions with
    /// [`SemanticErrorCode::IndexCorrupted`].
    pub fn add_embedding(
        &mut self,
        doc_id: DocId,
        embedding: &Embedding,
    ) -> Result<(), SemanticError> {
        if embedding.dim() != self.dim {
            return Err(SemanticError::new(
                SemanticErrorCode::SemDimMismatch,
                format!(
                    "embedding dim {} != builder dim {} for doc {doc_id}",
                    embedding.dim(),
                    self.dim
                ),
            ));
        }
        if self.by_doc.contains_key(&doc_id) {
            return Err(SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                format!("duplicate doc {doc_id}"),
            ));
        }
        let v = embedding.as_slice().to_vec();
        let _prev = self.by_doc.insert(doc_id, v);
        Ok(())
    }

    /// Finalise the builder into a [`SemanticIndex`]. The result has
    /// no attached HNSW configuration; use
    /// [`SemanticIndex::with_hnsw_params`] to opt in.
    #[must_use]
    pub fn finish(self) -> SemanticIndex {
        SemanticIndex {
            generation: self.generation,
            dim: self.dim,
            by_doc: self.by_doc,
            hnsw_params: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SemanticIndex, SemanticIndexBuilder};
    use crate::errors::{LimitDimension, SemanticErrorCode};
    use crate::types::{DocId, Embedding, MAX_EMBEDDING_DIM};

    fn emb(v: Vec<f32>) -> Embedding {
        let Ok(e) = Embedding::new(v) else {
            std::process::abort();
        };
        e
    }

    #[test]
    fn builder_rejects_zero_generation() {
        match SemanticIndexBuilder::new(0, 3) {
            Ok(_) => assert!(false, "must reject"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::IndexCorrupted),
        }
    }

    #[test]
    fn builder_rejects_zero_dim() {
        match SemanticIndexBuilder::new(1, 0) {
            Ok(_) => assert!(false, "must reject"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn builder_rejects_oversized_dim() {
        let Ok(dim) = u32::try_from(MAX_EMBEDDING_DIM.saturating_add(1)) else {
            assert!(false, "MAX_EMBEDDING_DIM+1 must fit in u32 for test");
            return;
        };
        match SemanticIndexBuilder::new(1, dim) {
            Ok(_) => assert!(false, "must reject"),
            Err(e) => {
                assert_eq!(e.code, SemanticErrorCode::PlanLimitExceeded);
                assert_eq!(e.dimension, Some(LimitDimension::EmbeddingDim));
            }
        }
    }

    #[test]
    fn add_embedding_dim_mismatch_errors() {
        let Ok(mut b) = SemanticIndexBuilder::new(1, 3) else {
            assert!(false, "builder must construct");
            return;
        };
        let e = emb(vec![1.0_f32, 2.0_f32]);
        match b.add_embedding(DocId(1), &e) {
            Ok(()) => assert!(false, "must reject mismatch"),
            Err(err) => assert_eq!(err.code, SemanticErrorCode::SemDimMismatch),
        }
    }

    #[test]
    fn add_embedding_duplicate_doc_errors() {
        let Ok(mut b) = SemanticIndexBuilder::new(1, 2) else {
            assert!(false, "builder must construct");
            return;
        };
        let e1 = emb(vec![1.0_f32, 0.0_f32]);
        if let Err(err) = b.add_embedding(DocId(1), &e1) {
            assert!(false, "{err}");
            return;
        }
        let e2 = emb(vec![0.0_f32, 1.0_f32]);
        match b.add_embedding(DocId(1), &e2) {
            Ok(()) => assert!(false, "must reject duplicate"),
            Err(err) => assert_eq!(err.code, SemanticErrorCode::IndexCorrupted),
        }
    }

    fn fixture() -> SemanticIndex {
        let Ok(mut b) = SemanticIndexBuilder::new(1, 2) else {
            std::process::abort();
        };
        // Insert out of order to confirm `BTreeMap` sorts.
        let e3 = emb(vec![0.0_f32, 1.0_f32]);
        if b.add_embedding(DocId(3), &e3).is_err() {
            std::process::abort();
        }
        let e1 = emb(vec![1.0_f32, 0.0_f32]);
        if b.add_embedding(DocId(1), &e1).is_err() {
            std::process::abort();
        }
        let e2 = emb(vec![1.0_f32, 1.0_f32]);
        if b.add_embedding(DocId(2), &e2).is_err() {
            std::process::abort();
        }
        b.finish()
    }

    #[test]
    fn index_metadata_accessors() {
        let i = fixture();
        assert_eq!(i.generation(), 1);
        assert_eq!(i.dim(), 2);
        assert_eq!(i.corpus_size(), 3);
        assert!(!i.is_empty());
    }

    #[test]
    fn index_iter_sorted_by_docid() {
        let i = fixture();
        let docs: Vec<DocId> = i.iter().map(|(d, _)| d).collect();
        assert_eq!(docs, vec![DocId(1), DocId(2), DocId(3)]);
    }

    #[test]
    fn index_get_returns_vector() {
        let i = fixture();
        let Some(v) = i.get(DocId(2)) else {
            assert!(false, "doc 2 must exist");
            return;
        };
        assert_eq!(v, &[1.0_f32, 1.0_f32]);
        assert!(i.get(DocId(99)).is_none());
    }

    #[test]
    fn cbor_roundtrip_preserves_value() {
        let i = fixture();
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = i.serialize_cbor(&mut buf) {
            assert!(false, "{e}");
            return;
        }
        match SemanticIndex::deserialize_cbor(buf.as_slice()) {
            Ok(got) => assert_eq!(got, i),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn cbor_encoding_byte_identical_across_runs() {
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
        match SemanticIndex::deserialize_cbor(t) {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::IndexDeserialize),
        }
    }

    #[test]
    fn empty_corpus_serialises_and_roundtrips() {
        let b = match SemanticIndexBuilder::new(7, 4) {
            Ok(b) => b,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let i = b.finish();
        assert!(i.is_empty());
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = i.serialize_cbor(&mut buf) {
            assert!(false, "{e}");
            return;
        }
        match SemanticIndex::deserialize_cbor(buf.as_slice()) {
            Ok(got) => assert_eq!(got, i),
            Err(e) => assert!(false, "{e}"),
        }
    }
}
