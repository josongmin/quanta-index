//! Real managed Store/Catalog oracle for one multi-draft posting window.
use super::*;
use quanta_index_core::index::{ObjectBatchInput, SourceTextAdmission};
use quanta_index_lexical::native::{CaseMode, NativeTextIndex, TextReadContext, TextRecipe};

#[derive(Clone, Debug)]
pub(super) struct Sample {
    leaves: usize,
    directories: usize,
    commits: u64,
}

fn shape(inputs: &[ObjectBatchInput]) -> (usize, usize) {
    // Fixed persisted codecs; observe real batches rather than a producer flag.
    (
        inputs
            .iter()
            .filter(|input| input.payload.starts_with(b"QITXDP01"))
            .count(),
        inputs
            .iter()
            .filter(|input| input.payload.starts_with(b"QITXDD01"))
            .count(),
    )
}

pub(super) fn before(
    spy: &UpdateCostSpy,
    inputs: &[ObjectBatchInput],
) -> Result<Option<native_catalog_profile::Snapshot>, IndexError> {
    if !spy.active.load(std::sync::atomic::Ordering::Acquire) || shape(inputs) == (0, 0) {
        return Ok(None);
    }
    native_catalog_profile::snapshot().map_err(|error| IndexError::Storage(error.into()))
}

pub(super) fn after(
    spy: &UpdateCostSpy,
    inputs: &[ObjectBatchInput],
    before: Option<native_catalog_profile::Snapshot>,
) -> Result<(), IndexError> {
    let Some(before) = before else { return Ok(()) };
    let sql = native_catalog_profile::since(Some(before))
        .map_err(|error| IndexError::Storage(error.into()))?
        .ok_or_else(|| IndexError::BackendViolation("dense SQL profile disappeared".into()))?;
    let (leaves, directories) = shape(inputs);
    spy.state
        .lock()
        .map_err(|error| IndexError::Storage(error.to_string()))?
        .dense_install_samples
        .push(Sample {
            leaves,
            directories,
            commits: sql.commit.calls,
        });
    Ok(())
}

fn check_membership(
    engine: &Kernel<SqliteCatalog>,
    objects: &FileObjectStore,
    view: ViewRef,
) -> TestResult {
    let ticket = engine.pin_view(ViewSelection::Exact(view), &budget())?;
    let binding = member(engine, objects, view)?;
    let reader = NativeTextIndex::new(TextRecipe::new(Digest([31; 32])));
    for (term, position) in [("alpha", 0_u32), ("beta", 1), ("gamma", 2)] {
        for case in [CaseMode::Sensitive, CaseMode::Folded] {
            let mut hits = Vec::new();
            let scan = reader.visit_term(
                &binding,
                case,
                term,
                TextReadContext {
                    ticket: ticket.record(),
                    objects,
                    maps: objects,
                    budget: &mut budget(),
                },
                |hit| {
                    hits.push((hit.owner, hit.tf, hit.positions.to_vec()));
                    Ok(())
                },
            )?;
            assert_eq!(scan.df, 9);
            let expected: Vec<_> = (1..=9_u32)
                .map(|id| (large_owner(id), 1_u64, vec![position]))
                .collect();
            assert_eq!(hits, expected);
        }
    }
    ticket.release()?;
    Ok(())
}

#[test]
fn native_text_dense_window_without_backend_credit_keeps_separate_batches_and_reopens() -> TestResult
{
    let _profile = native_catalog_profile::Guard::start()?;
    let directory = tempfile::tempdir()?;
    let observer = Arc::new(quanta_index_storage::ObjectIoObservation::default());
    let ((engine, objects, catalog, spy), text_spec) =
        native_text_services(directory.path(), 64, 64, &observer)?;
    assert!(
        objects
            .install_batch_memory_bound(
                quanta_index_core::index::ports::ObjectInstallMemoryShape {
                    maximum_inputs: 12,
                    maximum_total_payload_bytes: 4020,
                    maximum_payload_bytes: 588,
                    maximum_total_children: 6,
                },
            )?
            .is_none(),
        "this fixture intentionally has no production I/O memory bound"
    );
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
    let next_operation = operation(2);
    let inputs: Vec<_> = (1..=9_u32)
        .map(|id| SourceInput {
            path: format!("dense-{id}.rs"),
            bytes: b"alpha beta gamma".to_vec(),
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
        })
        .collect();
    let references = engine.stage_sources(
        engine.begin_input(next_operation)?,
        inputs,
        &mut large_fixture_budget(),
    )?;
    assert_eq!(references.len(), 9);
    let request = native_text_cost_request(
        &engine,
        configured,
        source(&engine, configured)?,
        next_operation,
        1,
        (1..=9_u32)
            .zip(references)
            .map(|(id, next)| OwnerMutation {
                owner: large_owner(id),
                expected: None,
                next: Some(next),
            })
            .collect(),
    )?;
    spy.arm(source(&engine, configured)?.owners, Default::default())?;
    let view = published(block_on(
        engine.submit(request, &mut large_fixture_budget()),
    )?)?;
    let _observed = spy.disarm()?;
    let samples = spy
        .state
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?
        .dense_install_samples
        .clone();
    check_membership(&engine, &objects, view)?;
    let artifact = member(&engine, &objects, view)?.artifact;
    drop((engine, objects, catalog, spy));
    let ((reopened, reopened_objects, _, _), _) =
        native_text_services(directory.path(), 64, 64, &observer)?;
    assert_eq!(
        member(&reopened, &reopened_objects, view)?.artifact,
        artifact
    );
    check_membership(&reopened, &reopened_objects, view)?;
    assert!(samples.iter().map(|sample| sample.leaves).sum::<usize>() >= 2);
    assert!(
        samples
            .iter()
            .map(|sample| sample.directories)
            .sum::<usize>()
            >= 2
    );
    // Actual SQLite COMMITs inside the concrete managed-object calls. Source,
    // inventory, wrapper, and final publication transactions are outside this sum.
    assert_eq!(
        samples.iter().map(|sample| sample.commits).sum::<u64>(),
        4,
        "unfunded leaf/directory fallback: {samples:?}",
    );
    assert_eq!(
        samples.len(),
        2,
        "unfunded leaf/directory fallback: {samples:?}"
    );
    Ok(())
}
