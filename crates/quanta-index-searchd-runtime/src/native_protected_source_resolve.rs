//! Resolve protected revisions from the live selected BOOT manifest.
//! The caller supplies only a repository pair, never an object or workspace.

use super::*;
use quanta_index_core::index::{
    MAX_NATIVE_PROTECTED_VIEW_MARKER_BYTES_V1, MemoryLifetime,
    NativeProtectedSourceRoutesV1, native_immutable_object_ref_v1,
    native_protected_view_marker_bytes_v1,
    ports::ImmutableObjectPort,
    with_native_protected_source_routes_v1,
};

impl NativeDispatcher {
    pub(super) fn verify_protected_view_markers_v1(
        &self,
        routes: &NativeProtectedSourceRoutesV1,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let marker_scratch = u64::try_from(4 * MAX_NATIVE_PROTECTED_VIEW_MARKER_BYTES_V1)
            .map_err(|_width| IndexError::ResourceLimited)?;
        let _marker_memory = budget
            .reserve_memory(marker_scratch, MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited)?;
        // Route validation already binds each shared Source/RepoMap pair
        // to one exact publication. Read that marker once.
        let repomap_only = routes.repomap_heads.iter().filter(|head| {
            let pair = (head.repo_id.as_str(), head.revision_id.as_str());
            routes
                .heads
                .binary_search_by(|source| {
                    (source.repo_id.as_str(), source.revision_id.as_str()).cmp(&pair)
                })
                .is_err()
        });
        let genesis = routes
            .repomap_heads
            .iter()
            .filter(|head| head.source_operation.is_none())
            .map(|head| head.source_publication.identity);
        for view in routes
            .heads
            .iter()
            .map(|head| head.publication.identity)
            .chain(repomap_only.map(|head| head.publication.identity))
            .chain(genesis)
        {
            let expected = native_protected_view_marker_bytes_v1(view)
                .map_err(|message| IndexError::InvalidInput(message.into()))?;
            let marker_ref = native_immutable_object_ref_v1(&expected, &[]).ok_or_else(|| {
                IndexError::InvalidInput("protected view marker identity overflows".into())
            })?;
            let marker = self.objects.get(marker_ref, budget)?;
            if marker.payload != expected || !marker.registration.children.is_empty() {
                return Err(IndexError::DigestConflict);
            }
        }
        Ok(())
    }

    fn with_selected_protected_routes_v1<T>(
        &self,
        budget: &mut WorkBudget,
        visit: impl FnOnce(
            &quanta_index_catalog::NativeBootManifest,
            &NativeProtectedSourceRoutesV1,
            &mut WorkBudget,
        ) -> Result<T, IndexError>,
    ) -> Result<
        (
            T,
            quanta_index_core::publication_capacity::InodeKey,
            [u8; 32],
        ),
        IndexError,
    > {
        let before = self.current_root_identity_for_view(budget)?;
        let selected = self
            .catalog
            .native_boot_manifest(budget)?
            .ok_or(IndexError::SnapshotUnavailable)?;
        let route = selected
            .protected_source_routes
            .ok_or(IndexError::SnapshotUnavailable)?;
        let stored = self.objects.get(route.object, budget)?;
        let result = with_native_protected_source_routes_v1(
            route.object,
            Some(route.count),
            stored,
            budget,
            |routes, budget| {
                self.verify_protected_view_markers_v1(routes, budget)?;
                visit(&selected, routes, budget)
            },
        )?;
        let nonce = self.catalog.observe_native_root_nonce_v1(budget)?;
        if self.current_root_identity_for_view(budget)? != before || nonce == [0; 32] {
            return Err(IndexError::DigestConflict);
        }
        budget.checkpoint()?;
        Ok((result, before, nonce))
    }

    pub(super) fn resolve_protected_source_head_v1(
        &self,
        repo_id: &quanta_index_contract::RepoId,
        revision_id: &quanta_index_contract::RevisionId,
        budget: &mut WorkBudget,
    ) -> Result<NativeIndexResponse, IndexError> {
        let (view, before, nonce) =
            self.with_selected_protected_routes_v1(budget, |selected, routes, budget| {
                self.catalog
                    .resolve_selected_native_protected_source_head_v1(
                        selected,
                        routes,
                        repo_id,
                        revision_id,
                        budget,
                    )
            })?;
        Ok(NativeIndexResponse::RootScopedView(
            quanta_index_contract::native_protocol::NativeRootScopedViewV1 {
                view,
                root_nonce: quanta_index_core::index::Digest(nonce),
                root_device: before.device,
                root_inode: before.inode,
            },
        ))
    }

}
