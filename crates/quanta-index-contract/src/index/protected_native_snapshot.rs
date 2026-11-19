//! Exact persisted Catalog DATA for an independently authenticated Native
//! protected-workspace transfer. It cannot authorize a Source publication.
use super::immutable_object_wire::{
    native_immutable_object_hasher_v1, serialize_protected_with_work_v1,
};
use super::{
    CatalogView, Digest, NativeProtectedEncodingErrorV1, ObjectRef, ProducerId, StreamId,
    WorkspaceKey,
};
use crate::bounded_cluster_members::BoundedVecV1;
use serde::ser::SerializeStruct as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::convert::Infallible;
use std::io;

pub const MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1: usize = 32 * 1024 * 1024;
pub const MAX_NATIVE_PROTECTED_SNAPSHOT_ROWS_V1: usize = 16_384;
pub const NATIVE_PROTECTED_SNAPSHOT_PAGE_BYTES_V1: usize = 16 * 1024 * 1024;

const PAGED_SNAPSHOT_DOMAIN_V1: &[u8] = b"qidx.native.protected-snapshot.pages.v1\0";
const PAGED_SNAPSHOT_MANIFEST_BYTES_V1: usize = PAGED_SNAPSHOT_DOMAIN_V1.len() + 8;

fn paged_snapshot_identity_v1(
    bytes: usize,
    pages: &[ObjectRef; 2],
) -> Result<([u8; PAGED_SNAPSHOT_MANIFEST_BYTES_V1], ObjectRef), &'static str> {
    let [first, second] = pages;
    if first == second {
        return Err("protected Native snapshot page identities collide");
    }
    let mut manifest = [0; PAGED_SNAPSHOT_MANIFEST_BYTES_V1];
    let (tag, length) = manifest.split_at_mut(PAGED_SNAPSHOT_DOMAIN_V1.len());
    tag.copy_from_slice(PAGED_SNAPSHOT_DOMAIN_V1);
    length.copy_from_slice(
        &u64::try_from(bytes)
            .map_err(|_| "protected Native snapshot length overflows")?
            .to_be_bytes(),
    );
    let reference = super::native_immutable_object_ref_v1(&manifest, pages)
        .ok_or("protected Native snapshot page manifest overflows")?;
    Ok((manifest, reference))
}

/// Recognize the fixed physical manifest without accepting a malformed page
/// prefix as an inline JSON snapshot. Page identities are checked after both
/// child objects have been read and their exact bytes are available.
pub fn native_protected_snapshot_paged_len_v1(bytes: &[u8]) -> Result<Option<usize>, &'static str> {
    if bytes.first() == Some(&b'{') {
        return if bytes.len() <= NATIVE_PROTECTED_SNAPSHOT_PAGE_BYTES_V1 {
            Ok(None)
        } else {
            Err("inline protected Native snapshot exceeds physical page")
        };
    }
    let length = bytes
        .strip_prefix(PAGED_SNAPSHOT_DOMAIN_V1)
        .and_then(|suffix| suffix.try_into().ok())
        .map(u64::from_be_bytes)
        .ok_or("protected Native snapshot page manifest differs")?;
    let length =
        usize::try_from(length).map_err(|_| "protected Native snapshot page length overflows")?;
    if !(NATIVE_PROTECTED_SNAPSHOT_PAGE_BYTES_V1 + 1..=MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1)
        .contains(&length)
    {
        return Err("protected Native snapshot page manifest differs");
    }
    Ok(Some(length))
}

enum SnapshotPhysicalLayoutV1 {
    Inline(ObjectRef),
    Paged {
        pages: [ObjectRef; 2],
        manifest: [u8; PAGED_SNAPSHOT_MANIFEST_BYTES_V1],
        reference: ObjectRef,
    },
}

/// Bounded physical object identities and byte ranges for one canonical
/// semantic snapshot. Pages are emitted in their original payload order.
pub struct NativeProtectedSnapshotPlanV1 {
    payload_len: usize,
    layout: SnapshotPhysicalLayoutV1,
}

impl NativeProtectedSnapshotPlanV1 {
    #[must_use]
    pub fn reference_v1(&self) -> ObjectRef {
        match &self.layout {
            SnapshotPhysicalLayoutV1::Inline(reference)
            | SnapshotPhysicalLayoutV1::Paged { reference, .. } => *reference,
        }
    }

    pub fn pages_v1(&self) -> impl Iterator<Item = (ObjectRef, usize)> + '_ {
        let (first, second) = match &self.layout {
            SnapshotPhysicalLayoutV1::Inline(reference) => ((*reference, self.payload_len), None),
            SnapshotPhysicalLayoutV1::Paged { pages, .. } => (
                (pages[0], NATIVE_PROTECTED_SNAPSHOT_PAGE_BYTES_V1),
                Some((
                    pages[1],
                    self.payload_len - NATIVE_PROTECTED_SNAPSHOT_PAGE_BYTES_V1,
                )),
            ),
        };
        std::iter::once(first).chain(second)
    }

    #[must_use]
    pub fn manifest_v1(&self) -> Option<(ObjectRef, &[u8], &[ObjectRef])> {
        match &self.layout {
            SnapshotPhysicalLayoutV1::Inline(_) => None,
            SnapshotPhysicalLayoutV1::Paged {
                pages,
                manifest,
                reference,
            } => Some((*reference, manifest, pages)),
        }
    }
}

/// Canonical physical layout for the same bounded semantic snapshot. An
/// inline payload retains its original object identity; larger payloads use
/// two child objects and one fixed manifest object.
pub struct NativeProtectedSnapshotPhysicalV1<'a> {
    payload: &'a [u8],
    roots: &'a [ObjectRef],
    plan: NativeProtectedSnapshotPlanV1,
}

impl<'a> NativeProtectedSnapshotPhysicalV1<'a> {
    pub fn new_v1(payload: &'a [u8], roots: &'a [ObjectRef]) -> Result<Self, &'static str> {
        if payload.first() != Some(&b'{')
            || payload.len() > MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1
            || roots.is_empty()
            || roots.len() > crate::native_protocol::MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN
            || roots.iter().any(|root| root.0 == Digest([0; 32]))
            || roots.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err("protected Native snapshot physical shape differs");
        }
        let layout = if payload.len() <= NATIVE_PROTECTED_SNAPSHOT_PAGE_BYTES_V1 {
            SnapshotPhysicalLayoutV1::Inline(
                super::native_immutable_object_ref_v1(payload, roots)
                    .ok_or("protected Native snapshot object overflows")?,
            )
        } else {
            let (first, second) = payload.split_at(NATIVE_PROTECTED_SNAPSHOT_PAGE_BYTES_V1);
            let first_ref = super::native_immutable_object_ref_v1(first, roots)
                .ok_or("protected Native snapshot first page overflows")?;
            let second_ref = super::native_immutable_object_ref_v1(second, &[])
                .ok_or("protected Native snapshot second page overflows")?;
            let pages = [first_ref, second_ref];
            let (manifest, reference) = paged_snapshot_identity_v1(payload.len(), &pages)?;
            SnapshotPhysicalLayoutV1::Paged {
                pages,
                manifest,
                reference,
            }
        };
        Ok(Self {
            payload,
            roots,
            plan: NativeProtectedSnapshotPlanV1 {
                payload_len: payload.len(),
                layout,
            },
        })
    }

    #[must_use]
    pub fn reference_v1(&self) -> ObjectRef {
        self.plan.reference_v1()
    }

    pub fn visit_v1<E>(
        &self,
        mut visit: impl FnMut(ObjectRef, &[u8], &[ObjectRef]) -> Result<(), E>,
    ) -> Result<(), E> {
        match &self.plan.layout {
            SnapshotPhysicalLayoutV1::Inline(reference) => {
                visit(*reference, self.payload, self.roots)
            }
            SnapshotPhysicalLayoutV1::Paged {
                pages,
                manifest,
                reference,
            } => {
                let (first, second) = self
                    .payload
                    .split_at(NATIVE_PROTECTED_SNAPSHOT_PAGE_BYTES_V1);
                let [first_ref, second_ref] = *pages;
                visit(first_ref, first, self.roots)?;
                visit(second_ref, second, &[])?;
                visit(*reference, manifest, pages)
            }
        }
    }
}

struct SnapshotDigestWriterV1 {
    first: Sha256,
    second: Option<Sha256>,
    first_remaining: usize,
    remaining: usize,
}

impl io::Write for SnapshotDigestWriterV1 {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.remaining = self
            .remaining
            .checked_sub(bytes.len())
            .ok_or_else(|| io::Error::other("protected Native snapshot changed width"))?;
        let first_len = self.first_remaining.min(bytes.len());
        let (first, second) = bytes.split_at(first_len);
        if !first.is_empty() {
            self.first.update(first);
            self.first_remaining = self.first_remaining.saturating_sub(first_len);
        }
        if !second.is_empty() {
            self.second
                .as_mut()
                .ok_or_else(|| io::Error::other("protected Native snapshot page absent"))?
                .update(second);
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeProtectedFrontierV1 {
    pub workspace: WorkspaceKey,
    pub producer: ProducerId,
    pub stream: StreamId,
    pub stream_epoch: u64,
    pub sequence: u64,
}
crate::native_serde_struct!(NativeProtectedFrontierV1 {
    workspace: WorkspaceKey,
    producer: ProducerId,
    stream: StreamId,
    stream_epoch: u64,
    sequence: u64
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeProtectedSourceAuthorityV1 {
    pub workspace: WorkspaceKey,
    pub workspace_epoch: u64,
    pub producer: ProducerId,
    pub source_epoch: u64,
}
crate::native_serde_struct!(NativeProtectedSourceAuthorityV1 {
    workspace: WorkspaceKey,
    workspace_epoch: u64,
    producer: ProducerId,
    source_epoch: u64
});

/// Operation and event-claim payloads use the ONE canonical persisted Catalog
/// codecs. No substitute event, fence, Source epoch or ACK is synthesized.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeProtectedNativeSnapshotV1 {
    pub wire_version: u32,
    pub source_root_nonce: Digest,
    pub source_routes: ObjectRef,
    pub operation_rows: Vec<Vec<u8>>,
    pub genesis_views: Vec<CatalogView>,
    pub frontiers: Vec<NativeProtectedFrontierV1>,
    pub source_authorities: Vec<NativeProtectedSourceAuthorityV1>,
    pub event_claims: Vec<Vec<u8>>,
    /// Sorted exact immutable closure roots derived by the Catalog supplier.
    pub roots: Vec<ObjectRef>,
}

impl Serialize for NativeProtectedNativeSnapshotV1 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut record = serializer.serialize_struct("NativeProtectedNativeSnapshotV1", 9)?;
        record.serialize_field("wire_version", &self.wire_version)?;
        record.serialize_field("source_root_nonce", &self.source_root_nonce)?;
        record.serialize_field("source_routes", &self.source_routes)?;
        record.serialize_field("operation_rows", &self.operation_rows)?;
        record.serialize_field("genesis_views", &self.genesis_views)?;
        record.serialize_field("frontiers", &self.frontiers)?;
        record.serialize_field("source_authorities", &self.source_authorities)?;
        record.serialize_field("event_claims", &self.event_claims)?;
        record.serialize_field("roots", &self.roots)?;
        record.end()
    }
}

impl<'de> Deserialize<'de> for NativeProtectedNativeSnapshotV1 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        type SnapshotRowsV1<T> = BoundedVecV1<T, MAX_NATIVE_PROTECTED_SNAPSHOT_ROWS_V1>;
        type SnapshotGenesisV1 =
            BoundedVecV1<CatalogView, { super::MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1 }>;
        type SnapshotRootsV1 = BoundedVecV1<
            ObjectRef,
            { crate::native_protocol::MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN },
        >;
        const FIELDS: &[&str] = &[
            "wire_version",
            "source_root_nonce",
            "source_routes",
            "operation_rows",
            "genesis_views",
            "frontiers",
            "source_authorities",
            "event_claims",
            "roots",
        ];

        struct SnapshotVisitor;

        impl<'de> serde::de::Visitor<'de> for SnapshotVisitor {
            type Value = NativeProtectedNativeSnapshotV1;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a complete NativeProtectedNativeSnapshotV1 record")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut wire_version = None;
                let mut source_root_nonce = None;
                let mut source_routes = None;
                let mut operation_rows = None;
                let mut genesis_views = None;
                let mut frontiers = None;
                let mut source_authorities = None;
                let mut event_claims = None;
                let mut roots = None;
                while let Some(key) =
                    map.next_key_seed(quanta_index_contract_base::ClosedFieldKeySeedV1(FIELDS))?
                {
                    match key {
                        "wire_version" => {
                            if wire_version.is_some() {
                                return Err(serde::de::Error::duplicate_field("wire_version"));
                            }
                            wire_version = Some(map.next_value::<u32>()?);
                        }
                        "source_root_nonce" => {
                            if source_root_nonce.is_some() {
                                return Err(serde::de::Error::duplicate_field("source_root_nonce"));
                            }
                            source_root_nonce = Some(map.next_value::<Digest>()?);
                        }
                        "source_routes" => {
                            if source_routes.is_some() {
                                return Err(serde::de::Error::duplicate_field("source_routes"));
                            }
                            source_routes = Some(map.next_value::<ObjectRef>()?);
                        }
                        "operation_rows" => {
                            if operation_rows.is_some() {
                                return Err(serde::de::Error::duplicate_field("operation_rows"));
                            }
                            operation_rows =
                                Some(map.next_value::<SnapshotRowsV1<Vec<u8>>>()?.into_inner());
                        }
                        "genesis_views" => {
                            if genesis_views.is_some() {
                                return Err(serde::de::Error::duplicate_field("genesis_views"));
                            }
                            genesis_views =
                                Some(map.next_value::<SnapshotGenesisV1>()?.into_inner());
                        }
                        "frontiers" => {
                            if frontiers.is_some() {
                                return Err(serde::de::Error::duplicate_field("frontiers"));
                            }
                            frontiers = Some(
                                map.next_value::<SnapshotRowsV1<NativeProtectedFrontierV1>>()?
                                    .into_inner(),
                            );
                        }
                        "source_authorities" => {
                            if source_authorities.is_some() {
                                return Err(serde::de::Error::duplicate_field(
                                    "source_authorities",
                                ));
                            }
                            source_authorities = Some(
                                map.next_value::<SnapshotRowsV1<NativeProtectedSourceAuthorityV1>>(
                                )?
                                .into_inner(),
                            );
                        }
                        "event_claims" => {
                            if event_claims.is_some() {
                                return Err(serde::de::Error::duplicate_field("event_claims"));
                            }
                            event_claims =
                                Some(map.next_value::<SnapshotRowsV1<Vec<u8>>>()?.into_inner());
                        }
                        "roots" => {
                            if roots.is_some() {
                                return Err(serde::de::Error::duplicate_field("roots"));
                            }
                            roots = Some(map.next_value::<SnapshotRootsV1>()?.into_inner());
                        }
                        _ => unreachable!("closed field seed only returns schema keys"),
                    }
                }
                Ok(NativeProtectedNativeSnapshotV1 {
                    wire_version: wire_version
                        .ok_or_else(|| serde::de::Error::missing_field("wire_version"))?,
                    source_root_nonce: source_root_nonce
                        .ok_or_else(|| serde::de::Error::missing_field("source_root_nonce"))?,
                    source_routes: source_routes
                        .ok_or_else(|| serde::de::Error::missing_field("source_routes"))?,
                    operation_rows: operation_rows
                        .ok_or_else(|| serde::de::Error::missing_field("operation_rows"))?,
                    genesis_views: genesis_views
                        .ok_or_else(|| serde::de::Error::missing_field("genesis_views"))?,
                    frontiers: frontiers
                        .ok_or_else(|| serde::de::Error::missing_field("frontiers"))?,
                    source_authorities: source_authorities
                        .ok_or_else(|| serde::de::Error::missing_field("source_authorities"))?,
                    event_claims: event_claims
                        .ok_or_else(|| serde::de::Error::missing_field("event_claims"))?,
                    roots: roots.ok_or_else(|| serde::de::Error::missing_field("roots"))?,
                })
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let wire_version = seq
                    .next_element::<u32>()?
                    .ok_or_else(|| serde::de::Error::missing_field("wire_version"))?;
                let source_root_nonce = seq
                    .next_element::<Digest>()?
                    .ok_or_else(|| serde::de::Error::missing_field("source_root_nonce"))?;
                let source_routes = seq
                    .next_element::<ObjectRef>()?
                    .ok_or_else(|| serde::de::Error::missing_field("source_routes"))?;
                let operation_rows = seq
                    .next_element::<SnapshotRowsV1<Vec<u8>>>()?
                    .ok_or_else(|| serde::de::Error::missing_field("operation_rows"))?
                    .into_inner();
                let genesis_views = seq
                    .next_element::<SnapshotGenesisV1>()?
                    .ok_or_else(|| serde::de::Error::missing_field("genesis_views"))?
                    .into_inner();
                let frontiers = seq
                    .next_element::<SnapshotRowsV1<NativeProtectedFrontierV1>>()?
                    .ok_or_else(|| serde::de::Error::missing_field("frontiers"))?
                    .into_inner();
                let source_authorities = seq
                    .next_element::<SnapshotRowsV1<NativeProtectedSourceAuthorityV1>>()?
                    .ok_or_else(|| serde::de::Error::missing_field("source_authorities"))?
                    .into_inner();
                let event_claims = seq
                    .next_element::<SnapshotRowsV1<Vec<u8>>>()?
                    .ok_or_else(|| serde::de::Error::missing_field("event_claims"))?
                    .into_inner();
                let roots = seq
                    .next_element::<SnapshotRootsV1>()?
                    .ok_or_else(|| serde::de::Error::missing_field("roots"))?
                    .into_inner();
                if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    return Err(serde::de::Error::invalid_length(
                        FIELDS.len().saturating_add(1),
                        &self,
                    ));
                }
                Ok(NativeProtectedNativeSnapshotV1 {
                    wire_version,
                    source_root_nonce,
                    source_routes,
                    operation_rows,
                    genesis_views,
                    frontiers,
                    source_authorities,
                    event_claims,
                    roots,
                })
            }
        }

        deserializer.deserialize_struct("NativeProtectedNativeSnapshotV1", FIELDS, SnapshotVisitor)
    }
}

impl NativeProtectedNativeSnapshotV1 {
    pub const WIRE_VERSION: u32 = 1;

    pub fn validate_shape_v1(&self) -> Result<(), &'static str> {
        let zero = Digest([0; 32]);
        let rows = [
            self.operation_rows.len(),
            self.genesis_views.len(),
            self.frontiers.len(),
            self.source_authorities.len(),
            self.event_claims.len(),
        ]
        .into_iter()
        .try_fold(0usize, |total, count| total.checked_add(count));
        if rows.is_none_or(|count| count > MAX_NATIVE_PROTECTED_SNAPSHOT_ROWS_V1)
            || self.wire_version != Self::WIRE_VERSION
            || self.source_root_nonce == zero
            || self.source_routes.0 == zero
            || self.operation_rows.is_empty()
            || self.operation_rows.len() > MAX_NATIVE_PROTECTED_SNAPSHOT_ROWS_V1
            || self.genesis_views.len() > super::MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1
            || self.frontiers.len() > MAX_NATIVE_PROTECTED_SNAPSHOT_ROWS_V1
            || self.source_authorities.len() > MAX_NATIVE_PROTECTED_SNAPSHOT_ROWS_V1
            || self.event_claims.len() > MAX_NATIVE_PROTECTED_SNAPSHOT_ROWS_V1
            || self.roots.is_empty()
            || self.roots.len() > crate::native_protocol::MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN
            || !self.roots.contains(&self.source_routes)
            || self.roots.iter().any(|root| root.0 == zero)
            || self.roots.windows(2).any(|pair| pair[0] >= pair[1])
            || self
                .operation_rows
                .iter()
                .chain(&self.event_claims)
                .any(Vec::is_empty)
        {
            return Err("protected Native snapshot shape differs");
        }
        Ok(())
    }

    pub fn encoded_payload_len_v1(&self) -> Result<usize, &'static str> {
        self.encoded_payload_len_with_work_v1(&mut || Ok::<(), Infallible>(()))
            .map_err(NativeProtectedEncodingErrorV1::into_format_error)
    }

    pub fn encoded_payload_len_with_work_v1<E>(
        &self,
        checkpoint: &mut impl FnMut() -> Result<(), E>,
    ) -> Result<usize, NativeProtectedEncodingErrorV1<E>> {
        self.validate_shape_v1()
            .map_err(NativeProtectedEncodingErrorV1::Format)?;
        struct Counter(usize);
        impl std::io::Write for Counter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0 = self
                    .0
                    .checked_add(bytes.len())
                    .filter(|count| *count <= MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1)
                    .ok_or_else(|| std::io::Error::other("protected snapshot exceeds bound"))?;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut counter = Counter(0);
        serialize_protected_with_work_v1(
            self,
            &mut counter,
            checkpoint,
            "count protected Native snapshot",
        )?;
        Ok(counter.0)
    }

    pub fn object_ref_v1(&self) -> Result<ObjectRef, &'static str> {
        self.physical_plan_v1().map(|plan| plan.reference_v1())
    }

    pub fn physical_plan_v1(&self) -> Result<NativeProtectedSnapshotPlanV1, &'static str> {
        self.physical_plan_with_work_v1(&mut || Ok::<(), Infallible>(()))
            .map_err(NativeProtectedEncodingErrorV1::into_format_error)
    }

    pub fn physical_plan_with_work_v1<E>(
        &self,
        checkpoint: &mut impl FnMut() -> Result<(), E>,
    ) -> Result<NativeProtectedSnapshotPlanV1, NativeProtectedEncodingErrorV1<E>> {
        let expected = self.encoded_payload_len_with_work_v1(checkpoint)?;
        let first_len = expected.min(NATIVE_PROTECTED_SNAPSHOT_PAGE_BYTES_V1);
        let second_len = expected.saturating_sub(first_len);
        let first = native_immutable_object_hasher_v1(first_len, &self.roots).ok_or(
            NativeProtectedEncodingErrorV1::Format(
                "protected Native snapshot first page overflows",
            ),
        )?;
        let second = if second_len == 0 {
            None
        } else {
            Some(native_immutable_object_hasher_v1(second_len, &[]).ok_or(
                NativeProtectedEncodingErrorV1::Format(
                    "protected Native snapshot second page overflows",
                ),
            )?)
        };
        let mut writer = SnapshotDigestWriterV1 {
            first,
            second,
            first_remaining: first_len,
            remaining: expected,
        };
        serialize_protected_with_work_v1(
            self,
            &mut writer,
            checkpoint,
            "encode protected Native snapshot",
        )?;
        if writer.remaining != 0 {
            return Err(NativeProtectedEncodingErrorV1::Format(
                "protected Native snapshot changed",
            ));
        }
        let first = ObjectRef(Digest(writer.first.finalize().into()));
        let layout = if let Some(second) = writer.second {
            let second = ObjectRef(Digest(second.finalize().into()));
            let pages = [first, second];
            let (manifest, reference) = paged_snapshot_identity_v1(expected, &pages)
                .map_err(NativeProtectedEncodingErrorV1::Format)?;
            SnapshotPhysicalLayoutV1::Paged {
                pages,
                manifest,
                reference,
            }
        } else {
            SnapshotPhysicalLayoutV1::Inline(first)
        };
        Ok(NativeProtectedSnapshotPlanV1 {
            payload_len: expected,
            layout,
        })
    }
}

#[cfg(test)]
mod physical_tests {
    use super::*;

    #[test]
    fn snapshot_wire_keeps_ordered_fields_and_positional_input() -> Result<(), &'static str> {
        let snapshot = NativeProtectedNativeSnapshotV1 {
            wire_version: 1,
            source_root_nonce: Digest([7; 32]),
            source_routes: ObjectRef(Digest([9; 32])),
            operation_rows: vec![vec![1]],
            genesis_views: Vec::new(),
            frontiers: Vec::new(),
            source_authorities: Vec::new(),
            event_claims: Vec::new(),
            roots: vec![ObjectRef(Digest([9; 32]))],
        };
        let nonce = serde_json::to_string(&Digest([7; 32])).map_err(|_| "encode nonce")?;
        let route =
            serde_json::to_string(&ObjectRef(Digest([9; 32]))).map_err(|_| "encode route")?;
        let expected = format!(
            r#"{{"wire_version":1,"source_root_nonce":{nonce},"source_routes":{route},"operation_rows":[[1]],"genesis_views":[],"frontiers":[],"source_authorities":[],"event_claims":[],"roots":[{route}]}}"#
        );
        assert_eq!(
            serde_json::to_string(&snapshot).map_err(|_| "encode snapshot")?,
            expected
        );
        assert_eq!(
            serde_json::from_str::<NativeProtectedNativeSnapshotV1>(&expected)
                .map_err(|_| "decode object snapshot")?,
            snapshot
        );
        let positional = format!("[1,{nonce},{route},[[1]],[],[],[],[],[{route}]]");
        assert_eq!(
            serde_json::from_str::<NativeProtectedNativeSnapshotV1>(&positional)
                .map_err(|_| "decode positional snapshot")?,
            snapshot
        );
        Ok(())
    }

    #[test]
    fn snapshot_decode_refuses_over_limit_row_sequence() -> Result<(), &'static str> {
        let snapshot = NativeProtectedNativeSnapshotV1 {
            wire_version: NativeProtectedNativeSnapshotV1::WIRE_VERSION,
            source_root_nonce: Digest([7; 32]),
            source_routes: ObjectRef(Digest([9; 32])),
            operation_rows: vec![vec![1]; MAX_NATIVE_PROTECTED_SNAPSHOT_ROWS_V1 + 1],
            genesis_views: Vec::new(),
            frontiers: Vec::new(),
            source_authorities: Vec::new(),
            event_claims: Vec::new(),
            roots: vec![ObjectRef(Digest([9; 32]))],
        };
        // Deliberately serialize without supplier validation to exercise the
        // receiving sequence, before the caller can check semantic shape.
        let data = serde_json::to_vec(&snapshot).map_err(|_| "encode oversized row fixture")?;
        assert!(serde_json::from_slice::<NativeProtectedNativeSnapshotV1>(&data).is_err());
        Ok(())
    }

    fn roots(count: usize) -> Vec<ObjectRef> {
        (1..=count)
            .map(|ordinal| {
                let mut digest = [0; 32];
                let ordinal = ordinal.to_be_bytes();
                digest
                    .split_at_mut(ordinal.len())
                    .0
                    .copy_from_slice(&ordinal);
                ObjectRef(Digest(digest))
            })
            .collect()
    }

    #[test]
    fn snapshot_plan_preserves_typed_work_refusal_during_each_serialization()
    -> Result<(), &'static str> {
        let roots = roots(1);
        let snapshot = NativeProtectedNativeSnapshotV1 {
            wire_version: NativeProtectedNativeSnapshotV1::WIRE_VERSION,
            source_root_nonce: Digest([7; 32]),
            source_routes: roots[0],
            operation_rows: vec![vec![0; 40_000]],
            genesis_views: Vec::new(),
            frontiers: Vec::new(),
            source_authorities: Vec::new(),
            event_claims: Vec::new(),
            roots,
        };
        let mut count_calls = 0;
        let counted = snapshot
            .encoded_payload_len_with_work_v1(&mut || {
                count_calls += 1;
                Ok::<(), &'static str>(())
            })
            .map_err(|error| error.into_error(|format| format))?;
        assert!(counted > 64 * 1024);
        assert!(count_calls >= 2);

        let mut calls = 0;
        assert_eq!(
            snapshot.encoded_payload_len_with_work_v1(&mut || {
                calls += 1;
                if calls == 2 {
                    Err("count cancelled")
                } else {
                    Ok(())
                }
            }),
            Err(NativeProtectedEncodingErrorV1::Work("count cancelled"))
        );

        calls = 0;
        assert!(matches!(
            snapshot.physical_plan_with_work_v1(&mut || {
                calls += 1;
                if calls == count_calls + 2 {
                    Err("hash cancelled")
                } else {
                    Ok(())
                }
            }),
            Err(NativeProtectedEncodingErrorV1::Work("hash cancelled"))
        ));
        assert_eq!(calls, count_calls + 2);
        Ok(())
    }

    #[test]
    fn streaming_plan_matches_materialized_inline_and_paged_objects() -> Result<(), &'static str> {
        let roots = roots(1);
        for (row_len, expected_pages) in [(1, 1), (NATIVE_PROTECTED_SNAPSHOT_PAGE_BYTES_V1 / 2, 2)]
        {
            let semantic = NativeProtectedNativeSnapshotV1 {
                wire_version: NativeProtectedNativeSnapshotV1::WIRE_VERSION,
                source_root_nonce: Digest([7; 32]),
                source_routes: roots[0],
                operation_rows: vec![vec![0; row_len]],
                genesis_views: Vec::new(),
                frontiers: Vec::new(),
                source_authorities: Vec::new(),
                event_claims: Vec::new(),
                roots: roots.clone(),
            };
            let payload = serde_json::to_vec(&semantic).map_err(|_| "encode snapshot fixture")?;
            let materialized = NativeProtectedSnapshotPhysicalV1::new_v1(&payload, &roots)?;
            let plan = semantic.physical_plan_v1()?;
            let pages = plan.pages_v1().collect::<Vec<_>>();
            assert_eq!(pages.len(), expected_pages);
            assert_eq!(plan.reference_v1(), materialized.reference_v1());
            assert_eq!(semantic.object_ref_v1()?, materialized.reference_v1());
            let mut seen = 0;
            materialized.visit_v1(|reference, bytes, children| {
                if let Some((expected, length)) = pages.get(seen).copied() {
                    assert_eq!((reference, bytes.len()), (expected, length));
                    assert_eq!(children, if seen == 0 { roots.as_slice() } else { &[] });
                } else {
                    let (expected, manifest, page_refs) =
                        plan.manifest_v1().ok_or("unexpected snapshot manifest")?;
                    assert_eq!(reference, expected);
                    assert_eq!(bytes, manifest);
                    assert_eq!(children, page_refs);
                }
                seen += 1;
                Ok::<(), &'static str>(())
            })?;
            assert_eq!(
                seen,
                pages.len() + usize::from(plan.manifest_v1().is_some())
            );
        }
        Ok(())
    }

    #[test]
    fn inline_boundary_retains_original_object_identity() -> Result<(), &'static str> {
        let roots = roots(1);
        let semantic = NativeProtectedNativeSnapshotV1 {
            wire_version: NativeProtectedNativeSnapshotV1::WIRE_VERSION,
            source_root_nonce: Digest([7; 32]),
            source_routes: roots[0],
            operation_rows: vec![vec![1]],
            genesis_views: Vec::new(),
            frontiers: Vec::new(),
            source_authorities: Vec::new(),
            event_claims: Vec::new(),
            roots: roots.clone(),
        };
        let canonical = serde_json::to_vec(&semantic).map_err(|_| "encode semantic snapshot")?;
        assert_eq!(
            semantic.object_ref_v1()?,
            super::super::native_immutable_object_ref_v1(&canonical, &roots)
                .ok_or("original inline ref absent")?
        );
        let mut payload = vec![b'{'; NATIVE_PROTECTED_SNAPSHOT_PAGE_BYTES_V1];
        let physical = NativeProtectedSnapshotPhysicalV1::new_v1(&payload, &roots)?;
        assert_eq!(
            physical.reference_v1(),
            super::super::native_immutable_object_ref_v1(&payload, &roots)
                .ok_or("inline object ref absent")?
        );
        let mut observed = 0;
        physical.visit_v1(|reference, bytes, children| {
            assert_eq!(reference, physical.reference_v1());
            assert_eq!(bytes, payload.as_slice());
            assert_eq!(children, roots.as_slice());
            observed += 1;
            Ok::<(), &'static str>(())
        })?;
        assert_eq!(observed, 1);
        assert_eq!(native_protected_snapshot_paged_len_v1(&payload)?, None);
        payload.push(b'}');
        assert_eq!(
            native_protected_snapshot_paged_len_v1(&payload),
            Err("inline protected Native snapshot exceeds physical page")
        );
        Ok(())
    }

    #[test]
    fn paged_layout_binds_full_roots_and_both_ordered_pages() -> Result<(), &'static str> {
        let roots = roots(crate::native_protocol::MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN);
        let mut payload = vec![b'{'; NATIVE_PROTECTED_SNAPSHOT_PAGE_BYTES_V1 + 1];
        let physical = NativeProtectedSnapshotPhysicalV1::new_v1(&payload, &roots)?;
        let mut records = Vec::new();
        physical.visit_v1(|reference, bytes, children| {
            records.push((
                reference,
                bytes.len(),
                children.to_vec(),
                (bytes.len() < 1024).then(|| bytes.to_vec()),
            ));
            Ok::<(), &'static str>(())
        })?;
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].1, NATIVE_PROTECTED_SNAPSHOT_PAGE_BYTES_V1);
        assert_eq!(records[0].2, roots);
        assert_eq!(records[1].1, 1);
        assert!(records[1].2.is_empty());
        assert_eq!(records[2].0, physical.reference_v1());
        assert_eq!(records[2].2, vec![records[0].0, records[1].0]);
        assert_eq!(
            native_protected_snapshot_paged_len_v1(
                records[2].3.as_deref().ok_or("manifest bytes absent")?
            )?,
            Some(payload.len())
        );
        assert_eq!(
            records[0].0,
            super::super::native_immutable_object_ref_v1(
                &payload[..NATIVE_PROTECTED_SNAPSHOT_PAGE_BYTES_V1],
                &roots
            )
            .ok_or("first page ref absent")?
        );
        assert_eq!(
            records[1].0,
            super::super::native_immutable_object_ref_v1(
                &payload[NATIVE_PROTECTED_SNAPSHOT_PAGE_BYTES_V1..],
                &[]
            )
            .ok_or("second page ref absent")?
        );
        let manifest = records[2].3.as_deref().ok_or("manifest bytes absent")?;
        assert_eq!(
            records[2].0,
            super::super::native_immutable_object_ref_v1(manifest, &records[2].2)
                .ok_or("manifest ref absent")?
        );
        assert_ne!(
            records[2].0,
            super::super::native_immutable_object_ref_v1(manifest, &[records[1].0, records[0].0])
                .ok_or("reordered manifest ref absent")?
        );
        let mut malformed = manifest.to_vec();
        malformed[0] ^= 1;
        assert!(native_protected_snapshot_paged_len_v1(&malformed).is_err());
        assert!(native_protected_snapshot_paged_len_v1(&manifest[..10]).is_err());
        let mut malformed_length = manifest.to_vec();
        malformed_length[PAGED_SNAPSHOT_DOMAIN_V1.len()..]
            .copy_from_slice(&(NATIVE_PROTECTED_SNAPSHOT_PAGE_BYTES_V1 as u64).to_be_bytes());
        assert!(native_protected_snapshot_paged_len_v1(&malformed_length).is_err());
        payload.resize(MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1, b'}');
        let maximum = NativeProtectedSnapshotPhysicalV1::new_v1(&payload, &roots)?;
        let mut lengths = Vec::new();
        maximum.visit_v1(|_, bytes, _| {
            lengths.push(bytes.len());
            Ok::<(), &'static str>(())
        })?;
        assert_eq!(
            lengths,
            vec![
                NATIVE_PROTECTED_SNAPSHOT_PAGE_BYTES_V1,
                NATIVE_PROTECTED_SNAPSHOT_PAGE_BYTES_V1,
                PAGED_SNAPSHOT_MANIFEST_BYTES_V1,
            ]
        );
        payload.push(b'}');
        assert!(NativeProtectedSnapshotPhysicalV1::new_v1(&payload, &roots).is_err());
        Ok(())
    }
}
