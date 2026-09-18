//! One shard of the text authority: a doc-id range's slice of every
//! derived structure, in one immutable file.
//!
//! A shard file is a fixed-order CBOR array of the shard's doc-table rows
//! (doc id, candidate id, NFC text, folded text) and its four derived
//! indexes — byte trigrams over the NFC text, byte trigrams over the folded
//! text, token positions under case-sensitive and under folded
//! normalization — each restricted to the shard's documents. Doc ids are
//! global, so the shards' indexes union without remapping.
//!
//! [`ShardBuilders`] owns "add this document everywhere" and "retire this
//! document everywhere", so a full rebuild and an in-place update cannot
//! drift on which structures a document lives in.

use std::collections::BTreeMap;
use std::path::Path;

use quanta_index_contract::ManifestGeneration;
use quanta_index_core::CoreError;
use quanta_index_lq_positions::{
    DocId as PositionsDocId, NormalizerVersion, Position, PositionsBuilder, PositionsIndex,
};
use quanta_index_lq_trigram::{DocId as TrigramDocId, TrigramIndex, TrigramIndexBuilder};
use sha2::{Digest as _, Sha256};

use crate::normalize::{self, CaseMode, TEXT_NORMALIZER_VERSION};
use crate::text_authority::manifest::{SHARD_DOCS, shard_doc_range};
use crate::{map_positions_error, map_trigram_error};

/// SHA-256 of in-memory bytes: the shard's content digest as the manifest
/// commits to it and as its file name carries it. Infallible by construction.
pub(crate) fn sha256_of_bytes(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// The position sidecars' normalizer stamp.
///
/// It is the shared text normalizer's version, so a shard built under
/// another contract is refused by the positions engine as well as by the
/// manifest.
const POSITIONS_NORMALIZER_VERSION: NormalizerVersion =
    NormalizerVersion::new(TEXT_NORMALIZER_VERSION.major, TEXT_NORMALIZER_VERSION.minor);

/// One doc-table row as encoded: `(doc_id, candidate_id, text, folded)`.
type DocTableRow = (u64, String, String, String);

/// Wire shape of a shard file: a fixed-order CBOR array so the encoding is
/// auditable without a derive and an extra or missing element is a decode
/// failure.
type ShardRow = (
    Vec<DocTableRow>,
    TrigramIndex,
    TrigramIndex,
    PositionsIndex,
    PositionsIndex,
);

/// One document as the text authority holds it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TextAuthorityDoc {
    pub(crate) candidate_id: String,
    /// The indexed text, NFC.
    pub(crate) indexed_text: String,
    /// The same text under the shared case fold, for `case:no` byte surfaces.
    pub(crate) folded_indexed_text: String,
}

/// A decoded shard.
#[derive(Clone, Debug)]
pub(crate) struct ShardBody {
    pub(crate) docs_by_id: BTreeMap<u64, TextAuthorityDoc>,
    pub(crate) trigram: TrigramIndex,
    pub(crate) trigram_folded: TrigramIndex,
    pub(crate) positions: PositionsIndex,
    pub(crate) positions_folded: PositionsIndex,
}

impl ShardBody {
    /// Bytes this decoded shard keeps on the heap: the document texts
    /// (NFC and folded) with their ids, the trigram posting lists and the
    /// positions posting bytes, plus a fixed per-entry allowance for the
    /// map nodes and vector headers that hold them. An estimate that is
    /// monotone in the real cost, which is what a byte budget needs; the
    /// on-disk CBOR is smaller than this by the folded copy and the
    /// expanded postings.
    pub(crate) fn heap_bytes_estimate(&self) -> u64 {
        const ENTRY_OVERHEAD: u64 = 64;
        let mut bytes = 0_u64;
        for doc in self.docs_by_id.values() {
            bytes = bytes
                .saturating_add(ENTRY_OVERHEAD)
                .saturating_add(len_u64(doc.candidate_id.len()))
                .saturating_add(len_u64(doc.indexed_text.len()))
                .saturating_add(len_u64(doc.folded_indexed_text.len()));
        }
        for index in [&self.trigram, &self.trigram_folded] {
            for (_trigram, postings) in index.iter() {
                bytes = bytes
                    .saturating_add(ENTRY_OVERHEAD)
                    .saturating_add(len_u64(postings.len()).saturating_mul(8));
            }
        }
        for index in [&self.positions, &self.positions_folded] {
            for term in index.terms() {
                let postings = index.raw_postings(term).map_or(0, <[u8]>::len);
                bytes = bytes
                    .saturating_add(ENTRY_OVERHEAD)
                    .saturating_add(len_u64(term.len()))
                    .saturating_add(len_u64(postings));
            }
        }
        bytes
    }

    /// Documents in the shard.
    pub(crate) fn rows(&self) -> Result<u64, CoreError> {
        u64::try_from(self.docs_by_id.len()).map_err(|err| {
            CoreError::InvalidContract(format!("lexical: text authority shard row count: {err}"))
        })
    }

    /// Lowest and highest doc id, or `None` for an empty shard.
    pub(crate) fn doc_id_extremes(&self) -> Option<(u64, u64)> {
        let min = self.docs_by_id.keys().next()?;
        let max = self.docs_by_id.keys().next_back()?;
        Some((*min, *max))
    }

    /// Encode as the shard file's bytes.
    pub(crate) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        let rows: Vec<DocTableRow> = self
            .docs_by_id
            .iter()
            .map(|(doc_id, doc)| {
                (
                    *doc_id,
                    doc.candidate_id.clone(),
                    doc.indexed_text.clone(),
                    doc.folded_indexed_text.clone(),
                )
            })
            .collect();
        let row: (
            &Vec<DocTableRow>,
            &TrigramIndex,
            &TrigramIndex,
            &PositionsIndex,
            &PositionsIndex,
        ) = (
            &rows,
            &self.trigram,
            &self.trigram_folded,
            &self.positions,
            &self.positions_folded,
        );
        crate::encode_cbor(&row, "text authority shard")
    }

    /// Decode the bytes of shard `index`, named `name` under `generation_dir`.
    ///
    /// Fail-closed and typed (`GENERATION_SIDECAR_CORRUPT`): the bytes must
    /// decode as the shard row, rows must be strictly ascending doc ids
    /// inside the shard's range, the position postings must carry this
    /// build's normalizer, and the embedded indexes decode under their own
    /// unknown-field and ordering checks.
    pub(crate) fn decode(
        bytes: &[u8],
        index: u64,
        generation_dir: &Path,
        name: &str,
    ) -> Result<Self, CoreError> {
        let corrupt = |reason: String| crate::sidecar_corrupt(generation_dir, name, &reason);
        let (rows, trigram, trigram_folded, positions, positions_folded): ShardRow =
            ciborium::from_reader(bytes)
                .map_err(|error| corrupt(format!("shard {index} does not decode: {error}")))?;
        let (first, last) = shard_doc_range(index)?;
        let mut docs_by_id: BTreeMap<u64, TextAuthorityDoc> = BTreeMap::new();
        for (doc_id, candidate_id, indexed_text, folded_indexed_text) in rows {
            if doc_id < first || doc_id > last {
                return Err(corrupt(format!(
                    "row doc id {doc_id} is outside the shard's range {first}..={last}"
                )));
            }
            if let Some((previous, _)) = docs_by_id.last_key_value()
                && *previous >= doc_id
            {
                return Err(corrupt(format!(
                    "row doc id {doc_id} does not follow {previous}; rows must be strictly ascending"
                )));
            }
            let _prior = docs_by_id.insert(
                doc_id,
                TextAuthorityDoc {
                    candidate_id,
                    indexed_text,
                    folded_indexed_text,
                },
            );
        }
        if positions.normalizer_version() != POSITIONS_NORMALIZER_VERSION
            || positions_folded.normalizer_version() != POSITIONS_NORMALIZER_VERSION
        {
            return Err(corrupt(format!(
                "position postings carry normalizer {} (this build runs {POSITIONS_NORMALIZER_VERSION})",
                positions.normalizer_version()
            )));
        }
        Ok(Self {
            docs_by_id,
            trigram,
            trigram_folded,
            positions,
            positions_folded,
        })
    }
}

fn sidecar_generation_id(generation: ManifestGeneration) -> u64 {
    generation.get().max(1)
}

/// The `(term, position)` pairs of one document for a position sidecar.
///
/// The same tokenizer the inverted index runs: NFC text, `case` fold, shared
/// boundaries, over-long runs skipped but still counted, so a phrase lookup
/// over the sidecar and a keyword sequence over the index agree.
fn phrase_term_positions(text: &str, case: CaseMode) -> Result<Vec<(String, Position)>, CoreError> {
    normalize::tokenize(text, case)
        .indexable()
        .map(|token| {
            let position = u32::try_from(token.position).map_err(|err| {
                CoreError::InvalidContract(format!(
                    "lexical: text authority token position {} overflows the position sidecar: {err}",
                    token.position
                ))
            })?;
            Ok((token.text.clone(), Position(position)))
        })
        .collect()
}

/// One shard's four derived indexes plus its doc table, under construction.
///
/// The embedded generation stamp is the generation that writes the shard;
/// a shard a later generation inherits keeps its writer's stamp, which is
/// its provenance, not a serving condition.
pub(crate) struct ShardBuilders {
    index: u64,
    trigram: TrigramIndexBuilder,
    trigram_folded: TrigramIndexBuilder,
    positions: PositionsBuilder,
    positions_folded: PositionsBuilder,
    table: BTreeMap<u64, TextAuthorityDoc>,
}

impl ShardBuilders {
    /// Start shard `index` from nothing.
    pub(crate) fn empty(index: u64, generation: ManifestGeneration) -> Result<Self, CoreError> {
        let sidecar_generation = sidecar_generation_id(generation);
        Ok(Self {
            index,
            trigram: TrigramIndexBuilder::new(sidecar_generation)
                .map_err(|err| map_trigram_error("init trigram sidecar", &err))?,
            trigram_folded: TrigramIndexBuilder::new(sidecar_generation)
                .map_err(|err| map_trigram_error("init folded trigram sidecar", &err))?,
            positions: PositionsBuilder::new(sidecar_generation, POSITIONS_NORMALIZER_VERSION),
            positions_folded: PositionsBuilder::new(
                sidecar_generation,
                POSITIONS_NORMALIZER_VERSION,
            ),
            table: BTreeMap::new(),
        })
    }

    /// Continue shard `index` from its prior body instead of from nothing:
    /// the untouched documents are carried as postings, never re-derived.
    pub(crate) fn from_prior(
        index: u64,
        generation: ManifestGeneration,
        prior: ShardBody,
    ) -> Result<Self, CoreError> {
        let sidecar_generation = sidecar_generation_id(generation);
        Ok(Self {
            index,
            trigram: TrigramIndexBuilder::from_prior(&prior.trigram, sidecar_generation)
                .map_err(|err| map_trigram_error("inherit trigram sidecar", &err))?,
            trigram_folded: TrigramIndexBuilder::from_prior(
                &prior.trigram_folded,
                sidecar_generation,
            )
            .map_err(|err| map_trigram_error("inherit folded trigram sidecar", &err))?,
            positions: PositionsBuilder::from_prior(
                &prior.positions,
                sidecar_generation,
                POSITIONS_NORMALIZER_VERSION,
            )
            .map_err(|err| map_positions_error("inherit positions sidecar", &err))?,
            positions_folded: PositionsBuilder::from_prior(
                &prior.positions_folded,
                sidecar_generation,
                POSITIONS_NORMALIZER_VERSION,
            )
            .map_err(|err| map_positions_error("inherit folded positions sidecar", &err))?,
            table: prior.docs_by_id,
        })
    }

    fn ensure_in_range(&self, doc_id: u64) -> Result<(), CoreError> {
        let (first, last) = shard_doc_range(self.index)?;
        if doc_id < first || doc_id > last {
            return Err(CoreError::InvalidContract(format!(
                "lexical: doc {doc_id} does not belong to text authority shard {} ({first}..={last}, {SHARD_DOCS} per shard)",
                self.index
            )));
        }
        Ok(())
    }

    /// Add one document to every structure of the shard.
    ///
    /// `indexed_text` is normalized to NFC here regardless of its source (a
    /// stored Tantivy field or a chunk straight from the batch), and the
    /// folded copy the `case:no` trigram and raw-substring paths read is the
    /// shared [`normalize::fold`].
    pub(crate) fn upsert(
        &mut self,
        doc_id: u64,
        candidate_id: String,
        indexed_text: &str,
    ) -> Result<(), CoreError> {
        self.ensure_in_range(doc_id)?;
        let indexed_text = normalize::nfc(indexed_text).into_owned();
        let folded_indexed_text = normalize::fold(&indexed_text);
        self.trigram
            .upsert_doc(TrigramDocId(doc_id), indexed_text.as_bytes())
            .map_err(|err| map_trigram_error("build trigram sidecar", &err))?;
        self.trigram_folded
            .upsert_doc(TrigramDocId(doc_id), folded_indexed_text.as_bytes())
            .map_err(|err| map_trigram_error("build folded trigram sidecar", &err))?;
        let sensitive_pairs = phrase_term_positions(&indexed_text, CaseMode::Sensitive)?;
        self.positions
            .upsert_doc(
                PositionsDocId(doc_id),
                sensitive_pairs
                    .iter()
                    .map(|(term, pos)| (term.as_str(), *pos)),
            )
            .map_err(|err| map_positions_error("build positions sidecar", &err))?;
        let folded_pairs = phrase_term_positions(&indexed_text, CaseMode::Folded)?;
        self.positions_folded
            .upsert_doc(
                PositionsDocId(doc_id),
                folded_pairs.iter().map(|(term, pos)| (term.as_str(), *pos)),
            )
            .map_err(|err| map_positions_error("build folded positions sidecar", &err))?;
        let _prior = self.table.insert(
            doc_id,
            TextAuthorityDoc {
                candidate_id,
                indexed_text,
                folded_indexed_text,
            },
        );
        Ok(())
    }

    /// Remove one document from every structure of the shard; `false` when
    /// the shard never held it.
    pub(crate) fn retire(&mut self, doc_id: u64) -> Result<bool, CoreError> {
        self.ensure_in_range(doc_id)?;
        let _removed = self
            .trigram
            .remove_doc(TrigramDocId(doc_id))
            .map_err(|err| map_trigram_error("retire trigram sidecar doc", &err))?;
        let _removed = self
            .trigram_folded
            .remove_doc(TrigramDocId(doc_id))
            .map_err(|err| map_trigram_error("retire folded trigram sidecar doc", &err))?;
        let _removed = self
            .positions
            .remove_doc(PositionsDocId(doc_id))
            .map_err(|err| map_positions_error("retire positions sidecar doc", &err))?;
        let _removed = self
            .positions_folded
            .remove_doc(PositionsDocId(doc_id))
            .map_err(|err| map_positions_error("retire folded positions sidecar doc", &err))?;
        Ok(self.table.remove(&doc_id).is_some())
    }

    /// Finalize into an immutable shard body.
    pub(crate) fn finish(self) -> Result<ShardBody, CoreError> {
        Ok(ShardBody {
            docs_by_id: self.table,
            trigram: self.trigram.finish(),
            trigram_folded: self.trigram_folded.finish(),
            positions: self
                .positions
                .finish()
                .map_err(|err| map_positions_error("finalize positions sidecar", &err))?,
            positions_folded: self
                .positions_folded
                .finish()
                .map_err(|err| map_positions_error("finalize folded positions sidecar", &err))?,
        })
    }
}

/// A length as bytes; a length past `u64` saturates rather than wraps.
fn len_u64(length: usize) -> u64 {
    u64::try_from(length).map_or(u64::MAX, |value| value)
}

#[cfg(test)]
mod tests {
    use super::{ShardBody, ShardBuilders};
    use crate::text_authority::manifest::SHARD_DOCS;
    use quanta_index_contract::ManifestGeneration;
    use std::path::Path;

    #[test]
    fn a_shard_round_trips_through_its_file_bytes() {
        let mut builders = ShardBuilders::empty(1, ManifestGeneration::new(3)).expect("empty");
        builders
            .upsert(SHARD_DOCS, "cand-a".to_string(), "Alpha Beta")
            .expect("upsert");
        builders
            .upsert(SHARD_DOCS + 7, "cand-b".to_string(), "gamma")
            .expect("upsert");
        let body = builders.finish().expect("finish");
        assert_eq!(body.rows().expect("rows"), 2);
        assert_eq!(body.doc_id_extremes(), Some((SHARD_DOCS, SHARD_DOCS + 7)));
        let bytes = body.encode().expect("encode");
        let decoded = ShardBody::decode(&bytes, 1, Path::new("/g1"), "shard").expect("decode");
        assert_eq!(decoded.docs_by_id, body.docs_by_id);
        assert_eq!(decoded.trigram, body.trigram);
        assert_eq!(decoded.positions_folded, body.positions_folded);
        assert_eq!(
            decoded
                .docs_by_id
                .get(&SHARD_DOCS)
                .map(|doc| doc.folded_indexed_text.as_str()),
            Some("alpha beta")
        );
    }

    #[test]
    fn a_shard_refuses_documents_outside_its_range() {
        let mut builders = ShardBuilders::empty(1, ManifestGeneration::new(3)).expect("empty");
        assert!(
            builders
                .upsert(SHARD_DOCS - 1, "cand".to_string(), "text")
                .is_err()
        );
        assert!(builders.retire(2 * SHARD_DOCS).is_err());
        let body = builders.finish().expect("finish");
        let bytes = body.encode().expect("encode");
        assert!(ShardBody::decode(&bytes, 1, Path::new("/g1"), "shard").is_ok());
        let mut other = ShardBuilders::empty(0, ManifestGeneration::new(3)).expect("empty");
        other.upsert(5, "cand".to_string(), "text").expect("upsert");
        let bytes = other.finish().expect("finish").encode().expect("encode");
        assert!(ShardBody::decode(&bytes, 1, Path::new("/g1"), "shard").is_err());
    }

    #[test]
    fn retiring_reports_whether_the_shard_held_the_document() {
        let mut builders = ShardBuilders::empty(0, ManifestGeneration::new(1)).expect("empty");
        builders
            .upsert(4, "cand".to_string(), "text")
            .expect("upsert");
        assert!(builders.retire(4).expect("retire"));
        assert!(!builders.retire(4).expect("retire again"));
        let body = builders.finish().expect("finish");
        assert_eq!(body.rows().expect("rows"), 0);
        assert!(body.doc_id_extremes().is_none());
    }
}
