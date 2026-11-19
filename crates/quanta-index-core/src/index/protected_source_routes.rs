//! One Original-funded decoder for a provisional protected Source route object.
//! Selection and producer authority must be supplied independently by callers.

use quanta_index_contract::{
    RepoId, RevisionId,
    index::{
        MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1, MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1,
        NativeProtectedRepoMapHeadV1, NativeProtectedSourceHeadV1,
        native_immutable_object_ref_with_work_v1,
    },
};
use serde::{
    Deserialize, Deserializer,
    de::{Error as _, IgnoredAny, MapAccess, SeqAccess, Visitor},
};
use std::{cell::RefCell, io};

use super::mutations::matches_json;
#[path = "protected_snapshot_decode.rs"]
mod snapshot_decode;
use self::snapshot_decode::{require, set_field};

use super::{
    IndexError, MAX_NATIVE_PROTECTED_SOURCE_ROUTE_BYTES_V1, MemoryLease, MemoryLifetime,
    NATIVE_PROTECTED_SNAPSHOT_PAGE_BYTES_V1, NativeProtectedSourceRoutesV1, ObjectRef,
    RegisteredObject, StoredObject, WorkBudget, WorkUsage, native_protected_snapshot_paged_len_v1,
};

// A supported ID uses at most six JSON bytes per UTF-8 byte; route keys,
// fixed hexadecimal fields and integer values are all shorter.
const MAX_ROUTE_TOKEN_BYTES: usize = 6 * RepoId::MAX_UTF8_BYTES_V1;
const MAX_ROUTE_JSON_DEPTH: usize = 128;
// Serde reuses one scratch Vec for tokens and its ignored-value stack.
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

struct RouteHeadCounts {
    heads: HeadCount<MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1>,
    repomap_heads: HeadCount<MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1>,
}

impl<'de> Deserialize<'de> for RouteHeadCounts {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        const FIELDS: &[&str] = &["wire_version", "native_origin", "heads", "repomap_heads"];
        struct RouteCountVisitor;

        impl<'de> Visitor<'de> for RouteCountVisitor {
            type Value = RouteHeadCounts;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct RouteHeadCounts")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut wire_version: Option<IgnoredAny> = None;
                let mut native_origin: Option<Option<IgnoredAny>> = None;
                let mut heads = None;
                let mut repomap_heads = None;
                while let Some(field) = map.next_key::<String>()? {
                    match field.as_str() {
                        "wire_version" => {
                            set_field(&mut wire_version, "wire_version", || map.next_value())?
                        }
                        "native_origin" => {
                            set_field(&mut native_origin, "native_origin", || map.next_value())?
                        }
                        "heads" => set_field(&mut heads, "heads", || map.next_value())?,
                        "repomap_heads" => {
                            set_field(&mut repomap_heads, "repomap_heads", || map.next_value())?
                        }
                        other => return Err(A::Error::unknown_field(other, FIELDS)),
                    }
                }
                let _wire_version = require(wire_version, "wire_version")?;
                Ok(RouteHeadCounts {
                    heads: require(heads, "heads")?,
                    repomap_heads: require(repomap_heads, "repomap_heads")?,
                })
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let expecting = "struct RouteHeadCounts with 4 elements";
                let _wire_version = seq
                    .next_element::<IgnoredAny>()?
                    .ok_or_else(|| A::Error::invalid_length(0, &expecting))?;
                let _native_origin = seq.next_element::<Option<IgnoredAny>>()?;
                let heads = seq
                    .next_element()?
                    .ok_or_else(|| A::Error::invalid_length(2, &expecting))?;
                let repomap_heads = seq
                    .next_element()?
                    .ok_or_else(|| A::Error::invalid_length(3, &expecting))?;
                Ok(RouteHeadCounts {
                    heads,
                    repomap_heads,
                })
            }
        }

        decoder.deserialize_struct("RouteHeadCounts", FIELDS, RouteCountVisitor)
    }
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

/// Authenticate a borrowed route payload while returning its owned decoded
/// heads and the loan that must outlive them. The caller retains raw custody.
pub fn decode_native_protected_source_routes_v1(
    reference: ObjectRef,
    expected_source_count: Option<u32>,
    payload: &[u8],
    stored_children: &[ObjectRef],
    budget: &mut WorkBudget,
) -> Result<(NativeProtectedSourceRoutesV1, MemoryLease), IndexError> {
    if payload.is_empty() || payload.len() > MAX_NATIVE_PROTECTED_SOURCE_ROUTE_BYTES_V1 {
        return Err(IndexError::DigestConflict);
    }
    let bytes = u64::try_from(payload.len()).map_err(|_width| IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes: bytes.checked_mul(5).ok_or(IndexError::ResourceLimited)?,
        ..Default::default()
    })?;
    check_route_json_bounds(payload, budget)?;
    let parser = budget
        .reserve_memory(ROUTE_PARSER_BACKING_BYTES, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    let counts: RouteHeadCounts =
        snapshot_decode::deserialize_borrowed_json_v1(payload, budget, "protected Source routes")?;
    budget.checkpoint()?;
    if counts.heads.0 == 0 && counts.repomap_heads.0 == 0 {
        return Err(IndexError::InvalidInput(
            "protected Source route version or count is invalid".into(),
        ));
    }
    let backing = heads_backing_bytes::<NativeProtectedSourceHeadV1>(counts.heads.0)
        .and_then(|source| {
            heads_backing_bytes::<NativeProtectedRepoMapHeadV1>(counts.repomap_heads.0)
                .and_then(|repomap| source.checked_add(repomap))
        })
        .ok_or(IndexError::ResourceLimited)?;
    let backing = u64::try_from(backing).map_err(|_width| IndexError::ResourceLimited)?;
    let heads_memory = budget
        .reserve_memory(backing, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    let routes: NativeProtectedSourceRoutesV1 =
        snapshot_decode::deserialize_borrowed_json_v1(payload, budget, "protected Source routes")?;
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
            .children_with_work_v1(&mut || budget.checkpoint())
            .map_err(|cause| {
                cause.into_error(|message| IndexError::InvalidInput(message.into()))
            })?;
        if expected_source_count
            .is_some_and(|expected| usize::try_from(expected) != Ok(routes.heads.len()))
            || children.as_slice() != stored_children
            || !matches_json(&routes, payload, budget, "protected Source routes")?
            || native_immutable_object_ref_with_work_v1(payload, &children, &mut || {
                budget.checkpoint()
            })? != Some(reference)
        {
            return Err(IndexError::DigestConflict);
        }
    }
    Ok((routes, heads_memory))
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
    if stored.registration.reference != reference {
        return Err(IndexError::DigestConflict);
    }
    let decoded = decode_native_protected_source_routes_v1(
        reference,
        expected_source_count,
        &stored.payload,
        &stored.registration.children,
        budget,
    )?;
    drop(stored);
    budget.checkpoint()?;
    visit(&decoded.0, budget)
}

/// Decode one bounded physical Native ledger snapshot under its caller's
/// Original memory loan. This authenticates DATA, not its source provenance.
pub fn with_native_protected_snapshot_v1<T>(
    reference: ObjectRef,
    mut get: impl FnMut(ObjectRef, &mut WorkBudget) -> Result<StoredObject, IndexError>,
    budget: &mut WorkBudget,
    visit: impl FnOnce(
        &super::NativeProtectedNativeSnapshotV1,
        &mut WorkBudget,
    ) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    let stored = get(reference, budget)?;
    let (first, second) = if let Some(layout) = snapshot_layout_v1(reference, &stored, budget)? {
        let first = get(layout.first, budget)?;
        let second = get(layout.second, budget)?;
        layout.validate_pages(
            &first.registration,
            first.payload.len(),
            &second.registration,
            second.payload.len(),
        )?;
        if native_immutable_object_ref_with_work_v1(
            &first.payload,
            &first.registration.children,
            &mut || budget.checkpoint(),
        )? != Some(layout.first)
            || native_immutable_object_ref_with_work_v1(
                &second.payload,
                &second.registration.children,
                &mut || budget.checkpoint(),
            )? != Some(layout.second)
        {
            return Err(IndexError::DigestConflict);
        }
        drop(stored);
        (first, Some(second))
    } else {
        (stored, None)
    };
    // Retain only the ordered edges while the decoder consumes the pages.
    let registration = first.registration.clone();
    let funded = snapshot_decode::decode_owned(first, second, budget)?;
    verify_decoded_snapshot_v1(reference, &funded.data, &registration.children, budget)?;
    drop(registration);
    budget.checkpoint()?;
    visit(&funded.data, budget)
}

/// Read a provisional immutable page from one held object. The registration
/// and bytes acquire authority only after `finish` checks the exact wire
/// digest and EOF on that same physical input.
pub trait NativeProtectedSnapshotPageV1: std::io::Read {
    fn registration(&self) -> &RegisteredObject;
    fn payload_len(&self) -> usize;
    fn finish(self: Box<Self>) -> Result<(), IndexError>;
}

/// Decode a paged Native snapshot without retaining either physical payload.
/// A successful callback requires both page streams to finish authentication.
pub fn with_native_protected_snapshot_stream_v1<'a, T>(
    reference: ObjectRef,
    get: impl FnOnce(ObjectRef, &mut WorkBudget) -> Result<StoredObject, IndexError>,
    mut open_page: impl FnMut(
        ObjectRef,
        &mut WorkBudget,
    ) -> Result<Box<dyn NativeProtectedSnapshotPageV1 + 'a>, IndexError>,
    budget: &mut WorkBudget,
    visit: impl FnOnce(
        &super::NativeProtectedNativeSnapshotV1,
        &mut WorkBudget,
    ) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    let stored = get(reference, budget)?;
    let (funded, registration) =
        if let Some(layout) = snapshot_layout_v1(reference, &stored, budget)? {
            drop(stored);
            let first = open_page(layout.first, budget)?;
            layout.validate_first(first.registration(), first.payload_len())?;
            let registration = first.registration().clone();
            let work = RefCell::new(&mut *budget);
            let mut pages = SequentialSnapshotPages {
                current: Some(first),
                open_second: Some(|| {
                    let second = open_page(layout.second, &mut **work.borrow_mut())?;
                    layout.validate_second(second.registration(), second.payload_len())?;
                    Ok(second)
                }),
                read_failed: false,
            };
            let funded = match snapshot_decode::decode_streamed(&mut pages, &work) {
                Ok(funded) => funded,
                Err(error)
                    if pages.read_failed
                        || matches!(
                            &error,
                            IndexError::Cancelled
                                | IndexError::ResourceLimited
                                | IndexError::WorkLimitExceeded
                                | IndexError::FenceLost
                                | IndexError::WorkFenceLost
                        ) =>
                {
                    return Err(error);
                }
                Err(error) => {
                    // A parser refusal can precede the physical digest or a
                    // missing later page. Complete both held-file checks before
                    // reporting the provisional JSON refusal.
                    pages.finish_remaining()?;
                    return Err(error);
                }
            };
            if pages.current.is_some() || pages.open_second.is_some() {
                return Err(IndexError::DigestConflict);
            }
            (funded, registration)
        } else {
            let registration = stored.registration.clone();
            (
                snapshot_decode::decode_owned(stored, None, budget)?,
                registration,
            )
        };
    verify_decoded_snapshot_v1(reference, &funded.data, &registration.children, budget)?;
    drop(registration);
    budget.checkpoint()?;
    visit(&funded.data, budget)
}

struct SequentialSnapshotPages<'a, Open> {
    current: Option<Box<dyn NativeProtectedSnapshotPageV1 + 'a>>,
    open_second: Option<Open>,
    read_failed: bool,
}

impl<'a, Open> SequentialSnapshotPages<'a, Open>
where
    Open: FnOnce() -> Result<Box<dyn NativeProtectedSnapshotPageV1 + 'a>, IndexError>,
{
    fn finish_remaining(&mut self) -> Result<(), IndexError> {
        let mut scratch = [0; 4096];
        loop {
            match io::Read::read(self, &mut scratch) {
                Ok(0) => return Ok(()),
                Ok(_) => {}
                Err(error) => {
                    return Err(error
                        .get_ref()
                        .and_then(|cause| cause.downcast_ref::<IndexError>())
                        .cloned()
                        .unwrap_or_else(|| IndexError::Storage(error.to_string())));
                }
            }
        }
    }
}

impl<'a, Open> io::Read for SequentialSnapshotPages<'a, Open>
where
    Open: FnOnce() -> Result<Box<dyn NativeProtectedSnapshotPageV1 + 'a>, IndexError>,
{
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        let result = (|| {
            while let Some(page) = self.current.as_mut() {
                let count = page.read(output)?;
                if count != 0 {
                    return Ok(count);
                }
                let finished = self
                    .current
                    .take()
                    .ok_or_else(|| io::Error::other(IndexError::DigestConflict))?;
                // Release the first authenticated FD before admitting the second.
                finished.finish().map_err(io::Error::other)?;
                if let Some(open) = self.open_second.take() {
                    self.current = Some(open().map_err(io::Error::other)?);
                }
            }
            Ok(0)
        })();
        self.read_failed |= result.is_err();
        result
    }
}

struct SnapshotPageLayout {
    first: ObjectRef,
    second: ObjectRef,
    second_len: usize,
}

impl SnapshotPageLayout {
    fn validate_pages(
        &self,
        first: &RegisteredObject,
        first_len: usize,
        second: &RegisteredObject,
        second_len: usize,
    ) -> Result<(), IndexError> {
        self.validate_first(first, first_len)?;
        self.validate_second(second, second_len)
    }

    fn validate_first(&self, first: &RegisteredObject, first_len: usize) -> Result<(), IndexError> {
        if first.reference != self.first || first_len != NATIVE_PROTECTED_SNAPSHOT_PAGE_BYTES_V1 {
            return Err(IndexError::DigestConflict);
        }
        Ok(())
    }

    fn validate_second(
        &self,
        second: &RegisteredObject,
        second_len: usize,
    ) -> Result<(), IndexError> {
        if second.reference != self.second
            || second_len != self.second_len
            || !second.children.is_empty()
        {
            return Err(IndexError::DigestConflict);
        }
        Ok(())
    }
}

fn snapshot_layout_v1(
    reference: ObjectRef,
    stored: &StoredObject,
    budget: &mut WorkBudget,
) -> Result<Option<SnapshotPageLayout>, IndexError> {
    if stored.registration.reference != reference || stored.payload.is_empty() {
        return Err(IndexError::DigestConflict);
    }
    let paged_len = native_protected_snapshot_paged_len_v1(&stored.payload)
        .map_err(|_| IndexError::DigestConflict)?;
    let total_len = match paged_len {
        Some(total) => total,
        None if stored.payload.len() <= NATIVE_PROTECTED_SNAPSHOT_PAGE_BYTES_V1 => {
            stored.payload.len()
        }
        None => return Err(IndexError::DigestConflict),
    };
    if total_len == 0 || total_len > super::MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1 {
        return Err(IndexError::DigestConflict);
    }
    charge_snapshot_decode_v1(total_len, budget)?;
    if native_immutable_object_ref_with_work_v1(
        &stored.payload,
        &stored.registration.children,
        &mut || budget.checkpoint(),
    )? != Some(reference)
    {
        return Err(IndexError::DigestConflict);
    }

    if let Some(total) = paged_len {
        let [first_ref, second_ref] = stored.registration.children.as_slice() else {
            return Err(IndexError::DigestConflict);
        };
        if first_ref == second_ref {
            return Err(IndexError::DigestConflict);
        }
        Ok(Some(SnapshotPageLayout {
            first: *first_ref,
            second: *second_ref,
            second_len: total
                .checked_sub(NATIVE_PROTECTED_SNAPSHOT_PAGE_BYTES_V1)
                .ok_or(IndexError::DigestConflict)?,
        }))
    } else {
        Ok(None)
    }
}

/// Verify an inline snapshot while its caller keeps custody of the exact
/// physical bytes and ordered children for staging.
pub fn validate_inline_native_protected_snapshot_v1(
    origin: &super::NativeProtectedNativeOriginV1,
    payload: &[u8],
    roots: &[ObjectRef],
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if native_protected_snapshot_paged_len_v1(payload) != Ok(None) {
        return Err(IndexError::DigestConflict);
    }
    charge_snapshot_decode_v1(payload.len(), budget)?;
    if native_immutable_object_ref_with_work_v1(payload, roots, &mut || budget.checkpoint())?
        != Some(origin.snapshot)
    {
        return Err(IndexError::DigestConflict);
    }
    let funded = snapshot_decode::decode(payload, &[], budget)?;
    verify_decoded_snapshot_v1(origin.snapshot, &funded.data, roots, budget)?;
    if funded.data.source_root_nonce != origin.source_root_nonce
        || funded.data.source_routes != origin.source_routes
    {
        return Err(IndexError::DigestConflict);
    }
    budget.checkpoint()
}

fn charge_snapshot_decode_v1(bytes: usize, budget: &mut WorkBudget) -> Result<(), IndexError> {
    let bytes = u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes: bytes.checked_mul(5).ok_or(IndexError::ResourceLimited)?,
        ..Default::default()
    })
}

fn verify_decoded_snapshot_v1(
    reference: ObjectRef,
    snapshot: &super::NativeProtectedNativeSnapshotV1,
    roots: &[ObjectRef],
    budget: &WorkBudget,
) -> Result<(), IndexError> {
    snapshot
        .validate_shape_v1()
        .map_err(|message| IndexError::InvalidInput(message.into()))?;
    if snapshot.roots.as_slice() != roots
        || snapshot
            .physical_plan_with_work_v1(&mut || budget.checkpoint())
            .map_err(|cause| cause.into_error(|_| IndexError::DigestConflict))?
            .reference_v1()
            != reference
    {
        return Err(IndexError::DigestConflict);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        collections::BTreeMap,
        io::{self, Cursor},
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };

    use super::*;
    use crate::index::{
        Digest, MemoryLease, MemoryLeaseOwner, MemoryScope, MemoryScopePort,
        NativeProtectedNativeSnapshotV1, NativeProtectedSnapshotPhysicalV1, RegisteredObject,
        WorkLimits, native_immutable_object_ref_v1,
    };

    const TEST_CLASS_BYTES: u64 = 64 * 1024 * 1024;

    #[test]
    fn route_head_count_accepts_the_existing_wire_forms() -> Result<(), serde_json::Error> {
        for wire in [
            r#"{"wire_version":1,"heads":[{}],"repomap_heads":[{},{}]}"#,
            r#"{"wire_version":1,"native_origin":null,"heads":[{}],"repomap_heads":[{},{}]}"#,
            r#"[1,null,[{}],[{},{}]]"#,
        ] {
            let counts: RouteHeadCounts = serde_json::from_str(wire)?;
            assert_eq!((counts.heads.0, counts.repomap_heads.0), (1, 2));
        }
        assert!(serde_json::from_str::<RouteHeadCounts>(
            r#"{"wire_version":1,"native_origin":null,"native_origin":null,"heads":[],"repomap_heads":[]}"#
        )
        .is_err());
        Ok(())
    }

    #[derive(Clone, Default)]
    struct TestMemory(Arc<AtomicU64>);

    impl MemoryLeaseOwner for TestMemory {
        fn release(&self, bytes: u64) {
            let previous = self.0.fetch_sub(bytes, Ordering::Relaxed);
            assert!(previous >= bytes);
        }
    }

    impl MemoryScopePort for TestMemory {
        fn reserve(
            &self,
            bytes: u64,
            _lifetime: MemoryLifetime,
        ) -> Result<MemoryLease, IndexError> {
            let _previous = self
                .0
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                    live.checked_add(bytes)
                        .filter(|next| *next <= TEST_CLASS_BYTES)
                })
                .map_err(|_| IndexError::ResourceLimited)?;
            Ok(MemoryLease::new(bytes, Arc::new(self.clone())))
        }
    }

    struct FixtureSnapshotPage<'a> {
        stored: &'a StoredObject,
        input: Cursor<&'a [u8]>,
        finished: &'a Cell<u8>,
    }

    impl io::Read for FixtureSnapshotPage<'_> {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            io::Read::read(&mut self.input, output)
        }
    }

    impl NativeProtectedSnapshotPageV1 for FixtureSnapshotPage<'_> {
        fn registration(&self) -> &RegisteredObject {
            &self.stored.registration
        }

        fn payload_len(&self) -> usize {
            self.stored.payload.len()
        }

        fn finish(self: Box<Self>) -> Result<(), IndexError> {
            if usize::try_from(self.input.position()).ok() != Some(self.payload_len()) {
                return Err(IndexError::CorruptObject(
                    "object payload not fully consumed".into(),
                ));
            }
            if native_immutable_object_ref_v1(
                &self.stored.payload,
                &self.stored.registration.children,
            ) != Some(self.stored.registration.reference)
            {
                return Err(IndexError::CorruptObject(
                    "object digest or size mismatch".into(),
                ));
            }
            self.finished.set(self.finished.get() + 1);
            Ok(())
        }
    }

    #[test]
    fn paged_snapshot_round_trips_and_rejects_reordered_pages() -> Result<(), IndexError> {
        let root = ObjectRef(Digest([7; 32]));
        let snapshot = NativeProtectedNativeSnapshotV1 {
            wire_version: NativeProtectedNativeSnapshotV1::WIRE_VERSION,
            source_root_nonce: Digest([8; 32]),
            source_routes: root,
            // Four JSON bytes per value, with space for the envelope, keep
            // the snapshot near 32 MiB while using both physical pages.
            operation_rows: vec![vec![
                255;
                (crate::index::MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1
                    - 4096)
                    / 4
            ]],
            genesis_views: Vec::new(),
            frontiers: Vec::new(),
            source_authorities: Vec::new(),
            event_claims: Vec::new(),
            roots: vec![root],
        };
        let payload = serde_json::to_vec(&snapshot)
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let physical = NativeProtectedSnapshotPhysicalV1::new_v1(&payload, &snapshot.roots)
            .map_err(|message| IndexError::InvalidInput(message.into()))?;
        let reference = physical.reference_v1();
        assert_eq!(
            snapshot
                .object_ref_v1()
                .map_err(|message| IndexError::InvalidInput(message.into()))?,
            reference
        );
        let mut objects = BTreeMap::new();
        physical.visit_v1(|candidate, bytes, children| {
            let wire_bytes = u64::try_from(bytes.len() + 16 + 32 * children.len())
                .map_err(|_| IndexError::ResourceLimited)?;
            let object = StoredObject::new(
                RegisteredObject::unscoped(candidate, wire_bytes, children.to_vec()),
                bytes.to_vec(),
                None,
            );
            assert!(objects.insert(candidate, object).is_none());
            Ok::<(), IndexError>(())
        })?;
        drop(payload);
        assert_eq!(objects.len(), 3);
        let manifest = objects
            .get(&reference)
            .ok_or(IndexError::SnapshotUnavailable)?;
        let page_refs: [ObjectRef; 2] = manifest
            .registration
            .children
            .as_slice()
            .try_into()
            .map_err(|_| IndexError::DigestConflict)?;
        let mut swapped = manifest.registration.children.clone();
        swapped.swap(0, 1);
        // The new root is itself content-addressed. Reject its page layout,
        // even when parent hash validation succeeds.
        let wrong_reference = native_immutable_object_ref_v1(&manifest.payload, &swapped)
            .ok_or(IndexError::ResourceLimited)?;
        let wrong_manifest = StoredObject::new(
            RegisteredObject::unscoped(wrong_reference, manifest.registration.bytes, swapped),
            manifest.payload.clone(),
            None,
        );
        assert!(objects.insert(wrong_reference, wrong_manifest).is_none());
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 6,
                writes: 0,
                bytes: 1024 * 1024 * 1024,
                mutations: 0,
            },
            Instant::now() + Duration::from_secs(30),
        );
        let memory = TestMemory::default();
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(memory.clone()))));
        let get = |requested, budget: &mut WorkBudget| {
            let stored = objects
                .get(&requested)
                .ok_or(IndexError::SnapshotUnavailable)?;
            let backing = StoredObject::allocation_bytes(
                stored.payload.len(),
                stored.registration.children.len(),
            )?;
            let lease = budget
                .reserve_memory(backing, MemoryLifetime::Temporary)?
                .ok_or(IndexError::ResourceLimited)?;
            Ok(StoredObject::new(
                stored.registration.clone(),
                stored.payload.clone(),
                Some(lease),
            ))
        };
        with_native_protected_snapshot_v1(reference, get, &mut budget, |actual, _budget| {
            assert_eq!(actual, &snapshot);
            let live = memory.0.load(Ordering::Relaxed);
            let row = actual
                .operation_rows
                .first()
                .ok_or(IndexError::DigestConflict)?;
            assert!(
                live >= u64::try_from(row.capacity()).map_err(|_| IndexError::ResourceLimited)?
            );
            assert!(
                live < TEST_CLASS_BYTES / 4,
                "physical pages survived decoding"
            );
            Ok(())
        })?;
        assert_eq!(memory.0.load(Ordering::Relaxed), 0);

        // The later row growth needs the first page's grant to be released
        // while decoding. The two complete pages plus the old and new row
        // buffers would otherwise exceed the 64 MiB class.
        let occupied_bytes = 20 * 1024 * 1024;
        let occupied = budget
            .reserve_memory(occupied_bytes, MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited)?;
        with_native_protected_snapshot_v1(reference, get, &mut budget, |actual, _budget| {
            assert_eq!(actual, &snapshot);
            Ok(())
        })?;
        assert_eq!(memory.0.load(Ordering::Relaxed), occupied_bytes);
        drop(occupied);
        assert_eq!(memory.0.load(Ordering::Relaxed), 0);

        // Two materialized pages cannot fit beside this loan. The page
        // stream must still finish both authenticators before the visitor.
        let occupied_bytes = 48 * 1024 * 1024;
        let occupied = budget
            .reserve_memory(occupied_bytes, MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited)?;
        let finished = Cell::new(0);
        with_native_protected_snapshot_stream_v1(
            reference,
            |requested, budget| {
                assert_eq!(requested, reference);
                get(requested, budget)
            },
            |requested, _budget| {
                let next = page_refs
                    .get(usize::from(finished.get()))
                    .ok_or(IndexError::DigestConflict)?;
                assert_eq!(requested, *next);
                let stored = objects
                    .get(&requested)
                    .ok_or(IndexError::SnapshotUnavailable)?;
                Ok::<Box<dyn NativeProtectedSnapshotPageV1 + '_>, IndexError>(Box::new(
                    FixtureSnapshotPage {
                        stored,
                        input: Cursor::new(stored.payload.as_slice()),
                        finished: &finished,
                    },
                ))
            },
            &mut budget,
            |actual, _budget| {
                assert_eq!(finished.get(), 2);
                assert_eq!(actual, &snapshot);
                Ok(())
            },
        )?;
        assert_eq!(memory.0.load(Ordering::Relaxed), occupied_bytes);
        drop(occupied);
        assert_eq!(memory.0.load(Ordering::Relaxed), 0);

        // The first JSON byte refuses parsing. The physical page also has a
        // bad digest, which must be reported before that parser refusal.
        let original_first = objects
            .get(&page_refs[0])
            .ok_or(IndexError::SnapshotUnavailable)?;
        let mut corrupt_bytes = original_first.payload.clone();
        corrupt_bytes[0] = b'!';
        let corrupt_first =
            StoredObject::new(original_first.registration.clone(), corrupt_bytes, None);
        let finished = Cell::new(0);
        let result: Result<(), IndexError> = with_native_protected_snapshot_stream_v1(
            reference,
            |requested, budget| get(requested, budget),
            |requested, _budget| {
                assert_eq!(requested, page_refs[0]);
                Ok::<Box<dyn NativeProtectedSnapshotPageV1 + '_>, IndexError>(Box::new(
                    FixtureSnapshotPage {
                        stored: &corrupt_first,
                        input: Cursor::new(corrupt_first.payload.as_slice()),
                        finished: &finished,
                    },
                ))
            },
            &mut budget,
            |_actual, _budget| panic!("corrupt page reached Catalog callback"),
        );
        assert_eq!(
            result,
            Err(IndexError::CorruptObject(
                "object digest or size mismatch".into()
            ))
        );
        assert_eq!(finished.get(), 0);
        assert_eq!(memory.0.load(Ordering::Relaxed), 0);

        let result = with_native_protected_snapshot_v1(
            wrong_reference,
            get,
            &mut budget,
            |_snapshot, _budget| Err::<(), _>(IndexError::Conflict),
        );
        assert_eq!(result, Err(IndexError::DigestConflict));
        assert_eq!(memory.0.load(Ordering::Relaxed), 0);

        // Leave room to read both pages, but not to grow the decoded row.
        let occupied_bytes = 28 * 1024 * 1024;
        let occupied = budget
            .reserve_memory(occupied_bytes, MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited)?;
        let result =
            with_native_protected_snapshot_v1(reference, get, &mut budget, |_snapshot, _budget| {
                Err::<(), _>(IndexError::Conflict)
            });
        assert_eq!(result, Err(IndexError::ResourceLimited));
        assert_eq!(memory.0.load(Ordering::Relaxed), occupied_bytes);
        drop(occupied);
        assert_eq!(memory.0.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
