//! Real Catalog/FileObjectStore ANN region custody across selected views.

use super::*;
use quanta_index_contract::native_protocol::NativeSemanticTraversal;
use quanta_index_core::index::{WorkIntent, WorkScope};

#[expect(
    clippy::disallowed_methods,
    reason = "the ANN integration test must poll Lance region creation inside its owned Tokio reactor"
)]
fn ann_block_on<F: std::future::Future>(runtime: &tokio::runtime::Runtime, future: F) -> F::Output {
    runtime.block_on(future)
}

fn ann_budget() -> WorkBudget {
    WorkBudget::with_request(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 1_500_000_000,
            mutations: 1_000_000,
        },
        RequestBudgetV1::for_duration(Duration::from_secs(120)),
    )
}

fn extra_owner(id: u8) -> OwnerKey {
    OwnerKey {
        producer: PRODUCER,
        stable_id: StableOwnerId([id; 16]),
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
    count: u8,
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
    staged.push(Ok(OwnerMutation {
        owner: owner(),
        expected: None,
        next: Some(primary),
    }));
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
        staged.push(Ok(OwnerMutation {
            owner: extra_owner(id),
            expected: None,
            next: Some(source),
        }));
        extra_sources.push((extra_owner(id), source));
    }
    let owners = fixture
        .kernel
        .stage_mutations(ingress, staged, &mut ann_budget())?;
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
    let mut work = ann_budget();
    let mut session = fixture
        .kernel
        .begin_query_session(ViewSelection::Exact(view), &mut work)?;
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

#[test]
fn managed_ann_region_reopens_after_owner_replacement_with_old_view_pinned() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?,
    );
    let fixture = fixture_with_ann(root.path(), Arc::clone(&runtime))?;
    let configured = configured(&fixture)?;
    let AnnSources {
        view: source_view,
        primary,
        extra_sources,
    } = advance_ann_sources(&fixture, configured)?;
    let sealed_view = materialize_ann_owners(&fixture, runtime.as_ref(), source_view)?;
    let sealed_source = source(&fixture.kernel, sealed_view)?;
    let (winner, cosine, coverage) = semantic_hit(&fixture, sealed_view)?;
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
    let (old_winner, old_score, old_coverage) = semantic_hit(&fixture, sealed_view)?;
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

    // Reconstruct the real storage and Catalog handles. The selected query
    // must load the managed file-set chunks and hydrate a new private Lance
    // session; no process-global Lance memory store survives this reopen.
    let reopened = fixture_with_ann(root.path(), runtime)?;
    let (reopened_winner, reopened_score, reopened_coverage) =
        semantic_hit(&reopened, sealed_view)?;
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
    let fixture = fixture_with_ann(root.path(), Arc::clone(&runtime))?;
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

    let reopened = fixture_with_ann(root.path(), runtime)?;
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
