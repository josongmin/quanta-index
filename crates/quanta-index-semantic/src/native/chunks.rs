//! Deterministic, owner-local UTF-8 chunk membership for native semantics.
//!
//! This is a file-local policy. A dependency-derived chunk needs its own
//! producer certificate and cannot be inferred from these source bytes.

use quanta_index_core::{
    RequestBudgetV1,
    index::{Digest, IndexError, MemoryLease, MemoryLifetime, WorkBudget, WorkUsage},
};
use sha2::{Digest as _, Sha256};

const POLICY_DOMAIN: &[u8] = b"quanta-index-native-semantic-utf8-chunks-v1\0";
const MEMBER_ID_DOMAIN: &[u8] = b"quanta-index-native-semantic-chunk-id-v1\0";
const MAX_SOURCE_BYTES: usize = 16_777_216;
const MAX_OWNER_MEMBERS: u32 = 4096;

/// A largest-scalar-boundary, non-overlapping UTF-8 partition.
/// The byte limit changes output identity and is part of the policy digest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Utf8ChunkPolicy {
    pub max_utf8_bytes: u32,
}
quanta_index_contract::native_serde_struct!(Utf8ChunkPolicy {
    max_utf8_bytes: u32
});

impl Utf8ChunkPolicy {
    pub fn validate(self) -> Result<(), IndexError> {
        if self.max_utf8_bytes < 4 || self.max_utf8_bytes > 16_777_216 {
            return Err(IndexError::InvalidInput(
                "UTF-8 chunk size must be between 4 bytes and 16 MiB".into(),
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn output_policy(self) -> Digest {
        let mut hash = Sha256::new();
        hash.update(POLICY_DOMAIN);
        hash.update(self.max_utf8_bytes.to_be_bytes());
        Digest(hash.finalize().into())
    }
}

/// Coordinate within a selected payload or certified rendered-member manifest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChunkLocation {
    PayloadBytes { start: u64, end: u64 },
    RenderedMember { ordinal: u32 },
}
quanta_index_contract::native_serde_enum!(ChunkLocation {
    units {}
    newtypes {}
    records {
        PayloadBytes = 0 { start: u64, end: u64 },
        RenderedMember = 1 { ordinal: u32 }
    }
});

/// One ordered member of a selected UTF-8 payload. `id` names the logical
/// vector, independently of ANN row offsets and of a unique member's span.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChunkMember {
    pub id: Digest,
    pub location: ChunkLocation,
    pub input_digest: Digest,
}
quanta_index_contract::native_serde_struct!(ChunkMember {
    id: Digest,
    location: ChunkLocation,
    input_digest: Digest,
});

impl ChunkMember {
    pub fn payload_span(&self) -> Result<(u64, u64), IndexError> {
        match self.location {
            ChunkLocation::PayloadBytes { start, end } => Ok((start, end)),
            ChunkLocation::RenderedMember { .. } => Err(IndexError::InvalidInput(
                "rendered member has no payload byte span".into(),
            )),
        }
    }
}

/// Owns the admitted member backing. The data field precedes its lease so
/// dropping a plan releases data before returning retained capacity.
#[derive(Debug)]
pub(crate) struct ChunkPlan {
    pub members: Vec<ChunkMember>,
    pub memory: Option<MemoryLease>,
}

fn checkpoint(budget: &RequestBudgetV1) -> Result<(), IndexError> {
    if budget.is_cancelled() {
        Err(IndexError::Cancelled)
    } else if std::time::Instant::now() >= budget.deadline() {
        Err(IndexError::ResourceLimited)
    } else {
        Ok(())
    }
}

fn visit_spans(
    text: &str,
    max_bytes: usize,
    max_vectors: u32,
    budget: &RequestBudgetV1,
    mut visit: impl FnMut(usize, usize) -> Result<(), IndexError>,
) -> Result<u32, IndexError> {
    let mut count = 0u32;
    let mut start = 0usize;
    let mut end = 0usize;
    let mut seen_scalars = 0usize;
    if text.is_empty() {
        if max_vectors == 0 {
            return Err(IndexError::ResourceLimited);
        }
        checkpoint(budget)?;
        visit(0, 0)?;
        return Ok(1);
    }
    for (offset, scalar) in text.char_indices() {
        if seen_scalars.trailing_zeros() >= 10 {
            checkpoint(budget)?;
        }
        seen_scalars = seen_scalars
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let next = offset
            .checked_add(scalar.len_utf8())
            .ok_or(IndexError::ResourceLimited)?;
        if next.checked_sub(start).ok_or(IndexError::ResourceLimited)? > max_bytes {
            count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            if count > max_vectors {
                return Err(IndexError::ResourceLimited);
            }
            visit(start, end)?;
            start = offset;
        }
        end = next;
    }
    count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    if count > max_vectors {
        return Err(IndexError::ResourceLimited);
    }
    checkpoint(budget)?;
    visit(start, end)?;
    Ok(count)
}

/// Count members before any member backing is allocated. The same limit and
/// cancellation checks apply to counting and to later construction.
pub(crate) fn count_chunks(
    payload: &[u8],
    policy: Utf8ChunkPolicy,
    max_vectors: u32,
    budget: &RequestBudgetV1,
) -> Result<usize, IndexError> {
    policy.validate()?;
    if payload.len() > MAX_SOURCE_BYTES {
        return Err(IndexError::ResourceLimited);
    }
    checkpoint(budget)?;
    let text = std::str::from_utf8(payload)
        .map_err(|_error| IndexError::InvalidInput("semantic chunk input is not UTF-8".into()))?;
    let admitted_limit = max_vectors.min(MAX_OWNER_MEMBERS);
    let count = visit_spans(
        text,
        usize::try_from(policy.max_utf8_bytes).map_err(|_error| IndexError::ResourceLimited)?,
        admitted_limit,
        budget,
        |_start, _end| Ok(()),
    )?;
    usize::try_from(count).map_err(|_error| IndexError::ResourceLimited)
}

/// Plan an owner with admitted member backing.
///
/// Count without allocating, admit retained backing, then allocate.
/// Duplicate chunks deliberately depend on
/// the whole selected payload and occurrence number: identical text provides
/// no stable authority to match duplicates across arbitrary insertions.
pub(crate) fn plan_chunks(
    payload: &[u8],
    policy: Utf8ChunkPolicy,
    max_vectors: u32,
    budget: &RequestBudgetV1,
) -> Result<ChunkPlan, IndexError> {
    let count = count_chunks(payload, policy, max_vectors, budget)?;
    plan_chunks_impl(
        payload,
        policy,
        max_vectors,
        budget,
        count,
        |bytes, lifetime| {
            budget
                .reserve_provider_memory_v1(bytes, lifetime)
                .map_err(|error| {
                    checkpoint(budget).err().unwrap_or_else(|| {
                        IndexError::Storage(format!("chunk memory admission: {error}"))
                    })
                })
        },
    )
}

/// Use the same planner under the original owner work and physical memory
/// authority. Parsing and the bounded sort charge precede any allocation.
pub(crate) fn plan_chunks_within_work(
    payload: &[u8],
    policy: Utf8ChunkPolicy,
    max_vectors: u32,
    budget: &mut WorkBudget,
) -> Result<ChunkPlan, IndexError> {
    budget.charge(WorkUsage {
        bytes: u64::try_from(payload.len()).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let count = count_chunks(payload, policy, max_vectors, budget.request())?;
    let comparison_levels = usize::BITS
        .checked_sub(
            count
                .checked_sub(1)
                .ok_or(IndexError::ResourceLimited)?
                .leading_zeros(),
        )
        .ok_or(IndexError::ResourceLimited)?;
    let comparisons = 32u64
        .checked_mul(u64::try_from(count).map_err(|_error| IndexError::ResourceLimited)?)
        .and_then(|total| total.checked_mul(u64::from(comparison_levels).checked_add(1)?))
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes: comparisons,
        mutations: u64::try_from(count).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let request = budget.request().clone();
    plan_chunks_impl(
        payload,
        policy,
        max_vectors,
        &request,
        count,
        |bytes, lifetime| budget.reserve_memory(bytes, lifetime),
    )
}

fn plan_chunks_impl(
    payload: &[u8],
    policy: Utf8ChunkPolicy,
    max_vectors: u32,
    budget: &RequestBudgetV1,
    count: usize,
    mut reserve: impl FnMut(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<ChunkPlan, IndexError> {
    let text = std::str::from_utf8(payload)
        .map_err(|_error| IndexError::InvalidInput("semantic chunk input is not UTF-8".into()))?;
    let admitted_limit = max_vectors.min(MAX_OWNER_MEMBERS);
    let backing = count
        .checked_mul(std::mem::size_of::<ChunkMember>())
        .ok_or(IndexError::ResourceLimited)?;
    let backing = u64::try_from(backing).map_err(|_error| IndexError::ResourceLimited)?;
    let memory = reserve(backing, MemoryLifetime::Retained)?;
    checkpoint(budget)?;
    let mut members = Vec::new();
    members
        .try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let filled = visit_spans(
        text,
        usize::try_from(policy.max_utf8_bytes).map_err(|_error| IndexError::ResourceLimited)?,
        admitted_limit,
        budget,
        |start, end| {
            members.push(ChunkMember {
                id: Digest([0; 32]),
                location: ChunkLocation::PayloadBytes {
                    start: u64::try_from(start).map_err(|_error| IndexError::ResourceLimited)?,
                    end: u64::try_from(end).map_err(|_error| IndexError::ResourceLimited)?,
                },
                input_digest: Digest(
                    Sha256::digest(payload.get(start..end).ok_or_else(|| {
                        IndexError::BackendViolation("chunk span escaped selected payload".into())
                    })?)
                    .into(),
                ),
            });
            Ok(())
        },
    )?;
    if members.len() != count
        || usize::try_from(filled).map_err(|_error| IndexError::ResourceLimited)? != count
    {
        return Err(IndexError::BackendViolation(
            "chunk partition count changed between admission and fill".into(),
        ));
    }

    let index_backing = count
        .checked_mul(std::mem::size_of::<(Digest, usize)>())
        .ok_or(IndexError::ResourceLimited)?;
    let index_backing =
        u64::try_from(index_backing).map_err(|_error| IndexError::ResourceLimited)?;
    let _index_memory = reserve(index_backing, MemoryLifetime::Temporary)?;
    let mut sorted = Vec::new();
    sorted
        .try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    sorted.extend(
        members
            .iter()
            .enumerate()
            .map(|(index, member)| (member.input_digest, index)),
    );
    sorted.sort_unstable();
    checkpoint(budget)?;
    let policy_digest = policy.output_policy();
    let payload_digest: Digest = Digest(Sha256::digest(payload).into());
    let mut group_start = 0usize;
    while group_start < sorted.len() {
        checkpoint(budget)?;
        let (_, first_index) = *sorted.get(group_start).ok_or_else(|| {
            IndexError::BackendViolation("chunk group start escaped admitted index".into())
        })?;
        let first = *members.get(first_index).ok_or_else(|| {
            IndexError::BackendViolation("chunk group index escaped admitted members".into())
        })?;
        let mut group_end = group_start
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        while sorted
            .get(group_end)
            .is_some_and(|(digest, _index)| *digest == first.input_digest)
        {
            group_end = group_end
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
        }
        let first_start = usize::try_from(first.payload_span()?.0)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let first_end = usize::try_from(first.payload_span()?.1)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let first_bytes = payload.get(first_start..first_end).ok_or_else(|| {
            IndexError::BackendViolation("chunk group span escaped selected payload".into())
        })?;
        let group_len = group_end
            .checked_sub(group_start)
            .ok_or(IndexError::ResourceLimited)?;
        for (ordinal, sorted_index) in (group_start..group_end).enumerate() {
            let (_, index) = *sorted.get(sorted_index).ok_or_else(|| {
                IndexError::BackendViolation("chunk group position escaped admitted index".into())
            })?;
            let member = *members.get(index).ok_or_else(|| {
                IndexError::BackendViolation("chunk group index escaped admitted members".into())
            })?;
            let start = usize::try_from(member.payload_span()?.0)
                .map_err(|_error| IndexError::ResourceLimited)?;
            let end = usize::try_from(member.payload_span()?.1)
                .map_err(|_error| IndexError::ResourceLimited)?;
            if payload.get(start..end) != Some(first_bytes) {
                return Err(IndexError::BackendViolation(
                    "different UTF-8 chunks share a content digest".into(),
                ));
            }
            let mut hash = Sha256::new();
            hash.update(MEMBER_ID_DOMAIN);
            hash.update(policy_digest.0);
            hash.update(member.input_digest.0);
            if group_len == 1 {
                hash.update([0]);
            } else {
                hash.update([1]);
                hash.update(payload_digest.0);
                hash.update(
                    u32::try_from(ordinal)
                        .map_err(|_error| IndexError::ResourceLimited)?
                        .to_be_bytes(),
                );
            }
            members
                .get_mut(index)
                .ok_or_else(|| {
                    IndexError::BackendViolation(
                        "chunk group index escaped admitted members".into(),
                    )
                })?
                .id = Digest(hash.finalize().into());
        }
        group_start = group_end;
    }
    checkpoint(budget)?;
    Ok(ChunkPlan { members, memory })
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::index::{MemoryLeaseOwner, MemoryScope, MemoryScopePort, WorkLimits};
    use std::{
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::Duration,
    };

    struct Released(Arc<AtomicU64>);

    impl MemoryLeaseOwner for Released {
        fn release(&self, bytes: u64) {
            let _previous = self.0.fetch_add(bytes, Ordering::SeqCst);
        }
    }

    struct DenySecondReservation {
        calls: Arc<AtomicU64>,
        released: Arc<AtomicU64>,
    }

    impl MemoryScopePort for DenySecondReservation {
        fn reserve(
            &self,
            bytes: u64,
            _lifetime: MemoryLifetime,
        ) -> Result<MemoryLease, IndexError> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 1 {
                return Err(IndexError::ResourceLimited);
            }
            Ok(MemoryLease::new(
                bytes,
                Arc::new(Released(Arc::clone(&self.released))),
            ))
        }
    }

    fn budget() -> RequestBudgetV1 {
        RequestBudgetV1::for_duration(Duration::from_secs(30))
    }

    fn hex(digest: Digest) -> Result<String, IndexError> {
        use std::fmt::Write as _;
        let mut output = String::with_capacity(64);
        for byte in digest.0 {
            write!(&mut output, "{byte:02x}").map_err(|error| {
                IndexError::BackendViolation(format!("fixture hex formatting: {error}"))
            })?;
        }
        Ok(output)
    }

    fn member(plan: &ChunkPlan, index: usize) -> Result<ChunkMember, IndexError> {
        plan.members
            .get(index)
            .copied()
            .ok_or_else(|| IndexError::BackendViolation("fixture member position is absent".into()))
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            let left = &$left;
            let right = &$right;
            if left != right {
                return Err(IndexError::BackendViolation(format!(
                    "{} != {}; left={left:?}, right={right:?}",
                    stringify!($left),
                    stringify!($right)
                )));
            }
        }};
    }

    macro_rules! check_ne {
        ($left:expr, $right:expr $(,)?) => {{
            let left = &$left;
            let right = &$right;
            if left == right {
                return Err(IndexError::BackendViolation(format!(
                    "{} == {}; value={left:?}",
                    stringify!($left),
                    stringify!($right)
                )));
            }
        }};
    }

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(IndexError::BackendViolation(format!(
                    "assertion failed: {}",
                    stringify!($condition)
                )));
            }
        }};
    }

    #[test]
    fn scalar_boundaries_and_independent_digest_gold() -> Result<(), IndexError> {
        let policy = Utf8ChunkPolicy { max_utf8_bytes: 4 };
        check_eq!(
            hex(policy.output_policy())?,
            "299358f73921c4016b3acb9715356b0e4e73b5d369192da2032ed67010400934"
        );
        let plan = plan_chunks("Aé\n🙂B".as_bytes(), policy, 3, &budget())?;
        let actual: Vec<_> = plan
            .members
            .iter()
            .map(|member| {
                Ok((
                    member.payload_span()?,
                    hex(member.input_digest)?,
                    hex(member.id)?,
                ))
            })
            .collect::<Result<Vec<_>, IndexError>>()?;
        let expected: Vec<((u64, u64), String, String)> = [
            (
                (0, 4),
                "6da52b0aeb87c36b8b897f991a0c21570222565e3ac7f7708f8204353f1bb91f",
                "9213d9f4b6b52d840364898d2e7b32a97b966d91519028b9150cb8d460dd3fe7",
            ),
            (
                (4, 8),
                "d06f1525f791397809f9bc98682b5c13318eca4c3123433467fd4dffda44fd14",
                "717b86677c251e81a63c741a7028c84cc94e0fa47ad8dae18e61baa3a0ce9a25",
            ),
            (
                (8, 9),
                "df7e70e5021544f4834bbee64a9e3789febc4be81470df629cad6ddb03320a5c",
                "a545f6ce5a551fbe1e8440e4df186f27863492d22ed8cf890fc6cde3cb672e56",
            ),
        ]
        .into_iter()
        .map(|(span, digest, id)| (span, digest.to_owned(), id.to_owned()))
        .collect();
        check_eq!(actual, expected);
        Ok(())
    }

    #[test]
    fn unique_movement_retains_identity_and_duplicate_insertion_reidentifies_group()
    -> Result<(), IndexError> {
        let policy = Utf8ChunkPolicy { max_utf8_bytes: 4 };
        let before = plan_chunks(b"AAAABBBB", policy, 4, &budget())?;
        let inserted = plan_chunks(b"AAAABBBBAAAA", policy, 4, &budget())?;
        check_eq!(
            hex(member(&before, 0)?.id)?,
            "1f460651ad4283c655f9925112a98476c6546b7347a891c9ce3af85d435dc2f2"
        );
        check_eq!(
            hex(member(&inserted, 0)?.id)?,
            "a28a5877eedbb4a948af57be0b470eca0b1089f9933f9c30ecec7852ad8409ab"
        );
        check_eq!(
            hex(member(&inserted, 2)?.id)?,
            "f117481c26b757552c63ec39bf44543277d8571acb5e2476c026fdd1b3274a4f"
        );
        check_ne!(member(&before, 0)?.id, member(&inserted, 0)?.id);
        check_ne!(member(&inserted, 0)?.id, member(&inserted, 2)?.id);
        check_eq!(member(&before, 1)?.id, member(&inserted, 1)?.id);
        let moved = plan_chunks(b"BBBBAAAA", policy, 4, &budget())?;
        check_eq!(member(&before, 0)?.id, member(&moved, 1)?.id);
        check_eq!(member(&before, 1)?.id, member(&moved, 0)?.id);
        // Earlier insertion into an already ambiguous duplicate class must
        // reidentity every old duplicate; unique BBBB retains identity.
        let duplicated = plan_chunks(b"AAAAAAAABBBBAAAA", policy, 4, &budget())?;
        for old in [member(&inserted, 0)?, member(&inserted, 2)?] {
            for new in [
                member(&duplicated, 0)?,
                member(&duplicated, 1)?,
                member(&duplicated, 3)?,
            ] {
                check_ne!(old.id, new.id);
            }
        }
        check_eq!(member(&inserted, 1)?.id, member(&duplicated, 2)?.id);
        Ok(())
    }

    #[test]
    fn empty_utf8_and_budget_refusals() -> Result<(), IndexError> {
        let policy = Utf8ChunkPolicy { max_utf8_bytes: 4 };
        let empty = plan_chunks(b"", policy, 1, &budget())?;
        check_eq!(
            (
                member(&empty, 0)?.payload_span()?.0,
                member(&empty, 0)?.payload_span()?.1
            ),
            (0, 0)
        );
        check_eq!(
            plan_chunks(b"AAAAB", policy, 1, &budget()).err(),
            Some(IndexError::ResourceLimited)
        );
        check!(matches!(
            plan_chunks(b"\xff", policy, 1, &budget()),
            Err(IndexError::InvalidInput(_))
        ));
        check!(matches!(
            Utf8ChunkPolicy { max_utf8_bytes: 3 }.validate(),
            Err(IndexError::InvalidInput(_))
        ));
        check!(matches!(
            Utf8ChunkPolicy {
                max_utf8_bytes: 16_777_217
            }
            .validate(),
            Err(IndexError::InvalidInput(_))
        ));
        let stopped = budget();
        stopped.cancel_handle().cancel();
        check_eq!(
            plan_chunks(b"AAAA", policy, 1, &stopped).err(),
            Some(IndexError::Cancelled)
        );
        Ok(())
    }

    #[test]
    fn owner_work_refuses_before_memory_and_releases_member_lease_on_index_denial()
    -> Result<(), IndexError> {
        let policy = Utf8ChunkPolicy { max_utf8_bytes: 4 };
        let calls = Arc::new(AtomicU64::new(0));
        let released = Arc::new(AtomicU64::new(0));
        let scope = MemoryScope::new(Arc::new(DenySecondReservation {
            calls: Arc::clone(&calls),
            released: Arc::clone(&released),
        }));
        let mut work = WorkBudget::with_request(
            WorkLimits {
                reads: 0,
                writes: 0,
                bytes: 35,
                mutations: 1,
            },
            budget(),
        );
        work.bind_memory_scope(Some(scope.clone()));
        check_eq!(
            plan_chunks_within_work(b"AAAA", policy, 1, &mut work).err(),
            Some(IndexError::ResourceLimited)
        );
        check_eq!(work.usage().bytes, 4);
        check_eq!(work.usage().mutations, 0);
        check_eq!(calls.load(Ordering::SeqCst), 0);

        let mut admitted = WorkBudget::with_request(
            WorkLimits {
                reads: 0,
                writes: 0,
                bytes: 36,
                mutations: 1,
            },
            budget(),
        );
        admitted.bind_memory_scope(Some(scope));
        check_eq!(
            plan_chunks_within_work(b"AAAA", policy, 1, &mut admitted).err(),
            Some(IndexError::ResourceLimited)
        );
        check_eq!(admitted.usage().bytes, 36);
        check_eq!(admitted.usage().mutations, 1);
        check_eq!(calls.load(Ordering::SeqCst), 2);
        check_eq!(
            released.load(Ordering::SeqCst),
            u64::try_from(std::mem::size_of::<ChunkMember>())
                .map_err(|_error| IndexError::ResourceLimited)?
        );
        Ok(())
    }
}
