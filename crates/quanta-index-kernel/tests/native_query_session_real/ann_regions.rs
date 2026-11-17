//! Real Catalog/FileObjectStore ANN region custody across selected views.

use super::*;
use quanta_index_contract::native_protocol::NativeSemanticTraversal;
use quanta_index_core::diagnostic_trace::{
    DiagnosticAnnRegionObservationV1, DiagnosticAnnRegionPhaseV1, DiagnosticCaptureIdV1,
    DiagnosticClockV1, DiagnosticContextV1, DiagnosticEmitV1, DiagnosticEventKindV1,
    DiagnosticEventSinkV1, DiagnosticLaneV1, DiagnosticRootKindV1, DiagnosticTerminalV1,
};
use quanta_index_core::index::{WorkIntent, WorkScope};
use std::sync::{Mutex, atomic::AtomicBool};

#[expect(
    clippy::disallowed_methods,
    reason = "the ANN integration test drives Native materialization inside its owned Tokio reactor"
)]
fn ann_block_on<F: std::future::Future>(runtime: &tokio::runtime::Runtime, future: F) -> F::Output {
    runtime.block_on(future)
}

fn ann_budget() -> WorkBudget {
    ann_budget_with_request(RequestBudgetV1::for_duration(Duration::from_secs(120)))
}

fn ann_budget_with_request(request: RequestBudgetV1) -> WorkBudget {
    WorkBudget::with_request(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 1_500_000_000,
            mutations: 1_000_000,
        },
        request,
    )
}

fn extra_owner(id: u16) -> OwnerKey {
    let stable_id = if let Ok(id) = u8::try_from(id) {
        [id; 16]
    } else {
        let mut bytes = [0; 16];
        bytes[..2].copy_from_slice(&id.to_be_bytes());
        bytes
    };
    OwnerKey {
        producer: PRODUCER,
        stable_id: StableOwnerId(stable_id),
    }
}

struct AnnSources {
    view: ViewRef,
    primary: ObjectRef,
    extra_sources: Vec<(OwnerKey, ObjectRef)>,
}

/// Stage actual UTF-8 sources under one ingress and publish one source cut.
/// The first owner is the existing fixture's `owner()` so `advance` can later
/// replace it without a second fixture protocol.
fn advance_ann_sources_count(
    fixture: &Fixture,
    view: ViewRef,
    count: u16,
) -> TestResult<AnnSources> {
    let extra_count = count
        .checked_sub(1)
        .ok_or("ANN source count must be positive")?;
    let last_id = count.checked_add(5).ok_or("ANN source identity overflow")?;
    let key = operation(2);
    let ingress = fixture.kernel.begin_input(key)?;
    let mut staged = Vec::new();
    staged.try_reserve_exact(usize::from(count))?;
    let primary = fixture.kernel.stage_source_text(
        ingress,
        "a.rs",
        b"alpha",
        SourceTextAdmission::Utf8 { policy: ADMISSION },
        &mut ann_budget(),
    )?;
    staged.push(OwnerMutation {
        owner: owner(),
        expected: None,
        next: Some(primary),
    });
    let mut extra_sources = Vec::new();
    extra_sources.try_reserve_exact(usize::from(extra_count))?;
    for id in 7..=last_id {
        let source = fixture.kernel.stage_source_text(
            ingress,
            &format!("src/{id}.rs"),
            b"beta",
            SourceTextAdmission::Utf8 { policy: ADMISSION },
            &mut ann_budget(),
        )?;
        let owner = extra_owner(id);
        staged.push(OwnerMutation {
            owner,
            expected: None,
            next: Some(source),
        });
        extra_sources.push((owner, source));
    }
    staged.sort_unstable_by_key(|mutation| mutation.owner);
    let owners =
        fixture
            .kernel
            .stage_mutations(ingress, staged.into_iter().map(Ok), &mut ann_budget())?;
    let mut transition = SourceTransition {
        expected_source: source(&fixture.kernel, view)?,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: PRODUCER,
        stream: StreamId([5; 16]),
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    let next = published(block_on(fixture.kernel.submit(
        UpdateRequest {
            operation: key,
            expected_view: view,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &mut ann_budget(),
    ))?)?;
    Ok(AnnSources {
        view: next,
        primary,
        extra_sources,
    })
}

fn advance_ann_sources(fixture: &Fixture, view: ViewRef) -> TestResult<AnnSources> {
    advance_ann_sources_count(fixture, view, 64)
}

fn delete_extra_ann_sources(
    fixture: &Fixture,
    view: ViewRef,
    extra_sources: &[(OwnerKey, ObjectRef)],
) -> TestResult<ViewRef> {
    assert_eq!(extra_sources.len(), 63);
    let key = operation(69);
    let ingress = fixture.kernel.begin_input(key)?;
    let owners = fixture.kernel.stage_mutations(
        ingress,
        extra_sources.iter().map(|(owner, reference)| {
            Ok(OwnerMutation {
                owner: *owner,
                expected: Some(*reference),
                next: None,
            })
        }),
        &mut ann_budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: source(&fixture.kernel, view)?,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: PRODUCER,
        stream: StreamId([5; 16]),
        stream_epoch: 1,
        predecessor: 2,
        sequence: 3,
        payload: transition.owner_digest()?,
    });
    published(block_on(fixture.kernel.submit(
        UpdateRequest {
            operation: key,
            expected_view: view,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &mut ann_budget(),
    ))?)
}

fn materialize_ann_owners_range(
    fixture: &Fixture,
    runtime: &tokio::runtime::Runtime,
    mut view: ViewRef,
    first_sequence: u64,
    last_sequence: u64,
    expected_remaining: usize,
    mut pinned: Option<&mut quanta_index_kernel::ReadTicket>,
) -> TestResult<ViewRef> {
    let initial = fixture
        .kernel
        .ready_work(workspace(), 1, 64, &mut ann_budget())?;
    assert_eq!(initial.len(), 64);
    for sequence in first_sequence..=last_sequence {
        if let Some(ticket) = pinned.as_deref_mut() {
            ticket.renew()?;
        }
        let ready = fixture
            .kernel
            .ready_work(workspace(), 1, 64, &mut ann_budget())?;
        let work = ready
            .first()
            .ok_or("ANN owner work ended before expected materializations")?;
        view = materialize_ann_work(fixture, runtime, view, work, sequence)?;
    }
    let remaining = fixture
        .kernel
        .ready_work(workspace(), 1, 64, &mut ann_budget())?;
    assert_eq!(remaining.len(), expected_remaining);
    Ok(view)
}

fn materialize_ann_work(
    fixture: &Fixture,
    runtime: &tokio::runtime::Runtime,
    view: ViewRef,
    work: &WorkIntent,
    sequence: u64,
) -> TestResult<ViewRef> {
    let claim = fixture
        .kernel
        .claim_work(work, Duration::from_secs(120), &ann_budget())?;
    published(ann_block_on(
        runtime,
        fixture.kernel.submit(
            UpdateRequest {
                operation: operation(sequence),
                expected_view: view,
                kind: UpdateKind::Materialize { claim },
            },
            &mut ann_budget(),
        ),
    )?)
}

fn materialize_ann_owners(
    fixture: &Fixture,
    runtime: &tokio::runtime::Runtime,
    view: ViewRef,
) -> TestResult<ViewRef> {
    materialize_ann_owners_range(fixture, runtime, view, 3, 66, 0, None)
}

fn semantic_hit_observed(
    fixture: &Fixture,
    view: ViewRef,
) -> TestResult<(
    OwnerKey,
    f64,
    CoverageBinding,
    NativeSemanticTraversal,
    bool,
)> {
    semantic_hit_with_budget(fixture, view, &mut ann_budget())
}

fn semantic_hit_with_budget(
    fixture: &Fixture,
    view: ViewRef,
    work: &mut WorkBudget,
) -> TestResult<(
    OwnerKey,
    f64,
    CoverageBinding,
    NativeSemanticTraversal,
    bool,
)> {
    let mut session = fixture
        .kernel
        .begin_query_session(ViewSelection::Exact(view), work)?;
    let stage = session.prepare_stage(&fixture.semantic, SEMANTIC_INSTANCE, semantic_request())?;
    let need = stage.required_resources();
    let outcome = stage.execute(&guard(need)?)?;
    assert_eq!(outcome.view(), view);
    let [hit] = outcome.result().hits.as_slice() else {
        return Err("expected one selected ANN hit".into());
    };
    let coverage = match outcome.coverage() {
        QueryCoverage::Async(binding) => **binding,
        QueryCoverage::Core => return Err("ANN query returned core coverage".into()),
    };
    let result = (
        hit.owner,
        hit.cosine,
        coverage,
        outcome.result().traversal,
        outcome.exhaustive(),
    );
    assert_eq!(outcome.result().exhaustive, outcome.exhaustive());
    session.close()?;
    Ok(result)
}

fn semantic_hit(fixture: &Fixture, view: ViewRef) -> TestResult<(OwnerKey, f64, CoverageBinding)> {
    let (owner, cosine, coverage, _traversal, _exhaustive) = semantic_hit_observed(fixture, view)?;
    Ok((owner, cosine, coverage))
}

fn sealed_semantic_hit(
    fixture: &Fixture,
    view: ViewRef,
) -> TestResult<(OwnerKey, f64, CoverageBinding)> {
    let (owner, cosine, coverage, traversal, exhaustive) = semantic_hit_observed(fixture, view)?;
    assert!(matches!(
        traversal,
        NativeSemanticTraversal::Approximate { .. }
    ));
    assert!(!exhaustive);
    Ok((owner, cosine, coverage))
}

#[derive(Debug, Default)]
struct CarryTraceClock(AtomicU64);

impl DiagnosticClockV1 for CarryTraceClock {
    fn now_ns(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

#[derive(Debug)]
struct CarryRegionSink {
    selected: Mutex<Vec<DiagnosticAnnRegionObservationV1>>,
    dropped: AtomicBool,
}

impl Default for CarryRegionSink {
    fn default() -> Self {
        Self {
            selected: Mutex::new(Vec::with_capacity(8)),
            dropped: AtomicBool::new(false),
        }
    }
}

impl DiagnosticEventSinkV1 for CarryRegionSink {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        if matches!(event.kind, DiagnosticEventKindV1::Health { .. }) {
            self.dropped.store(true, Ordering::Relaxed);
            return true;
        }
        let DiagnosticEventKindV1::AnnRegionObservation { observation, .. } = event.kind else {
            return true;
        };
        if observation.phase != DiagnosticAnnRegionPhaseV1::SelectedQuery {
            return true;
        }
        let Ok(mut selected) = self.selected.try_lock() else {
            self.dropped.store(true, Ordering::Relaxed);
            return false;
        };
        if selected.len() == 8 {
            self.dropped.store(true, Ordering::Relaxed);
            return false;
        }
        selected.push(observation);
        true
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SelectedRegion {
    ordinal: u64,
    rows: u32,
    descriptor: [u8; 32],
}

fn selected_ann_regions(
    fixture: &Fixture,
    view: ViewRef,
) -> TestResult<(OwnerKey, f64, CoverageBinding, Vec<SelectedRegion>)> {
    let sink = Arc::new(CarryRegionSink::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([91; 16]),
        Arc::new(CarryTraceClock::default()),
        sink.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Request, DiagnosticLaneV1(1))?;
    let request = RequestBudgetV1::for_duration(Duration::from_secs(120))
        .with_diagnostic_context(context.child_context(&root));
    let result = semantic_hit_with_budget(fixture, view, &mut ann_budget_with_request(request));
    root.finish(DiagnosticTerminalV1::from_result(&result))?;
    let (owner, cosine, coverage, traversal, exhaustive) = result?;
    assert!(matches!(
        traversal,
        NativeSemanticTraversal::Approximate { .. }
    ));
    assert!(!exhaustive);
    assert!(!sink.dropped.load(Ordering::Relaxed));
    let mut selected = Vec::new();
    for observation in sink
        .selected
        .lock()
        .map_err(|_| "ANN carry trace sink poisoned")?
        .iter()
    {
        let descriptor = observation
            .descriptor_ref
            .ok_or("selected ANN region did not report its descriptor")?;
        selected.push(SelectedRegion {
            ordinal: observation.ordinal,
            rows: observation.rows,
            descriptor,
        });
    }
    selected.sort_unstable_by_key(|region| region.ordinal);
    Ok((owner, cosine, coverage, selected))
}

#[test]
fn managed_ann_region_reopens_after_owner_replacement_with_old_view_pinned() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?,
    );
    let fixture = fixture_with_ann(root.path())?;
    let configured = configured(&fixture)?;
    let AnnSources {
        view: source_view,
        primary,
        extra_sources,
    } = advance_ann_sources(&fixture, configured)?;
    let sealed_view = materialize_ann_owners(&fixture, runtime.as_ref(), source_view)?;
    let sealed_source = source(&fixture.kernel, sealed_view)?;
    let (winner, cosine, coverage) = sealed_semantic_hit(&fixture, sealed_view)?;
    assert_eq!(winner, owner());
    assert_eq!(cosine.to_bits(), 1.0f64.to_bits());
    assert_eq!(coverage.status, CoverageStatus::Complete);
    assert_eq!(coverage.covered, Some(coverage.desired));
    assert_eq!(coverage.desired.root, sealed_source.owners);
    assert_ne!(coverage.searchable, coverage.owners.0);
    let old_pin = fixture
        .kernel
        .pin_view(ViewSelection::Exact(sealed_view), &ann_budget())?;
    old_pin.validate()?;

    let (replaced_source, replacement) =
        advance(&fixture, sealed_view, 67, 2, b"beta", Some(primary))?;
    assert_ne!(primary, replacement);
    let replaced_view = materialize_ann_replacement(&fixture, runtime.as_ref(), replaced_source)?;
    let (old_winner, old_score, old_coverage) = sealed_semantic_hit(&fixture, sealed_view)?;
    assert_eq!(old_winner, owner());
    assert_eq!(old_score.to_bits(), 1.0f64.to_bits());
    assert_eq!(old_coverage, coverage);
    old_pin.validate()?;
    let (_, replacement_score, replacement_coverage) = semantic_hit(&fixture, replaced_view)?;
    assert_eq!(replacement_score.to_bits(), 0.0f64.to_bits());
    assert_eq!(replacement_coverage.status, CoverageStatus::Complete);
    assert_ne!(replacement_coverage.owners, coverage.owners);
    let retired_view = delete_extra_ann_sources(&fixture, replaced_view, &extra_sources)?;
    let (retired_winner, retired_score, retired_coverage, retired_traversal, retired_exhaustive) =
        semantic_hit_observed(&fixture, retired_view)?;
    assert_eq!(retired_winner, owner());
    assert_eq!(retired_score.to_bits(), 0.0f64.to_bits());
    assert_eq!(retired_coverage.status, CoverageStatus::Complete);
    assert_eq!(retired_coverage.covered, Some(retired_coverage.desired));
    assert_eq!(retired_traversal, NativeSemanticTraversal::ExactExhausted);
    assert!(retired_exhaustive);
    old_pin.validate()?;
    drop(fixture);

    // Reconstruct the real storage and Catalog handles. Query the persisted
    // Native ANN region while the original pin still protects the old view.
    let reopened = fixture_with_ann(root.path())?;
    old_pin.validate()?;
    assert_eq!(native_pin_count(&reopened.catalog)?, 1);
    let (reopened_winner, reopened_score, reopened_coverage) =
        sealed_semantic_hit(&reopened, sealed_view)?;
    assert_eq!(reopened_winner, owner());
    assert_eq!(reopened_score.to_bits(), 1.0f64.to_bits());
    assert_eq!(reopened_coverage, coverage);
    let (
        reopened_retired_winner,
        reopened_retired_score,
        reopened_retired_coverage,
        reopened_retired_traversal,
        reopened_retired_exhaustive,
    ) = semantic_hit_observed(&reopened, retired_view)?;
    assert_eq!(reopened_retired_winner, owner());
    assert_eq!(reopened_retired_score.to_bits(), 0.0f64.to_bits());
    assert_eq!(reopened_retired_coverage, retired_coverage);
    assert_eq!(
        reopened_retired_traversal,
        NativeSemanticTraversal::ExactExhausted
    );
    assert!(reopened_retired_exhaustive);
    old_pin.release()?;
    assert_eq!(native_pin_count(&reopened.catalog)?, 0);
    Ok(())
}

#[test]
fn managed_ann_second_seal_preserves_prior_selected_view_across_reopen() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?,
    );
    let fixture = fixture_with_ann(root.path())?;
    let configured = configured(&fixture)?;
    let sources = advance_ann_sources_count(&fixture, configured, 128)?;
    assert_eq!(sources.extra_sources.len(), 127);
    // Ready work is ordered by its durable identity, not by source-owner ID.
    // Select the fixed alpha oracle explicitly before the first partial seal.
    let initial = fixture
        .kernel
        .ready_work(workspace(), 1, 128, &mut ann_budget())?;
    assert_eq!(initial.len(), 128);
    let primary = initial
        .iter()
        .find(|work| matches!(work.scope, WorkScope::Owner { owner: candidate, .. } if candidate == owner()))
        .ok_or("ANN primary source has no materialization work")?;
    let primary_view = materialize_ann_work(&fixture, runtime.as_ref(), sources.view, primary, 3)?;
    let first_view =
        materialize_ann_owners_range(&fixture, runtime.as_ref(), primary_view, 4, 66, 64, None)?;
    let first_source = source(&fixture.kernel, first_view)?;
    let (first_winner, first_score, first_coverage) = sealed_semantic_hit(&fixture, first_view)?;
    assert_eq!(first_winner, owner());
    assert_eq!(first_score.to_bits(), 1.0f64.to_bits());
    assert_eq!(first_coverage.desired.root, first_source.owners);
    assert_ne!(first_coverage.searchable, first_coverage.owners.0);
    assert!(matches!(
        first_coverage.status,
        CoverageStatus::Partial {
            missing: quanta_index_core::index::MapRoot(Some(_))
        }
    ));
    let mut old_pin = fixture
        .kernel
        .pin_view(ViewSelection::Exact(first_view), &ann_budget())?;
    old_pin.validate()?;

    let second_view = materialize_ann_owners_range(
        &fixture,
        runtime.as_ref(),
        first_view,
        67,
        130,
        0,
        Some(&mut old_pin),
    )?;
    old_pin.validate()?;
    let (second_winner, second_score, second_coverage) =
        sealed_semantic_hit(&fixture, second_view)?;
    assert_eq!(second_winner, owner());
    assert_eq!(second_score.to_bits(), 1.0f64.to_bits());
    assert_eq!(second_coverage.status, CoverageStatus::Complete);
    assert_eq!(second_coverage.covered, Some(second_coverage.desired));
    assert_eq!(second_coverage.desired, first_coverage.desired);
    assert_ne!(second_coverage.owners, first_coverage.owners);
    assert_ne!(second_coverage.searchable, first_coverage.searchable);
    let (pinned_winner, pinned_score, pinned_coverage) = sealed_semantic_hit(&fixture, first_view)?;
    assert_eq!(pinned_winner, owner());
    assert_eq!(pinned_score.to_bits(), 1.0f64.to_bits());
    assert_eq!(pinned_coverage, first_coverage);
    drop(fixture);

    let reopened = fixture_with_ann(root.path())?;
    let (reopened_first_winner, reopened_first_score, reopened_first_coverage) =
        sealed_semantic_hit(&reopened, first_view)?;
    assert_eq!(reopened_first_winner, owner());
    assert_eq!(reopened_first_score.to_bits(), 1.0f64.to_bits());
    assert_eq!(reopened_first_coverage, first_coverage);
    let (reopened_second_winner, reopened_second_score, reopened_second_coverage) =
        sealed_semantic_hit(&reopened, second_view)?;
    assert_eq!(reopened_second_winner, owner());
    assert_eq!(reopened_second_score.to_bits(), 1.0f64.to_bits());
    assert_eq!(reopened_second_coverage, second_coverage);
    old_pin.release()?;
    assert_eq!(native_pin_count(&reopened.catalog)?, 0);
    Ok(())
}

#[test]
fn managed_ann_multi_region_carry_unlinks_both_old_descriptors_across_reopen() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?,
    );
    let fixture = fixture_with_ann(root.path())?;
    let configured = configured(&fixture)?;
    let sources = advance_ann_sources_count(&fixture, configured, 256)?;
    assert_eq!(sources.extra_sources.len(), 255);

    // Select the fixed alpha oracle before the first 64-owner seal. Work is
    // ordered by durable identity rather than by the source owner number.
    let initial = fixture
        .kernel
        .ready_work(workspace(), 1, 256, &mut ann_budget())?;
    assert_eq!(initial.len(), 256);
    let primary = initial
        .iter()
        .find(|work| matches!(work.scope, WorkScope::Owner { owner: candidate, .. } if candidate == owner()))
        .ok_or("ANN primary source has no materialization work")?;
    let primary_view = materialize_ann_work(&fixture, runtime.as_ref(), sources.view, primary, 3)?;
    let first_view =
        materialize_ann_owners_range(&fixture, runtime.as_ref(), primary_view, 4, 66, 64, None)?;
    let second_view =
        materialize_ann_owners_range(&fixture, runtime.as_ref(), first_view, 67, 130, 64, None)?;
    let third_view =
        materialize_ann_owners_range(&fixture, runtime.as_ref(), second_view, 131, 194, 64, None)?;
    let (old_winner, old_score, old_coverage, old_regions) =
        selected_ann_regions(&fixture, third_view)?;
    assert_eq!(old_winner, owner());
    assert_eq!(old_score.to_bits(), 1.0f64.to_bits());
    assert!(matches!(
        old_coverage.status,
        CoverageStatus::Partial { .. }
    ));
    assert_ne!(old_coverage.covered, Some(old_coverage.desired));
    assert_eq!(
        old_coverage.desired.root,
        source(&fixture.kernel, sources.view)?.owners
    );
    let [old_larger, old_smaller] = old_regions.as_slice() else {
        return Err("third seal did not retain two selected ANN regions".into());
    };
    assert_eq!(
        [
            (old_larger.ordinal, old_larger.rows),
            (old_smaller.ordinal, old_smaller.rows)
        ],
        [(1, 128), (2, 64)]
    );
    assert_ne!(old_larger.descriptor, old_smaller.descriptor);

    let mut old_pin = fixture
        .kernel
        .pin_view(ViewSelection::Exact(third_view), &ann_budget())?;
    old_pin.validate()?;
    let final_view = materialize_ann_owners_range(
        &fixture,
        runtime.as_ref(),
        third_view,
        195,
        258,
        0,
        Some(&mut old_pin),
    )?;
    old_pin.validate()?;
    let (new_winner, new_score, new_coverage, new_regions) =
        selected_ann_regions(&fixture, final_view)?;
    assert_eq!(new_winner, owner());
    assert_eq!(new_score.to_bits(), 1.0f64.to_bits());
    assert_eq!(new_coverage.status, CoverageStatus::Complete);
    assert_eq!(new_coverage.covered, Some(new_coverage.desired));
    assert_eq!(new_coverage.desired, old_coverage.desired);
    let [new_region] = new_regions.as_slice() else {
        return Err("final carry did not select exactly one ANN region".into());
    };
    assert_eq!((new_region.ordinal, new_region.rows), (3, 256));
    assert_ne!(old_larger.descriptor, new_region.descriptor);
    assert_ne!(old_smaller.descriptor, new_region.descriptor);

    let (held_winner, held_score, held_coverage, held_regions) =
        selected_ann_regions(&fixture, third_view)?;
    assert_eq!(held_winner, old_winner);
    assert_eq!(held_score.to_bits(), old_score.to_bits());
    assert_eq!(held_coverage, old_coverage);
    assert_eq!(held_regions, old_regions);
    drop(fixture);

    let reopened = fixture_with_ann(root.path())?;
    old_pin.validate()?;
    assert_eq!(native_pin_count(&reopened.catalog)?, 1);
    let (reopened_old_winner, reopened_old_score, reopened_old_coverage, reopened_old_regions) =
        selected_ann_regions(&reopened, third_view)?;
    assert_eq!(reopened_old_winner, old_winner);
    assert_eq!(reopened_old_score.to_bits(), old_score.to_bits());
    assert_eq!(reopened_old_coverage, old_coverage);
    assert_eq!(reopened_old_regions, old_regions);
    let (reopened_new_winner, reopened_new_score, reopened_new_coverage, reopened_new_regions) =
        selected_ann_regions(&reopened, final_view)?;
    assert_eq!(reopened_new_winner, new_winner);
    assert_eq!(reopened_new_score.to_bits(), new_score.to_bits());
    assert_eq!(reopened_new_coverage, new_coverage);
    assert_eq!(reopened_new_regions, new_regions);
    old_pin.release()?;
    assert_eq!(native_pin_count(&reopened.catalog)?, 0);
    Ok(())
}

fn materialize_ann_replacement(
    fixture: &Fixture,
    runtime: &tokio::runtime::Runtime,
    view: ViewRef,
) -> TestResult<ViewRef> {
    let ready = fixture
        .kernel
        .ready_work(workspace(), 1, 8, &mut ann_budget())?;
    let [work] = ready.as_slice() else {
        return Err("expected one ANN replacement work item".into());
    };
    let claim = fixture
        .kernel
        .claim_work(work, Duration::from_secs(120), &ann_budget())?;
    published(ann_block_on(
        runtime,
        fixture.kernel.submit(
            UpdateRequest {
                operation: operation(68),
                expected_view: view,
                kind: UpdateKind::Materialize { claim },
            },
            &mut ann_budget(),
        ),
    )?)
}
