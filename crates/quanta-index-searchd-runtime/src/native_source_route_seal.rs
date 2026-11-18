//! Candidate route objects transfer directly from ingress to Catalog BOOT
//! custody. No unreferenced interval or second durable head is introduced.

use super::*;
use quanta_index_core::index::{
    IngressRef, ports::ImmutableObjectPort, with_native_protected_snapshot_v1,
    with_native_protected_source_routes_v1,
};

impl NativeDispatcher {
    pub(super) fn seal_source_routes_v1(
        &self,
        primary: OperationKey,
        ingress: IngressRef,
        routes: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<NativeIndexResponse, IndexError> {
        let stored = self.objects.get(routes, budget)?;
        with_native_protected_source_routes_v1(routes, None, &stored, budget, |decoded, budget| {
            if let Some(origin) = &decoded.native_origin {
                let snapshot_object = self.objects.get(origin.snapshot, budget)?;
                let old_route_object = self.objects.get(origin.source_routes, budget)?;
                with_native_protected_snapshot_v1(
                    origin.snapshot,
                    &snapshot_object,
                    budget,
                    |snapshot, budget| {
                        if snapshot.source_root_nonce != origin.source_root_nonce
                            || snapshot.source_routes != origin.source_routes
                        {
                            return Err(IndexError::DigestConflict);
                        }
                        with_native_protected_source_routes_v1(
                            origin.source_routes,
                            None,
                            &old_route_object,
                            budget,
                            |old, budget| {
                                if old.heads != decoded.heads
                                    || old.repomap_heads != decoded.repomap_heads
                                {
                                    return Err(IndexError::DigestConflict);
                                }
                                // This import is provisional. The held previous root is
                                // independently compared before deployment selection CAS.
                                self.catalog.restore_protected_native_heads_v1(
                                    primary, ingress, snapshot, decoded, budget,
                                )
                            },
                        )
                    },
                )?;
            }
            self.verify_protected_view_markers_v1(decoded, budget)?;
            let before = self.current_root_identity_for_view(budget)?;
            let selected = self
                .catalog
                .select_native_boot_manifest_with_protected_routes_if_current(
                    primary, routes, decoded, ingress, budget,
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
        })
    }
}
