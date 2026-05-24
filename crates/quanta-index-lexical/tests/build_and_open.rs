//! Integration tests for the Tantivy-backed lexical adapter.
//!
//! Test bodies obey the workspace `panic` / `unwrap` / `expect` ban via the
//! `ok_or_fail!` macro pattern lifted from
//! `crates/quanta-index-control/tests/control_plane.rs`.

use std::path::Path;
use std::thread;

use quanta_index_contract::{
    BundleArtifactRef, BundleEncoding, GenerationId, ManifestDigest, ManifestGeneration,
    PublishedGenerationSet, PublishedSearchBundleManifest, RepoId, RevisionId,
};
use quanta_index_core::{
    CoreError, LexicalBuildInput, SearchPlaneLexicalIndexBuildPort,
    SearchPlaneLexicalIndexStorePort,
};
use quanta_index_lexical::TantivyLexicalAdapter;
use tantivy::collector::Count;
use tantivy::query::AllQuery;
use tempfile::tempdir;

macro_rules! ok_or_fail {
    ($expr:expr, $msg:expr) => {
        match $expr {
            Ok(v) => v,
            Err(error) => {
                assert!(false, "{}: {error}", $msg);
                return;
            }
        }
    };
}

const SAMPLE_DIGEST: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

fn sample_artifact_ref(name: &str) -> BundleArtifactRef {
    BundleArtifactRef {
        relative_path: format!("bundle/{name}.json"),
        encoding: BundleEncoding::Json,
        byte_length: 0,
        content_digest: ManifestDigest::new(SAMPLE_DIGEST),
    }
}

fn sample_manifest(generation: u64) -> PublishedSearchBundleManifest {
    PublishedSearchBundleManifest {
        repo_id: RepoId::new("repo-x"),
        revision_id: RevisionId::new("rev-y"),
        manifest_generation: ManifestGeneration::new(generation),
        bundle_schema_version: 1,
        lexical_chunk_rows: sample_artifact_ref("lexical_chunk_rows"),
        symbol_rows: sample_artifact_ref("symbol_rows"),
        metadata_rows: None,
        graph_rows: None,
        embedding_input_views: None,
        embedding_records: None,
        mutation_delta: None,
    }
}

fn sample_generation_set(generation: u64) -> PublishedGenerationSet {
    PublishedGenerationSet {
        repo_id: RepoId::new("repo-x"),
        revision_id: RevisionId::new("rev-y"),
        manifest_generation: ManifestGeneration::new(generation),
        lexical_generation: GenerationId::new(1),
        symbol_generation: GenerationId::new(1),
        structural_generation: None,
        history_generation: None,
        semantic_generation: None,
        metadata_generation: None,
    }
}

fn sample_chunk_rows_json() -> Vec<u8> {
    br#"[
        {"repo_relative_path":"src/a.rs","start_line":1,"end_line":10,"text":"alpha bravo charlie"},
        {"repo_relative_path":"src/b.rs","start_line":20,"end_line":40,"text":"delta echo foxtrot"}
    ]"#
    .to_vec()
}

fn marker_path(root: &Path, generation: u64) -> std::path::PathBuf {
    root.join("lexical")
        .join(generation.to_string())
        .join("MARKER_OK")
}

fn building_path(root: &Path, generation: u64) -> std::path::PathBuf {
    let mut name = root
        .join("lexical")
        .join(generation.to_string())
        .as_os_str()
        .to_owned();
    name.push(".building");
    std::path::PathBuf::from(name)
}

#[test]
fn happy_path_build_open_and_query() {
    let temp = ok_or_fail!(tempdir(), "tempdir");
    let mut adapter = TantivyLexicalAdapter::with_state_root(temp.path());
    let manifest = sample_manifest(42);
    let generation_set = sample_generation_set(42);

    let chunks = sample_chunk_rows_json();
    let input = LexicalBuildInput {
        chunk_rows: &chunks,
        symbol_rows: b"",
    };

    ok_or_fail!(adapter.build_lexical_index(&manifest, input), "build");

    assert!(
        marker_path(temp.path(), 42).is_file(),
        "MARKER_OK must exist after a successful build"
    );

    ok_or_fail!(adapter.open_lexical_store(&generation_set), "open store");

    let index = ok_or_fail!(adapter.open_index_for_query(&generation_set), "open query");
    let reader = ok_or_fail!(index.reader(), "reader");
    let searcher = reader.searcher();
    let count = ok_or_fail!(searcher.search(&AllQuery, &Count), "search count");
    assert_eq!(count, 2, "expected 2 indexed documents, got {count}");
}

#[test]
fn open_before_build_returns_not_ready() {
    let temp = ok_or_fail!(tempdir(), "tempdir");
    let adapter = TantivyLexicalAdapter::with_state_root(temp.path());
    let generation_set = sample_generation_set(7);

    let result = adapter.open_lexical_store(&generation_set);
    assert!(
        matches!(result, Err(CoreError::NotReady(_))),
        "expected NotReady, got {result:?}"
    );

    let query_result = adapter.open_index_for_query(&generation_set);
    assert!(
        matches!(query_result, Err(CoreError::NotReady(_))),
        "expected NotReady from open_index_for_query, got {query_result:?}"
    );
}

#[test]
fn duplicate_build_is_idempotent_noop() {
    let temp = ok_or_fail!(tempdir(), "tempdir");
    let mut adapter = TantivyLexicalAdapter::with_state_root(temp.path());
    let manifest = sample_manifest(11);

    let chunks = sample_chunk_rows_json();
    let first_input = LexicalBuildInput {
        chunk_rows: &chunks,
        symbol_rows: b"",
    };
    ok_or_fail!(
        adapter.build_lexical_index(&manifest, first_input),
        "first build"
    );

    let marker = marker_path(temp.path(), 11);
    assert!(marker.is_file());
    let building = building_path(temp.path(), 11);
    assert!(
        !building.exists(),
        "building dir must not remain after a successful build"
    );

    // Second call: must short-circuit. We pass deliberately bogus chunk_rows
    // to prove no decode happened.
    let second_input = LexicalBuildInput {
        chunk_rows: b"not-json-at-all",
        symbol_rows: b"",
    };
    ok_or_fail!(
        adapter.build_lexical_index(&manifest, second_input),
        "second build (idempotent)"
    );
    assert!(
        !building.exists(),
        "building dir must not be recreated by an idempotent build"
    );
}

#[test]
fn malformed_json_returns_invalid_contract() {
    let temp = ok_or_fail!(tempdir(), "tempdir");
    let mut adapter = TantivyLexicalAdapter::with_state_root(temp.path());
    let manifest = sample_manifest(99);

    let input = LexicalBuildInput {
        chunk_rows: b"{not-valid-json",
        symbol_rows: b"",
    };
    let result = adapter.build_lexical_index(&manifest, input);
    assert!(
        matches!(result, Err(CoreError::InvalidContract(_))),
        "expected InvalidContract, got {result:?}"
    );

    let marker = marker_path(temp.path(), 99);
    assert!(
        !marker.exists(),
        "MARKER_OK must not be written when decode fails"
    );
}

#[test]
fn empty_chunk_rows_builds_empty_index() {
    let temp = ok_or_fail!(tempdir(), "tempdir");
    let mut adapter = TantivyLexicalAdapter::with_state_root(temp.path());
    let manifest = sample_manifest(5);
    let generation_set = sample_generation_set(5);

    let input = LexicalBuildInput {
        chunk_rows: b"[]",
        symbol_rows: b"",
    };
    ok_or_fail!(adapter.build_lexical_index(&manifest, input), "build empty");

    assert!(marker_path(temp.path(), 5).is_file());
    ok_or_fail!(adapter.open_lexical_store(&generation_set), "open empty");

    let index = ok_or_fail!(
        adapter.open_index_for_query(&generation_set),
        "open query empty"
    );
    let reader = ok_or_fail!(index.reader(), "reader");
    let searcher = reader.searcher();
    let count = ok_or_fail!(searcher.search(&AllQuery, &Count), "search count empty");
    assert_eq!(count, 0, "empty input must produce zero docs");
}

#[test]
fn concurrent_builds_for_different_generations_are_isolated() {
    let temp = ok_or_fail!(tempdir(), "tempdir");
    let root = temp.path().to_path_buf();

    let first_chunks =
        br#"[{"repo_relative_path":"a.rs","start_line":1,"end_line":2,"text":"alpha"}]"#.to_vec();
    let second_chunks =
        br#"[{"repo_relative_path":"b.rs","start_line":3,"end_line":4,"text":"bravo"}]"#.to_vec();

    thread::scope(|scope| {
        let first_root = root.clone();
        let first_payload = &first_chunks;
        let first_handle = scope.spawn(move || {
            let mut adapter = TantivyLexicalAdapter::with_state_root(&first_root);
            let manifest = sample_manifest(100);
            let input = LexicalBuildInput {
                chunk_rows: first_payload,
                symbol_rows: b"",
            };
            adapter.build_lexical_index(&manifest, input)
        });
        let second_root = root.clone();
        let second_payload = &second_chunks;
        let second_handle = scope.spawn(move || {
            let mut adapter = TantivyLexicalAdapter::with_state_root(&second_root);
            let manifest = sample_manifest(101);
            let input = LexicalBuildInput {
                chunk_rows: second_payload,
                symbol_rows: b"",
            };
            adapter.build_lexical_index(&manifest, input)
        });
        let Ok(first_result) = first_handle.join() else {
            assert!(false, "thread A panicked");
            return;
        };
        let Ok(second_result) = second_handle.join() else {
            assert!(false, "thread B panicked");
            return;
        };
        assert!(first_result.is_ok(), "build A failed: {first_result:?}");
        assert!(second_result.is_ok(), "build B failed: {second_result:?}");
    });

    assert!(
        marker_path(&root, 100).is_file(),
        "generation 100 marker missing"
    );
    assert!(
        marker_path(&root, 101).is_file(),
        "generation 101 marker missing"
    );

    // Doc counts are independent.
    let gen_a = sample_generation_set(100);
    let gen_b = sample_generation_set(101);
    let adapter = TantivyLexicalAdapter::with_state_root(&root);
    let index_a = ok_or_fail!(adapter.open_index_for_query(&gen_a), "open A");
    let index_b = ok_or_fail!(adapter.open_index_for_query(&gen_b), "open B");
    let reader_a = ok_or_fail!(index_a.reader(), "reader A");
    let reader_b = ok_or_fail!(index_b.reader(), "reader B");
    let count_a = ok_or_fail!(reader_a.searcher().search(&AllQuery, &Count), "count A");
    let count_b = ok_or_fail!(reader_b.searcher().search(&AllQuery, &Count), "count B");
    assert_eq!(count_a, 1);
    assert_eq!(count_b, 1);
}

#[test]
fn symbol_rows_are_ignored_at_schema_level() {
    let temp = ok_or_fail!(tempdir(), "tempdir");
    let mut adapter = TantivyLexicalAdapter::with_state_root(temp.path());
    let manifest = sample_manifest(77);
    let generation_set = sample_generation_set(77);

    // Empty chunk_rows but a non-empty symbol_rows payload. The adapter must
    // not look inside symbol_rows: any byte sequence is accepted.
    let input = LexicalBuildInput {
        chunk_rows: b"[]",
        symbol_rows: b"this-is-not-json-and-must-be-ignored",
    };
    ok_or_fail!(adapter.build_lexical_index(&manifest, input), "build");

    let index = ok_or_fail!(adapter.open_index_for_query(&generation_set), "open");
    let reader = ok_or_fail!(index.reader(), "reader");
    let count = ok_or_fail!(reader.searcher().search(&AllQuery, &Count), "count");
    assert_eq!(count, 0, "symbol_rows must not contribute documents");
}
