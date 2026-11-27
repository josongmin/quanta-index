//! Resolve a retired route's actual projection/instance pair from its
//! physically authenticated old Published profile.

use quanta_index_catalog::NativeBootManifest;
use quanta_index_contract::index::{
    IndexInstanceId, IndexProfile, InputDomain, MAX_RETIRED_PROJECTION_IDENTITIES_V1,
    NativeProtectedRepoMapHeadV1, NativeProtectedSourceRoutesV1, RetiredProjectionIdentityV1,
    RetiredProjectionPolicyErrorV1, RetiredProjectionPolicyViewV1,
    decode_retired_projection_policy_v1, preflight_retired_projection_policy_v1,
};
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, UpdateKind, WorkBudget, WorkScope,
};
use quanta_index_storage::FileObjectReader;
use sha2::{Digest as _, Sha256};

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
        let InputDomain::Projection {
            projection,
            producer,
        } = spec.input_domain
        else {
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

/// Collect the selected old root's physical RepoMap IDs and the identities it
/// had already retired. The returned lease covers both the result and scratch.
pub(crate) fn collect_old_retired_projection_identities_v1(
    manifest: &NativeBootManifest,
    routes: &NativeProtectedSourceRoutesV1,
    reader: &FileObjectReader,
    budget: &mut WorkBudget,
) -> Result<(Vec<RetiredProjectionIdentityV1>, MemoryLease), IndexError> {
    let policy_count = manifest
        .retired_projection_policy
        .map_or(0, |policy| policy.identity_count as usize);
    let maximum = routes
        .repomap_heads
        .len()
        .checked_add(policy_count)
        .ok_or(IndexError::ResourceLimited)?;
    if maximum > MAX_RETIRED_PROJECTION_IDENTITIES_V1 {
        return Err(IndexError::ResourceLimited);
    }
    let identity_bytes = maximum
        .checked_mul(std::mem::size_of::<RetiredProjectionIdentityV1>())
        .ok_or(IndexError::ResourceLimited)?;
    let instance_bytes = maximum
        .checked_add(policy_count)
        .and_then(|count| count.checked_mul(std::mem::size_of::<IndexInstanceId>()))
        .ok_or(IndexError::ResourceLimited)?;
    let granted = u64::try_from(
        identity_bytes
            .checked_add(instance_bytes)
            .and_then(|bytes| bytes.checked_mul(2))
            .and_then(|bytes| bytes.checked_add(128))
            .ok_or(IndexError::ResourceLimited)?,
    )
    .map_err(|_| IndexError::ResourceLimited)?;
    let lease = budget
        .reserve_memory(granted, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    let mut identities = Vec::new();
    identities
        .try_reserve_exact(maximum)
        .map_err(|_| IndexError::ResourceLimited)?;
    for head in &routes.repomap_heads {
        budget.checkpoint()?;
        identities.push(old_repomap_identity_v1(head, reader, budget)?);
    }
    let mut scratch = Vec::new();
    if let Some(policy) = manifest.retired_projection_policy {
        policy
            .validate_v1()
            .map_err(|_| IndexError::DigestConflict)?;
        let stored = reader.get(policy.object, budget)?;
        if stored.registration.reference != policy.object
            || !stored.registration.children.is_empty()
            || preflight_retired_projection_policy_v1(&stored.payload)
                .map_err(|_| IndexError::DigestConflict)?
                != policy_count
            || quanta_index_contract::index::Digest(Sha256::digest(&stored.payload).into())
                != policy.sha256
        {
            return Err(IndexError::DigestConflict);
        }
        scratch
            .try_reserve_exact(policy_count)
            .map_err(|_| IndexError::ResourceLimited)?;
        scratch.resize(policy_count, IndexInstanceId([0; 16]));
        let view = decode_retired_projection_policy_v1(&stored.payload, &mut scratch, &mut || {
            budget.checkpoint()
        })
        .map_err(|error| match error {
            RetiredProjectionPolicyErrorV1::Admission(error) => error,
            _ => IndexError::DigestConflict,
        })?;
        if view.len_v1() != policy_count
            || view.header_v1().candidate_profile_ref != manifest.publication.profile
        {
            return Err(IndexError::DigestConflict);
        }
        let profile = reader.get(manifest.publication.profile, budget)?;
        if profile.registration.reference != manifest.publication.profile
            || !profile.registration.children.is_empty()
        {
            return Err(IndexError::DigestConflict);
        }
        reject_candidate_profile_overlap_v1(&view, &scratch, &profile.payload, budget)?;
        for index in 0..policy_count {
            budget.checkpoint()?;
            identities.push(view.identity_v1(index).ok_or(IndexError::DigestConflict)?);
        }
    }
    let mut instances = Vec::new();
    instances
        .try_reserve_exact(maximum)
        .map_err(|_| IndexError::ResourceLimited)?;
    let actual_bytes = identities
        .capacity()
        .checked_mul(std::mem::size_of::<RetiredProjectionIdentityV1>())
        .and_then(|bytes| {
            bytes.checked_add(
                scratch
                    .capacity()
                    .checked_mul(std::mem::size_of::<IndexInstanceId>())?,
            )
        })
        .and_then(|bytes| {
            bytes.checked_add(
                instances
                    .capacity()
                    .checked_mul(std::mem::size_of::<IndexInstanceId>())?,
            )
        })
        .ok_or(IndexError::ResourceLimited)?;
    if u64::try_from(actual_bytes).map_err(|_| IndexError::ResourceLimited)? > granted {
        return Err(IndexError::ResourceLimited);
    }
    identities.sort_unstable_by_key(|identity| identity.projection.0);
    if identities.windows(2).any(|pair| {
        pair[0].projection == pair[1].projection && pair[0].instance != pair[1].instance
    }) {
        return Err(IndexError::DigestConflict);
    }
    identities.dedup();
    instances.extend(identities.iter().map(|identity| identity.instance));
    instances.sort_unstable_by_key(|instance| instance.0);
    if instances.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(IndexError::DigestConflict);
    }
    Ok((identities, lease))
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
            || sorted_instances
                .binary_search_by_key(&spec.instance.0, |instance| instance.0)
                .is_ok()
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
        let prior = policy
            .identity_v1(middle)
            .ok_or(IndexError::DigestConflict)?;
        if prior.projection.0 < projection.0 {
            lower = middle + 1;
        } else {
            upper = middle;
        }
    }
    Ok(lower < policy.len_v1()
        && policy
            .identity_v1(lower)
            .ok_or(IndexError::DigestConflict)?
            .projection
            == projection)
}

/// Check identities in the retained request as well as the resulting maps.
/// An old predecessor or materialization claim can disappear from the map.
pub(crate) fn reject_retired_published_request_v1(
    policy: &RetiredProjectionPolicyViewV1<'_>,
    sorted_instances: &[IndexInstanceId],
    kind: &UpdateKind,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    match kind {
        UpdateKind::ProjectionAdvance(transition) => {
            if retired_projection_present_v1(policy, transition.projection, budget)? {
                return Err(IndexError::DigestConflict);
            }
            if let Some(previous) = transition.expected
                && retired_projection_present_v1(policy, previous.projection, budget)?
            {
                return Err(IndexError::DigestConflict);
            }
        }
        UpdateKind::CompactSourceLocal { instance, .. } => {
            if sorted_instances
                .binary_search_by_key(&instance.0, |value| value.0)
                .is_ok()
            {
                return Err(IndexError::DigestConflict);
            }
        }
        UpdateKind::Materialize { claim } => {
            if sorted_instances
                .binary_search_by_key(&claim.intent.instance.0, |value| value.0)
                .is_ok()
            {
                return Err(IndexError::DigestConflict);
            }
            if let WorkScope::ProjectionDomain { projection, .. } = claim.intent.scope
                && retired_projection_present_v1(policy, projection, budget)?
            {
                return Err(IndexError::DigestConflict);
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::index::{
        CollectionId, Digest, GroupActivation, IndexKind, InstanceSpec, MapRoot, ObjectRef,
        OwnerMutationSet, ProducerCompleteness, ProducerId, ProjectionBinding, ProjectionId,
        ProjectionTransition, PublicationGroupId, RetiredProjectionPolicyHeaderV1, SchemaId,
        SlotId, SourceCut, StorageFormatId, TenantId, WorkClaim, WorkIntent, WorkspaceKey,
        WorktreeId, decode_retired_projection_policy_v1, encode_retired_projection_policy_v1,
    };
    use quanta_index_core::index::{MemoryLease, MemoryLeaseOwner, MemoryScopePort, WorkLimits};
    use quanta_index_ipc::MemoryScope;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };

    #[derive(Clone, Default)]
    struct TestMemory(Arc<AtomicU64>);

    impl MemoryLeaseOwner for TestMemory {
        fn release(&self, bytes: u64) {
            assert!(self.0.fetch_sub(bytes, Ordering::AcqRel) >= bytes);
        }
    }

    impl MemoryScopePort for TestMemory {
        fn reserve(&self, bytes: u64, _: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            let _previous_live = self
                .0
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                    live.checked_add(bytes).filter(|next| *next <= 1 << 20)
                })
                .map_err(|_| IndexError::ResourceLimited)?;
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
            projection: ProjectionId([7; 16]),
            instance: IndexInstanceId([8; 16]),
        };
        let mut bytes = Vec::new();
        let mut encode_scratch = [IndexInstanceId([0; 16]); 1];
        let _policy_digest = encode_retired_projection_policy_v1(
            header,
            &[old],
            &mut encode_scratch,
            &mut || Ok::<(), ()>(()),
            &mut bytes,
        )
        .unwrap();
        let mut sorted_instances = [IndexInstanceId([0; 16]); 1];
        let policy =
            decode_retired_projection_policy_v1(&bytes, &mut sorted_instances, &mut || {
                Ok::<(), ()>(())
            })
            .unwrap();
        let memory_owner = TestMemory::default();
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 1_000,
                writes: 0,
                bytes: 1 << 20,
                mutations: 0,
            },
            Instant::now() + Duration::from_secs(30),
        );
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(memory_owner.clone()))));
        let independent_graph = InstanceSpec {
            instance: IndexInstanceId([9; 16]),
            kind: IndexKind([10; 16]),
            group: PublicationGroupId([11; 16]),
            activation: GroupActivation::CoreRequired,
            input_domain: InputDomain::Projection {
                projection: ProjectionId([12; 16]),
                producer: ProducerId([13; 16]),
            },
            schema: SchemaId([14; 16]),
            policy: Digest([15; 32]),
            format: StorageFormatId([16; 16]),
        };
        let profile = |spec| {
            serde_json::to_vec(&IndexProfile {
                instances: vec![spec],
            })
            .unwrap()
        };
        assert!(
            reject_candidate_profile_overlap_v1(
                &policy,
                &sorted_instances,
                &profile(independent_graph.clone()),
                &mut budget,
            )
            .is_ok()
        );
        assert!(retired_projection_present_v1(&policy, old.projection, &mut budget).unwrap());
        let mut old_projection = independent_graph.clone();
        old_projection.input_domain = InputDomain::Projection {
            projection: old.projection,
            producer: ProducerId([13; 16]),
        };
        assert!(
            reject_candidate_profile_overlap_v1(
                &policy,
                &sorted_instances,
                &profile(old_projection),
                &mut budget,
            )
            .is_err()
        );
        let mut old_instance = independent_graph;
        old_instance.instance = old.instance;
        assert!(
            reject_candidate_profile_overlap_v1(
                &policy,
                &sorted_instances,
                &profile(old_instance),
                &mut budget,
            )
            .is_err()
        );

        let source = SourceCut {
            owners: MapRoot(None),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        };
        let binding = ProjectionBinding {
            projection: old.projection,
            producer: ProducerId([13; 16]),
            owners: MapRoot(None),
            source,
            schema: SchemaId([14; 16]),
            producer_policy: Digest([15; 32]),
            dependency_context: Digest([16; 32]),
            certificate: ObjectRef(Digest([17; 32])),
        };
        let transition = ProjectionTransition {
            projection: ProjectionId([12; 16]),
            producer: binding.producer,
            expected: Some(binding),
            target_source: source,
            schema: binding.schema,
            producer_policy: binding.producer_policy,
            dependency_context: binding.dependency_context,
            completeness: ProducerCompleteness::Partial {
                missing: MapRoot(None),
            },
            owners: OwnerMutationSet {
                root: MapRoot(None),
                count: 0,
                digest: Digest([18; 32]),
            },
            events: vec![],
        };
        assert!(
            reject_retired_published_request_v1(
                &policy,
                &sorted_instances,
                &UpdateKind::ProjectionAdvance(Box::new(transition.clone())),
                &mut budget,
            )
            .is_err()
        );
        let mut independent = transition;
        independent.expected = None;
        assert!(
            reject_retired_published_request_v1(
                &policy,
                &sorted_instances,
                &UpdateKind::ProjectionAdvance(Box::new(independent)),
                &mut budget,
            )
            .is_ok()
        );
        assert!(
            reject_retired_published_request_v1(
                &policy,
                &sorted_instances,
                &UpdateKind::CompactSourceLocal {
                    instance: old.instance,
                    expected_artifact: ObjectRef(Digest([19; 32])),
                    owner: quanta_index_contract::index::OwnerKey {
                        producer: binding.producer,
                        stable_id: quanta_index_contract::index::StableOwnerId([20; 16]),
                    },
                },
                &mut budget,
            )
            .is_err()
        );
        let intent = WorkIntent {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 1,
            instance: old.instance,
            scope: WorkScope::ProjectionDomain {
                projection: ProjectionId([12; 16]),
                source,
                producer_policy: Digest([21; 32]),
                dependency_context: Digest([22; 32]),
            },
        };
        let claim = |intent| WorkClaim {
            intent,
            fence: 1,
            lease_until_ms: 1,
        };
        assert!(
            reject_retired_published_request_v1(
                &policy,
                &sorted_instances,
                &UpdateKind::Materialize {
                    claim: claim(intent)
                },
                &mut budget,
            )
            .is_err()
        );
        let mut retired_scope = intent;
        retired_scope.instance = IndexInstanceId([9; 16]);
        retired_scope.scope = WorkScope::ProjectionDomain {
            projection: old.projection,
            source,
            producer_policy: Digest([21; 32]),
            dependency_context: Digest([22; 32]),
        };
        assert!(
            reject_retired_published_request_v1(
                &policy,
                &sorted_instances,
                &UpdateKind::Materialize {
                    claim: claim(retired_scope)
                },
                &mut budget,
            )
            .is_err()
        );
        let mut independent_scope = retired_scope;
        independent_scope.scope = intent.scope;
        assert!(
            reject_retired_published_request_v1(
                &policy,
                &sorted_instances,
                &UpdateKind::Materialize {
                    claim: claim(independent_scope)
                },
                &mut budget,
            )
            .is_ok()
        );
        assert_eq!(memory_owner.0.load(Ordering::Acquire), 0);
    }
}
