//! Fixed Source producer for pinned Legacy process fixtures.
//! The public SDK owns wire digests, durable publication and activation.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;

use quanta_index_contract::{
    BatchPublishReceipt, ChunkId, ChunkRecord, ManifestGeneration, RepoId, RepoRelativePath,
    RevisionId, SearchCorpusActiveHeadV1, SourceFileKey, SourcePublicationEvent,
    lex::{LanguageCode, SymbolRecord},
};
use quanta_index_sdk::{QuantaIndex, SearchCorpusBatch};

use crate::{fixture_source_scope_v1, semantic_source_scopes_for_chunk_records};

fn legacy_source_chunk(path: &str, body: &str, id: u64) -> Result<ChunkRecord, Box<dyn Error>> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(format!("e2e-{id}-{path}")),
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::new("rust")?,
        start_byte: 0,
        end_byte: u32::try_from(body.len())?,
        start_line: 1,
        end_line: 2,
        text: body.to_owned().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    })
}

/// Preserve the original E2eRuntime fixture's chunk IDs, semantic scopes,
/// event stream and manifest names while using the public Legacy SDK.
pub fn legacy_source_batch(
    repo: &RepoId,
    revision: &RevisionId,
    generation: u64,
    previous: Option<(&str, &str, u64)>,
    files: &[(&str, &str, u64)],
) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let base_generation = previous
        .map(|_| generation.checked_sub(1).ok_or("missing base generation"))
        .transpose()?;
    // Replacing the same fixture owner already replaces its semantic scope.
    // A new chunk ID at the same path still retires the previous owner.
    let retired_chunk = previous
        .filter(|(old_path, _, old_id)| {
            !files
                .iter()
                .any(|(path, _, id)| path == old_path && id == old_id)
        })
        .map(|(path, body, id)| legacy_source_chunk(path, body, id))
        .transpose()?;
    let chunks = files
        .iter()
        .map(|&(path, body, id)| legacy_source_chunk(path, body, id))
        .collect::<Result<Vec<_>, _>>()?;
    legacy_source_batch_for_chunks(
        repo,
        revision,
        generation,
        base_generation,
        retired_chunk,
        chunks,
        Vec::new(),
    )
}

/// A fixed replacement fixture with explicit chunk metadata, including
/// searchable source-repository identity and per-file languages.
pub fn legacy_source_replacement_from_chunks(
    repo: &RepoId,
    revision: &RevisionId,
    generation: u64,
    chunks: Vec<ChunkRecord>,
) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    legacy_source_batch_for_chunks(repo, revision, generation, None, None, chunks, Vec::new())
}

/// One generation with explicit source chunks and symbols. Symbols belong to
/// the serving repository; the canonical file key joins them to its chunks.
pub fn legacy_source_replacement_from_chunks_and_symbols(
    repo: &RepoId,
    revision: &RevisionId,
    generation: u64,
    chunks: Vec<ChunkRecord>,
    symbols: Vec<SymbolRecord>,
) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    legacy_source_batch_for_chunks(repo, revision, generation, None, None, chunks, symbols)
}

/// One source-authored delta: retired semantic owners, removed file scopes,
/// and replacement file scopes are all explicit. The caller supplies only the
/// changed files; the previous generation supplies every unchanged file.
pub fn legacy_source_delta_from_chunks(
    repo: &RepoId,
    revision: &RevisionId,
    generation: u64,
    retired_chunks: &[ChunkRecord],
    deleted_files: &[SourceFileKey],
    replacement_chunks: Vec<ChunkRecord>,
) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let base = generation.checked_sub(1).ok_or("missing base generation")?;
    let replacement_files = replacement_chunks
        .iter()
        .map(|chunk| SourceFileKey {
            source_repo_id: chunk.searchable_repo_id(repo).clone(),
            repo_relative_path: chunk.repo_relative_path.clone(),
        })
        .collect::<BTreeSet<_>>();
    let deleted = deleted_files.iter().cloned().collect::<BTreeSet<_>>();
    if deleted.len() != deleted_files.len() || !deleted.is_disjoint(&replacement_files) {
        return Err("Source delta repeats or replaces a deleted file".into());
    }
    for file in &deleted {
        file.validate()?;
    }
    for retired in retired_chunks {
        let file = SourceFileKey {
            source_repo_id: retired.searchable_repo_id(repo).clone(),
            repo_relative_path: retired.repo_relative_path.clone(),
        };
        file.validate()?;
        if !replacement_files.contains(&file) && !deleted.contains(&file) {
            return Err("Source delta retires an unchanged file owner".into());
        }
    }
    let mut batch = SearchCorpusBatch::delta(
        repo.clone(),
        revision.clone(),
        ManifestGeneration::new(generation),
        ManifestGeneration::new(base),
        format!("lex-seal:{generation}"),
    )
    .source_event(source_event(generation, Some(base)));
    for scope in semantic_source_scopes_for_chunk_records(repo, retired_chunks) {
        batch = batch.tombstone_semantic_scope(scope.scope);
    }
    for file in deleted {
        batch = batch.tombstone_scope(file);
    }
    append_source_scopes(batch, repo, revision, replacement_chunks, Vec::new())
}

pub(crate) fn source_event(
    sequence: u64,
    previous_sequence: Option<u64>,
) -> SourcePublicationEvent {
    SourcePublicationEvent {
        stream_id: "e2e-harness-source-v1".into(),
        event_id: format!("e2e-source-event-{sequence}"),
        expected_base_event_id: previous_sequence
            .map(|previous| format!("e2e-source-event-{previous}")),
        payload_sha256: [0; 32],
    }
}

fn legacy_source_batch_for_chunks(
    repo: &RepoId,
    revision: &RevisionId,
    generation: u64,
    base_generation: Option<u64>,
    retired_chunk: Option<ChunkRecord>,
    chunks: Vec<ChunkRecord>,
    symbols: Vec<SymbolRecord>,
) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let mut batch = if let Some(base) = base_generation {
        SearchCorpusBatch::delta(
            repo.clone(),
            revision.clone(),
            ManifestGeneration::new(generation),
            ManifestGeneration::new(base),
            format!("lex-seal:{generation}"),
        )
    } else {
        SearchCorpusBatch::replace_generation(
            repo.clone(),
            revision.clone(),
            ManifestGeneration::new(generation),
            format!("lex-seal:{generation}"),
        )
    }
    .source_event(source_event(generation, base_generation));
    if let Some(old) = retired_chunk {
        for scope in semantic_source_scopes_for_chunk_records(repo, &[old]) {
            batch = batch.tombstone_semantic_scope(scope.scope);
        }
    }
    append_source_scopes(batch, repo, revision, chunks, symbols)
}

fn append_source_scopes(
    mut batch: SearchCorpusBatch,
    repo: &RepoId,
    revision: &RevisionId,
    chunks: Vec<ChunkRecord>,
    symbols: Vec<SymbolRecord>,
) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    // The source file key, rather than its relative path alone, owns a scope.
    // Preserve input order within a file while emitting scopes in key order.
    let mut files = BTreeMap::<SourceFileKey, (Vec<ChunkRecord>, Vec<SymbolRecord>)>::new();
    for chunk in chunks {
        let file = SourceFileKey {
            source_repo_id: chunk.searchable_repo_id(repo).clone(),
            repo_relative_path: chunk.repo_relative_path.clone(),
        };
        file.validate()?;
        files.entry(file).or_default().0.push(chunk);
    }
    for symbol in symbols {
        let file = SourceFileKey {
            source_repo_id: repo.clone(),
            repo_relative_path: symbol.repo_relative_path.clone(),
        };
        file.validate()?;
        files.entry(file).or_default().1.push(symbol);
    }
    for (file, (chunks, symbols)) in files {
        let scope = fixture_source_scope_v1(file, revision.clone(), chunks, symbols)?;
        for semantic in semantic_source_scopes_for_chunk_records(repo, &scope.chunks) {
            batch = batch.replace_semantic_scope(
                semantic.scope,
                semantic.scope_digest,
                semantic.sources,
                semantic.cluster_memberships,
            );
        }
        batch = batch.replace_scope(
            scope.coverage,
            scope.source_bytes,
            scope.chunks,
            scope.symbols,
        );
    }
    Ok(batch)
}

/// Publish and observe one exact Legacy Source head through its real process.
pub fn publish_legacy_source(
    client: &QuantaIndex,
    repo: &RepoId,
    revision: &RevisionId,
    generation: u64,
    previous: Option<(&str, &str, u64)>,
    files: &[(&str, &str, u64)],
    expected_active: Option<SearchCorpusActiveHeadV1>,
) -> Result<(SearchCorpusActiveHeadV1, BatchPublishReceipt), Box<dyn Error>> {
    let batch = legacy_source_batch(repo, revision, generation, previous, files)?;
    publish_legacy_source_batch(client, &batch, expected_active)
}

/// Publish one already constructed Source batch through the same Legacy SDK
/// path, then read back its activated head. The caller may reserve the batch's
/// source event first when it needs to publish generation-scoped overlays.
pub fn publish_legacy_source_batch(
    client: &QuantaIndex,
    batch: &SearchCorpusBatch,
    expected_active: Option<SearchCorpusActiveHeadV1>,
) -> Result<(SearchCorpusActiveHeadV1, BatchPublishReceipt), Box<dyn Error>> {
    if client
        .generations()
        .active_head(batch.repo_id().clone(), batch.revision_id().clone())?
        != expected_active
    {
        return Err("old-Q active head before Source publication differs".into());
    }
    let (receipt, activation) = client
        .search_corpus()
        .publish_and_activate(batch, expected_active)?;
    if !receipt.applied || receipt.durable_sequence == 0 {
        return Err("old-Q Source publication was not durable".into());
    }
    let observed = client
        .generations()
        .active_head(batch.repo_id().clone(), batch.revision_id().clone())?
        .ok_or("old-Q Source activation has no observed active head")?;
    if observed != activation.active {
        return Err("old-Q observed Source head differs from activation".into());
    }
    if observed.generation.lexical.manifest_generation != batch.generation() {
        return Err("old-Q active Source generation differs".into());
    }
    if observed.generation.lexical.manifest_digest != batch.manifest_digest() {
        return Err("old-Q active Source manifest differs".into());
    }
    Ok((observed, receipt))
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use quanta_index_contract::{
        ChunkId, ChunkRecord, RepoId, RepoRelativePath, RevisionId, lex::LanguageCode,
    };

    use super::{
        legacy_source_batch, legacy_source_delta_from_chunks, legacy_source_replacement_from_chunks,
    };

    fn scoped_chunk(
        source_repo_id: Option<RepoId>,
        id: &str,
        path: &str,
        text: &str,
    ) -> Result<ChunkRecord, Box<dyn Error>> {
        Ok(ChunkRecord {
            chunk_id: ChunkId::new(id),
            repo_relative_path: RepoRelativePath::new(path),
            language: LanguageCode::new("rust")?,
            start_byte: 0,
            end_byte: u32::try_from(text.len())?,
            start_line: 1,
            end_line: 2,
            text: text.to_owned().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id,
        })
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "fixed fixture scope identities are checked directly"
    )]
    fn replacement_keeps_same_relative_path_in_distinct_source_repositories()
    -> Result<(), Box<dyn Error>> {
        let serving_repo = RepoId::new("serving-repo")?;
        let source_a = RepoId::new("source-a")?;
        let source_b = RepoId::new("source-b")?;
        let revision = RevisionId::new("revision")?;
        let path = "src/file.rs";
        let batch = legacy_source_replacement_from_chunks(
            &serving_repo,
            &revision,
            1,
            vec![
                scoped_chunk(Some(source_b.clone()), "chunk-b", path, "from b")?,
                scoped_chunk(Some(source_a.clone()), "chunk-a1", path, "from a1")?,
                scoped_chunk(Some(source_a.clone()), "chunk-a2", path, "from a2")?,
                scoped_chunk(None, "chunk-default", path, "from serving")?,
            ],
        )?;
        let scopes = batch.replace_scopes();
        assert_eq!(scopes.len(), 3);
        assert_eq!(scopes[0].coverage.source.file.source_repo_id, serving_repo);
        assert_eq!(scopes[1].coverage.source.file.source_repo_id, source_a);
        assert_eq!(scopes[2].coverage.source.file.source_repo_id, source_b);
        assert!(
            scopes
                .iter()
                .all(|scope| scope.coverage.source.file.repo_relative_path.as_str() == path)
        );
        assert_eq!(scopes[0].source_bytes.as_slice(), b"from serving\n");
        assert_eq!(scopes[1].source_bytes.as_slice(), b"from a1\nfrom a2\n");
        assert_eq!(scopes[2].source_bytes.as_slice(), b"from b\n");
        let semantic = batch.semantic_replace_scopes();
        assert_eq!(semantic.len(), 4);
        for scope in semantic {
            let source_file = &scope.scope.source_file;
            let owner = scope.scope.owner_id.as_str();
            let expected_repo = match owner {
                "chunk-a1" | "chunk-a2" => &source_a,
                "chunk-b" => &source_b,
                "chunk-default" => &serving_repo,
                _ => return Err("unexpected semantic source owner".into()),
            };
            assert_eq!(&source_file.source_repo_id, expected_repo);
            assert_eq!(source_file.repo_relative_path.as_str(), path);
            assert_eq!(scope.sources[0].source_file, *source_file);
        }
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "the invalid assigned source must be rejected before publication"
    )]
    fn replacement_rejects_invalid_assigned_source_path() -> Result<(), Box<dyn Error>> {
        let serving_repo = RepoId::new("serving-repo")?;
        let revision = RevisionId::new("revision")?;
        let result = legacy_source_replacement_from_chunks(
            &serving_repo,
            &revision,
            1,
            vec![scoped_chunk(
                Some(RepoId::new("source-a")?),
                "chunk-a",
                "../escaped.rs",
                "content",
            )?],
        );
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "fixed delta scope and tombstone identities are checked directly"
    )]
    fn delta_retires_old_semantic_owner_and_deleted_source_file() -> Result<(), Box<dyn Error>> {
        let repo = RepoId::new("serving-repo")?;
        let revision = RevisionId::new("revision")?;
        let source = RepoId::new("source-a")?;
        let path = "src/file.rs";
        let old = scoped_chunk(Some(source.clone()), "old", path, "old token")?;
        let file = quanta_index_contract::SourceFileKey {
            source_repo_id: source,
            repo_relative_path: RepoRelativePath::new(path),
        };
        let update = legacy_source_delta_from_chunks(
            &repo,
            &revision,
            2,
            std::slice::from_ref(&old),
            &[],
            vec![scoped_chunk(
                old.source_repo_id.clone(),
                "new",
                path,
                "new token",
            )?],
        )?;
        assert_eq!(
            update.base_generation().map(|generation| generation.get()),
            Some(1)
        );
        assert_eq!(update.replace_scopes().len(), 1);
        assert_eq!(update.replace_scopes()[0].coverage.source.file, file);
        assert!(update.tombstone_scopes().is_empty());
        assert_eq!(update.semantic_tombstone_scopes().len(), 1);
        assert_eq!(update.semantic_tombstone_scopes()[0].owner_id, "old");
        let removed = scoped_chunk(old.source_repo_id.clone(), "new", path, "new token")?;
        let delete = legacy_source_delta_from_chunks(
            &repo,
            &revision,
            3,
            &[removed],
            std::slice::from_ref(&file),
            Vec::new(),
        )?;
        assert!(delete.replace_scopes().is_empty());
        assert_eq!(delete.tombstone_scopes()[0].file, file);
        assert_eq!(delete.semantic_tombstone_scopes()[0].owner_id, "new");
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "fixture construction errors propagate while expected scope identities are fixed"
    )]
    fn delta_tombstones_the_previous_chunk_only_when_its_scope_changes()
    -> Result<(), Box<dyn Error>> {
        let repo = RepoId::new("fixture-repo")?;
        let revision = RevisionId::new("fixture-revision")?;
        let path = "src/chunk.rs";
        let previous = Some((path, "fn original() {}", 1));

        let same = legacy_source_batch(
            &repo,
            &revision,
            2,
            previous,
            &[(path, "fn updated() {}", 1)],
        )?;
        assert!(same.semantic_tombstone_scopes().is_empty());
        let [replacement] = same.semantic_replace_scopes() else {
            return Err("expected exactly one replacement for the existing chunk".into());
        };
        assert_eq!(replacement.scope.owner_id, "e2e-1-src/chunk.rs");

        let different = legacy_source_batch(
            &repo,
            &revision,
            2,
            previous,
            &[(path, "fn updated() {}", 2)],
        )?;
        let [removed] = different.semantic_tombstone_scopes() else {
            return Err("expected exactly one tombstone for the previous chunk".into());
        };
        let [replacement] = different.semantic_replace_scopes() else {
            return Err("expected exactly one replacement for the new chunk".into());
        };
        assert_eq!(removed.owner_id, "e2e-1-src/chunk.rs");
        assert_eq!(replacement.scope.owner_id, "e2e-2-src/chunk.rs");
        assert_eq!(removed.source_file, replacement.scope.source_file);
        Ok(())
    }
}
