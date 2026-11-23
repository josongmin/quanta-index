use super::*;

#[test]
fn admitted_borrowed_history_uses_product_uds_and_retains_response_charge() {
    use std::io::Write;
    use std::os::unix::net::UnixListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use quanta_index_contract::{
        BorrowedAuxiliaryIngestIpcRequestEnvelopeV1, BorrowedAuxiliaryIngestIpcRequestV1,
        DiagnosticAttemptFieldV1, HistoryIngestBatch, SearchPlaneIngestIpcResponseEnvelope,
    };
    use quanta_index_ipc::{
        IndexError, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort,
        RequestBudgetV1,
    };

    struct LeaseOwner(Arc<AtomicU64>);
    impl MemoryLeaseOwner for LeaseOwner {
        fn release(&self, bytes: u64) {
            let _previous = self.0.fetch_sub(bytes, Ordering::SeqCst);
        }
    }
    struct Scope(Arc<AtomicU64>);
    impl MemoryScopePort for Scope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if lifetime != MemoryLifetime::Retained {
                return Err(IndexError::ResourceLimited);
            }
            let _previous = self.0.fetch_add(bytes, Ordering::SeqCst);
            Ok(MemoryLease::new(
                bytes,
                Arc::new(LeaseOwner(Arc::clone(&self.0))),
            ))
        }
    }

    let mut batch = HistoryIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation: ManifestGeneration::new(9),
        manifest_digest: None,
        batch_digest: String::new(),
        commits: Vec::new(),
        refs: Vec::new(),
        tags: Vec::new(),
        diff_hunks: Vec::new(),
    };
    ok_or_fail!(crate::stamp_batch_digest_v1(&mut batch));
    let socket_dir = ok_or_fail!(tempfile::tempdir());
    let socket = socket_dir.path().join("admitted-history.sock");
    let listener = ok_or_fail!(UnixListener::bind(&socket));
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept admitted History request");
        let request: SearchPlaneIngestIpcRequestEnvelope =
            quanta_index_ipc::decode_request(&mut stream).expect("decode canonical request");
        let SearchPlaneIngestIpcRequest::PublishHistoryBatch(batch) = request.payload else {
            panic!("unexpected ingest variant");
        };
        let reply =
            SearchPlaneIngestIpcResponseEnvelope {
                request_id: request.request_id,
                payload: SearchPlaneIngestIpcResponse::HistoryReceipt(
                    BatchPublishReceipt::empty_for(batch.generation, None, batch.batch_digest),
                ),
            };
        stream
            .write_all(&quanta_index_ipc::encode_response(&reply).expect("encode receipt"))
            .expect("write receipt");
    });
    let active = Arc::new(AtomicU64::new(0));
    let memory = MemoryScope::new(Arc::new(Scope(Arc::clone(&active))));
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(2));
    let transport = crate::transport::UdsIngestTransport::new(
        socket,
        quanta_index_ipc::ClientIoPolicy::default(),
    );
    let mut charged_work = 0_usize;
    let body = ok_or_fail!(
        crate::transport::IngestTransport::send_borrowed_auxiliary_admitted_v1(
            &transport,
            BorrowedAuxiliaryIngestIpcRequestEnvelopeV1 {
                request_id: 77,
                payload: BorrowedAuxiliaryIngestIpcRequestV1::History(&batch),
                diagnostic_attempt: DiagnosticAttemptFieldV1::Absent,
            },
            None,
            &budget,
            &memory,
            &mut |bytes| {
                charged_work = charged_work.saturating_add(bytes);
                Ok(())
            },
        )
    );
    peer.join().expect("scripted History peer");
    let decoded: SearchPlaneIngestIpcResponseEnvelope =
        ok_or_fail!(quanta_index_ipc::decode_cbor_payload(body.as_slice()));
    assert_eq!(decoded.request_id, 77);
    assert!(matches!(
        decoded.payload,
        SearchPlaneIngestIpcResponse::HistoryReceipt(_)
    ));
    assert_eq!(
        active.load(Ordering::SeqCst),
        u64::try_from(body.as_slice().len()).expect("response length fits u64")
    );
    assert!(charged_work > 0);
    drop(body);
    assert_eq!(active.load(Ordering::SeqCst), 0);
}

#[test]
fn borrowed_history_routes_preserve_canonical_body_and_receipt_binding() {
    use quanta_index_contract::{
        FileContributorEntry, FileContributorIdentityEntry, FileContributorIngestBatch,
        HistoryIngestBatch, RepoCommitRecencyEntry, RepoCommitRecencyIngestBatch,
    };

    let generation = ManifestGeneration::new(9);
    let mut history = HistoryIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation,
        manifest_digest: Some("manifest:history-9".to_string()),
        batch_digest: String::new(),
        commits: vec![sample_commit_record()],
        refs: Vec::new(),
        tags: Vec::new(),
        diff_hunks: Vec::new(),
    };
    let mut recency = RepoCommitRecencyIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation,
        batch_digest: String::new(),
        entries: vec![RepoCommitRecencyEntry {
            source_repo_id: repo_id(),
            latest_committer_time_ms: 17,
        }],
    };
    let mut contributor = FileContributorIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation,
        batch_digest: String::new(),
        entries: vec![FileContributorEntry {
            source_repo_id: repo_id(),
            repo_relative_path: RepoRelativePath::new("src/history.rs"),
            contributors: vec![FileContributorIdentityEntry {
                canonical: "alice".to_string(),
                name: None,
                email: None,
            }],
        }],
    };
    ok_or_fail!(crate::stamp_batch_digest_v1(&mut history));
    ok_or_fail!(crate::stamp_batch_digest_v1(&mut recency));
    ok_or_fail!(crate::stamp_batch_digest_v1(&mut contributor));

    for (expected, wire) in [
        (
            "history_receipt",
            SearchPlaneIngestIpcRequest::PublishHistoryBatch(history.clone()),
        ),
        (
            "repo_commit_recency_receipt",
            SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(recency.clone()),
        ),
        (
            "file_contributor_receipt",
            SearchPlaneIngestIpcRequest::PublishFileContributorBatch(contributor.clone()),
        ),
    ] {
        let digest = if let SearchPlaneIngestIpcRequest::PublishHistoryBatch(value) = &wire {
            value.batch_digest.clone()
        } else if let SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(value) = &wire {
            value.batch_digest.clone()
        } else if let SearchPlaneIngestIpcRequest::PublishFileContributorBatch(value) = &wire {
            value.batch_digest.clone()
        } else {
            panic!("fixed three-route fixture changed");
        };
        let receipt = BatchPublishReceipt {
            generation,
            manifest_digest: None,
            batch_digest: String::new(),
            applied: true,
            durable_sequence: 1,
            semantic_content: None,
            accepted_clear_surfaces: 0,
            accepted_replace_scopes: 1,
            accepted_tombstone_scopes: 0,
            accepted_semantic_replace_scopes: 0,
            accepted_semantic_tombstone_scopes: 0,
            sealed: false,
        };
        let response = match expected {
            "history_receipt" => SearchPlaneIngestIpcResponse::HistoryReceipt(receipt),
            "repo_commit_recency_receipt" => {
                SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(receipt)
            }
            "file_contributor_receipt" => {
                SearchPlaneIngestIpcResponse::FileContributorReceipt(receipt)
            }
            _ => panic!("fixed three-route fixture changed"),
        };
        let ingest = Arc::new(StubIngestTransport::new(response));
        let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
        let observed = if let SearchPlaneIngestIpcRequest::PublishHistoryBatch(value) = &wire {
            ok_or_fail!(client.history().publish_history_borrowed_v1(value))
        } else if let SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(value) = &wire {
            ok_or_fail!(
                client
                    .history()
                    .publish_repo_commit_recency_borrowed_v1(value)
            )
        } else if let SearchPlaneIngestIpcRequest::PublishFileContributorBatch(value) = &wire {
            ok_or_fail!(client.history().publish_file_contributor_borrowed_v1(value))
        } else {
            panic!("fixed three-route fixture changed");
        };
        assert_eq!(observed.batch_digest, digest);
        assert_eq!(
            ok_or_fail!(only_ingest_request(ingest.as_ref())).payload,
            wire
        );
    }
}

#[test]
fn borrowed_history_refuses_bad_digest_and_wrong_receipt_identity() {
    let generation = ManifestGeneration::new(10);
    let mut batch = quanta_index_contract::HistoryIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation,
        manifest_digest: None,
        batch_digest: "forged".to_string(),
        commits: vec![sample_commit_record()],
        refs: Vec::new(),
        tags: Vec::new(),
        diff_hunks: Vec::new(),
    };
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(11),
        manifest_digest: None,
        batch_digest: String::new(),
        applied: true,
        durable_sequence: 1,
        semantic_content: None,
        accepted_clear_surfaces: 0,
        accepted_replace_scopes: 1,
        accepted_tombstone_scopes: 0,
        accepted_semantic_replace_scopes: 0,
        accepted_semantic_tombstone_scopes: 0,
        sealed: false,
    };
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::HistoryReceipt(receipt),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    assert!(
        client
            .history()
            .publish_history_borrowed_v1(&batch)
            .is_err()
    );
    assert!(ok_or_fail!(ingest.requests.lock()).is_empty());
    ok_or_fail!(crate::stamp_batch_digest_v1(&mut batch));
    assert!(
        client
            .history()
            .publish_history_borrowed_v1(&batch)
            .is_err()
    );
    assert_eq!(ok_or_fail!(ingest.requests.lock()).len(), 1);
}

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

fn sample_runtime_catalog_batch() -> quanta_index_contract::RuntimeCatalogIngestBatch {
    use quanta_index_contract::{
        RuntimeCatalogIngestBatch, RuntimeChangedRecord, RuntimeDocFacetRecord,
        RuntimeEdgeAuthorityRecord, RuntimeSnapshotRecord,
    };

    let doc_id = ChunkId::new("chunk-runtime");
    RuntimeCatalogIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation: ManifestGeneration::new(4),
        overlay_epoch_ms: 20,
        batch_digest: "caller-supplied-digest-is-not-authority".to_string(),
        producer_head_applied_at_ms: 100,
        generation_materialized_at_ms: 20,
        changed_entries: vec![RuntimeChangedRecord {
            doc_id: doc_id.clone(),
            applied_at_ms: 25,
            payload_hash: [0xaa; 32],
        }],
        facet_entries: vec![RuntimeDocFacetRecord {
            doc_id: doc_id.clone(),
            owner: Some("team-a".to_string()),
            service: Some("search".to_string()),
            layer: Some("index".to_string()),
            surface: Some("lexical".to_string()),
        }],
        snapshot_entries: vec![RuntimeSnapshotRecord {
            name: "active".to_string(),
            doc_ids: vec![doc_id.clone()],
        }],
        affected_entries: vec![RuntimeEdgeAuthorityRecord {
            key: "rebuild=lexical".to_string(),
            doc_ids: vec![doc_id.clone()],
        }],
        invalidated_by_entries: vec![RuntimeEdgeAuthorityRecord {
            key: "rebuild=lexical".to_string(),
            doc_ids: vec![doc_id],
        }],
    }
}

#[test]
fn runtime_catalog_publish_stamps_canonical_digest_and_binds_receipt() {
    let batch = sample_runtime_catalog_batch();
    let mut expected = batch.clone();
    ok_or_fail!(crate::stamp_batch_digest_v1(&mut expected));
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(BatchPublishReceipt {
            generation: batch.generation,
            applied: true,
            durable_sequence: 7,
            ..BatchPublishReceipt::default()
        }),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let receipt = ok_or_fail!(client.producer().publish_runtime_catalog(batch));
    assert_eq!(receipt.generation, expected.generation);
    assert_eq!(receipt.batch_digest, expected.batch_digest);
    assert!(receipt.applied);
    assert_eq!(receipt.durable_sequence, 7);
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert_eq!(
        captured.payload,
        SearchPlaneIngestIpcRequest::PublishRuntimeCatalogBatch(expected)
    );
}

#[test]
fn runtime_catalog_publish_refuses_wrong_receipt_identity_and_query_only_client() {
    let batch = sample_runtime_catalog_batch();
    let ingest = Arc::new(StubIngestTransport::answering_verbatim(
        SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(BatchPublishReceipt {
            generation: batch.generation,
            batch_digest: "wrong-digest".to_string(),
            ..BatchPublishReceipt::default()
        }),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    assert!(matches!(
        client.runtime().publish_catalog(batch.clone()),
        Err(crate::SdkError::Binding { .. })
    ));
    assert_eq!(ok_or_fail!(ingest.requests.lock()).len(), 1);

    let wrong_route = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::DirtyReceipt(BatchPublishReceipt {
            generation: batch.generation,
            ..BatchPublishReceipt::default()
        }),
    ));
    let client =
        QuantaIndex::from_transports(unused_query(), unused_control(), wrong_route.clone());
    assert!(matches!(
        client.runtime().publish_catalog(batch.clone()),
        Err(crate::SdkError::Binding { .. })
    ));
    assert_eq!(ok_or_fail!(wrong_route.requests.lock()).len(), 1);

    let root = ok_or_fail!(tempfile::tempdir());
    let query_only = ok_or_fail!(QuantaIndex::connect_legacy_query_only(
        ConnectOptions::from_state_root(root.path())
    ));
    assert!(matches!(
        query_only.producer().publish_runtime_catalog(batch),
        Err(crate::SdkError::PlaneUnavailable { plane: "ingest" })
    ));
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
        source_file: quanta_index_contract::SourceFileKey {
            source_repo_id: repo_id(),
            repo_relative_path: RepoRelativePath::new("src/old.rs"),
        },
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
fn structural_publish_wire_preserves_unsealed_delta_without_base() {
    let batch = quanta_index_contract::StructuralIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation: ManifestGeneration::new(5),
        base_generation: None,
        manifest_digest: "manifest:structural".to_owned(),
        batch_digest: "stale-digest".to_owned(),
        mode: quanta_index_contract::BatchIngestMode::Delta,
        replace_scopes: vec![quanta_index_contract::StructuralReplaceScope {
            scope: sample_search_scope(),
            scope_digest: "scope:structural".to_owned(),
            trees: vec![quanta_index_contract::StructuralTreeRecord {
                chunk_id: ChunkId::new("chunk-tree"),
                record: sample_parse_tree_record(),
            }],
        }],
        tombstone_scopes: Vec::new(),
        seal: false,
    };
    let mut expected = batch.clone();
    ok_or_fail!(crate::stamp_batch_digest_v1(&mut expected));
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::StructuralReceipt(BatchPublishReceipt {
            generation: batch.generation,
            manifest_digest: Some(batch.manifest_digest.clone()),
            ..BatchPublishReceipt::default()
        }),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let receipt = ok_or_fail!(client.structural().publish_wire_batch(batch.clone()));
    assert_eq!(receipt.batch_digest, expected.batch_digest);
    assert!(!receipt.sealed);
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert_eq!(
        captured.payload,
        SearchPlaneIngestIpcRequest::PublishStructuralBatch(expected)
    );

    let wrong_generation = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::StructuralReceipt(BatchPublishReceipt {
            generation: ManifestGeneration::new(4),
            ..BatchPublishReceipt::default()
        }),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), wrong_generation);
    assert!(matches!(
        client.structural().publish_wire_batch(batch),
        Err(crate::SdkError::Binding { .. })
    ));
}
