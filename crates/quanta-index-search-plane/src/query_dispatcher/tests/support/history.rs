//! History authority ledgers and request builders.

use std::sync::{Arc, RwLock};

use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::lex::{CommitRecord, CommitSha};
use quanta_index_contract::{
    HistoryOrderV1, HistoryQueryRequest, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneQueryIpcRequest, SearchPlaneTrackKind, TextQueryRequest, TextQuerySyntax,
};

use crate::Ledger;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::tests::support::common::{
    ready_ledger, ready_pin, test_activation_catalog,
};
use crate::query_dispatcher::tests::support::lexical::RejectLexicalOpener;
use crate::query_dispatcher::tests::support::repo_map::StubRepoMapSnapshotPort;
use crate::query_dispatcher::tests::support::semantic::RejectSemanticOpener;
use crate::query_dispatcher::tests::support::structural::FailClosedStructuralProducer;

pub(crate) fn history_commit_sha() -> CommitSha {
    CommitSha::from_bytes([
        0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe, 0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc,
        0xfe, 0x10, 0x32, 0x54, 0x76,
    ])
}

pub(crate) fn rev_at_time_ancestor_sha() -> CommitSha {
    CommitSha::from_hex("1111111111111111111111111111111111111111")
        .expect("valid ancestor commit sha hex")
}

pub(crate) fn rev_at_time_head_sha() -> CommitSha {
    CommitSha::from_hex("2222222222222222222222222222222222222222")
        .expect("valid head commit sha hex")
}

pub(crate) fn history_commit_record() -> CommitRecord {
    CommitRecord {
        wire_version: 1,
        sha: history_commit_sha(),
        parents: Vec::new(),
        author_time_ms: 1,
        committer_time_ms: 2,
        applied_at_ms: 3,
        author: "alice".to_string().into_boxed_str(),
        author_name: None,
        author_email: None,
        committer: "alice".to_string().into_boxed_str(),
        committer_name: None,
        committer_email: None,
        message: "fix: history lane".to_string().into_boxed_str(),
        is_merge: false,
        tags: Vec::new(),
    }
}

pub(crate) fn history_query_request(query_text: &str) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::History(HistoryQueryRequest {
        text_query: TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: query_text.to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 5,
            cursor: None,
        },
        order: HistoryOrderV1::Recency,
        cursor: None,
    })
}

pub(crate) fn history_dispatcher_with_ledger(
    ledger: Arc<RwLock<Ledger>>,
) -> Result<SearchPlaneDispatcher, Box<dyn std::error::Error>> {
    Ok(SearchPlaneDispatcher::new(
        Arc::new(RejectLexicalOpener),
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapSnapshotPort::default()),
        Arc::new(FailClosedStructuralProducer),
        ledger,
        test_activation_catalog()?,
    ))
}

pub(crate) fn ledger_with_history_ops(
    ops: Vec<LexicalChannelOp>,
) -> Result<Arc<RwLock<Ledger>>, Box<dyn std::error::Error>> {
    let ledger = ready_ledger();
    {
        let mut guard = ledger
            .write()
            .map_err(|err| format!("history test ledger poisoned: {err}"))?;
        for op in ops {
            guard.apply_lexical_authority_op(&op, std::time::Instant::now())?;
        }
    }
    Ok(ledger)
}

pub(crate) fn ledger_with_rev_at_time_history()
-> Result<Arc<RwLock<Ledger>>, Box<dyn std::error::Error>> {
    let ledger = Arc::new(RwLock::new(Ledger::default()));
    let repo_id =
        RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy");
    let base_revision_id = RevisionId::new("2222222222222222222222222222222222222222")
        .expect("static fixture ID satisfies canonical policy");
    let ancestor_revision_id = RevisionId::new("1111111111111111111111111111111111111111")
        .expect("static fixture ID satisfies canonical policy");
    {
        let mut guard = ledger
            .write()
            .map_err(|err| format!("rev_at_time ledger poisoned: {err}"))?;
        guard.record_track_materialized(
            &repo_id,
            &base_revision_id,
            SearchPlaneTrackKind::Lexical,
            ManifestGeneration::new(9),
            None,
        );
        guard.record_track_seal(
            &repo_id,
            &base_revision_id,
            SearchPlaneTrackKind::Lexical,
            ManifestGeneration::new(9),
        );
        guard.record_track_materialized(
            &repo_id,
            &ancestor_revision_id,
            SearchPlaneTrackKind::Lexical,
            ManifestGeneration::new(7),
            None,
        );
        guard.record_track_seal(
            &repo_id,
            &ancestor_revision_id,
            SearchPlaneTrackKind::Lexical,
            ManifestGeneration::new(7),
        );
        guard.record_historically_sealed_search_corpus(
            &repo_id,
            &base_revision_id,
            ManifestGeneration::new(9),
            "manifest-digest-9",
        );
        guard.record_historically_sealed_search_corpus(
            &repo_id,
            &ancestor_revision_id,
            ManifestGeneration::new(7),
            "manifest-digest-7",
        );
        guard.apply_history_batch(
            &quanta_index_contract::HistoryIngestBatch {
                repo_id,
                revision_id: base_revision_id,
                generation: ManifestGeneration::new(9),
                manifest_digest: Some("history-rev-at-time".to_string()),
                batch_digest: "history-rev-at-time-batch".to_string(),
                commits: vec![
                    CommitRecord {
                        wire_version: 1,
                        sha: rev_at_time_ancestor_sha(),
                        parents: Vec::new(),
                        author_time_ms: 100,
                        committer_time_ms: 100,
                        applied_at_ms: 100,
                        author: "alice".to_string().into_boxed_str(),
                        author_name: None,
                        author_email: None,
                        committer: "alice".to_string().into_boxed_str(),
                        committer_name: None,
                        committer_email: None,
                        message: "old commit".to_string().into_boxed_str(),
                        is_merge: false,
                        tags: Vec::new(),
                    },
                    CommitRecord {
                        wire_version: 1,
                        sha: rev_at_time_head_sha(),
                        parents: vec![rev_at_time_ancestor_sha()],
                        author_time_ms: 200,
                        committer_time_ms: 200,
                        applied_at_ms: 200,
                        author: "alice".to_string().into_boxed_str(),
                        author_name: None,
                        author_email: None,
                        committer: "alice".to_string().into_boxed_str(),
                        committer_name: None,
                        committer_email: None,
                        message: "head commit".to_string().into_boxed_str(),
                        is_merge: false,
                        tags: Vec::new(),
                    },
                ],
                refs: vec![quanta_index_contract::HistoryRefMutation::Upsert(
                    quanta_index_contract::HistoryRefUpsert {
                        name: "HEAD".to_string().into_boxed_str(),
                        sha: rev_at_time_head_sha(),
                    },
                )],
                tags: Vec::new(),
                diff_hunks: Vec::new(),
            },
            std::time::Instant::now(),
        )?;
    }
    Ok(ledger)
}
