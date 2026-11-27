//! Catalog lock files belong to the publication file authority, not SQLite.

use std::{
    cell::RefCell,
    fs::File,
    marker::PhantomData,
    num::NonZeroU64,
    os::unix::fs::MetadataExt as _,
    path::Path,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::{
    CoreError, RequestBudgetV1,
    index::{IndexError, ObjectReadFile, ObjectReadIoPort as _, WorkBudget, WorkLimits},
    publication_admission::CatalogPublicationFundingScope,
    publication_capacity::PublicationAllowance,
};

use crate::{
    native_object_read_io::NativeObjectReadIo, publication_file_capacity::PublicationFileCapacity,
};

/// Operation funding shared by regular Catalog files and any SQLite callbacks
/// executing for the same owner. A foreign active owner cannot grant fallback.
#[derive(Clone, Copy)]
pub(crate) struct CatalogFileFunding(NonZeroU64);

static NEXT_SCOPE: AtomicU64 = AtomicU64::new(1);

struct CurrentFunding {
    owner: NonZeroU64,
    parent: Arc<PublicationAllowance>,
}

thread_local! {
    static CURRENT_FUNDING: RefCell<Option<CurrentFunding>> = const { RefCell::new(None) };
}

pub(crate) struct CatalogFileFundingGuard {
    previous: Option<CurrentFunding>,
    _thread_local: PhantomData<Rc<()>>,
}

impl CatalogPublicationFundingScope for CatalogFileFundingGuard {}

impl Drop for CatalogFileFundingGuard {
    fn drop(&mut self) {
        CURRENT_FUNDING.with(|slot| {
            let _finished = slot.replace(self.previous.take());
        });
    }
}

pub(crate) struct ForeignCatalogFileFunding;

impl CatalogFileFunding {
    pub(crate) fn allocate() -> Result<Self, CoreError> {
        let number = NEXT_SCOPE
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(1)
            })
            .map_err(|_error| CoreError::InvalidContract("SQLite scope IDs exhausted".into()))?;
        let scope = NonZeroU64::new(number)
            .ok_or_else(|| CoreError::InvalidContract("zero SQLite scope".into()))?;
        Ok(Self(scope))
    }

    #[cfg(test)]
    pub(crate) const fn new(owner: NonZeroU64) -> Self {
        Self(owner)
    }

    pub(crate) fn bind(self, parent: Arc<PublicationAllowance>) -> CatalogFileFundingGuard {
        let previous = CURRENT_FUNDING.with(|slot| {
            slot.replace(Some(CurrentFunding {
                owner: self.0,
                parent,
            }))
        });
        CatalogFileFundingGuard {
            previous,
            _thread_local: PhantomData,
        }
    }

    pub(crate) fn parent(
        self,
    ) -> Result<Option<Arc<PublicationAllowance>>, ForeignCatalogFileFunding> {
        CURRENT_FUNDING.with(|slot| match slot.borrow().as_ref() {
            Some(funding) if funding.owner == self.0 => Ok(Some(Arc::clone(&funding.parent))),
            Some(_) => Err(ForeignCatalogFileFunding),
            None => Ok(None),
        })
    }

    pub(crate) fn open(
        self,
        common: Arc<PublicationFileCapacity>,
        path: &Path,
        create_new: bool,
    ) -> Result<Box<dyn ObjectReadFile>, IndexError> {
        let parent = if create_new {
            self.parent().map_err(|_foreign| {
                IndexError::BackendViolation("Catalog lock funding belongs to another scope".into())
            })?
        } else {
            None
        };
        open(common, path, create_new, parent)
    }
}

pub(crate) fn catalog_file_error(error: &CoreError) -> IndexError {
    if matches!(
        error,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
            ..
        }
    ) {
        IndexError::ResourceLimited
    } else {
        IndexError::Storage(format!("Catalog regular-file I/O: {error}"))
    }
}

/// Create and open a Catalog lock under the shared publication authority.
/// The returned read owner retains its FD lease through close.
pub(crate) fn open(
    common: Arc<PublicationFileCapacity>,
    path: &Path,
    create_new: bool,
    parent: Option<Arc<PublicationAllowance>>,
) -> Result<Box<dyn ObjectReadFile>, IndexError> {
    if create_new {
        common
            .prepare_empty_lock_with_parent(path, true, parent)
            .map_err(|error| catalog_file_error(&error))?;
    }
    // Session creation has no caller WorkBudget. The process-lived owner is
    // bounded by the common handle and registry ceilings.
    let mut budget = WorkBudget::with_request(
        WorkLimits {
            reads: 0,
            writes: 0,
            bytes: 0,
            mutations: 0,
        },
        RequestBudgetV1::unbounded(),
    );
    NativeObjectReadIo::new(common).open(path, &mut budget)
}

/// Only the held inode may be removed. The authority performs the actual
/// unlink and parent sync, then retires its tracked charge conservatively.
pub(crate) fn unlink(
    common: &PublicationFileCapacity,
    path: &Path,
    held: &File,
) -> Result<(), IndexError> {
    let mut deletion = common
        .begin_delete(path)
        .map_err(|error| catalog_file_error(&error))?;
    let (source, _parent) = deletion
        .source_metadata()
        .map_err(|error| catalog_file_error(&error))?;
    let expected = held
        .metadata()
        .map_err(|error| IndexError::Storage(format!("Catalog held lock metadata: {error}")))?;
    if source.dev() != expected.dev()
        || source.ino() != expected.ino()
        || source.len() != expected.len()
        || source.nlink() != expected.nlink()
    {
        return Err(IndexError::Storage(
            "Catalog delete source differs from held lock".into(),
        ));
    }
    deletion
        .perform_and_sync()
        .map_err(|error| catalog_file_error(&error))
}
