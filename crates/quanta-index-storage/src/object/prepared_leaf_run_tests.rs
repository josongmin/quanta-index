//! Public prepared-run behavior, exact original grants, and decline boundaries.

use super::*;

#[path = "prepared_leaf_run_scope_tests.rs"]
mod scope_tests;

fn attempt() -> Result<AttemptRef, IndexError> {
    let ObjectInstallScope::Attempt(attempt) = attempt_sink().0 else {
        return Err(IndexError::BackendViolation(
            "fixture is not an attempt".into(),
        ));
    };
    Ok(attempt)
}

fn funded(work: &mut WorkBudget, refuse_at: u64) -> (Arc<Admission>, Arc<AtomicU64>) {
    let live = Arc::new(AtomicU64::new(0));
    let admission = Arc::new(Admission {
        calls: AtomicU64::new(0),
        live: Arc::clone(&live),
        refuse_at,
        cancel: None,
    });
    work.bind_memory_scope(Some(MemoryScope::new(admission.clone())));
    (admission, live)
}

fn identity(store: &FileObjectStore, payload: &[u8]) -> Result<ObjectRef, IndexError> {
    store
        .identify(payload, &[], &mut budget(100_000, 32 * 1024 * 1024))
        .map(|registration| registration.reference)
}

struct Ceiling {
    live: Arc<AtomicU64>,
    maximum: u64,
}

impl MemoryScopePort for Ceiling {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _previous = self
            .live
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                live.checked_add(bytes).filter(|next| *next <= self.maximum)
            })
            .map_err(|_live| IndexError::ResourceLimited)?;
        Ok(MemoryLease::new(
            bytes,
            Arc::new(Release(Arc::clone(&self.live))),
        ))
    }
}

#[test]
fn insufficient_union_live_memory_leaves_original_single_packet_fundable()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::with_inputs(PairProbe::default(), false, 0)?;
    let mut work = budget(100_000, 32 * 1024 * 1024);
    let live = Arc::new(AtomicU64::new(0));
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Ceiling {
        live: Arc::clone(&live),
        maximum: 1024,
    }))));
    let prepared =
        fixture
            .store
            .try_prepare_leaf_install_run(attempt()?, 8192, 1024 * 1024, &mut work)?;
    if prepared.is_some() || live.load(Ordering::SeqCst) != 0 || work.first_denial().is_some() {
        return Err("unfunded union changed terminal or live-memory state".into());
    }
    fixture.require_no_effect(&[])?;
    let installed = fixture.store.install_batch(
        &[ObjectBatchInput {
            payload: b"original bounded packet".to_vec(),
            children: vec![],
        }],
        &attempt_sink(),
        &mut work,
    )?;
    if installed.len() != 1 || fixture.observer.snapshot()?.file_sync_calls != 1 {
        return Err("original bounded installer was not fundable after decline".into());
    }
    drop(installed);
    if live.load(Ordering::SeqCst) != 0 || work.first_denial().is_some() {
        return Err("original fallback lost grant settlement".into());
    }
    Ok(())
}

#[test]
fn optional_leaf_admission_declines_without_effect_denial_or_leaked_grants()
-> Result<(), Box<dyn std::error::Error>> {
    for refuse_at in [1, 2, 3, 4, 5, 6, 7, 8, 257, 518] {
        let fixture = Fixture::with_inputs(PairProbe::default(), false, 0)?;
        let mut work = budget(100_000, 32 * 1024 * 1024);
        let (_admission, live) = funded(&mut work, refuse_at);
        let before = work.usage();
        let prepared =
            fixture
                .store
                .try_prepare_leaf_install_run(attempt()?, 64, 1024, &mut work)?;
        if prepared.is_some()
            || live.load(Ordering::SeqCst) != 0
            || work.first_denial().is_some()
            || work.usage() != before
        {
            return Err("optional decline changed work, terminal denial, or live custody".into());
        }
        fixture.require_no_effect(&[])?;
        // The original single-packet implementation still owns its grants.
        let objects = fixture.store.install_batch(
            &[ObjectBatchInput {
                payload: b"original fallback".to_vec(),
                children: vec![],
            }],
            &attempt_sink(),
            &mut work,
        )?;
        if objects.len() != 1 || fixture.observer.snapshot()?.write_calls != 1 {
            return Err("declined run did not leave the original installer usable".into());
        }
        drop(objects);
        if live.load(Ordering::SeqCst) != 0 {
            return Err("fallback leaked a grant".into());
        }
    }
    Ok(())
}

#[test]
fn leaf_union_boundaries_keep_order_three_custody_calls_and_file_syncs()
-> Result<(), Box<dyn std::error::Error>> {
    for first_width in [95usize, 96, 160, 161, 256] {
        let fixture = Fixture::with_inputs(PairProbe::default(), false, 0)?;
        let payloads: Vec<_> = (0..256u16)
            .map(|slot| {
                let length = if usize::from(slot) < first_width {
                    64
                } else {
                    32
                };
                let mut payload = vec![0x5a; length];
                payload
                    .get_mut(..2)
                    .ok_or(IndexError::ResourceLimited)?
                    .copy_from_slice(&slot.to_be_bytes());
                Ok(payload)
            })
            .collect::<Result<_, IndexError>>()?;
        let expected = payloads
            .iter()
            .map(|payload| identity(&fixture.store, payload))
            .collect::<Result<Vec<_>, _>>()?;
        let mut work = budget(100_000, 32 * 1024 * 1024);
        let (admission, live) = funded(&mut work, u64::MAX);
        let mut prepared = fixture
            .store
            .try_prepare_leaf_install_run(
                attempt()?,
                64,
                first_width
                    .checked_mul(64)
                    .ok_or(IndexError::ResourceLimited)?,
                &mut work,
            )?
            .ok_or("funded union declined")?;
        let calls = admission.calls.load(Ordering::SeqCst);
        for (payload, reference) in payloads.iter().zip(&expected) {
            if !prepared.accepts(payload.len()) {
                return Err("bounded union refused early".into());
            }
            prepared.push(payload, *reference, &mut work)?;
        }
        if prepared.accepts(0) || prepared.references() != expected.as_slice() {
            return Err("union exceeded 256 or changed caller order".into());
        }
        for (slot, payload) in payloads.iter().enumerate() {
            prepared.check_payload(slot, payload)?;
        }
        let installed = prepared.install(attempt()?, &attempt_sink(), &mut work)?;
        let calls_after = admission.calls.load(Ordering::SeqCst);
        let observed = fixture.observer.snapshot()?;
        let events = fixture
            .probe
            .events
            .lock()
            .map_err(|_error| "probe poisoned")?;
        let expected_events: &[&str] = if first_width == 256 {
            &["reserve", "confirm"]
        } else {
            &["reserve", "advance", "confirm"]
        };
        if &*installed != expected.as_slice()
            || events.as_slice() != expected_events
            || observed.write_calls != 256
            || observed.file_sync_calls != 256
            || calls != calls_after
            || live.load(Ordering::SeqCst)
                != u64::try_from(
                    256usize
                        .checked_mul(std::mem::size_of::<ObjectRef>())
                        .ok_or(IndexError::ResourceLimited)?,
                )?
        {
            return Err(
                "prepared run changed order, grants, custody boundaries, or file syncs".into(),
            );
        }
        drop(events);
        #[cfg(target_os = "macos")]
        if observed.barrier_calls != if first_width == 256 { 1 } else { 2 } {
            return Err("prepared run changed per-packet durability barriers".into());
        }
        drop(installed);
        if live.load(Ordering::SeqCst) != 0 {
            return Err("returned slots lost their grant release".into());
        }
        let mut read = budget(100_000, 32 * 1024 * 1024);
        for (reference, payload) in expected.iter().zip(&payloads) {
            if fixture.store.get(*reference, &mut read)?.payload.as_slice() != payload.as_slice() {
                return Err("installed object public read differs from fixed input".into());
            }
        }
    }
    Ok(())
}

#[test]
fn prepared_leaf_scope_and_identity_errors_precede_installation()
-> Result<(), Box<dyn std::error::Error>> {
    for failure in 0..4 {
        let fixture = Fixture::with_inputs(PairProbe::default(), false, 0)?;
        let mut work = budget(100_000, 32 * 1024 * 1024);
        let (_admission, live) = funded(&mut work, u64::MAX);
        let mut prepared = fixture
            .store
            .try_prepare_leaf_install_run(attempt()?, 64, 64, &mut work)?
            .ok_or("funded union declined")?;
        let reference = identity(&fixture.store, b"leaf")?;
        let result = match failure {
            0 => prepared.push(b"leaf", ObjectRef(Digest([0xff; 32])), &mut work),
            1 => {
                prepared.push(b"leaf", reference, &mut work)?;
                prepared.push(b"leaf", reference, &mut work)
            }
            2 => {
                prepared.push(b"leaf", reference, &mut work)?;
                let mut foreign = attempt()?;
                foreign.fence = foreign
                    .fence
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                prepared
                    .install(foreign, &attempt_sink(), &mut work)
                    .map(|_| ())
            }
            _ => {
                prepared.push(b"leaf", reference, &mut work)?;
                let mut foreign_budget = budget(100_000, 32 * 1024 * 1024);
                prepared
                    .install(attempt()?, &attempt_sink(), &mut foreign_budget)
                    .map(|_| ())
            }
        };
        if !matches!(
            result,
            Err(IndexError::BackendViolation(_) | IndexError::FenceLost)
        ) {
            return Err("prepared identity/scope mismatch did not refuse".into());
        }
        // The first two error branches still own the handle until this scope
        // ends; file and Catalog effects are nevertheless forbidden.
        fixture.require_no_effect(&[])?;
        let _retained_until_drop = live;
    }
    Ok(())
}

#[test]
fn prepared_leaf_cancel_and_confirmation_failure_keep_synced_prefixes()
-> Result<(), Box<dyn std::error::Error>> {
    for failure in 0..3 {
        let mut work = budget(100_000, 32 * 1024 * 1024);
        let probe = PairProbe {
            cancel_on_advance: (failure == 1).then(|| work.request().cancel_handle()),
            ..PairProbe::default()
        };
        let fixture = Fixture::with_inputs(probe, failure == 2, 0)?;
        let (_admission, live) = funded(&mut work, u64::MAX);
        let first = identity(&fixture.store, b"aaaa")?;
        let second = identity(&fixture.store, b"bbbb")?;
        let mut prepared = fixture
            .store
            .try_prepare_leaf_install_run(attempt()?, 4, 4, &mut work)?
            .ok_or("funded union declined")?;
        prepared.push(b"aaaa", first, &mut work)?;
        prepared.push(b"bbbb", second, &mut work)?;
        if failure == 0 {
            work.request().cancel_handle().cancel();
        }
        let result = prepared.install(attempt()?, &attempt_sink(), &mut work);
        let reached = *[0u64, 1, 2]
            .get(failure)
            .ok_or(IndexError::ResourceLimited)?;
        let expected_error = if failure == 2 {
            IndexError::BackendViolation("injected managed confirmation failure".into())
        } else {
            IndexError::Cancelled
        };
        if result.as_ref().err() != Some(&expected_error)
            || live.load(Ordering::SeqCst) != 0
            || fixture.observer.snapshot()?.file_sync_calls != reached
            || fixture.store.path_for(first).exists() != (reached >= 1)
            || fixture.store.path_for(second).exists() != (reached == 2)
        {
            return Err("failure crossed a packet or lost synced-file custody".into());
        }
    }
    Ok(())
}

#[test]
fn failed_leaf_adoption_cannot_install_an_earlier_valid_prefix()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::with_inputs(PairProbe::default(), false, 0)?;
    let mut work = budget(100_000, 32 * 1024 * 1024);
    let (_admission, live) = funded(&mut work, u64::MAX);
    let mut prepared = fixture
        .store
        .try_prepare_leaf_install_run(attempt()?, 4, 4, &mut work)?
        .ok_or("funded union declined")?;
    prepared.push(b"aaaa", identity(&fixture.store, b"aaaa")?, &mut work)?;
    if !matches!(
        prepared.push(b"bbbb", ObjectRef(Digest([0xff; 32])), &mut work),
        Err(IndexError::BackendViolation(_))
    ) {
        return Err("mismatched later identity was accepted".into());
    }
    if !matches!(
        prepared.install(attempt()?, &attempt_sink(), &mut work),
        Err(IndexError::BackendViolation(_))
    ) || live.load(Ordering::SeqCst) != 0
    {
        return Err("failed adoption installed a valid prefix or leaked custody".into());
    }
    fixture.require_no_effect(&[])?;
    Ok(())
}

#[test]
fn prepared_leaf_short_write_and_fsync_faults_keep_only_the_synced_prefix()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::object::io_observation::PhysicalFault;
    for call in [1u64, 2] {
        for short_write in [false, true] {
            let fixture = Fixture::with_inputs(PairProbe::default(), false, 0)?;
            let fault = if short_write {
                PhysicalFault::ShortWrite(call)
            } else {
                PhysicalFault::FileSync(call)
            };
            fixture.observer.inject_physical_fault(fault)?;
            let mut work = budget(100_000, 32 * 1024 * 1024);
            let (_admission, live) = funded(&mut work, u64::MAX);
            let first = identity(&fixture.store, b"aaaa")?;
            let second = identity(&fixture.store, b"bbbb")?;
            let mut prepared = fixture
                .store
                .try_prepare_leaf_install_run(attempt()?, 4, 4, &mut work)?
                .ok_or("funded union declined")?;
            prepared.push(b"aaaa", first, &mut work)?;
            prepared.push(b"bbbb", second, &mut work)?;
            let result = prepared.install(attempt()?, &attempt_sink(), &mut work);
            let observed = fixture.observer.snapshot()?;
            let expected_events: &[&str] = if call == 1 {
                &["reserve"]
            } else {
                &["reserve", "advance"]
            };
            let events = fixture
                .probe
                .events
                .lock()
                .map_err(|_error| "probe poisoned")?;
            if !matches!(result, Err(IndexError::Storage(_)))
                || live.load(Ordering::SeqCst) != 0
                || events.as_slice() != expected_events
                || observed.failed_writes != u64::from(short_write)
                || observed.failed_file_syncs != u64::from(!short_write)
                || fixture.store.path_for(first).exists() != (call == 2)
                || fixture.store.path_for(second).exists()
            {
                return Err(
                    "physical failure crossed confirmation or lost the synced prefix".into(),
                );
            }
        }
    }
    Ok(())
}
