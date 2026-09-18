//! QI-BB-006, second half — the text authority is sharded by doc-id range
//! so a delta's write bytes are proportional to what it changes.
//!
//! A generation's text authority lives under `text-authority/`: a bounded
//! manifest plus one immutable file per shard of `SHARD_DOCS` documents.
//! A delta rewrites only the shards holding a retired or added document
//! and inherits every other shard file from its base by hard link. These
//! tests pin that from the outside, with independent oracles:
//!
//! - the **bytes oracle** measures inodes and sizes on disk: after a
//!   one-scope delta on a base of four shards' worth of documents, the
//!   files whose inode is not the base's are at most two shards and the
//!   manifest, and every other shard file *is* the base's inode;
//! - the **correctness oracle** is an independent full rebuild of the same
//!   final corpus, compared probe by probe on documents that sit exactly on
//!   shard boundaries;
//! - the **refusal oracle** is fault injection on the real files and the
//!   real manifest: a disagreement is a typed refusal, and the generation
//!   never seals or opens.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, GenerationSnapshot, LQ_VERSION_TAG, LqExpr, LqLeaf,
    LqOptions, LqQuery, LqSpan, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusIngestBatch, SearchCorpusReplaceScope, SearchCorpusTombstoneScope,
    SearchPlaneTrackKind, SearchScopeKey, SearchScopeSurface,
};
use quanta_index_core::{
    CoreError, GenerationIdentityValidatePort, GenerationStorageKeyV1, LexicalIndexOpenPort,
    RequestBudgetV1, SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;

type TestResult = Result<(), Box<dyn Error>>;

/// Documents per shard, as the adapter lays them out. The bytes oracle
/// pins it independently: a base of `COST_DOCS` documents must land in
/// exactly `ceil(COST_DOCS / SHARD_DOCS)` shard files.
const SHARD_DOCS: u64 = 2048;
/// The correctness fixture: three shards, the last holding two documents,
/// so every boundary position below exists.
const DOCS: usize = 2 * 2048 + 2;
/// The cost fixture: four shards, the last one partial, so the untouched
/// shards are the bulk of the base and the byte budget is not a coin toss
/// between two of three shards.
const COST_DOCS: usize = 3 * 2048 + 50;
/// Sorted scope positions whose doc ids sit on shard boundaries.
///
/// The last document of shard 0, the first of shard 1 and the first of
/// shard 2. Doc ids are handed out in op order, and the fixture orders its
/// scopes by path, so scope `i` is doc id `i + 1`.
const EDGE_LOW: usize = 2046;
const EDGE_MID: usize = 2047;
const EDGE_HIGH: usize = 4095;
const EDGE_LOW_MARKER: &str = "edgelowmarker";
const EDGE_MID_MARKER: &str = "edgemidmarker";
const EDGE_HIGH_MARKER: &str = "edgehighmarker";
const TEXT_AUTHORITY_DIR: &str = "text-authority";
const TEXT_AUTHORITY_MANIFEST: &str = "manifest.cbor";
const SEALED_MANIFEST: &str = "search-corpus-generation-manifest.cbor";

fn repo() -> RepoId {
    RepoId::new("shard-repo")
}

fn revision() -> RevisionId {
    RevisionId::new("shard-rev")
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
    Ok(SearchCorpusReplaceScope {
        scope: SearchScopeKey {
            doc_surface: SearchScopeSurface::File,
            repo_relative_path: RepoRelativePath::new(&path),
        },
        scope_digest: format!("scope:{path}:{body}"),
        chunks: vec![ChunkRecord {
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
        symbols: Vec::new(),
    })
}

fn batch(
    generation: ManifestGeneration,
    base: Option<ManifestGeneration>,
    replace_scopes: Vec<SearchCorpusReplaceScope>,
    tombstone_paths: &[usize],
    seal: bool,
) -> SearchCorpusIngestBatch {
    let mut replace_scopes = replace_scopes;
    replace_scopes.sort_by(|left, right| {
        left.scope
            .repo_relative_path
            .as_str()
            .cmp(right.scope.repo_relative_path.as_str())
    });
    SearchCorpusIngestBatch {
        repo_id: repo(),
        revision_id: revision(),
        generation,
        base_generation: base,
        manifest_digest: format!("shard-manifest:{}", generation.get()),
        batch_digest: format!("shard-batch:{}:{}", generation.get(), replace_scopes.len()),
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
                scope: SearchScopeKey {
                    doc_surface: SearchScopeSurface::File,
                    repo_relative_path: RepoRelativePath::new(scope_path(*index)),
                },
            })
            .collect(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal,
    }
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
    Ok(batch(generation, None, scopes, &[], true))
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
    let searcher = adapter.open(&repo(), &revision(), generation)?;
    let mut ids: Vec<String> = searcher
        .search(&leaf_query(leaf), 64, &RequestBudgetV1::unbounded())?
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    ids.sort();
    Ok(ids)
}

/// One text-authority file: its path, size and inode.
struct AuthorityFile {
    path: PathBuf,
    bytes: u64,
    inode: u64,
}

impl AuthorityFile {
    fn is_manifest(&self) -> bool {
        self.path.file_name().and_then(|name| name.to_str()) == Some(TEXT_AUTHORITY_MANIFEST)
    }
}

fn text_authority_files(generation_dir: &Path) -> Result<Vec<AuthorityFile>, Box<dyn Error>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(generation_dir.join(TEXT_AUTHORITY_DIR))? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            return Err(format!("unexpected non-file entry {}", entry.path().display()).into());
        }
        files.push(AuthorityFile {
            path: entry.path(),
            bytes: metadata.len(),
            inode: metadata.ino(),
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn shard_files(files: &[AuthorityFile]) -> Vec<&AuthorityFile> {
    files.iter().filter(|file| !file.is_manifest()).collect()
}

/// Machine-readable cost evidence; visible with `-- --nocapture`.
#[expect(
    clippy::print_stdout,
    reason = "QI-BB-006 is a cost claim, so the measured byte counts belong in the run log the finding cites"
)]
fn emit_evidence(fields: &[(&str, String)]) {
    let rendered: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    println!("QI-BB-006-SHARD-EVIDENCE {}", rendered.join(" "));
}

/// The bytes oracle: a one-scope delta writes at most two shards and the
/// manifest, and every other shard file is the base's inode.
#[test]
fn one_scope_delta_rewrites_touched_shards_and_links_the_rest() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let g1 = ManifestGeneration::new(1);
    let g2 = ManifestGeneration::new(2);

    adapter.build_batch(&base_batch(g1, COST_DOCS)?)?;
    let base_files = text_authority_files(&generation_dir(&root, g1))?;
    let base_shards = shard_files(&base_files);
    let expected_shards = u64::try_from(COST_DOCS)?.div_ceil(SHARD_DOCS);
    if u64::try_from(base_shards.len())? != expected_shards {
        return Err(format!(
            "base holds {} shard files for {COST_DOCS} documents; expected {expected_shards} at {SHARD_DOCS} per shard",
            base_shards.len()
        )
        .into());
    }
    let base_inodes: BTreeSet<u64> = base_files.iter().map(|file| file.inode).collect();
    let base_bytes: u64 = base_files
        .iter()
        .fold(0_u64, |total, file| total.saturating_add(file.bytes));
    let before = adapter.text_authority_update_stats()?;

    // Replace one scope in shard 0; its replacement takes the next doc id,
    // which lands in the last shard.
    let replaced = 100;
    adapter.build_batch(&batch(
        g2,
        Some(g1),
        vec![scope(replaced, "fn replaced() { replacedsentinel }")?],
        &[],
        true,
    ))?;
    let after = adapter.text_authority_update_stats()?;
    let delta_files = text_authority_files(&generation_dir(&root, g2))?;
    let delta_shards = shard_files(&delta_files);
    let (linked, written): (Vec<&AuthorityFile>, Vec<&AuthorityFile>) = delta_shards
        .iter()
        .copied()
        .partition(|file| base_inodes.contains(&file.inode));
    let manifest_is_new = delta_files
        .iter()
        .any(|file| file.is_manifest() && !base_inodes.contains(&file.inode));
    let delta_new_bytes: u64 = delta_files
        .iter()
        .filter(|file| !base_inodes.contains(&file.inode))
        .fold(0_u64, |total, file| total.saturating_add(file.bytes));
    let written_names: Vec<String> = written
        .iter()
        .map(|file| {
            file.path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .map(Option::unwrap_or_default)
        .collect();
    emit_evidence(&[
        ("docs", COST_DOCS.to_string()),
        ("base_bytes", base_bytes.to_string()),
        ("delta_new_bytes", delta_new_bytes.to_string()),
        ("shards", base_shards.len().to_string()),
        ("touched", written.len().to_string()),
        ("linked", linked.len().to_string()),
        ("written_shards", written_names.join(",")),
    ]);

    if !manifest_is_new {
        return Err("the delta must publish its own manifest, not share the base's".into());
    }
    if written.len() > 2 {
        return Err(format!(
            "a one-scope delta rewrote {} shards ({written_names:?}); at most two may change",
            written.len()
        )
        .into());
    }
    if linked.is_empty() {
        return Err(
            "no shard of the delta is the base's inode; untouched shards were rewritten".into(),
        );
    }
    if delta_shards.len() != written.len().saturating_add(linked.len()) {
        return Err("every shard of the delta is either written or linked".into());
    }
    let budget = base_bytes.saturating_div(2);
    if delta_new_bytes > budget {
        return Err(format!(
            "the delta wrote {delta_new_bytes} new text-authority bytes against a {base_bytes}-byte base (budget {budget})"
        )
        .into());
    }
    // The metric twin of the bytes oracle.
    let shards_written = after.shards_written.saturating_sub(before.shards_written);
    let shards_inherited = after
        .shards_inherited
        .saturating_sub(before.shards_inherited);
    if shards_written != u64::try_from(written.len())?
        || shards_inherited != u64::try_from(linked.len())?
    {
        return Err(format!(
            "stats say {shards_written} written / {shards_inherited} inherited, disk says {} / {}",
            written.len(),
            linked.len()
        )
        .into());
    }
    if before.shards_written != expected_shards || before.shards_inherited != 0 {
        return Err(format!("the base rebuild writes every shard once: {before:?}").into());
    }

    // Both generations serve: the base its old text, the delta the new.
    if leaf_hit_ids(&adapter, g1, LqLeaf::Regex(format!("quartz_{replaced:05}")))?
        != vec![scope_chunk(replaced)]
    {
        return Err("the base must still serve the replaced scope's old text".into());
    }
    if leaf_hit_ids(&adapter, g2, LqLeaf::Regex(format!("quartz_{replaced:05}")))?
        != Vec::<String>::new()
    {
        return Err("the delta must not serve the replaced scope's old text".into());
    }
    if leaf_hit_ids(&adapter, g2, LqLeaf::Regex("replacedsentinel".to_string()))?
        != vec![scope_chunk(replaced)]
    {
        return Err("the delta must serve the replacement".into());
    }
    if leaf_hit_ids(
        &adapter,
        g2,
        LqLeaf::Phrase(format!("{EDGE_MID_MARKER} phraseanchor")),
    )? != vec![scope_chunk(EDGE_MID)]
    {
        return Err("the delta must serve an inherited shard's phrase".into());
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
    adapter.build_batch(&base_batch(g1, DOCS)?)?;

    // Delta: rewrite the low edge in place (same marker, new tail), move the
    // mid edge to a fresh doc id with a new marker, tombstone the high edge,
    // and add a scope past every existing path.
    let low_body = format!("{} rewrittenlow", scope_body(EDGE_LOW));
    let mid_body = "fn moved() { edgemovedmarker phraseanchor edgemovedmarker_tail }".to_string();
    let new_index = DOCS + 7;
    let new_body = "fn added() { addedmarker phraseanchor addedmarker_tail }";
    adapter.build_batch(&batch(
        g2,
        Some(g1),
        vec![
            scope(EDGE_LOW, &low_body)?,
            scope(EDGE_MID, &mid_body)?,
            scope(new_index, new_body)?,
        ],
        &[EDGE_HIGH],
        true,
    ))?;

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
    adapter.build_batch(&batch(g9, None, scopes, &[], true))?;

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
    Ok(batch(generation, base, scopes, &[], seal))
}

fn typed_code(result: Result<(), CoreError>) -> Result<String, Box<dyn Error>> {
    match result {
        Err(CoreError::Typed { code, .. }) => Ok(code),
        other => Err(format!("expected a typed refusal, got {other:?}").into()),
    }
}

fn open_code(adapter: &LexicalAdapter, generation: ManifestGeneration) -> Option<String> {
    match adapter.open(&repo(), &revision(), generation) {
        Err(CoreError::Typed { code, .. }) => Some(code),
        _ => None,
    }
}

/// The manifest's wire row, as the refusal oracle rewrites it.
type ManifestRow = (
    u32,
    u64,
    (u16, u16),
    u64,
    Vec<(u64, u64, u64, u64, u64, [u8; 32])>,
);

fn read_manifest_row(generation_dir: &Path) -> Result<ManifestRow, Box<dyn Error>> {
    let bytes = std::fs::read(
        generation_dir
            .join(TEXT_AUTHORITY_DIR)
            .join(TEXT_AUTHORITY_MANIFEST),
    )?;
    Ok(ciborium::from_reader(bytes.as_slice())?)
}

fn write_manifest_row(generation_dir: &Path, row: &ManifestRow) -> TestResult {
    let mut bytes = Vec::new();
    ciborium::into_writer(row, &mut bytes)?;
    std::fs::write(
        generation_dir
            .join(TEXT_AUTHORITY_DIR)
            .join(TEXT_AUTHORITY_MANIFEST),
        bytes,
    )?;
    Ok(())
}

/// The refusal oracle: a text authority whose manifest lies never seals.
///
/// A duplicate shard, a listed shard without its file, a manifest that
/// disowns the index's documents, a shard whose digest is not the listed
/// one, another format or normalizer — each is refused typed at the seal,
/// and the generation never opens. Each fault is injected into a fresh
/// unsealed generation so the manifest itself is the thing under test, not
/// the seal's tree digest.
#[test]
fn a_text_authority_that_disagrees_with_its_manifest_never_seals_or_opens() -> TestResult {
    type Fault = fn(&Path, &mut ManifestRow) -> TestResult;
    let cases: Vec<(&str, &str, Fault)> = vec![
        (
            "duplicate shard",
            "GENERATION_SIDECAR_CORRUPT",
            |_dir, row| {
                let first = row.4.first().copied().ok_or("a shard")?;
                row.4.push(first);
                Ok(())
            },
        ),
        (
            "listed shard without its file",
            "GENERATION_SIDECAR_CORRUPT",
            |dir, row| {
                let shard = row.4.first().ok_or("a shard")?;
                std::fs::remove_file(
                    dir.join(TEXT_AUTHORITY_DIR)
                        .join(shard_file_name(shard.0, &shard.5)),
                )?;
                Ok(())
            },
        ),
        (
            "manifest that lists no shard for the index's documents",
            "GENERATION_SIDECAR_CORRUPT",
            |dir, row| {
                // The file stays; the manifest disowns it. The publish
                // semantics drop an unowned file, so what refuses the seal
                // is the authority covering fewer documents than the index.
                let shard = row.4.pop().ok_or("a shard")?;
                let path = dir
                    .join(TEXT_AUTHORITY_DIR)
                    .join(shard_file_name(shard.0, &shard.5));
                if !path.is_file() {
                    return Err("the disowned shard's file must remain".into());
                }
                Ok(())
            },
        ),
        (
            "shard content other than the listed digest",
            "GENERATION_SIDECAR_CORRUPT",
            |dir, row| {
                let shard = row.4.first().ok_or("a shard")?;
                let path = dir
                    .join(TEXT_AUTHORITY_DIR)
                    .join(shard_file_name(shard.0, &shard.5));
                let mut bytes = std::fs::read(&path)?;
                let last = bytes.len().checked_sub(1).ok_or("empty shard")?;
                let byte = bytes.get_mut(last).ok_or("index")?;
                *byte ^= 0xff;
                std::fs::write(&path, bytes)?;
                Ok(())
            },
        ),
        (
            "another text-authority format",
            "GENERATION_TEXT_AUTHORITY_FORMAT_UNSUPPORTED",
            |_dir, row| {
                row.0 = row.0.wrapping_add(1);
                Ok(())
            },
        ),
        (
            "another shard size",
            "GENERATION_TEXT_AUTHORITY_FORMAT_UNSUPPORTED",
            |_dir, row| {
                row.1 = row.1.wrapping_mul(2);
                Ok(())
            },
        ),
        (
            "another normalizer",
            "GENERATION_NORMALIZER_UNSUPPORTED",
            |_dir, row| {
                row.2 = (row.2.0.wrapping_add(7), row.2.1);
                Ok(())
            },
        ),
    ];
    for (label, expected_code, fault) in cases {
        let temp = tempfile::tempdir()?;
        let root = temp.path().to_path_buf();
        let adapter = LexicalAdapter::with_state_root(root.clone());
        let g1 = ManifestGeneration::new(1);
        adapter.build_batch(&small_batch(g1, None, &[0, 1, 2], false)?)?;
        let dir = generation_dir(&root, g1);
        let mut row = read_manifest_row(&dir)?;
        fault(&dir, &mut row)?;
        write_manifest_row(&dir, &row)?;

        // The seal is the first door: it reads the manifest and measures the
        // tree, and must refuse under the typed code.
        let seal = adapter.build_batch(&small_batch(g1, None, &[], true)?);
        let code = typed_code(seal)
            .map_err(|err| -> Box<dyn Error> { format!("{label}: seal: {err}").into() })?;
        if code != expected_code {
            return Err(
                format!("{label}: seal refused with {code}, expected {expected_code}").into(),
            );
        }
        // Never sealed, so never opened: the identity was never written.
        if open_code(&adapter, g1).as_deref() != Some("GENERATION_IDENTITY_INCOMPLETE") {
            return Err(
                format!("{label}: a refused seal must leave the generation unopenable").into(),
            );
        }
        if adapter.validate_generation_identity(&identity(g1)).is_ok() {
            return Err(
                format!("{label}: the validator must not admit an unsealed generation").into(),
            );
        }
    }
    Ok(())
}

/// `shard-<index>-<first eight digest bytes as hex>.cbor`, as the adapter
/// names shard files.
fn shard_file_name(index: u64, sha256: &[u8; 32]) -> String {
    let prefix = sha256
        .iter()
        .take(8)
        .fold(0_u64, |word, byte| word.wrapping_shl(8) | u64::from(*byte));
    format!("shard-{index:08}-{prefix:016x}.cbor")
}

/// The incremental writer proves the shards it loads: a shard whose bytes
/// no longer match the manifest is refused typed by the next delta that
/// touches it, instead of being carried into a new generation.
#[test]
fn a_delta_refuses_to_build_on_a_shard_whose_digest_changed() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let g1 = ManifestGeneration::new(1);
    adapter.build_batch(&small_batch(g1, None, &[0, 1, 2], true)?)?;
    let dir = generation_dir(&root, g1);
    let row = read_manifest_row(&dir)?;
    let shard = row.4.first().ok_or("a shard")?;
    let path = dir
        .join(TEXT_AUTHORITY_DIR)
        .join(shard_file_name(shard.0, &shard.5));
    let mut bytes = std::fs::read(&path)?;
    let middle = bytes.len().div_euclid(2);
    let byte = bytes.get_mut(middle).ok_or("index")?;
    *byte ^= 0x01;
    std::fs::write(&path, bytes)?;

    let g2 = ManifestGeneration::new(2);
    let delta = adapter.build_batch(&small_batch(g2, Some(g1), &[1], true)?);
    let code = typed_code(delta)?;
    if code != "GENERATION_SIDECAR_CORRUPT" {
        return Err(
            format!("the delta refused with {code}, expected GENERATION_SIDECAR_CORRUPT").into(),
        );
    }
    if open_code(&adapter, g2).as_deref() != Some("GENERATION_IDENTITY_INCOMPLETE") {
        return Err("a refused delta must leave its generation unopenable".into());
    }
    Ok(())
}

/// One file of the text-authority directory: its name and bytes.
type NamedFileBytes = (String, Vec<u8>);

/// Every file of the text-authority directory, by name.
fn snapshot_text_authority(generation_dir: &Path) -> Result<Vec<NamedFileBytes>, Box<dyn Error>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(generation_dir.join(TEXT_AUTHORITY_DIR))? {
        let entry = entry?;
        files.push((
            entry.file_name().to_string_lossy().into_owned(),
            std::fs::read(entry.path())?,
        ));
    }
    files.sort();
    Ok(files)
}

fn restore_text_authority(generation_dir: &Path, files: &[(String, Vec<u8>)]) -> TestResult {
    let dir = generation_dir.join(TEXT_AUTHORITY_DIR);
    for entry in std::fs::read_dir(&dir)? {
        std::fs::remove_file(entry?.path())?;
    }
    for (name, bytes) in files {
        std::fs::write(dir.join(name), bytes)?;
    }
    Ok(())
}

/// A publish that crashed after its index commit is caught up by the
/// next batch's rebuild.
///
/// The crash leaves the index ahead of the authority; the next batch that
/// retires one of the unlisted documents catches up by a full derivation,
/// continues doc ids past what the index stores, and the generation seals
/// consistent. The crash is staged by publishing twice and restoring the first
/// publish's `text-authority/` files over the second's.
#[test]
fn a_publish_that_crashed_after_its_commit_is_caught_up_by_a_rebuild() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let g1 = ManifestGeneration::new(1);
    let dir = generation_dir(&root, g1);

    adapter.build_batch(&small_batch(g1, None, &[0, 1, 2], false)?)?;
    let first_publish = snapshot_text_authority(&dir)?;
    // Scopes 3 and 4 take doc ids 4 and 5; then their publish "never
    // happened".
    adapter.build_batch(&small_batch(g1, None, &[3, 4], false)?)?;
    restore_text_authority(&dir, &first_publish)?;
    let before = adapter.text_authority_update_stats()?;

    // Replacing scope 3 names doc 4 for retirement, which the restored
    // authority never listed.
    adapter.build_batch(&batch(
        g1,
        None,
        vec![scope(3, "fn replaced() { caughtupsentinel }")?],
        &[],
        true,
    ))?;
    let after = adapter.text_authority_update_stats()?;
    if after.rebuilds.saturating_sub(before.rebuilds) != 1
        || after.incremental_updates != before.incremental_updates
    {
        return Err(format!(
            "an index ahead of its authority must be caught up by exactly one rebuild: before={before:?} after={after:?}"
        )
        .into());
    }
    // Every live document answers, the unlisted survivor included, and the
    // retired text is gone.
    for (label, leaf, expected) in [
        (
            "unlisted survivor",
            LqLeaf::Regex("quartz_00004".to_string()),
            vec![scope_chunk(4)],
        ),
        (
            "replacement",
            LqLeaf::Regex("caughtupsentinel".to_string()),
            vec![scope_chunk(3)],
        ),
        (
            "retired text",
            LqLeaf::Regex("quartz_00003".to_string()),
            Vec::new(),
        ),
        (
            "phrase over every live document",
            LqLeaf::Phrase("ipsum dolor sit".to_string()),
            vec![
                scope_chunk(0),
                scope_chunk(1),
                scope_chunk(2),
                scope_chunk(4),
            ],
        ),
        (
            "everything",
            LqLeaf::Regex("quartz_0000[0-9]".to_string()),
            vec![
                scope_chunk(0),
                scope_chunk(1),
                scope_chunk(2),
                scope_chunk(4),
            ],
        ),
    ] {
        let observed = leaf_hit_ids(&adapter, g1, leaf)?;
        if observed != expected {
            return Err(format!("{label}: expected {expected:?}, got {observed:?}").into());
        }
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
    adapter.build_batch(&small_batch(g1, None, &[0, 1], true)?)?;
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
    let delta = typed_code(adapter.build_batch(&small_batch(g2, Some(g1), &[1], true)?))?;
    if delta != "GENERATION_TEXT_AUTHORITY_FORMAT_UNSUPPORTED" {
        return Err(format!("the delta refused with {delta}").into());
    }
    Ok(())
}
