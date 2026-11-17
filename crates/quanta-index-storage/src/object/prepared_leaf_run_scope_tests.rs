//! Original request identity and cancellation remain mandatory during admission.

use super::*;

#[test]
fn cancellation_during_optional_leaf_admission_is_not_a_fallback_decline()
-> Result<(), Box<dyn std::error::Error>> {
    for cancel_at in [1u64, 7, 518] {
        let fixture = Fixture::with_inputs(PairProbe::default(), false, 0)?;
        let mut work = budget(100_000, 32 * 1024 * 1024);
        let before = work.usage();
        let live = Arc::new(AtomicU64::new(0));
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Admission {
            calls: AtomicU64::new(0),
            live: Arc::clone(&live),
            refuse_at: cancel_at,
            cancel: Some(work.request().cancel_handle()),
        }))));
        let result = fixture
            .store
            .try_prepare_leaf_install_run(attempt()?, 64, 1024, &mut work);
        if result.as_ref().err() != Some(&IndexError::Cancelled)
            || live.load(Ordering::SeqCst) != 0
            || work.usage() != before
            || work.first_denial() != Some(quanta_index_core::index::WorkBudgetDenial::Cancelled)
        {
            return Err("cancelled admission became an optional decline or leaked a grant".into());
        }
        fixture.require_no_effect(&[])?;
    }
    Ok(())
}

#[test]
fn prepared_leaf_same_request_with_changed_deadline_cannot_install()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::with_inputs(PairProbe::default(), false, 0)?;
    let mut work = budget(100_000, 32 * 1024 * 1024);
    let (_admission, live) = funded(&mut work, u64::MAX);
    let mut prepared = fixture
        .store
        .try_prepare_leaf_install_run(attempt()?, 4, 4, &mut work)?
        .ok_or("funded union declined")?;
    prepared.push(b"aaaa", identity(&fixture.store, b"aaaa")?, &mut work)?;
    let earlier = work
        .request()
        .deadline()
        .checked_sub(Duration::from_secs(1))
        .ok_or(IndexError::ResourceLimited)?;
    let request = work.request().capped_at(earlier);
    if !request.same_request_as(work.request()) || request.deadline() == work.request().deadline() {
        return Err("deadline fixture did not preserve cancellation scope".into());
    }
    let mut changed = WorkBudget::with_request(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 32 * 1024 * 1024,
            mutations: 100_000,
        },
        request,
    );
    if !matches!(
        prepared.install(attempt()?, &attempt_sink(), &mut changed),
        Err(IndexError::BackendViolation(_))
    ) || live.load(Ordering::SeqCst) != 0
    {
        return Err("changed deadline crossed installation or retained original grants".into());
    }
    fixture.require_no_effect(&[])?;
    Ok(())
}
