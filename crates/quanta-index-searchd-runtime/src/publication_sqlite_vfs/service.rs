//! Service-owned Catalog funding over the same registered physical VFS.

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use quanta_index_catalog::{CatalogIoAdmission, CatalogIoScope};
use quanta_index_core::CoreError;
use quanta_index_core::index::{IndexError, ObjectReadFile};
use quanta_index_core::publication_capacity::PublicationAllowance;

use super::{CURRENT_FUNDING, CatalogSqliteScope, FundingPolicy, SqliteCapacityVfs};

/// A pre-reserved parent for Catalog operations and physical connection close.
///
/// An existing publication event's parent wins
/// for its own operation; foreign TLS funding never falls back to this parent.
pub struct ServiceFundedCatalogIo {
    vfs: Arc<SqliteCapacityVfs>,
    service_parent: Arc<PublicationAllowance>,
}

impl ServiceFundedCatalogIo {
    /// Authenticate the held allowance before the first mutable Catalog open.
    pub fn new(
        vfs: Arc<SqliteCapacityVfs>,
        service_parent: Arc<PublicationAllowance>,
    ) -> Result<Self, CoreError> {
        if !matches!(vfs.funding_policy, FundingPolicy::HeldParent) {
            return Err(CoreError::InvalidContract(
                "service Catalog requires a held-parent SQLite VFS".into(),
            ));
        }
        vfs.common_for_scope()?
            .validate_held_allowance(&service_parent)?;
        Ok(Self {
            vfs,
            service_parent,
        })
    }

    /// Complete queued native FD retirements without closing this reusable scope.
    pub(crate) fn drain_retirements(&self) -> Result<(), CoreError> {
        self.vfs.drain_retirements()
    }

    /// Require native scope retirement before the caller releases this owner.
    /// A failed check leaves the same service and held parent available for retry.
    pub fn retire_checked(&self) -> Result<(), CoreError> {
        self.vfs.retire_checked()
    }
}

impl CatalogIoAdmission for ServiceFundedCatalogIo {
    fn vfs_name(&self) -> &str {
        self.vfs.name()
    }

    fn enter_scope(
        &self,
        diagnostics: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Box<dyn CatalogIoScope> {
        let parent = CURRENT_FUNDING.with(|slot| match slot.borrow().as_ref() {
            Some(existing) if existing.scope == self.vfs.scope => {
                Some(Arc::clone(&existing.parent))
            }
            Some(_) => None,
            None => Some(Arc::clone(&self.service_parent)),
        });
        // Enter funding before the Catalog native scope; the scope field is
        // dropped first and funding then restores the enclosing TLS scope.
        let funding = parent.map(|parent| self.vfs.bind_funding_scope(parent));
        Box::new(CatalogSqliteScope {
            scope: self.vfs.scope(),
            _diagnostics: super::diagnostic::Scope::enter(self.vfs.scope, diagnostics),
            _funding: funding,
        })
    }

    fn enter_close_scope(&self) -> Box<dyn CatalogIoScope> {
        self.vfs
            .close_scope_with_parent(Arc::clone(&self.service_parent))
    }

    fn open_lock_file(
        &self,
        path: &Path,
        create_new: bool,
    ) -> Result<Box<dyn ObjectReadFile>, IndexError> {
        self.vfs.open_lock_file(path, create_new)
    }

    fn unlink_lock_file(&self, path: &Path, held: &File) -> Result<(), IndexError> {
        self.vfs.unlink_lock_file(path, held)
    }
}
