//! Selected Lance read interruption through the admitted private object store.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use quanta_index_core::RequestBudgetV1;
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort,
    ObjectRef, WorkBudget, WorkLimits,
};

use super::*;
use crate::native::{
    ann_session::AnnLanceSession,
    ann_store::{AnnStoreLimits, TestReadGate},
};

#[derive(Default)]
struct LiveMemory(AtomicU64);

impl MemoryLeaseOwner for LiveMemory {
    fn release(&self, bytes: u64) {
        let previous = self.0.fetch_sub(bytes, Ordering::SeqCst);
        assert!(previous >= bytes);
    }
}

struct CountedMemory(Arc<LiveMemory>);

impl MemoryScopePort for CountedMemory {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _previous = self
            .0
            .0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                live.checked_add(bytes)
            })
            .map_err(|_live| IndexError::ResourceLimited)?;
        Ok(MemoryLease::new(bytes, self.0.clone()))
    }
}

fn vector(ordinal: u16) -> [f32; 8] {
    let theta = f32::from(ordinal) * std::f32::consts::TAU / 256.0;
    [
        theta.cos(),
        theta.sin(),
        (3.0 * theta).cos() * 0.25,
        (3.0 * theta).sin() * 0.25,
        0.0,
        0.0,
        0.0,
        0.0,
    ]
}

#[derive(Clone, Copy)]
enum Interruption {
    Cancel,
    ExternalCancel,
    Deadline,
}

struct PhysicalFixture {
    work: WorkBudget,
    memory: Arc<LiveMemory>,
    session: AnnLanceSession,
    region: OpenRegion,
    vectors: Vec<[f32; 8]>,
    baseline: u64,
}

async fn physical_fixture(
    rows: usize,
    trained: bool,
    external_cancel: Option<Arc<AtomicBool>>,
) -> Result<PhysicalFixture, Box<dyn std::error::Error>> {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(180))
        .ok_or(IndexError::ResourceLimited)?;
    let limits = WorkLimits {
        reads: 100_000,
        writes: 100_000,
        bytes: 256 * 1024 * 1024,
        mutations: 100_000,
    };
    let request = match external_cancel {
        Some(flag) => RequestBudgetV1::until_with_external_cancellation(deadline, flag),
        None => RequestBudgetV1::until(deadline),
    };
    let mut work = WorkBudget::with_request(limits, request);
    let memory = Arc::new(LiveMemory::default());
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(CountedMemory(Arc::clone(
        &memory,
    ))))));
    let session = AnnLanceSession::new(AnnStoreLimits::default(), &mut work)?;
    let connection = session.connect().await?;
    let recipe = Digest([9; 32]);
    let vectors: Vec<_> = (0..rows)
        .map(|ordinal| u16::try_from(ordinal).map(vector))
        .collect::<Result<_, _>>()?;
    let ids: Vec<_> = (0..rows)
        .map(|ordinal| u16::try_from(ordinal).map(u16::to_be_bytes))
        .collect::<Result<_, _>>()?;
    let region_rows: Vec<_> = ids
        .iter()
        .zip(&vectors)
        .map(|(vector_id, vector)| RegionRow {
            owner: OwnerKey {
                producer: ProducerId([1; 16]),
                stable_id: StableOwnerId([2; 16]),
            },
            member: Digest([7; 32]),
            vector_id,
            input: OwnerInputBinding {
                source: Some(ObjectRef(Digest([3; 32]))),
                projection: None,
                producer_context: Digest([4; 32]),
                derivation_policy: recipe,
            },
            vector,
        })
        .collect();
    let (table, seal) = build_region(
        &connection,
        "physical_query",
        &region_rows,
        recipe,
        1,
        8,
        &mut work,
    )
    .await?;
    if trained != seal.vector_index.ann.is_some() {
        return Err("Lance fixture selected the wrong index branch".into());
    }
    let region = open_region(table, seal, &mut work)
        .await?
        .with_read_store(session.read_store());
    let _files = session.freeze_and_snapshot().await?;
    let baseline = memory.0.load(Ordering::SeqCst);
    Ok(PhysicalFixture {
        work,
        memory,
        session,
        region,
        vectors,
        baseline,
    })
}

async fn interrupted_physical_read(
    rows: usize,
    trained: bool,
    interruption: Interruption,
) -> Result<(), Box<dyn std::error::Error>> {
    let external_cancel = matches!(interruption, Interruption::ExternalCancel)
        .then(|| Arc::new(AtomicBool::new(false)));
    let PhysicalFixture {
        mut work,
        memory,
        session,
        region,
        vectors,
        baseline,
    } = physical_fixture(rows, trained, external_cancel.clone()).await?;
    // Bound only the query, after fixture construction and index training.
    let query_deadline = if matches!(interruption, Interruption::Deadline) {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(5))
            .ok_or(IndexError::ResourceLimited)?;
        work.cap_deadline(deadline);
        Some(deadline)
    } else {
        None
    };
    let (gate, started) = TestReadGate::new();
    session.arm_test_read_gate(Arc::clone(&gate));
    let cancel = work.request().cancel_handle();
    let query_vector = vectors.get(rows / 2).ok_or("query vector missing")?;
    let mut query = Box::pin(query_region(
        &region,
        query_vector,
        1,
        1,
        1,
        &mut work,
        |_candidate, _budget| Ok(Some(RefillProgress::Vector)),
    ));
    tokio::select! {
        started = tokio::time::timeout(Duration::from_secs(10), started) => {
            started.map_err(|_| "Lance query never entered object read")?
                .map_err(|_| "Lance read start signal dropped")?;
        }
        result = &mut query => {
            return Err(format!("Lance query returned before object read: {result:?}").into());
        }
    }
    if gate.active() != 1 {
        gate.release();
        return Err("Lance read gate did not hold a physical read".into());
    }
    let observation_deadline = match interruption {
        Interruption::Cancel => {
            cancel.cancel();
            Instant::now()
                .checked_add(Duration::from_secs(2))
                .ok_or(IndexError::ResourceLimited)?
        }
        Interruption::ExternalCancel => {
            external_cancel
                .ok_or("external cancellation flag missing")?
                .store(true, Ordering::Release);
            Instant::now()
                .checked_add(Duration::from_secs(2))
                .ok_or(IndexError::ResourceLimited)?
        }
        Interruption::Deadline => query_deadline
            .ok_or("query deadline missing")?
            .checked_add(Duration::from_secs(2))
            .ok_or(IndexError::ResourceLimited)?,
    };
    // Keep polling the pending vendor query while the first object read stays parked.
    let early = tokio::time::timeout_at(
        tokio::time::Instant::from_std(observation_deadline),
        &mut query,
    )
    .await;
    let returned_before_release = early.is_ok();
    let active_before_release = gate.active();
    gate.release();
    let response = match early {
        Ok(response) => response,
        Err(_) => query.await,
    };
    let response = session.resolve(response);
    let expected = match interruption {
        Interruption::Cancel | Interruption::ExternalCancel => IndexError::Cancelled,
        Interruption::Deadline => IndexError::ResourceLimited,
    };
    if !matches!(&response, Err(error) if *error == expected) {
        return Err(format!(
            "interrupted Lance query returned {response:?}, expected {expected:?}"
        )
        .into());
    }
    if !returned_before_release || active_before_release != 0 {
        return Err("interrupted Lance query waited for the parked physical read".into());
    }
    if gate.active() != 0 {
        return Err("Lance read remained active after query settlement".into());
    }
    if memory.0.load(Ordering::SeqCst) != baseline {
        return Err("Lance query retained its scratch memory after physical stop".into());
    }
    Ok(())
}

#[test]
fn flat_query_cancel_owns_parked_physical_read() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    crate::run_blocking(
        &runtime,
        interrupted_physical_read(4, false, Interruption::Cancel),
    )
}

#[test]
fn trained_query_cancel_owns_parked_physical_read() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    crate::run_blocking(
        &runtime,
        interrupted_physical_read(256, true, Interruption::Cancel),
    )
}

#[test]
fn flat_query_observes_external_cancel_while_physical_read_is_parked()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    crate::run_blocking(
        &runtime,
        interrupted_physical_read(4, false, Interruption::ExternalCancel),
    )
}

#[test]
fn flat_query_deadline_owns_parked_physical_read() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    crate::run_blocking(
        &runtime,
        interrupted_physical_read(4, false, Interruption::Deadline),
    )
}

#[test]
fn flat_query_preserves_first_candidate_refusal() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    crate::run_blocking(&runtime, async {
        let PhysicalFixture {
            mut work,
            memory,
            session,
            region,
            vectors,
            baseline,
        } = physical_fixture(4, false, None).await?;
        let cancel = work.request().cancel_handle();
        let sentinel = IndexError::Storage("first flat candidate refusal".into());
        let mut callbacks = 0;
        let response = tokio::time::timeout(
            Duration::from_secs(10),
            query_region(
                &region,
                &vectors[2],
                2,
                2,
                2,
                &mut work,
                |_candidate, _budget| {
                    callbacks += 1;
                    cancel.cancel();
                    Err(sentinel.clone())
                },
            ),
        )
        .await
        .map_err(|_| "flat query did not settle after candidate refusal")?;
        let response = session.resolve(response);
        if !matches!(&response, Err(error) if *error == sentinel) {
            return Err(format!(
                "flat candidate refusal returned {response:?}, expected {sentinel:?}"
            )
            .into());
        }
        if callbacks != 1 {
            return Err(format!("flat query invoked {callbacks} callbacks after refusal").into());
        }
        if memory.0.load(Ordering::SeqCst) != baseline {
            return Err("flat query retained scratch memory after candidate refusal".into());
        }
        Ok(())
    })
}

#[test]
fn cancelling_a_parked_read_leaves_the_other_private_lance_session_usable()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    crate::run_blocking(&runtime, async {
        let mut parked = physical_fixture(4, false, None).await?;
        let mut healthy = physical_fixture(7, false, None).await?;
        let (gate, started) = TestReadGate::new();
        parked.session.arm_test_read_gate(Arc::clone(&gate));
        let cancel = parked.work.request().cancel_handle();
        let parked_vector = parked.vectors.first().ok_or("parked query missing")?;
        let query = query_region(
            &parked.region,
            parked_vector,
            1,
            1,
            1,
            &mut parked.work,
            |_candidate, _budget| {
                Err(IndexError::BackendViolation(
                    "parked query admitted a candidate".into(),
                ))
            },
        );
        tokio::pin!(query);
        tokio::select! {
            signal = tokio::time::timeout(Duration::from_secs(5), started) => {
                signal.map_err(|_| "parked store was never read")??;
            }
            _result = &mut query => return Err("parked query returned before cancellation".into()),
        }

        let mut matched = false;
        let healthy_vector = healthy.vectors.last().ok_or("healthy query missing")?;
        let sibling = query_region(
            &healthy.region,
            healthy_vector,
            1,
            1,
            1,
            &mut healthy.work,
            |candidate, _budget| {
                // ID 6 exists only in this private session, despite sharing
                // the logical URI and table name with the parked query.
                matched = candidate.vector_id.as_slice() == 6_u16.to_be_bytes();
                Ok(Some(RefillProgress::Vector))
            },
        );
        cancel.cancel();
        let (cancelled, sibling) = tokio::join!(
            tokio::time::timeout(Duration::from_secs(5), &mut query),
            tokio::time::timeout(Duration::from_secs(5), sibling),
        );
        let reads_before_release = gate.active();
        gate.release();
        if !matches!(cancelled?, Err(IndexError::Cancelled)) || reads_before_release != 0 {
            return Err("cancelled query did not stop its private read".into());
        }
        if sibling??.inspected != 1 || !matched {
            return Err("healthy query did not return its own Lance row".into());
        }
        if parked.memory.0.load(Ordering::SeqCst) != parked.baseline
            || healthy.memory.0.load(Ordering::SeqCst) != healthy.baseline
        {
            return Err("one of the private queries retained scratch memory".into());
        }
        Ok(())
    })
}
