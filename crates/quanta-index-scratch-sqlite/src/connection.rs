//! A named-driver connection retains physical I/O custody through actual close.

use std::{
    cell::RefCell,
    ops::Deref,
    path::Path,
    rc::{Rc, Weak},
    sync::Arc,
};

use quanta_index_core::index::{IndexError, ScratchAllocation, ScratchIoBinding, ScratchIoScope};
use rusqlite::{Connection, OpenFlags};

thread_local! {
    static CURRENT_SCOPE: RefCell<Option<Weak<dyn ScratchIoScope>>> = RefCell::new(None);
}

struct OperationScope {
    previous: Option<Weak<dyn ScratchIoScope>>,
    _scope: Option<Rc<dyn ScratchIoScope>>,
}

impl OperationScope {
    fn enter(binding: Option<&Arc<dyn ScratchIoBinding>>) -> Self {
        let scope: Option<Rc<dyn ScratchIoScope>> =
            binding.map(|binding| Rc::from(binding.enter_scope()));
        let previous = CURRENT_SCOPE.with(|slot| slot.replace(scope.as_ref().map(Rc::downgrade)));
        Self {
            previous,
            _scope: scope,
        }
    }
}

impl Drop for OperationScope {
    fn drop(&mut self) {
        CURRENT_SCOPE.with(|slot| {
            let _replaced = slot.replace(self.previous.take());
        });
    }
}

/// Only the currently active operation may classify a private capacity refusal.
/// Raw FULL/NOMEM/IO and unqualified operations preserve their storage meaning.
#[must_use]
pub fn storage_error(error: &rusqlite::Error, context: &str) -> IndexError {
    if let rusqlite::Error::SqliteFailure(failure, _) = error
        && failure.code == rusqlite::ErrorCode::DiskFull
    {
        let scope = CURRENT_SCOPE.with(|slot| slot.borrow().as_ref().and_then(Weak::upgrade));
        if scope.is_some_and(|scope| scope.is_capacity_refusal(failure.extended_code)) {
            return IndexError::ResourceLimited;
        }
    }
    IndexError::Storage(format!("{context}: {error}"))
}

pub struct ScratchConnection {
    connection: Option<Connection>,
    binding: Option<Arc<dyn ScratchIoBinding>>,
}

impl ScratchConnection {
    pub fn open(path: &Path, allocation: &dyn ScratchAllocation) -> Result<Self, IndexError> {
        let binding = allocation.io_binding();
        let _scope = OperationScope::enter(binding.as_ref());
        let connection = match binding.as_ref() {
            Some(binding) => {
                if binding.driver_name().is_empty() {
                    return Err(IndexError::InvalidInput("empty scratch file driver".into()));
                }
                Connection::open_with_flags_and_vfs(
                    path,
                    OpenFlags::default(),
                    binding.driver_name(),
                )
            }
            None => Connection::open(path),
        }
        .map_err(|error| storage_error(&error, "open scratch connection"))?;
        Ok(Self {
            connection: Some(connection),
            binding,
        })
    }

    pub fn scoped(&self) -> Result<ScratchConnectionScope<'_>, IndexError> {
        Ok(ScratchConnectionScope {
            connection: self.connection.as_ref().ok_or_else(|| {
                IndexError::BackendViolation("scratch connection is closed".into())
            })?,
            _scope: OperationScope::enter(self.binding.as_ref()),
        })
    }
}

impl Drop for ScratchConnection {
    fn drop(&mut self) {
        let _scope = OperationScope::enter(self.binding.as_ref());
        drop(self.connection.take());
    }
}

pub struct ScratchConnectionScope<'a> {
    connection: &'a Connection,
    _scope: OperationScope,
}

impl Deref for ScratchConnectionScope<'_> {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        self.connection
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::index::WorkBudget;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const PRIVATE_FULL: i32 = rusqlite::ffi::SQLITE_FULL | (127 << 8);

    #[derive(Debug)]
    struct Binding {
        name: &'static str,
        live: Arc<AtomicUsize>,
    }
    struct Scope(Arc<AtomicUsize>);
    #[derive(Debug)]
    struct NamespaceLease;
    impl quanta_index_core::index::ScratchNamespaceLease for NamespaceLease {
        fn namespace_identity(&self) -> quanta_index_core::publication_capacity::InodeKey {
            quanta_index_core::publication_capacity::InodeKey {
                device: 0,
                inode: 0,
            }
        }
    }
    impl ScratchIoScope for Scope {
        fn is_capacity_refusal(&self, code: i32) -> bool {
            code == PRIVATE_FULL
        }
    }
    impl Drop for Scope {
        fn drop(&mut self) {
            let _previous = self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    impl ScratchIoBinding for Binding {
        fn driver_name(&self) -> &str {
            self.name
        }
        fn enter_scope(&self) -> Box<dyn ScratchIoScope> {
            let _previous = self.live.fetch_add(1, Ordering::SeqCst);
            Box::new(Scope(Arc::clone(&self.live)))
        }
        fn acquire_namespace(
            &self,
            _: &Path,
        ) -> Result<Box<dyn quanta_index_core::index::ScratchNamespaceLease>, IndexError> {
            Ok(Box::new(NamespaceLease))
        }
        fn retire_namespace(
            &self,
            _: &Path,
            _budget: Option<&mut quanta_index_core::index::WorkBudget>,
        ) -> Result<(), IndexError> {
            Ok(())
        }
    }
    struct Allocation {
        path: std::path::PathBuf,
        binding: Option<Arc<dyn ScratchIoBinding>>,
    }
    impl ScratchAllocation for Allocation {
        fn path(&self) -> &Path {
            &self.path
        }
        fn max_bytes(&self) -> u64 {
            1_048_576
        }
        fn reserved_bytes(&self) -> Result<u64, IndexError> {
            Ok(1_048_576)
        }
        fn reserve_at_least(&self, _: u64, _: &mut WorkBudget) -> Result<(), IndexError> {
            Ok(())
        }
        fn io_binding(&self) -> Option<Arc<dyn ScratchIoBinding>> {
            self.binding.clone()
        }
    }
    fn require(value: bool) -> Result<(), IndexError> {
        if value {
            Ok(())
        } else {
            Err(IndexError::BackendViolation(
                "scratch connection invariant".into(),
            ))
        }
    }
    fn full(code: i32) -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), None)
    }

    #[test]
    fn missing_named_driver_never_falls_back_or_creates_database()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let live = Arc::new(AtomicUsize::new(0));
        let allocation = Allocation {
            path: directory.path().to_path_buf(),
            binding: Some(Arc::new(Binding {
                name: "quanta-absent-scratch-driver",
                live: Arc::clone(&live),
            })),
        };
        let file = allocation.path().join("denied.sqlite");
        require(matches!(
            ScratchConnection::open(&file, &allocation),
            Err(IndexError::Storage(_))
        ))?;
        require(!file.exists() && live.load(Ordering::SeqCst) == 0)?;
        Ok(())
    }

    #[test]
    fn unqualified_nested_operation_masks_and_restores_capacity_witness()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let live = Arc::new(AtomicUsize::new(0));
        let binding: Arc<dyn ScratchIoBinding> = Arc::new(Binding {
            name: "unix",
            live: Arc::clone(&live),
        });
        let outer = OperationScope::enter(Some(&binding));
        require(matches!(
            storage_error(&full(PRIVATE_FULL), "fixture"),
            IndexError::ResourceLimited
        ))?;
        let allocation = Allocation {
            path: directory.path().to_path_buf(),
            binding: None,
        };
        let connection =
            ScratchConnection::open(&allocation.path().join("plain.sqlite"), &allocation)?;
        {
            let scope = connection.scoped()?;
            scope.execute_batch("CREATE TABLE t(v);")?;
            require(matches!(
                storage_error(&full(PRIVATE_FULL), "fixture"),
                IndexError::Storage(_)
            ))?;
        }
        require(matches!(
            storage_error(&full(PRIVATE_FULL), "fixture"),
            IndexError::ResourceLimited
        ))?;
        require(matches!(
            storage_error(&full(rusqlite::ffi::SQLITE_FULL), "fixture"),
            IndexError::Storage(_)
        ))?;
        drop(connection);
        drop(outer);
        require(live.load(Ordering::SeqCst) == 0)?;
        Ok(())
    }
}
