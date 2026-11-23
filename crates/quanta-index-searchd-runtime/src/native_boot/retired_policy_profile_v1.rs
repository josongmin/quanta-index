//! Resolve a retired route's actual projection/instance pair from its
//! physically authenticated old Published profile.

use quanta_index_contract::index::{
    IndexInstanceId, IndexProfile, InputDomain, NativeProtectedRepoMapHeadV1, RetiredProjectionIdentityV1,
    RetiredProjectionPolicyViewV1,
};
use quanta_index_core::index::{IndexError, MemoryLifetime, WorkBudget};
use quanta_index_storage::FileObjectReader;

pub(crate) fn old_repomap_identity_v1(
    head: &NativeProtectedRepoMapHeadV1,
    reader: &FileObjectReader,
    budget: &mut WorkBudget,
) -> Result<RetiredProjectionIdentityV1, IndexError> {
    let stored = reader.get(head.publication.profile, budget)?;
    if !stored.registration.children.is_empty() {
        return Err(IndexError::DigestConflict);
    }
    let bytes = u64::try_from(stored.payload.len())
        .map_err(|_| IndexError::ResourceLimited)?
        .checked_mul(32)
        .and_then(|size| size.checked_add(4096))
        .ok_or(IndexError::ResourceLimited)?;
    let _memory = budget
        .reserve_memory(bytes, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    let profile: IndexProfile =
        serde_json::from_slice(&stored.payload).map_err(|_| IndexError::DigestConflict)?;
    if profile.instances.len()
        > quanta_index_contract::native_protocol::MAX_NATIVE_READ_PROFILE_INSTANCES
    {
        return Err(IndexError::DigestConflict);
    }
    let mut identity = None;
    for spec in &profile.instances {
        budget.checkpoint()?;
        if spec.instance != head.instance {
            continue;
        }
        let InputDomain::Projection { projection, producer } = spec.input_domain else {
            return Err(IndexError::DigestConflict);
        };
        if producer != head.operation.key.producer || identity.is_some() {
            return Err(IndexError::DigestConflict);
        }
        identity = Some(RetiredProjectionIdentityV1 {
            projection,
            instance: head.instance,
        });
    }
    identity.ok_or(IndexError::DigestConflict)
}

/// A retained independent Graph, Source or History instance cannot be
/// disabled by a predecessor RepoMap denylist collision.
pub(crate) fn reject_candidate_profile_overlap_v1(
    policy: &RetiredProjectionPolicyViewV1<'_>,
    sorted_instances: &[IndexInstanceId],
    payload: &[u8],
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if sorted_instances.len() != policy.len_v1() {
        return Err(IndexError::DigestConflict);
    }
    let bytes = u64::try_from(payload.len())
        .map_err(|_| IndexError::ResourceLimited)?
        .checked_mul(32)
        .and_then(|size| size.checked_add(4096))
        .ok_or(IndexError::ResourceLimited)?;
    let _memory = budget
        .reserve_memory(bytes, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    let profile: IndexProfile =
        serde_json::from_slice(payload).map_err(|_| IndexError::DigestConflict)?;
    if profile.instances.len()
        > quanta_index_contract::native_protocol::MAX_NATIVE_READ_PROFILE_INSTANCES
    {
        return Err(IndexError::DigestConflict);
    }
    for spec in &profile.instances {
        budget.checkpoint()?;
        let projection = match spec.input_domain {
            InputDomain::Projection { projection, .. } => projection,
            _ => quanta_index_contract::index::ProjectionId([0; 16]),
        };
        if retired_projection_present_v1(policy, projection, budget)?
            || sorted_instances.binary_search_by_key(&spec.instance.0, |instance| instance.0).is_ok()
        {
            return Err(IndexError::DigestConflict);
        }
    }
    Ok(())
}

pub(crate) fn retired_projection_present_v1(
    policy: &RetiredProjectionPolicyViewV1<'_>,
    projection: quanta_index_contract::index::ProjectionId,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    let mut lower = 0;
    let mut upper = policy.len_v1();
    while lower < upper {
        budget.checkpoint()?;
        let middle = lower + (upper - lower) / 2;
        let prior = policy.identity_v1(middle).ok_or(IndexError::DigestConflict)?;
        if prior.projection.0 < projection.0 {
            lower = middle + 1;
        } else {
            upper = middle;
        }
    }
    Ok(lower < policy.len_v1()
        && policy.identity_v1(lower).ok_or(IndexError::DigestConflict)?.projection == projection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::index::{
        Digest, GroupActivation, IndexKind, InstanceSpec, ObjectRef, ProducerId, ProjectionId,
        PublicationGroupId, RetiredProjectionPolicyHeaderV1, SchemaId, StorageFormatId,
        encode_retired_projection_policy_v1, decode_retired_projection_policy_v1,
    };
    use quanta_index_core::index::{MemoryLease, MemoryLeaseOwner, MemoryScopePort, WorkLimits};
    use quanta_index_ipc::MemoryScope;
    use std::{sync::{Arc, atomic::{AtomicU64, Ordering}}, time::{Duration, Instant}};

    #[derive(Clone, Default)]
    struct TestMemory(Arc<AtomicU64>);

    impl MemoryLeaseOwner for TestMemory {
        fn release(&self, bytes: u64) {
            assert!(self.0.fetch_sub(bytes, Ordering::AcqRel) >= bytes);
        }
    }

    impl MemoryScopePort for TestMemory {
        fn reserve(&self, bytes: u64, _: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            self.0.fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                live.checked_add(bytes).filter(|next| *next <= 1 << 20)
            }).map_err(|_| IndexError::ResourceLimited)?;
            Ok(MemoryLease::new(bytes, Arc::new(self.clone())))
        }
    }

    #[test]
    fn retired_policy_rejects_old_id_but_keeps_independent_graph_profile_v1() {
        let header = RetiredProjectionPolicyHeaderV1 {
            format_version: 1,
            old_selection_sha256: Digest([1; 32]),
            old_root_nonce: Digest([2; 32]),
            old_routes_ref: Some(ObjectRef(Digest([3; 32]))),
            old_source_head_count: 1,
            candidate_root_nonce: Digest([4; 32]),
            candidate_config_sha256: Digest([5; 32]),
            candidate_profile_ref: ObjectRef(Digest([6; 32])),
        };
        let old = RetiredProjectionIdentityV1 {
            projection: ProjectionId([7; 16]), instance: IndexInstanceId([8; 16]),
        };
        let mut bytes = Vec::new();
        let mut encode_scratch = [IndexInstanceId([0; 16]); 1];
        encode_retired_projection_policy_v1(
            header, &[old], &mut encode_scratch, &mut || Ok::<(), ()>(()), &mut bytes,
        ).unwrap();
        let mut sorted_instances = [IndexInstanceId([0; 16]); 1];
        let policy = decode_retired_projection_policy_v1(
            &bytes, &mut sorted_instances, &mut || Ok::<(), ()>(()),
        ).unwrap();
        let memory_owner = TestMemory::default();
        let mut budget = WorkBudget::new(
            WorkLimits { reads: 1_000, writes: 0, bytes: 1 << 20, mutations: 0 },
            Instant::now() + Duration::from_secs(30),
        );
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(memory_owner.clone()))));
        let independent_graph = InstanceSpec {
            instance: IndexInstanceId([9; 16]),
            kind: IndexKind([10; 16]),
            group: PublicationGroupId([11; 16]),
            activation: GroupActivation::CoreRequired,
            input_domain: InputDomain::Projection {
                projection: ProjectionId([12; 16]), producer: ProducerId([13; 16]),
            },
            schema: SchemaId([14; 16]),
            policy: Digest([15; 32]),
            format: StorageFormatId([16; 16]),
        };
        let profile = |spec| serde_json::to_vec(&IndexProfile { instances: vec![spec] }).unwrap();
        assert!(reject_candidate_profile_overlap_v1(
            &policy, &sorted_instances, &profile(independent_graph.clone()), &mut budget,
        ).is_ok());
        assert!(retired_projection_present_v1(&policy, old.projection, &mut budget).unwrap());
        let mut old_projection = independent_graph.clone();
        old_projection.input_domain = InputDomain::Projection {
            projection: old.projection, producer: ProducerId([13; 16]),
        };
        assert!(reject_candidate_profile_overlap_v1(
            &policy, &sorted_instances, &profile(old_projection), &mut budget,
        ).is_err());
        let mut old_instance = independent_graph;
        old_instance.instance = old.instance;
        assert!(reject_candidate_profile_overlap_v1(
            &policy, &sorted_instances, &profile(old_instance), &mut budget,
        ).is_err());
        assert_eq!(memory_owner.0.load(Ordering::Acquire), 0);
    }
}
