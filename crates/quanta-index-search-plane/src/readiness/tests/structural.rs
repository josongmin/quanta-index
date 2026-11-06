use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ManifestGeneration, ReplaceStructuralScope, StructuralReplaceScope,
    StructuralTreeRecord, UpsertParseTree,
};

use crate::readiness::ledger::Ledger;
use crate::readiness::tests::support::{
    TestResult, encode_cbor, expect_decode_fail, generation, install_chunk, install_parse_tree,
    parse_tree_record, repo_id, revision_id, scope,
};

#[test]
fn structural_upsert_parse_tree_rejects_unknown_chunk() -> TestResult {
    let mut ledger = Ledger::default();
    let op = LexicalChannelOp::UpsertParseTree(UpsertParseTree {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation: generation(),
        chunk_id: ChunkId::new("chunk-1"),
        payload: encode_cbor(&parse_tree_record("fn main() {}")?)?,
    });
    let err = ledger
        .apply_lexical_authority_op(&op, std::time::Instant::now())
        .err()
        .ok_or_else(|| "expected unknown-chunk parse tree apply to fail".to_string())?;
    expect_decode_fail(err, "reason=source_chunk_missing")
}

#[test]
fn structural_upsert_parse_tree_rejects_source_hash_mismatch() -> TestResult {
    let mut ledger = Ledger::default();
    install_chunk(&mut ledger, "src/lib.rs", "fn main() {}")?;
    let op = LexicalChannelOp::UpsertParseTree(UpsertParseTree {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation: generation(),
        chunk_id: ChunkId::new("chunk-1"),
        payload: encode_cbor(&parse_tree_record("fn other() {}")?)?,
    });
    let err = ledger
        .apply_lexical_authority_op(&op, std::time::Instant::now())
        .err()
        .ok_or_else(|| "expected source_hash mismatch to fail".to_string())?;
    expect_decode_fail(err, "reason=source_hash_mismatch")
}

#[test]
fn structural_replace_scope_rejects_chunk_outside_scope_path() -> TestResult {
    let mut ledger = Ledger::default();
    install_chunk(&mut ledger, "src/lib.rs", "fn main() {}")?;
    let op = LexicalChannelOp::ReplaceStructuralScope(ReplaceStructuralScope {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation: generation(),
        payload: encode_cbor(&(
            BatchIngestMode::ReplaceGeneration,
            None::<ManifestGeneration>,
            StructuralReplaceScope {
                scope: scope("src/other.rs"),
                scope_digest: "scope:str".to_string(),
                trees: vec![StructuralTreeRecord {
                    chunk_id: ChunkId::new("chunk-1"),
                    record: parse_tree_record("fn main() {}")?,
                }],
            },
        ))?,
    });
    let err = ledger
        .apply_lexical_authority_op(&op, std::time::Instant::now())
        .err()
        .ok_or_else(|| "expected structural scope mismatch to fail".to_string())?;
    expect_decode_fail(err, "reason=scope_chunk_path_mismatch")
}

#[test]
fn structural_replace_scope_failure_preserves_existing_parse_tree_set() -> TestResult {
    let mut ledger = Ledger::default();
    install_chunk(&mut ledger, "src/lib.rs", "fn main() {}")?;
    install_parse_tree(&mut ledger, "fn main() {}")?;
    let prior = ledger
        .structural_state(&repo_id(), &revision_id(), generation())
        .ok_or_else(|| "expected structural state after initial tree install".to_string())?
        .parse_trees()
        .get(&ChunkId::new("chunk-1"))
        .cloned()
        .ok_or_else(|| "expected installed parse tree".to_string())?;

    let op = LexicalChannelOp::ReplaceStructuralScope(ReplaceStructuralScope {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation: generation(),
        payload: encode_cbor(&(
            BatchIngestMode::ReplaceGeneration,
            None::<ManifestGeneration>,
            StructuralReplaceScope {
                scope: scope("src/lib.rs"),
                scope_digest: "scope:str".to_string(),
                trees: vec![StructuralTreeRecord {
                    chunk_id: ChunkId::new("chunk-1"),
                    record: parse_tree_record("fn other() {}")?,
                }],
            },
        ))?,
    });
    let err = ledger
        .apply_lexical_authority_op(&op, std::time::Instant::now())
        .err()
        .ok_or_else(|| "expected structural replace with bad tree to fail".to_string())?;
    expect_decode_fail(err, "reason=source_hash_mismatch")?;

    let after = ledger
        .structural_state(&repo_id(), &revision_id(), generation())
        .ok_or_else(|| "expected structural state after failed replace".to_string())?
        .parse_trees()
        .get(&ChunkId::new("chunk-1"))
        .cloned()
        .ok_or_else(|| "expected prior parse tree to remain after failed replace".to_string())?;
    if after != prior {
        return Err(format!(
            "expected prior parse tree to survive failed replace, got after={after:?} prior={prior:?}"
        )
        .into());
    }
    Ok(())
}

#[test]
fn structural_scope_preserves_same_path_in_another_source_repository() -> TestResult {
    use crate::readiness::StructuralAuthorityState;
    use crate::readiness::tests::support::chunk_record;
    use quanta_index_contract::{RepoId, RepoRelativePath, SourceFileKey};

    let target = repo_id();
    let file_a = SourceFileKey {
        source_repo_id: RepoId::new("source-a")?,
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
    };
    let file_b = SourceFileKey {
        source_repo_id: RepoId::new("source-b")?,
        repo_relative_path: file_a.repo_relative_path.clone(),
    };
    let text = "fn main() {}";
    let mut a = chunk_record("src/lib.rs", text)?;
    a.chunk_id = ChunkId::new("source-a-chunk");
    let mut b = a.clone();
    b.chunk_id = ChunkId::new("source-b-chunk");
    let mut state = StructuralAuthorityState::default();
    state.replace_file_chunks(&file_a, &target, vec![a.clone()]);
    state.replace_file_chunks(&file_b, &target, vec![b.clone()]);
    state.upsert_parse_tree(a.chunk_id.clone(), parse_tree_record(text)?)?;
    state.upsert_parse_tree(b.chunk_id.clone(), parse_tree_record(text)?)?;
    let before = state.parse_trees().clone();
    let error = state
        .replace_scope_parse_trees(
            &file_a,
            &target,
            vec![(b.chunk_id.clone(), parse_tree_record(text)?)],
        )
        .expect_err("foreign source tree must refuse before pruning");
    expect_decode_fail(error, "reason=scope_chunk_owner_mismatch")?;
    assert_eq!(state.parse_trees(), &before);
    state.replace_scope_parse_trees(
        &file_a,
        &target,
        vec![(a.chunk_id.clone(), parse_tree_record(text)?)],
    )?;
    assert!(state.parse_trees().contains_key(&b.chunk_id));
    state.tombstone_scope_parse_trees(&file_a, &target);
    assert!(!state.parse_trees().contains_key(&a.chunk_id));
    assert!(state.parse_trees().contains_key(&b.chunk_id));
    Ok(())
}
