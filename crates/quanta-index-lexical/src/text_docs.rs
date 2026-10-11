//! Text-authority doc ids: allocating them and reading them back from stored documents.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::SchemaFields;
use quanta_index_contract::SourceFileKey;
use quanta_index_core::CoreError;
use tantivy::Term;
use tantivy::query::{BooleanQuery, Occur, TermQuery};
use tantivy::schema::{IndexRecordOption, TantivyDocument, Value};

/// The text-authority doc id stored on a text document.
///
/// Every text document this adapter writes carries one; a text document
/// without it belongs to a generation built before doc ids were stored,
/// which the sealed-manifest format refuses. It is an error here too, so
/// an unsealed such generation cannot be continued into an authority
/// that cannot name what it retires.
/// Multiple values, malformed values and IDs outside the writer's domain are
/// corrupt routing authority; choosing the first value is not a valid fallback.
pub(crate) fn stored_text_authority_doc_id(
    doc: &TantivyDocument,
    fields: &SchemaFields,
    candidate_id: &str,
) -> Result<u64, CoreError> {
    let mut values = doc.get_all(fields.text_authority_doc_id);
    let value = values.next().ok_or_else(|| CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationTextAuthorityFormatUnsupported,
            message: format!(
                "lexical: text document {candidate_id} stores no text-authority doc id; the generation predates the sharded text authority and must be rebuilt"
            ),
        })?;
    if values.next().is_some() {
        return Err(CoreError::Storage(format!(
            "lexical: text document {candidate_id} stores multiple text-authority doc ids"
        )));
    }
    let doc_id = Value::as_u64(&value).ok_or_else(|| {
        CoreError::Storage(format!(
            "lexical: text document {candidate_id} stores a malformed text-authority doc id"
        ))
    })?;
    if doc_id == 0 {
        return Err(CoreError::Storage(format!(
            "lexical: text document {candidate_id} stores text-authority doc id {doc_id} outside the allocated domain"
        )));
    }
    Ok(doc_id)
}

/// A text-authority doc id as a restriction member.
///
/// Zero is reserved for the absence of a text document. Logical IDs remain
/// u64 through the query bridge; only Tantivy's segment-local IDs are u32.
pub(crate) fn authority_member(doc_id: u64, surface: &str) -> Result<u64, CoreError> {
    if doc_id == 0 {
        return Err(CoreError::Storage(format!(
            "lexical: {surface} names reserved text-authority doc id zero"
        )));
    }
    Ok(doc_id)
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

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "source-authority regressions assert fixed decoding invariants"
)]
mod l4_authority_id_regressions {
    use super::{SchemaFields, stored_text_authority_doc_id};
    use crate::documents::required_stored_text;
    use tantivy::schema::TantivyDocument;

    #[test]
    fn stored_authority_id_rejects_ambiguity_and_invalid_domain() {
        let fields = SchemaFields::build();
        for ids in [vec![1, 2], vec![2, 1], vec![1, 1], vec![0]] {
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
        for id in [1, u64::from(u32::MAX), u64::MAX] {
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
    fn stored_authority_id_refuses_duplicate_ids() {
        let fields = SchemaFields::build();
        let mut doc = TantivyDocument::new();
        doc.add_text(fields.doc_kind, crate::TEXT_DOC_KIND);
        doc.add_text(fields.candidate_id, "candidate");
        doc.add_text(fields.chunk_text, "needle");
        doc.add_u64(fields.text_authority_doc_id, 1);
        doc.add_u64(fields.text_authority_doc_id, 2);
        assert!(
            stored_text_authority_doc_id(&doc, &fields, "candidate").is_err(),
            "stored decoding silently chose the first ID"
        );
    }

    #[test]
    fn stored_text_refuses_ambiguous_required_fields() {
        let fields = SchemaFields::build();
        for duplicate in [fields.candidate_id, fields.chunk_text] {
            let mut doc = TantivyDocument::new();
            doc.add_text(fields.doc_kind, crate::TEXT_DOC_KIND);
            doc.add_text(fields.candidate_id, "candidate");
            doc.add_text(fields.chunk_text, "needle");
            doc.add_u64(fields.text_authority_doc_id, 1);
            doc.add_text(duplicate, "conflicting");
            assert!(
                required_stored_text(&doc, duplicate, "required").is_err(),
                "stored decoding accepted a duplicate required field"
            );
        }
    }
}
