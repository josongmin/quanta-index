//! Bounded directory namespace operations under the common publication
//! authority. Regular-file byte charge remains in `PublicationCapacity`.
use super::*;
use rustix::fs::Dir;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt as _;

fn confirmed_absent(anchor: &Anchor) -> Result<bool, CoreError> {
    match statat(
        &anchor.parent,
        Path::new(&anchor.name),
        AtFlags::SYMLINK_NOFOLLOW,
    ) {
        Err(rustix::io::Errno::NOENT) => Ok(true),
        Ok(_) => Ok(false),
        Err(error) => Err(storage_io("prove path absent", error)),
    }
}

fn require_current_anchored_parent(
    binder: &PublicationFileCapacity,
    anchor: &Anchor,
) -> Result<(), CoreError> {
    let current = binder.anchor(&anchor.relative)?;
    let checked = (|| {
        let held = anchor
            .parent
            .metadata()
            .map_err(|error| storage_io("stat held directory parent", error))?;
        let actual = current
            .parent
            .metadata()
            .map_err(|error| storage_io("stat current directory parent", error))?;
        if !held.is_dir()
            || !actual.is_dir()
            || held.dev() != actual.dev()
            || held.ino() != actual.ino()
            || anchor.name != current.name
            || anchor.relative != current.relative
        {
            return Err(contract("directory parent incarnation changed"));
        }
        Ok(())
    })();
    if let Err(error) = close_owned_file(current.parent) {
        binder.mark_close_unwitnessed();
        return Err(storage_io("close current directory parent observer", error));
    }
    checked
}

struct Scanned {
    relative: PathBuf,
    directory: bool,
    observed: InodeObservation,
    retained: Option<RetainedId>,
}
pub(super) fn directory_observation(anchor: &Anchor) -> Result<InodeObservation, CoreError> {
    let stat = statat(
        &anchor.parent,
        Path::new(&anchor.name),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(|error| {
        path_error(
            "stat anchored directory",
            error,
            MissingDisposition::NotFound,
        )
    })?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid != rustix::process::getuid().as_raw()
        || stat.st_mode & 0o022 != 0
    {
        return Err(contract("directory identity, owner, or mode invalid"));
    }
    Ok(InodeObservation {
        inode: InodeKey {
            device: crate::publication_inventory::checked_device_id(stat.st_dev)
                .map_err(|error| storage("directory device", error))?,
            inode: stat.st_ino,
        },
        bytes: 0,
        links: u64::from(stat.st_nlink),
    })
}
fn close_rejected_observer(
    binder: &PublicationFileCapacity,
    observer: File,
    primary: CoreError,
) -> CoreError {
    match close_owned_file(observer) {
        Ok(()) => primary,
        Err(error) => {
            binder.mark_close_unwitnessed();
            let context = format!("; rejected directory observer close also failed: {error}");
            match primary {
                CoreError::StorageIo {
                    kind,
                    raw_os_error,
                    mut message,
                } => {
                    message.push_str(&context);
                    CoreError::StorageIo {
                        kind,
                        raw_os_error,
                        message,
                    }
                }
                other @ (CoreError::InvalidContract(_)
                | CoreError::Typed { .. }
                | CoreError::NotReady(_)
                | CoreError::NotImplemented(_)
                | CoreError::NotFound(_)
                | CoreError::Storage(_)) => CoreError::Storage(format!("{other:?}{context}")),
            }
        }
    }
}

fn open_directory(binder: &PublicationFileCapacity, anchor: &Anchor) -> Result<File, CoreError> {
    let expected = directory_observation(anchor)?;
    let file = File::from(
        openat(
            &anchor.parent,
            Path::new(&anchor.name),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| storage_io("open anchored directory", error))?,
    );
    let check = file
        .metadata()
        .map_err(|error| storage_io("fstat directory", error))
        .and_then(|meta| {
            if !meta.is_dir()
                || meta.dev() != expected.inode.device
                || meta.ino() != expected.inode.inode
            {
                Err(contract("directory FD and pathname differ"))
            } else {
                Ok(())
            }
        });
    if let Err(error) = check {
        return Err(close_rejected_observer(binder, file, error));
    }
    Ok(file)
}
fn child_anchor(
    binder: &PublicationFileCapacity,
    parent: &File,
    name: OsString,
    relative: PathBuf,
) -> Result<Anchor, CoreError> {
    let cloned = File::from(
        parent
            .as_fd()
            .try_clone_to_owned()
            .map_err(|error| storage_io("clone parent directory", error))?,
    );
    Ok(Anchor {
        shared: Arc::clone(&binder.shared),
        parent: cloned,
        name,
        relative,
    })
}
fn scan_tree(
    binder: &PublicationFileCapacity,
    prefix: &Path,
    result: &mut Vec<Scanned>,
    before_child: &mut impl FnMut() -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    // Keep path/inode witnesses only. Retaining an Anchor per entry would
    // consume up to max_tracked_entries directory FDs.
    let mut stack = vec![(prefix.to_path_buf(), None::<InodeObservation>)];
    while let Some((current, expected)) = stack.pop() {
        let current_anchor = binder.anchor(&current)?;
        if let Some(expected) = expected
            && directory_observation(&current_anchor)? != expected
        {
            return Err(contract("directory changed during bounded scan"));
        }
        let directory = open_directory(binder, &current_anchor)?;
        let mut entries =
            Dir::read_from(&directory).map_err(|error| storage_io("read directory", error))?;
        while let Some(entry) = entries.read() {
            let entry = entry.map_err(|error| storage_io("directory entry", error))?;
            let bytes = entry.file_name().to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            if bytes.is_empty() || bytes.contains(&b'/') || bytes.contains(&0) {
                return Err(contract("directory entry basename invalid"));
            }
            before_child()?;
            if result.len() >= binder.shared.max_tracked_entries {
                return Err(capacity("directory entry registry ceiling exceeded"));
            }
            let name = OsString::from_vec(bytes.to_vec());
            let relative = current.join(&name);
            let anchor = child_anchor(binder, &directory, name, relative.clone())?;
            let stat = statat(
                &anchor.parent,
                Path::new(&anchor.name),
                AtFlags::SYMLINK_NOFOLLOW,
            )
            .map_err(|error| storage_io("stat directory child", error))?;
            if stat.st_uid != rustix::process::getuid().as_raw() || stat.st_mode & 0o022 != 0 {
                return Err(contract("directory child owner or write mode invalid"));
            }
            match FileType::from_raw_mode(stat.st_mode) {
                FileType::RegularFile => {
                    let observed = anchor
                        .stat()?
                        .ok_or_else(|| contract("directory file disappeared"))?;
                    let retained = binder
                        .shared
                        .authority
                        .retained_inode(observed.inode, observed.bytes)?
                        .ok_or_else(|| contract("directory file absent from sealed authority"))?;
                    result.push(Scanned {
                        relative,
                        directory: false,
                        observed,
                        retained: Some(retained),
                    });
                }
                FileType::Directory => {
                    let observed = directory_observation(&anchor)?;
                    result.push(Scanned {
                        relative: relative.clone(),
                        directory: true,
                        observed,
                        retained: None,
                    });
                    stack.push((relative, Some(observed)));
                }
                FileType::Symlink
                | FileType::Fifo
                | FileType::Socket
                | FileType::CharacterDevice
                | FileType::BlockDevice
                | FileType::Unknown => {
                    return Err(contract("directory tree contains unsupported inode type"));
                }
            }
        }
    }
    Ok(())
}
fn claim_scopes(binder: &PublicationFileCapacity, scopes: &[PathBuf]) -> Result<(), CoreError> {
    let mut state = lock(&binder.shared.state)?;
    require_publication_open(&state)?;
    if state.directory_scopes.len().saturating_add(scopes.len()) > binder.shared.max_tracked_entries
    {
        return Err(capacity("directory scope registry ceiling exceeded"));
    }
    for scope in scopes {
        if directory_scope_busy(&state, scope)
            || state
                .paths_busy
                .iter()
                .any(|path| path.starts_with(scope) || scope.starts_with(path))
            || state
                .opening_readers
                .keys()
                .any(|path| path.starts_with(scope) || scope.starts_with(path))
        {
            return Err(capacity("directory subtree has an active operation"));
        }
    }
    for scope in scopes {
        if !state.directory_scopes.insert(scope.clone()) {
            return Err(contract("directory scope was already claimed"));
        }
    }
    drop(state);
    Ok(())
}
fn lock_inodes(
    binder: &PublicationFileCapacity,
    scanned: &[Scanned],
) -> Result<BTreeSet<InodeKey>, CoreError> {
    let keys: BTreeSet<_> = scanned
        .iter()
        .filter(|entry| !entry.directory)
        .map(|entry| entry.observed.inode)
        .collect();
    let mut state = lock(&binder.shared.state)?;
    if keys.iter().any(|key| state.inodes_busy.contains(key)) {
        return Err(capacity("directory child inode has an active mutation"));
    }
    state.inodes_busy.extend(keys.iter().copied());
    drop(state);
    Ok(keys)
}
fn release_scopes(
    binder: &PublicationFileCapacity,
    scopes: &[PathBuf],
    inodes: &BTreeSet<InodeKey>,
) {
    let all_removed = binder.shared.state.lock().is_ok_and(|mut state| {
        let mut complete = true;
        for scope in scopes {
            complete &= state.directory_scopes.remove(scope);
        }
        for inode in inodes {
            complete &= state.inodes_busy.remove(inode);
        }
        complete
    });
    if !all_removed {
        binder.mark_close_unwitnessed();
    }
}

pub struct DirectoryPathTicket {
    binder: PublicationFileCapacity,
    from: Anchor,
    to: Anchor,
    source: Option<File>,
    source_identity: InodeObservation,
    scanned: Vec<Scanned>,
    inodes: BTreeSet<InodeKey>,
    started: bool,
    settled: bool,
}
impl PublicationFileCapacity {
    pub fn begin_directory_move(
        &self,
        from: &Path,
        to: &Path,
    ) -> Result<DirectoryPathTicket, CoreError> {
        let from = self.anchor(from)?;
        let to = self.anchor(to)?;
        if from.relative == to.relative
            || from.relative.starts_with(&to.relative)
            || to.relative.starts_with(&from.relative)
        {
            return Err(contract("directory move source/target conflict"));
        }
        if !confirmed_absent(&to)? {
            return Err(existing_target("directory move"));
        }
        let scopes = [from.relative.clone(), to.relative.clone()];
        claim_scopes(self, &scopes)?;
        let mut opened_source = None;
        let outcome = (|| {
            let source_identity = directory_observation(&from)?;
            opened_source = Some(open_directory(self, &from)?);
            let mut scanned = Vec::new();
            scan_tree(self, &from.relative, &mut scanned, &mut || Ok(()))?;
            let inodes = lock_inodes(self, &scanned)?;
            Ok(DirectoryPathTicket {
                binder: self.clone(),
                from,
                to,
                source: Some(
                    opened_source
                        .take()
                        .ok_or_else(|| contract("directory move source FD absent"))?,
                ),
                source_identity,
                scanned,
                inodes,
                started: false,
                settled: false,
            })
        })();
        let outcome = outcome.map_err(|error| match opened_source.take() {
            Some(source) => close_rejected_observer(self, source, error),
            None => error,
        });
        if outcome.is_err() {
            release_scopes(self, &scopes, &BTreeSet::new());
        }
        outcome
    }
    pub fn begin_directory_delete(
        &self,
        path: &Path,
        before_child: &mut impl FnMut() -> Result<(), CoreError>,
    ) -> Result<DirectoryDeleteTicket, CoreError> {
        let root = self.anchor(path)?;
        let scopes = [root.relative.clone()];
        claim_scopes(self, &scopes)?;
        let mut opened_directory = None;
        let outcome = (|| {
            let root_identity = directory_observation(&root)?;
            opened_directory = Some(open_directory(self, &root)?);
            let mut scanned = Vec::new();
            scan_tree(self, &root.relative, &mut scanned, before_child)?;
            // Reserve the bounded retirement registry before any unlink. An
            // exhausted registry must refuse the operation at admission.
            for entry in scanned.iter().filter(|entry| !entry.directory) {
                self.track_retained(
                    entry
                        .retained
                        .ok_or_else(|| contract("missing child retained ID"))?,
                    entry.observed,
                )?;
            }
            let aliases = delete_alias_plan(&scanned)?;
            let zero_link_inodes = aliases
                .iter()
                .filter(|(_inode, (count, links, _bytes, _retained))| count == links)
                .map(|(inode, (_count, _links, bytes, retained))| (*inode, *bytes, *retained))
                .collect();
            let inodes = lock_inodes(self, &scanned)?;
            Ok(DirectoryDeleteTicket {
                binder: self.clone(),
                root,
                directory: Some(
                    opened_directory
                        .take()
                        .ok_or_else(|| contract("directory delete root FD absent"))?,
                ),
                root_identity,
                scanned,
                aliases,
                zero_link_inodes,
                removed: BTreeMap::new(),
                cursor: 0,
                pending: None,
                root_phase: RootDeletePhase::Present,
                tombstone_cursor: 0,
                inodes,
                started: false,
                settled: false,
            })
        })();
        let outcome = outcome.map_err(|error| match opened_directory.take() {
            Some(directory) => close_rejected_observer(self, directory, error),
            None => error,
        });
        if outcome.is_err() {
            release_scopes(self, &scopes, &BTreeSet::new());
        }
        outcome
    }
}

type DeleteAliasPlan = BTreeMap<InodeKey, (u64, u64, u64, RetainedId)>;

fn delete_alias_plan(scanned: &[Scanned]) -> Result<DeleteAliasPlan, CoreError> {
    let mut aliases = BTreeMap::new();
    for entry in scanned.iter().filter(|entry| !entry.directory) {
        let retained = entry
            .retained
            .ok_or_else(|| contract("missing child retained ID"))?;
        let record = aliases.entry(entry.observed.inode).or_insert((
            0_u64,
            entry.observed.links,
            entry.observed.bytes,
            retained,
        ));
        if record.1 != entry.observed.links
            || record.2 != entry.observed.bytes
            || record.3 != retained
        {
            return Err(contract(
                "directory aliases have inconsistent inode witness",
            ));
        }
        record.0 = record
            .0
            .checked_add(1)
            .ok_or_else(|| capacity("directory alias count overflow"))?;
        if record.0 > record.1 {
            return Err(contract("directory aliases exceed inode link count"));
        }
    }
    Ok(aliases)
}
impl DirectoryPathTicket {
    #[must_use]
    pub fn has_started(&self) -> bool {
        self.started
    }

    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.settled
    }

    pub fn change_started(&mut self) -> Result<(), CoreError> {
        require_current_anchored_parent(&self.binder, &self.from)?;
        require_current_anchored_parent(&self.binder, &self.to)?;
        if self.started
            || self.settled
            || directory_observation(&self.from)? != self.source_identity
            || self.to.stat()?.is_some()
        {
            return Err(contract("directory move changed before syscall"));
        }
        self.started = true;
        Ok(())
    }
    pub fn perform_and_sync(&mut self) -> Result<(), CoreError> {
        self.change_started()?;
        renameat_with(
            &self.from.parent,
            Path::new(&self.from.name),
            &self.to.parent,
            Path::new(&self.to.name),
            RenameFlags::NOREPLACE,
        )
        .map_err(|error| storage_io("renameat directory", error))?;
        self.changed_and_synced()
    }
    pub fn changed_and_synced(&mut self) -> Result<(), CoreError> {
        require_current_anchored_parent(&self.binder, &self.from)?;
        require_current_anchored_parent(&self.binder, &self.to)?;
        if !self.started
            || directory_observation(&self.to)? != self.source_identity
            || !confirmed_absent(&self.from)?
        {
            return Err(contract("directory move lacks destination/source proof"));
        }
        for entry in &self.scanned {
            let suffix = entry
                .relative
                .strip_prefix(&self.from.relative)
                .map_err(|error| {
                    CoreError::InvalidContract(format!(
                        "directory move child escaped source: {error}"
                    ))
                })?;
            let moved = self.binder.anchor(&self.to.relative.join(suffix))?;
            if entry.directory {
                if directory_observation(&moved)? != entry.observed {
                    return Err(contract("moved directory child changed"));
                }
            } else if moved.stat()? != Some(entry.observed) {
                return Err(contract("moved file child changed"));
            }
        }
        self.source
            .as_ref()
            .ok_or_else(|| contract("moved directory observer FD absent"))?
            .sync_all()
            .map_err(|error| storage_io("moved directory fsync", error))?;
        self.from.sync_parent()?;
        self.to.sync_parent()?;
        require_current_anchored_parent(&self.binder, &self.from)?;
        require_current_anchored_parent(&self.binder, &self.to)?;
        self.binder.shared.root.require_current_v1()?;
        self.close_source()?;
        self.settled = true;
        release_scopes(
            &self.binder,
            &[self.from.relative.clone(), self.to.relative.clone()],
            &self.inodes,
        );
        Ok(())
    }

    /// Reconcile the original started move without issuing another rename.
    /// A foreign or mixed namespace state keeps the original claims held.
    pub fn reconcile_after_failure(&mut self) -> Result<DirectoryMoveRecovery, CoreError> {
        if !self.started || self.settled {
            return Err(contract(
                "directory move has no started operation to reconcile",
            ));
        }
        require_current_anchored_parent(&self.binder, &self.from)?;
        require_current_anchored_parent(&self.binder, &self.to)?;
        let source = self
            .source
            .as_ref()
            .ok_or_else(|| contract("directory move observer FD absent"))?
            .metadata()
            .map_err(|error| storage_io("fstat directory move observer", error))?;
        if !source.is_dir()
            || source.dev() != self.source_identity.inode.device
            || source.ino() != self.source_identity.inode.inode
        {
            return Err(contract("directory move observer identity changed"));
        }
        if confirmed_absent(&self.from)? {
            self.changed_and_synced()?;
            return Ok(DirectoryMoveRecovery::Moved);
        }
        if directory_observation(&self.from)? != self.source_identity
            || !confirmed_absent(&self.to)?
        {
            return Err(contract("failed directory move has mixed namespace state"));
        }
        for entry in &self.scanned {
            let anchor = self.binder.anchor(&entry.relative)?;
            if entry.directory {
                if directory_observation(&anchor)? != entry.observed {
                    return Err(contract("failed directory move child changed"));
                }
            } else if anchor.stat()? != Some(entry.observed) {
                return Err(contract("failed directory move file changed"));
            }
        }
        self.source
            .as_ref()
            .ok_or_else(|| contract("directory move observer FD absent"))?
            .sync_all()
            .map_err(|error| storage_io("unchanged directory fsync", error))?;
        self.from.sync_parent()?;
        self.to.sync_parent()?;
        require_current_anchored_parent(&self.binder, &self.from)?;
        require_current_anchored_parent(&self.binder, &self.to)?;
        self.binder.shared.root.require_current_v1()?;
        self.close_source()?;
        self.settled = true;
        release_scopes(
            &self.binder,
            &[self.from.relative.clone(), self.to.relative.clone()],
            &self.inodes,
        );
        Ok(DirectoryMoveRecovery::Unchanged)
    }

    fn close_source(&mut self) -> Result<(), CoreError> {
        let source = self
            .source
            .take()
            .ok_or_else(|| contract("directory move observer FD absent"))?;
        if let Err(error) = close_owned_file(source) {
            self.binder.mark_close_unwitnessed();
            return Err(storage_io("close directory move observer", error));
        }
        Ok(())
    }
}
impl Drop for DirectoryPathTicket {
    fn drop(&mut self) {
        if let Some(source) = self.source.take()
            && close_owned_file(source).is_err()
        {
            self.binder.mark_close_unwitnessed();
        }
        if !self.started && !self.settled {
            release_scopes(
                &self.binder,
                &[self.from.relative.clone(), self.to.relative.clone()],
                &self.inodes,
            );
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectoryMoveRecovery {
    Moved,
    Unchanged,
}

pub struct DirectoryDeleteTicket {
    binder: PublicationFileCapacity,
    root: Anchor,
    directory: Option<File>,
    root_identity: InodeObservation,
    scanned: Vec<Scanned>,
    aliases: DeleteAliasPlan,
    zero_link_inodes: Vec<(InodeKey, u64, RetainedId)>,
    removed: BTreeMap<InodeKey, u64>,
    cursor: usize,
    pending: Option<PendingUnlink>,
    root_phase: RootDeletePhase,
    tombstone_cursor: usize,
    inodes: BTreeSet<InodeKey>,
    started: bool,
    settled: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RootDeletePhase {
    Present,
    UnlinkAttempted,
    UnlinkSucceeded,
    Removed,
}

struct PendingUnlink {
    anchor: Anchor,
    observer: Option<File>,
    inode: InodeKey,
    bytes: u64,
    links_before: u64,
    removed_after: Option<u64>,
    directory: bool,
    attempted: bool,
    unlink_succeeded: bool,
}

#[derive(Clone, Copy)]
enum DeleteBoundary {
    EntryUnlinked,
    RootUnlinked,
}

impl DirectoryDeleteTicket {
    /// Frozen path-length receipt; hardlink aliases count separately here.
    ///
    /// Physical refunds remain controlled by unique inode lifetime.
    pub fn planned_file_bytes(&self) -> Result<u64, CoreError> {
        self.scanned
            .iter()
            .filter(|entry| !entry.directory)
            .try_fold(0_u64, |total, entry| {
                total
                    .checked_add(entry.observed.bytes)
                    .ok_or_else(|| capacity("directory delete path-byte receipt overflow"))
            })
    }

    #[must_use]
    pub fn has_started(&self) -> bool {
        self.started
    }

    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.settled
    }

    pub fn deletion_started(&mut self) -> Result<(), CoreError> {
        require_current_anchored_parent(&self.binder, &self.root)?;
        if self.started || self.settled || directory_observation(&self.root)? != self.root_identity
        {
            return Err(contract("directory delete changed before syscall"));
        }
        self.started = true;
        Ok(())
    }
    pub fn perform_and_sync(
        &mut self,
        before_child: &mut impl FnMut() -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        self.perform_and_sync_inner(before_child, &mut |_boundary| Ok(()))
    }

    fn perform_and_sync_inner(
        &mut self,
        before_child: &mut impl FnMut() -> Result<(), CoreError>,
        after_unlink: &mut impl FnMut(DeleteBoundary) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        if self.settled {
            return self.binder.sweep_retired();
        }
        if !self.started {
            // Cancellation is honored only before the first destructive syscall.
            for _entry in &self.scanned {
                before_child()?;
            }
            before_child()?;
            self.deletion_started()?;
        }
        while self.cursor < self.scanned.len() {
            if self.pending.is_none() {
                self.prepare_next_entry()?;
            }
            self.finish_pending_entry(after_unlink)?;
        }
        self.finish_root(after_unlink)?;
        for (inode, (count, _links, _bytes, _retained)) in &self.aliases {
            if self.removed.get(inode).copied().unwrap_or(0) != *count {
                return Err(contract("directory delete alias progress incomplete"));
            }
        }
        self.binder.shared.root.require_current_v1()?;
        while let Some((inode, bytes, retained)) =
            self.zero_link_inodes.get(self.tombstone_cursor).copied()
        {
            self.binder.track_retained(
                retained,
                InodeObservation {
                    inode,
                    bytes,
                    links: 0,
                },
            )?;
            let mut state = lock(&self.binder.shared.state)?;
            state
                .inodes
                .get_mut(&inode)
                .ok_or_else(|| contract("deleted child inode missing"))?
                .tombstone_synced = true;
            state.may_have_tombstones = true;
            drop(state);
            self.tombstone_cursor = self
                .tombstone_cursor
                .checked_add(1)
                .ok_or_else(|| contract("directory tombstone cursor overflow"))?;
        }
        self.settled = true;
        release_scopes(
            &self.binder,
            std::slice::from_ref(&self.root.relative),
            &self.inodes,
        );
        self.binder.sweep_retired()
    }

    fn prepare_next_entry(&mut self) -> Result<(), CoreError> {
        let next_cursor = self
            .cursor
            .checked_add(1)
            .ok_or_else(|| contract("directory delete cursor overflow"))?;
        let index = self
            .scanned
            .len()
            .checked_sub(next_cursor)
            .ok_or_else(|| contract("directory delete cursor exhausted"))?;
        let entry = self
            .scanned
            .get(index)
            .ok_or_else(|| contract("directory delete entry absent at cursor"))?;
        let anchor = self.binder.anchor(&entry.relative)?;
        let (observer, links_before, removed_after) = if entry.directory {
            let current = directory_observation(&anchor)?;
            if current.inode != entry.observed.inode {
                return Err(contract("directory delete child changed before unlink"));
            }
            (open_directory(&self.binder, &anchor)?, current.links, None)
        } else {
            let already = self
                .removed
                .get(&entry.observed.inode)
                .copied()
                .unwrap_or(0);
            let mut expected = entry.observed;
            expected.links = expected
                .links
                .checked_sub(already)
                .ok_or_else(|| contract("directory alias link count underflow"))?;
            if anchor.stat()? != Some(expected) {
                return Err(contract("directory delete file changed before unlink"));
            }
            let observer = anchor.open()?;
            let check = InodeObservation::from_file(&observer).and_then(|actual| {
                if actual == expected {
                    Ok(())
                } else {
                    Err(contract("directory delete observer differs from pathname"))
                }
            });
            if let Err(primary) = check {
                return Err(close_rejected_observer(&self.binder, observer, primary));
            }
            let next = already
                .checked_add(1)
                .ok_or_else(|| contract("directory alias removal count overflow"))?;
            (observer, expected.links, Some(next))
        };
        self.pending = Some(PendingUnlink {
            anchor,
            observer: Some(observer),
            inode: entry.observed.inode,
            bytes: entry.observed.bytes,
            links_before,
            removed_after,
            directory: entry.directory,
            attempted: false,
            unlink_succeeded: false,
        });
        Ok(())
    }

    fn finish_pending_entry(
        &mut self,
        after_unlink: &mut impl FnMut(DeleteBoundary) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        let pending = self
            .pending
            .as_mut()
            .ok_or_else(|| contract("directory delete pending entry absent"))?;
        require_current_anchored_parent(&self.binder, &pending.anchor)?;
        if pending.directory && pending.unlink_succeeded && !confirmed_absent(&pending.anchor)? {
            return Err(contract("removed directory pathname was replaced"));
        }
        if !pending.attempted || !confirmed_absent(&pending.anchor)? {
            pending.verify_unchanged()?;
            pending.attempted = true;
            unlinkat(
                &pending.anchor.parent,
                Path::new(&pending.anchor.name),
                if pending.directory {
                    AtFlags::REMOVEDIR
                } else {
                    AtFlags::empty()
                },
            )
            .map_err(|error| storage_io("unlinkat directory entry", error))?;
            pending.unlink_succeeded = true;
            after_unlink(DeleteBoundary::EntryUnlinked)?;
        }
        pending.verify_removed()?;
        require_current_anchored_parent(&self.binder, &pending.anchor)?;
        pending.anchor.sync_parent()?;
        require_current_anchored_parent(&self.binder, &pending.anchor)?;
        pending.close_observer(&self.binder)?;
        if let Some(next) = pending.removed_after {
            let _previous = self.removed.insert(pending.inode, next);
        }
        self.pending = None;
        self.cursor = self
            .cursor
            .checked_add(1)
            .ok_or_else(|| contract("directory delete cursor overflow"))?;
        Ok(())
    }

    fn finish_root(
        &mut self,
        after_unlink: &mut impl FnMut(DeleteBoundary) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        if self.root_phase == RootDeletePhase::Removed {
            return Ok(());
        }
        require_current_anchored_parent(&self.binder, &self.root)?;
        if self.root_phase == RootDeletePhase::UnlinkSucceeded && !confirmed_absent(&self.root)? {
            return Err(contract("removed root directory pathname was replaced"));
        }
        if self.root_phase == RootDeletePhase::Present || !confirmed_absent(&self.root)? {
            // Removing child directories changes the root's link count, but
            // must not change the held root inode.
            if directory_observation(&self.root)?.inode != self.root_identity.inode {
                return Err(contract("directory delete root changed before unlink"));
            }
            self.root_phase = RootDeletePhase::UnlinkAttempted;
            unlinkat(
                &self.root.parent,
                Path::new(&self.root.name),
                AtFlags::REMOVEDIR,
            )
            .map_err(|error| storage_io("unlinkat root directory", error))?;
            self.root_phase = RootDeletePhase::UnlinkSucceeded;
            after_unlink(DeleteBoundary::RootUnlinked)?;
        }
        if !confirmed_absent(&self.root)? {
            return Err(contract("deleted root pathname still present"));
        }
        if self.root_phase != RootDeletePhase::UnlinkSucceeded {
            return Err(contract("absent root lacks successful rmdir witness"));
        }
        let directory = self
            .directory
            .as_ref()
            .ok_or_else(|| contract("deleted root observer FD absent"))?;
        let metadata = directory
            .metadata()
            .map_err(|error| storage_io("fstat deleted root directory", error))?;
        if !metadata.is_dir()
            || metadata.dev() != self.root_identity.inode.device
            || metadata.ino() != self.root_identity.inode.inode
        {
            return Err(contract("deleted root observer identity changed"));
        }
        directory
            .sync_all()
            .map_err(|error| storage_io("deleted directory fsync", error))?;
        require_current_anchored_parent(&self.binder, &self.root)?;
        self.root.sync_parent()?;
        require_current_anchored_parent(&self.binder, &self.root)?;
        self.binder.shared.root.require_current_v1()?;
        let directory = self
            .directory
            .take()
            .ok_or_else(|| contract("deleted root observer FD absent"))?;
        if let Err(error) = close_owned_file(directory) {
            self.binder.mark_close_unwitnessed();
            return Err(storage_io("close deleted root observer", error));
        }
        self.root_phase = RootDeletePhase::Removed;
        Ok(())
    }
}

impl PendingUnlink {
    fn verify_unchanged(&self) -> Result<(), CoreError> {
        let observer = self
            .observer
            .as_ref()
            .ok_or_else(|| contract("directory delete observer FD absent"))?;
        if self.directory {
            let current = directory_observation(&self.anchor)?;
            let expected = InodeObservation {
                inode: self.inode,
                bytes: 0,
                links: self.links_before,
            };
            if current != expected {
                return Err(contract("directory delete original directory changed"));
            }
            let metadata = observer
                .metadata()
                .map_err(|error| storage_io("fstat directory delete observer", error))?;
            if !metadata.is_dir()
                || metadata.dev() != self.inode.device
                || metadata.ino() != self.inode.inode
                || metadata.nlink() != current.links
            {
                return Err(contract("directory delete original directory changed"));
            }
        } else {
            let expected = InodeObservation {
                inode: self.inode,
                bytes: self.bytes,
                links: self.links_before,
            };
            if self.anchor.stat()? != Some(expected)
                || InodeObservation::from_file(observer)? != expected
            {
                return Err(contract("directory delete original file changed"));
            }
        }
        Ok(())
    }

    fn verify_removed(&self) -> Result<(), CoreError> {
        if !confirmed_absent(&self.anchor)? {
            return Err(contract(
                "directory delete attempted pathname still present",
            ));
        }
        let observer = self
            .observer
            .as_ref()
            .ok_or_else(|| contract("directory delete observer FD absent"))?;
        if self.directory {
            if !self.unlink_succeeded {
                return Err(contract("absent directory lacks successful rmdir witness"));
            }
            let metadata = observer
                .metadata()
                .map_err(|error| storage_io("fstat removed directory", error))?;
            if !metadata.is_dir()
                || metadata.dev() != self.inode.device
                || metadata.ino() != self.inode.inode
            {
                return Err(contract("removed directory observer identity changed"));
            }
        } else {
            let expected_links = self
                .links_before
                .checked_sub(1)
                .ok_or_else(|| contract("directory delete file link count underflow"))?;
            if InodeObservation::from_file(observer)?
                != (InodeObservation {
                    inode: self.inode,
                    bytes: self.bytes,
                    links: expected_links,
                })
            {
                return Err(contract("removed file lacks exact observer link proof"));
            }
        }
        Ok(())
    }

    fn close_observer(&mut self, binder: &PublicationFileCapacity) -> Result<(), CoreError> {
        let observer = self
            .observer
            .take()
            .ok_or_else(|| contract("directory delete observer FD absent"))?;
        if let Err(error) = close_owned_file(observer) {
            binder.mark_close_unwitnessed();
            return Err(storage_io("close directory delete observer", error));
        }
        Ok(())
    }
}

impl Drop for DirectoryDeleteTicket {
    fn drop(&mut self) {
        if let Some(pending) = self.pending.as_mut()
            && let Some(observer) = pending.observer.take()
            && close_owned_file(observer).is_err()
        {
            self.binder.mark_close_unwitnessed();
        }
        if let Some(directory) = self.directory.take()
            && close_owned_file(directory).is_err()
        {
            self.binder.mark_close_unwitnessed();
        }
        if !self.started && !self.settled {
            release_scopes(
                &self.binder,
                std::slice::from_ref(&self.root.relative),
                &self.inodes,
            );
        }
    }
}

#[cfg(test)]
#[path = "publication_directory_capacity/tests.rs"]
mod tests;
