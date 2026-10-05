//! Owner-local exact live BM25 regressions.

#![expect(
    clippy::panic_in_result_fn,
    reason = "Independent test assertions report mismatches; Result carries native setup errors."
)]

use std::collections::BTreeMap;

use tantivy::collector::TopDocs;
use tantivy::indexer::NoMergePolicy;
use tantivy::query::{Bm25StatisticsProvider as _, PhraseQuery, TermQuery};
use tantivy::schema::{IndexRecordOption, TantivyDocument, Value as _};
use tantivy::{Index, Term};

use super::codec::Wire;
use super::{LiveBm25Provider, LiveBm25Statistics};
use crate::SchemaFields;

fn add_doc(
    writer: &tantivy::IndexWriter,
    fields: &SchemaFields,
    id: &str,
    text: &str,
    symbol: &str,
    authority: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut doc = TantivyDocument::new();
    doc.add_text(fields.candidate_id, id);
    doc.add_text(fields.chunk_text, text);
    doc.add_text(fields.symbol_local_name, symbol);
    doc.add_text(fields.symbol_component_folded, symbol);
    doc.add_u64(fields.text_authority_doc_id, authority);
    crate::doc_census::attach(writer.index(), fields, &mut doc)?;
    let _op = writer.add_document(doc)?;
    Ok(())
}

fn scored(
    index: &Index,
    fields: &SchemaFields,
    statistics: Option<&LiveBm25Statistics>,
    query: &dyn tantivy::query::Query,
) -> Result<BTreeMap<String, u32>, Box<dyn std::error::Error>> {
    let reader = index.reader()?;
    let searcher = reader.searcher();
    let collector = TopDocs::with_limit(8);
    let hits = if let Some(statistics) = statistics {
        let provider = LiveBm25Provider {
            statistics,
            searcher: &searcher,
        };
        searcher.search_with_statistics_provider(query, &collector, &provider)?
    } else {
        searcher.search(query, &collector)?
    };
    let mut rows = BTreeMap::new();
    for (score, address) in hits {
        let doc: TantivyDocument = searcher.doc(address)?;
        let id = doc
            .get_first(fields.candidate_id)
            .and_then(|value| value.as_str())
            .ok_or("candidate id missing")?;
        let _previous = rows.insert(id.to_owned(), score.to_bits());
    }
    Ok(rows)
}

#[test]
fn retained_deletes_match_independent_fresh_native_statistics_and_score_bits()
-> Result<(), Box<dyn std::error::Error>> {
    let fields = SchemaFields::build();
    let delta_dir = tempfile::tempdir()?;
    let delta_root = delta_dir.path().canonicalize()?;
    let fresh_dir = tempfile::tempdir()?;
    let fresh_root = fresh_dir.path().canonicalize()?;
    let delta = crate::index_store::open_or_create_index(&fields, &delta_root)?;
    let mut writer = delta.writer_with_num_threads(1, 50_000_000)?;
    writer.set_merge_policy(Box::new(NoMergePolicy));
    add_doc(
        &writer,
        &fields,
        "retired",
        "needle needle retired",
        "old",
        1,
    )?;
    add_doc(&writer, &fields, "one", "needle shared", "new", 2)?;
    add_doc(&writer, &fields, "two", "needle needle shared", "other", 3)?;
    let _commit = writer.commit()?;
    let _deleted = writer.delete_term(Term::from_field_text(fields.candidate_id, "retired"));
    let _commit = writer.commit()?;
    writer.wait_merging_threads()?;

    let fresh = crate::index_store::open_or_create_index(&fields, &fresh_root)?;
    let mut fresh_writer = fresh.writer_with_num_threads(1, 50_000_000)?;
    fresh_writer.set_merge_policy(Box::new(NoMergePolicy));
    add_doc(&fresh_writer, &fields, "one", "needle shared", "new", 2)?;
    add_doc(
        &fresh_writer,
        &fields,
        "two",
        "needle needle shared",
        "other",
        3,
    )?;
    let _commit = fresh_writer.commit()?;
    fresh_writer.wait_merging_threads()?;

    let digest = [9_u8; 32];
    let encoded =
        LiveBm25Statistics::build(&delta, &fields, digest, &delta_root, None, &[])?.encode()?;
    let reader = delta.reader()?;
    let searcher = reader.searcher();
    let [retained] = searcher.segment_readers() else {
        return Err("retained-delete fixture must have exactly one segment".into());
    };
    assert_eq!(retained.max_doc(), 3);
    assert_eq!(retained.num_docs(), 2);
    assert!(retained.is_deleted(0));
    let stats = LiveBm25Statistics::decode(&encoded, &delta_root, digest, &searcher)?;
    let fresh_reader = fresh.reader()?;
    let fresh_searcher = fresh_reader.searcher();
    let provider = LiveBm25Provider {
        statistics: &stats,
        searcher: &searcher,
    };
    for (field, fixed_source_tokens) in [
        (fields.chunk_text, 5),
        (fields.symbol_local_name, 2),
        (fields.symbol_component_folded, 2),
        (fields.candidate_id, 2),
        (fields.text_authority_doc_id, 2),
    ] {
        assert_eq!(provider.total_num_tokens(field)?, fixed_source_tokens);
    }
    assert_eq!(fresh_searcher.total_num_tokens(fields.chunk_text)?, 5);
    assert_eq!(provider.total_num_docs()?, 2);
    let retired_term = Term::from_field_text(fields.chunk_text, "retired");
    let repeated_term = Term::from_field_text(fields.chunk_text, "needle");
    assert_eq!(provider.doc_freq(&retired_term)?, 0);
    assert_eq!(provider.doc_freq(&repeated_term)?, 2);
    assert_eq!(searcher.doc_freq(&repeated_term)?, 3);
    assert_eq!(
        provider.doc_freq(&Term::from_field_text(fields.symbol_local_name, "old"))?,
        0
    );
    assert_eq!(
        provider.doc_freq(&Term::from_field_u64(fields.text_authority_doc_id, 1))?,
        0
    );
    let queries: [Box<dyn tantivy::query::Query>; 2] = [
        Box::new(TermQuery::new(repeated_term, IndexRecordOption::WithFreqs)),
        Box::new(PhraseQuery::new(vec![
            Term::from_field_text(fields.chunk_text, "needle"),
            Term::from_field_text(fields.chunk_text, "shared"),
        ])),
    ];
    for query in queries {
        assert_eq!(
            scored(&delta, &fields, Some(&stats), query.as_ref())?,
            scored(&fresh, &fields, None, query.as_ref())?
        );
    }
    Ok(())
}

#[test]
fn all_tombstoned_segment_and_empty_full_have_zero_live_statistics()
-> Result<(), Box<dyn std::error::Error>> {
    let fields = SchemaFields::build();
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    let index = crate::index_store::open_or_create_index(&fields, &root)?;
    let mut writer = index.writer(50_000_000)?;
    writer.set_merge_policy(Box::new(NoMergePolicy));
    add_doc(&writer, &fields, "gone", "needle", "", 1)?;
    let _commit = writer.commit()?;
    let _deleted = writer.delete_term(Term::from_field_text(fields.candidate_id, "gone"));
    let _commit = writer.commit()?;
    writer.wait_merging_threads()?;
    let digest = [3_u8; 32];
    let bytes = LiveBm25Statistics::build(&index, &fields, digest, &root, None, &[])?.encode()?;
    let reader = index.reader()?;
    let searcher = reader.searcher();
    let statistics = LiveBm25Statistics::decode(&bytes, &root, digest, &searcher)?;
    let provider = LiveBm25Provider {
        statistics: &statistics,
        searcher: &searcher,
    };
    assert_eq!(provider.total_num_docs()?, 0);
    assert_eq!(provider.total_num_tokens(fields.chunk_text)?, 0);
    assert_eq!(
        provider.doc_freq(&Term::from_field_text(fields.chunk_text, "needle"))?,
        0
    );
    assert_eq!(
        provider.doc_freq(&Term::from_field_text(fields.symbol_local_name, ""))?,
        0
    );
    assert!(
        scored(
            &index,
            &fields,
            Some(&statistics),
            &TermQuery::new(
                Term::from_field_text(fields.chunk_text, "needle"),
                IndexRecordOption::WithFreqs
            )
        )?
        .is_empty()
    );

    let empty_dir = tempfile::tempdir()?;
    let empty_root = empty_dir.path().canonicalize()?;
    let empty = crate::index_store::open_or_create_index(&fields, &empty_root)?;
    let bytes =
        LiveBm25Statistics::build(&empty, &fields, digest, &empty_root, None, &[])?.encode()?;
    let reader = empty.reader()?;
    let searcher = reader.searcher();
    let statistics = LiveBm25Statistics::decode(&bytes, &empty_root, digest, &searcher)?;
    assert_eq!(statistics.live_docs, 0);
    assert!(statistics.segments.is_empty());
    let provider = LiveBm25Provider {
        statistics: &statistics,
        searcher: &searcher,
    };
    for field in [
        fields.chunk_text,
        fields.candidate_id,
        fields.text_authority_doc_id,
    ] {
        assert_eq!(provider.total_num_tokens(field)?, 0);
    }
    assert!(
        provider
            .total_num_tokens(fields.live_bm25_doc_census)
            .is_err()
    );
    Ok(())
}

#[test]
fn committed_row_rejects_cap_and_structural_mutations() -> Result<(), Box<dyn std::error::Error>> {
    let fields = SchemaFields::build();
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    let index = crate::index_store::open_or_create_index(&fields, &root)?;
    let mut writer = index.writer(50_000_000)?;
    add_doc(&writer, &fields, "one", "needle", "symbol", 1)?;
    let _commit = writer.commit()?;
    writer.wait_merging_threads()?;
    let reader = index.reader()?;
    let searcher = reader.searcher();
    let digest = [1_u8; 32];
    let raw = LiveBm25Statistics::build(&index, &fields, digest, &root, None, &[])?.encode()?;
    let mut row: Wire = crate::channel_payloads::decode_cbor_exact(&raw)?;
    row.3.first_mut().ok_or("missing segment")?.0.3 = Some(999);
    let tampered = crate::channel_payloads::encode_cbor(&row, "test")?;
    assert!(LiveBm25Statistics::decode(&tampered, &root, digest, &searcher).is_err());
    let mut row: Wire = crate::channel_payloads::decode_cbor_exact(&raw)?;
    row.3.first_mut().ok_or("missing segment")?.2.push((
        fields.chunk_text.field_id(),
        Term::from_field_text(fields.chunk_text, "needle")
            .serialized_value_bytes()
            .to_vec(),
        100,
    ));
    let tampered = crate::channel_payloads::encode_cbor(&row, "test")?;
    assert!(LiveBm25Statistics::decode(&tampered, &root, digest, &searcher).is_err());
    let oversized = vec![0_u8; super::MAX_BYTES + 1];
    assert!(LiveBm25Statistics::decode(&oversized, &root, digest, &searcher).is_err());
    Ok(())
}
