//! QI-BB-030 — a sealed lexical generation commits to every byte a query
//! needs, and both doors that admit it check the commitment.
//!
//! Before this, the sealed identity proved only itself: the activation
//! validator checked the identity and opened the Tantivy index, while the
//! query open additionally decoded five text-authority sidecars it had never
//! verified. A sidecar lost, truncated or bit-flipped after the seal passed
//! activation and failed the first query.
//!
//! Now the seal writes a manifest (Tantivy commit digest, every sidecar's
//! length and SHA-256, the identity's digest) before the identity, and both
//! `validate_generation_identity` (activation, restart) and `open` (query)
//! verify it. The oracle here is fault injection on the real files: every
//! sidecar in turn is removed, truncated, bit-flipped and replaced by a
//! stale copy from another generation, and both doors must refuse under the
//! typed code and admit again once the file is restored.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::{Path, PathBuf};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, GenerationSnapshot, LQ_VERSION_TAG, LqExpr, LqLeaf,
    LqOptions, LqQuery, LqSpan, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusIngestBatch, SearchCorpusReplaceScope, SearchPlaneTrackKind, SearchScopeKey,
    SearchScopeSurface,
};
use quanta_index_core::{
    CoreError, GenerationIdentityValidatePort, GenerationStorageKeyV1, LexicalIndexOpenPort,
    SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;

type TestResult = Result<(), Box<dyn Error>>;

const SIDECARS: [&str; 5] = [
    "text-authority-docs.cbor",
    "text-authority-trigram.cbor",
    "text-authority-trigram-folded.cbor",
    "text-authority-positions.cbor",
    "text-authority-positions-folded.cbor",
];
const MANIFEST: &str = "search-corpus-generation-manifest.cbor";
const IDENTITY: &str = "search-corpus-generation-identity.cbor";
const TANTIVY_META: &str = "meta.json";

fn repo() -> RepoId {
    RepoId::new("manifest-repo")
}

fn revision() -> RevisionId {
    RevisionId::new("manifest-rev")
}

fn scope(
    path: &str,
    chunk_id: &str,
    body: &str,
) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let language = LanguageCode::new("rust")
        .map_err(|err| -> Box<dyn Error> { format!("language code: {err}").into() })?;
    Ok(SearchCorpusReplaceScope {
        scope: SearchScopeKey {
            doc_surface: SearchScopeSurface::File,
            repo_relative_path: RepoRelativePath::new(path),
        },
        scope_digest: format!("scope:{path}:{chunk_id}"),
        chunks: vec![ChunkRecord {
            chunk_id: ChunkId::new(chunk_id),
            repo_relative_path: RepoRelativePath::new(path),
            language,
            start_byte: 0,
            end_byte: u32::try_from(body.len())?,
            start_line: 1,
            end_line: 1,
            text: body.to_string().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        }],
        symbols: Vec::new(),
    })
}

fn sealed_batch(
    generation: ManifestGeneration,
    body: &str,
) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    Ok(SearchCorpusIngestBatch {
        repo_id: repo(),
        revision_id: revision(),
        generation,
        base_generation: None,
        manifest_digest: format!("manifest-digest:{}", generation.get()),
        batch_digest: format!("batch-digest:{}", generation.get()),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![scope("src/lib.rs", "chunk-lib", body)?],
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    })
}

fn identity(generation: ManifestGeneration) -> GenerationSnapshot {
    GenerationSnapshot {
        repo_id: repo(),
        revision_id: revision(),
        track: SearchPlaneTrackKind::Lexical,
        manifest_generation: generation,
        manifest_digest: format!("manifest-digest:{}", generation.get()),
    }
}

fn generation_dir(root: &Path, generation: ManifestGeneration) -> PathBuf {
    GenerationStorageKeyV1::for_repo_revision(&repo(), &revision()).generation_dir(root, generation)
}

fn query(term: &str) -> LqQuery {
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(LqLeaf::Keyword(term.to_string())),
        filters: Vec::new(),
        options: LqOptions::defaults(),
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

/// What both doors said about a generation: the validator's verdict and
/// whether a query could open and serve it.
struct Doors {
    validate: Result<(), CoreError>,
    open: Result<usize, CoreError>,
}

fn knock(adapter: &LexicalAdapter, generation: ManifestGeneration) -> Doors {
    let validate = adapter.validate_generation_identity(&identity(generation));
    let open = adapter
        .open(&repo(), &revision(), generation)
        .and_then(|searcher| searcher.search(&query("sealed_needle"), 5))
        .map(|hits| hits.len());
    Doors { validate, open }
}

fn typed_code(result: &Result<(), CoreError>) -> Option<String> {
    match result {
        Err(CoreError::Typed { code, .. }) => Some(code.clone()),
        _ => None,
    }
}

fn typed_open_code(result: &Result<usize, CoreError>) -> Option<String> {
    match result {
        Err(CoreError::Typed { code, .. }) => Some(code.clone()),
        _ => None,
    }
}

fn expect_admitted(doors: &Doors, what: &str) -> TestResult {
    if let Err(err) = &doors.validate {
        return Err(format!("{what}: validator refused an intact generation: {err}").into());
    }
    match &doors.open {
        Ok(1) => Ok(()),
        Ok(other) => {
            Err(format!("{what}: intact generation served {other} hits, expected 1").into())
        }
        Err(err) => Err(format!("{what}: open refused an intact generation: {err}").into()),
    }
}

fn expect_refused(doors: &Doors, what: &str, code: &str) -> TestResult {
    if typed_code(&doors.validate).as_deref() != Some(code) {
        return Err(format!(
            "{what}: validator answered {:?}, expected typed {code}",
            doors.validate
        )
        .into());
    }
    if typed_open_code(&doors.open).as_deref() != Some(code) {
        return Err(format!(
            "{what}: open answered {:?}, expected typed {code}",
            doors.open.as_ref().map(|_| "served")
        )
        .into());
    }
    Ok(())
}

/// Every sidecar, under every corruption, is refused by both doors and
/// admitted again once restored.
#[test]
fn both_doors_refuse_a_sidecar_that_does_not_match_the_manifest() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    let stale_generation = ManifestGeneration::new(2);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    adapter.build_batch(&sealed_batch(
        stale_generation,
        "fn two() { sealed_needle other }",
    )?)?;
    expect_admitted(&knock(&adapter, generation), "intact")?;

    let dir = generation_dir(&root, generation);
    let stale_dir = generation_dir(&root, stale_generation);
    for name in SIDECARS {
        let path = dir.join(name);
        let original = std::fs::read(&path)?;

        std::fs::remove_file(&path)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} missing"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;

        let half = original.len().div_euclid(2);
        std::fs::write(
            &path,
            original.get(..half).ok_or("sidecar shorter than half")?,
        )?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} truncated"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;

        let mut flipped = original.clone();
        let middle = flipped.len().div_euclid(2);
        if let Some(byte) = flipped.get_mut(middle) {
            *byte ^= 0x01;
        }
        std::fs::write(&path, &flipped)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} bit-flipped"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;

        let stale = std::fs::read(stale_dir.join(name))?;
        std::fs::write(&path, &stale)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} stale copy"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;

        std::fs::write(&path, &original)?;
        expect_admitted(&knock(&adapter, generation), &format!("{name} restored"))?;
    }
    Ok(())
}

/// The Tantivy commit is part of the commitment: a `meta.json` other than
/// the sealed one is refused even though Tantivy itself would open it.
#[test]
fn both_doors_refuse_an_index_commit_other_than_the_sealed_one() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    let meta = generation_dir(&root, generation).join(TANTIVY_META);
    let original = std::fs::read(&meta)?;
    // Same JSON, different bytes: whitespace Tantivy tolerates, the manifest does not.
    let mut altered = original.clone();
    altered.push(b'\n');
    std::fs::write(&meta, &altered)?;
    expect_refused(
        &knock(&adapter, generation),
        "meta.json altered",
        "GENERATION_SIDECAR_CORRUPT",
    )?;
    std::fs::write(&meta, &original)?;
    expect_admitted(&knock(&adapter, generation), "meta.json restored")
}

/// A sealed identity without its manifest is not a sealed generation this
/// adapter will serve: the manifest is missing, and that is a typed refusal
/// with a migration message, not a silent "no sidecars".
#[test]
fn a_sealed_identity_without_a_manifest_is_refused_by_both_doors() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    let manifest = generation_dir(&root, generation).join(MANIFEST);
    let original = std::fs::read(&manifest)?;
    std::fs::remove_file(&manifest)?;
    expect_refused(
        &knock(&adapter, generation),
        "manifest missing",
        "GENERATION_MANIFEST_MISSING",
    )?;
    std::fs::write(&manifest, &original)?;
    expect_admitted(&knock(&adapter, generation), "manifest restored")
}

/// The identity and the manifest bind each other: an identity rewritten
/// for a different digest no longer matches the manifest that was sealed
/// with it.
#[test]
fn an_identity_that_does_not_match_the_manifest_is_refused() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    let dir = generation_dir(&root, generation);
    let identity_path = dir.join(IDENTITY);
    let original = std::fs::read(&identity_path)?;
    let mut forged = identity(generation);
    forged.manifest_digest = "manifest-digest:forged".to_string();
    let mut bytes = Vec::new();
    ciborium::into_writer(&forged, &mut bytes)?;
    std::fs::write(&identity_path, &bytes)?;
    // The validator sees the identity disagree with its candidate first; the
    // open, which has no candidate digest, is caught by the manifest binding.
    let doors = knock(&adapter, generation);
    if typed_code(&doors.validate).as_deref() != Some("GENERATION_IDENTITY_DIGEST_MISMATCH") {
        return Err(format!("validator answered {:?}", doors.validate).into());
    }
    if typed_open_code(&doors.open).as_deref() != Some("GENERATION_IDENTITY_DIGEST_MISMATCH") {
        return Err(format!("open answered {:?}", doors.open.as_ref().map(|_| "served")).into());
    }
    std::fs::write(&identity_path, &original)?;
    expect_admitted(&knock(&adapter, generation), "identity restored")
}

/// After the seal nothing may change the index: an index-mutating op is
/// refused as immutable, while the mutable overlay (repo metadata) still
/// lands, and the manifest keeps matching.
#[test]
fn a_sealed_generation_refuses_index_mutation_but_keeps_its_overlay() -> TestResult {
    use quanta_index_contract::channel::{LexicalChannelOp, LexicalFullBundle, UpsertChunk};
    use quanta_index_core::LexicalIndexBuildPort;

    let temp = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(temp.path().to_path_buf());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;

    let mutation = LexicalChannelOp::UpsertChunk(UpsertChunk {
        repo_id: repo(),
        revision_id: revision(),
        generation,
        chunk_id: ChunkId::new("late"),
        payload: Vec::new(),
    });
    match adapter.build(&repo(), &revision(), generation, &[mutation]) {
        Err(CoreError::Typed { code, .. }) if code == "GENERATION_IMMUTABLE" => {}
        other => return Err(format!("index mutation after seal answered {other:?}").into()),
    }

    let overlay = LexicalChannelOp::FullBundle(LexicalFullBundle {
        repo_id: repo(),
        revision_id: revision(),
        generation,
        payload: Vec::new(),
    });
    adapter.build(&repo(), &revision(), generation, &[overlay])?;
    expect_admitted(&knock(&adapter, generation), "overlay after seal")
}
