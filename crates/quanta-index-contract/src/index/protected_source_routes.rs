//! A bounded, immutable route set for preserved Source and RepoMap heads.
//! These bytes describe expected publications; they confer no producer authority.

use super::{
    CatalogView, Digest, ImmutableOperation, IndexInstanceId, NativeProtectedEncodingErrorV1,
    ObjectRef, OperationKey, StreamId, ViewRef, WorkspaceKey, native_immutable_object_ref_v1,
};
use crate::{
    RepoId, RevisionId, bounded_cluster_members::BoundedVecV1, repomap::RepoMapExpectedActiveV2,
};
use serde::{Deserialize, Deserializer, Serialize, de, ser::SerializeStruct};
use sha2::{Digest as _, Sha256};
use std::convert::Infallible;

use super::immutable_object_wire::{
    native_immutable_object_hasher_v1, serialize_protected_with_work_v1,
};

pub const MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1: usize = 368;
pub const MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1: usize = 4_096;
pub const MAX_NATIVE_PROTECTED_SOURCE_ROUTE_BYTES_V1: usize = 16 * 1024 * 1024;
pub const MAX_NATIVE_PROTECTED_VIEW_MARKER_BYTES_V1: usize = 1024;

/// Derive the candidate route-seal ingress from the original primary operation
/// and root nonce. This labels retry lookup; it does not grant route authority.
pub fn native_protected_source_route_seal_key_v1(
    primary: OperationKey,
    root_nonce: Digest,
) -> Result<OperationKey, &'static str> {
    if primary.sequence == 0 || root_nonce.0 == [0; 32] {
        return Err("route seal scope is invalid");
    }
    let mut hash = blake3::Hasher::new();
    let _updated = hash.update(b"semantica/imported-source-route-seal/v1\0");
    let _updated = hash.update(&root_nonce.0);
    for axis in [
        &primary.workspace.tenant.0,
        &primary.workspace.collection.0,
        &primary.workspace.worktree.0,
        &primary.workspace.slot.0,
        &primary.producer.0,
        &primary.stream.0,
    ] {
        let _updated = hash.update(axis);
    }
    for scalar in [primary.epoch, primary.stream_epoch, primary.sequence] {
        let _updated = hash.update(&scalar.to_be_bytes());
    }
    let mut stream = [0; 16];
    stream.copy_from_slice(&hash.finalize().as_bytes()[..16]);
    if stream == [0; 16] || stream == primary.stream.0 {
        return Err("route seal stream collision");
    }
    Ok(OperationKey {
        stream: StreamId(stream),
        sequence: 1,
        ..primary
    })
}

/// Childless physical marker bytes for one exact view. Staging this object is
/// required before staging its route parent; a digest alone is not custody.
pub fn native_protected_view_marker_bytes_v1(view: ViewRef) -> Result<Vec<u8>, &'static str> {
    let bytes = serde_json::to_vec(&("qidx.native.protected-view.v1", view))
        .map_err(|_| "encode protected view marker")?;
    if bytes.is_empty() || bytes.len() > MAX_NATIVE_PROTECTED_VIEW_MARKER_BYTES_V1 {
        return Err("protected view marker exceeds bound");
    }
    Ok(bytes)
}

pub fn native_protected_view_marker_ref_v1(view: ViewRef) -> Result<ObjectRef, &'static str> {
    native_immutable_object_ref_v1(&native_protected_view_marker_bytes_v1(view)?, &[])
        .ok_or("protected view marker identity overflows")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeProtectedSourceHeadV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    /// Actual signed Source FULL/DELTA publication for this pair.
    pub source_operation: ImmutableOperation,
    pub source_publication: CatalogView,
    /// Current query head can be a derived AST/History publication.
    pub operation: ImmutableOperation,
    pub publication: CatalogView,
}
crate::native_serde_struct!(NativeProtectedSourceHeadV1 {
    repo_id: RepoId,
    revision_id: RevisionId,
    source_operation: ImmutableOperation,
    source_publication: CatalogView,
    operation: ImmutableOperation,
    publication: CatalogView,
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeProtectedRepoMapHeadV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    /// Independently verified active candidate under the held old Legacy root.
    pub active: RepoMapExpectedActiveV2,
    /// A real signed Source publication, or None only for this workspace's
    /// durable genesis with an empty SourceCut.
    pub source_operation: Option<ImmutableOperation>,
    pub source_publication: CatalogView,
    /// A query-visible Native publication containing the imported member.
    pub operation: ImmutableOperation,
    pub publication: CatalogView,
    pub instance: IndexInstanceId,
    pub member_binding: ObjectRef,
}

/// Physical provenance for an exact Native snapshot carry-over. These bytes
/// remain provisional until Q joins them to the independently held old root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeProtectedNativeOriginV1 {
    pub source_root_nonce: super::Digest,
    pub source_routes: ObjectRef,
    pub snapshot: ObjectRef,
}
crate::native_serde_struct!(NativeProtectedNativeOriginV1 {
    source_root_nonce: super::Digest,
    source_routes: ObjectRef,
    snapshot: ObjectRef,
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeProtectedSourceRoutesV1 {
    pub wire_version: u32,
    pub native_origin: Option<NativeProtectedNativeOriginV1>,
    pub heads: Vec<NativeProtectedSourceHeadV1>,
    pub repomap_heads: Vec<NativeProtectedRepoMapHeadV1>,
}

impl Serialize for NativeProtectedRepoMapHeadV1 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut record = serializer.serialize_struct("NativeProtectedRepoMapHeadV1", 9)?;
        record.serialize_field("repo_id", &self.repo_id)?;
        record.serialize_field("revision_id", &self.revision_id)?;
        record.serialize_field("active", &self.active)?;
        record.serialize_field("source_operation", &self.source_operation)?;
        record.serialize_field("source_publication", &self.source_publication)?;
        record.serialize_field("operation", &self.operation)?;
        record.serialize_field("publication", &self.publication)?;
        record.serialize_field("instance", &self.instance)?;
        record.serialize_field("member_binding", &self.member_binding)?;
        record.end()
    }
}

impl<'de> Deserialize<'de> for NativeProtectedRepoMapHeadV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const FIELDS: &[&str] = &[
            "repo_id",
            "revision_id",
            "active",
            "source_operation",
            "source_publication",
            "operation",
            "publication",
            "instance",
            "member_binding",
        ];
        struct WireVisitor;
        impl<'de> de::Visitor<'de> for WireVisitor {
            type Value = NativeProtectedRepoMapHeadV1;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a NativeProtectedRepoMapHeadV1 record")
            }

            fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut repo_id = None;
                let mut revision_id = None;
                let mut active = None;
                // An absent field and an explicit null both decode as None, but
                // a second occurrence must still be rejected.
                let mut source_operation: Option<Option<ImmutableOperation>> = None;
                let mut source_publication = None;
                let mut operation = None;
                let mut publication = None;
                let mut instance = None;
                let mut member_binding = None;
                while let Some(key) =
                    map.next_key_seed(quanta_index_contract_base::ClosedFieldKeySeedV1(FIELDS))?
                {
                    match key {
                        "repo_id" => {
                            if repo_id.is_some() {
                                return Err(de::Error::duplicate_field("repo_id"));
                            }
                            repo_id = Some(map.next_value::<RepoId>()?);
                        }
                        "revision_id" => {
                            if revision_id.is_some() {
                                return Err(de::Error::duplicate_field("revision_id"));
                            }
                            revision_id = Some(map.next_value::<RevisionId>()?);
                        }
                        "active" => {
                            if active.is_some() {
                                return Err(de::Error::duplicate_field("active"));
                            }
                            active = Some(map.next_value::<RepoMapExpectedActiveV2>()?);
                        }
                        "source_operation" => {
                            if source_operation.is_some() {
                                return Err(de::Error::duplicate_field("source_operation"));
                            }
                            source_operation =
                                Some(map.next_value::<Option<ImmutableOperation>>()?);
                        }
                        "source_publication" => {
                            if source_publication.is_some() {
                                return Err(de::Error::duplicate_field("source_publication"));
                            }
                            source_publication = Some(map.next_value::<CatalogView>()?);
                        }
                        "operation" => {
                            if operation.is_some() {
                                return Err(de::Error::duplicate_field("operation"));
                            }
                            operation = Some(map.next_value::<ImmutableOperation>()?);
                        }
                        "publication" => {
                            if publication.is_some() {
                                return Err(de::Error::duplicate_field("publication"));
                            }
                            publication = Some(map.next_value::<CatalogView>()?);
                        }
                        "instance" => {
                            if instance.is_some() {
                                return Err(de::Error::duplicate_field("instance"));
                            }
                            instance = Some(map.next_value::<IndexInstanceId>()?);
                        }
                        "member_binding" => {
                            if member_binding.is_some() {
                                return Err(de::Error::duplicate_field("member_binding"));
                            }
                            member_binding = Some(map.next_value::<ObjectRef>()?);
                        }
                        _ => unreachable!("closed field seed only returns schema keys"),
                    }
                }
                Ok(NativeProtectedRepoMapHeadV1 {
                    repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
                    revision_id: revision_id
                        .ok_or_else(|| de::Error::missing_field("revision_id"))?,
                    active: active.ok_or_else(|| de::Error::missing_field("active"))?,
                    source_operation: source_operation.unwrap_or(None),
                    source_publication: source_publication
                        .ok_or_else(|| de::Error::missing_field("source_publication"))?,
                    operation: operation.ok_or_else(|| de::Error::missing_field("operation"))?,
                    publication: publication
                        .ok_or_else(|| de::Error::missing_field("publication"))?,
                    instance: instance.ok_or_else(|| de::Error::missing_field("instance"))?,
                    member_binding: member_binding
                        .ok_or_else(|| de::Error::missing_field("member_binding"))?,
                })
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let repo_id = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let revision_id = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                let active = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(2, &self))?;
                let source_operation = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(3, &self))?;
                let source_publication = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(4, &self))?;
                let operation = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(5, &self))?;
                let publication = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(6, &self))?;
                let instance = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(7, &self))?;
                let member_binding = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(8, &self))?;
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::invalid_length(FIELDS.len() + 1, &self));
                }
                Ok(NativeProtectedRepoMapHeadV1 {
                    repo_id,
                    revision_id,
                    active,
                    source_operation,
                    source_publication,
                    operation,
                    publication,
                    instance,
                    member_binding,
                })
            }
        }
        deserializer.deserialize_struct("NativeProtectedRepoMapHeadV1", FIELDS, WireVisitor)
    }
}

impl Serialize for NativeProtectedSourceRoutesV1 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut record = serializer.serialize_struct(
            "NativeProtectedSourceRoutesV1",
            if self.native_origin.is_some() { 4 } else { 3 },
        )?;
        record.serialize_field("wire_version", &self.wire_version)?;
        if self.native_origin.is_some() {
            record.serialize_field("native_origin", &self.native_origin)?;
        } else {
            record.skip_field("native_origin")?;
        }
        record.serialize_field("heads", &self.heads)?;
        record.serialize_field("repomap_heads", &self.repomap_heads)?;
        record.end()
    }
}

impl<'de> Deserialize<'de> for NativeProtectedSourceRoutesV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const FIELDS: &[&str] = &["wire_version", "native_origin", "heads", "repomap_heads"];
        struct WireVisitor;
        impl<'de> de::Visitor<'de> for WireVisitor {
            type Value = NativeProtectedSourceRoutesV1;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a NativeProtectedSourceRoutesV1 record")
            }

            fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut wire_version = None;
                let mut native_origin: Option<Option<NativeProtectedNativeOriginV1>> = None;
                let mut heads = None;
                let mut repomap_heads = None;
                while let Some(key) =
                    map.next_key_seed(quanta_index_contract_base::ClosedFieldKeySeedV1(FIELDS))?
                {
                    match key {
                        "wire_version" => {
                            if wire_version.is_some() {
                                return Err(de::Error::duplicate_field("wire_version"));
                            }
                            wire_version = Some(map.next_value::<u32>()?);
                        }
                        "native_origin" => {
                            if native_origin.is_some() {
                                return Err(de::Error::duplicate_field("native_origin"));
                            }
                            native_origin =
                                Some(map.next_value::<Option<NativeProtectedNativeOriginV1>>()?);
                        }
                        "heads" => {
                            if heads.is_some() {
                                return Err(de::Error::duplicate_field("heads"));
                            }
                            heads = Some(
                                map.next_value::<BoundedVecV1<
                                    NativeProtectedSourceHeadV1,
                                    MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1,
                                >>()?
                                .into_inner(),
                            );
                        }
                        "repomap_heads" => {
                            if repomap_heads.is_some() {
                                return Err(de::Error::duplicate_field("repomap_heads"));
                            }
                            repomap_heads = Some(
                                map.next_value::<BoundedVecV1<
                                    NativeProtectedRepoMapHeadV1,
                                    MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1,
                                >>()?
                                .into_inner(),
                            );
                        }
                        _ => unreachable!("closed field seed only returns schema keys"),
                    }
                }
                Ok(NativeProtectedSourceRoutesV1 {
                    wire_version: wire_version
                        .ok_or_else(|| de::Error::missing_field("wire_version"))?,
                    native_origin: native_origin.unwrap_or(None),
                    heads: heads.ok_or_else(|| de::Error::missing_field("heads"))?,
                    repomap_heads: repomap_heads
                        .ok_or_else(|| de::Error::missing_field("repomap_heads"))?,
                })
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let wire_version = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let native_origin: Option<NativeProtectedNativeOriginV1> =
                    seq.next_element()?.unwrap_or(None);
                let heads =
                    seq.next_element::<BoundedVecV1<
                        NativeProtectedSourceHeadV1,
                        MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1,
                    >>()?
                    .ok_or_else(|| de::Error::invalid_length(2, &self))?
                    .into_inner();
                let repomap_heads = seq
                    .next_element::<BoundedVecV1<
                        NativeProtectedRepoMapHeadV1,
                        MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1,
                    >>()?
                    .ok_or_else(|| de::Error::invalid_length(3, &self))?
                    .into_inner();
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::invalid_length(FIELDS.len() + 1, &self));
                }
                Ok(NativeProtectedSourceRoutesV1 {
                    wire_version,
                    native_origin,
                    heads,
                    repomap_heads,
                })
            }
        }
        deserializer.deserialize_struct("NativeProtectedSourceRoutesV1", FIELDS, WireVisitor)
    }
}

fn order_unique_workspaces(
    order: &mut [u16],
    workspace: impl Fn(usize) -> Option<WorkspaceKey>,
) -> bool {
    for (index, slot) in order.iter_mut().enumerate() {
        if workspace(index).is_none() {
            return false;
        }
        let Ok(index) = u16::try_from(index) else {
            return false;
        };
        *slot = index;
    }
    order.sort_unstable_by_key(|index| workspace(usize::from(*index)));
    order
        .iter()
        .zip(order.iter().skip(1))
        .all(|(left, right)| workspace(usize::from(*left)) != workspace(usize::from(*right)))
}

impl NativeProtectedSourceRoutesV1 {
    pub const WIRE_VERSION: u32 = 1;

    /// Validate the complete deterministic identity set before it is staged.
    pub fn validate(&self) -> Result<(), &'static str> {
        self.validate_with_work_v1(&mut || Ok::<(), Infallible>(()))
            .map_err(NativeProtectedEncodingErrorV1::into_format_error)
    }

    fn validate_with_work_v1<E>(
        &self,
        checkpoint: &mut impl FnMut() -> Result<(), E>,
    ) -> Result<(), NativeProtectedEncodingErrorV1<E>> {
        let format = NativeProtectedEncodingErrorV1::Format;
        checkpoint().map_err(NativeProtectedEncodingErrorV1::Work)?;
        if self.wire_version != Self::WIRE_VERSION
            || (self.heads.is_empty() && self.repomap_heads.is_empty())
            || self.heads.len() > MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1
            || self.repomap_heads.len() > MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1
        {
            return Err(format("protected Source route version or count is invalid"));
        }
        if self.native_origin.as_ref().is_some_and(|origin| {
            origin.source_root_nonce == super::Digest([0; 32])
                || origin.source_routes.0 == super::Digest([0; 32])
                || origin.snapshot.0 == super::Digest([0; 32])
        }) {
            return Err(format("protected Native origin is invalid"));
        }
        let mut previous_pair = None;
        for head in &self.heads {
            checkpoint().map_err(NativeProtectedEncodingErrorV1::Work)?;
            let pair = (head.repo_id.as_str(), head.revision_id.as_str());
            if previous_pair.is_some_and(|previous| previous >= pair)
                || head.operation.key.workspace != head.publication.identity.workspace
                || head.operation.key.epoch != head.publication.identity.epoch
                || head.source_operation.key.workspace != head.operation.key.workspace
                || head.source_operation.key.epoch != head.source_publication.identity.epoch
                || head.source_publication.identity.workspace != head.operation.key.workspace
                || head.source_publication.identity.epoch != head.operation.key.epoch
                || head.source_publication.source != head.publication.source
                || (
                    head.source_publication.identity.epoch,
                    head.source_publication.identity.sequence,
                ) > (
                    head.publication.identity.epoch,
                    head.publication.identity.sequence,
                )
                || head.publication.identity_digest().ok() != Some(head.publication.identity.digest)
                || head.source_publication.identity_digest().ok()
                    != Some(head.source_publication.identity.digest)
            {
                return Err(format("protected Source route scope or order is invalid"));
            }
            previous_pair = Some(pair);
        }
        // Sort only bounded indices. The canonical pair order and its wire
        // bytes remain intact, and validation does not allocate a workspace map.
        let mut source_order = [0; MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1];
        let source_order = source_order
            .get_mut(..self.heads.len())
            .ok_or(format("protected Source route scope or order is invalid"))?;
        let source_workspace = |index: usize| {
            self.heads
                .get(index)
                .map(|head| head.operation.key.workspace)
        };
        if !order_unique_workspaces(source_order, source_workspace) {
            return Err(format("protected Source route scope or order is invalid"));
        }
        let mut previous_repomap_pair = None;
        for head in &self.repomap_heads {
            checkpoint().map_err(NativeProtectedEncodingErrorV1::Work)?;
            let pair = (head.repo_id.as_str(), head.revision_id.as_str());
            if previous_repomap_pair.is_some_and(|previous| previous >= pair)
                || head.member_binding.0 == super::Digest([0; 32])
                || head.publication.members.0.is_none()
                || head.operation.key.workspace != head.publication.identity.workspace
                || head.operation.key.epoch != head.publication.identity.epoch
                || head.source_operation.as_ref().is_some_and(|source| {
                    source.key.workspace != head.operation.key.workspace
                        || source.key.epoch != head.source_publication.identity.epoch
                        || head.source_publication.identity.sequence == 0
                })
                || (head.source_operation.is_none()
                    && (head.source_publication.identity.sequence != 0
                        || head.source_publication.source.owners.0.is_some()
                        || head.source_publication.source.paths.0.is_some()
                        || head.source_publication.source.frontiers.0.is_some()
                        || head.source_publication.projections.0.is_some()
                        || head.source_publication.members.0.is_some()
                        || head.source_publication.coverage.0.is_some()))
                || head.source_publication.identity.workspace != head.operation.key.workspace
                || head.source_publication.identity.epoch != head.operation.key.epoch
                || head.source_publication.source != head.publication.source
                || (
                    head.source_publication.identity.epoch,
                    head.source_publication.identity.sequence,
                ) > (
                    head.publication.identity.epoch,
                    head.publication.identity.sequence,
                )
                || head.publication.identity_digest().ok() != Some(head.publication.identity.digest)
                || head.source_publication.identity_digest().ok()
                    != Some(head.source_publication.identity.digest)
            {
                return Err(format("protected RepoMap route order is invalid"));
            }
            let inconsistent_source = match self.heads.binary_search_by(|source| {
                (source.repo_id.as_str(), source.revision_id.as_str()).cmp(&pair)
            }) {
                Ok(index) => {
                    let source = self
                        .heads
                        .get(index)
                        .ok_or(format("protected RepoMap route order is invalid"))?;
                    source.operation.key.workspace != head.operation.key.workspace
                        || source.publication != head.publication
                        || head.source_operation.as_ref() != Some(&source.source_operation)
                        || head.source_publication != source.source_publication
                }
                Err(_insertion) => source_order
                    .binary_search_by_key(&Some(head.operation.key.workspace), |index| {
                        source_workspace(usize::from(*index))
                    })
                    .is_ok(),
            };
            if inconsistent_source {
                return Err(format("protected RepoMap route order is invalid"));
            }
            previous_repomap_pair = Some(pair);
        }
        let mut repomap_order = [0; MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1];
        let repomap_order = repomap_order
            .get_mut(..self.repomap_heads.len())
            .ok_or(format("protected RepoMap route order is invalid"))?;
        if !order_unique_workspaces(repomap_order, |index| {
            self.repomap_heads
                .get(index)
                .map(|head| head.operation.key.workspace)
        }) {
            return Err(format("protected RepoMap route order is invalid"));
        }
        Ok(())
    }

    /// Canonical direct children keep every protected head's original boot
    /// input reachable from the route object. The store still verifies bytes.
    pub fn children(&self) -> Result<Vec<ObjectRef>, &'static str> {
        self.children_with_work_v1(&mut || Ok::<(), Infallible>(()))
            .map_err(NativeProtectedEncodingErrorV1::into_format_error)
    }

    pub fn children_with_work_v1<E>(
        &self,
        checkpoint: &mut impl FnMut() -> Result<(), E>,
    ) -> Result<Vec<ObjectRef>, NativeProtectedEncodingErrorV1<E>> {
        self.validate_with_work_v1(checkpoint)?;
        self.validated_children_with_work_v1(checkpoint)
    }

    /// Storage required for the canonical child list before duplicate removal.
    /// The caller budgets marker serialization scratch separately.
    pub fn children_backing_bytes(&self) -> Result<usize, &'static str> {
        self.child_slots()?
            .checked_mul(std::mem::size_of::<ObjectRef>())
            .ok_or("protected Source route child backing overflows")
    }

    fn child_slots(&self) -> Result<usize, &'static str> {
        if self.heads.len() > MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1
            || self.repomap_heads.len() > MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1
        {
            return Err("protected Source route version or count is invalid");
        }
        self.heads
            .len()
            .checked_mul(11)
            .and_then(|source| source.checked_add(self.repomap_heads.len().checked_mul(12)?))
            .and_then(|slots| slots.checked_add(2))
            .ok_or("protected Source route child backing overflows")
    }

    fn validated_children_with_work_v1<E>(
        &self,
        checkpoint: &mut impl FnMut() -> Result<(), E>,
    ) -> Result<Vec<ObjectRef>, NativeProtectedEncodingErrorV1<E>> {
        let format = NativeProtectedEncodingErrorV1::Format;
        checkpoint().map_err(NativeProtectedEncodingErrorV1::Work)?;
        let mut children = Vec::new();
        children
            .try_reserve_exact(self.child_slots().map_err(format)?)
            .map_err(|_allocation| format("protected Source route child allocation refused"))?;
        if let Some(origin) = &self.native_origin {
            children.extend([origin.source_routes, origin.snapshot]);
        }
        for head in &self.heads {
            checkpoint().map_err(NativeProtectedEncodingErrorV1::Work)?;
            children.push(head.operation.request);
            children.push(head.source_operation.request);
            children.push(head.publication.profile);
            children.push(head.source_publication.profile);
            children.extend(head.publication.source.owners.0);
            children.extend(head.publication.source.paths.0);
            children.extend(head.publication.source.frontiers.0);
            children.extend(head.publication.projections.0);
            children.extend(head.publication.members.0);
            children.extend(head.publication.coverage.0);
            children.push(
                native_protected_view_marker_ref_v1(head.publication.identity).map_err(format)?,
            );
        }
        for head in &self.repomap_heads {
            checkpoint().map_err(NativeProtectedEncodingErrorV1::Work)?;
            children.push(head.operation.request);
            if let Some(source) = &head.source_operation {
                children.push(source.request);
            }
            children.push(head.publication.profile);
            children.push(head.source_publication.profile);
            children.extend(head.publication.source.owners.0);
            children.extend(head.publication.source.paths.0);
            children.extend(head.publication.source.frontiers.0);
            children.extend(head.publication.projections.0);
            children.extend(head.publication.members.0);
            children.extend(head.publication.coverage.0);
            children.push(head.member_binding);
            children.push(
                native_protected_view_marker_ref_v1(head.publication.identity).map_err(format)?,
            );
            if head.source_operation.is_none() {
                children.push(
                    native_protected_view_marker_ref_v1(head.source_publication.identity)
                        .map_err(format)?,
                );
            }
        }
        checkpoint().map_err(NativeProtectedEncodingErrorV1::Work)?;
        children.sort_unstable();
        children.dedup();
        if children.len() > crate::native_protocol::MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN {
            let origin = self
                .native_origin
                .as_ref()
                .ok_or(format("protected Source route has too many children"))?;
            // A Native snapshot retains the old route as one of its exact
            // roots. That route already owns the head roots and markers, so
            // the two origin edges preserve the same physical closure when
            // the complete direct set exceeds the object child bound.
            let mut origin_children = vec![origin.source_routes, origin.snapshot];
            origin_children.sort_unstable();
            origin_children.dedup();
            return Ok(origin_children);
        }
        Ok(children)
    }

    pub fn encoded_payload_len(&self) -> Result<usize, &'static str> {
        self.validate()?;
        self.encoded_payload_len_validated_with_work_v1(&mut || Ok::<(), Infallible>(()))
            .map_err(NativeProtectedEncodingErrorV1::into_format_error)
    }

    fn encoded_payload_len_validated_with_work_v1<E>(
        &self,
        checkpoint: &mut impl FnMut() -> Result<(), E>,
    ) -> Result<usize, NativeProtectedEncodingErrorV1<E>> {
        struct Counter(usize);
        impl std::io::Write for Counter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0 = self
                    .0
                    .checked_add(bytes.len())
                    .ok_or_else(|| std::io::Error::other("route byte count overflow"))?;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut count = Counter(0);
        serialize_protected_with_work_v1(
            self,
            &mut count,
            checkpoint,
            "count protected Source routes",
        )?;
        if count.0 == 0 || count.0 > MAX_NATIVE_PROTECTED_SOURCE_ROUTE_BYTES_V1 {
            return Err(NativeProtectedEncodingErrorV1::Format(
                "protected Source route payload exceeds bound",
            ));
        }
        Ok(count.0)
    }

    pub fn object_ref(&self) -> Result<ObjectRef, &'static str> {
        self.object_identity_with_work_v1(&mut || Ok::<(), Infallible>(()))
            .map(|(reference, _, _)| reference)
            .map_err(NativeProtectedEncodingErrorV1::into_format_error)
    }

    /// Validate once, then return the exact immutable-object identity and its
    /// canonical child list without materializing the route JSON payload.
    pub fn object_identity_with_work_v1<E>(
        &self,
        checkpoint: &mut impl FnMut() -> Result<(), E>,
    ) -> Result<(ObjectRef, usize, Vec<ObjectRef>), NativeProtectedEncodingErrorV1<E>> {
        self.validate_with_work_v1(checkpoint)?;
        let size = self.encoded_payload_len_validated_with_work_v1(checkpoint)?;
        let children = self.validated_children_with_work_v1(checkpoint)?;
        struct HashWriter {
            digest: Sha256,
            remaining: usize,
        }
        impl std::io::Write for HashWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.remaining = self
                    .remaining
                    .checked_sub(bytes.len())
                    .ok_or_else(|| std::io::Error::other("route hash exceeds counted payload"))?;
                self.digest.update(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut output = HashWriter {
            digest: native_immutable_object_hasher_v1(size, &children).ok_or(
                NativeProtectedEncodingErrorV1::Format(
                    "protected Source route object identity overflows",
                ),
            )?,
            remaining: size,
        };
        serialize_protected_with_work_v1(
            self,
            &mut output,
            checkpoint,
            "encode protected Source routes",
        )?;
        if output.remaining != 0 {
            return Err(NativeProtectedEncodingErrorV1::Format(
                "protected Source route payload changed during encode",
            ));
        }
        Ok((
            ObjectRef(Digest(output.digest.finalize().into())),
            size,
            children,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::*;

    #[test]
    fn route_seal_key_matches_original_derivation() {
        let primary = OperationKey {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 8,
            producer: ProducerId([5; 16]),
            stream: StreamId([6; 16]),
            stream_epoch: 9,
            sequence: 10,
        };
        let nonce = Digest([7; 32]);
        let seal = native_protected_source_route_seal_key_v1(primary, nonce).expect("seal key");
        assert_eq!(
            seal.stream,
            StreamId([
                148, 156, 94, 175, 17, 197, 233, 39, 22, 171, 120, 200, 182, 10, 32, 57
            ])
        );
        assert_eq!(seal.sequence, 1);
        assert_eq!(seal.workspace, primary.workspace);
        assert_eq!(seal.epoch, primary.epoch);
        assert_eq!(seal.producer, primary.producer);
        assert_eq!(seal.stream_epoch, primary.stream_epoch);
        assert_eq!(
            native_protected_source_route_seal_key_v1(primary, nonce),
            Ok(seal)
        );
        assert_ne!(
            native_protected_source_route_seal_key_v1(primary, Digest([8; 32])).expect("new root"),
            seal
        );
        assert_eq!(
            native_protected_source_route_seal_key_v1(
                OperationKey {
                    sequence: 0,
                    ..primary
                },
                nonce
            ),
            Err("route seal scope is invalid")
        );
        assert_eq!(
            native_protected_source_route_seal_key_v1(primary, Digest([0; 32])),
            Err("route seal scope is invalid")
        );
    }

    fn head(repo: &str, revision: &str, seed: u8) -> NativeProtectedSourceHeadV1 {
        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([seed; 16]),
            slot: SlotId([3; 16]),
        };
        let operation = ImmutableOperation {
            key: OperationKey {
                workspace,
                epoch: 1,
                producer: ProducerId([4; 16]),
                stream: StreamId([5; 16]),
                stream_epoch: 1,
                sequence: 1,
            },
            request: ObjectRef(Digest([seed; 32])),
            source_mode: SourceOperationMode::Advance,
        };
        let mut publication = CatalogView {
            identity: ViewRef {
                workspace,
                epoch: 1,
                sequence: 1,
                digest: Digest([0; 32]),
            },
            profile: ObjectRef(Digest([seed.wrapping_add(1); 32])),
            source: SourceCut {
                owners: MapRoot(Some(ObjectRef(Digest([seed.wrapping_add(2); 32])))),
                paths: MapRoot(None),
                frontiers: MapRoot(None),
            },
            projections: MapRoot(Some(ObjectRef(Digest([seed.wrapping_add(3); 32])))),
            members: MapRoot(None),
            coverage: MapRoot(None),
        };
        publication.identity.digest = publication.identity_digest().expect("identity");
        NativeProtectedSourceHeadV1 {
            repo_id: RepoId::new(repo).expect("repo"),
            revision_id: RevisionId::new(revision).expect("revision"),
            source_operation: operation.clone(),
            source_publication: publication.clone(),
            operation,
            publication,
        }
    }

    fn dense_object(index: u16) -> ObjectRef {
        let mut bytes = [0; 32];
        bytes[..2].copy_from_slice(&index.to_be_bytes());
        bytes[2] = 1;
        ObjectRef(Digest(bytes))
    }

    fn dense_head(index: u16) -> NativeProtectedSourceHeadV1 {
        let mut head = head(&format!("repo-{index:04}"), "revision", 1);
        let mut workspace = head.operation.key.workspace;
        let mut worktree = [0; 16];
        worktree[..2].copy_from_slice(&(index + 1).to_be_bytes());
        workspace.worktree = WorktreeId(worktree);
        head.operation.key.workspace = workspace;
        head.operation.key.sequence = 2;
        head.source_operation.key.workspace = workspace;
        head.source_operation.key.sequence = 1;
        head.publication.identity.workspace = workspace;
        head.publication.identity.sequence = 2;
        head.source_publication.identity.workspace = workspace;
        head.source_publication.identity.sequence = 1;
        let base = index * 16 + 1;
        head.operation.request = dense_object(base);
        head.source_operation.request = dense_object(base + 1);
        head.publication.profile = dense_object(base + 2);
        head.source_publication.profile = dense_object(base + 3);
        head.publication.source.owners = MapRoot(Some(dense_object(base + 4)));
        head.publication.source.paths = MapRoot(Some(dense_object(base + 5)));
        head.publication.source.frontiers = MapRoot(Some(dense_object(base + 6)));
        head.source_publication.source = head.publication.source;
        head.publication.projections = MapRoot(Some(dense_object(base + 7)));
        head.publication.members = MapRoot(Some(dense_object(base + 8)));
        head.publication.coverage = MapRoot(Some(dense_object(base + 9)));
        head.publication.identity.digest = head.publication.identity_digest().expect("query view");
        head.source_publication.identity.digest = head
            .source_publication
            .identity_digest()
            .expect("source view");
        head
    }

    fn dense_routes(repomap_count: u16) -> NativeProtectedSourceRoutesV1 {
        let heads = (0..MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1)
            .map(|index| dense_head(u16::try_from(index).expect("head index")))
            .collect();
        let repomap_heads = (0..repomap_count)
            .map(|offset| {
                let index = u16::try_from(MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1)
                    .expect("source count")
                    + offset;
                let source = dense_head(index);
                NativeProtectedRepoMapHeadV1 {
                    repo_id: source.repo_id,
                    revision_id: source.revision_id,
                    active: RepoMapExpectedActiveV2::new(
                        std::num::NonZeroU64::MIN,
                        crate::CandidateCommitmentV1::from_bytes([80; 32]),
                    ),
                    source_operation: Some(source.source_operation),
                    source_publication: source.source_publication,
                    operation: source.operation,
                    publication: source.publication,
                    instance: IndexInstanceId([81; 16]),
                    member_binding: dense_object(index * 16 + 11),
                }
            })
            .collect();
        NativeProtectedSourceRoutesV1 {
            native_origin: None,
            wire_version: NativeProtectedSourceRoutesV1::WIRE_VERSION,
            heads,
            repomap_heads,
        }
    }

    #[test]
    fn native_origin_overflow_uses_only_exact_origin_edges() {
        let mut source = dense_routes(4);
        source.validate().expect("complete source routes");
        assert_eq!(
            source.children().expect("physical source route").len(),
            4096
        );
        let source_route = source.object_ref().expect("source route identity");
        let snapshot = dense_object(65000);
        let origin = NativeProtectedNativeOriginV1 {
            source_root_nonce: Digest([1; 32]),
            source_routes: source_route,
            snapshot,
        };
        let mut prior_shape = dense_routes(3);
        prior_shape.native_origin = Some(origin.clone());
        let direct = prior_shape.children().expect("old direct shape");
        assert_eq!(direct.len(), 4086);
        assert!(direct.contains(&prior_shape.heads[0].operation.request));
        let prior_bytes = serde_json::to_vec(&prior_shape).expect("old route payload");
        assert_eq!(
            native_immutable_object_ref_v1(&prior_bytes, &direct),
            Some(prior_shape.object_ref().expect("old route identity"))
        );

        source.native_origin = Some(origin);
        let mut expected = vec![source_route, snapshot];
        expected.sort_unstable();
        assert_eq!(source.children().expect("compressed edges"), expected);
        let payload = serde_json::to_vec(&source).expect("compressed route payload");
        assert_eq!(
            native_immutable_object_ref_v1(&payload, &expected),
            Some(source.object_ref().expect("compressed route identity"))
        );

        let extra = dense_routes(5);
        assert_eq!(
            extra.children(),
            Err("protected Source route has too many children")
        );
    }

    #[test]
    fn protected_routes_bind_exact_views_and_complete_direct_children() {
        let first = head("repo", "rev-a", 10);
        let second = head("repo", "rev-b", 20);
        let routes = NativeProtectedSourceRoutesV1 {
            native_origin: None,
            wire_version: 1,
            heads: vec![first.clone(), second.clone()],
            repomap_heads: vec![],
        };
        routes.validate().expect("ordered distinct heads");
        let children = routes.children().expect("canonical children");
        assert!(children.contains(&first.publication.projections.0.expect("projection")));
        assert!(children.contains(&second.publication.projections.0.expect("projection")));
        assert!(children.contains(
            &native_protected_view_marker_ref_v1(first.publication.identity).expect("marker")
        ));
        assert!(children.contains(
            &native_protected_view_marker_ref_v1(second.publication.identity).expect("marker")
        ));
        let identity = routes.object_ref().expect("route identity");
        let payload = serde_json::to_vec(&routes).expect("canonical route payload");
        assert_eq!(
            native_immutable_object_ref_v1(&payload, &children),
            Some(identity)
        );
        let mut changed = routes.clone();
        changed.heads[1].publication.identity.sequence = 2;
        changed.heads[1].publication.identity.digest = changed.heads[1]
            .publication
            .identity_digest()
            .expect("changed identity");
        assert_ne!(changed.object_ref().expect("changed route"), identity);
        let mut duplicate_pair = routes.clone();
        duplicate_pair.heads[1].revision_id = duplicate_pair.heads[0].revision_id.clone();
        assert!(duplicate_pair.validate().is_err());
        let mut duplicate_workspace = routes;
        *duplicate_workspace
            .heads
            .get_mut(1)
            .expect("second Source head") = head("repo", "rev-b", 10);
        assert_eq!(
            duplicate_workspace.validate(),
            Err("protected Source route scope or order is invalid")
        );
    }

    #[test]
    fn route_identity_work_interrupts_and_matches_materialized_object() {
        let routes = NativeProtectedSourceRoutesV1 {
            native_origin: None,
            wire_version: NativeProtectedSourceRoutesV1::WIRE_VERSION,
            heads: vec![head("repo", "revision", 10)],
            repomap_heads: Vec::new(),
        };
        let mut calls = 0;
        assert_eq!(
            routes.object_identity_with_work_v1(&mut || {
                calls += 1;
                Err::<(), _>("interrupted")
            }),
            Err(NativeProtectedEncodingErrorV1::Work("interrupted"))
        );
        assert_eq!(calls, 1);

        let mut validation_calls = 0;
        routes
            .validate_with_work_v1(&mut || {
                validation_calls += 1;
                Ok::<(), &'static str>(())
            })
            .expect("route validation");
        let mut calls = 0;
        let count_pass = validation_calls + 1;
        assert_eq!(
            routes.object_identity_with_work_v1(&mut || {
                calls += 1;
                if calls == count_pass {
                    Err("count interrupted")
                } else {
                    Ok(())
                }
            }),
            Err(NativeProtectedEncodingErrorV1::Work("count interrupted"))
        );
        assert_eq!(calls, count_pass);

        let mut calls = 0;
        let (reference, size, children) = routes
            .object_identity_with_work_v1(&mut || {
                calls += 1;
                Ok::<(), &'static str>(())
            })
            .expect("route identity");
        let payload = serde_json::to_vec(&routes).expect("route payload");
        assert!(calls > 2);
        assert_eq!(size, payload.len());
        assert_eq!(children, routes.children().expect("route children"));
        assert_eq!(
            Some(reference),
            native_immutable_object_ref_v1(&payload, &children)
        );
        assert_eq!(reference, routes.object_ref().expect("pure route identity"));
    }

    #[test]
    fn repomap_shares_a_source_workspace_only_for_the_exact_pair() {
        let mut source = head("repo", "rev-a", 10);
        let member_binding = ObjectRef(Digest([79; 32]));
        source.publication.members = MapRoot(Some(member_binding));
        source.publication.identity.digest = source.publication.identity_digest().expect("view");
        let repomap = NativeProtectedRepoMapHeadV1 {
            repo_id: source.repo_id.clone(),
            revision_id: source.revision_id.clone(),
            active: RepoMapExpectedActiveV2::new(
                std::num::NonZeroU64::MIN,
                crate::CandidateCommitmentV1::from_bytes([80; 32]),
            ),
            source_operation: Some(source.source_operation),
            source_publication: source.source_publication.clone(),
            operation: source.operation,
            publication: source.publication.clone(),
            instance: IndexInstanceId([81; 16]),
            member_binding,
        };
        let mut routes = NativeProtectedSourceRoutesV1 {
            native_origin: None,
            wire_version: 1,
            heads: vec![source],
            repomap_heads: vec![repomap],
        };
        routes.validate().expect("exact shared route");
        routes
            .repomap_heads
            .first_mut()
            .expect("RepoMap head")
            .revision_id = RevisionId::new("rev-b").expect("revision");
        assert_eq!(
            routes.validate(),
            Err("protected RepoMap route order is invalid")
        );
    }

    #[test]
    fn route_decoder_rejects_oversized_source_heads_during_deserialization() {
        let routes = NativeProtectedSourceRoutesV1 {
            native_origin: None,
            wire_version: 1,
            heads: vec![head("repo", "rev-a", 10); MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1 + 1],
            repomap_heads: vec![],
        };
        let encoded = serde_json::to_vec(&routes).expect("oversized Source heads fixture");
        let error = serde_json::from_slice::<NativeProtectedSourceRoutesV1>(&encoded)
            .expect_err("oversized heads decoded");
        assert!(error.to_string().contains("bounded sequence elements"));
    }

    #[test]
    fn route_wire_preserves_omitted_origin_and_closed_shape() {
        const CANONICAL: &str = r#"{"wire_version":1,"heads":[],"repomap_heads":[]}"#;
        let route: NativeProtectedSourceRoutesV1 =
            serde_json::from_str(CANONICAL).expect("legacy route map");
        assert_eq!(route.native_origin, None);
        assert_eq!(
            serde_json::to_string(&route).expect("route JSON"),
            CANONICAL
        );
        let explicit_null =
            r#"{"wire_version":1,"native_origin":null,"heads":[],"repomap_heads":[]}"#;
        assert_eq!(
            serde_json::from_str::<NativeProtectedSourceRoutesV1>(explicit_null)
                .expect("explicit null origin"),
            route
        );
        assert_eq!(
            serde_json::from_str::<NativeProtectedSourceRoutesV1>("[1,null,[],[]]")
                .expect("positional route"),
            route
        );
        let duplicate_null = r#"{"wire_version":1,"native_origin":null,"native_origin":null,"heads":[],"repomap_heads":[]}"#;
        assert!(
            serde_json::from_str::<NativeProtectedSourceRoutesV1>(duplicate_null)
                .expect_err("duplicate origin")
                .to_string()
                .contains("duplicate field")
        );
        let unknown = r#"{"wire_version":1,"heads":[],"repomap_heads":[],"extra":0}"#;
        assert!(serde_json::from_str::<NativeProtectedSourceRoutesV1>(unknown).is_err());
    }

    #[test]
    fn repomap_head_wire_preserves_optional_map_field_and_required_sequence_slot() {
        let source = head("repo", "revision", 10);
        let repomap = NativeProtectedRepoMapHeadV1 {
            repo_id: source.repo_id,
            revision_id: source.revision_id,
            active: RepoMapExpectedActiveV2::new(
                std::num::NonZeroU64::MIN,
                crate::CandidateCommitmentV1::from_bytes([80; 32]),
            ),
            source_operation: None,
            source_publication: source.source_publication,
            operation: source.operation,
            publication: source.publication,
            instance: IndexInstanceId([81; 16]),
            member_binding: ObjectRef(Digest([79; 32])),
        };
        let mut fields = serde_json::to_value(&repomap).expect("RepoMap head JSON");
        let map = fields.as_object_mut().expect("RepoMap object");
        assert_eq!(map.get("source_operation"), Some(&serde_json::Value::Null));
        let mut sequence = Vec::new();
        for field in [
            "repo_id",
            "revision_id",
            "active",
            "source_operation",
            "source_publication",
            "operation",
            "publication",
            "instance",
            "member_binding",
        ] {
            sequence.push(map.get(field).expect("named field").clone());
        }
        assert_eq!(
            map.remove("source_operation"),
            Some(serde_json::Value::Null)
        );
        assert_eq!(
            serde_json::from_value::<NativeProtectedRepoMapHeadV1>(fields.clone())
                .expect("omitted nullable map field"),
            repomap
        );
        assert_eq!(
            serde_json::from_value::<NativeProtectedRepoMapHeadV1>(serde_json::Value::Array(
                sequence.clone()
            ))
            .expect("positional RepoMap head"),
            repomap
        );
        assert_eq!(sequence.remove(3), serde_json::Value::Null);
        assert!(
            serde_json::from_value::<NativeProtectedRepoMapHeadV1>(serde_json::Value::Array(
                sequence
            ))
            .is_err()
        );
        let canonical = serde_json::to_string(&repomap).expect("RepoMap head bytes");
        let duplicate_null = canonical.replacen(
            "\"source_operation\":null,",
            "\"source_operation\":null,\"source_operation\":null,",
            1,
        );
        assert_ne!(duplicate_null, canonical);
        assert!(serde_json::from_str::<NativeProtectedRepoMapHeadV1>(&duplicate_null).is_err());
    }
}
