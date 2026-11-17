//! Real immutable object/map persistence and adapter execution. Ticket fixtures
//! are supplied explicitly; this is not a Kernel CAS, UDS or SDK qualification.
use super::TestResult;
use super::history_source_readback::{budget, commit, diff};
use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::lex::CommitSha;
use quanta_index_contract::{
    AuxEpochV1, BatchPublishReceipt, DiffHunkSide, HistoryCursor, HistoryDiffHunkUpsert,
    HistoryIngestBatch, HistoryOrderV1, HistoryRefDelete, HistoryRefMutation, HistoryRefUpsert,
    LQ_VERSION_TAG, LqExpr, LqFilter, LqLeaf, LqOptions, LqQuery, LqSpan, LqType,
    ManifestGeneration, RepoId, RevisionId, index::*,
};
use quanta_index_core::{
    AuxiliaryAuthorityCatalogPort, AuxiliaryGenerationKeyV1, AuxiliaryMutationBatchV1,
    AuxiliaryRowMutationV1, CoreError,
    index::{
        AttemptContext, IndexError, MapEdit, MemoryLease, MemoryLeaseOwner, MemoryLifetime,
        MemoryScope, MemoryScopePort, QueryReadContext, ReadSessionRef, ReadTicketLease,
        RegisteredObject, TicketRecord, WorkBudget,
        effort::ResourceNeed,
        member_map_key,
        ports::{
            ImmutableObjectPort, IndexHistoryBuildPort, ObjectCustodyPort, ObjectWriteSink,
            PersistentMapPort,
        },
    },
};
use quanta_index_ipc::stamp_batch_digest_v1;
use quanta_index_search_plane::{
    HistoryReplayFenceV1, HistorySourceReplayV1,
    native_history::{
        NATIVE_HISTORY_FORMAT_V1, NATIVE_HISTORY_KIND_V1, NATIVE_HISTORY_SCHEMA_V1,
        NativeHistoryBuildV1, NativeHistoryQueryResultV1, NativeHistoryReadV1,
        PreparedNativeHistoryV1, native_history_policy_v1, prepare_native_history_full_v1,
        prepare_native_history_snapshot_full_v1, verify_native_history_input_v1,
    },
};
use quanta_index_storage::{FileObjectStore, FileScratchSpace};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

struct FixtureCustody;
impl ObjectCustodyPort for FixtureCustody {
    fn reserve_objects(&self, _: AttemptRef, _: &[RegisteredObject]) -> Result<(), IndexError> {
        Ok(())
    }
    fn register_objects(&self, _: AttemptRef, _: &[RegisteredObject]) -> Result<(), IndexError> {
        Ok(())
    }
}
impl ObjectWriteSink for FixtureCustody {
    fn reserve(&self, _: &RegisteredObject) -> Result<(), IndexError> {
        Ok(())
    }
    fn installed(&self, _: &RegisteredObject) -> Result<(), IndexError> {
        Ok(())
    }
    fn reserve_batch(&self, _: &[RegisteredObject]) -> Result<(), IndexError> {
        Ok(())
    }
    fn installed_batch(&self, _: &[RegisteredObject]) -> Result<(), IndexError> {
        Ok(())
    }
}
struct CountedMemory(Arc<AtomicU64>);
impl MemoryLeaseOwner for CountedMemory {
    fn release(&self, bytes: u64) {
        let previous = self.0.fetch_sub(bytes, Ordering::SeqCst);
        assert!(previous >= bytes);
    }
}
impl MemoryScopePort for CountedMemory {
    fn reserve(&self, bytes: u64, _: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _previous = self
            .0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |old| {
                old.checked_add(bytes)
                    .filter(|next| *next <= 512 * 1024 * 1024)
            })
            .map_err(|_| IndexError::ResourceLimited)?;
        Ok(MemoryLease::new(bytes, Arc::new(Self(Arc::clone(&self.0)))))
    }
}
fn funded_budget(live: &Arc<AtomicU64>) -> WorkBudget {
    let mut budget = budget();
    budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(CountedMemory(Arc::clone(
        live,
    ))))));
    budget
}
fn workspace() -> WorkspaceKey {
    WorkspaceKey {
        tenant: TenantId([1; 16]),
        collection: CollectionId([2; 16]),
        worktree: WorktreeId([3; 16]),
        slot: SlotId([4; 16]),
    }
}
fn attempt() -> AttemptRef {
    AttemptRef {
        operation: OperationKey {
            workspace: workspace(),
            epoch: 1,
            producer: ProducerId([5; 16]),
            stream: StreamId([6; 16]),
            stream_epoch: 1,
            sequence: 1,
        },
        attempt: AttemptId([7; 16]),
        fence: 1,
    }
}
fn spec() -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([8; 16]),
        kind: NATIVE_HISTORY_KIND_V1,
        group: PublicationGroupId([9; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: InputDomain::History,
        schema: NATIVE_HISTORY_SCHEMA_V1,
        policy: native_history_policy_v1(),
        format: NATIVE_HISTORY_FORMAT_V1,
    }
}
fn query(kind: LqType) -> LqQuery {
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(LqLeaf::Keyword("needle".into())),
        filters: vec![LqFilter::Type { kind }],
        options: LqOptions::defaults(),
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}
fn ack(
    batch: &HistoryIngestBatch,
    sequence: u64,
) -> Result<BatchPublishReceipt, Box<dyn std::error::Error>> {
    let mut ack = BatchPublishReceipt::empty_for(
        batch.generation,
        batch.manifest_digest.clone(),
        batch.batch_digest.clone(),
    );
    ack.durable_sequence = sequence;
    ack.accepted_replace_scopes = u32::try_from(batch.commits.len() + batch.diff_hunks.len())?;
    for change in batch.refs.iter().chain(&batch.tags) {
        match change {
            HistoryRefMutation::Upsert(_) => ack.accept_replace_scope(),
            HistoryRefMutation::Delete(_) => ack.accept_tombstone_scope(),
        }
    }
    Ok(ack)
}
fn publication(
    ordinal: u64,
    before: Digest,
    after: Digest,
    batch: &HistoryIngestBatch,
    sequence: u64,
) -> Result<HistoryReplayPublicationV1, Box<dyn std::error::Error>> {
    Ok(HistoryReplayPublicationV1 {
        ordinal,
        before,
        after,
        batch: batch.clone(),
        ack: ack(batch, sequence)?,
    })
}
fn source_input(
    store: &FileObjectStore,
    fence: HistoryReplayFenceV1,
    publications: Vec<HistoryReplayPublicationV1>,
    work: &mut WorkBudget,
) -> Result<ObjectRef, Box<dyn std::error::Error>> {
    let carrier = HistorySourceInputV1 {
        wire_version: 1,
        fence,
        publications,
    };
    carrier.validate_shape().map_err(std::io::Error::other)?;
    Ok(store
        .install(&serde_json::to_vec(&carrier)?, &[], &FixtureCustody, work)?
        .reference)
}

fn ticket(
    store: &FileObjectStore,
    prepared: PreparedNativeHistoryV1,
    sequence: u64,
) -> Result<TicketRecord, Box<dyn std::error::Error>> {
    let member = prepared.into_member(&spec())?;
    let reference = store
        .put(
            &serde_json::to_vec(&member)?,
            &[
                member.artifact,
                member.input.root.0.ok_or("History root absent")?,
            ],
            &mut budget(),
        )?
        .reference;
    let members = store
        .apply(
            MapRoot(None),
            &[MapEdit::objects(
                member_map_key(member.instance).to_vec(),
                None,
                Some(reference),
            )],
            &FixtureCustody,
            &mut budget(),
        )?
        .root;
    let profile = store
        .put(
            &serde_json::to_vec(&IndexProfile {
                instances: vec![spec()],
            })?,
            &[],
            &mut budget(),
        )?
        .reference;
    let mut view = CatalogView {
        identity: ViewRef {
            workspace: workspace(),
            epoch: 1,
            sequence,
            digest: Digest([0; 32]),
        },
        profile,
        source: SourceCut {
            owners: MapRoot(None),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        },
        projections: MapRoot(None),
        members,
        coverage: MapRoot(None),
    };
    view.identity.digest = view.identity_digest()?;
    Ok(TicketRecord {
        lease: ReadTicketLease {
            id: TicketId([10; 16]),
            session: ReadSessionRef { id: 1, fence: 1 },
            fence: 1,
            expires_at_ms: u64::MAX,
        },
        view,
    })
}
fn open(
    store: &FileObjectStore,
    ticket: &TicketRecord,
    budget: &mut WorkBudget,
) -> Result<NativeHistoryReadV1, quanta_index_core::CoreError> {
    NativeHistoryReadV1::open(&mut QueryReadContext {
        ticket,
        instance: &spec(),
        objects: store,
        maps: store,
        budget,
    })
}
fn read(
    store: &FileObjectStore,
    ticket: &TicketRecord,
    query: &LqQuery,
    order: HistoryOrderV1,
    top_k: u32,
    boundary: Option<&HistoryCursor>,
    budget: &mut WorkBudget,
) -> Result<NativeHistoryQueryResultV1, quanta_index_core::CoreError> {
    open(store, ticket, budget)?.query(ticket.view.identity, query, order, top_k, boundary, budget)
}
fn gold_score(n: f32, df: f32, dl: f32, average: f32, tf: f32) -> f32 {
    let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
    idf * 2.2 * tf / (tf + 1.2 * (0.25 + 0.75 * dl / average))
}

#[test]
fn native_history_full_delta_query_old_view_and_reopen() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = FileObjectStore::open(directory.path().join("objects"))?;
    let scratch =
        FileScratchSpace::open_standalone(directory.path().join("scratch"), 512 * 1024 * 1024)?;
    let generation = AuxiliaryGenerationKeyV1 {
        repo_id: RepoId::new("repo")?,
        revision_id: RevisionId::new("main")?,
        generation: ManifestGeneration::new(1),
    };
    let mut records = vec![
        commit(CommitSha::from_bytes([1; 20]), Vec::new(), "needle needle"),
        commit(
            CommitSha::from_bytes([2; 20]),
            Vec::new(),
            "needle other words added here",
        ),
        commit(CommitSha::from_bytes([3; 20]), Vec::new(), "noise"),
    ];
    for (record, time) in records.iter_mut().zip([10, 30, 20]) {
        record.committer_time_ms = time;
    }
    records[1].author = "bob".into();
    let mut full = HistoryIngestBatch {
        repo_id: generation.repo_id.clone(),
        revision_id: generation.revision_id.clone(),
        generation: generation.generation,
        manifest_digest: None,
        batch_digest: String::new(),
        commits: records,
        refs: vec![HistoryRefMutation::Upsert(HistoryRefUpsert {
            name: "main".into(),
            sha: CommitSha::from_bytes([2; 20]),
        })],
        tags: vec![HistoryRefMutation::Upsert(HistoryRefUpsert {
            name: "old".into(),
            sha: CommitSha::from_bytes([1; 20]),
        })],
        diff_hunks: vec![HistoryDiffHunkUpsert {
            commit_sha: CommitSha::from_bytes([1; 20]),
            file_path: "archive/deleted.rs".into(),
            record: diff(DiffHunkSide::After, "needle old addition"),
        }],
    };
    let long_path = format!("{}leaf.rs", "p/".repeat(2038));
    full.diff_hunks.push(HistoryDiffHunkUpsert {
        commit_sha: CommitSha::from_bytes([1; 20]),
        file_path: long_path.clone().into(),
        record: diff(DiffHunkSide::After, "needle long historical path"),
    });
    stamp_batch_digest_v1(&mut full)?;
    let fence = HistoryReplayFenceV1 {
        generation: generation.clone(),
        manifest_digest: None,
        publication_count: 1,
        first_replay: Digest([1; 32]),
        final_replay: Digest([2; 32]),
        final_durable_sequence: 5,
    };
    let live = Arc::new(AtomicU64::new(0));
    let mut work = funded_budget(&live);
    let source = HistorySourceReplayV1::new(fence.clone())?
        .apply(
            1,
            fence.first_replay,
            fence.final_replay,
            &full,
            &ack(&full, 5)?,
            &mut work,
        )?
        .finish(&mut work)?;
    let full_source_input = source_input(
        &store,
        fence.clone(),
        vec![publication(1, Digest([1; 32]), Digest([2; 32]), &full, 5)?],
        &mut work,
    )?;
    let mut context = AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &store,
        custody: &FixtureCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    let prepared = prepare_native_history_full_v1(&source, full_source_input, &mut context)?;
    // An actual SQL snapshot supplies the raw source rows. The replay fold,
    // independent of the Native target, supplies the expected cut for this
    // fixture; production must retain its real writer and producer fence.
    let sql = SqliteCatalog::open(
        &directory.path().join("history-sql"),
        Duration::from_millis(500),
    )?;
    let mut sql_rows = Vec::new();
    source.visit_rows(context.budget, &mut |row, _| {
        sql_rows.push(AuxiliaryRowMutationV1::Upsert(row.clone()));
        Ok(())
    })?;
    let _written = sql.apply(&AuxiliaryMutationBatchV1 {
        rows: sql_rows,
        tracks: Vec::new(),
    })?;
    let snapshot_prepared = prepare_native_history_snapshot_full_v1(
        source.cut(),
        5,
        None,
        full_source_input,
        |budget, visit| sql.for_each_history_row(&generation, budget, visit),
        &mut context,
    )?;
    assert_eq!(snapshot_prepared.artifact(), prepared.artifact());
    let before_snapshot_refusal = live.load(Ordering::SeqCst);
    let mut wrong_source_cut = source.cut().clone();
    wrong_source_cut.snapshot_digest = Digest([91; 32]);
    assert!(prepare_native_history_snapshot_full_v1(
        &wrong_source_cut,
        5,
        None,
        full_source_input,
        |budget, visit| sql.for_each_history_row(&generation, budget, visit),
        &mut context,
    ).is_err());
    assert_eq!(live.load(Ordering::SeqCst), before_snapshot_refusal);
    assert!(prepare_native_history_snapshot_full_v1(
        source.cut(),
        0,
        None,
        full_source_input,
        |budget, visit| sql.for_each_history_row(&generation, budget, visit),
        &mut context,
    ).is_err());
    assert!(prepare_native_history_snapshot_full_v1(
        source.cut(),
        5,
        None,
        full_source_input,
        |budget, visit| {
            let mut ordinal = 0_u64;
            sql.for_each_history_row(&generation, budget, &mut |row, budget| {
                ordinal += 1;
                if ordinal == 2 { Ok(()) } else { visit(row, budget) }
            })
        },
        &mut context,
    ).is_err());
    assert_eq!(live.load(Ordering::SeqCst), before_snapshot_refusal);
    assert!(prepare_native_history_snapshot_full_v1(
        source.cut(),
        5,
        None,
        full_source_input,
        |budget, visit| {
            sql.for_each_history_row(&generation, budget, visit)?;
            Err(CoreError::Storage("fixture source close refused after rows".into()))
        },
        &mut context,
    ).is_err());
    assert_eq!(live.load(Ordering::SeqCst), before_snapshot_refusal);
    let input = prepared.artifact();
    let mut wrong = source.cut().clone();
    wrong.replay_start = Digest([99; 32]);
    assert!(verify_native_history_input_v1(input, &wrong, &spec(), &mut context).is_err());
    assert!(verify_native_history_input_v1(input, source.cut(), &spec(), &mut context).is_ok());
    let full_cut = source.cut().clone();
    let profile = store
        .put(
            &serde_json::to_vec(&IndexProfile {
                instances: vec![spec()],
            })?,
            &[],
            context.budget,
        )?
        .reference;
    let build = NativeHistoryBuildV1::new(ResourceNeed {
        cpu_units: 1,
        io_bytes: 512 * 1024 * 1024,
        memory_bytes: 512 * 1024 * 1024,
        provider_units: 0,
    })?;
    let full_transition = HistoryTransition {
        instance: spec().instance,
        expected: None,
        next: full_cut.clone(),
        input: full_source_input,
        profile,
    };
    let prior_member = build.verify_history(&spec(), None, &full_transition, &mut context)?;
    assert_eq!(prior_member.artifact, prepared.artifact());
    assert_ne!(prior_member.artifact, full_source_input);
    let wrong_mode = HistoryTransition {
        expected: Some(full_cut.clone()),
        ..full_transition.clone()
    };
    assert!(
        build
            .verify_history(&spec(), None, &wrong_mode, &mut context)
            .is_err()
    );

    // A late whole-inventory failure must retire every provisionally decoded
    // row grant. This changes only the fixture's immutable manifest count.
    let original = store.get(input, context.budget)?;
    let mut malformed: serde_json::Value = serde_json::from_slice(&original.payload)?;
    let count = malformed
        .get("row_count")
        .and_then(serde_json::Value::as_u64)
        .ok_or("row count absent")?;
    malformed["row_count"] = serde_json::Value::from(count + 1);
    let malformed = context.put(
        &serde_json::to_vec(&malformed)?,
        &original.registration.children,
    )?;
    drop(original);
    let before_failure = live.load(Ordering::SeqCst);
    assert!(
        verify_native_history_input_v1(malformed, source.cut(), &spec(), &mut context).is_err()
    );
    assert_eq!(live.load(Ordering::SeqCst), before_failure);
    let old = ticket(&store, prepared, 1)?;
    drop(source);
    drop(work);
    let baseline = live.load(Ordering::SeqCst);
    let mut work = funded_budget(&live);
    let q = query(LqType::Commit);
    let first = read(
        &store,
        &old,
        &q,
        HistoryOrderV1::Recency,
        1,
        None,
        &mut work,
    )?;
    assert_eq!(first.page().commits[0].sha, CommitSha::from_bytes([2; 20]));
    assert_eq!((first.page().matched, first.page().remaining), (2, 2));
    assert!(live.load(Ordering::SeqCst) > baseline);
    let cursor = first
        .page()
        .next_cursor
        .clone()
        .ok_or("first page must continue")?;
    drop(first);
    assert_eq!(live.load(Ordering::SeqCst), baseline);
    let second = read(
        &store,
        &old,
        &q,
        HistoryOrderV1::Recency,
        1,
        Some(&cursor),
        &mut work,
    )?;
    assert_eq!(second.page().commits[0].sha, CommitSha::from_bytes([1; 20]));
    assert_eq!((second.page().matched, second.page().remaining), (2, 1));
    drop(second);
    let relevant = read(
        &store,
        &old,
        &q,
        HistoryOrderV1::Relevance,
        2,
        None,
        &mut work,
    )?;
    assert_eq!(
        relevant
            .page()
            .commits
            .iter()
            .map(|row| row.sha)
            .collect::<Vec<_>>(),
        [
            CommitSha::from_bytes([1; 20]),
            CommitSha::from_bytes([2; 20])
        ]
    );
    // Independent corpus gold: whitespace words in the three fixed messages,
    // N=3, df=2, lengths2/5/1, first tf2. No tokenizer/scorer reads the output.
    let gold = gold_score(3.0, 2.0, 2.0, 8.0 / 3.0, 2.0);
    assert!(
        (relevant.page().commits[0]
            .score
            .ok_or("score absent")?
            .get()
            - gold)
            .abs()
            < 0.00001
    );
    drop(relevant);
    let mut filtered = q.clone();
    filtered.filters.push(LqFilter::Author {
        pattern: "bob".into(),
    });
    let filtered_page = read(
        &store,
        &old,
        &filtered,
        HistoryOrderV1::Relevance,
        1,
        None,
        &mut work,
    )?;
    assert_eq!(
        filtered_page.page().commits[0].sha,
        CommitSha::from_bytes([2; 20])
    );
    assert!(
        (filtered_page.page().commits[0]
            .score
            .ok_or("score absent")?
            .get()
            - gold_score(3.0, 2.0, 5.0, 8.0 / 3.0, 1.0))
        .abs()
            < 0.00001
    );
    assert_eq!(filtered_page.page().matched, 1);
    drop(filtered_page);
    let diffs = read(
        &store,
        &old,
        &query(LqType::Diff),
        HistoryOrderV1::Relevance,
        5,
        None,
        &mut work,
    )?;
    assert_eq!(
        diffs.page().diffs[0].repo_relative_path,
        "archive/deleted.rs"
    );
    assert!(
        diffs
            .page()
            .diffs
            .iter()
            .any(|row| row.repo_relative_path == long_path)
    );
    let original_diff_score = diffs.page().diffs[0].score;
    drop(diffs);
    let mut forged = cursor.clone();
    forged.committer_time_ms += 1;
    assert!(
        read(
            &store,
            &old,
            &q,
            HistoryOrderV1::Recency,
            1,
            Some(&forged),
            &mut work
        )
        .is_err()
    );
    assert_eq!(live.load(Ordering::SeqCst), baseline);
    let mut delta = HistoryIngestBatch {
        commits: vec![commit(
            CommitSha::from_bytes([4; 20]),
            vec![CommitSha::from_bytes([2; 20])],
            "needle newest",
        )],
        refs: vec![HistoryRefMutation::Upsert(HistoryRefUpsert {
            name: "main".into(),
            sha: CommitSha::from_bytes([4; 20]),
        })],
        tags: vec![HistoryRefMutation::Delete(HistoryRefDelete {
            name: "old".into(),
        })],
        diff_hunks: Vec::new(),
        ..full.clone()
    };
    delta.commits[0].committer_time_ms = 40;
    stamp_batch_digest_v1(&mut delta)?;
    let next_fence = HistoryReplayFenceV1 {
        generation,
        manifest_digest: None,
        publication_count: 2,
        first_replay: Digest([2; 32]),
        final_replay: Digest([3; 32]),
        final_durable_sequence: 17,
    };
    // The expected successor is reconstructed from the fixed original two
    // publications, not from the target Native manifest being checked.
    let complete_source = HistorySourceReplayV1::new(HistoryReplayFenceV1 {
        publication_count: 2,
        final_replay: Digest([3; 32]),
        final_durable_sequence: 17,
        ..fence.clone()
    })?
    .apply(
        1,
        Digest([1; 32]),
        Digest([2; 32]),
        &full,
        &ack(&full, 5)?,
        &mut work,
    )?
    .apply(
        2,
        Digest([2; 32]),
        Digest([3; 32]),
        &delta,
        &ack(&delta, 17)?,
        &mut work,
    )?
    .finish(&mut work)?;
    let next_cut = complete_source.cut().clone();
    let full_inventory_fence = HistoryReplayFenceV1 {
        publication_count: 2,
        final_replay: Digest([3; 32]),
        final_durable_sequence: 17,
        ..fence.clone()
    };
    let full_substitute = source_input(
        &store,
        full_inventory_fence,
        vec![
            publication(1, Digest([1; 32]), Digest([2; 32]), &full, 5)?,
            publication(2, Digest([2; 32]), Digest([3; 32]), &delta, 17)?,
        ],
        &mut work,
    )?;
    let delta_source_input = source_input(
        &store,
        next_fence.clone(),
        vec![publication(
            2,
            Digest([2; 32]),
            Digest([3; 32]),
            &delta,
            17,
        )?],
        &mut work,
    )?;
    let read_handle = open(&store, &old, &mut work)?;
    let mut context = AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &store,
        custody: &FixtureCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    let next = read_handle.prepare_delta(
        next_fence.clone(),
        &delta,
        &ack(&delta, 17)?,
        delta_source_input,
        &mut context,
    )?;
    let transition = HistoryTransition {
        instance: spec().instance,
        expected: Some(full_cut.clone()),
        next: next_cut.clone(),
        input: delta_source_input,
        profile,
    };
    let verified_next =
        build.verify_history(&spec(), Some(&prior_member), &transition, &mut context)?;
    assert_eq!(verified_next.artifact, next.artifact());
    assert_ne!(verified_next.artifact, delta_source_input);
    assert_eq!(
        verified_next.input.context,
        next_cut.input_context_v1().map_err(std::io::Error::other)?
    );
    assert!(
        verify_native_history_input_v1(next.artifact(), &next_cut, &spec(), &mut context).is_ok()
    );
    let fake_full = HistoryTransition {
        expected: None,
        ..transition.clone()
    };
    assert!(
        build
            .verify_history(&spec(), None, &fake_full, &mut context)
            .is_err()
    );
    let fake_delta = HistoryTransition {
        input: full_substitute,
        ..transition.clone()
    };
    assert!(
        build
            .verify_history(&spec(), Some(&prior_member), &fake_delta, &mut context)
            .is_err()
    );
    let mut forged_prior = prior_member.clone();
    forged_prior.input.context = next_cut.input_context_v1().map_err(std::io::Error::other)?;
    let forged_expected = HistoryTransition {
        expected: Some(next_cut.clone()),
        ..transition.clone()
    };
    assert!(
        build
            .verify_history(&spec(), Some(&forged_prior), &forged_expected, &mut context)
            .is_err()
    );
    // A body+ACK with a valid self-consistent digest still cannot prove an
    // unchanged next row set when its actual commit message was changed.
    let mut altered_batch = delta.clone();
    altered_batch.commits[0].message = "foreign publication body".into();
    stamp_batch_digest_v1(&mut altered_batch)?;
    let false_input = source_input(
        &store,
        next_fence,
        vec![publication(
            2,
            Digest([2; 32]),
            Digest([3; 32]),
            &altered_batch,
            17,
        )?],
        context.budget,
    )?;
    let false_transition = HistoryTransition {
        input: false_input,
        ..transition.clone()
    };
    assert!(
        build
            .verify_history(
                &spec(),
                Some(&prior_member),
                &false_transition,
                &mut context
            )
            .is_err()
    );
    let artifact_as_source = HistoryTransition {
        input: next.artifact(),
        ..transition.clone()
    };
    assert!(
        build
            .verify_history(
                &spec(),
                Some(&prior_member),
                &artifact_as_source,
                &mut context
            )
            .is_err()
    );
    drop(complete_source);
    let new = ticket(&store, next, 2)?;
    let latest = read(
        &store,
        &new,
        &q,
        HistoryOrderV1::Recency,
        5,
        None,
        &mut work,
    )?;
    assert_eq!(
        latest
            .page()
            .commits
            .iter()
            .map(|row| row.sha)
            .collect::<Vec<_>>(),
        [
            CommitSha::from_bytes([4; 20]),
            CommitSha::from_bytes([2; 20]),
            CommitSha::from_bytes([1; 20])
        ]
    );
    assert_eq!(latest.page().epoch, AuxEpochV1::new(2));
    drop(latest);
    let retained = read(
        &store,
        &old,
        &q,
        HistoryOrderV1::Recency,
        1,
        Some(&cursor),
        &mut work,
    )?;
    assert_eq!(
        retained.page().commits[0].sha,
        CommitSha::from_bytes([1; 20])
    );
    drop(retained);
    assert!(
        read(
            &store,
            &new,
            &q,
            HistoryOrderV1::Recency,
            1,
            Some(&cursor),
            &mut work
        )
        .is_err()
    );
    drop(store);
    let reopened = FileObjectStore::open(directory.path().join("objects"))?;
    let restored_old = read(
        &reopened,
        &old,
        &q,
        HistoryOrderV1::Relevance,
        5,
        None,
        &mut work,
    )?;
    assert_eq!(restored_old.page().matched, 2);
    assert_eq!(restored_old.page().epoch, AuxEpochV1::new(1));
    drop(restored_old);
    let restored_new = read(
        &reopened,
        &new,
        &q,
        HistoryOrderV1::Relevance,
        5,
        None,
        &mut work,
    )?;
    assert_eq!(restored_new.page().matched, 3);
    assert_eq!(restored_new.page().epoch, AuxEpochV1::new(2));
    drop(restored_new);
    let restored_diff = read(
        &reopened,
        &new,
        &query(LqType::Diff),
        HistoryOrderV1::Relevance,
        5,
        None,
        &mut work,
    )?;
    assert_eq!(restored_diff.page().diffs[0].score, original_diff_score);
    drop(restored_diff);
    let mut main_query = q.clone();
    main_query.filters.push(LqFilter::Rev {
        spec: "main".into(),
    });
    let main_page = read(
        &reopened,
        &new,
        &main_query,
        HistoryOrderV1::Recency,
        5,
        None,
        &mut work,
    )?;
    assert_eq!(
        main_page.page().commits[0].sha,
        CommitSha::from_bytes([4; 20])
    );
    drop(main_page);
    let mut old_tag = q.clone();
    old_tag.filters.push(LqFilter::Rev { spec: "old".into() });
    let removed_tag = read(
        &reopened,
        &new,
        &old_tag,
        HistoryOrderV1::Recency,
        5,
        None,
        &mut work,
    )?;
    assert!(removed_tag.page().commits.is_empty());
    drop(removed_tag);
    let retained_tag = read(
        &reopened,
        &old,
        &old_tag,
        HistoryOrderV1::Recency,
        5,
        None,
        &mut work,
    )?;
    assert_eq!(
        retained_tag.page().commits[0].sha,
        CommitSha::from_bytes([1; 20])
    );
    drop(retained_tag);

    let mut cancelled = funded_budget(&live);
    cancelled.request().cancel_handle().cancel();
    assert!(open(&reopened, &new, &mut cancelled).is_err());
    drop(work);
    assert_eq!(live.load(Ordering::SeqCst), baseline);
    Ok(())
}

#[test]
fn native_history_replay_memory_retires_superseded_rows() -> TestResult {
    let generation = AuxiliaryGenerationKeyV1 {
        repo_id: RepoId::new("replay-memory")?,
        revision_id: RevisionId::new("main")?,
        generation: ManifestGeneration::new(1),
    };
    let position = |ordinal: u64| {
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&ordinal.to_le_bytes());
        Digest(bytes)
    };
    let live = Arc::new(AtomicU64::new(0));
    let mut work = funded_budget(&live);
    let fence = HistoryReplayFenceV1 {
        generation: generation.clone(),
        manifest_digest: None,
        publication_count: 301,
        first_replay: position(1),
        final_replay: position(302),
        final_durable_sequence: 301,
    };
    let sha = CommitSha::from_bytes([1; 20]);
    let mut batch = HistoryIngestBatch {
        repo_id: generation.repo_id,
        revision_id: generation.revision_id,
        generation: generation.generation,
        manifest_digest: None,
        batch_digest: String::new(),
        commits: vec![commit(sha, Vec::new(), "needle")],
        refs: Vec::new(),
        tags: Vec::new(),
        diff_hunks: Vec::new(),
    };
    stamp_batch_digest_v1(&mut batch)?;
    let mut replay = HistorySourceReplayV1::new(fence)?.apply(
        1,
        position(1),
        position(2),
        &batch,
        &ack(&batch, 1)?,
        &mut work,
    )?;
    let initial = live.load(Ordering::SeqCst);
    batch.commits.clear();
    for ordinal in 2..=301 {
        // Each body has a distinct new ref and removes its predecessor. This
        // avoids inventing different durable ACKs for the same idempotency key.
        batch.refs = vec![HistoryRefMutation::Upsert(HistoryRefUpsert {
            name: format!("ref-{ordinal}").into(),
            sha,
        })];
        if ordinal > 2 {
            batch
                .refs
                .push(HistoryRefMutation::Delete(HistoryRefDelete {
                    name: format!("ref-{}", ordinal - 1).into(),
                }));
        }
        stamp_batch_digest_v1(&mut batch)?;
        replay = replay.apply(
            ordinal,
            position(ordinal),
            position(ordinal + 1),
            &batch,
            &ack(&batch, ordinal)?,
            &mut work,
        )?;
        assert!(
            live.load(Ordering::SeqCst) <= initial + 32 * 1024,
            "superseded History publications retained DATA grants"
        );
    }
    let complete = replay.finish(&mut work)?;
    assert_eq!(complete.cut().durable_epoch, AuxEpochV1::new(301));
    drop(complete);
    drop(work);
    assert_eq!(live.load(Ordering::SeqCst), 0);
    Ok(())
}
