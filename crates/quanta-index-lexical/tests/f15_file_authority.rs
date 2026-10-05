//! F15 file authority through the public sealed adapter. Fixed source rows
//! establish admission and provenance; a separate full build is the oracle
//! for delta/no-op ranking, scores and pagination.

#![forbid(unsafe_code)]
#![expect(
    clippy::indexing_slicing,
    clippy::panic_in_result_fn,
    reason = "fixed fixture assertions report product regressions"
)]

#[path = "support/current_source_fixture.rs"]
mod current_source_fixture;

#[path = "support/source_fixture.rs"]
mod source_fixture;

use std::error::Error;
use std::path::Path;
use std::time::{Duration, Instant};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, LQ_VERSION_TAG, LexicalCandidate, LexicalCursor, LqExpr,
    LqFilter, LqLeaf, LqOptions, LqPatternType, LqQuery, LqSelect, LqSpan, ManifestGeneration,
    QueryConstraintSetV1, RepoId, RepoRelativePath, RevisionId, SearchCorpusIngestBatch,
    SearchCorpusReplaceScope, SearchCorpusTombstoneScope,
};
use quanta_index_core::{
    CoreError, GenerationStorageKeyV1, LexicalIndexOpenPort, LexicalPageSpec, LexicalSearcher,
    RequestBudgetV1, SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;
use sha2::{Digest as _, Sha256};

type TestResult = Result<(), Box<dyn Error>>;
type Row = (String, String, String, u32, Option<(u64, u64)>);

const SHARED: &str = "abc Café unique";
const EARLY_PATH: &str = "src/zabc.r";
const LATE_PATH: &str = "src/abc.rs";
const COPY_PATH: &str = "src/copy.rs";
const RETIRED_PATH: &str = "src/retire.rs";

fn repo() -> Result<RepoId, Box<dyn Error>> {
    Ok(RepoId::new("f15-snapshot")?)
}

fn revision() -> Result<RevisionId, Box<dyn Error>> {
    Ok(RevisionId::new("f15-revision")?)
}

fn scope(
    owner: &str,
    path: &str,
    body: &str,
    admitted: bool,
) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let source_repo = RepoId::new(owner)?;
    let source_revision = RevisionId::new("f15-source-revision")?;
    let language = LanguageCode::new("rust")?;
    // The accented witness is deliberately outside the only chunk. CodeSearch
    // must consult the committed full source, not reconstruct it from chunks.
    let chunk_text = if body == SHARED { "abc " } else { body };
    let chunks = if admitted {
        vec![ChunkRecord {
            chunk_id: ChunkId::new(format!("f15-{owner}-{path}")),
            repo_relative_path: RepoRelativePath::new(path),
            language: language.clone(),
            start_byte: 0,
            end_byte: u32::try_from(chunk_text.len())?,
            start_line: 1,
            end_line: 1,
            text: chunk_text.to_string().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: Some(source_repo.clone()),
        }]
    } else {
        Vec::new()
    };
    current_source_fixture::text_scope(&source_repo, &source_revision, path, language, body, chunks)
}

fn batch(
    generation: u64,
    base: Option<u64>,
    replacements: Vec<SearchCorpusReplaceScope>,
    delete: &[(&str, &str)],
) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    let mut result = SearchCorpusIngestBatch {
        source_event: current_source_fixture::empty_event(),
        repo_id: repo()?,
        revision_id: revision()?,
        generation: ManifestGeneration::new(generation),
        base_generation: base.map(ManifestGeneration::new),
        manifest_digest: format!("f15-manifest-{generation}"),
        batch_digest: "0".repeat(64),
        mode: if base.is_some() {
            BatchIngestMode::Delta
        } else {
            BatchIngestMode::ReplaceGeneration
        },
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: replacements,
        tombstone_scopes: delete
            .iter()
            .map(|(owner, path)| {
                Ok(SearchCorpusTombstoneScope {
                    file: quanta_index_contract::SourceFileKey {
                        source_repo_id: RepoId::new(*owner)?,
                        repo_relative_path: RepoRelativePath::new(*path),
                    },
                })
            })
            .collect::<Result<Vec<_>, Box<dyn Error>>>()?,
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    };
    current_source_fixture::finish_batch(&mut result)?;
    Ok(result)
}

fn base_scopes() -> Result<Vec<SearchCorpusReplaceScope>, Box<dyn Error>> {
    Ok(vec![
        scope("source-a", EARLY_PATH, "neutral path owner", true)?,
        scope("source-b", LATE_PATH, SHARED, true)?,
        // Same source bytes, distinct key and no admitted content.
        scope("source-a", COPY_PATH, SHARED, false)?,
        scope("source-a", RETIRED_PATH, "retiretoken", true)?,
    ])
}

fn final_scopes() -> Result<Vec<SearchCorpusReplaceScope>, Box<dyn Error>> {
    Ok(vec![
        scope("source-a", EARLY_PATH, "neutral path owner", true)?,
        scope("source-b", LATE_PATH, SHARED, false)?,
        scope("source-a", COPY_PATH, SHARED, true)?,
    ])
}

fn query(term: &str) -> LqQuery {
    let mut options = LqOptions::defaults();
    options.pattern_type = LqPatternType::CodeSearch;
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(LqLeaf::RawString(term.to_string())),
        filters: vec![LqFilter::Select {
            dim: LqSelect::File,
        }],
        options,
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

fn open(
    adapter: &LexicalAdapter,
    generation: u64,
) -> Result<Box<dyn LexicalSearcher>, Box<dyn Error>> {
    Ok(adapter.open(
        &repo()?,
        &revision()?,
        ManifestGeneration::new(generation),
        &RequestBudgetV1::unbounded(),
    )?)
}

fn page(
    searcher: &dyn LexicalSearcher,
    query: &LqQuery,
    fetch: u32,
    after: Option<LexicalCursor>,
) -> Result<Vec<LexicalCandidate>, CoreError> {
    Ok(searcher
        .search_constrained(
            query,
            &QueryConstraintSetV1::unconstrained(),
            &LexicalPageSpec { fetch, after },
            &RequestBudgetV1::unbounded(),
        )?
        .candidates)
}

fn project(row: &LexicalCandidate) -> Row {
    (
        row.source_repo_id.as_str().to_string(),
        row.repo_relative_path.as_str().to_string(),
        row.candidate_id.clone(),
        row.score.to_bits(),
        row.preview
            .as_ref()
            .and_then(|preview| preview.original_focus)
            .map(|span| (span.start, span.end)),
    )
}

fn rows(
    searcher: &dyn LexicalSearcher,
    generation: u64,
    term: &str,
    fetch: u32,
) -> Result<Vec<Row>, Box<dyn Error>> {
    let request = query(term);
    let mut result = Vec::new();
    let mut after = None;
    loop {
        let next = page(searcher, &request, fetch, after)?;
        let Some(last) = next.last() else { break };
        after = Some(LexicalCursor::at(
            ManifestGeneration::new(generation),
            last.order_key(),
        ));
        result.extend(next.iter().map(project));
        if result.len() > 8 {
            return Err("file page walk exceeded fixed fixture universe".into());
        }
    }
    Ok(result)
}

fn generation_dir(root: &Path, generation: u64) -> Result<std::path::PathBuf, Box<dyn Error>> {
    Ok(
        GenerationStorageKeyV1::for_repo_revision(&repo()?, &revision()?)
            .generation_dir(root, ManifestGeneration::new(generation)),
    )
}

#[test]
fn delta_delete_noop_and_cold_open_match_independent_full_build() -> TestResult {
    let incremental_root = tempfile::tempdir()?;
    let incremental = LexicalAdapter::with_state_root(incremental_root.path().to_path_buf());
    assert!(
        incremental
            .build_batch(&batch(1, None, base_scopes()?, &[])?)?
            .is_some()
    );

    let base = open(&incremental, 1)?;
    let base_abc = rows(base.as_ref(), 1, "abc", 1)?;
    assert_eq!(
        base_abc
            .iter()
            .map(|row| (row.0.as_str(), row.1.as_str()))
            .collect::<Vec<_>>(),
        vec![("source-b", LATE_PATH), ("source-a", EARLY_PATH)]
    );
    assert_eq!(
        rows(base.as_ref(), 1, "CAFÉ", 1)?
            .iter()
            .map(|row| row.0.as_str())
            .collect::<Vec<_>>(),
        vec!["source-b"]
    );
    let base_full = page(base.as_ref(), &query("abc"), 10, None)?;
    assert_eq!(base_abc, base_full.iter().map(project).collect::<Vec<_>>());
    let witness = page(base.as_ref(), &query("CAFÉ"), 10, None)?;
    assert_eq!(witness.len(), 1);
    assert_eq!(
        witness[0]
            .source
            .as_ref()
            .ok_or("source identity")?
            .source_sha256,
        <[u8; 32]>::from(Sha256::digest(SHARED.as_bytes()))
    );
    assert_eq!(
        witness[0]
            .preview
            .as_ref()
            .and_then(|preview| preview.original_focus)
            .map(|span| (span.start, span.end)),
        Some((4, 9))
    );

    // The same bytes change admission in opposite directions at distinct
    // keys. A retired key disappears; untouched early path remains.
    let changes = vec![
        scope("source-b", LATE_PATH, SHARED, false)?,
        scope("source-a", COPY_PATH, SHARED, true)?,
    ];
    assert!(
        incremental
            .build_batch(&batch(2, Some(1), changes, &[("source-a", RETIRED_PATH)])?)?
            .is_some()
    );
    assert!(
        incremental
            .build_batch(&batch(3, Some(2), Vec::new(), &[])?)?
            .is_some()
    );

    let rebuilt_root = tempfile::tempdir()?;
    let rebuilt = LexicalAdapter::with_state_root(rebuilt_root.path().to_path_buf());
    assert!(
        rebuilt
            .build_batch(&batch(2, None, final_scopes()?, &[])?)?
            .is_some()
    );

    // New adapter instances force the sealed reader path rather than a
    // writer's in-memory state. One-row pages exercise the late path winner.
    let cold = LexicalAdapter::with_state_root(incremental_root.path().to_path_buf());
    let delta = open(&cold, 2)?;
    let noop = open(&cold, 3)?;
    let full_cold = LexicalAdapter::with_state_root(rebuilt_root.path().to_path_buf());
    let full = open(&full_cold, 2)?;
    for term in ["abc", "CAFÉ", "retiretoken", "neutral"] {
        let expected = rows(full.as_ref(), 2, term, 1)?;
        assert_eq!(rows(delta.as_ref(), 2, term, 1)?, expected, "delta: {term}");
        assert_eq!(rows(noop.as_ref(), 3, term, 1)?, expected, "no-op: {term}");
        assert_eq!(
            rows(delta.as_ref(), 2, term, 10)?,
            expected,
            "page cut: {term}"
        );
    }
    let delta_abc = rows(delta.as_ref(), 2, "abc", 1)?;
    assert_eq!(
        delta_abc
            .first()
            .map(|row| (row.0.as_str(), row.1.as_str())),
        Some(("source-b", LATE_PATH))
    );
    assert_eq!(delta_abc.len(), 3);
    assert!(rows(delta.as_ref(), 2, "retiretoken", 1)?.is_empty());
    let unicode = page(delta.as_ref(), &query("CAFÉ"), 10, None)?;
    assert_eq!(unicode.len(), 1);
    assert_eq!(unicode[0].source_repo_id.as_str(), "source-a");
    assert_eq!(unicode[0].repo_relative_path.as_str(), COPY_PATH);
    assert_eq!(
        unicode[0]
            .source
            .as_ref()
            .ok_or("source identity")?
            .source_sha256,
        <[u8; 32]>::from(Sha256::digest(SHARED.as_bytes()))
    );
    assert_eq!(
        unicode[0]
            .preview
            .as_ref()
            .and_then(|preview| preview.original_focus)
            .map(|span| (span.start, span.end)),
        Some((4, 9))
    );

    let expired = RequestBudgetV1::until(
        Instant::now()
            .checked_sub(Duration::from_millis(1))
            .ok_or("clock cannot represent expired deadline")?,
    );
    assert!(matches!(
        delta.search_constrained(
            &query("abc"),
            &QueryConstraintSetV1::unconstrained(),
            &LexicalPageSpec::first(1),
            &expired
        ),
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::RequestDeadlineExceeded,
            ..
        })
    ));
    Ok(())
}

#[test]
fn sealed_object_corruption_missing_and_symlink_refuse_cold_open() -> TestResult {
    for fault in ["corrupt", "missing", "symlink"] {
        let root = tempfile::tempdir()?;
        let adapter = LexicalAdapter::with_state_root(root.path().to_path_buf());
        assert!(
            adapter
                .build_batch(&batch(1, None, base_scopes()?, &[])?)?
                .is_some()
        );
        drop(adapter);
        let objects = generation_dir(root.path(), 1)?.join("file-authority/objects");
        let object = std::fs::read_dir(&objects)?
            .next()
            .ok_or("no sealed authority object")??
            .path();
        let mut original = std::fs::read(&object)?;
        assert!(!original.is_empty());
        match fault {
            "corrupt" => {
                let first = original.first_mut().ok_or("empty object")?;
                *first ^= 0xff;
                std::fs::write(&object, original)?;
            }
            "missing" => std::fs::remove_file(&object)?,
            "symlink" => {
                let external = root.path().join("outside-object.bin");
                std::fs::write(&external, original)?;
                std::fs::remove_file(&object)?;
                std::os::unix::fs::symlink(&external, &object)?;
            }
            _ => return Err("unknown fixed fault".into()),
        }
        let cold = LexicalAdapter::with_state_root(root.path().to_path_buf());
        assert!(
            matches!(
                cold.open(
                    &repo()?,
                    &revision()?,
                    ManifestGeneration::new(1),
                    &RequestBudgetV1::unbounded()
                ),
                Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                    ..
                })
            ),
            "cold open admitted {fault} committed object"
        );
    }
    Ok(())
}

fn raw_scope(path: &str, body: &[u8]) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    Ok(source_fixture::complete_file(
        source_fixture::file_key(&RepoId::new("source-a")?, path),
        &RevisionId::new("f15-source-revision")?,
        LanguageCode::new("rust")?,
        body,
        Vec::new(),
        Vec::new(),
    )?)
}

#[test]
fn nontext_source_stays_path_only_across_cold_delta_noop_and_delete() -> TestResult {
    const BINARY_PATH: &str = "src/binarystable.dat";
    const UTF8_PATH: &str = "src/utf8stable.dat";
    const BINARY_BODY: &[u8] = b"\xff\x00";
    let root = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(root.path().to_path_buf());
    let mut initial = source_fixture::sealed_batch(
        &repo()?,
        &revision()?,
        ManifestGeneration::new(1),
        vec![
            raw_scope(BINARY_PATH, BINARY_BODY)?,
            raw_scope(UTF8_PATH, b"hiddenbodytoken")?,
        ],
    )?;
    current_source_fixture::finish_batch(&mut initial)?;
    assert!(adapter.build_batch(&initial)?.is_some());
    let cold = LexicalAdapter::with_state_root(root.path().to_path_buf());
    let retained = open(&cold, 1)?;
    for term in ["binarystable", "utf8stable"] {
        assert_eq!(rows(retained.as_ref(), 1, term, 1)?.len(), 1);
    }
    assert!(rows(retained.as_ref(), 1, "hiddenbodytoken", 1)?.is_empty());
    let binary_hit = page(retained.as_ref(), &query("binarystable"), 10, None)?;
    assert_eq!(
        binary_hit
            .first()
            .ok_or("binary path absent")?
            .source
            .as_ref()
            .ok_or("binary path source identity absent")?
            .source_sha256,
        <[u8; 32]>::from(Sha256::digest(BINARY_BODY))
    );

    assert!(
        adapter
            .build_batch(&batch(
                2,
                Some(1),
                vec![scope("source-a", BINARY_PATH, "visiblebodytoken", true)?],
                &[],
            )?)?
            .is_some()
    );
    assert!(
        adapter
            .build_batch(&batch(3, Some(2), Vec::new(), &[])?)?
            .is_some()
    );
    for generation in [2, 3] {
        let fresh = LexicalAdapter::with_state_root(root.path().to_path_buf());
        let searcher = open(&fresh, generation)?;
        for term in ["binarystable", "utf8stable", "visiblebodytoken"] {
            assert_eq!(rows(searcher.as_ref(), generation, term, 1)?.len(), 1);
        }
        assert!(rows(searcher.as_ref(), generation, "hiddenbodytoken", 1)?.is_empty());
    }
    assert!(
        adapter
            .build_batch(&batch(
                4,
                Some(3),
                Vec::new(),
                &[("source-a", BINARY_PATH), ("source-a", UTF8_PATH)],
            )?)?
            .is_some()
    );
    let deleted = LexicalAdapter::with_state_root(root.path().to_path_buf());
    let searcher = open(&deleted, 4)?;
    for term in [
        "binarystable",
        "utf8stable",
        "hiddenbodytoken",
        "visiblebodytoken",
    ] {
        assert!(rows(searcher.as_ref(), 4, term, 1)?.is_empty());
    }
    assert_eq!(rows(retained.as_ref(), 1, "binarystable", 1)?.len(), 1);
    assert!(rows(retained.as_ref(), 1, "hiddenbodytoken", 1)?.is_empty());
    Ok(())
}
