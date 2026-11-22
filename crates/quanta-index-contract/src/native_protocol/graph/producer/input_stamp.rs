//! Canonical per-file inputs for selecting Native RepoMap rank recompilation.

use std::collections::BTreeMap;

use sha2::{Digest as _, Sha256};

use crate::{
    RepoMapEdgeKind, RepoMapNodeRef, RepoMapSourceBundle, RepoRelativePath,
    index::Digest,
    repomap::compiler::{
        REPOMAP_COMPILE_PROJECTION_PROFILE_DESCRIPTOR, REPOMAP_COMPILE_SCHEMA_DESCRIPTOR,
        RepoMapGraphCompiler, edge_parts_v1, node_identity,
    },
};

use super::{
    NativeRepoMapOwnerError, graph_node_payload_digest_v1, node_owner_path, stamp_bytes_v1,
    stamp_count_v1, stamp_typed_v1,
};

#[derive(Clone, Copy)]
struct IncidentEdgeV1<'a> {
    kind: RepoMapEdgeKind,
    source: &'a RepoMapNodeRef,
    target: &'a RepoMapNodeRef,
}

#[derive(Default)]
struct FileInputsV1<'a> {
    nodes: Vec<(RepoMapNodeRef, Digest)>,
    incident_edges: Vec<IncidentEdgeV1<'a>>,
    remote_chunks: BTreeMap<RepoMapNodeRef, Digest>,
}

impl RepoMapGraphCompiler {
    /// Fingerprint raw inputs that can change this file's compiled graph or
    /// rank. This does not compile rank rows or replace the later whole-bundle
    /// validation in `compile_native_selected_file_owners_v1`.
    pub fn native_file_rank_input_stamps_v1(
        &self,
        bundle: &RepoMapSourceBundle,
    ) -> Result<BTreeMap<RepoRelativePath, Digest>, NativeRepoMapOwnerError> {
        let mut identities = BTreeMap::new();
        let mut files = BTreeMap::<RepoRelativePath, FileInputsV1<'_>>::new();
        for node in &bundle.nodes {
            let identity = node_identity(node);
            let path = node_owner_path(node);
            let payload_digest =
                graph_node_payload_digest_v1(node).map_err(NativeRepoMapOwnerError::Encoding)?;
            if identities
                .insert(identity.clone(), (path, payload_digest))
                .is_some()
            {
                return Err(NativeRepoMapOwnerError::OwnerPartition);
            }
            files
                .entry(path.clone())
                .or_default()
                .nodes
                .push((identity, payload_digest));
        }

        for edge in &bundle.edges {
            let (kind, source, target) = edge_parts_v1(edge);
            let (source_path, _) = identities
                .get(source)
                .ok_or(NativeRepoMapOwnerError::OwnerPartition)?;
            let (target_path, target_payload_digest) = identities
                .get(target)
                .ok_or(NativeRepoMapOwnerError::OwnerPartition)?;
            let incident = IncidentEdgeV1 {
                kind,
                source,
                target,
            };
            files
                .get_mut(*source_path)
                .ok_or(NativeRepoMapOwnerError::OwnerPartition)?
                .incident_edges
                .push(incident);
            if source_path != target_path {
                files
                    .get_mut(*target_path)
                    .ok_or(NativeRepoMapOwnerError::OwnerPartition)?
                    .incident_edges
                    .push(incident);
                if kind == RepoMapEdgeKind::OwnsChunk {
                    let _previous = files
                        .get_mut(*source_path)
                        .ok_or(NativeRepoMapOwnerError::OwnerPartition)?
                        .remote_chunks
                        .insert(target.clone(), *target_payload_digest);
                }
            }
        }

        let rank_policy = crate::native_protocol::native_repomap_rank_policy();
        let mut stamps = BTreeMap::new();
        for (path, mut inputs) in files {
            inputs
                .nodes
                .sort_unstable_by(|left, right| left.0.cmp(&right.0));
            inputs.incident_edges.sort_unstable_by(|left, right| {
                (left.kind.as_code_str(), left.source, left.target).cmp(&(
                    right.kind.as_code_str(),
                    right.source,
                    right.target,
                ))
            });
            if inputs.incident_edges.windows(2).any(|pair| {
                (pair[0].kind.as_code_str(), pair[0].source, pair[0].target)
                    == (pair[1].kind.as_code_str(), pair[1].source, pair[1].target)
            }) {
                return Err(NativeRepoMapOwnerError::OwnerPartition);
            }

            let mut hash = Sha256::new();
            hash.update(b"quanta-index/native-repomap-file-rank-input/v1\0");
            hash.update(rank_policy.0);
            stamp_bytes_v1(&mut hash, REPOMAP_COMPILE_SCHEMA_DESCRIPTOR.as_bytes());
            stamp_bytes_v1(
                &mut hash,
                REPOMAP_COMPILE_PROJECTION_PROFILE_DESCRIPTOR.as_bytes(),
            );
            stamp_typed_v1(&mut hash, &path)?;
            stamp_count_v1(&mut hash, inputs.nodes.len());
            for (identity, payload_digest) in &inputs.nodes {
                stamp_typed_v1(&mut hash, identity)?;
                hash.update(payload_digest.0);
            }
            // Every incident edge is a safe superset of the exact degree
            // inputs. It also retains source-owned outgoing graph adjacency.
            stamp_count_v1(&mut hash, inputs.incident_edges.len());
            for edge in &inputs.incident_edges {
                stamp_bytes_v1(&mut hash, edge.kind.as_code_str().as_bytes());
                stamp_typed_v1(&mut hash, edge.source)?;
                stamp_typed_v1(&mut hash, edge.target)?;
            }
            // A cross-file OwnsChunk target contributes its typed payload to
            // the source symbol's preview and score even though it is remote.
            stamp_count_v1(&mut hash, inputs.remote_chunks.len());
            for (identity, payload_digest) in &inputs.remote_chunks {
                stamp_typed_v1(&mut hash, identity)?;
                hash.update(payload_digest.0);
            }
            let _previous = stamps.insert(path, Digest(hash.finalize().into()));
        }
        Ok(stamps)
    }
}
