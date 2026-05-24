//! Lexical adapter — Tantivy 0.22-backed inverted index.
//!
//! Implements [`LexicalIndexBuildPort`] and [`LexicalIndexOpenPort`] from
//! `quanta-index-core::domains::lexical`. The adapter materializes channel
//! events into a `(repo, revision, generation) -> Tantivy index` directory
//! tree rooted at the adapter's `state_root` and services queries via
//! BM25 over the indexed `chunk_text` field.
//!
//! Layout:
//!   `{state_root}/{repo_id}/{revision_id}/g{generation}/`
//!
//! The adapter caches one `IndexWriter` per generation to amortize the
//! per-commit cost across many ops, and one `IndexReader` per opened
//! generation.

#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]
#![expect(
    clippy::multiple_crate_versions,
    reason = "tantivy 0.22 pulls multiple transitive versions (rustix, linux-raw-sys, windows-sys) we cannot collapse; scoped allowance in deny.toml [bans] skip-tree."
)]


use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use quanta_index_contract::{
    LexicalCandidate, LexicalChannelOp, LqExpr, LqQuery, ManifestGeneration, RepoId,
    RepoRelativePath, RevisionId,
};
use quanta_index_core::{
    CoreError, LexicalIndexBuildPort, LexicalIndexOpenPort, LexicalSearcher,
    domains::lexical::LexicalPolicy,
};
use tantivy::collector::TopDocs;
use tantivy::query::{AllQuery, BooleanQuery, Occur, Query, QueryParser};
use tantivy::schema::{Field, OwnedValue, STORED, STRING, Schema, TEXT, TantivyDocument, Value};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, Term};

/// Memory budget for a Tantivy `IndexWriter`. Pinned to the upstream-documented
/// minimum so adapter setup is bounded and reproducible across test runs.
const WRITER_MEMORY_BUDGET_BYTES: usize = 15_000_000;

/// Schema field handles for the lexical index. Cloned cheaply into every
/// searcher; constructed once per adapter instance.
#[derive(Clone)]
struct SchemaFields {
    schema: Schema,
    candidate_id: Field,
    repo_id: Field,
    revision_id: Field,
    repo_relative_path: Field,
    chunk_text: Field,
}

impl SchemaFields {
    fn build() -> Self {
        let mut builder = Schema::builder();
        let candidate_id = builder.add_text_field("candidate_id", STRING | STORED);
        let repo_id = builder.add_text_field("repo_id", STRING | STORED);
        let revision_id = builder.add_text_field("revision_id", STRING | STORED);
        let repo_relative_path = builder.add_text_field("repo_relative_path", STRING | STORED);
        let chunk_text = builder.add_text_field("chunk_text", TEXT | STORED);
        let schema = builder.build();
        Self {
            schema,
            candidate_id,
            repo_id,
            revision_id,
            repo_relative_path,
            chunk_text,
        }
    }
}

/// Per-generation cache key.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct GenKey {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
}

/// Open writer + index handle for an active generation. Shared via
/// `Arc<Mutex<_>>` so multiple `build` invocations for the same generation
/// serialize on a single Tantivy writer (Tantivy writers are not Sync).
struct GenerationWriter {
    index: Index,
    writer: IndexWriter,
}

/// Tantivy-backed lexical adapter.
pub struct LexicalAdapter {
    state_root: PathBuf,
    fields: SchemaFields,
    writers: Arc<RwLock<BTreeMap<GenKey, Arc<Mutex<GenerationWriter>>>>>,
}

impl LexicalAdapter {
    /// Construct an adapter rooted at the given directory. The directory will
    /// be created lazily as generations are materialized.
    #[must_use]
    pub fn with_state_root(state_root: PathBuf) -> Self {
        Self {
            state_root,
            fields: SchemaFields::build(),
            writers: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    fn index_path(&self, key: &GenKey) -> PathBuf {
        self.state_root
            .join(key.repo_id.as_str())
            .join(key.revision_id.as_str())
            .join(format!("g{}", key.generation.get()))
    }

    fn open_or_create_index(&self, path: &Path) -> Result<Index, CoreError> {
        std::fs::create_dir_all(path)
            .map_err(|err| CoreError::Storage(format!("lexical: mkdir {path:?}: {err}")))?;
        let directory = tantivy::directory::MmapDirectory::open(path)
            .map_err(|err| CoreError::Storage(format!("lexical: mmap open {path:?}: {err}")))?;
        Index::builder()
            .schema(self.fields.schema.clone())
            .open_or_create(directory)
            .map_err(|err| CoreError::Storage(format!("lexical: open_or_create: {err}")))
    }

    fn writer_handle(&self, key: &GenKey) -> Result<Arc<Mutex<GenerationWriter>>, CoreError> {
        {
            let guard = self
                .writers
                .read()
                .map_err(|err| CoreError::Storage(format!("lexical writers poisoned: {err}")))?;
            if let Some(existing) = guard.get(key) {
                return Ok(Arc::clone(existing));
            }
        }
        let mut guard = self
            .writers
            .write()
            .map_err(|err| CoreError::Storage(format!("lexical writers poisoned: {err}")))?;
        if let Some(existing) = guard.get(key) {
            return Ok(Arc::clone(existing));
        }
        let path = self.index_path(key);
        let index = self.open_or_create_index(&path)?;
        let writer: IndexWriter = index
            .writer(WRITER_MEMORY_BUDGET_BYTES)
            .map_err(|err| CoreError::Storage(format!("lexical: writer: {err}")))?;
        let handle = Arc::new(Mutex::new(GenerationWriter { index, writer }));
        drop(guard.insert(key.clone(), Arc::clone(&handle)));
        Ok(handle)
    }

    fn apply_op(
        &self,
        writer: &mut IndexWriter,
        key: &GenKey,
        op: &LexicalChannelOp,
    ) -> Result<bool, CoreError> {
        match op {
            LexicalChannelOp::UpsertChunk(upsert) => {
                let candidate_id = upsert.chunk_id.as_str();
                let term = Term::from_field_text(self.fields.candidate_id, candidate_id);
                let _opstamp = writer.delete_term(term);
                let text = String::from_utf8_lossy(&upsert.payload).into_owned();
                let mut doc = TantivyDocument::new();
                doc.add_text(self.fields.candidate_id, candidate_id);
                doc.add_text(self.fields.repo_id, key.repo_id.as_str());
                doc.add_text(self.fields.revision_id, key.revision_id.as_str());
                doc.add_text(self.fields.repo_relative_path, "");
                doc.add_text(self.fields.chunk_text, text);
                let _opstamp = writer
                    .add_document(doc)
                    .map_err(|err| CoreError::Storage(format!("lexical: add_document: {err}")))?;
                Ok(true)
            }
            LexicalChannelOp::DeleteChunk(delete) => {
                let term =
                    Term::from_field_text(self.fields.candidate_id, delete.chunk_id.as_str());
                let _opstamp = writer.delete_term(term);
                Ok(true)
            }
            // FullBundle / Seal carry no document-level effect on the adapter;
            // the dispatcher's ledger update observes Seal, not us.
            LexicalChannelOp::FullBundle(_) | LexicalChannelOp::Seal(_) => Ok(false),
            // Symbols are out-of-scope for the lexical chunk index.
            LexicalChannelOp::UpsertSymbol(_) | LexicalChannelOp::DeleteSymbol(_) => Ok(false),
        }
    }
}

impl LexicalIndexBuildPort for LexicalAdapter {
    fn build(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
        ops: &[LexicalChannelOp],
    ) -> Result<(), CoreError> {
        if ops.is_empty() {
            return Ok(());
        }
        // All ops in a single `build` invocation must share the (repo, rev, gen)
        // triple. The dispatcher feeds us one op per call today; defending the
        // invariant here keeps the adapter safe if that changes.
        for op in ops {
            if op.repo_id() != repo || op.revision_id() != revision || op.generation() != generation
            {
                return Err(CoreError::InvalidContract(
                    "lexical: op (repo, revision, generation) mismatch with batch key"
                        .to_string(),
                ));
            }
        }
        let key = GenKey {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            generation,
        };
        let handle = self.writer_handle(&key)?;
        let mut guarded = handle
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical writer poisoned: {err}")))?;
        let mut needs_commit = false;
        for op in ops {
            if self.apply_op(&mut guarded.writer, &key, op)? {
                needs_commit = true;
            }
        }
        if needs_commit {
            let _opstamp = guarded
                .writer
                .commit()
                .map_err(|err| CoreError::Storage(format!("lexical: commit: {err}")))?;
        }
        Ok(())
    }
}

impl LexicalIndexOpenPort for LexicalAdapter {
    fn open(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
        let key = GenKey {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            generation,
        };
        let path = self.index_path(&key);
        if !path.exists() {
            return Err(CoreError::NotFound(format!(
                "lexical: no index at {path:?}"
            )));
        }
        // Prefer the cached writer's index handle when present (it reflects
        // commits that may not yet be visible to a freshly-opened reader
        // before its first reload).
        let index = {
            let guard = self
                .writers
                .read()
                .map_err(|err| CoreError::Storage(format!("lexical writers poisoned: {err}")))?;
            match guard.get(&key) {
                Some(handle) => {
                    let guarded = handle.lock().map_err(|err| {
                        CoreError::Storage(format!("lexical writer poisoned: {err}"))
                    })?;
                    guarded.index.clone()
                }
                None => self.open_or_create_index(&path)?,
            }
        };
        let reader: IndexReader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|err| CoreError::Storage(format!("lexical: reader: {err}")))?;
        reader
            .reload()
            .map_err(|err| CoreError::Storage(format!("lexical: reader reload: {err}")))?;
        Ok(Box::new(TantivySearcher {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            generation,
            fields: self.fields.clone(),
            index,
            reader,
        }))
    }
}

struct TantivySearcher {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    fields: SchemaFields,
    index: Index,
    reader: IndexReader,
}

impl TantivySearcher {
    fn compile(&self, expr: &LqExpr) -> Result<Box<dyn Query>, CoreError> {
        match expr {
            LqExpr::MatchAll => Err(CoreError::InvalidContract(
                "lexical: MatchAll not compilable (must be rejected by policy)".to_string(),
            )),
            LqExpr::Raw(text) => {
                let parser = QueryParser::for_index(&self.index, vec![self.fields.chunk_text]);
                let parsed = parser
                    .parse_query(text)
                    .map_err(|err| CoreError::InvalidContract(format!("lexical: parse: {err}")))?;
                Ok(parsed)
            }
            LqExpr::All(parts) => {
                let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(parts.len());
                for part in parts {
                    clauses.push((Occur::Must, self.compile(part)?));
                }
                Ok(Box::new(BooleanQuery::new(clauses)))
            }
            LqExpr::Any(parts) => {
                let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(parts.len());
                for part in parts {
                    clauses.push((Occur::Should, self.compile(part)?));
                }
                Ok(Box::new(BooleanQuery::new(clauses)))
            }
            LqExpr::Not(inner) => {
                let inner_q = self.compile(inner)?;
                let clauses: Vec<(Occur, Box<dyn Query>)> = vec![
                    (Occur::Must, Box::new(AllQuery)),
                    (Occur::MustNot, inner_q),
                ];
                Ok(Box::new(BooleanQuery::new(clauses)))
            }
        }
    }

    fn document_to_candidate(
        &self,
        doc: &TantivyDocument,
        score: f32,
    ) -> Result<LexicalCandidate, CoreError> {
        let candidate_id = stored_text(doc, self.fields.candidate_id).ok_or_else(|| {
            CoreError::Storage("lexical: stored doc missing candidate_id field".to_string())
        })?;
        let snippet = stored_text(doc, self.fields.chunk_text).unwrap_or_default();
        let repo_relative_path =
            stored_text(doc, self.fields.repo_relative_path).unwrap_or_default();
        Ok(LexicalCandidate {
            candidate_id,
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            manifest_generation: self.generation,
            repo_relative_path: RepoRelativePath::new(repo_relative_path),
            start_line: 0,
            end_line: 0,
            score,
            snippet,
        })
    }
}

fn stored_text(doc: &TantivyDocument, field: Field) -> Option<String> {
    let value: &OwnedValue = doc.get_first(field)?;
    Value::as_str(&value).map(str::to_owned)
}

impl LexicalSearcher for TantivySearcher {
    fn search(&self, query: &LqQuery, top_k: u32) -> Result<Vec<LexicalCandidate>, CoreError> {
        LexicalPolicy::validate_query(query)?;
        let compiled = self.compile(&query.expr)?;
        let limit = usize::try_from(top_k)
            .map_err(|err| CoreError::InvalidContract(format!("lexical: top_k: {err}")))?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        let searcher = self.reader.searcher();
        let hits = searcher
            .search(&*compiled, &TopDocs::with_limit(limit))
            .map_err(|err| CoreError::Storage(format!("lexical: search: {err}")))?;
        let mut out: Vec<LexicalCandidate> = Vec::with_capacity(hits.len());
        for (score, doc_address) in hits {
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            out.push(self.document_to_candidate(&doc, score)?);
        }
        Ok(out)
    }
}
