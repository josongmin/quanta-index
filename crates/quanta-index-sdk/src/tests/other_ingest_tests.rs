use super::*;

#[test]
fn history_publish_routes_through_ingest_transport_and_carries_typed_authority_records() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(3),
        manifest_digest: None,
        batch_digest: String::new(),
        applied: true,
        durable_sequence: 3,
        semantic_content: None,
        accepted_clear_surfaces: 0,
        accepted_replace_scopes: 4,
        accepted_tombstone_scopes: 0,
        accepted_semantic_replace_scopes: 0,
        accepted_semantic_tombstone_scopes: 0,
        sealed: false,
    };
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::HistoryReceipt(receipt.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch = HistoryBatch::new(repo_id(), revision_id(), ManifestGeneration::new(3))
        .manifest_digest("manifest:history-3")
        .commit(sample_commit_record())
        .ref_upsert("refs/heads/main", sample_commit_sha())
        .tag_upsert("v1.0.0", sample_commit_sha())
        .diff_hunk(sample_commit_sha(), "src/lib.rs", sample_diff_record());
    let observed = ok_or_fail!(client.history().publish(&batch));
    let expected = BatchPublishReceipt {
        batch_digest: ok_or_fail!(batch.batch_digest()),
        ..receipt
    };
    assert_eq!(observed, expected);
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            SearchPlaneIngestIpcRequest::PublishHistoryBatch(_)
        ),
        "expected PublishHistoryBatch, got {:?}",
        captured.payload
    );
    let SearchPlaneIngestIpcRequest::PublishHistoryBatch(wire) = &captured.payload else {
        return;
    };
    assert_eq!(wire.manifest_digest.as_deref(), Some("manifest:history-3"));
    assert_eq!(wire.batch_digest, ok_or_fail!(batch.batch_digest()));
    assert_eq!(wire.commits.len(), 1);
    assert_eq!(wire.refs.len(), 1);
    assert_eq!(wire.tags.len(), 1);
    assert_eq!(wire.diff_hunks.len(), 1);
    assert_eq!(wire.commits.len(), 1, "expected one history commit");
    let Some(first_commit) = wire.commits.first() else {
        return;
    };
    assert_eq!(first_commit.author_time_ms, 11);
    assert_eq!(wire.diff_hunks.len(), 1, "expected one history diff hunk");
    let Some(first_diff) = wire.diff_hunks.first() else {
        return;
    };
    assert_eq!(first_diff.record.hunk_header.as_ref(), "@@ -1,1 +1,2 @@");
}

#[test]
fn history_publish_repo_commit_recency_routes_through_ingest_transport() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(3),
        manifest_digest: None,
        batch_digest: String::new(),
        applied: true,
        durable_sequence: 7,
        semantic_content: None,
        accepted_clear_surfaces: 0,
        accepted_replace_scopes: 2,
        accepted_tombstone_scopes: 0,
        accepted_semantic_replace_scopes: 0,
        accepted_semantic_tombstone_scopes: 0,
        sealed: false,
    };
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(receipt.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch =
        crate::RepoCommitRecencyBatch::new(repo_id(), revision_id(), ManifestGeneration::new(3))
            .entry(
                RepoId::new("corp-a").expect("static fixture ID satisfies canonical policy"),
                1_717_171_717_000,
            )
            .entry(
                RepoId::new("corp-b").expect("static fixture ID satisfies canonical policy"),
                1_617_171_717_000,
            );
    let observed = ok_or_fail!(client.history().publish_repo_commit_recency(&batch));
    let expected = BatchPublishReceipt {
        batch_digest: ok_or_fail!(batch.batch_digest()),
        ..receipt
    };
    assert_eq!(observed, expected);
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(_)
        ),
        "expected PublishRepoCommitRecencyBatch, got {:?}",
        captured.payload
    );
    let SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(wire) = &captured.payload else {
        return;
    };
    assert_eq!(wire.batch_digest, ok_or_fail!(batch.batch_digest()));
    let [entry_a, entry_b] = wire.entries.as_slice() else {
        assert!(
            false,
            "expected two repo-commit-recency entries, got {}",
            wire.entries.len()
        );
        return;
    };
    assert_eq!(entry_a.source_repo_id.as_str(), "corp-a");
    assert_eq!(entry_a.latest_committer_time_ms, 1_717_171_717_000);
    assert_eq!(entry_b.source_repo_id.as_str(), "corp-b");
    assert_eq!(entry_b.latest_committer_time_ms, 1_617_171_717_000);
}

#[test]
fn history_publish_repo_meta_routes_through_ingest_transport() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(4),
        manifest_digest: None,
        batch_digest: String::new(),
        applied: true,
        durable_sequence: 7,
        semantic_content: None,
        accepted_clear_surfaces: 0,
        accepted_replace_scopes: 2,
        accepted_tombstone_scopes: 0,
        accepted_semantic_replace_scopes: 0,
        accepted_semantic_tombstone_scopes: 0,
        sealed: false,
    };
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::RepoMetaReceipt(receipt.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch = crate::RepoMetaBatch::new(repo_id(), revision_id(), ManifestGeneration::new(4))
        .entry(
            RepoId::new("corp-a").expect("static fixture ID satisfies canonical policy"),
            "license",
            "apache-2.0",
        )
        .entry(
            RepoId::new("corp-b").expect("static fixture ID satisfies canonical policy"),
            "license",
            "gpl-3.0",
        );
    let observed = ok_or_fail!(client.history().publish_repo_meta(&batch));
    let expected = BatchPublishReceipt {
        batch_digest: ok_or_fail!(batch.batch_digest()),
        ..receipt
    };
    assert_eq!(observed, expected);
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(_)
        ),
        "expected PublishRepoMetaBatch, got {:?}",
        captured.payload
    );
    let SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(wire) = &captured.payload else {
        return;
    };
    assert_eq!(wire.batch_digest, ok_or_fail!(batch.batch_digest()));
    let [entry_a, entry_b] = wire.entries.as_slice() else {
        assert!(
            false,
            "expected two repo-meta entries, got {}",
            wire.entries.len()
        );
        return;
    };
    assert_eq!(entry_a.source_repo_id.as_str(), "corp-a");
    assert_eq!(entry_a.key, "license");
    assert_eq!(entry_a.value, "apache-2.0");
    assert_eq!(entry_b.source_repo_id.as_str(), "corp-b");
    assert_eq!(entry_b.key, "license");
    assert_eq!(entry_b.value, "gpl-3.0");
}

#[test]
fn history_publish_repo_topic_routes_through_ingest_transport() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(5),
        manifest_digest: None,
        batch_digest: String::new(),
        applied: true,
        durable_sequence: 7,
        semantic_content: None,
        accepted_clear_surfaces: 0,
        accepted_replace_scopes: 3,
        accepted_tombstone_scopes: 0,
        accepted_semantic_replace_scopes: 0,
        accepted_semantic_tombstone_scopes: 0,
        sealed: false,
    };
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::RepoTopicReceipt(receipt.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch = crate::RepoTopicBatch::new(repo_id(), revision_id(), ManifestGeneration::new(5))
        .entry(
            RepoId::new("corp-a").expect("static fixture ID satisfies canonical policy"),
            "security",
        )
        .entry(
            RepoId::new("corp-a").expect("static fixture ID satisfies canonical policy"),
            "platform",
        )
        .entry(
            RepoId::new("corp-b").expect("static fixture ID satisfies canonical policy"),
            "ml",
        );
    let observed = ok_or_fail!(client.history().publish_repo_topic(&batch));
    let expected = BatchPublishReceipt {
        batch_digest: ok_or_fail!(batch.batch_digest()),
        ..receipt
    };
    assert_eq!(observed, expected);
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(_)
        ),
        "expected PublishRepoTopicBatch, got {:?}",
        captured.payload
    );
    let SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(wire) = &captured.payload else {
        return;
    };
    assert_eq!(wire.batch_digest, ok_or_fail!(batch.batch_digest()));
    let [entry_a, entry_b, entry_c] = wire.entries.as_slice() else {
        assert!(
            false,
            "expected three repo-topic entries, got {}",
            wire.entries.len()
        );
        return;
    };
    assert_eq!(entry_a.source_repo_id.as_str(), "corp-a");
    assert_eq!(entry_a.topic, "security");
    assert_eq!(entry_b.source_repo_id.as_str(), "corp-a");
    assert_eq!(entry_b.topic, "platform");
    assert_eq!(entry_c.source_repo_id.as_str(), "corp-b");
    assert_eq!(entry_c.topic, "ml");
}

#[test]
fn history_publish_file_ownership_routes_through_ingest_transport() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(5),
        manifest_digest: None,
        batch_digest: String::new(),
        applied: true,
        durable_sequence: 7,
        semantic_content: None,
        accepted_clear_surfaces: 0,
        accepted_replace_scopes: 2,
        accepted_tombstone_scopes: 0,
        accepted_semantic_replace_scopes: 0,
        accepted_semantic_tombstone_scopes: 0,
        sealed: false,
    };
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::FileOwnershipReceipt(receipt.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch =
        crate::FileOwnershipBatch::new(repo_id(), revision_id(), ManifestGeneration::new(5))
            .entry(
                RepoId::new("corp-a").expect("static fixture ID satisfies canonical policy"),
                RepoRelativePath::new("src/gate-a.rs"),
                vec!["@alice".to_string(), "@acme/platform".to_string()],
            )
            .entry(
                RepoId::new("corp-b").expect("static fixture ID satisfies canonical policy"),
                RepoRelativePath::new("src/gate-b.rs"),
                Vec::new(),
            );
    let observed = ok_or_fail!(client.history().publish_file_ownership(&batch));
    let expected = BatchPublishReceipt {
        batch_digest: ok_or_fail!(batch.batch_digest()),
        ..receipt
    };
    assert_eq!(observed, expected);
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            SearchPlaneIngestIpcRequest::PublishFileOwnershipBatch(_)
        ),
        "expected PublishFileOwnershipBatch, got {:?}",
        captured.payload
    );
    let SearchPlaneIngestIpcRequest::PublishFileOwnershipBatch(wire) = &captured.payload else {
        return;
    };
    assert_eq!(wire.batch_digest, ok_or_fail!(batch.batch_digest()));
    let [entry_a, entry_b] = wire.entries.as_slice() else {
        assert!(
            false,
            "expected two file-ownership entries, got {}",
            wire.entries.len()
        );
        return;
    };
    assert_eq!(entry_a.source_repo_id.as_str(), "corp-a");
    assert_eq!(entry_a.repo_relative_path.as_str(), "src/gate-a.rs");
    assert_eq!(entry_a.owners, vec!["@alice", "@acme/platform"]);
    assert_eq!(entry_b.source_repo_id.as_str(), "corp-b");
    assert_eq!(entry_b.repo_relative_path.as_str(), "src/gate-b.rs");
    assert!(entry_b.owners.is_empty());
}

#[test]
fn history_publish_file_contributor_routes_through_ingest_transport() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(6),
        manifest_digest: None,
        batch_digest: String::new(),
        applied: true,
        durable_sequence: 7,
        semantic_content: None,
        accepted_clear_surfaces: 0,
        accepted_replace_scopes: 2,
        accepted_tombstone_scopes: 0,
        accepted_semantic_replace_scopes: 0,
        accepted_semantic_tombstone_scopes: 0,
        sealed: false,
    };
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::FileContributorReceipt(receipt.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch =
        crate::FileContributorBatch::new(repo_id(), revision_id(), ManifestGeneration::new(6))
            .entry(
                RepoId::new("corp-a").expect("static fixture ID satisfies canonical policy"),
                RepoRelativePath::new("src/gate-a.rs"),
                vec!["alice".to_string(), "carol".to_string()],
            )
            .entry(
                RepoId::new("corp-b").expect("static fixture ID satisfies canonical policy"),
                RepoRelativePath::new("src/gate-b.rs"),
                vec!["bob".to_string()],
            );
    let observed = ok_or_fail!(client.history().publish_file_contributor(&batch));
    let expected = BatchPublishReceipt {
        batch_digest: ok_or_fail!(batch.batch_digest()),
        ..receipt
    };
    assert_eq!(observed, expected);
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            SearchPlaneIngestIpcRequest::PublishFileContributorBatch(_)
        ),
        "expected PublishFileContributorBatch, got {:?}",
        captured.payload
    );
    let SearchPlaneIngestIpcRequest::PublishFileContributorBatch(wire) = &captured.payload else {
        return;
    };
    assert_eq!(wire.batch_digest, ok_or_fail!(batch.batch_digest()));
    let [entry_a, entry_b] = wire.entries.as_slice() else {
        assert!(
            false,
            "expected two file-contributor entries, got {}",
            wire.entries.len()
        );
        return;
    };
    assert_eq!(entry_a.source_repo_id.as_str(), "corp-a");
    assert_eq!(entry_a.repo_relative_path.as_str(), "src/gate-a.rs");
    let canon_a: Vec<&str> = entry_a
        .contributors
        .iter()
        .map(|c| c.canonical.as_str())
        .collect();
    assert_eq!(canon_a, vec!["alice", "carol"]);
    assert_eq!(entry_b.source_repo_id.as_str(), "corp-b");
    assert_eq!(entry_b.repo_relative_path.as_str(), "src/gate-b.rs");
    let canon_b: Vec<&str> = entry_b
        .contributors
        .iter()
        .map(|c| c.canonical.as_str())
        .collect();
    assert_eq!(canon_b, vec!["bob"]);
}

#[test]
fn dirty_publish_routes_through_ingest_transport_and_carries_typed_entries() {
    let batch = DirtyBatch::new(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(4),
        1_717_171_717_000,
    )
    .upsert(sample_dirty_record())
    .delete(ChunkId::new("chunk-evict"));
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::DirtyReceipt(BatchPublishReceipt {
            generation: ManifestGeneration::new(4),
            batch_digest: ok_or_fail!(batch.batch_digest()),
            ..BatchPublishReceipt::default()
        }),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let _receipt = ok_or_fail!(client.runtime().publish_dirty(&batch));
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            SearchPlaneIngestIpcRequest::PublishDirtyBatch(_)
        ),
        "expected PublishDirtyBatch, got {:?}",
        captured.payload
    );
    let SearchPlaneIngestIpcRequest::PublishDirtyBatch(wire) = &captured.payload else {
        return;
    };
    assert_eq!(wire.overlay_epoch_ms, 1_717_171_717_000);
    assert_eq!(wire.batch_digest, ok_or_fail!(batch.batch_digest()));
    assert_eq!(wire.entries.len(), 2);
}

#[test]
fn structural_publish_routes_through_ingest_transport_and_carries_parse_trees() {
    let batch = StructuralBatch::delta(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(5),
        ManifestGeneration::new(4),
        "manifest:structural",
    )
    .replace_scope(
        sample_search_scope(),
        "scope:structural",
        vec![quanta_index_contract::StructuralTreeRecord {
            chunk_id: ChunkId::new("chunk-tree"),
            record: sample_parse_tree_record(),
        }],
    )
    .tombstone_scope(SearchScopeKey {
        doc_surface: SearchScopeSurface::Chunk,
        repo_relative_path: RepoRelativePath::new("src/old.rs"),
    });
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::StructuralReceipt(BatchPublishReceipt {
            generation: ManifestGeneration::new(5),
            batch_digest: ok_or_fail!(batch.batch_digest()),
            ..BatchPublishReceipt::default()
        }),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let _receipt = ok_or_fail!(client.structural().publish(&batch));
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            SearchPlaneIngestIpcRequest::PublishStructuralBatch(_)
        ),
        "expected PublishStructuralBatch, got {:?}",
        captured.payload
    );
    let SearchPlaneIngestIpcRequest::PublishStructuralBatch(wire) = &captured.payload else {
        return;
    };
    assert_eq!(wire.replace_scopes.len(), 1);
    assert_eq!(wire.tombstone_scopes.len(), 1);
    let Some(first_scope) = wire.replace_scopes.first() else {
        return;
    };
    assert_eq!(first_scope.trees.len(), 1);
}

#[test]
fn repomap_publish_routes_through_ingest_transport() {
    let ack = RepoMapMutationAck {
        prior_candidate_commitment: None,
        new_candidate_commitment: format!("sha256:{}", "ab".repeat(32)),
        activation_epoch: 1,
        terminal_sequence: 1,
        replayed: false,
        repo_id: repo_id(),
        revision_id: revision_id(),
        manifest_generation: ManifestGeneration::new(1),
    };
    let bundle = quanta_index_contract::RepoMapSourceBundle::new(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest-digest",
        "snap",
        1,
        "digest",
        quanta_index_contract::RepoMapGraphCoverage {
            item_index_availability: RepoMapItemIndexAvailability::Available,
            graph_coverage_class: RepoMapGraphCoverageClass::Full,
        },
        RepoMapExactnessSummary::Exact,
        RepoMapRedactionState::Unredacted,
    )
    .with_node(quanta_index_contract::RepoMapNode::File(
        quanta_index_contract::RepoMapFileNode {
            file_id: quanta_index_contract::FileId::new("file://src/lib.rs"),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            line_count: 12,
        },
    ))
    .with_node(quanta_index_contract::RepoMapNode::Symbol(
        quanta_index_contract::RepoMapSymbolNode {
            symbol_id: SymbolId::new("symbol://repomap"),
            owner_path: RepoRelativePath::new("src/lib.rs"),
            local_name: "RepoMapOwner".to_string(),
            qualified_name: "crate::RepoMapOwner".to_string(),
            symbol_kind: ok_or_fail!(SymbolKindCode::new("struct")),
        },
    ))
    .with_node(quanta_index_contract::RepoMapNode::Chunk(
        quanta_index_contract::RepoMapChunkNode {
            chunk_id: ChunkId::new("chunk://repomap"),
            owner_path: RepoRelativePath::new("src/lib.rs"),
            language: ok_or_fail!(LanguageCode::new("rust")),
            start_byte: 0,
            end_byte: 32,
            start_line: 1,
            end_line: 3,
            token_count: 16,
            preview_text: "repomap preview".to_string(),
            exactness: RepoMapChunkExactness::Exact,
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::Contains(
        quanta_index_contract::RepoMapContainsEdge {
            container: quanta_index_contract::RepoMapNodeRef::File(
                quanta_index_contract::FileId::new("file://src/lib.rs"),
            ),
            contained: quanta_index_contract::RepoMapNodeRef::Symbol(SymbolId::new(
                "symbol://repomap",
            )),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::OwnsChunk(
        quanta_index_contract::RepoMapOwnsChunkEdge {
            owner: quanta_index_contract::RepoMapNodeRef::Symbol(SymbolId::new("symbol://repomap")),
            chunk: quanta_index_contract::RepoMapNodeRef::Chunk(ChunkId::new("chunk://repomap")),
        },
    ));
    let request = ok_or_fail!(quanta_index_contract::RepoMapPublishBundleRequestV2::new(
        bundle.clone()
    ));
    let receipt = quanta_index_contract::RepoMapTerminalReceiptV2 {
        phase: quanta_index_contract::RepoMapMutationPhaseV2::Publish,
        mutation: ack,
        manifest_digest: bundle.manifest_digest.clone(),
        snapshot_id: bundle.snapshot_id.clone(),
        projection_version: bundle.projection_version,
        authority_digest: bundle.authority_digest,
        source_bundle_digest: request.source_bundle_digest.clone(),
    };
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(receipt.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let observed = ok_or_fail!(client.repomap().publish(&request));
    assert_eq!(observed, receipt);
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(matches!(
        captured.payload,
        SearchPlaneIngestIpcRequest::PublishRepoMapBundleV2(_)
    ));
}
