//! Memory admission adapter for the optional registry-compatible SQLite VFS.
//!
//! This module only converts a real Runtime retained lease into the VFS grant.
//! It never installs SQLite's process-global syscall hook or registers a VFS.

use std::{alloc::Layout, num::NonZeroU64, sync::Arc};

use quanta_index_core::index::{IndexError, MemoryLease, MemoryLifetime, MemoryScope};
use quanta_sqlite_retained_grant::RetainedMemoryGrant;

struct RuntimeOwnerVfsGrant {
    backing: MemoryLease,
    _arc_backing: MemoryLease,
}

impl RetainedMemoryGrant for RuntimeOwnerVfsGrant {
    fn bytes(&self) -> u64 {
        self.backing.bytes()
    }
}

/// Admit the VFS structure, fixed file slots, and this returned Arc before
/// allocating the Arc. Both leases are retained until its last owner drops.
/// Pass the exact nonzero layout from `InstalledHook::owner_vfs_backing_bytes`;
/// the wrapper validates this amount when registration is separately allowed.
/// The caller must also fund the FD ledger and its authority separately.
pub fn reserve_owner_vfs_grant(
    scope: &MemoryScope,
    vfs_backing_bytes: NonZeroU64,
) -> Result<Arc<dyn RetainedMemoryGrant>, IndexError> {
    let arc_layout =
        MemoryLease::shared_allocation_backing_layout(Layout::new::<RuntimeOwnerVfsGrant>())?;
    let arc_bytes =
        u64::try_from(arc_layout.size()).map_err(|_error| IndexError::ResourceLimited)?;
    let arc_backing = scope.reserve(arc_bytes, MemoryLifetime::Retained)?;
    let backing = scope.reserve(vfs_backing_bytes.get(), MemoryLifetime::Retained)?;
    Ok(Arc::new(RuntimeOwnerVfsGrant {
        backing,
        _arc_backing: arc_backing,
    }))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use quanta_index_core::index::{MemoryLeaseOwner, MemoryScopePort};

    use super::*;

    struct Account {
        limit: u64,
        held: Mutex<u64>,
        lifetimes: Mutex<Vec<MemoryLifetime>>,
    }

    impl MemoryLeaseOwner for Account {
        fn release(&self, bytes: u64) {
            let mut held = self.held.lock().expect("held bytes");
            *held = held.checked_sub(bytes).expect("charge underflow");
        }
    }

    struct TestScope(Arc<Account>);

    impl MemoryScopePort for TestScope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            let mut held = self.0.held.lock().expect("held bytes");
            let next = held.checked_add(bytes).ok_or(IndexError::ResourceLimited)?;
            if next > self.0.limit {
                return Err(IndexError::ResourceLimited);
            }
            *held = next;
            drop(held);
            self.0.lifetimes.lock().expect("lifetimes").push(lifetime);
            Ok(MemoryLease::new(
                bytes,
                Arc::clone(&self.0) as Arc<dyn MemoryLeaseOwner>,
            ))
        }
    }

    fn scope(limit: u64) -> (MemoryScope, Arc<Account>) {
        let account = Arc::new(Account {
            limit,
            held: Mutex::new(0),
            lifetimes: Mutex::new(Vec::new()),
        });
        let scope = MemoryScope::new(Arc::new(TestScope(Arc::clone(&account))));
        (scope, account)
    }

    fn vfs_bytes() -> NonZeroU64 {
        NonZeroU64::new(4096).expect("nonzero VFS backing")
    }

    fn admitted_bytes() -> u64 {
        let arc =
            MemoryLease::shared_allocation_backing_layout(Layout::new::<RuntimeOwnerVfsGrant>())
                .expect("finite Arc layout");
        vfs_bytes().get() + u64::try_from(arc.size()).expect("finite Arc bytes")
    }

    #[test]
    fn retained_grant_survives_scope_and_releases_on_last_owner() {
        let total = admitted_bytes();
        let (scope, account) = scope(total);
        let grant = reserve_owner_vfs_grant(&scope, vfs_bytes()).expect("retained grant");
        assert_eq!(grant.bytes(), vfs_bytes().get());
        assert_eq!(*account.held.lock().expect("held bytes"), total);
        assert_eq!(
            *account.lifetimes.lock().expect("lifetimes"),
            [MemoryLifetime::Retained, MemoryLifetime::Retained]
        );
        drop(scope);
        let last_owner = Arc::clone(&grant);
        drop(grant);
        assert_eq!(*account.held.lock().expect("held bytes"), total);
        drop(last_owner);
        assert_eq!(*account.held.lock().expect("held bytes"), 0);
    }

    #[test]
    fn insufficient_quota_refunds_first_lease() {
        let (scope, account) = scope(admitted_bytes() - 1);
        assert!(matches!(
            reserve_owner_vfs_grant(&scope, vfs_bytes()),
            Err(IndexError::ResourceLimited)
        ));
        assert_eq!(*account.held.lock().expect("held bytes"), 0);
        assert_eq!(
            *account.lifetimes.lock().expect("lifetimes"),
            [MemoryLifetime::Retained]
        );
    }
}
