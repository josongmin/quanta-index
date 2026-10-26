//! Optional per-session physical admission callbacks for local files.
//! The trait is defined once here and re-exported by lance-io.

use std::fmt::Debug;
use std::fs::File;
use std::io;
use std::path::Path as StdPath;
use std::sync::Arc;

/// The destination contract is part of a retained move's retry identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileMoveMode {
    /// Refuse any existing destination.
    Create,
    /// Replace a regular-file destination while retaining its old inode.
    Overwrite,
    /// The source is an admitted temporary alias. If the original move is
    /// proved unchanged, its owner must remove that exact alias before retry.
    OverwriteFromAlias,
}

/// Authenticated file operations under one session's existing capacity ledger.
/// Implementations retain uncertain writes until their actual FD and namespace
/// state can be reconciled; pathname absence alone cannot authorize a refund.
pub trait FileAdmission: Debug + Send + Sync {
    /// Reconcile the original retained namespace operation before a new scan
    /// or absence probe. This never authorizes a new unreserved mutation.
    fn reconcile_pending_namespace(&self) -> io::Result<()>;
    /// Authenticate the actual listing prefix below the held canonical root.
    /// This scope check does not claim aggregate directory-FD or scan budgets.
    fn validate_list_scope(&self, path: &StdPath) -> io::Result<()>;
    /// Authenticated nofollow preparation of every missing private parent.
    /// This does not create or charge a regular-file inode.
    fn prepare_parent_dirs(&self, final_path: &StdPath) -> io::Result<()>;

    /// Reserve one new inode/file slot before creating a temp or final path.
    fn begin_create(&self, final_path: &StdPath) -> io::Result<Box<dyn FileWriteTicket>>;

    /// Reserve a distinct destination inode before copying source bytes.
    fn begin_byte_copy(
        &self,
        source: &File,
        final_path: &StdPath,
    ) -> io::Result<Box<dyn FileWriteTicket>>;

    /// Bind another path to an existing inode without another unique-inode
    /// byte or file-slot charge.
    fn begin_link(&self, source: &StdPath, target: &StdPath)
        -> io::Result<Box<dyn FilePathTicket>>;

    /// Transfer a regular file or directory, replaying an original retained
    /// move before inspecting a source that may already have disappeared.
    /// Create never replaces an existing destination. Directory moves retain
    /// the authority's create-only descendant-scope contract.
    fn begin_move(
        &self,
        source: &StdPath,
        target: &StdPath,
        mode: FileMoveMode,
    ) -> io::Result<Box<dyn FilePathTicket>>;

    /// Execute the authenticated unlink and parent sync. The authority owns
    /// any started ticket across errors and session recreation; a retry must
    /// reconcile the original inode instead of unlinking a replacement.
    fn delete_file_and_sync(&self, path: &StdPath) -> io::Result<()>;

    /// A directory move must atomically claim the entire descendant scope.
    /// The returned ticket performs the rename through authenticated dirfds.
    fn begin_directory_move(
        &self,
        source: &StdPath,
        target: &StdPath,
    ) -> io::Result<Box<dyn FileDirectoryPathTicket>>;

    /// A recursive deletion must first inventory every descendant and retain
    /// unlink custody until directory and parent sync complete.
    fn begin_directory_delete(
        &self,
        path: &StdPath,
    ) -> io::Result<Box<dyn FileDirectoryDeleteTicket>>;

    /// Guard the pathname before the file-open syscall. Unlink/replacement must
    /// retain its inode while an open is in flight, including async cancellation.
    fn begin_reader_open(&self, path: &StdPath) -> io::Result<Box<dyn FileReaderOpenTicket>>;
}

/// Custody of a new file from pre-create until data and parent directory sync.
pub trait FileWriteTicket: Debug + Send {
    /// Bind the precise private pathname before the create syscall. A caller
    /// must not create an opaque/random temp and report it after the fact.
    fn declare_temp_path(&mut self, path: &StdPath) -> io::Result<()>;
    /// Mark possible mutation before creating a temp file or asking the
    /// underlying object store to start a write. Drop after this point must
    /// retain an uncertain charge until physical absence is authenticated.
    fn creation_started(&mut self) -> io::Result<()>;
    /// Authenticate the original writer FD and declared staging inode. The
    /// caller retains this exact FD until checked retirement or absence proof.
    fn created(&mut self, file: &File, temporary_path: &StdPath) -> io::Result<()>;
    /// Called before any attempted write. A short/pending/failed write does not
    /// release the full `candidate_end` reservation on this ticket's Drop.
    fn reserve_end(&mut self, candidate_end: u64) -> io::Result<()>;
    /// Retain any old target inode before attempting a namespace replacement.
    fn before_replace(&mut self, final_path: &StdPath) -> io::Result<()>;
    /// Settle the published inode after both data and parent directory sync.
    fn replaced_and_synced(&mut self, file: &File, final_path: &StdPath) -> io::Result<()>;
    /// Physically close the actual writer FD after settled publication. The
    /// binder may transfer its owned lease only after this succeeds.
    fn confirmed_writer_fd_retired(&mut self, writer: File) -> io::Result<()>;
    /// Close a duplicate staging FD with an observable result. The original
    /// FD authenticated by `created` remains live until it is transferred to
    /// `confirmed_writer_fd_retired` or absent-output settlement.
    fn confirmed_auxiliary_writer_fd_retired(&mut self, writer: File) -> io::Result<()>;
    /// A failed create syscall that did not return a writer FD. Both the
    /// declared temp and final namespace states must still be authenticated.
    fn confirmed_uncreated_absent_and_synced(&mut self) -> io::Result<()>;
    /// Settle an unlinked output only after transferring the caller's actual
    /// writer FD. The authority consumes it and its own clone before refund.
    /// The caller must not retain an untracked duplicate of that writer FD.
    fn confirmed_safe_absent_after_writer_close(&mut self, writer: File) -> io::Result<()>;
}

/// Path-only mutation; target replacement and aliases remain separately held.
pub trait FilePathTicket: Debug + Send {
    /// Execute and settle the original authenticated link or move. The owner
    /// retains a started ticket across caller errors and dropped handles.
    fn perform_and_sync(&mut self) -> io::Result<()>;
}

/// Namespace ticket for a whole directory tree, not an individual file.
pub trait FileDirectoryPathTicket: Debug + Send {
    /// Rename the inventoried descendant scope and sync affected directories.
    fn perform_and_sync(&mut self) -> io::Result<()>;
}

/// The authority owns the authenticated recursive unlink operation.
pub trait FileDirectoryDeleteTicket: Debug + Send {
    /// Remove the authenticated descendant scope and sync the parent directory.
    fn perform_and_sync(&mut self) -> io::Result<()>;
}

/// Acquired before an FD is opened. `opened` binds the exact inode, then the
/// caller may release this guard only after it holds the returned lease.
pub trait FileReaderOpenTicket: Debug + Send {
    /// Transfer the actual opened FD into its inode pin and returned lease.
    fn opened_owned(&mut self, file: File) -> io::Result<Arc<dyn FileReaderLease>>;
}

/// The lease owns the actual reader FD through the final task/stream clone.
/// Its final Drop checks the physical close before retiring the inode pin.
pub trait FileReaderLease: Debug + Send + Sync {
    /// Borrow the actual FD while the reader lease keeps it open.
    fn file(&self) -> io::Result<&File>;
}
