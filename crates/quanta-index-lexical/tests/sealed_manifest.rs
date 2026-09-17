//! QI-BB-030 — a sealed lexical generation commits to every byte a query
//! needs, and both doors that admit it check the commitment.
//!
//! Before this, the sealed identity proved only itself: the activation
//! validator checked the identity and opened the Tantivy index, while the
//! query open additionally decoded text-authority sidecars it had never
//! verified. A sidecar lost, truncated or bit-flipped after the seal passed
//! activation and failed the first query.
//!
//! Now the seal writes a manifest (Tantivy commit digest, every file of the
//! `text-authority/` tree — its manifest and each shard — with length and
//! SHA-256, the identity's digest) before the identity, and both
//! `validate_generation_identity` (activation, restart) and `open` (query)
//! verify it. The oracle here is fault injection on the real files: every
//! text-authority file in turn is removed, truncated, bit-flipped and
//! replaced by a stale copy from another generation, and both doors must
//! refuse under the typed code and admit again once the file is restored.

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
    RequestBudgetV1, SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;

type TestResult = Result<(), Box<dyn Error>>;

const TEXT_AUTHORITY_DIR: &str = "text-authority";
const TEXT_AUTHORITY_MANIFEST: &str = "manifest.cbor";
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
        .and_then(|searcher| {
            searcher.search(&query("sealed_needle"), 5, &RequestBudgetV1::unbounded())
        })
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

/// The files of a generation's `text-authority/` tree: its manifest and
/// its shards, as the seal committed them.
fn text_authority_files(generation_dir: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(generation_dir.join(TEXT_AUTHORITY_DIR))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()?;
    files.sort();
    if files.len() < 2 {
        return Err(format!("expected a manifest and at least one shard, got {files:?}").into());
    }
    Ok(files)
}

/// The stale generation's counterpart of one text-authority file.
///
/// Its manifest for the manifest, its one shard for a shard. Shard files
/// are named by content, so the counterpart is found by kind, not by name.
fn stale_counterpart(stale_dir: &Path, path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let is_manifest =
        path.file_name().and_then(|name| name.to_str()) == Some(TEXT_AUTHORITY_MANIFEST);
    let mut candidates: Vec<PathBuf> = text_authority_files(stale_dir)?
        .into_iter()
        .filter(|stale| {
            (stale.file_name().and_then(|name| name.to_str()) == Some(TEXT_AUTHORITY_MANIFEST))
                == is_manifest
        })
        .collect();
    let [counterpart] = candidates.as_mut_slice() else {
        return Err(format!(
            "expected one stale counterpart for {}, got {candidates:?}",
            path.display()
        )
        .into());
    };
    Ok(counterpart.clone())
}

/// Every text-authority file, under every corruption, is refused by both
/// doors and admitted again once restored.
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
    for path in text_authority_files(&dir)? {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or("name")?;
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

        let stale = std::fs::read(stale_counterpart(&stale_dir, &path)?)?;
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

/// A generation that indexed nothing still seals to an openable state: the
/// seal creates the empty index it commits to, and the manifest says
/// explicitly that there is no text authority.
#[test]
fn a_generation_that_indexed_nothing_seals_openable() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(temp.path().to_path_buf());
    let generation = ManifestGeneration::new(1);
    let mut empty = sealed_batch(generation, "unused")?;
    empty.replace_scopes.clear();
    adapter.build_batch(&empty)?;
    let doors = knock(&adapter, generation);
    if let Err(err) = doors.validate {
        return Err(format!("validator refused an empty sealed generation: {err}").into());
    }
    match doors.open {
        Ok(0) => Ok(()),
        Ok(hits) => Err(format!("empty generation served {hits} hits").into()),
        Err(err) => Err(format!("open refused an empty sealed generation: {err}").into()),
    }
}
