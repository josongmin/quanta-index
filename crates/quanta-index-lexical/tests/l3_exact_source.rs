//! Explicit name/source contracts through the real sealed adapter. Fixed IDs
//! and cardinalities come from the fixture, never a full-search baseline.
#![forbid(unsafe_code)]

use quanta_index_contract::lex::{
    LanguageCode, SymbolKindCode, SymbolRecord, SymbolRelationship, SymbolSpan,
};
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, LQ_VERSION_TAG, LqCase, LqExpr, LqLeaf, LqOptions,
    LqPredicateArg, LqQuery, LqSpan, LqYesNoOnly, ManifestGeneration, RepoId, RepoRelativePath,
    RevisionId, SearchCorpusIngestBatch, SearchCorpusReplaceScope, SourceFileCoverage,
    SourceFileKey, SourceFileRevision, SourcePublicationEvent, SymbolCoverage, SymbolId,
    source_event_payload_sha256, source_file_unit_set_sha256,
};
use quanta_index_core::{
    LexicalIndexOpenPort, LexicalSearcher, RequestBudgetV1, SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;
use sha2::{Digest as _, Sha256};
use std::collections::BTreeSet;
use std::error::Error;

type TestResult = Result<(), Box<dyn Error>>;

fn scope(
    owner: &str,
    definitions: &[(&str, &str, &str, Option<&str>)],
) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let language = LanguageCode::new("rust").map_err(str::to_string)?;
    let text = "content_only Café One::Café Café::Inner";
    let path = RepoRelativePath::new("same.rs");
    let chunks = vec![ChunkRecord {
        chunk_id: ChunkId::new(format!("chunk-{owner}")),
        repo_relative_path: path.clone(),
        language: language.clone(),
        start_byte: 0,
        end_byte: u32::try_from(text.len())?,
        start_line: 1,
        end_line: 1,
        text: text.into(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: Some(RepoId::new(owner)?),
    }];
    let symbols = definitions
        .iter()
        .map(|(id, local, qualified, container)| {
            Ok(SymbolRecord {
                symbol_id: SymbolId::new(*id),
                repo_relative_path: path.clone(),
                language: language.clone(),
                symbol_kind: SymbolKindCode::new("function").map_err(str::to_string)?,
                symbol_kind_family: None,
                local_name: (*local).into(),
                qualified_name: (*qualified).into(),
                signature: Some("fn()".into()),
                visibility: None,
                definition_span: SymbolSpan {
                    path: "same.rs".into(),
                    byte_start: 0,
                    byte_end: u32::try_from(text.len())?,
                    line_start: 1,
                    line_end: 1,
                },
                container_qualified_name: container.map(Into::into),
                relationship: SymbolRelationship::Def,
            })
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    Ok(SearchCorpusReplaceScope {
        coverage: SourceFileCoverage {
            source: SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new(owner)?,
                    repo_relative_path: path,
                },
                revision_id: RevisionId::new(format!("rev-{owner}"))?,
                source_sha256: Sha256::digest(text.as_bytes()).into(),
            },
            language,
            producer_policy_sha256: [3; 32],
            unit_set_sha256: source_file_unit_set_sha256(&chunks, &symbols)?,
            text_admitted: true,
            symbols: SymbolCoverage::Complete {
                symbol_count: u64::try_from(symbols.len())?,
            },
        },
        chunks,
        symbols,
    })
}

fn fixture() -> Result<(tempfile::TempDir, Box<dyn LexicalSearcher>), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let repo = RepoId::new("containing-snapshot")?;
    let revision = RevisionId::new("snapshot-rev")?;
    let mut batch = SearchCorpusIngestBatch {
        source_event: SourcePublicationEvent {
            stream_id: "l3".into(),
            event_id: "one".into(),
            expected_base_event_id: None,
            payload_sha256: [0; 32],
        },
        repo_id: repo.clone(),
        revision_id: revision.clone(),
        generation: ManifestGeneration::new(1),
        base_generation: None,
        manifest_digest: "l3-manifest".into(),
        batch_digest: "0".repeat(64),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![
            scope("source-b", &[("b", "Café", "Two::Café", Some("Two"))])?,
            scope(
                "source-a",
                &[
                    ("a1", "Café", "One::Café", Some("One")),
                    ("a2", "Café", "One::Café", Some("One")),
                    ("nested", "Inner", "Café::Inner", Some("Café")),
                ],
            )?,
            scope("zero-symbols", &[])?,
        ],
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    };
    batch.source_event.payload_sha256 = source_event_payload_sha256(&batch)?;
    adapter.build_batch(&batch)?;
    let searcher = adapter.open(&repo, &revision, ManifestGeneration::new(1))?;
    Ok((dir, searcher))
}

fn query(name: &str, value: &str, manual: bool, sensitive: bool) -> LqQuery {
    let mut options = LqOptions::defaults();
    if manual {
        options.index_mode = Some(LqYesNoOnly::No);
    }
    if sensitive {
        options.case = Some(LqCase::Sensitive);
    }
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(LqLeaf::Predicate {
            name: name.into(),
            args: vec![LqPredicateArg::Keyword(value.into())],
        }),
        filters: Vec::new(),
        options,
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

#[test]
fn l3_exact_symbol_policy_preserves_overloads_source_owner_and_content_control() -> TestResult {
    let (_dir, searcher) = fixture()?;
    let budget = RequestBudgetV1::unbounded();
    for manual in [false, true] {
        for (name, value, sensitive, expected) in [
            (
                "symbol.local_name.exact",
                "CAFE\u{301}",
                false,
                vec!["a1", "a2", "b"],
            ),
            ("symbol.local_name.exact", "café", true, vec![]),
            (
                "symbol.local_name.exact",
                "Café",
                true,
                vec!["a1", "a2", "b"],
            ),
            (
                "symbol.qualified_name.exact",
                "One::Café",
                false,
                vec!["a1", "a2"],
            ),
            ("symbol.qualified_name.exact", "Café", false, vec![]),
            ("symbol.local_name.exact", "One", false, vec![]),
            (
                "symbol.qualified_name.exact",
                "Café::Inner",
                false,
                vec!["nested"],
            ),
            ("symbol.local_name.exact", "absent", false, vec![]),
        ] {
            let request = query(name, value, manual, sensitive);
            let rows = searcher.search_symbols(&request, 20, &budget)?;
            let ids: BTreeSet<_> = rows.iter().map(|row| row.candidate_id.as_str()).collect();
            assert_eq!(
                ids,
                expected.iter().copied().collect(),
                "{name} {value} manual={manual}"
            );
            for row in rows {
                assert_eq!(row.repo_id.as_str(), "containing-snapshot");
                let source = row.source.ok_or("missing source revision")?;
                assert_eq!(row.source_repo_id, source.file.source_repo_id);
                assert_eq!(
                    source.revision_id.as_str(),
                    format!("rev-{}", row.source_repo_id.as_str())
                );
                assert_eq!(row.repo_relative_path, source.file.repo_relative_path);
            }
        }
        let broad = searcher.search_symbols(
            &query("symbol.has.name", "Café", manual, false),
            20,
            &budget,
        )?;
        assert!(
            broad.iter().any(|row| row.candidate_id == "nested"),
            "broad container matching remains available"
        );
        let mut content = query("symbol.local_name.exact", "absent", manual, false);
        content.expr = LqExpr::Leaf(LqLeaf::Keyword("content_only".into()));
        let content_rows = searcher.search(&content, 20, &budget)?;
        assert_eq!(content_rows.len(), 3);
    }
    Ok(())
}
