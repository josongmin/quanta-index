//! Bounded segment-local posting producer and independent seal oracle.
//!
//! Logical document IDs are mapped outside this module; these builders use
//! only local IDs and feed independently readable paged Native objects.

use std::collections::BTreeMap;
use std::path::Path;

use quanta_index_contract::ManifestGeneration;
use quanta_index_core::CoreError;
use quanta_index_lq_positions::{
    DocId as PositionsDocId, MAX_POSITIONS_PER_CELL, NormalizerVersion, Position, PositionsBuilder,
    PositionsIndex,
};
use quanta_index_lq_trigram::{DocId as TrigramDocId, Trigram, TrigramIndex, TrigramIndexBuilder};

use crate::normalize::{self, CaseMode, TEXT_NORMALIZER_VERSION};
use crate::query_errors::map_positions_error;
use crate::query_errors::map_trigram_error;
// The local builder is retained as a bounded producer for paged Native
// segments. Its IDs are segment-local and never persisted as logical IDs.
const SHARD_DOCS: u64 = 2048;

fn shard_doc_range(index: u64) -> Result<(u64, u64), CoreError> {
    let first = index.checked_mul(SHARD_DOCS).ok_or_else(|| {
        CoreError::InvalidContract("lexical: Native local shard index overflows".into())
    })?;
    let last = first
        .checked_add(SHARD_DOCS.saturating_sub(1))
        .ok_or_else(|| {
            CoreError::InvalidContract("lexical: Native local shard range overflows".into())
        })?;
    Ok((first, last))
}

/// The position sidecars' normalizer stamp.
///
/// It is the shared text normalizer's version, so a shard built under
/// another contract is refused by the positions engine as well as by the
/// manifest.
const POSITIONS_NORMALIZER_VERSION: NormalizerVersion =
    NormalizerVersion::new(TEXT_NORMALIZER_VERSION.major, TEXT_NORMALIZER_VERSION.minor);

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
    /// Independently derive the four posting relations from the document table.
    /// This seal oracle shares only the text normalizer with the writer. It
    /// neither calls a posting builder nor decodes untrusted position counts
    /// before comparing their complete canonical byte strings.
    pub(crate) fn verify_semantics(
        &self,
        index: u64,
        generation_dir: &Path,
        name: &str,
    ) -> Result<(), CoreError> {
        let corrupt =
            |reason: &str| crate::index_store::sidecar_corrupt(generation_dir, name, reason);
        let generation = self.trigram.generation();
        if generation == 0
            || self.trigram_folded.generation() != generation
            || self.positions.generation() != generation
            || self.positions_folded.generation() != generation
        {
            return Err(corrupt(
                "shard derived indexes carry different generation stamps",
            ));
        }
        if self.positions.normalizer_version() != POSITIONS_NORMALIZER_VERSION
            || self.positions_folded.normalizer_version() != POSITIONS_NORMALIZER_VERSION
        {
            return Err(corrupt("shard position normalizer differs from policy"));
        }
        let (first, last) = shard_doc_range(index)?;
        if self.docs_by_id.len()
            > usize::try_from(SHARD_DOCS).map_err(|_error| {
                corrupt("shard row limit cannot be represented on this platform")
            })?
        {
            return Err(corrupt(
                "shard has more document rows than its doc-id range",
            ));
        }
        for (doc_id, doc) in &self.docs_by_id {
            if *doc_id < first || *doc_id > last {
                return Err(corrupt("shard document lies outside its doc-id range"));
            }
            if normalize::nfc(&doc.indexed_text).as_ref() != doc.indexed_text.as_str()
                || normalize::fold(&doc.indexed_text) != doc.folded_indexed_text
            {
                return Err(corrupt("shard document normalization differs from policy"));
            }
        }
        verify_trigrams(&self.docs_by_id, &self.trigram, false, &corrupt)?;
        verify_trigrams(&self.docs_by_id, &self.trigram_folded, true, &corrupt)?;
        verify_positions(
            &self.docs_by_id,
            &self.positions,
            CaseMode::Sensitive,
            &corrupt,
        )?;
        verify_positions(
            &self.docs_by_id,
            &self.positions_folded,
            CaseMode::Folded,
            &corrupt,
        )?;
        Ok(())
    }
}

/// Compare every gram key and its complete sorted doc list. A gram repeated
/// within one document has one posting, including across UTF-8 byte windows.
fn verify_trigrams(
    docs: &BTreeMap<u64, TextAuthorityDoc>,
    observed: &TrigramIndex,
    folded: bool,
    corrupt: &impl Fn(&str) -> CoreError,
) -> Result<(), CoreError> {
    let mut expected: BTreeMap<Trigram, Vec<TrigramDocId>> = BTreeMap::new();
    for (doc_id, doc) in docs {
        let text = if folded {
            &doc.folded_indexed_text
        } else {
            &doc.indexed_text
        };
        let mut grams = Vec::<Trigram>::new();
        grams
            .try_reserve(text.len().saturating_sub(2))
            .map_err(|error| {
                CoreError::Storage(format!("lexical: reserve shard gram oracle: {error}"))
            })?;
        for window in text.as_bytes().windows(3) {
            let gram: Trigram = window
                .try_into()
                .map_err(|_error| corrupt("shard gram window has wrong width"))?;
            grams.push(gram);
        }
        grams.sort_unstable();
        grams.dedup();
        for gram in grams {
            let postings = expected.entry(gram).or_default();
            postings.try_reserve(1).map_err(|error| {
                CoreError::Storage(format!(
                    "lexical: reserve shard gram posting oracle: {error}"
                ))
            })?;
            postings.push(TrigramDocId(*doc_id));
        }
    }
    let mut actual = observed.iter();
    for (gram, postings) in &expected {
        if actual.next() != Some((*gram, postings.as_slice())) {
            return Err(corrupt(
                "shard trigram postings differ from its document table",
            ));
        }
    }
    if actual.next().is_some() {
        return Err(corrupt("shard trigram dictionary has an extra key"));
    }
    Ok(())
}

/// Independently encode a u32 as unsigned LEB128 for exact raw-posting
/// comparison. Keeping this local avoids depending on the writer codec.
fn oracle_varint(mut value: u32, bytes: &mut Vec<u8>) -> Result<(), CoreError> {
    bytes.try_reserve(5).map_err(|error| {
        CoreError::Storage(format!("lexical: reserve shard varint oracle: {error}"))
    })?;
    while value >= 128 {
        let low = u8::try_from(value & 0x7f).map_err(|error| {
            CoreError::InvalidContract(format!("lexical: oracle varint byte: {error}"))
        })?;
        bytes.push(low | 0x80);
        value >>= 7;
    }
    let last = u8::try_from(value).map_err(|error| {
        CoreError::InvalidContract(format!("lexical: oracle varint tail: {error}"))
    })?;
    bytes.push(last);
    Ok(())
}

/// Compare exact term inventory and canonical per-term bytes.
///
/// The observed byte stream is never decoded here: a forged count cannot
/// drive an allocation, and trailing/nonminimal bytes fail by inequality.
fn verify_positions(
    docs: &BTreeMap<u64, TextAuthorityDoc>,
    observed: &PositionsIndex,
    case: CaseMode,
    corrupt: &impl Fn(&str) -> CoreError,
) -> Result<(), CoreError> {
    type OracleTermDocs = Vec<(u64, Vec<u32>)>;
    let mut expected: BTreeMap<String, OracleTermDocs> = BTreeMap::new();
    for (doc_id, doc) in docs {
        let tokens = normalize::tokenize(&doc.indexed_text, case);
        for token in tokens.indexable() {
            let position = u32::try_from(token.position)
                .map_err(|_error| corrupt("shard token position exceeds u32"))?;
            let entries = expected.entry(token.text.clone()).or_default();
            if entries.last().is_none_or(|(prior, _)| prior != doc_id) {
                entries.try_reserve(1).map_err(|error| {
                    CoreError::Storage(format!("lexical: reserve shard position oracle: {error}"))
                })?;
                entries.push((*doc_id, Vec::new()));
            }
            let Some((_, positions)) = entries.last_mut() else {
                return Err(corrupt("shard position oracle lost document entry"));
            };
            if positions.len()
                >= usize::try_from(MAX_POSITIONS_PER_CELL)
                    .map_err(|_error| corrupt("position cell cap exceeds usize"))?
            {
                return Err(corrupt("shard position cell exceeds policy cap"));
            }
            positions.try_reserve(1).map_err(|error| {
                CoreError::Storage(format!("lexical: reserve shard position cell: {error}"))
            })?;
            positions.push(position);
        }
    }

    let mut actual_terms = observed.terms();
    for (term, entries) in &expected {
        if actual_terms.next() != Some(term.as_str()) {
            return Err(corrupt(
                "shard position term inventory differs from its document table",
            ));
        }
        let mut bytes = Vec::new();
        let doc_count = u32::try_from(entries.len())
            .map_err(|_error| corrupt("shard term has too many documents"))?;
        oracle_varint(doc_count, &mut bytes)?;
        let mut prior_doc = 0_u64;
        for (doc_id, positions) in entries {
            let gap = doc_id
                .checked_sub(prior_doc)
                .ok_or_else(|| corrupt("shard position doc IDs are not ascending"))?;
            let gap = u32::try_from(gap)
                .map_err(|_error| corrupt("shard position doc gap exceeds u32"))?;
            oracle_varint(gap, &mut bytes)?;
            prior_doc = *doc_id;

            let position_count = u32::try_from(positions.len())
                .map_err(|_error| corrupt("shard position count exceeds u32"))?;
            oracle_varint(position_count, &mut bytes)?;
            let mut prior_position = 0_u32;
            for position in positions {
                let gap = position
                    .checked_sub(prior_position)
                    .ok_or_else(|| corrupt("shard token positions are not ascending"))?;
                oracle_varint(gap, &mut bytes)?;
                prior_position = *position;
            }
        }
        if observed.raw_postings(term) != Some(bytes.as_slice()) {
            return Err(corrupt(
                "shard position postings differ from its document table",
            ));
        }
    }
    if actual_terms.next().is_some() {
        return Err(corrupt("shard position dictionary has an extra term"));
    }
    Ok(())
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

#[derive(Clone, Copy)]
enum PositionWriteMode {
    Append,
    #[cfg(test)]
    Replace,
}

fn write_positions(
    builder: &mut PositionsBuilder,
    doc_id: PositionsDocId,
    pairs: &[(String, Position)],
    mode: PositionWriteMode,
    what: &str,
) -> Result<(), CoreError> {
    match mode {
        PositionWriteMode::Append => {
            for (term, position) in pairs {
                builder
                    .add_token(doc_id, term, *position)
                    .map_err(|err| map_positions_error(what, &err))?;
            }
            Ok(())
        }
        #[cfg(test)]
        PositionWriteMode::Replace => builder
            .upsert_doc(
                doc_id,
                pairs
                    .iter()
                    .map(|(term, position)| (term.as_str(), *position)),
            )
            .map_err(|err| map_positions_error(what, &err)),
    }
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
    #[cfg(test)]
    pub(crate) fn upsert(
        &mut self,
        doc_id: u64,
        candidate_id: String,
        indexed_text: &str,
    ) -> Result<(), CoreError> {
        self.write_doc(
            doc_id,
            candidate_id,
            indexed_text,
            PositionWriteMode::Replace,
        )
    }

    /// Append a document to a freshly built segment. Local IDs are unique;
    /// replacement's all-term removal is unnecessary and would rescan the
    /// growing dictionary for every document.
    pub(crate) fn append(
        &mut self,
        doc_id: u64,
        candidate_id: String,
        indexed_text: &str,
    ) -> Result<(), CoreError> {
        self.ensure_in_range(doc_id)?;
        if self.table.contains_key(&doc_id) {
            return Err(CoreError::InvalidContract(format!(
                "lexical: duplicate fresh text authority local ID {doc_id}"
            )));
        }
        self.write_doc(
            doc_id,
            candidate_id,
            indexed_text,
            PositionWriteMode::Append,
        )
    }

    fn write_doc(
        &mut self,
        doc_id: u64,
        candidate_id: String,
        indexed_text: &str,
        mode: PositionWriteMode,
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
        write_positions(
            &mut self.positions,
            PositionsDocId(doc_id),
            &sensitive_pairs,
            mode,
            "build positions sidecar",
        )?;
        let folded_pairs = phrase_term_positions(&indexed_text, CaseMode::Folded)?;
        write_positions(
            &mut self.positions_folded,
            PositionsDocId(doc_id),
            &folded_pairs,
            mode,
            "build folded positions sidecar",
        )?;
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

#[cfg(test)]
mod tests {
    use super::SHARD_DOCS;
    use super::{ShardBody, ShardBuilders};
    use quanta_index_contract::ManifestGeneration;
    use quanta_index_lq_positions::PositionsIndex;
    use quanta_index_lq_trigram::{DocId as TrigramDocId, TrigramIndexBuilder};
    use std::collections::BTreeMap;
    use std::path::Path;

    fn semantic_fixture() -> ShardBody {
        let mut builders = ShardBuilders::empty(0, ManifestGeneration::new(1)).expect("empty");
        builders
            .upsert(1, "candidate".to_string(), "abc abc")
            .expect("upsert");
        builders.finish().expect("finish")
    }

    fn verify_fixture(body: &ShardBody) -> bool {
        body.verify_semantics(0, Path::new("/g1"), "shard").is_ok()
    }

    #[test]
    fn local_builder_rejects_documents_outside_its_segment() {
        let mut builders = ShardBuilders::empty(1, ManifestGeneration::new(3)).expect("empty");
        assert!(
            builders
                .upsert(SHARD_DOCS - 1, "cand".into(), "text")
                .is_err()
        );
        assert!(builders.upsert(SHARD_DOCS, "cand".into(), "text").is_ok());
        assert!(
            builders
                .upsert(2 * SHARD_DOCS, "cand".into(), "text")
                .is_err()
        );
    }

    #[test]
    fn independent_semantic_oracle_accepts_repeated_tokens_unicode_and_short_text() {
        let mut builders = ShardBuilders::empty(0, ManifestGeneration::new(1)).expect("empty");
        builders
            .upsert(1, "one".to_string(), "Straße abc abc")
            .expect("unicode and repeated token");
        builders
            .upsert(2, "two".to_string(), "é")
            .expect("short text");
        let body = builders.finish().expect("finish");
        assert!(verify_fixture(&body));
    }

    #[test]
    fn fresh_append_has_fixed_unicode_and_repeated_term_postings() {
        let mut builders = ShardBuilders::empty(0, ManifestGeneration::new(1)).expect("empty");
        builders
            .append(0, "first".into(), "CAFÉ cafe\u{301} red red")
            .expect("first fresh document");
        builders
            .append(1, "second".into(), "café RED")
            .expect("second fresh document");
        builders
            .append(2, "empty".into(), "")
            .expect("empty fresh document");
        let body = builders.finish().expect("finish fresh segment");
        assert_eq!(body.docs_by_id.len(), 3);
        assert_eq!(
            body.docs_by_id.get(&0).map(|doc| doc.indexed_text.as_str()),
            Some("CAFÉ café red red")
        );
        assert!(verify_fixture(&body));

        // Fixed canonical varints: doc count, ascending doc gaps, then
        // position counts and gaps. These bytes do not come from either
        // posting builder or its tokenizer.
        assert_eq!(body.positions.raw_postings("CAFÉ"), Some(&[1, 0, 1, 0][..]));
        assert_eq!(
            body.positions.raw_postings("café"),
            Some(&[2, 0, 1, 1, 1, 1, 0][..])
        );
        assert_eq!(
            body.positions.raw_postings("red"),
            Some(&[1, 0, 2, 2, 1][..])
        );
        assert_eq!(body.positions.raw_postings("RED"), Some(&[1, 1, 1, 1][..]));
        assert_eq!(
            body.positions_folded.raw_postings("café"),
            Some(&[2, 0, 2, 0, 1, 1, 1, 0][..])
        );
        assert_eq!(
            body.positions_folded.raw_postings("red"),
            Some(&[2, 0, 2, 2, 1, 1, 1, 1][..])
        );
        assert!(body.positions_folded.raw_postings("CAFÉ").is_none());
    }

    #[test]
    fn fresh_append_rejects_duplicate_local_id_before_changing_any_index() {
        let mut builders = ShardBuilders::empty(0, ManifestGeneration::new(1)).expect("empty");
        builders
            .append(7, "first".into(), "café alpha")
            .expect("first fresh document");
        assert!(matches!(
            builders.append(7, "replacement".into(), "other term"),
            Err(quanta_index_core::CoreError::InvalidContract(_))
        ));
        let body = builders.finish().expect("finish unchanged segment");
        assert_eq!(body.docs_by_id.len(), 1);
        assert_eq!(
            body.docs_by_id.get(&7).map(|doc| doc.candidate_id.as_str()),
            Some("first")
        );
        assert_eq!(body.positions.raw_postings("café"), Some(&[1, 7, 1, 0][..]));
        assert!(body.positions.raw_postings("other").is_none());
        assert!(verify_fixture(&body));
    }

    #[test]
    fn replacement_keeps_empty_and_replayed_nonempty_document_semantics() {
        let mut builders = ShardBuilders::empty(0, ManifestGeneration::new(1)).expect("empty");
        builders
            .upsert(3, "old".into(), "obsolete")
            .expect("initial document");
        builders
            .upsert(3, "blank".into(), "")
            .expect("clear tokens");
        builders
            .upsert(3, "current".into(), "CAFÉ café")
            .expect("replace empty document");
        builders
            .upsert(3, "current".into(), "CAFÉ café")
            .expect("idempotent replay");
        let body = builders.finish().expect("finish replacement segment");
        assert_eq!(body.docs_by_id.len(), 1);
        assert_eq!(
            body.docs_by_id.get(&3).map(|doc| doc.candidate_id.as_str()),
            Some("current")
        );
        assert!(body.positions.raw_postings("obsolete").is_none());
        assert_eq!(body.positions.raw_postings("CAFÉ"), Some(&[1, 3, 1, 0][..]));
        assert_eq!(body.positions.raw_postings("café"), Some(&[1, 3, 1, 1][..]));
        assert_eq!(
            body.positions_folded.raw_postings("café"),
            Some(&[1, 3, 2, 0, 1][..])
        );
        assert!(verify_fixture(&body));
    }

    #[test]
    fn independent_semantic_oracle_refuses_missing_extra_and_wrong_trigram_postings() {
        let valid = semantic_fixture();
        assert!(verify_fixture(&valid));
        for (content, doc_id) in [
            (b"".as_slice(), 1_u64),
            (b"abc abczzz".as_slice(), 1_u64),
            (b"abc abc".as_slice(), 2_u64),
        ] {
            let mut builder = TrigramIndexBuilder::new(1).expect("trigram builder");
            builder
                .upsert_doc(TrigramDocId(doc_id), content)
                .expect("mutant posting");
            let mut mutant = valid.clone();
            mutant.trigram = builder.finish();
            assert!(!verify_fixture(&mutant));
        }
        let mut builder = TrigramIndexBuilder::new(1).expect("folded builder");
        builder
            .upsert_doc(TrigramDocId(1), b"abc xyz")
            .expect("mutant folded posting");
        let mut mutant = valid;
        mutant.trigram_folded = builder.finish();
        assert!(!verify_fixture(&mutant));
    }

    #[test]
    fn independent_semantic_oracle_refuses_position_key_and_raw_byte_mutants() {
        let valid = semantic_fixture();
        assert!(verify_fixture(&valid));
        // Exactly one term "abc", one doc 1, and token positions 0 and 1.
        let canonical = [1_u8, 1, 2, 0, 1];
        let raw_mutants: [(&str, Option<Vec<u8>>); 7] = [
            ("missing", None),
            ("wrong doc", Some(vec![1, 2, 2, 0, 1])),
            ("wrong position", Some(vec![1, 1, 2, 0, 2])),
            ("trailing byte", Some(vec![1, 1, 2, 0, 1, 0])),
            ("nonminimal varint", Some(vec![0x81, 0, 1, 2, 0, 1])),
            ("truncated", Some(vec![1, 1, 2, 0])),
            (
                "forged huge doc count",
                Some(vec![0xff, 0xff, 0xff, 0xff, 0x0f]),
            ),
        ];
        for (label, raw) in raw_mutants {
            let mut by_term = BTreeMap::new();
            if let Some(raw) = raw {
                let _prior = by_term.insert(Box::<str>::from("abc"), raw);
            }
            let mut mutant = valid.clone();
            mutant.positions = PositionsIndex::from_raw(
                valid.positions.generation(),
                valid.positions.normalizer_version(),
                by_term,
            );
            assert!(!verify_fixture(&mutant), "{label}");
        }
        let mut extra_term = BTreeMap::new();
        let _prior = extra_term.insert(Box::<str>::from("abc"), canonical.to_vec());
        let _prior = extra_term.insert(Box::<str>::from("zzz"), canonical.to_vec());
        let mut mutant = valid.clone();
        mutant.positions = PositionsIndex::from_raw(
            valid.positions.generation(),
            valid.positions.normalizer_version(),
            extra_term,
        );
        assert!(!verify_fixture(&mutant));

        let mut wrong_folded = BTreeMap::new();
        let _prior = wrong_folded.insert(Box::<str>::from("abc"), vec![1, 1, 2, 0, 2]);
        let mut mutant = valid;
        mutant.positions_folded = PositionsIndex::from_raw(
            mutant.positions_folded.generation(),
            mutant.positions_folded.normalizer_version(),
            wrong_folded,
        );
        assert!(!verify_fixture(&mutant));
    }

    #[test]
    fn independent_semantic_oracle_refuses_stamp_range_and_normalization_mutants() {
        let valid = semantic_fixture();
        assert!(verify_fixture(&valid));

        let mut changed_generation = TrigramIndexBuilder::new(2).expect("trigram builder");
        changed_generation
            .upsert_doc(TrigramDocId(1), b"abc abc")
            .expect("same posting, wrong stamp");
        let mut mutant = valid.clone();
        mutant.trigram_folded = changed_generation.finish();
        assert!(!verify_fixture(&mutant));

        let mut mutant = valid.clone();
        let mut by_term = BTreeMap::new();
        let _prior = by_term.insert(Box::<str>::from("abc"), vec![1, 1, 2, 0, 1]);
        mutant.positions = PositionsIndex::from_raw(
            1,
            quanta_index_lq_positions::NormalizerVersion::new(99, 0),
            by_term,
        );
        assert!(!verify_fixture(&mutant));

        let mut mutant = valid.clone();
        let doc = mutant.docs_by_id.remove(&1).expect("doc");
        let _prior = mutant.docs_by_id.insert(SHARD_DOCS, doc);
        assert!(!verify_fixture(&mutant));

        let mut mutant = valid.clone();
        mutant.docs_by_id.get_mut(&1).expect("doc").indexed_text = "e\u{301}".to_string();
        assert!(!verify_fixture(&mutant));

        let mut mutant = valid;
        mutant
            .docs_by_id
            .get_mut(&1)
            .expect("doc")
            .folded_indexed_text = "wrong fold".to_string();
        assert!(!verify_fixture(&mutant));
    }
}
