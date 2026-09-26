//! L2: storage-free file mutation refusals precede both track builders and
//! the durable publication authority, including direct materializer calls.

use quanta_index_contract::{
    RepoRelativePath, SearchCorpusIngestBatch, SearchCorpusTombstoneScope, SearchScopeSurface,
};
use quanta_index_core::{CoreError, RequestBudgetV1, SearchCorpusIngestPort as _};
use quanta_index_ipc::stamp_batch_digest_v1;

use super::support::{
    TestRes, ZeroMutationProbe, always_valid_generation, fixture_search_corpus_batch,
};

fn assert_refused_by_both_entry_points(mut batch: SearchCorpusIngestBatch) -> TestRes {
    batch.source_event.payload_sha256 = quanta_index_contract::source_event_payload_sha256(&batch)?;
    stamp_batch_digest_v1(&mut batch)?;
    let probe = ZeroMutationProbe::new(always_valid_generation());
    if !matches!(
        probe.materializer.preflight_batch(&batch),
        Err(CoreError::InvalidContract(_))
    ) {
        return Err("file mutation preflight did not refuse with InvalidContract".into());
    }
    probe.assert_nothing_touched("file mutation preflight")?;
    if !matches!(
        probe
            .materializer
            .publish_batch(&batch, &RequestBudgetV1::unbounded()),
        Err(CoreError::InvalidContract(_))
    ) {
        return Err("direct file mutation publication did not refuse with InvalidContract".into());
    }
    probe.assert_nothing_touched("direct file mutation publication")
}

#[test]
fn surface_aliases_refuse_before_either_track_or_authority_is_mutated() -> TestRes {
    for reverse in [false, true] {
        let mut batch = fixture_search_corpus_batch()?;
        let scope = batch.replace_scopes.first().ok_or("fixture scope")?;
        let mut alias = scope.clone();
        alias.chunks.clear();
        alias.symbols.clear();
        alias.coverage.symbols =
            quanta_index_contract::SymbolCoverage::Complete { symbol_count: 0 };
        alias.coverage.unit_set_sha256 =
            quanta_index_contract::source_file_unit_set_sha256(&[], &[])?;
        batch.replace_scopes.push(alias);
        if reverse {
            batch.replace_scopes.reverse();
        }
        assert_refused_by_both_entry_points(batch)?;
    }
    Ok(())
}

#[test]
fn foreign_record_path_refuses_before_either_track_or_authority_is_mutated() -> TestRes {
    let mut batch = fixture_search_corpus_batch()?;
    let scope = batch.replace_scopes.first_mut().ok_or("fixture scope")?;
    scope
        .chunks
        .first_mut()
        .ok_or("fixture chunk")?
        .repo_relative_path = RepoRelativePath::new("src/not-the-owner.rs");
    assert_refused_by_both_entry_points(batch)
}

#[test]
fn replace_tombstone_alias_refuses_before_either_track_or_authority_is_mutated() -> TestRes {
    let mut batch = fixture_search_corpus_batch()?;
    let key = batch
        .replace_scopes
        .first()
        .ok_or("fixture scope")?
        .coverage
        .source
        .file
        .clone();
    batch
        .tombstone_scopes
        .push(SearchCorpusTombstoneScope { file: key });
    assert_refused_by_both_entry_points(batch)
}

#[test]
fn clear_replace_overlap_refuses_before_either_track_or_authority_is_mutated() -> TestRes {
    for surface in [SearchScopeSurface::Chunk, SearchScopeSurface::Symbol] {
        let mut batch = fixture_search_corpus_batch()?;
        batch.clear_surfaces.push(surface);
        assert_refused_by_both_entry_points(batch)?;
    }
    Ok(())
}
