//! Text-document restriction by text-authority doc id (QI-BB-024).
//!
//! The byte and token surfaces — regex, raw substring, phrase — answer
//! from the text authority, and the gates that resolve to text documents —
//! file owner, file contributor, scoped content — answer from the stored
//! documents. Each answer is a set of text-authority doc ids held as a
//! compressed bitmap of 32-bit ids: two bytes a member where the ids are
//! sparse, one bit a member where they are dense, shared by `Arc` from the
//! regex match cache to every query that restricts by it.
//!
//! [`AuthorityDocSetQuery`] restricts the index to those documents through
//! the doc id every text document carries beside its fields — indexed for
//! a lookup, a fast column for a scan — so nothing is built per member: no
//! candidate-id string, no term query. Per segment the cheaper of the two
//! walks is chosen from the set's size against the segment's (see
//! [`restriction_walk`]); both yield exactly the documents whose stored doc
//! id is a member, which the tests prove against each other and against
//! the stored fields.
//!
//! A restriction matches; it does not rank. Every document it admits
//! scores the query's boost, as the index's own match-only queries (regex,
//! range, exists) do.

use std::fmt;
use std::sync::Arc;

use roaring::RoaringBitmap;
use tantivy::fastfield::Column;
use tantivy::query::{ConstScorer, EnableScoring, Explanation, Query, Scorer, Weight};
use tantivy::schema::{Field, IndexRecordOption};
use tantivy::{DocId, DocSet, Score, SegmentReader, TERMINATED, TantivyError, Term};

/// One posting lookup costs about as much as scanning this many rows of the
/// doc-id column: a term-dictionary walk plus opening a posting list,
/// against one bit-unpacked column read and one bitmap probe.
const LOOKUP_COST_IN_SCANNED_ROWS: u64 = 16;

/// How a segment finds the members of a restriction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RestrictionWalk {
    /// One posting lookup per member: the set is small next to the segment.
    Lookup,
    /// One pass over the segment's doc-id column: the set is dense.
    Scan,
}

/// The cheaper walk for a set of `members` over a segment of `max_doc`
/// documents.
pub(crate) fn restriction_walk(members: u64, max_doc: DocId) -> RestrictionWalk {
    if members.saturating_mul(LOOKUP_COST_IN_SCANNED_ROWS) <= u64::from(max_doc) {
        RestrictionWalk::Lookup
    } else {
        RestrictionWalk::Scan
    }
}

/// The text documents whose text-authority doc id is a member of a set.
#[derive(Clone)]
pub(crate) struct AuthorityDocSetQuery {
    field: Field,
    members: Arc<RoaringBitmap>,
}

impl AuthorityDocSetQuery {
    /// Restrict to `members` through `field`, the schema's text-authority
    /// doc-id field.
    pub(crate) const fn new(field: Field, members: Arc<RoaringBitmap>) -> Self {
        Self { field, members }
    }
}

impl fmt::Debug for AuthorityDocSetQuery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthorityDocSetQuery")
            .field("field", &self.field)
            .field("members", &self.members.len())
            .finish()
    }
}

impl Query for AuthorityDocSetQuery {
    fn weight(&self, enable_scoring: EnableScoring<'_>) -> tantivy::Result<Box<dyn Weight>> {
        let entry = enable_scoring.schema().get_field_entry(self.field);
        if !entry.is_indexed() || !entry.is_fast() {
            return Err(TantivyError::SchemaError(format!(
                "the text-authority doc-id field `{}` must be indexed and a fast column",
                entry.name()
            )));
        }
        Ok(Box::new(AuthorityDocSetWeight {
            field: self.field,
            field_name: entry.name().to_string(),
            members: Arc::clone(&self.members),
        }))
    }
}

struct AuthorityDocSetWeight {
    field: Field,
    field_name: String,
    members: Arc<RoaringBitmap>,
}

impl AuthorityDocSetWeight {
    fn member_docs(&self, reader: &SegmentReader) -> tantivy::Result<Box<dyn DocSet>> {
        match restriction_walk(self.members.len(), reader.max_doc()) {
            RestrictionWalk::Lookup => Ok(Box::new(MemberDocs::new(member_docs_by_lookup(
                reader,
                self.field,
                &self.members,
            )?))),
            RestrictionWalk::Scan => Ok(Box::new(ColumnScan::new(
                reader.fast_fields().u64(&self.field_name)?,
                Arc::clone(&self.members),
                reader.max_doc(),
            ))),
        }
    }
}

impl Weight for AuthorityDocSetWeight {
    fn scorer(&self, reader: &SegmentReader, boost: Score) -> tantivy::Result<Box<dyn Scorer>> {
        Ok(Box::new(ConstScorer::new(self.member_docs(reader)?, boost)))
    }

    fn explain(&self, reader: &SegmentReader, doc: DocId) -> tantivy::Result<Explanation> {
        let mut scorer = self.scorer(reader, 1.0)?;
        if scorer.doc() > doc || scorer.seek(doc) != doc {
            return Err(TantivyError::InvalidArgument(format!(
                "document {doc} is not a member of the text-authority restriction"
            )));
        }
        Ok(Explanation::new(
            "text-authority doc-id restriction",
            scorer.score(),
        ))
    }
}

/// The segment documents indexed under any member id, one posting lookup
/// per member.
///
/// Deleted documents are included, as every posting list includes them;
/// the collectors drop them against the segment's alive set.
pub(crate) fn member_docs_by_lookup(
    reader: &SegmentReader,
    field: Field,
    members: &RoaringBitmap,
) -> tantivy::Result<RoaringBitmap> {
    let inverted = reader.inverted_index(field)?;
    let mut docs = RoaringBitmap::new();
    let mut term = Term::from_field_u64(field, 0);
    for member in members {
        term.set_u64(u64::from(member));
        let Some(mut postings) = inverted.read_postings(&term, IndexRecordOption::Basic)? else {
            continue;
        };
        let mut doc = postings.doc();
        while doc != TERMINATED {
            let _inserted: bool = docs.insert(doc);
            doc = postings.advance();
        }
    }
    Ok(docs)
}

/// A materialized set of segment documents, walked in order.
pub(crate) struct MemberDocs {
    docs: roaring::bitmap::IntoIter,
    doc: DocId,
    size: u32,
}

impl MemberDocs {
    pub(crate) fn new(docs: RoaringBitmap) -> Self {
        let size = u32::try_from(docs.len()).map_or(u32::MAX, |size| size);
        let mut docs = docs.into_iter();
        let doc = docs.next().unwrap_or(TERMINATED);
        Self { docs, doc, size }
    }
}

impl DocSet for MemberDocs {
    fn advance(&mut self) -> DocId {
        self.doc = self.docs.next().unwrap_or(TERMINATED);
        self.doc
    }

    fn seek(&mut self, target: DocId) -> DocId {
        if self.doc >= target {
            return self.doc;
        }
        self.docs.advance_to(target);
        self.advance()
    }

    fn doc(&self) -> DocId {
        self.doc
    }

    fn size_hint(&self) -> u32 {
        self.size
    }
}

/// A lazy pass over a segment's doc-id column, stopping on each document
/// whose id is a member.
///
/// Nothing is allocated per segment: the walk reads the column where the
/// collector asks for the next document, so a search observes its request
/// budget between members exactly as it does over a posting list.
pub(crate) struct ColumnScan {
    column: Column<u64>,
    members: Arc<RoaringBitmap>,
    max_doc: DocId,
    doc: DocId,
}

impl ColumnScan {
    pub(crate) fn new(column: Column<u64>, members: Arc<RoaringBitmap>, max_doc: DocId) -> Self {
        let mut scan = Self {
            column,
            members,
            max_doc,
            doc: 0,
        };
        scan.doc = scan.first_member_from(0);
        scan
    }

    fn is_member(&self, doc: DocId) -> bool {
        self.column
            .values_for_doc(doc)
            .any(|id| u32::try_from(id).is_ok_and(|id| self.members.contains(id)))
    }

    fn first_member_from(&self, start: DocId) -> DocId {
        (start..self.max_doc)
            .find(|doc| self.is_member(*doc))
            .unwrap_or(TERMINATED)
    }
}

impl DocSet for ColumnScan {
    fn advance(&mut self) -> DocId {
        if self.doc != TERMINATED {
            self.doc = self.first_member_from(self.doc.saturating_add(1));
        }
        self.doc
    }

    fn seek(&mut self, target: DocId) -> DocId {
        if self.doc >= target {
            return self.doc;
        }
        self.doc = self.first_member_from(target);
        self.doc
    }

    fn doc(&self) -> DocId {
        self.doc
    }

    fn size_hint(&self) -> u32 {
        u32::try_from(self.members.len()).map_or(self.max_doc, |members| members.min(self.max_doc))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use roaring::RoaringBitmap;
    use tantivy::collector::TopDocs;
    use tantivy::query::Query;
    use tantivy::schema::{TantivyDocument, Value};
    use tantivy::{DocAddress, DocId, DocSet, Index, IndexWriter, Searcher, TERMINATED, Term};

    use super::{
        AuthorityDocSetQuery, ColumnScan, MemberDocs, RestrictionWalk, member_docs_by_lookup,
        restriction_walk,
    };
    use crate::SchemaFields;

    /// Text documents carry authority ids 1..=`texts`; symbols carry none.
    /// Two commits make two segments, and a delete leaves a dead document
    /// whose id is still in the first segment's postings and column.
    fn corpus(texts: u64, symbols: u64) -> (Index, SchemaFields) {
        let fields = SchemaFields::build();
        let index = Index::create_in_ram(fields.schema.clone());
        crate::index_store::register_index_tokenizers(&index);
        let mut writer: IndexWriter = index.writer(15_000_000).expect("writer");
        let half = texts.div_euclid(2);
        for id in 1..=texts {
            let mut doc = TantivyDocument::new();
            doc.add_u64(fields.text_authority_doc_id, id);
            doc.add_text(fields.candidate_id, format!("chunk-{id}"));
            doc.add_text(fields.doc_kind, "text");
            let _opstamp = writer.add_document(doc).expect("add");
            if id == half {
                let _opstamp = writer.commit().expect("first segment");
            }
        }
        for symbol in 0..symbols {
            let mut doc = TantivyDocument::new();
            doc.add_text(fields.candidate_id, format!("symbol-{symbol}"));
            doc.add_text(fields.doc_kind, "symbol");
            let _opstamp = writer.add_document(doc).expect("add");
        }
        let _opstamp = writer.commit().expect("second segment");
        let _opstamp = writer.delete_term(Term::from_field_text(fields.candidate_id, "chunk-2"));
        let _opstamp = writer.commit().expect("delete");
        (index, fields)
    }

    fn searcher(index: &Index) -> Searcher {
        index.reader().expect("reader").searcher()
    }

    /// Per segment, the documents — dead ones too — whose stored id is a
    /// member: the oracle, read from the stored fields the walks never
    /// touch.
    fn stored_members(
        searcher: &Searcher,
        fields: &SchemaFields,
        members: &RoaringBitmap,
    ) -> Vec<BTreeSet<DocId>> {
        searcher
            .segment_readers()
            .iter()
            .map(|reader| {
                let store = reader.get_store_reader(1).expect("store");
                (0..reader.max_doc())
                    .filter(|doc| {
                        let stored: TantivyDocument = store.get(*doc).expect("stored doc");
                        stored
                            .get_first(fields.text_authority_doc_id)
                            .and_then(|value| value.as_u64())
                            .is_some_and(|id| {
                                u32::try_from(id).is_ok_and(|id| members.contains(id))
                            })
                    })
                    .collect()
            })
            .collect()
    }

    fn drain(mut docs: impl DocSet) -> BTreeSet<DocId> {
        let mut out = BTreeSet::new();
        let mut doc = docs.doc();
        while doc != TERMINATED {
            assert!(out.insert(doc), "a walk yields each document once");
            doc = docs.advance();
        }
        out
    }

    fn member_sets() -> Vec<RoaringBitmap> {
        vec![
            RoaringBitmap::new(),
            std::iter::once(7).collect(),
            std::iter::once(2).collect(),
            [1, 3, 40, 41, 64, 999].into_iter().collect(),
            (1..=64).collect(),
            (100..200).collect(),
        ]
    }

    #[test]
    fn the_cheaper_walk_is_chosen_from_the_set_against_the_segment() {
        assert_eq!(restriction_walk(0, 0), RestrictionWalk::Lookup);
        assert_eq!(restriction_walk(1, 16), RestrictionWalk::Lookup);
        assert_eq!(restriction_walk(1, 15), RestrictionWalk::Scan);
        assert_eq!(restriction_walk(62_500, 1_000_000), RestrictionWalk::Lookup);
        assert_eq!(restriction_walk(62_501, 1_000_000), RestrictionWalk::Scan);
        assert_eq!(restriction_walk(u64::MAX, u32::MAX), RestrictionWalk::Scan);
    }

    /// Both walks find exactly the documents whose stored id is a member —
    /// symbols (no id), dead documents and ids no document carries
    /// included — segment by segment.
    #[test]
    fn both_walks_agree_with_the_stored_ids() {
        let (index, fields) = corpus(64, 5);
        let searcher = searcher(&index);
        assert_eq!(searcher.segment_readers().len(), 2);
        for members in member_sets() {
            let oracle = stored_members(&searcher, &fields, &members);
            let shared = Arc::new(members.clone());
            for (reader, expected) in searcher.segment_readers().iter().zip(&oracle) {
                let looked_up = drain(MemberDocs::new(
                    member_docs_by_lookup(reader, fields.text_authority_doc_id, &members)
                        .expect("lookup"),
                ));
                let scanned = drain(ColumnScan::new(
                    reader
                        .fast_fields()
                        .u64("text_authority_doc_id")
                        .expect("column"),
                    Arc::clone(&shared),
                    reader.max_doc(),
                ));
                assert_eq!(&looked_up, expected, "lookup over {members:?}");
                assert_eq!(&scanned, expected, "scan over {members:?}");
            }
        }
    }

    /// Through a searcher, the query admits the live documents whose stored
    /// id is a member, every one at the query's constant score.
    #[test]
    fn the_query_admits_exactly_the_live_members_at_a_constant_score() {
        let (index, fields) = corpus(64, 5);
        let searcher = searcher(&index);
        for members in member_sets() {
            let oracle: BTreeSet<DocAddress> = stored_members(&searcher, &fields, &members)
                .iter()
                .zip(searcher.segment_readers())
                .enumerate()
                .flat_map(|(segment, (docs, reader))| {
                    let segment = u32::try_from(segment).expect("segment ordinal");
                    docs.iter()
                        .filter(|doc| {
                            reader
                                .alive_bitset()
                                .is_none_or(|alive| alive.is_alive(**doc))
                        })
                        .map(move |doc| DocAddress::new(segment, *doc))
                        .collect::<Vec<_>>()
                })
                .collect();
            let query = AuthorityDocSetQuery::new(fields.text_authority_doc_id, Arc::new(members));
            let hits = searcher
                .search(&query, &TopDocs::with_limit(1_000))
                .expect("search");
            assert!(
                hits.iter()
                    .all(|(score, _)| (*score - 1.0).abs() < f32::EPSILON)
            );
            let admitted: BTreeSet<DocAddress> = hits.into_iter().map(|(_, doc)| doc).collect();
            assert_eq!(admitted, oracle);
        }
    }

    /// Seek lands on the first member at or past the target, on both walks,
    /// and explain names a member and refuses a non-member.
    #[test]
    fn seek_and_explain_follow_membership() {
        let (index, fields) = corpus(64, 0);
        let searcher = searcher(&index);
        // Members in both commits' halves, never two adjacent ids, so every
        // segment holds several and the document after one is not one.
        let members: RoaringBitmap = [5, 9, 30, 40, 45, 60].into_iter().collect();
        let oracle = stored_members(&searcher, &fields, &members);
        let weight =
            AuthorityDocSetQuery::new(fields.text_authority_doc_id, Arc::new(members.clone()))
                .weight(tantivy::query::EnableScoring::disabled_from_searcher(
                    &searcher,
                ))
                .expect("weight");
        for (reader, expected) in searcher.segment_readers().iter().zip(&oracle) {
            assert!(
                expected.len() >= 2,
                "every segment holds members: {oracle:?}"
            );
            let first = *expected.first().expect("a member");
            let last = *expected.last().expect("a member");
            assert!(!expected.contains(&(first + 1)));
            let mut looked_up = MemberDocs::new(
                member_docs_by_lookup(reader, fields.text_authority_doc_id, &members)
                    .expect("lookup"),
            );
            let mut scanned = ColumnScan::new(
                reader
                    .fast_fields()
                    .u64("text_authority_doc_id")
                    .expect("column"),
                Arc::new(members.clone()),
                reader.max_doc(),
            );
            let walks: [&mut dyn DocSet; 2] = [&mut looked_up, &mut scanned];
            for walk in walks {
                assert_eq!(walk.doc(), first);
                assert_eq!(walk.seek(first), first, "seek to the current doc stays");
                assert_eq!(
                    walk.seek(first + 1),
                    *expected.range(first + 1..).next().expect("next")
                );
                assert_eq!(walk.seek(last + 1), TERMINATED);
                assert_eq!(walk.advance(), TERMINATED, "a finished walk stays finished");
            }
            assert!(weight.explain(reader, first).is_ok());
            assert!(weight.explain(reader, first + 1).is_err());
        }
    }

    /// A schema whose doc-id field is not indexed and a fast column is
    /// refused when the weight is built, before any segment is read.
    #[test]
    fn the_query_refuses_a_field_it_cannot_walk() {
        let fields = SchemaFields::build();
        let index = Index::create_in_ram(fields.schema.clone());
        let searcher = searcher(&index);
        let refused = AuthorityDocSetQuery::new(fields.doc_kind, Arc::new(RoaringBitmap::new()))
            .weight(tantivy::query::EnableScoring::disabled_from_searcher(
                &searcher,
            ));
        assert!(refused.is_err());
    }
}
