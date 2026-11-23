//! Resolve a retired route's actual projection/instance pair from its
//! physically authenticated old Published profile.

use quanta_index_contract::index::{
    IndexProfile, InputDomain, NativeProtectedRepoMapHeadV1, RetiredProjectionIdentityV1,
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
    payload: &[u8],
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
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
        if policy.rejects_v1(projection, spec.instance, &mut || budget.checkpoint())? {
            return Err(IndexError::DigestConflict);
        }
    }
    Ok(())
}
