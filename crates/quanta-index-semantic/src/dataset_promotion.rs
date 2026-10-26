//! Crash-recoverable dataset/contract promotion with one durable intent.
//!
//! The marker name is the phase. Its bounded, immutable payload identifies
//! both old and new directory inodes and both contract inodes and digests.
//! The caller holds the target generation mutation/write guard through each
//! promotion or recovery operation.

#![expect(
    clippy::redundant_pub_crate,
    reason = "sibling build module needs crate-visible API while unreachable_pub is denied"
)]

#[cfg(test)]
use std::cell::Cell;
use std::fs::{self, File};
use std::io::{Cursor, Read as _};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lance_io::object_store::file_admission::FileAdmission;
use quanta_index_core::CoreError;
use quanta_index_core::publication_admission::PublicationOutputAdmission;
use rustix::fs::{Mode, OFlags, open, openat};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::control_file::MAX_GENERATION_CONTRACT_BYTES;
use crate::durable_write::{AtomicOutput, write_atomic};
use crate::errors::{admitted_fs_err, fs_err};
use crate::layout;

const INTENT_VERSION: u8 = 1;
const MAX_INTENT_BYTES: usize = 512;
const MAX_STAGING_DEPTH: usize = 64;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Inode {
    device: u64,
    inode: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Contract {
    inode: Inode,
    bytes: u64,
    sha256: [u8; 32],
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Intent {
    version: u8,
    old_dataset: Option<Inode>,
    new_dataset: Inode,
    old_contract: Option<Contract>,
    new_contract: Contract,
}

struct Paths {
    dataset: PathBuf,
    staging: PathBuf,
    backup: PathBuf,
    aborted: PathBuf,
    contract: PathBuf,
    next: PathBuf,
    promotion: PathBuf,
    rollback: PathBuf,
    generation: PathBuf,
}

impl Paths {
    fn new(generation: &Path) -> Self {
        let contract = layout::build_contract_path(generation);
        Self {
            dataset: layout::dataset_dir(generation),
            staging: generation.join("dataset.staging"),
            backup: generation.join("dataset.backup"),
            aborted: generation.join("dataset.aborted"),
            next: contract.with_extension("next"),
            contract,
            promotion: generation.join("dataset.promotion"),
            rollback: generation.join("dataset.rollback"),
            generation: generation.to_path_buf(),
        }
    }
}

#[derive(Default)]
struct FsOps {
    admission: Option<Arc<dyn FileAdmission>>,
    #[cfg(test)]
    fault: Option<(&'static str, usize)>,
    #[cfg(test)]
    seen: Cell<usize>,
}

impl FsOps {
    fn reconcile_namespace(&self) -> Result<(), CoreError> {
        if let Some(admission) = &self.admission {
            admission
                .reconcile_pending_namespace()
                .map_err(|error| admitted_fs_err("reconcile dataset promotion namespace", error))?;
        }
        Ok(())
    }
    #[cfg(test)]
    fn with_fault(operation: &'static str, occurrence: usize) -> Self {
        Self {
            admission: None,
            fault: Some((operation, occurrence)),
            seen: Cell::new(0),
        }
    }

    #[cfg(test)]
    fn check(&self, operation: &'static str) -> Result<(), CoreError> {
        if let Some((target, occurrence)) = self.fault
            && operation == target
        {
            let seen = self
                .seen
                .get()
                .checked_add(1)
                .ok_or_else(|| corrupt("fault occurrence overflow"))?;
            self.seen.set(seen);
            if seen == occurrence {
                return Err(CoreError::Storage(format!(
                    "semantic dataset promotion injected {operation} failure"
                )));
            }
        }
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<(), CoreError> {
        #[cfg(test)]
        self.check("rename")?;
        crate::qualified_fs::rename(from, to, self.admission.as_deref())
    }

    fn sync(&self, generation: &Path) -> Result<(), CoreError> {
        #[cfg(test)]
        self.check("sync")?;
        File::open(generation)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| fs_err("sync dataset promotion directory", generation, &error))
    }

    fn sync_staging_entry(&self, opened: &File, path: &Path) -> Result<(), CoreError> {
        #[cfg(test)]
        self.check("fence_sync")?;
        opened
            .sync_all()
            .map_err(|error| fs_err("sync staged dataset entry", path, &error))
    }

    fn remove_file(&self, path: &Path) -> Result<(), CoreError> {
        #[cfg(test)]
        self.check("remove_file")?;
        crate::qualified_fs::remove_file(path, self.admission.as_deref())
    }

    fn remove_dir(&self, path: &Path) -> Result<(), CoreError> {
        #[cfg(test)]
        self.check("remove_dir")?;
        crate::qualified_fs::remove_dir_all(path, self.admission.as_deref())
    }
}

fn corrupt(detail: &str) -> CoreError {
    CoreError::Storage(format!("semantic dataset promotion: {detail}"))
}

fn open_child(path: &Path, directory: bool) -> Result<Option<File>, CoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| corrupt("promotion entry has no parent directory"))?;
    let name = path
        .file_name()
        .ok_or_else(|| corrupt("promotion entry has no file name"))?;
    let parent = open(
        parent,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::DIRECTORY | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|error| {
        CoreError::Storage(format!(
            "semantic dataset promotion open generation parent nofollow: {error}"
        ))
    })?;
    let mut flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
    if directory {
        flags |= OFlags::DIRECTORY;
    }
    match openat(&parent, Path::new(name), flags, Mode::empty()) {
        Ok(opened) => Ok(Some(File::from(opened))),
        Err(error) if error == rustix::io::Errno::NOENT => Ok(None),
        Err(error) => Err(CoreError::Storage(format!(
            "semantic dataset promotion open {} nofollow: {error}",
            path.display()
        ))),
    }
}

fn inode(metadata: &fs::Metadata) -> Inode {
    Inode {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

fn directory_inode(path: &Path) -> Result<Option<Inode>, CoreError> {
    let Some(opened) = open_child(path, true)? else {
        return Ok(None);
    };
    let metadata = opened
        .metadata()
        .map_err(|error| fs_err("stat dataset promotion directory", path, &error))?;
    if !metadata.is_dir() {
        return Err(corrupt("expected a real dataset directory"));
    }
    Ok(Some(inode(&metadata)))
}

fn fence_staging_tree(ops: &FsOps, directory: &Path, depth: usize) -> Result<(), CoreError> {
    if depth > MAX_STAGING_DEPTH {
        return Err(corrupt("staging directory depth exceeds supported bound"));
    }
    let opened = open_child(directory, true)?
        .ok_or_else(|| corrupt("staging directory disappeared during durability fence"))?;
    let entries = fs::read_dir(directory)
        .map_err(|error| fs_err("read staged dataset directory", directory, &error))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| fs_err("read staged dataset entry", directory, &error))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| fs_err("stat staged dataset entry", &path, &error))?;
        if metadata.is_dir() {
            let next_depth = depth
                .checked_add(1)
                .ok_or_else(|| corrupt("staging directory depth overflow"))?;
            fence_staging_tree(ops, &path, next_depth)?;
        } else if metadata.is_file() {
            let file = open_child(&path, false)?
                .ok_or_else(|| corrupt("staged file disappeared during durability fence"))?;
            if !file
                .metadata()
                .map_err(|error| fs_err("stat opened staged file", &path, &error))?
                .is_file()
            {
                return Err(corrupt("staged file changed type during durability fence"));
            }
            ops.sync_staging_entry(&file, &path)?;
        } else {
            return Err(corrupt(
                "staging tree contains a symlink or nonregular entry",
            ));
        }
    }
    ops.sync_staging_entry(&opened, directory)
}

fn contract(path: &Path, limit: usize) -> Result<Option<Contract>, CoreError> {
    let Some(opened) = open_child(path, false)? else {
        return Ok(None);
    };
    let metadata = opened
        .metadata()
        .map_err(|error| fs_err("stat dataset promotion contract", path, &error))?;
    if !metadata.is_file()
        || metadata.len() > u64::try_from(limit).map_err(|error| corrupt(&error.to_string()))?
    {
        return Err(corrupt("contract is not a bounded regular file"));
    }
    let mut bytes = Vec::new();
    let max_read = u64::try_from(limit)
        .map_err(|error| corrupt(&error.to_string()))?
        .checked_add(1)
        .ok_or_else(|| corrupt("contract read bound overflow"))?;
    let read_bytes = opened
        .take(max_read)
        .read_to_end(&mut bytes)
        .map_err(|error| fs_err("read dataset promotion contract", path, &error))?;
    if read_bytes > limit
        || u64::try_from(read_bytes).map_err(|error| corrupt(&error.to_string()))? != metadata.len()
    {
        return Err(corrupt("contract length changed or exceeded bound"));
    }
    Ok(Some(Contract {
        inode: inode(&metadata),
        bytes: metadata.len(),
        sha256: Sha256::digest(&bytes).into(),
    }))
}

fn require<T: Eq + std::fmt::Debug + ?Sized>(
    observed: &T,
    expected: &T,
    label: &str,
) -> Result<(), CoreError> {
    if observed == expected {
        Ok(())
    } else {
        Err(corrupt(&format!(
            "{label} identity differs: observed {observed:?}, expected {expected:?}"
        )))
    }
}

fn intent_bytes(intent: &Intent) -> Result<Vec<u8>, CoreError> {
    let mut bytes = Vec::new();
    ciborium::into_writer(intent, &mut bytes)
        .map_err(|error| corrupt(&format!("encode intent: {error}")))?;
    if bytes.len() > MAX_INTENT_BYTES {
        return Err(corrupt("intent exceeds bounded format"));
    }
    Ok(bytes)
}

fn read_intent(path: &Path) -> Result<Option<Intent>, CoreError> {
    let Some(opened) = open_child(path, false)? else {
        return Ok(None);
    };
    let metadata = opened
        .metadata()
        .map_err(|error| fs_err("stat dataset promotion intent", path, &error))?;
    let max_bytes = u64::try_from(MAX_INTENT_BYTES)
        .map_err(|error| corrupt(&format!("intent bound conversion: {error}")))?;
    if !metadata.is_file() || metadata.len() > max_bytes {
        return Err(corrupt("intent is not a bounded regular file"));
    }
    let mut bytes = Vec::new();
    let read_bytes = opened
        .take(
            max_bytes
                .checked_add(1)
                .ok_or_else(|| corrupt("intent read bound overflow"))?,
        )
        .read_to_end(&mut bytes)
        .map_err(|error| fs_err("read dataset promotion intent", path, &error))?;
    let observed_bytes = u64::try_from(read_bytes)
        .map_err(|error| corrupt(&format!("intent width conversion: {error}")))?;
    if read_bytes > MAX_INTENT_BYTES || observed_bytes != metadata.len() {
        return Err(corrupt("intent length changed or exceeded bound"));
    }
    let mut cursor = Cursor::new(bytes.as_slice());
    let intent: Intent = ciborium::from_reader(&mut cursor)
        .map_err(|error| corrupt(&format!("decode intent: {error}")))?;
    if cursor.position() != observed_bytes || intent.version != INTENT_VERSION {
        return Err(corrupt("intent has trailing bytes or unsupported version"));
    }
    if intent.old_dataset.is_some() != intent.old_contract.is_some() {
        return Err(corrupt("intent old dataset/contract pair is incomplete"));
    }
    if intent.old_dataset == Some(intent.new_dataset)
        || intent
            .old_contract
            .is_some_and(|old| old.inode == intent.new_contract.inode)
        || intent_bytes(&intent)? != bytes
    {
        return Err(corrupt(
            "intent identities overlap or encoding is noncanonical",
        ));
    }
    Ok(Some(intent))
}

fn remove_regular(
    ops: &FsOps,
    path: &Path,
    expected: Contract,
    generation: &Path,
) -> Result<(), CoreError> {
    require(
        &contract(path, MAX_GENERATION_CONTRACT_BYTES)?,
        &Some(expected),
        "removed contract",
    )?;
    ops.remove_file(path)?;
    require(
        &contract(path, MAX_GENERATION_CONTRACT_BYTES)?,
        &None,
        "removed contract",
    )?;
    ops.sync(generation)
}

fn remove_directory(
    ops: &FsOps,
    path: &Path,
    expected: Inode,
    generation: &Path,
) -> Result<(), CoreError> {
    require(&directory_inode(path)?, &Some(expected), "removed dataset")?;
    ops.remove_dir(path)?;
    require(&directory_inode(path)?, &None, "removed dataset")?;
    ops.sync(generation)
}

fn remove_intent(
    ops: &FsOps,
    path: &Path,
    expected: &Intent,
    generation: &Path,
) -> Result<(), CoreError> {
    require(
        &read_intent(path)?,
        &Some(expected.clone()),
        "settled intent",
    )?;
    ops.remove_file(path)?;
    if read_intent(path)?.is_some() {
        return Err(corrupt("intent remains after removal"));
    }
    ops.sync(generation)
}

/// One live promotion, including its charged durable intent output.
pub(crate) struct DatasetPromotion {
    paths: Paths,
    intent: Intent,
    marker: AtomicOutput,
    ops: FsOps,
}

impl DatasetPromotion {
    #[cfg(test)]
    pub(crate) fn begin(
        generation_dir: &Path,
        staged_contract: &AtomicOutput,
        publication: Option<PublicationOutputAdmission<'_>>,
    ) -> Result<Self, CoreError> {
        Self::begin_with_ops(
            generation_dir,
            staged_contract,
            publication,
            FsOps::default(),
        )
    }

    pub(crate) fn begin_with_admission(
        generation_dir: &Path,
        staged_contract: &AtomicOutput,
        publication: Option<PublicationOutputAdmission<'_>>,
        admission: Option<Arc<dyn FileAdmission>>,
    ) -> Result<Self, CoreError> {
        Self::begin_with_ops(
            generation_dir,
            staged_contract,
            publication,
            FsOps {
                admission,
                #[cfg(test)]
                fault: None,
                #[cfg(test)]
                seen: Cell::new(0),
            },
        )
    }

    fn begin_with_ops(
        generation_dir: &Path,
        staged_contract: &AtomicOutput,
        publication: Option<PublicationOutputAdmission<'_>>,
        ops: FsOps,
    ) -> Result<Self, CoreError> {
        ops.reconcile_namespace()?;
        let paths = Paths::new(generation_dir);
        require(
            staged_contract.path(),
            paths.next.as_path(),
            "staged contract path",
        )?;
        if read_intent(&paths.promotion)?.is_some()
            || read_intent(&paths.rollback)?.is_some()
            || directory_inode(&paths.backup)?.is_some()
            || directory_inode(&paths.aborted)?.is_some()
        {
            return Err(corrupt("existing transaction or debris before begin"));
        }
        let old_dataset = directory_inode(&paths.dataset)?;
        let old_contract = contract(&paths.contract, MAX_GENERATION_CONTRACT_BYTES)?;
        if old_dataset.is_some() != old_contract.is_some() {
            return Err(corrupt("old dataset/contract pair is incomplete"));
        }
        let new_dataset =
            directory_inode(&paths.staging)?.ok_or_else(|| corrupt("staging dataset is absent"))?;
        let new_contract = contract(&paths.next, MAX_GENERATION_CONTRACT_BYTES)?
            .ok_or_else(|| corrupt("staged contract is absent"))?;
        if old_dataset == Some(new_dataset)
            || old_contract.is_some_and(|old| old.inode == new_contract.inode)
        {
            return Err(corrupt("old and new identities overlap"));
        }
        // The intent may survive a crash immediately after its own fsync. All
        // staged file contents and directory entries must already be durable.
        fence_staging_tree(&ops, &paths.staging, 0)?;
        require(
            &directory_inode(&paths.staging)?,
            &Some(new_dataset),
            "fenced staging dataset",
        )?;
        let intent = Intent {
            version: INTENT_VERSION,
            old_dataset,
            new_dataset,
            old_contract,
            new_contract,
        };
        let marker = write_atomic(
            &paths.promotion,
            &intent_bytes(&intent)?,
            "write dataset promotion intent",
            publication,
        )?;
        Ok(Self {
            paths,
            intent,
            marker,
            ops,
        })
    }

    pub(crate) fn promote_dataset(&self) -> Result<(), CoreError> {
        require(
            self.marker.path(),
            self.paths.promotion.as_path(),
            "promotion intent path",
        )?;
        require(
            &read_intent(&self.paths.promotion)?,
            &Some(self.intent.clone()),
            "promotion intent",
        )?;
        require(
            &directory_inode(&self.paths.dataset)?,
            &self.intent.old_dataset,
            "old dataset",
        )?;
        require(
            &directory_inode(&self.paths.staging)?,
            &Some(self.intent.new_dataset),
            "new staging dataset",
        )?;
        require(
            &directory_inode(&self.paths.backup)?,
            &None,
            "backup dataset",
        )?;
        require(
            &directory_inode(&self.paths.aborted)?,
            &None,
            "aborted dataset",
        )?;
        require(
            &contract(&self.paths.contract, MAX_GENERATION_CONTRACT_BYTES)?,
            &self.intent.old_contract,
            "old contract",
        )?;
        require(
            &contract(&self.paths.next, MAX_GENERATION_CONTRACT_BYTES)?,
            &Some(self.intent.new_contract),
            "new contract",
        )?;
        if self.intent.old_dataset.is_some() {
            self.ops.rename(&self.paths.dataset, &self.paths.backup)?;
            self.ops.sync(&self.paths.generation)?;
        }
        self.ops.rename(&self.paths.staging, &self.paths.dataset)?;
        self.ops.sync(&self.paths.generation)?;
        Ok(())
    }

    pub(crate) fn rollback(mut self, staged_contract: AtomicOutput) -> Result<(), CoreError> {
        require(
            staged_contract.path(),
            self.paths.next.as_path(),
            "rollback staged contract path",
        )?;
        require(
            self.marker.path(),
            self.paths.promotion.as_path(),
            "rollback intent path",
        )?;
        self.marker
            .rename_to(&self.paths.rollback, "begin dataset rollback")?;
        require(
            &read_intent(&self.paths.rollback)?,
            &Some(self.intent.clone()),
            "rollback intent",
        )?;
        rollback(
            &self.paths,
            &self.intent,
            &self.ops,
            Some(staged_contract),
            Some(self.marker),
        )
    }

    pub(crate) fn finish(self) -> Result<(), CoreError> {
        require(
            self.marker.path(),
            self.paths.promotion.as_path(),
            "finish intent path",
        )?;
        require(
            &read_intent(&self.paths.promotion)?,
            &Some(self.intent.clone()),
            "finish intent",
        )?;
        finish(&self.paths, &self.intent, &self.ops, Some(self.marker))
    }

    #[cfg(test)]
    pub(crate) fn recover(generation_dir: &Path) -> Result<(), CoreError> {
        let ops = FsOps::default();
        recover(generation_dir, &ops)
    }

    pub(crate) fn recover_with_admission(
        generation_dir: &Path,
        admission: Option<Arc<dyn FileAdmission>>,
    ) -> Result<(), CoreError> {
        let ops = FsOps {
            admission,
            #[cfg(test)]
            fault: None,
            #[cfg(test)]
            seen: Cell::new(0),
        };
        recover(generation_dir, &ops)
    }
}

fn validate_locations(paths: &Paths, intent: &Intent) -> Result<[Option<Inode>; 4], CoreError> {
    let positions = [
        directory_inode(&paths.dataset)?,
        directory_inode(&paths.staging)?,
        directory_inode(&paths.backup)?,
        directory_inode(&paths.aborted)?,
    ];
    for identity in positions.into_iter().flatten() {
        if Some(identity) != intent.old_dataset && identity != intent.new_dataset {
            return Err(corrupt("dataset directory has an unrelated inode"));
        }
    }
    let old_count = positions
        .into_iter()
        .filter(|value| *value == intent.old_dataset && value.is_some())
        .count();
    let new_count = positions
        .into_iter()
        .filter(|value| *value == Some(intent.new_dataset))
        .count();
    if old_count > 1 || new_count > 1 {
        return Err(corrupt(
            "one dataset inode appears at multiple promotion paths",
        ));
    }
    Ok(positions)
}

fn finish(
    paths: &Paths,
    intent: &Intent,
    ops: &FsOps,
    marker: Option<AtomicOutput>,
) -> Result<(), CoreError> {
    let [dataset, staging, backup, aborted] = validate_locations(paths, intent)?;
    require(&dataset, &Some(intent.new_dataset), "finished dataset")?;
    require(&staging, &None, "finished staging")?;
    require(&aborted, &None, "finished aborted")?;
    if backup.is_some() && backup != intent.old_dataset {
        return Err(corrupt("backup is not the old dataset"));
    }
    require(
        &contract(&paths.contract, MAX_GENERATION_CONTRACT_BYTES)?,
        &Some(intent.new_contract),
        "finished contract",
    )?;
    require(
        &contract(&paths.next, MAX_GENERATION_CONTRACT_BYTES)?,
        &None,
        "finished staged contract",
    )?;
    // A prior contract rename may have succeeded while its parent sync failed.
    // Durably establish the new pair before removing the last old dataset.
    ops.sync(&paths.generation)?;
    if let Some(old) = backup {
        remove_directory(ops, &paths.backup, old, &paths.generation)?;
    }
    marker.map_or_else(
        || remove_intent(ops, &paths.promotion, intent, &paths.generation),
        |marker| marker.remove_and_sync("settle dataset promotion intent"),
    )
}

fn rollback(
    paths: &Paths,
    intent: &Intent,
    ops: &FsOps,
    staged: Option<AtomicOutput>,
    marker: Option<AtomicOutput>,
) -> Result<(), CoreError> {
    let [dataset, staging, backup, aborted] = validate_locations(paths, intent)?;
    require(
        &contract(&paths.contract, MAX_GENERATION_CONTRACT_BYTES)?,
        &intent.old_contract,
        "rollback old contract",
    )?;
    let next = contract(&paths.next, MAX_GENERATION_CONTRACT_BYTES)?;
    if next.is_some() && next != Some(intent.new_contract) {
        return Err(corrupt("rollback staged contract identity differs"));
    }
    if staged.is_some() && next != Some(intent.new_contract) {
        return Err(corrupt("live staged contract disappeared before rollback"));
    }
    if backup.is_some() && backup != intent.old_dataset {
        return Err(corrupt("rollback backup is not old dataset"));
    }
    if staging.is_some() && staging != Some(intent.new_dataset) {
        return Err(corrupt("rollback staging is not new dataset"));
    }
    if aborted.is_some() && aborted != Some(intent.new_dataset) {
        return Err(corrupt("rollback aborted is not new dataset"));
    }
    if dataset == Some(intent.new_dataset) {
        require(&aborted, &None, "rollback aborted before move")?;
        ops.rename(&paths.dataset, &paths.aborted)?;
        ops.sync(&paths.generation)?;
    } else if staging == Some(intent.new_dataset) {
        require(&aborted, &None, "rollback aborted before move")?;
        ops.rename(&paths.staging, &paths.aborted)?;
        ops.sync(&paths.generation)?;
    } else if dataset != intent.old_dataset && !(dataset.is_none() && backup == intent.old_dataset)
    {
        return Err(corrupt("rollback cannot locate a valid old/new dataset"));
    }
    if let Some(old) = intent.old_dataset {
        if backup == Some(old) {
            require(
                &directory_inode(&paths.dataset)?,
                &None,
                "rollback destination",
            )?;
            ops.rename(&paths.backup, &paths.dataset)?;
            ops.sync(&paths.generation)?;
        } else {
            require(
                &directory_inode(&paths.dataset)?,
                &Some(old),
                "restored old dataset",
            )?;
        }
    } else {
        require(&directory_inode(&paths.dataset)?, &None, "restored absence")?;
    }
    require(
        &directory_inode(&paths.dataset)?,
        &intent.old_dataset,
        "verified restored dataset",
    )?;
    require(
        &contract(&paths.contract, MAX_GENERATION_CONTRACT_BYTES)?,
        &intent.old_contract,
        "verified restored contract",
    )?;
    // Recovery may resume just after B→D and before that rename was synced.
    // The old pair must be durable before releasing the staged contract.
    ops.sync(&paths.generation)?;
    if let Some(staged) = staged {
        staged.remove_and_sync("remove aborted staged contract")?;
    } else if next.is_some() {
        remove_regular(ops, &paths.next, intent.new_contract, &paths.generation)?;
    }
    if directory_inode(&paths.aborted)? == Some(intent.new_dataset) {
        remove_directory(ops, &paths.aborted, intent.new_dataset, &paths.generation)?;
    }
    marker.map_or_else(
        || remove_intent(ops, &paths.rollback, intent, &paths.generation),
        |marker| marker.remove_and_sync("settle dataset rollback intent"),
    )
}

fn recover(generation_dir: &Path, ops: &FsOps) -> Result<(), CoreError> {
    ops.reconcile_namespace()?;
    match fs::symlink_metadata(generation_dir) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(fs_err("stat promotion generation", generation_dir, &error)),
        Ok(metadata) if !metadata.is_dir() => {
            return Err(corrupt("generation root is not a real directory"));
        }
        Ok(_) => {}
    }
    if directory_inode(generation_dir)?.is_none() {
        return Err(corrupt("generation root disappeared before recovery"));
    }
    let paths = Paths::new(generation_dir);
    let promotion = read_intent(&paths.promotion)?;
    let rollback_intent = read_intent(&paths.rollback)?;
    match (promotion, rollback_intent) {
        (Some(_), Some(_)) => Err(corrupt("both promotion and rollback intents exist")),
        (None, Some(intent)) => rollback(&paths, &intent, ops, None, None),
        (Some(intent), None) => {
            let [dataset, staging, backup, aborted] = validate_locations(&paths, &intent)?;
            if aborted.is_some() {
                return Err(corrupt("aborted dataset under promotion phase"));
            }
            let committed = contract(&paths.contract, MAX_GENERATION_CONTRACT_BYTES)?;
            let next = contract(&paths.next, MAX_GENERATION_CONTRACT_BYTES)?;
            if dataset == Some(intent.new_dataset) && staging.is_none() {
                if committed == Some(intent.new_contract) && next.is_none() {
                    return finish(&paths, &intent, ops, None);
                }
                if committed == intent.old_contract
                    && next == Some(intent.new_contract)
                    && backup == intent.old_dataset
                {
                    ops.rename(&paths.next, &paths.contract)?;
                    ops.sync(&paths.generation)?;
                    return finish(&paths, &intent, ops, None);
                }
                return Err(corrupt(
                    "promoted dataset has an unrecognized contract state",
                ));
            }
            if committed != intent.old_contract || next != Some(intent.new_contract) {
                return Err(corrupt("pre-swap promotion contract identity differs"));
            }
            if (dataset == intent.old_dataset
                && backup.is_none()
                && staging == Some(intent.new_dataset))
                || (dataset.is_none()
                    && backup == intent.old_dataset
                    && staging == Some(intent.new_dataset))
            {
                ops.rename(&paths.promotion, &paths.rollback)?;
                ops.sync(&paths.generation)?;
                return rollback(&paths, &intent, ops, None, None);
            }
            Err(corrupt("promotion directory state is ambiguous"))
        }
        (None, None) => {
            if directory_inode(&paths.backup)?.is_some()
                || directory_inode(&paths.aborted)?.is_some()
            {
                return Err(corrupt("legacy backup or aborted directory without intent"));
            }
            let dataset = directory_inode(&paths.dataset)?;
            let committed = contract(&paths.contract, MAX_GENERATION_CONTRACT_BYTES)?;
            if dataset.is_some() != committed.is_some() {
                return Err(corrupt(
                    "dataset/contract pair without intent is incomplete",
                ));
            }
            // Authenticate both stale entries before deleting either one.
            let staging = directory_inode(&paths.staging)?;
            let next = contract(&paths.next, MAX_GENERATION_CONTRACT_BYTES)?;
            if let Some(staging) = staging {
                remove_directory(ops, &paths.staging, staging, &paths.generation)?;
            }
            if let Some(next) = next {
                remove_regular(ops, &paths.next, next, &paths.generation)?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(corrupt(&format!(
                    "fixture assertion failed: {}",
                    stringify!($condition)
                )));
            }
        }};
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) if left != right => {
                    return Err(corrupt(&format!(
                        "fixture equality failed: {} = {left:?}, {} = {right:?}",
                        stringify!($left),
                        stringify!($right)
                    )));
                }
                _ => {}
            }
        }};
    }

    macro_rules! check_ne {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) if left == right => {
                    return Err(corrupt(&format!(
                        "fixture inequality failed: {} = {left:?}, {} = {right:?}",
                        stringify!($left),
                        stringify!($right)
                    )));
                }
                _ => {}
            }
        }};
    }

    fn io(operation: &str, path: &Path, error: &std::io::Error) -> CoreError {
        fs_err(operation, path, error)
    }

    #[derive(Clone, Copy)]
    struct ContractInodes {
        old: Inode,
        new: Inode,
    }

    fn contract_inode(path: &Path) -> Result<Inode, CoreError> {
        let metadata = fs::symlink_metadata(path).map_err(|error| {
            io(
                "stat fixture contract without following links",
                path,
                &error,
            )
        })?;
        if !metadata.file_type().is_file() {
            return Err(corrupt("fixture contract is not a regular file"));
        }
        Ok(Inode {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    fn fixture() -> Result<(TempDir, Paths, AtomicOutput, Inode, Inode, ContractInodes), CoreError>
    {
        let temporary = tempfile::tempdir()
            .map_err(|error| io("create promotion fixture", Path::new("."), &error))?;
        let generation = temporary.path().join("generation");
        fs::create_dir(&generation)
            .map_err(|error| io("create generation", &generation, &error))?;
        let paths = Paths::new(&generation);
        fs::create_dir(&paths.dataset)
            .map_err(|error| io("create old dataset", &paths.dataset, &error))?;
        fs::write(paths.dataset.join("old"), b"old payload")
            .map_err(|error| io("write old payload", &paths.dataset, &error))?;
        fs::write(&paths.contract, b"identical contract body")
            .map_err(|error| io("write old contract", &paths.contract, &error))?;
        fs::create_dir(&paths.staging)
            .map_err(|error| io("create staged dataset", &paths.staging, &error))?;
        fs::write(paths.staging.join("new"), b"new payload")
            .map_err(|error| io("write new payload", &paths.staging, &error))?;
        // Identical bytes deliberately require inode identity, not digest alone.
        let staged = write_atomic(
            &paths.next,
            b"identical contract body",
            "fixture staged contract",
            None,
        )?;
        let old = directory_inode(&paths.dataset)?.ok_or_else(|| corrupt("missing old fixture"))?;
        let new = directory_inode(&paths.staging)?.ok_or_else(|| corrupt("missing new fixture"))?;
        let contract_inodes = ContractInodes {
            old: contract_inode(&paths.contract)?,
            new: contract_inode(&paths.next)?,
        };
        if contract_inodes.old == contract_inodes.new {
            return Err(corrupt("fixture old and staged contract inodes overlap"));
        }
        Ok((temporary, paths, staged, old, new, contract_inodes))
    }

    fn first_generation_fixture() -> Result<(TempDir, Paths, AtomicOutput, Inode, Inode), CoreError>
    {
        let temporary = tempfile::tempdir()
            .map_err(|error| io("create first generation fixture", Path::new("."), &error))?;
        let generation = temporary.path().join("generation");
        fs::create_dir(&generation)
            .map_err(|error| io("create first generation", &generation, &error))?;
        let paths = Paths::new(&generation);
        fs::create_dir(&paths.staging)
            .map_err(|error| io("create first staging dataset", &paths.staging, &error))?;
        fs::write(paths.staging.join("new"), b"first generation payload")
            .map_err(|error| io("write first generation payload", &paths.staging, &error))?;
        let staged = write_atomic(
            &paths.next,
            b"first generation contract",
            "fixture first generation contract",
            None,
        )?;
        let new = directory_inode(&paths.staging)?
            .ok_or_else(|| corrupt("first staging dataset is absent"))?;
        let new_contract = contract_inode(&paths.next)?;
        Ok((temporary, paths, staged, new, new_contract))
    }

    fn assert_first_generation_pair(
        paths: &Paths,
        new: Inode,
        new_contract: Inode,
    ) -> Result<(), CoreError> {
        require(
            &directory_inode(&paths.dataset)?,
            &Some(new),
            "first generation dataset",
        )?;
        require(
            &fs::read(paths.dataset.join("new"))
                .map_err(|error| io("read first generation payload", &paths.dataset, &error))?,
            &b"first generation payload".to_vec(),
            "first generation payload",
        )?;
        require(
            &fs::read(&paths.contract)
                .map_err(|error| io("read first generation contract", &paths.contract, &error))?,
            &b"first generation contract".to_vec(),
            "first generation contract",
        )?;
        require(
            &contract_inode(&paths.contract)?,
            &new_contract,
            "first generation contract inode",
        )
    }

    fn assert_first_generation_absent(paths: &Paths) -> Result<(), CoreError> {
        require(
            &directory_inode(&paths.dataset)?,
            &None,
            "absent first dataset",
        )?;
        require(
            &contract(&paths.contract, MAX_GENERATION_CONTRACT_BYTES)?,
            &None,
            "absent first contract",
        )?;
        require(&directory_inode(&paths.staging)?, &None, "absent staging")?;
        require(&directory_inode(&paths.backup)?, &None, "absent backup")?;
        require(&directory_inode(&paths.aborted)?, &None, "absent aborted")?;
        require(
            &contract(&paths.next, MAX_GENERATION_CONTRACT_BYTES)?,
            &None,
            "absent staged contract",
        )?;
        require(
            &read_intent(&paths.promotion)?,
            &None,
            "absent promotion intent",
        )?;
        require(
            &read_intent(&paths.rollback)?,
            &None,
            "absent rollback intent",
        )
    }

    fn assert_old_pair(paths: &Paths, old: Inode, old_contract: Inode) -> Result<(), CoreError> {
        require(
            &directory_inode(&paths.dataset)?,
            &Some(old),
            "fixture old dataset",
        )?;
        let bytes = fs::read(paths.dataset.join("old"))
            .map_err(|error| io("read old payload", &paths.dataset, &error))?;
        require(&bytes, &b"old payload".to_vec(), "fixture old payload")?;
        require(
            &fs::read(&paths.contract)
                .map_err(|error| io("read old contract", &paths.contract, &error))?,
            &b"identical contract body".to_vec(),
            "fixture old contract",
        )?;
        require(
            &contract_inode(&paths.contract)?,
            &old_contract,
            "fixture old contract inode",
        )
    }

    fn assert_new_pair(paths: &Paths, new: Inode, new_contract: Inode) -> Result<(), CoreError> {
        require(
            &directory_inode(&paths.dataset)?,
            &Some(new),
            "fixture new dataset",
        )?;
        let bytes = fs::read(paths.dataset.join("new"))
            .map_err(|error| io("read new payload", &paths.dataset, &error))?;
        require(&bytes, &b"new payload".to_vec(), "fixture new payload")?;
        require(
            &fs::read(&paths.contract)
                .map_err(|error| io("read new contract", &paths.contract, &error))?,
            &b"identical contract body".to_vec(),
            "fixture new contract",
        )?;
        require(
            &contract_inode(&paths.contract)?,
            &new_contract,
            "fixture new contract inode",
        )
    }

    #[test]
    fn commit_and_restart_use_new_directory_and_new_contract_inode() -> Result<(), CoreError> {
        let (_temporary, paths, mut staged, old, new, contract_inodes) = fixture()?;
        let old_contract = contract(&paths.contract, MAX_GENERATION_CONTRACT_BYTES)?;
        let transaction = DatasetPromotion::begin(&paths.generation, &staged, None)?;
        transaction.promote_dataset()?;
        staged.rename_to(&paths.contract, "promote fixture contract")?;
        transaction.finish()?;
        DatasetPromotion::recover(&paths.generation)?;
        assert_new_pair(&paths, new, contract_inodes.new)?;
        check_ne!(old, new);
        check_ne!(
            old_contract,
            contract(&paths.contract, MAX_GENERATION_CONTRACT_BYTES)?
        );
        check!(directory_inode(&paths.backup)?.is_none());
        check!(read_intent(&paths.promotion)?.is_none());
        Ok(())
    }

    #[test]
    fn first_generation_pre_swap_crash_restores_absence() -> Result<(), CoreError> {
        let (_temporary, paths, staged, _, _) = first_generation_fixture()?;
        let transaction = DatasetPromotion::begin(&paths.generation, &staged, None)?;
        drop(transaction);
        drop(staged);
        DatasetPromotion::recover(&paths.generation)?;
        assert_first_generation_absent(&paths)
    }

    #[test]
    fn first_generation_post_swap_crash_finishes_new_pair() -> Result<(), CoreError> {
        let (_temporary, paths, staged, new, new_contract) = first_generation_fixture()?;
        let transaction = DatasetPromotion::begin(&paths.generation, &staged, None)?;
        transaction.promote_dataset()?;
        drop(transaction);
        drop(staged);
        DatasetPromotion::recover(&paths.generation)?;
        assert_first_generation_pair(&paths, new, new_contract)?;
        require(
            &directory_inode(&paths.backup)?,
            &None,
            "absent first backup",
        )?;
        require(
            &read_intent(&paths.promotion)?,
            &None,
            "settled first intent",
        )
    }

    #[test]
    fn first_generation_contract_pre_rename_refusal_rolls_back_to_absence() -> Result<(), CoreError>
    {
        let (_temporary, paths, staged, _, _) = first_generation_fixture()?;
        let transaction = DatasetPromotion::begin(&paths.generation, &staged, None)?;
        transaction.promote_dataset()?;
        require(
            &contract(&paths.contract, MAX_GENERATION_CONTRACT_BYTES)?,
            &None,
            "contract before rejected rename",
        )?;
        transaction.rollback(staged)?;
        assert_first_generation_absent(&paths)
    }

    #[test]
    fn restart_before_dataset_swap_restores_old_pair() -> Result<(), CoreError> {
        let (_temporary, paths, staged, old, _, contract_inodes) = fixture()?;
        let transaction = DatasetPromotion::begin(&paths.generation, &staged, None)?;
        drop(transaction);
        drop(staged);
        DatasetPromotion::recover(&paths.generation)?;
        assert_old_pair(&paths, old, contract_inodes.old)?;
        check!(directory_inode(&paths.staging)?.is_none());
        check!(contract(&paths.next, MAX_GENERATION_CONTRACT_BYTES)?.is_none());
        check!(read_intent(&paths.rollback)?.is_none());
        Ok(())
    }

    #[test]
    fn restart_after_dataset_swap_finishes_forward() -> Result<(), CoreError> {
        let (_temporary, paths, staged, _, new, contract_inodes) = fixture()?;
        let transaction = DatasetPromotion::begin(&paths.generation, &staged, None)?;
        transaction.promote_dataset()?;
        drop(transaction);
        drop(staged);
        DatasetPromotion::recover(&paths.generation)?;
        assert_new_pair(&paths, new, contract_inodes.new)?;
        check!(directory_inode(&paths.backup)?.is_none());
        check!(read_intent(&paths.promotion)?.is_none());
        Ok(())
    }

    #[test]
    fn failed_dataset_swap_and_predelete_rollback_recover() -> Result<(), CoreError> {
        let (_temporary, paths, staged, old, _, contract_inodes) = fixture()?;
        let mut transaction = DatasetPromotion::begin(&paths.generation, &staged, None)?;
        transaction.ops = FsOps::with_fault("rename", 2);
        check!(transaction.promote_dataset().is_err());
        transaction.ops = FsOps::with_fault("remove_dir", 1);
        check!(transaction.rollback(staged).is_err());
        assert_old_pair(&paths, old, contract_inodes.old)?;
        check!(read_intent(&paths.rollback)?.is_some());
        check!(directory_inode(&paths.aborted)?.is_some());
        DatasetPromotion::recover(&paths.generation)?;
        assert_old_pair(&paths, old, contract_inodes.old)?;
        check!(directory_inode(&paths.aborted)?.is_none());
        check!(read_intent(&paths.rollback)?.is_none());
        Ok(())
    }

    #[test]
    fn rollback_restarts_after_aborted_dataset_was_partly_deleted() -> Result<(), CoreError> {
        let (_temporary, paths, staged, old, new, contract_inodes) = fixture()?;
        let second = paths.staging.join("new-second");
        fs::write(&second, b"second new payload")
            .map_err(|error| io("write second staged payload", &second, &error))?;
        let mut transaction = DatasetPromotion::begin(&paths.generation, &staged, None)?;
        transaction.promote_dataset()?;
        transaction
            .marker
            .rename_to(&paths.rollback, "fixture rollback intent")?;
        fs::rename(&paths.dataset, &paths.aborted)
            .map_err(|error| io("move new dataset to aborted", &paths.aborted, &error))?;
        File::open(&paths.generation)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| io("sync aborted move", &paths.generation, &error))?;
        fs::rename(&paths.backup, &paths.dataset)
            .map_err(|error| io("restore old fixture dataset", &paths.dataset, &error))?;
        File::open(&paths.generation)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| io("sync old restoration", &paths.generation, &error))?;
        drop(transaction);
        drop(staged);
        let deleted = paths.aborted.join("new");
        fs::remove_file(&deleted)
            .map_err(|error| io("partly delete aborted dataset", &deleted, &error))?;
        File::open(&paths.aborted)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| io("sync partial aborted cleanup", &paths.aborted, &error))?;
        check!(!deleted.exists());
        check_eq!(directory_inode(&paths.aborted)?, Some(new));
        check!(paths.aborted.join("new-second").exists());
        assert_old_pair(&paths, old, contract_inodes.old)?;
        check!(read_intent(&paths.rollback)?.is_some());
        DatasetPromotion::recover(&paths.generation)?;
        assert_old_pair(&paths, old, contract_inodes.old)?;
        check!(directory_inode(&paths.aborted)?.is_none());
        check!(contract(&paths.next, MAX_GENERATION_CONTRACT_BYTES)?.is_none());
        check!(read_intent(&paths.rollback)?.is_none());
        Ok(())
    }

    #[test]
    fn forward_restarts_after_backup_dataset_was_partly_deleted() -> Result<(), CoreError> {
        let (_temporary, paths, mut staged, old, new, contract_inodes) = fixture()?;
        let second = paths.dataset.join("old-second");
        fs::write(&second, b"second old payload")
            .map_err(|error| io("write second old payload", &second, &error))?;
        let transaction = DatasetPromotion::begin(&paths.generation, &staged, None)?;
        transaction.promote_dataset()?;
        staged.rename_to(&paths.contract, "promote fixture contract")?;
        drop(transaction);
        let deleted = paths.backup.join("old-second");
        fs::remove_file(&deleted)
            .map_err(|error| io("partly delete backup dataset", &deleted, &error))?;
        File::open(&paths.backup)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| io("sync partial backup cleanup", &paths.backup, &error))?;
        check!(!deleted.exists());
        check_eq!(directory_inode(&paths.backup)?, Some(old));
        check!(paths.backup.join("old").exists());
        assert_new_pair(&paths, new, contract_inodes.new)?;
        check!(read_intent(&paths.promotion)?.is_some());
        DatasetPromotion::recover(&paths.generation)?;
        assert_new_pair(&paths, new, contract_inodes.new)?;
        check!(directory_inode(&paths.backup)?.is_none());
        check!(read_intent(&paths.promotion)?.is_none());
        Ok(())
    }

    #[test]
    fn failed_parent_sync_after_backup_move_keeps_recoverable_intent() -> Result<(), CoreError> {
        let (_temporary, paths, staged, old, _, contract_inodes) = fixture()?;
        let mut transaction = DatasetPromotion::begin(&paths.generation, &staged, None)?;
        transaction.ops = FsOps::with_fault("sync", 1);
        check!(transaction.promote_dataset().is_err());
        check!(read_intent(&paths.promotion)?.is_some());
        check!(directory_inode(&paths.dataset)?.is_none());
        check_eq!(directory_inode(&paths.backup)?, Some(old));
        drop(transaction);
        drop(staged);
        DatasetPromotion::recover(&paths.generation)?;
        assert_old_pair(&paths, old, contract_inodes.old)?;
        check!(directory_inode(&paths.backup)?.is_none());
        Ok(())
    }

    #[test]
    fn rollback_resumes_after_new_dataset_was_moved_to_aborted() -> Result<(), CoreError> {
        let (_temporary, paths, staged, old, _, contract_inodes) = fixture()?;
        let mut transaction = DatasetPromotion::begin(&paths.generation, &staged, None)?;
        transaction.promote_dataset()?;
        // Rollback moves D_new to Q_new, then its parent sync fails. B_old and
        // C_old are still present, and recovery must restore B_old to D_old.
        transaction.ops = FsOps::with_fault("sync", 1);
        check!(transaction.rollback(staged).is_err());
        check!(directory_inode(&paths.dataset)?.is_none());
        check_eq!(directory_inode(&paths.backup)?, Some(old));
        check!(directory_inode(&paths.aborted)?.is_some());
        check!(read_intent(&paths.rollback)?.is_some());
        DatasetPromotion::recover(&paths.generation)?;
        assert_old_pair(&paths, old, contract_inodes.old)?;
        check!(directory_inode(&paths.backup)?.is_none());
        check!(directory_inode(&paths.aborted)?.is_none());
        Ok(())
    }

    #[test]
    fn restored_old_pair_is_synced_before_staged_cleanup() -> Result<(), CoreError> {
        let (_temporary, paths, staged, old, _, contract_inodes) = fixture()?;
        let mut transaction = DatasetPromotion::begin(&paths.generation, &staged, None)?;
        transaction.promote_dataset()?;
        transaction.ops = FsOps::with_fault("sync", 2);
        check!(transaction.rollback(staged).is_err());
        assert_old_pair(&paths, old, contract_inodes.old)?;
        check!(read_intent(&paths.rollback)?.is_some());
        check!(contract(&paths.next, MAX_GENERATION_CONTRACT_BYTES)?.is_some());
        // The first sync on this resumed path is the mandatory old-pair
        // custody fence; its failure must leave the staged contract in place.
        check!(recover(&paths.generation, &FsOps::with_fault("sync", 1)).is_err());
        assert_old_pair(&paths, old, contract_inodes.old)?;
        check!(contract(&paths.next, MAX_GENERATION_CONTRACT_BYTES)?.is_some());
        check!(read_intent(&paths.rollback)?.is_some());
        DatasetPromotion::recover(&paths.generation)?;
        assert_old_pair(&paths, old, contract_inodes.old)?;
        check!(contract(&paths.next, MAX_GENERATION_CONTRACT_BYTES)?.is_none());
        Ok(())
    }

    #[test]
    fn new_pair_sync_precedes_old_backup_cleanup_after_contract_rename() -> Result<(), CoreError> {
        let (_temporary, paths, staged, _, new, contract_inodes) = fixture()?;
        let transaction = DatasetPromotion::begin(&paths.generation, &staged, None)?;
        transaction.promote_dataset()?;
        drop(transaction);
        drop(staged);
        // Represent a crash after the contract rename but before its parent
        // directory sync. Recovery must sync the authenticated new pair first.
        fs::rename(&paths.next, &paths.contract)
            .map_err(|error| io("simulate contract rename", &paths.contract, &error))?;
        check!(recover(&paths.generation, &FsOps::with_fault("sync", 1)).is_err());
        assert_new_pair(&paths, new, contract_inodes.new)?;
        check!(directory_inode(&paths.backup)?.is_some());
        check!(read_intent(&paths.promotion)?.is_some());
        DatasetPromotion::recover(&paths.generation)?;
        assert_new_pair(&paths, new, contract_inodes.new)?;
        check!(directory_inode(&paths.backup)?.is_none());
        Ok(())
    }

    #[test]
    fn failed_recovery_rename_retries_without_discarding_old_pair() -> Result<(), CoreError> {
        let (_temporary, paths, staged, old, _, contract_inodes) = fixture()?;
        let transaction = DatasetPromotion::begin(&paths.generation, &staged, None)?;
        drop(transaction);
        drop(staged);
        check!(recover(&paths.generation, &FsOps::with_fault("rename", 1)).is_err());
        assert_old_pair(&paths, old, contract_inodes.old)?;
        check!(read_intent(&paths.promotion)?.is_some());
        DatasetPromotion::recover(&paths.generation)?;
        assert_old_pair(&paths, old, contract_inodes.old)?;
        check!(read_intent(&paths.rollback)?.is_none());
        Ok(())
    }

    #[test]
    fn symlinked_stale_next_refuses_without_removing_staging() -> Result<(), CoreError> {
        use std::os::unix::fs::symlink;

        let (_temporary, paths, staged, old, _, contract_inodes) = fixture()?;
        drop(staged);
        fs::remove_file(&paths.next)
            .map_err(|error| io("remove fixture next", &paths.next, &error))?;
        symlink(&paths.contract, &paths.next)
            .map_err(|error| io("symlink fixture next", &paths.next, &error))?;
        check!(DatasetPromotion::recover(&paths.generation).is_err());
        assert_old_pair(&paths, old, contract_inodes.old)?;
        check!(directory_inode(&paths.staging)?.is_some());
        Ok(())
    }

    #[test]
    fn staging_fence_failure_keeps_old_pair_and_creates_no_intent() -> Result<(), CoreError> {
        let (_temporary, paths, staged, old, _, contract_inodes) = fixture()?;
        check!(
            DatasetPromotion::begin_with_ops(
                &paths.generation,
                &staged,
                None,
                FsOps::with_fault("fence_sync", 1),
            )
            .is_err()
        );
        assert_old_pair(&paths, old, contract_inodes.old)?;
        check!(directory_inode(&paths.staging)?.is_some());
        check!(read_intent(&paths.promotion)?.is_none());
        Ok(())
    }

    #[test]
    fn absent_generation_recovery_does_not_create_root() -> Result<(), CoreError> {
        let temporary = tempfile::tempdir()
            .map_err(|error| io("create absent generation fixture", Path::new("."), &error))?;
        let absent = temporary.path().join("absent-generation");
        DatasetPromotion::recover(&absent)?;
        check!(!absent.exists());
        Ok(())
    }

    #[test]
    fn forged_intent_and_ambiguous_legacy_backup_do_not_delete_data() -> Result<(), CoreError> {
        let (_temporary, paths, staged, old, _, contract_inodes) = fixture()?;
        let transaction = DatasetPromotion::begin(&paths.generation, &staged, None)?;
        drop(transaction);
        let mut forged =
            read_intent(&paths.promotion)?.ok_or_else(|| corrupt("fixture intent absent"))?;
        forged.old_dataset = Some(Inode {
            device: old.device,
            inode: old
                .inode
                .checked_add(1)
                .ok_or_else(|| corrupt("inode overflow"))?,
        });
        fs::write(&paths.promotion, intent_bytes(&forged)?)
            .map_err(|error| io("forge fixture intent", &paths.promotion, &error))?;
        check!(DatasetPromotion::recover(&paths.generation).is_err());
        assert_old_pair(&paths, old, contract_inodes.old)?;
        check!(directory_inode(&paths.staging)?.is_some());
        fs::remove_file(&paths.promotion)
            .map_err(|error| io("remove forged intent", &paths.promotion, &error))?;
        fs::rename(&paths.staging, &paths.backup)
            .map_err(|error| io("forge legacy backup", &paths.backup, &error))?;
        check!(DatasetPromotion::recover(&paths.generation).is_err());
        assert_old_pair(&paths, old, contract_inodes.old)?;
        check!(directory_inode(&paths.backup)?.is_some());
        Ok(())
    }
}
