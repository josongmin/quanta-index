//! Candidate route objects transfer directly from ingress to Catalog BOOT
//! custody. No unreferenced interval or second durable head is introduced.

use super::*;
use quanta_index_contract::index::{
    RetiredProjectionPolicyErrorV1, RetiredProjectionPolicyRefV1,
    decode_retired_projection_policy_v1, preflight_retired_projection_policy_v1,
};
use quanta_index_core::index::{
    IndexInstanceId, IngressRef, MemoryLifetime, ViewSelection, ports::ImmutableObjectPort,
    with_native_protected_snapshot_stream_v1, with_native_protected_source_routes_v1,
};
use sha2::{Digest as _, Sha256};

impl NativeDispatcher {
    pub(super) fn seal_retired_policy_only_v1(
        &self,
        primary: OperationKey,
        ingress: IngressRef,
        retired_policy: RetiredProjectionPolicyRefV1,
        budget: &mut WorkBudget,
    ) -> Result<NativeIndexResponse, IndexError> {
        self.authenticate_retired_policy_v1(primary, retired_policy, budget)?;
        let before = self.current_root_identity_for_view(budget)?;
        let selected = self
            .catalog
            .select_native_boot_manifest_with_retired_policy_ingress_if_current(
                primary,
                ingress,
                retired_policy,
                budget,
            )?;
        let nonce = self.catalog.observe_native_root_nonce_v1(budget)?;
        if self.current_root_identity_for_view(budget)? != before || nonce == [0; 32] {
            return Err(IndexError::DigestConflict);
        }
        budget.checkpoint()?;
        Ok(NativeIndexResponse::RootScopedView(
            quanta_index_contract::native_protocol::NativeRootScopedViewV1 {
                view: selected.publication,
                root_nonce: quanta_index_core::index::Digest(nonce),
                root_device: before.device,
                root_inode: before.inode,
            },
        ))
    }

    fn authenticate_retired_policy_v1(
        &self,
        primary: OperationKey,
        policy: RetiredProjectionPolicyRefV1,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        policy
            .validate_v1()
            .map_err(|_| IndexError::DigestConflict)?;
        let stored = self.objects.get(policy.object, budget)?;
        if !stored.registration.children.is_empty() {
            return Err(IndexError::DigestConflict);
        }
        let count = preflight_retired_projection_policy_v1(&stored.payload)
            .map_err(|_| IndexError::DigestConflict)?;
        let bytes = count
            .checked_mul(std::mem::size_of::<IndexInstanceId>())
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(IndexError::ResourceLimited)?;
        let _scratch_lease = budget
            .reserve_memory(bytes.max(1), MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited)?;
        let mut scratch = vec![IndexInstanceId([0; 16]); count];
        let view = decode_retired_projection_policy_v1(&stored.payload, &mut scratch, &mut || {
            budget.checkpoint()
        })
        .map_err(|error| match error {
            RetiredProjectionPolicyErrorV1::Admission(error) => error,
            _ => IndexError::DigestConflict,
        })?;
        let candidate_nonce = self.catalog.observe_native_root_nonce_v1(budget)?;
        let candidate_profile = self.with_read_view(
            ViewSelection::Latest {
                workspace: primary.workspace,
                epoch: primary.epoch,
            },
            budget,
            |current, _| Ok(current.profile),
        )?;
        if view.len_v1() != policy.identity_count as usize
            || quanta_index_core::index::Digest(Sha256::digest(&stored.payload).into())
                != policy.sha256
            || view.header_v1().candidate_root_nonce.0 != candidate_nonce
            || view.header_v1().candidate_profile_ref != candidate_profile
        {
            return Err(IndexError::DigestConflict);
        }
        let profile_object = self.objects.get(candidate_profile, budget)?;
        if !profile_object.registration.children.is_empty() {
            return Err(IndexError::DigestConflict);
        }
        crate::native_product::reject_candidate_profile_overlap_v1(
            &view,
            &scratch,
            &profile_object.payload,
            budget,
        )
    }

    pub(super) fn seal_source_routes_v1(
        &self,
        primary: OperationKey,
        ingress: IngressRef,
        routes: ObjectRef,
        retired_policy: Option<RetiredProjectionPolicyRefV1>,
        budget: &mut WorkBudget,
    ) -> Result<NativeIndexResponse, IndexError> {
        if let Some(policy) = retired_policy {
            self.authenticate_retired_policy_v1(primary, policy, budget)?;
        }
        let stored = self.objects.get(routes, budget)?;
        with_native_protected_source_routes_v1(routes, None, stored, budget, |decoded, budget| {
            let origin = decoded.native_origin.as_ref();
            if let Some(origin) = origin {
                let old_route_object = self.objects.get(origin.source_routes, budget)?;
                with_native_protected_source_routes_v1(
                    origin.source_routes,
                    None,
                    old_route_object,
                    budget,
                    |old, _| {
                        if old.heads != decoded.heads || old.repomap_heads != decoded.repomap_heads
                        {
                            return Err(IndexError::DigestConflict);
                        }
                        Ok(())
                    },
                )?;
            }
            // These checks depend only on staged objects and the held root.
            // Refuse before a Native carry-over can commit provisional rows.
            self.verify_protected_view_markers_v1(decoded, budget)?;
            let before = self.current_root_identity_for_view(budget)?;
            if let Some(origin) = origin {
                with_native_protected_snapshot_stream_v1(
                    origin.snapshot,
                    |reference, budget| self.objects.get(reference, budget),
                    |reference, budget| {
                        self.objects
                            .open_protected_snapshot_page_v1(reference, budget)
                    },
                    budget,
                    |snapshot, budget| {
                        if snapshot.source_root_nonce != origin.source_root_nonce
                            || snapshot.source_routes != origin.source_routes
                        {
                            return Err(IndexError::DigestConflict);
                        }
                        // This import is provisional. The held previous root is
                        // independently compared before deployment selection CAS.
                        self.catalog.restore_protected_native_heads_v1(
                            primary, ingress, snapshot, decoded, budget,
                        )
                    },
                )?;
            }
            let selected = if let Some(policy) = retired_policy {
                self.catalog.select_native_boot_manifest_with_protected_routes_and_retired_policy_if_current(
                    primary, routes, decoded, ingress, policy, budget,
                )?
            } else {
                self.catalog
                    .select_native_boot_manifest_with_protected_routes_if_current(
                        primary, routes, decoded, ingress, budget,
                    )?
            };
            let nonce = self.catalog.observe_native_root_nonce_v1(budget)?;
            if self.current_root_identity_for_view(budget)? != before || nonce == [0; 32] {
                return Err(IndexError::DigestConflict);
            }
            budget.checkpoint()?;
            Ok(NativeIndexResponse::RootScopedView(
                quanta_index_contract::native_protocol::NativeRootScopedViewV1 {
                    view: selected.publication,
                    root_nonce: quanta_index_core::index::Digest(nonce),
                    root_device: before.device,
                    root_inode: before.inode,
                },
            ))
        })
    }
}
