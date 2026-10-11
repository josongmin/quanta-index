//! Native text authority: bounded paged segments, immutable shared objects,
//! independent delta/rebuild query gold, and typed selected-read refusal.

#![forbid(unsafe_code)]

#[path = "support/publication_fixture.rs"]
mod publication_fixture;
use publication_fixture::FixtureBatchBuildPort;

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::Write as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, GenerationSnapshot, LQ_VERSION_TAG, LqExpr, LqLeaf,
    LqOptions, LqQuery, LqSpan, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusIngestBatch, SearchCorpusReplaceScope, SearchCorpusTombstoneScope,
    SearchPlaneTrackKind,
};
use quanta_index_core::{
    CoreError, GenerationIdentityValidatePort, GenerationStorageKeyV1, LexicalIndexOpenPort,
    RequestBudgetV1,
};
use quanta_index_lexical::LexicalAdapter;
use sha2::{Digest as _, Sha256};

#[path = "support/current_source_fixture.rs"]
mod current_source_fixture;

type TestResult = Result<(), Box<dyn Error>>;

/// Segment-local ID cap; byte target can split earlier.
const SHARD_DOCS: u64 = 2048;
const DOCS: usize = 2 * 2048 + 2;
const COST_DOCS: usize = 3 * 2048 + 50;
const EDGE_LOW: usize = 2046;
const EDGE_MID: usize = 2047;
const EDGE_HIGH: usize = 4095;
const EDGE_LOW_MARKER: &str = "edgelowmarker";
const EDGE_MID_MARKER: &str = "edgemidmarker";
const EDGE_HIGH_MARKER: &str = "edgehighmarker";
const TEXT_AUTHORITY_DIR: &str = "text-authority";
const TEXT_AUTHORITY_MANIFEST: &str = "manifest.cbor";
const SEALED_MANIFEST: &str = "search-corpus-generation-manifest.cbor";

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn repo() -> RepoId {
    RepoId::new("shard-repo").expect("static fixture ID satisfies canonical policy")
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn revision() -> RevisionId {
    RevisionId::new("shard-rev").expect("static fixture ID satisfies canonical policy")
}

fn scope_path(index: usize) -> String {
    format!("src/s/{index:05}.rs")
}

fn scope_chunk(index: usize) -> String {
    format!("chunk-{index:05}")
}

/// The fixture body of scope `index`: a unique token, filler, and an edge
/// marker sentence on the three boundary scopes.
fn scope_body(index: usize) -> String {
    let marker = match index {
        EDGE_LOW => Some(EDGE_LOW_MARKER),
        EDGE_MID => Some(EDGE_MID_MARKER),
        EDGE_HIGH => Some(EDGE_HIGH_MARKER),
        _ => None,
    };
    let body = format!(
        "fn scope_{index:05}() {{ let token = quartz_{index:05}; lorem ipsum dolor sit amet consectetur adipiscing elit }}"
    );
    match marker {
        Some(marker) => format!("{body} {marker} phraseanchor {marker}_tail"),
        None => body,
    }
}

fn scope(index: usize, body: &str) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let language = LanguageCode::new("rust")
        .map_err(|err| -> Box<dyn Error> { format!("language code: {err}").into() })?;
    let path = scope_path(index);
    current_source_fixture::text_scope(
        &repo(),
        &revision(),
        &path,
        language.clone(),
        body,
        vec![ChunkRecord {
            chunk_id: ChunkId::new(scope_chunk(index)),
            repo_relative_path: RepoRelativePath::new(&path),
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
    )
}

fn batch(
    generation: ManifestGeneration,
    base: Option<ManifestGeneration>,
    replace_scopes: Vec<SearchCorpusReplaceScope>,
    tombstone_paths: &[usize],
    seal: bool,
) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    let mut replace_scopes = replace_scopes;
    replace_scopes.sort_by(|left, right| {
        left.coverage
            .source
            .file
            .repo_relative_path
            .as_str()
            .cmp(right.coverage.source.file.repo_relative_path.as_str())
    });
    let mut batch = SearchCorpusIngestBatch {
        source_event: current_source_fixture::empty_event(),
        repo_id: repo(),
        revision_id: revision(),
        generation,
        base_generation: base,
        manifest_digest: format!("shard-manifest:{}", generation.get()),
        // The adapter validates shape here; IPC owns body-digest proof.
        batch_digest: "0".repeat(64),
        mode: if base.is_some() {
            BatchIngestMode::Delta
        } else {
            BatchIngestMode::ReplaceGeneration
        },
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes,
        tombstone_scopes: tombstone_paths
            .iter()
            .map(|index| SearchCorpusTombstoneScope {
                file: quanta_index_contract::SourceFileKey {
                    source_repo_id: repo(),
                    repo_relative_path: RepoRelativePath::new(scope_path(*index)),
                },
            })
            .collect(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal,
    };
    current_source_fixture::finish_batch(&mut batch)?;
    Ok(batch)
}

/// The first `docs` scopes of the fixture corpus as one sealed base.
fn base_batch(
    generation: ManifestGeneration,
    docs: usize,
) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    let mut scopes = Vec::with_capacity(docs);
    for index in 0..docs {
        scopes.push(scope(index, &scope_body(index))?);
    }
    batch(generation, None, scopes, &[], true)
}

fn identity(generation: ManifestGeneration) -> GenerationSnapshot {
    GenerationSnapshot {
        repo_id: repo(),
        revision_id: revision(),
        track: SearchPlaneTrackKind::Lexical,
        manifest_generation: generation,
        manifest_digest: format!("shard-manifest:{}", generation.get()),
    }
}

fn generation_dir(root: &Path, generation: ManifestGeneration) -> PathBuf {
    GenerationStorageKeyV1::for_repo_revision(&repo(), &revision()).generation_dir(root, generation)
}

fn leaf_query(leaf: LqLeaf) -> LqQuery {
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(leaf),
        filters: Vec::new(),
        options: LqOptions::defaults(),
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

fn leaf_hit_ids(
    adapter: &LexicalAdapter,
    generation: ManifestGeneration,
    leaf: LqLeaf,
) -> Result<Vec<String>, Box<dyn Error>> {
    let searcher = adapter.open(
        &repo(),
        &revision(),
        generation,
        &quanta_index_core::RequestBudgetV1::unbounded(),
    )?;
    let mut ids: Vec<String> = searcher
        .search(&leaf_query(leaf), 64, &RequestBudgetV1::unbounded())?
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    ids.sort();
    Ok(ids)
}

/// The root is a compact control row. Decode only the fields needed by
/// this independent disk oracle; all source and posting values stay in the
/// authenticated shared pool.
type ObjectRefWire = (ciborium::Value, u64, [u8; 32]);
type PageRefWire = (ObjectRefWire, Vec<u8>, Vec<u8>, u64, u8);
type MapRootWire = (Option<PageRefWire>, u64);
type NativeRootWire = (
    u16,
    (u16, u16),
    [u8; 32],
    MapRootWire,
    MapRootWire,
    MapRootWire,
    MapRootWire,
    MapRootWire,
    MapRootWire,
    MapRootWire,
    u64,
    u64,
);

fn native_root_wire(directory: &Path) -> Result<NativeRootWire, Box<dyn Error>> {
    let bytes = std::fs::read(
        directory
            .join(TEXT_AUTHORITY_DIR)
            .join(TEXT_AUTHORITY_MANIFEST),
    )?;
    Ok(ciborium::from_reader(bytes.as_slice())?)
}

fn current_page_path(state_root: &Path, directory: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let root = native_root_wire(directory)?;
    let page = root.4.0.ok_or("Native current map has no root page")?;
    let mut digest = String::with_capacity(64);
    for byte in page.0.2 {
        write!(&mut digest, "{byte:02x}")?;
    }
    Ok(state_root
        .join("lexical-object-pool/objects")
        .join(digest.get(..2).ok_or("short object digest")?)
        .join(digest))
}

/// Canonical physical objects: the test independently checks that a delta
/// neither rewrites nor unlinks a base object's inode, bytes, or SHA.
type PoolObjectFacts = (u64, u64, [u8; 32]);

fn pool_objects(root: &Path) -> Result<BTreeMap<String, PoolObjectFacts>, Box<dyn Error>> {
    let objects = root.join("lexical-object-pool/objects");
    let mut result = BTreeMap::new();
    for shard in std::fs::read_dir(&objects)? {
        let shard = shard?;
        if !shard.file_type()?.is_dir() {
            return Err("pool shard is not a real directory".into());
        }
        for object in std::fs::read_dir(shard.path())? {
            let object = object?;
            if !object.file_type()?.is_file() {
                return Err("pool object is not a real file".into());
            }
            let path = object.path();
            let name = path.strip_prefix(&objects)?.to_string_lossy().into_owned();
            let meta = object.metadata()?;
            let facts = (
                meta.ino(),
                meta.len(),
                Sha256::digest(std::fs::read(path)?).into(),
            );
            if result.insert(name, facts).is_some() {
                return Err("duplicate physical pool object".into());
            }
        }
    }
    Ok(result)
}

#[expect(
    clippy::print_stdout,
    reason = "bounded segment and physical pool evidence belongs in the test run log"
)]
fn emit_evidence(fields: &[(&str, String)]) {
    let rendered: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    println!("QI-BB-006-NATIVE-EVIDENCE {}", rendered.join(" "));
}

/// A one-source delta pays at most one due segment and keeps the other three
/// segment roots. The object pool, rather than generation-local hard links,
/// owns immutable bodies and postings.
#[test]
fn one_scope_delta_preserves_unaffected_paged_segments() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let adapter = LexicalAdapter::with_state_root(root.to_path_buf());
    let g1 = ManifestGeneration::new(1);
    let g2 = ManifestGeneration::new(2);
    let base_stages = adapter
        .build_fixture_batch(&base_batch(g1, COST_DOCS)?)?
        .ok_or("base timing")?;
    if base_stages.text_authority_ns == 0 {
        return Err("base Native build timing absent".into());
    }
    let base_root = native_root_wire(&generation_dir(root, g1))?;
    let expected_segments = u64::try_from(COST_DOCS)?.div_ceil(SHARD_DOCS);
    if base_root.6.1 != expected_segments {
        return Err(format!(
            "base has {} Native segments, expected {expected_segments}",
            base_root.6.1
        )
        .into());
    }
    let before_stats = adapter.text_authority_update_stats()?;
    let before_pool = pool_objects(root)?;
    let base_bytes = before_pool
        .values()
        .try_fold(0_u64, |sum, (_, len, _)| sum.checked_add(*len))
        .ok_or("base pool bytes overflow")?;

    let replaced = 100;
    let delta_stages = adapter
        .build_fixture_batch(&batch(
            g2,
            Some(g1),
            vec![scope(replaced, "fn replaced() { replacedsentinel }")?],
            &[],
            true,
        )?)?
        .ok_or("delta timing")?;
    if delta_stages.text_authority_ns == 0 {
        return Err("delta Native build timing absent".into());
    }
    let after_stats = adapter.text_authority_update_stats()?;
    let delta_root = native_root_wire(&generation_dir(root, g2))?;
    if delta_root.6.1 != expected_segments {
        return Err(format!(
            "delta has {} Native segments, expected {expected_segments}",
            delta_root.6.1
        )
        .into());
    }
    let written = after_stats
        .shards_written
        .checked_sub(before_stats.shards_written)
        .ok_or("written segment counter regressed")?;
    let inherited = after_stats
        .shards_inherited
        .checked_sub(before_stats.shards_inherited)
        .ok_or("inherited segment counter regressed")?;
    let compacted = after_stats
        .compaction_rederived_docs
        .checked_sub(before_stats.compaction_rederived_docs)
        .ok_or("compaction counter regressed")?;
    let compacted_bytes = after_stats
        .compaction_rederived_text_bytes
        .checked_sub(before_stats.compaction_rederived_text_bytes)
        .ok_or("compaction bytes regressed")?;
    let derived = after_stats
        .docs_derived
        .checked_sub(before_stats.docs_derived)
        .ok_or("derived counter regressed")?;
    let retired = after_stats
        .docs_retired
        .checked_sub(before_stats.docs_retired)
        .ok_or("retired counter regressed")?;
    let pages = after_stats
        .posting_pages_written
        .checked_sub(before_stats.posting_pages_written)
        .ok_or("posting page counter regressed")?;
    if (written, inherited, compacted, derived, retired)
        != (1, expected_segments - 1, SHARD_DOCS - 1, 1, 1)
        || compacted_bytes == 0
        || compacted_bytes > 2 * 1024 * 1024
        || pages == 0
    {
        return Err(format!("one-source delta rewrote unexpected Native work: before={before_stats:?} after={after_stats:?}").into());
    }
    let after_pool = pool_objects(root)?;
    for (name, facts) in &before_pool {
        if after_pool.get(name) != Some(facts) {
            return Err(format!("delta mutated inherited pool object {name}").into());
        }
    }
    let fresh_bytes = after_pool
        .iter()
        .filter(|(name, _)| !before_pool.contains_key(*name))
        .try_fold(0_u64, |sum, (_, (_, len, _))| sum.checked_add(*len))
        .ok_or("fresh pool bytes overflow")?;
    if base_bytes == 0 || fresh_bytes == 0 || fresh_bytes > base_bytes.div_euclid(2) {
        return Err(format!(
            "delta fresh pool bytes {fresh_bytes} exceed half of {base_bytes} base bytes"
        )
        .into());
    }
    emit_evidence(&[
        ("docs", COST_DOCS.to_string()),
        ("base_segments", expected_segments.to_string()),
        ("written_segments", written.to_string()),
        ("inherited_segments", inherited.to_string()),
        ("compacted_docs", compacted.to_string()),
        ("posting_pages", pages.to_string()),
        ("base_pool_bytes", base_bytes.to_string()),
        ("fresh_pool_bytes", fresh_bytes.to_string()),
    ]);
    if leaf_hit_ids(&adapter, g1, LqLeaf::Regex(format!("quartz_{replaced:05}")))?
        != vec![scope_chunk(replaced)]
        || !leaf_hit_ids(&adapter, g2, LqLeaf::Regex(format!("quartz_{replaced:05}")))?.is_empty()
        || leaf_hit_ids(&adapter, g2, LqLeaf::Regex("replacedsentinel".into()))?
            != vec![scope_chunk(replaced)]
        || leaf_hit_ids(
            &adapter,
            g2,
            LqLeaf::Phrase(format!("{EDGE_MID_MARKER} phraseanchor")),
        )? != vec![scope_chunk(EDGE_MID)]
    {
        return Err("base/replacement/inherited Native query golden differs".into());
    }
    Ok(())
}

/// The correctness oracle on shard boundaries.
///
/// After a delta that touches the last document of shard 0, the first of
/// shard 1 and the first of shard 2, every probe answers exactly as an
/// independent full rebuild.
#[test]
fn boundary_documents_answer_like_an_independent_rebuild() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(temp.path().to_path_buf());
    let g1 = ManifestGeneration::new(1);
    let g2 = ManifestGeneration::new(2);
    let g9 = ManifestGeneration::new(9);
    let _stages = adapter.build_fixture_batch(&base_batch(g1, DOCS)?)?;

    // Delta: rewrite the low edge in place (same marker, new tail), move the
    // mid edge to a fresh doc id with a new marker, tombstone the high edge,
    // and add a scope past every existing path.
    let low_body = format!("{} rewrittenlow", scope_body(EDGE_LOW));
    let mid_body = "fn moved() { edgemovedmarker phraseanchor edgemovedmarker_tail }".to_string();
    let new_index = DOCS + 7;
    let new_body = "fn added() { addedmarker phraseanchor addedmarker_tail }";
    let _stages = adapter.build_fixture_batch(&batch(
        g2,
        Some(g1),
        vec![
            scope(EDGE_LOW, &low_body)?,
            scope(EDGE_MID, &mid_body)?,
            scope(new_index, new_body)?,
        ],
        &[EDGE_HIGH],
        true,
    )?)?;

    // Oracle: the same final corpus, built fresh with no base.
    let mut scopes = Vec::with_capacity(DOCS);
    for index in 0..DOCS {
        if index == EDGE_HIGH {
            continue;
        }
        let body = match index {
            EDGE_LOW => low_body.clone(),
            EDGE_MID => mid_body.clone(),
            _ => scope_body(index),
        };
        scopes.push(scope(index, &body)?);
    }
    scopes.push(scope(new_index, new_body)?);
    let _stages = adapter.build_fixture_batch(&batch(g9, None, scopes, &[], true)?)?;

    let mut probes: Vec<(String, LqLeaf)> = Vec::new();
    for marker in [
        EDGE_LOW_MARKER,
        EDGE_MID_MARKER,
        EDGE_HIGH_MARKER,
        "edgemovedmarker",
        "addedmarker",
        "rewrittenlow",
    ] {
        probes.push((format!("regex {marker}"), LqLeaf::Regex(marker.to_string())));
        probes.push((
            format!("keyword {marker}"),
            LqLeaf::Keyword(marker.to_string()),
        ));
        probes.push((
            format!("phrase {marker}"),
            LqLeaf::Phrase(format!("{marker} phraseanchor")),
        ));
        probes.push((
            format!("raw {marker}"),
            LqLeaf::RawString(format!("{marker}_tail")),
        ));
    }
    probes.push((
        "regex all edges".to_string(),
        LqLeaf::Regex("edge[a-z]+marker".to_string()),
    ));
    probes.push((
        "regex neighbours of the boundaries".to_string(),
        LqLeaf::Regex("quartz_0(?:204[5-9]|409[4-7])".to_string()),
    ));

    let mut divergences: Vec<String> = Vec::new();
    for (label, leaf) in probes {
        let incremental = leaf_hit_ids(&adapter, g2, leaf.clone())?;
        let rebuilt = leaf_hit_ids(&adapter, g9, leaf)?;
        if incremental != rebuilt {
            divergences.push(format!(
                "{label}: incremental={incremental:?} rebuild={rebuilt:?}"
            ));
        }
    }
    if !divergences.is_empty() {
        return Err(format!(
            "the sharded delta diverged from the independent rebuild:\n  {}",
            divergences.join("\n  ")
        )
        .into());
    }

    // The oracle is not agreement on nothing.
    if leaf_hit_ids(&adapter, g9, LqLeaf::Regex(EDGE_HIGH_MARKER.to_string()))?
        != Vec::<String>::new()
    {
        return Err("the oracle must not hold the tombstoned high edge".into());
    }
    if leaf_hit_ids(
        &adapter,
        g9,
        LqLeaf::Phrase("edgemovedmarker phraseanchor".to_string()),
    )? != vec![scope_chunk(EDGE_MID)]
    {
        return Err("the oracle must hold the moved mid edge".into());
    }
    if leaf_hit_ids(
        &adapter,
        g9,
        LqLeaf::RawString("addedmarker_tail".to_string()),
    )? != vec![scope_chunk(new_index)]
    {
        return Err("the oracle must hold the added scope".into());
    }
    if leaf_hit_ids(&adapter, g9, LqLeaf::Regex("edge[a-z]+marker".to_string()))?
        != vec![scope_chunk(EDGE_LOW), scope_chunk(EDGE_MID)]
    {
        return Err("the oracle must hold the low edge and the moved mid edge only".into());
    }
    Ok(())
}

fn small_batch(
    generation: ManifestGeneration,
    base: Option<ManifestGeneration>,
    indexes: &[usize],
    seal: bool,
) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    let mut scopes = Vec::with_capacity(indexes.len());
    for index in indexes {
        scopes.push(scope(*index, &scope_body(*index))?);
    }
    batch(generation, base, scopes, &[], seal)
}

fn typed_code<T: std::fmt::Debug>(result: Result<T, CoreError>) -> Result<String, Box<dyn Error>> {
    match result {
        Err(CoreError::Typed { code, .. }) => Ok(code.to_string()),
        other => Err(format!("expected a typed refusal, got {other:?}").into()),
    }
}

fn open_code(adapter: &LexicalAdapter, generation: ManifestGeneration) -> Option<String> {
    match adapter.open(
        &repo(),
        &revision(),
        generation,
        &quanta_index_core::RequestBudgetV1::unbounded(),
    ) {
        Err(CoreError::Typed { code, .. }) => Some(code.to_string()),
        _ => None,
    }
}

/// A sealed root is immutable. Its own bytes, presence, and regular-file
/// type are checked by both cold admission doors.
#[test]
fn changed_native_root_is_refused_at_both_doors() -> TestResult {
    for fault in ["flip", "missing", "symlink"] {
        let temp = tempfile::tempdir()?;
        let adapter = LexicalAdapter::with_state_root(temp.path().to_path_buf());
        let g1 = ManifestGeneration::new(1);
        let _stages = adapter.build_fixture_batch(&small_batch(g1, None, &[0, 1, 2], true)?)?;
        let directory = generation_dir(temp.path(), g1);
        let path = directory
            .join(TEXT_AUTHORITY_DIR)
            .join(TEXT_AUTHORITY_MANIFEST);
        match fault {
            "flip" => {
                let mut bytes = std::fs::read(&path)?;
                let last = bytes.last_mut().ok_or("empty Native root")?;
                *last ^= 1;
                std::fs::write(&path, bytes)?;
            }
            "missing" => std::fs::remove_file(&path)?,
            "symlink" => {
                std::fs::remove_file(&path)?;
                std::os::unix::fs::symlink(directory.join(SEALED_MANIFEST), &path)?;
            }
            _ => return Err("unknown fault".into()),
        }
        let validate = typed_code(adapter.validate_generation_identity(&identity(g1)))?;
        if validate != "GENERATION_SIDECAR_CORRUPT"
            || open_code(&adapter, g1).as_deref() != Some("GENERATION_SIDECAR_CORRUPT")
        {
            return Err(format!("{fault}: root refused incorrectly: {validate}").into());
        }
    }
    Ok(())
}

/// The root stays sealed and valid; a selected current-map read authenticates
/// its physical page at query time. Hash mismatch, absence, and symlink must
/// all refuse without returning an incomplete answer.
#[test]
fn selected_native_pool_page_faults_are_typed() -> TestResult {
    for fault in ["flip", "missing", "symlink"] {
        let temp = tempfile::tempdir()?;
        let adapter = LexicalAdapter::with_state_root(temp.path().to_path_buf());
        let g1 = ManifestGeneration::new(1);
        let _stages = adapter.build_fixture_batch(&small_batch(g1, None, &[0, 1, 2], true)?)?;
        let budget = RequestBudgetV1::unbounded();
        let searcher = adapter.open(&repo(), &revision(), g1, &budget)?;
        let directory = generation_dir(temp.path(), g1);
        let page = current_page_path(temp.path(), &directory)?;
        match fault {
            "flip" => {
                let mut bytes = std::fs::read(&page)?;
                let last = bytes.last_mut().ok_or("empty current page")?;
                *last ^= 1;
                std::fs::write(&page, bytes)?;
            }
            "missing" => std::fs::remove_file(&page)?,
            "symlink" => {
                std::fs::remove_file(&page)?;
                std::os::unix::fs::symlink(directory.join(SEALED_MANIFEST), &page)?;
            }
            _ => return Err("unknown fault".into()),
        }
        let code = typed_code(searcher.search(
            &leaf_query(LqLeaf::Regex("quartz_00000".into())),
            64,
            &budget,
        ))?;
        if code != "GENERATION_SIDECAR_CORRUPT" {
            return Err(format!("{fault}: selected Native page refusal was {code}").into());
        }
    }
    Ok(())
}

#[test]
fn a_delta_refuses_a_corrupt_selected_base_page() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(temp.path().to_path_buf());
    let g1 = ManifestGeneration::new(1);
    let g2 = ManifestGeneration::new(2);
    let _stages = adapter.build_fixture_batch(&small_batch(g1, None, &[0, 1, 2], true)?)?;
    let page = current_page_path(temp.path(), &generation_dir(temp.path(), g1))?;
    let mut bytes = std::fs::read(&page)?;
    let middle = bytes.len().div_euclid(2);
    let byte = bytes.get_mut(middle).ok_or("empty current page")?;
    *byte ^= 1;
    std::fs::write(&page, bytes)?;
    let code = typed_code(adapter.build_fixture_batch(&small_batch(g2, Some(g1), &[1], true)?))?;
    if code != "GENERATION_SIDECAR_CORRUPT"
        || adapter
            .open(&repo(), &revision(), g2, &RequestBudgetV1::unbounded())
            .is_ok()
    {
        return Err(format!("corrupt base page delta admission differed: {code}").into());
    }
    Ok(())
}

// Canonical source publication refuses a partial unsealed batch before it
// creates an index or text-authority sidecar. A later sealed publication owns
// the complete generation and answers only its admitted files.
#[test]
fn an_unsealed_source_batch_cannot_leave_index_authority_skew() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let g1 = ManifestGeneration::new(1);
    let dir = generation_dir(&root, g1);

    let refused = adapter.build_fixture_batch(&small_batch(g1, None, &[0, 1, 2], false)?);
    if !matches!(refused, Err(CoreError::InvalidContract(_))) {
        return Err(format!("unsealed source batch was not rejected: {refused:?}").into());
    }
    if dir.exists() {
        return Err("rejected source batch created a generation directory".into());
    }

    let _stages = adapter.build_fixture_batch(&small_batch(g1, None, &[0, 1, 2], true)?)?;
    let observed = leaf_hit_ids(&adapter, g1, LqLeaf::Regex("quartz_00000".to_string()))?;
    if observed != vec![scope_chunk(0)] {
        return Err(format!("sealed source batch returned {observed:?}").into());
    }
    let absent = leaf_hit_ids(&adapter, g1, LqLeaf::Regex("quartz_00004".to_string()))?;
    if !absent.is_empty() {
        return Err(format!("rejected source leaked rows: {absent:?}").into());
    }
    Ok(())
}

/// A generation sealed under the whole-corpus text-authority layout is
/// refused by name at both doors: the migration is a rebuild.
#[test]
fn a_whole_corpus_layout_generation_is_refused_by_text_authority_format() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let g1 = ManifestGeneration::new(1);
    let _stages = adapter.build_fixture_batch(&small_batch(g1, None, &[0, 1], true)?)?;
    let dir = generation_dir(&root, g1);
    let manifest = dir.join(SEALED_MANIFEST);
    let mut value: ciborium::Value = ciborium::from_reader(std::fs::read(&manifest)?.as_slice())?;
    let ciborium::Value::Array(items) = &mut value else {
        return Err("the sealed manifest is a fixed-order array".into());
    };
    let format = items.first_mut().ok_or("a leading format version")?;
    // Format 2 is the whole-corpus layout; only the version gate is
    // rewritten so the refusal is proved to be by name.
    *format = ciborium::Value::Integer(2_u32.into());
    let mut bytes = Vec::new();
    ciborium::into_writer(&value, &mut bytes)?;
    std::fs::write(&manifest, bytes)?;

    let validate = typed_code(adapter.validate_generation_identity(&identity(g1)))?;
    if validate != "GENERATION_TEXT_AUTHORITY_FORMAT_UNSUPPORTED" {
        return Err(format!("the validator refused with {validate}").into());
    }
    if open_code(&adapter, g1).as_deref() != Some("GENERATION_TEXT_AUTHORITY_FORMAT_UNSUPPORTED") {
        return Err("the open must refuse the whole-corpus layout by name".into());
    }
    // A delta on such a base is refused the same way: nothing is inherited
    // from a layout this build cannot read.
    let g2 = ManifestGeneration::new(2);
    let delta = typed_code(adapter.build_fixture_batch(&small_batch(g2, Some(g1), &[1], true)?))?;
    if delta != "GENERATION_TEXT_AUTHORITY_FORMAT_UNSUPPORTED" {
        return Err(format!("the delta refused with {delta}").into());
    }
    Ok(())
}
