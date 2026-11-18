//! One Original-funded decoder for a provisional protected Source route object.
//! Selection and producer authority must be supplied independently by callers.

use super::mutations::matches_json;
use super::{
    IndexError, MAX_NATIVE_PROTECTED_SOURCE_ROUTE_BYTES_V1, MemoryLifetime,
    NativeProtectedSourceRoutesV1, ObjectRef, StoredObject, WorkBudget, WorkUsage,
};

pub fn with_native_protected_source_routes_v1<T>(
    reference: ObjectRef,
    expected_source_count: Option<u32>,
    stored: &StoredObject,
    budget: &mut WorkBudget,
    visit: impl FnOnce(&NativeProtectedSourceRoutesV1, &mut WorkBudget) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    if stored.registration.reference != reference
        || stored.payload.is_empty()
        || stored.payload.len() > MAX_NATIVE_PROTECTED_SOURCE_ROUTE_BYTES_V1
    {
        return Err(IndexError::DigestConflict);
    }
    let bytes = u64::try_from(stored.payload.len()).map_err(|_| IndexError::ResourceLimited)?;
    let backing = bytes
        .checked_mul(32)
        .and_then(|n| n.checked_add(65_536))
        .ok_or(IndexError::ResourceLimited)?;
    let _memory = budget
        .reserve_memory(backing, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes: bytes.checked_mul(3).ok_or(IndexError::ResourceLimited)?,
        ..Default::default()
    })?;
    let routes: NativeProtectedSourceRoutesV1 = serde_json::from_slice(&stored.payload)
        .map_err(|error| IndexError::CorruptObject(format!("protected Source routes: {error}")))?;
    let children = routes
        .children()
        .map_err(|message| IndexError::InvalidInput(message.into()))?;
    if expected_source_count
        .is_some_and(|expected| usize::try_from(expected).ok() != Some(routes.heads.len()))
        || children != stored.registration.children
        || !matches_json(&routes, &stored.payload, budget, "protected Source routes")?
        || native_immutable_object_ref_v1(&stored.payload, &children) != Some(reference)
    {
        return Err(IndexError::DigestConflict);
    }
    budget.checkpoint()?;
    visit(&routes, budget)
}

/// Decode one bounded physical Native ledger snapshot under its caller's
/// Original memory loan. This authenticates DATA, not its source provenance.
pub fn with_native_protected_snapshot_v1<T>(
    reference: ObjectRef,
    stored: &StoredObject,
    budget: &mut WorkBudget,
    visit: impl FnOnce(
        &super::NativeProtectedNativeSnapshotV1,
        &mut WorkBudget,
    ) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    if stored.registration.reference != reference
        || stored.payload.is_empty()
        || stored.payload.len() > super::MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1
    {
        return Err(IndexError::DigestConflict);
    }
    let bytes = u64::try_from(stored.payload.len()).map_err(|_| IndexError::ResourceLimited)?;
    let backing = bytes
        .checked_mul(8)
        .and_then(|n| n.checked_add(65_536))
        .ok_or(IndexError::ResourceLimited)?;
    let _memory = budget
        .reserve_memory(backing, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes: bytes.checked_mul(3).ok_or(IndexError::ResourceLimited)?,
        ..Default::default()
    })?;
    let snapshot: super::NativeProtectedNativeSnapshotV1 = serde_json::from_slice(&stored.payload)
        .map_err(|error| {
            IndexError::CorruptObject(format!("protected Native snapshot: {error}"))
        })?;
    snapshot
        .validate_shape_v1()
        .map_err(|message| IndexError::InvalidInput(message.into()))?;
    if snapshot
        .object_ref_v1()
        .map_err(|message| IndexError::InvalidInput(message.into()))?
        != reference
        || snapshot.roots != stored.registration.children
    {
        return Err(IndexError::DigestConflict);
    }
    visit(&snapshot, budget)
}
