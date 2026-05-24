//! Lexical index driven adapter backed by Tantivy 0.22.
//!
//! Implements `SearchPlaneLexicalIndexBuildPort` and
//! `SearchPlaneLexicalIndexStorePort` against an on-disk index laid out as
//! `{state_root}/lexical/{manifest_generation}/`.
//!
//! Phase 1 wire format (D15): `LexicalBuildInput::chunk_rows` is JSON-encoded
//! `Vec<ChunkRow>`; `symbol_rows` is accepted but not indexed yet.

#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]
#![expect(
    clippy::multiple_crate_versions,
    reason = "tantivy 0.22 pulls multiple transitive versions (rustix, linux-raw-sys, windows-sys, wit-bindgen) we cannot collapse; scoped allowance in deny.toml [bans] skip-tree."
)]
#![expect(
    clippy::redundant_pub_crate,
    reason = "schema items use pub(crate) because unreachable_pub forbids bare pub in private modules; both lints cannot be simultaneously satisfied without a module re-export gymnastics that hides the schema type from clippy."
)]

mod row;
mod schema;

use std::path::{Path, PathBuf};

use quanta_index_contract::{PublishedGenerationSet, PublishedSearchBundleManifest};
use quanta_index_core::{
    CoreError, LexicalBuildInput, SearchPlaneLexicalIndexBuildPort,
    SearchPlaneLexicalIndexStorePort,
};
use tantivy::{Index, IndexWriter, doc};

pub use row::ChunkRow;

/// Sentinel file written at the end of a successful build.
const MARKER_OK: &str = "MARKER_OK";

/// Suffix appended to the target directory while the build is in flight.
const BUILDING_SUFFIX: &str = ".building";

/// Memory budget given to the Tantivy writer.
///
/// 50 MiB matches the value used in the upstream `basic_search` example and
/// is plenty for Phase 1 (per-generation full rebuild).
const WRITER_MEMORY_BUDGET: usize = 50_000_000;

/// Tantivy-backed lexical adapter.
///
/// State layout: `{state_root}/lexical/{manifest_generation}/`. The adapter
/// owns no open handles between calls in Phase 1; each build opens, writes,
/// commits, and drops the writer.
pub struct TantivyLexicalAdapter {
    state_root: PathBuf,
}

impl TantivyLexicalAdapter {
    /// Build a new adapter rooted at `state_root`.
    ///
    /// The root directory does not need to exist yet; it will be created on
    /// the first build that resolves a target underneath it.
    pub fn with_state_root(root: impl Into<PathBuf>) -> Self {
        Self {
            state_root: root.into(),
        }
    }

    /// Resolve the per-generation index directory.
    fn index_dir(&self, generation_set: &PublishedGenerationSet) -> PathBuf {
        self.state_root
            .join("lexical")
            .join(generation_set.manifest_generation.get().to_string())
    }

    /// Resolve the per-generation index directory from a manifest.
    fn index_dir_for_manifest(&self, manifest: &PublishedSearchBundleManifest) -> PathBuf {
        self.state_root
            .join("lexical")
            .join(manifest.manifest_generation.get().to_string())
    }

    /// Open the Tantivy index for query-side consumption.
    ///
    /// Consumed by `searchd::query` only. Phase 1 query plumbing (T4.1) takes
    /// the returned `Index` handle and constructs its own reader; this crate
    /// does not expose query traits.
    pub fn open_index_for_query(
        &self,
        generation: &PublishedGenerationSet,
    ) -> Result<Index, CoreError> {
        let target = self.index_dir(generation);
        if !marker_ok_path(&target).is_file() {
            return Err(CoreError::NotReady(
                "lexical index not materialised".to_owned(),
            ));
        }
        Index::open_in_dir(&target)
            .map_err(|error| CoreError::Storage(format!("tantivy: open lexical index: {error}")))
    }
}

impl SearchPlaneLexicalIndexBuildPort for TantivyLexicalAdapter {
    fn build_lexical_index(
        &mut self,
        manifest: &PublishedSearchBundleManifest,
        input: LexicalBuildInput<'_>,
    ) -> Result<(), CoreError> {
        let target = self.index_dir_for_manifest(manifest);

        // Step 2 — idempotent short-circuit.
        if marker_ok_path(&target).is_file() {
            return Ok(());
        }

        // Step 4 — decode chunk rows up front. We do this BEFORE creating the
        // building directory so a malformed payload never leaves filesystem
        // residue. `symbol_rows` is part of the contract surface but does not
        // index into the lexical store in Phase 1; destructure to discard it
        // explicitly without an unused-underscore binding.
        let LexicalBuildInput {
            chunk_rows,
            symbol_rows: _,
        } = input;
        let rows = decode_chunk_rows(chunk_rows)?;

        // Step 3 — atomic-ish staging directory `{target}.building`. We
        // ensure the parent (`{state_root}/lexical/`) exists first; the
        // building dir itself is fresh per build (best-effort cleanup of a
        // prior interrupted attempt is NOT mandatory per D14).
        let building = building_path(&target);
        ensure_parent_exists(&building)?;
        std::fs::create_dir_all(&building).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: create building dir {}: {error}",
                building.display()
            ))
        })?;

        // Steps 5–7 — schema, writer, document insertion, commit.
        let (schema, fields) = schema::build_schema();
        let index = Index::create_in_dir(&building, schema)
            .map_err(|error| CoreError::Storage(format!("tantivy: create index: {error}")))?;
        let mut writer: IndexWriter = index
            .writer(WRITER_MEMORY_BUDGET)
            .map_err(|error| CoreError::Storage(format!("tantivy: open writer: {error}")))?;

        for (idx, row) in rows.iter().enumerate() {
            let chunk_idx = u64::try_from(idx).map_err(|error| {
                CoreError::InvalidContract(format!("chunk_rows index does not fit in u64: {error}"))
            })?;
            let candidate_id = format_candidate_id(manifest, chunk_idx);
            let document = doc!(
                fields.candidate_id => candidate_id,
                fields.repo_id => manifest.repo_id.as_str(),
                fields.revision_id => manifest.revision_id.as_str(),
                fields.manifest_generation => manifest.manifest_generation.get(),
                fields.repo_relative_path => row.repo_relative_path.as_str(),
                fields.start_line => row.start_line,
                fields.end_line => row.end_line,
                fields.text => row.text.as_str(),
            );
            let _opstamp = writer
                .add_document(document)
                .map_err(|error| CoreError::Storage(format!("tantivy: add_document: {error}")))?;
        }

        let _commit_opstamp = writer
            .commit()
            .map_err(|error| CoreError::Storage(format!("tantivy: commit: {error}")))?;
        drop(writer);
        drop(index);

        // Step 8 — atomic move into place. `rename` is rejected if `target`
        // already exists. We treat that as a typed Storage error rather than
        // attempting recovery, since concurrent builds for the same
        // generation are a contract violation.
        if target.exists() {
            return Err(CoreError::Storage(format!(
                "lexical: target directory already exists at {}",
                target.display()
            )));
        }
        std::fs::rename(&building, &target).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: rename {} -> {}: {error}",
                building.display(),
                target.display()
            ))
        })?;

        // Step 9 — sentinel file. The file handle is dropped immediately;
        // creation alone marks the index as ready.
        let marker = marker_ok_path(&target);
        let marker_file = std::fs::File::create(&marker).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: create marker {}: {error}",
                marker.display()
            ))
        })?;
        drop(marker_file);

        Ok(())
    }
}

impl SearchPlaneLexicalIndexStorePort for TantivyLexicalAdapter {
    fn open_lexical_store(&self, generation: &PublishedGenerationSet) -> Result<(), CoreError> {
        let target = self.index_dir(generation);
        if !marker_ok_path(&target).is_file() {
            return Err(CoreError::NotReady(
                "lexical index not materialised".to_owned(),
            ));
        }
        // Verify the index opens cleanly. We do not retain the handle here;
        // query-side consumers call `open_index_for_query` for an owned
        // `Index`.
        let _index = Index::open_in_dir(&target)
            .map_err(|error| CoreError::Storage(format!("tantivy: open lexical index: {error}")))?;
        Ok(())
    }
}

/// Decode the manifest's `lexical_chunk_rows` payload (JSON `Vec<ChunkRow>`).
fn decode_chunk_rows(bytes: &[u8]) -> Result<Vec<ChunkRow>, CoreError> {
    serde_json::from_slice::<Vec<ChunkRow>>(bytes)
        .map_err(|error| CoreError::InvalidContract(format!("chunk_rows decode: {error}")))
}

/// Build the `repo:rev:gen:idx` candidate identifier.
fn format_candidate_id(manifest: &PublishedSearchBundleManifest, chunk_idx: u64) -> String {
    format!(
        "{}:{}:{}:{}",
        manifest.repo_id.as_str(),
        manifest.revision_id.as_str(),
        manifest.manifest_generation.get(),
        chunk_idx,
    )
}

/// Append `.building` to the target directory.
fn building_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_owned();
    name.push(BUILDING_SUFFIX);
    PathBuf::from(name)
}

/// Resolve the `MARKER_OK` sentinel path for a target directory.
fn marker_ok_path(target: &Path) -> PathBuf {
    target.join(MARKER_OK)
}

/// Ensure the parent directory of `path` exists.
fn ensure_parent_exists(path: &Path) -> Result<(), CoreError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: create parent dir {}: {error}",
            parent.display()
        ))
    })
}
