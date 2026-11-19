//! Source body validation is shared by SourceApply and Native Text execution.

use super::*;
use quanta_index_core::index::{
    MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort,
    SourceTextAdmission, ports::CatalogCustodyMaintenancePort,
};
use quanta_index_lexical::native::{NativeTextIndex, TextRecipe};
use std::sync::atomic::{AtomicU64, Ordering};

const MEMORY_CAP: u64 = 64 * 1024 * 1024;

struct MemoryLedger(AtomicU64);

impl MemoryLeaseOwner for MemoryLedger {
    fn release(&self, bytes: u64) {
        let _previous = self.0.fetch_sub(bytes, Ordering::SeqCst);
    }
}

struct MemoryReservation(Arc<MemoryLedger>);

impl MemoryScopePort for MemoryReservation {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _previous = self
            .0
            .0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                live.checked_add(bytes).filter(|next| *next <= MEMORY_CAP)
            })
            .map_err(|_live| IndexError::ResourceLimited)?;
        Ok(MemoryLease::new(bytes, self.0.clone()))
    }
}

fn scoped_budget() -> WorkBudget {
    let mut work = budget();
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(MemoryReservation(
        Arc::new(MemoryLedger(AtomicU64::new(0))),
    )))));
    work
}

fn observed_kernel(
    root: &std::path::Path,
    text: bool,
) -> TestResult<(
    Kernel<SqliteCatalog>,
    Arc<FileObjectStore>,
    Arc<SqliteCatalog>,
    Arc<UpdateCostSpy>,
    InstanceSpec,
)> {
    let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
        root.join("scratch"),
        16 * 1024 * 1024 * 1024,
    )?);
    let objects = Arc::new(FileObjectStore::open_with_scratch(
        root.join("native-objects"),
        scratch.clone(),
    )?);
    let spy = Arc::new(UpdateCostSpy::new_observing_reads(
        Arc::clone(&objects),
        true,
    ));
    let catalog = Arc::new(SqliteCatalog::open(root, Duration::from_millis(100))?);
    let recipe = TextRecipe::new(Digest([31; 32]));
    let text_spec = InstanceSpec {
        kind: IndexKind([28; 16]),
        policy: recipe.output_policy(),
        ..spec()
    };
    let mut registry = AdapterRegistry::new();
    if text {
        registry.register(
            AdapterDescriptor {
                kind: text_spec.kind,
                schema: text_spec.schema,
                format: text_spec.format,
            },
            NativeTextIndex::new(recipe),
            |delta| Ok(delta.clone()),
        )?;
    }
    let services = KernelServices::new(
        spy.clone(),
        spy.clone(),
        catalog.clone(),
        scratch,
        Duration::from_secs(30),
    )?;
    Ok((
        Kernel::new(services, registry),
        objects,
        catalog,
        spy,
        text_spec,
    ))
}

#[test]
fn cold_native_text_bodies_are_authenticated_once_during_execute() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (engine, objects, catalog, spy, text_spec) = observed_kernel(directory.path(), true)?;
    let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
    let configure = operation(1);
    let profile = engine.stage_profile(
        engine.begin_input(configure)?,
        &IndexProfile {
            instances: vec![text_spec],
        },
        &mut budget(),
    )?;
    let configured = published(block_on(engine.submit(
        UpdateRequest {
            operation: configure,
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)?;
    let ingress = engine.begin_input(operation(2))?;
    let admitted = SourceTextAdmission::Utf8 {
        policy: Digest([31; 32]),
    };
    let records = engine.stage_sources(
        ingress,
        vec![
            SourceInput {
                path: "alpha.rs".into(),
                bytes: b"alpha shared".to_vec(),
                text: admitted,
                language: None,
            },
            SourceInput {
                path: "beta.rs".into(),
                bytes: b"beta shared".to_vec(),
                text: admitted,
                language: None,
            },
        ],
        &mut budget(),
    )?;
    let bodies: Vec<_> = records
        .iter()
        .map(|reference| {
            let stored = objects.get(*reference, &mut budget())?;
            let record: SourceRecord = serde_json::from_slice(&stored.payload)?;
            Ok(record.bytes)
        })
        .collect::<TestResult<_>>()?;
    assert_ne!(bodies[0], bodies[1]);
    let update = request(
        &engine,
        &catalog,
        &objects,
        configured,
        source(&engine, configured)?,
        2,
        1,
        [owner(1), owner(2)]
            .into_iter()
            .zip(records)
            .map(|(owner, next)| OwnerMutation {
                owner,
                expected: None,
                next: Some(next),
            })
            .collect(),
    )?;
    spy.arm(source(&engine, configured)?.owners, Default::default())?;
    let published_view = published(block_on(engine.submit(update, &mut scoped_budget()))?)?;
    let _observation = spy.disarm()?;
    let state = spy
        .state
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    for body in bodies {
        let physical = state.physical_reads.get(&body).copied().unwrap_or(0);
        let retained = state.retained_reads.get(&body).copied().unwrap_or(0);
        assert_eq!(
            physical + retained,
            1,
            "body {body:?} was authenticated more than once"
        );
    }
    drop(state);
    assert_eq!(
        native_text_test_df(&engine, &objects, published_view, "shared")?,
        2
    );
    Ok(())
}

#[test]
fn no_text_member_still_validates_deferred_utf8_claim_before_publication() -> TestResult {
    for (body, claimed_length, invalid_utf8) in
        [(vec![0xff], 1_u64, true), (b"valid".to_vec(), 6_u64, false)]
    {
        let directory = tempfile::tempdir()?;
        let (engine, objects, catalog, spy, _text_spec) = observed_kernel(directory.path(), false)?;
        let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
        let original = source(&engine, genesis)?;
        let body_ref = objects.put(&body, &[], &mut budget())?.reference;
        let record = SourceRecord {
            path: "unindexed.rs".into(),
            bytes: body_ref,
            byte_len: claimed_length,
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
            source_repo_id: None,
            language: None,
        };
        let record_ref = objects
            .put(&serde_json::to_vec(&record)?, &[body_ref], &mut budget())?
            .reference;
        let update = request(
            &engine,
            &catalog,
            &objects,
            genesis,
            original,
            2,
            1,
            vec![OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(record_ref),
            }],
        )?;
        spy.arm(original.owners, Default::default())?;
        let operation = update.operation;
        let result = block_on(engine.submit(update, &mut scoped_budget()))?;
        let _observation = spy.disarm()?;
        if invalid_utf8 {
            assert_eq!(
                result,
                SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::InvalidInput))
            );
        } else {
            assert_eq!(result, SubmitOutcome::Unknown(operation));
        }
        let state = spy
            .state
            .lock()
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        assert!(
            state.physical_reads.get(&record_ref).copied().unwrap_or(0) > 0,
            "source apply was not reached"
        );
        assert!(
            state.single_map_stream.calls + state.multi_map_stream.calls > 0,
            "the attempt did not reach deferred validation after map construction"
        );
        drop(state);
        assert_eq!(source(&engine, genesis)?, original);
        assert_eq!(
            engine
                .pin_view(
                    ViewSelection::Latest {
                        workspace: workspace(),
                        epoch: 1,
                    },
                    &budget(),
                )?
                .view()
                .identity,
            genesis
        );
        if invalid_utf8 {
            let released = catalog.release_terminal_custody(operation, 256, &mut budget())?;
            assert!(released.released > 0);
            assert!(!released.has_more);
            let replay = catalog.release_terminal_custody(operation, 256, &mut budget())?;
            assert_eq!(replay.released, 0);
            assert!(!replay.has_more);
        }
    }
    Ok(())
}

#[test]
fn cancelled_deferred_body_validation_recovers_the_original_operation() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (engine, objects, catalog, spy, _text_spec) = observed_kernel(directory.path(), false)?;
    let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
    let original = source(&engine, genesis)?;
    let mut sources = Vec::new();
    for (id, path, body) in [
        (1_u8, "first.rs", &b"first source body"[..]),
        (2_u8, "second.rs", &b"second source body"[..]),
    ] {
        let body_ref = objects.put(body, &[], &mut budget())?.reference;
        let record = SourceRecord {
            path: path.into(),
            bytes: body_ref,
            byte_len: u64::try_from(body.len())?,
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
            source_repo_id: None,
            language: None,
        };
        let record_ref = objects
            .put(&serde_json::to_vec(&record)?, &[body_ref], &mut budget())?
            .reference;
        sources.push((owner(id), body_ref, record_ref));
    }
    let update = request(
        &engine,
        &catalog,
        &objects,
        genesis,
        original,
        2,
        1,
        sources
            .iter()
            .map(|(owner, _, record)| OwnerMutation {
                owner: *owner,
                expected: None,
                next: Some(*record),
            })
            .collect(),
    )?;
    let operation = update.operation;
    let mut sorted_bodies = sources.iter().map(|(_, body, _)| *body).collect::<Vec<_>>();
    sorted_bodies.sort_unstable();
    assert_ne!(sorted_bodies[0], sorted_bodies[1]);

    spy.arm(original.owners, Default::default())?;
    spy.cancel_after_get(sorted_bodies[1])?;
    assert_eq!(
        block_on(engine.submit(update, &mut scoped_budget()))?,
        SubmitOutcome::Unknown(operation)
    );
    let _first_observation = spy.disarm()?;
    {
        let state = spy
            .state
            .lock()
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        assert!(state.single_map_stream.calls + state.multi_map_stream.calls > 0);
        for body in &sorted_bodies {
            assert!(
                state.physical_reads.get(body).copied().unwrap_or(0) > 0,
                "deferred validation did not read body {body:?}"
            );
        }
    }
    assert!(matches!(
        engine.status(
            operation,
            quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
        )?,
        OperationStatus::Pending(_)
    ));
    assert_eq!(source(&engine, genesis)?, original);
    assert_eq!(
        engine
            .pin_view(
                ViewSelection::Latest {
                    workspace: workspace(),
                    epoch: 1,
                },
                &budget(),
            )?
            .view()
            .identity,
        genesis
    );

    spy.arm(original.owners, Default::default())?;
    let published_view = published(block_on(engine.resume(operation, &mut scoped_budget()))?)?;
    let _second_observation = spy.disarm()?;
    {
        let state = spy
            .state
            .lock()
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        for body in &sorted_bodies {
            assert!(
                state.physical_reads.get(body).copied().unwrap_or(0) > 0,
                "recovery reused an earlier attempt's body proof for {body:?}"
            );
        }
    }
    let published_source = source(&engine, published_view)?;
    for (owner, _, record) in &sources {
        assert_eq!(
            objects.lookup_object(
                published_source.owners,
                &owner_map_key(*owner),
                &mut budget()
            )?,
            Some(*record)
        );
    }
    let released = catalog.release_terminal_custody(operation, 256, &mut budget())?;
    assert!(released.released > 0);
    assert!(!released.has_more);
    let replay = catalog.release_terminal_custody(operation, 256, &mut budget())?;
    assert_eq!(replay.released, 0);
    assert!(!replay.has_more);
    assert_eq!(
        block_on(engine.resume(operation, &mut budget()))?,
        SubmitOutcome::Terminal(OperationStatus::Published(published_view))
    );
    Ok(())
}
