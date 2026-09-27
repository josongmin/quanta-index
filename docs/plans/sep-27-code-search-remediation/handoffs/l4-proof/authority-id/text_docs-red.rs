//! Text-authority doc ids: allocating them and reading them back from stored documents.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::documents::stored_text;
use crate::text_authority::AddedTextDoc;
use crate::{IndexedTextDoc, SchemaFields, TEXT_DOC_KIND};
use quanta_index_contract::SourceFileKey;
use quanta_index_core::CoreError;
use roaring::RoaringBitmap;
use tantivy::collector::TopDocs;
use tantivy::query::{AllQuery, BooleanQuery, Occur, TermQuery};
use tantivy::schema::{IndexRecordOption, TantivyDocument, Value};
use tantivy::{Index, IndexReader, ReloadPolicy, Term};

/// The text-authority doc id stored on a text document.
///
/// Every text document this adapter writes carries one; a text document
/// without it belongs to a generation built before doc ids were stored,
/// which the sealed-manifest format refuses. It is an error here too, so
/// an unsealed such generation cannot be continued into an authority
/// that cannot name what it retires.
pub(crate) fn stored_text_authority_doc_id(
    doc: &TantivyDocument,
    fields: &SchemaFields,
    candidate_id: &str,
) -> Result<u64, CoreError> {
    doc.get_first(fields.text_authority_doc_id)
        .and_then(|value| Value::as_u64(&value))
        .ok_or_else(|| CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationTextAuthorityFormatUnsupported,
            message: format!(
                "lexical: text document {candidate_id} stores no text-authority doc id; the generation predates the sharded text authority and must be rebuilt"
            ),
        })
}

/// A text-authority doc id as a restriction member.
///
/// The authority allocates ids in `1..=MAX_DOC_ID`, the 32-bit range, so an
/// id outside it is a corrupt authority, refused rather than dropped.
pub(crate) fn authority_member(doc_id: u64, surface: &str) -> Result<u32, CoreError> {
    u32::try_from(doc_id).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: {surface} names text-authority doc id {doc_id} outside the 32-bit id space: {err}"
        ))
    })
}

/// Text-authority doc ids as the restriction set they form.
pub(crate) fn authority_member_set(
    doc_ids: impl IntoIterator<Item = u64>,
    surface: &str,
) -> Result<RoaringBitmap, CoreError> {
    let mut members = RoaringBitmap::new();
    for doc_id in doc_ids {
        let _inserted: bool = members.insert(authority_member(doc_id, surface)?);
    }
    Ok(members)
}

/// Exact source ownership is the conjunction of two separately indexed terms;
/// no delimiter-based identity encoding can alias another repository or path.
pub(crate) fn source_file_query(fields: &SchemaFields, file: &SourceFileKey) -> BooleanQuery {
    BooleanQuery::new(vec![
        (
            Occur::Must,
            Box::new(TermQuery::new(
                Term::from_field_text(fields.repo_id, file.source_repo_id.as_str()),
                IndexRecordOption::Basic,
            )),
        ),
        (
            Occur::Must,
            Box::new(TermQuery::new(
                Term::from_field_text(fields.repo_relative_path, file.repo_relative_path.as_str()),
                IndexRecordOption::Basic,
            )),
        ),
    ])
}

/// The text documents currently indexed under one path, with their doc ids.
///
/// Read from the committed index before a scope mutation is applied, so an
/// incremental text-authority update knows exactly which documents — and
/// therefore which shards — the mutation retires.
pub(crate) fn text_candidates_at_file(
    index: &Index,
    fields: &SchemaFields,
    file: &SourceFileKey,
) -> Result<Vec<IndexedTextDoc>, CoreError> {
    let reader: IndexReader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()
        .map_err(|err| CoreError::Storage(format!("lexical: scope candidate reader: {err}")))?;
    reader.reload().map_err(|err| {
        CoreError::Storage(format!("lexical: scope candidate reader reload: {err}"))
    })?;
    let searcher = reader.searcher();
    let query = source_file_query(fields, file);
    let limit = usize::try_from(searcher.num_docs()).map_err(|err| {
        CoreError::InvalidContract(format!(
            "lexical: num_docs overflow while collecting scope candidates: {err}"
        ))
    })?;
    if limit == 0 {
        return Ok(Vec::new());
    }
    let hits = searcher
        .search(&query, &TopDocs::with_limit(limit))
        .map_err(|err| CoreError::Storage(format!("lexical: scope candidate scan: {err}")))?;
    let mut candidates = Vec::with_capacity(hits.len());
    for (_score, doc_address) in hits {
        let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: fetch scope candidate doc {doc_address:?}: {err}"
            ))
        })?;
        if stored_text(&doc, fields.doc_kind).as_deref() != Some(TEXT_DOC_KIND) {
            continue;
        }
        let candidate_id = stored_text(&doc, fields.candidate_id).ok_or_else(|| {
            CoreError::Storage(
                "lexical: scope candidate doc missing candidate_id field".to_string(),
            )
        })?;
        let doc_id = stored_text_authority_doc_id(&doc, fields, &candidate_id)?;
        candidates.push(IndexedTextDoc {
            candidate_id,
            doc_id,
        });
    }
    Ok(candidates)
}

/// Every live text document with its doc id and stored text, in doc-id
/// order: the input of a full text-authority rebuild.
pub(crate) fn collect_text_authority_docs(
    index: &Index,
    fields: &SchemaFields,
) -> Result<Vec<AddedTextDoc>, CoreError> {
    let reader: IndexReader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()
        .map_err(|err| CoreError::Storage(format!("lexical: text authority reader: {err}")))?;
    reader.reload().map_err(|err| {
        CoreError::Storage(format!("lexical: text authority reader reload: {err}"))
    })?;
    let searcher = reader.searcher();
    let limit = usize::try_from(searcher.num_docs()).map_err(|err| {
        CoreError::InvalidContract(format!(
            "lexical: num_docs overflow while rebuilding text authority: {err}"
        ))
    })?;
    if limit == 0 {
        return Ok(Vec::new());
    }
    let hits = searcher
        .search(&AllQuery, &TopDocs::with_limit(limit))
        .map_err(|err| CoreError::Storage(format!("lexical: text authority scan: {err}")))?;
    let mut docs: Vec<AddedTextDoc> = Vec::new();
    for (_score, doc_address) in hits {
        let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: fetch text authority doc {doc_address:?}: {err}"
            ))
        })?;
        if stored_text(&doc, fields.doc_kind).as_deref() != Some(TEXT_DOC_KIND) {
            continue;
        }
        let candidate_id = stored_text(&doc, fields.candidate_id).ok_or_else(|| {
            CoreError::Storage("lexical: text authority doc missing candidate_id field".to_string())
        })?;
        let text = stored_text(&doc, fields.chunk_text).ok_or_else(|| {
            CoreError::Storage("lexical: text authority doc missing chunk_text field".to_string())
        })?;
        let doc_id = stored_text_authority_doc_id(&doc, fields, &candidate_id)?;
        docs.push(AddedTextDoc {
            doc_id,
            candidate_id,
            text,
        });
    }
    docs.sort_by_key(|doc| doc.doc_id);
    Ok(docs)
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "source-authority regressions assert fixed decoding invariants"
)]
mod l4_authority_id_regressions {
    use super::{SchemaFields, collect_text_authority_docs, stored_text_authority_doc_id};
    use tantivy::Index;
    use tantivy::schema::TantivyDocument;

    #[test]
    fn stored_authority_id_rejects_ambiguity_and_invalid_domain() {
        let fields = SchemaFields::build();
        for ids in [vec![1, 2], vec![2, 1], vec![1, 1], vec![0], vec![u64::MAX]] {
            let mut doc = TantivyDocument::new();
            for id in &ids {
                doc.add_u64(fields.text_authority_doc_id, *id);
            }
            assert!(
                stored_text_authority_doc_id(&doc, &fields, "candidate").is_err(),
                "accepted ambiguous or invalid authority IDs: {ids:?}"
            );
        }
        let mut malformed = TantivyDocument::new();
        malformed.add_u64(fields.text_authority_doc_id, 1);
        malformed.add_text(fields.text_authority_doc_id, "not-an-id");
        assert!(stored_text_authority_doc_id(&malformed, &fields, "candidate").is_err());
        assert!(
            stored_text_authority_doc_id(&TantivyDocument::new(), &fields, "candidate").is_err()
        );
    }

    #[test]
    fn stored_authority_id_accepts_single_valid_boundary_values()
    -> Result<(), quanta_index_core::CoreError> {
        let fields = SchemaFields::build();
        for id in [1, u64::from(u32::MAX)] {
            let mut doc = TantivyDocument::new();
            doc.add_u64(fields.text_authority_doc_id, id);
            assert_eq!(
                stored_text_authority_doc_id(&doc, &fields, "candidate")?,
                id
            );
        }
        Ok(())
    }

    #[test]
    fn authority_rebuild_refuses_duplicate_ids_from_stored_index()
    -> Result<(), Box<dyn std::error::Error>> {
        let fields = SchemaFields::build();
        let index = Index::create_in_ram(fields.schema.clone());
        crate::analyzer::register_analyzers(&index);
        let mut writer = index.writer_with_num_threads(1, 15_000_000)?;
        let mut doc = TantivyDocument::new();
        doc.add_text(fields.doc_kind, crate::TEXT_DOC_KIND);
        doc.add_text(fields.candidate_id, "candidate");
        doc.add_text(fields.chunk_text, "needle");
        doc.add_u64(fields.text_authority_doc_id, 1);
        doc.add_u64(fields.text_authority_doc_id, 2);
        let _operation = writer.add_document(doc)?;
        let _commit = writer.commit()?;
        assert!(
            collect_text_authority_docs(&index, &fields).is_err(),
            "stored decoding and authority rebuild silently chose the first ID"
        );
        Ok(())
    }
}
