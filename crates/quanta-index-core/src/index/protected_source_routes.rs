//! One Original-funded decoder for a provisional protected Source route object.
//! Selection and producer authority must be supplied independently by callers.

use quanta_index_contract::{
    RepoId, RevisionId,
    index::{
        MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1, MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1,
        NativeProtectedRepoMapHeadV1, NativeProtectedSourceHeadV1,
    },
};
use serde::{
    Deserialize, Deserializer,
    de::{IgnoredAny, SeqAccess, Visitor},
};

use super::mutations::matches_json;
use super::{
    IndexError, MAX_NATIVE_PROTECTED_SOURCE_ROUTE_BYTES_V1, MemoryLifetime,
    NativeProtectedSourceRoutesV1, ObjectRef, StoredObject, WorkBudget, WorkUsage,
    native_immutable_object_ref_v1,
};

// A supported ID uses at most six JSON bytes per UTF-8 byte; route keys,
// fixed hexadecimal fields and integer values are all shorter.
const MAX_ROUTE_TOKEN_BYTES: usize = 6 * RepoId::MAX_UTF8_BYTES_V1;
const MAX_ROUTE_JSON_DEPTH: usize = 128;
// Slice serde reuses one scratch Vec for tokens and its ignored-value stack.
// This allowance also covers one 512-byte ID's NFC validation, the current
// token's growth overlap and formatting a rejected token. Owned IDs and
// the two head Vecs have separate grants.
const ROUTE_PARSER_BACKING_BYTES: u64 = 128 * 1024;

fn check_route_json_bounds(payload: &[u8], budget: &WorkBudget) -> Result<(), IndexError> {
    let mut quoted = false;
    let mut escaped = false;
    let mut depth = 0usize;
    let mut token = 0usize;
    for chunk in payload.chunks(4096) {
        budget.checkpoint()?;
        for &byte in chunk {
            if quoted {
                if escaped {
                    escaped = false;
                } else if byte == b'"' {
                    quoted = false;
                    token = 0;
                    continue;
                } else if byte == b'\\' {
                    escaped = true;
                }
                token = token.saturating_add(1);
            } else {
                match byte {
                    b'"' => {
                        quoted = true;
                        token = 0;
                    }
                    b'{' | b'[' => {
                        depth = depth.saturating_add(1);
                        token = 0;
                    }
                    b'}' | b']' => {
                        depth = depth.saturating_sub(1);
                        token = 0;
                    }
                    b',' | b':' | b' ' | b'\t' | b'\n' | b'\r' => token = 0,
                    _ => token = token.saturating_add(1),
                }
            }
            if token > MAX_ROUTE_TOKEN_BYTES || depth > MAX_ROUTE_JSON_DEPTH {
                return Err(IndexError::CorruptObject(
                    "protected Source routes: JSON token or nesting exceeds bound".into(),
                ));
            }
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteHeadCounts {
    #[serde(rename = "wire_version")]
    _wire_version: IgnoredAny,
    #[serde(default, rename = "native_origin")]
    _native_origin: Option<IgnoredAny>,
    heads: HeadCount<MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1>,
    repomap_heads: HeadCount<MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1>,
}

struct HeadCount<const MAXIMUM: usize>(usize);

impl<'de, const MAXIMUM: usize> Deserialize<'de> for HeadCount<MAXIMUM> {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct CountVisitor<const MAXIMUM: usize>;
        impl<'de, const MAXIMUM: usize> Visitor<'de> for CountVisitor<MAXIMUM> {
            type Value = HeadCount<MAXIMUM>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, "at most {MAXIMUM} protected route heads")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut count = 0usize;
                while seq.next_element::<IgnoredAny>()?.is_some() {
                    count = count.saturating_add(1);
                    if count > MAXIMUM {
                        return Err(serde::de::Error::invalid_length(count, &self));
                    }
                }
                Ok(HeadCount(count))
            }
        }
        decoder.deserialize_seq(CountVisitor::<MAXIMUM>)
    }
}

fn heads_backing_bytes<T>(count: usize) -> Option<usize> {
    if count == 0 {
        return Some(0);
    }
    // JSON SeqAccess provides no size hint. BoundedVec grows from the Vec
    // minimum and doubles, so admit both the final capacity and the largest
    // possible preceding capacity during its last growth.
    let capacity = count.checked_next_power_of_two()?.max(8);
    let ids = count.checked_mul(RepoId::MAX_UTF8_BYTES_V1 + RevisionId::MAX_UTF8_BYTES_V1)?;
    capacity
        .checked_add(capacity.checked_div(2)?)?
        .checked_mul(std::mem::size_of::<T>())?
        .checked_add(ids)
}

/// Authenticate the route object and drop its owner before the visitor
/// consumes the independently admitted decoded data.
pub fn with_native_protected_source_routes_v1<T>(
    reference: ObjectRef,
    expected_source_count: Option<u32>,
    stored: StoredObject,
    budget: &mut WorkBudget,
    visit: impl FnOnce(&NativeProtectedSourceRoutesV1, &mut WorkBudget) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    if stored.registration.reference != reference
        || stored.payload.is_empty()
        || stored.payload.len() > MAX_NATIVE_PROTECTED_SOURCE_ROUTE_BYTES_V1
    {
        return Err(IndexError::DigestConflict);
    }
    let bytes =
        u64::try_from(stored.payload.len()).map_err(|_width| IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes: bytes.checked_mul(5).ok_or(IndexError::ResourceLimited)?,
        ..Default::default()
    })?;
    check_route_json_bounds(&stored.payload, budget)?;
    let parser = budget
        .reserve_memory(ROUTE_PARSER_BACKING_BYTES, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    let counts: RouteHeadCounts = serde_json::from_slice(&stored.payload)
        .map_err(|error| IndexError::CorruptObject(format!("protected Source routes: {error}")))?;
    budget.checkpoint()?;
    let backing = heads_backing_bytes::<NativeProtectedSourceHeadV1>(counts.heads.0)
        .and_then(|source| {
            heads_backing_bytes::<NativeProtectedRepoMapHeadV1>(counts.repomap_heads.0)
                .and_then(|repomap| source.checked_add(repomap))
        })
        .ok_or(IndexError::ResourceLimited)?;
    let backing = u64::try_from(backing).map_err(|_width| IndexError::ResourceLimited)?;
    let _heads_memory = if backing == 0 {
        None
    } else {
        Some(
            budget
                .reserve_memory(backing, MemoryLifetime::Temporary)?
                .ok_or(IndexError::ResourceLimited)?,
        )
    };
    let routes: NativeProtectedSourceRoutesV1 = serde_json::from_slice(&stored.payload)
        .map_err(|error| IndexError::CorruptObject(format!("protected Source routes: {error}")))?;
    drop(parser);
    {
        let children_backing = routes
            .children_backing_bytes()
            .map_err(|message| IndexError::InvalidInput(message.into()))?;
        let bytes = u64::try_from(children_backing)
            .map_err(|_width| IndexError::ResourceLimited)?
            .checked_add(4096)
            .ok_or(IndexError::ResourceLimited)?;
        let _children_memory = budget
            .reserve_memory(bytes, MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited)?;
        let children = routes
            .children()
            .map_err(|message| IndexError::InvalidInput(message.into()))?;
        if expected_source_count
            .is_some_and(|expected| usize::try_from(expected) != Ok(routes.heads.len()))
            || children != stored.registration.children
            || !matches_json(&routes, &stored.payload, budget, "protected Source routes")?
            || native_immutable_object_ref_v1(&stored.payload, &children) != Some(reference)
        {
            return Err(IndexError::DigestConflict);
        }
    }
    drop(stored);
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
