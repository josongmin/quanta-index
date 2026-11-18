//! A bounded, immutable route set for preserved Source and RepoMap heads.
//! These bytes describe expected publications; they confer no producer authority.

use super::{
    CatalogView, ImmutableOperation, IndexInstanceId, ObjectRef, ViewRef, WorkspaceKey,
    native_immutable_object_ref_v1,
};
use crate::{RepoId, RevisionId, repomap::RepoMapExpectedActiveV2};
use serde::{Deserialize, Serialize};

pub const MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1: usize = 368;
pub const MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1: usize = 4_096;
pub const MAX_NATIVE_PROTECTED_SOURCE_ROUTE_BYTES_V1: usize = 16 * 1024 * 1024;
pub const MAX_NATIVE_PROTECTED_VIEW_MARKER_BYTES_V1: usize = 1024;

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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeProtectedNativeOriginV1 {
    pub source_root_nonce: super::Digest,
    pub source_routes: ObjectRef,
    pub snapshot: ObjectRef,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeProtectedSourceRoutesV1 {
    pub wire_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_origin: Option<NativeProtectedNativeOriginV1>,
    pub heads: Vec<NativeProtectedSourceHeadV1>,
    pub repomap_heads: Vec<NativeProtectedRepoMapHeadV1>,
}

fn order_unique_workspaces(order: &mut [u16], workspace: impl Fn(usize) -> WorkspaceKey) -> bool {
    for (index, slot) in order.iter_mut().enumerate() {
        let Ok(index) = u16::try_from(index) else {
            return false;
        };
        *slot = index;
    }
    order.sort_unstable_by_key(|index| workspace(usize::from(*index)));
    order
        .windows(2)
        .all(|pair| workspace(usize::from(pair[0])) != workspace(usize::from(pair[1])))
}

impl NativeProtectedSourceRoutesV1 {
    pub const WIRE_VERSION: u32 = 1;

    /// Validate the complete deterministic identity set before it is staged.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.wire_version != Self::WIRE_VERSION
            || (self.heads.is_empty() && self.repomap_heads.is_empty())
            || self.heads.len() > MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1
            || self.repomap_heads.len() > MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1
        {
            return Err("protected Source route version or count is invalid");
        }
        if self.native_origin.as_ref().is_some_and(|origin| {
            origin.source_root_nonce == super::Digest([0; 32])
                || origin.source_routes.0 == super::Digest([0; 32])
                || origin.snapshot.0 == super::Digest([0; 32])
        }) {
            return Err("protected Native origin is invalid");
        }
        let mut previous_pair = None;
        for head in &self.heads {
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
                return Err("protected Source route scope or order is invalid");
            }
            previous_pair = Some(pair);
        }
        // Sort only bounded indices. The canonical pair order and its wire
        // bytes remain intact, and validation does not allocate a workspace map.
        let mut source_order = [0; MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1];
        let source_order = &mut source_order[..self.heads.len()];
        let source_workspace = |index: usize| self.heads[index].operation.key.workspace;
        if !order_unique_workspaces(source_order, source_workspace) {
            return Err("protected Source route scope or order is invalid");
        }
        let mut previous_repomap_pair = None;
        for head in &self.repomap_heads {
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
                return Err("protected RepoMap route order is invalid");
            }
            let inconsistent_source = match self.heads.binary_search_by(|source| {
                (source.repo_id.as_str(), source.revision_id.as_str()).cmp(&pair)
            }) {
                Ok(index) => {
                    let source = &self.heads[index];
                    source.operation.key.workspace != head.operation.key.workspace
                        || source.publication != head.publication
                        || head.source_operation.as_ref() != Some(&source.source_operation)
                        || head.source_publication != source.source_publication
                }
                Err(_) => source_order
                    .binary_search_by_key(&head.operation.key.workspace, |index| {
                        source_workspace(usize::from(*index))
                    })
                    .is_ok(),
            };
            if inconsistent_source {
                return Err("protected RepoMap route order is invalid");
            }
            previous_repomap_pair = Some(pair);
        }
        let mut repomap_order = [0; MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1];
        if !order_unique_workspaces(&mut repomap_order[..self.repomap_heads.len()], |index| {
            self.repomap_heads[index].operation.key.workspace
        }) {
            return Err("protected RepoMap route order is invalid");
        }
        Ok(())
    }

    /// Canonical direct children keep every protected head's original boot
    /// input reachable from the route object. The store still verifies bytes.
    pub fn children(&self) -> Result<Vec<ObjectRef>, &'static str> {
        self.validate()?;
        let mut children = Vec::new();
        if let Some(origin) = &self.native_origin {
            children.extend([origin.source_routes, origin.snapshot]);
        }
        for head in &self.heads {
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
            children.push(native_protected_view_marker_ref_v1(
                head.publication.identity,
            )?);
        }
        for head in &self.repomap_heads {
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
            children.push(native_protected_view_marker_ref_v1(
                head.publication.identity,
            )?);
            if head.source_operation.is_none() {
                children.push(native_protected_view_marker_ref_v1(
                    head.source_publication.identity,
                )?);
            }
        }
        children.sort_unstable();
        children.dedup();
        if children.len() > crate::native_protocol::MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN {
            return Err("protected Source route has too many children");
        }
        Ok(children)
    }

    pub fn encoded_payload_len(&self) -> Result<usize, &'static str> {
        self.validate()?;
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
        serde_json::to_writer(&mut count, self).map_err(|_| "count protected Source routes")?;
        if count.0 == 0 || count.0 > MAX_NATIVE_PROTECTED_SOURCE_ROUTE_BYTES_V1 {
            return Err("protected Source route payload exceeds bound");
        }
        Ok(count.0)
    }

    pub fn object_ref(&self) -> Result<ObjectRef, &'static str> {
        let size = self.encoded_payload_len()?;
        let children = self.children()?;
        let payload = serde_json::to_vec(self).map_err(|_| "encode protected Source routes")?;
        if payload.len() != size {
            return Err("protected Source route payload changed during encode");
        }
        native_immutable_object_ref_v1(&payload, &children)
            .ok_or("protected Source route object identity overflows")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::*;

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
        duplicate_workspace.heads[1] = head("repo", "rev-b", 10);
        assert_eq!(
            duplicate_workspace.validate(),
            Err("protected Source route scope or order is invalid")
        );
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
        routes.repomap_heads[0].revision_id = RevisionId::new("rev-b").expect("revision");
        assert_eq!(
            routes.validate(),
            Err("protected RepoMap route order is invalid")
        );
    }
}
