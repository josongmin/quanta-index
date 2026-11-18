//! Resolve protected revisions from the live selected BOOT manifest.
//! The caller supplies only a repository pair, never an object or workspace.

use super::*;
use quanta_index_core::index::{
    MAX_NATIVE_PROTECTED_VIEW_MARKER_BYTES_V1, MapValue, MemoryLifetime,
    NativeProtectedSourceRoutesV1, WorkUsage, member_map_key, native_immutable_object_ref_v1,
    native_protected_view_marker_bytes_v1,
    ports::{ImmutableObjectPort, PersistentMapPort},
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

    fn verified_imported_repomap_member_v1(
        &self,
        view: &quanta_index_contract::index::CatalogView,
        instance: quanta_index_contract::index::IndexInstanceId,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_contract::index::ObjectRef, IndexError> {
        let key = member_map_key(instance);
        let actual = match self.objects.lookup(view.members, &key, budget)? {
            Some(MapValue::Object(reference)) => reference,
            _ => return Err(IndexError::DigestConflict),
        };
        let stored = self.objects.get(actual, budget)?;
        let member = quanta_index_core::index::decode_member_binding(&stored.payload, budget)?;
        if stored.registration.reference != actual
            || member.instance != instance
            || member.format != quanta_index_repomap::native::NATIVE_LEGACY_REPOMAP_FORMAT_V1
            || !stored.registration.children.iter().copied().eq([
                Some(member.artifact),
                member.input.root.0,
            ]
            .into_iter()
            .flatten())
        {
            return Err(IndexError::DigestConflict);
        }
        let profile_object = self.objects.get(view.profile, budget)?;
        if profile_object.registration.reference != view.profile
            || !profile_object.registration.children.is_empty()
            || profile_object.payload.is_empty()
        {
            return Err(IndexError::DigestConflict);
        }
        let profile_bytes =
            u64::try_from(profile_object.payload.len()).map_err(|_| IndexError::ResourceLimited)?;
        let _profile_memory = budget
            .reserve_memory(
                profile_bytes
                    .checked_mul(32)
                    .and_then(|n| n.checked_add(65_536))
                    .ok_or(IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: profile_bytes,
            ..Default::default()
        })?;
        let profile: quanta_index_contract::index::IndexProfile =
            serde_json::from_slice(&profile_object.payload)
                .map_err(|_| IndexError::DigestConflict)?;
        let spec = profile
            .instances
            .iter()
            .find(|spec| spec.instance == instance)
            .ok_or(IndexError::DigestConflict)?;
        if spec.kind != quanta_index_repomap::native::NATIVE_LEGACY_REPOMAP_KIND_V1
            || spec.schema != quanta_index_repomap::native::NATIVE_LEGACY_REPOMAP_SCHEMA_V1
            || spec.format != member.format
            || spec.activation != quanta_index_contract::index::GroupActivation::CoreRequired
            || !matches!(
                spec.input_domain,
                quanta_index_contract::index::InputDomain::Projection { .. }
            )
            || spec.policy != quanta_index_contract::native_protocol::native_repomap_rank_policy()
            || member.input.schema != spec.schema
            || member.input.policy != spec.policy
        {
            return Err(IndexError::DigestConflict);
        }
        let _read = quanta_index_repomap::native::NativeLegacyRepoMapReadV1::open(
            &member,
            view.source,
            self.objects.as_ref(),
            budget,
        )?;
        Ok(actual)
    }

    pub(super) fn read_imported_repomap_member_v1(
        &self,
        requested: quanta_index_contract::index::ViewRef,
        instance: quanta_index_contract::index::IndexInstanceId,
        budget: &mut WorkBudget,
    ) -> Result<NativeIndexResponse, IndexError> {
        self.with_read_view(
            quanta_index_contract::index::ViewSelection::Exact(requested),
            budget,
            |view, budget| {
                if view.identity != requested {
                    return Err(IndexError::DigestConflict);
                }
                let member_binding =
                    self.verified_imported_repomap_member_v1(view, instance, budget)?;
                Ok(NativeIndexResponse::ImportedRepoMapMember(
                    quanta_index_contract::native_protocol::NativeImportedRepoMapMemberReadV1 {
                        view: requested,
                        instance,
                        member_binding,
                    },
                ))
            },
        )
    }

    pub(super) fn resolve_protected_repomap_head_v1(
        &self,
        repo_id: &quanta_index_contract::RepoId,
        revision_id: &quanta_index_contract::RevisionId,
        budget: &mut WorkBudget,
    ) -> Result<NativeIndexResponse, IndexError> {
        let ((view, instance, member_binding), before, nonce) = self
            .with_selected_protected_routes_v1(budget, |selected, routes, budget| {
                let head = routes
                    .repomap_heads
                    .iter()
                    .find(|head| &head.repo_id == repo_id && &head.revision_id == revision_id)
                    .ok_or(IndexError::SnapshotUnavailable)?;
                let view = self
                    .catalog
                    .resolve_selected_native_protected_repomap_head_v1(
                        selected,
                        routes,
                        repo_id,
                        revision_id,
                        budget,
                    )?;
                if view != head.publication {
                    return Err(IndexError::DigestConflict);
                }
                let actual =
                    self.verified_imported_repomap_member_v1(&view, head.instance, budget)?;
                if actual != head.member_binding {
                    return Err(IndexError::DigestConflict);
                }
                Ok((view, head.instance, actual))
            })?;
        Ok(NativeIndexResponse::ProtectedRepoMapHead(
            quanta_index_contract::native_protocol::NativeProtectedRepoMapResolutionV1 {
                root: quanta_index_contract::native_protocol::NativeRootScopedViewV1 {
                    view,
                    root_nonce: quanta_index_core::index::Digest(nonce),
                    root_device: before.device,
                    root_inode: before.inode,
                },
                instance,
                member_binding,
            },
        ))
    }
}
