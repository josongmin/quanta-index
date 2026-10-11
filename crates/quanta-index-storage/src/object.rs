use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use quanta_index_core::index::{
    Digest, IndexError, ObjectBatchInput, ObjectRef, RegisteredObject, StoredObject, WorkBudget,
    WorkUsage,
    ports::{ImmutableObjectPort, ObjectWriteSink},
};
use sha2::{Digest as _, Sha256};

const MAGIC: &[u8; 8] = b"QIOBJ001";
const HASH_DOMAIN: &[u8] = b"quanta-index-immutable-object-v1\0";
const HEADER_LEN: usize = 16;
const MAX_PAYLOAD: usize = 16 * 1024 * 1024;
const MAX_CHILDREN: usize = 4096;
const MAX_WIRE: usize = HEADER_LEN + MAX_CHILDREN * 32 + MAX_PAYLOAD;
pub(crate) const MAX_BATCH_INPUTS: usize = 64;
pub(crate) const MAX_BATCH_BYTES: u64 = 64 * 1024 * 1024;

pub(crate) struct PreparedObject {
    pub(crate) registration: RegisteredObject,
    pub(crate) wire: Vec<u8>,
}

/// A content-addressed object store. Every installed object is immutable and
/// synced before its reference is returned to a caller.
#[derive(Debug)]
pub struct FileObjectStore {
    objects: PathBuf,
    temp_sequence: AtomicU64,
}

impl FileObjectStore {
    /// Opens or creates a store. The caller must keep its directory private to
    /// this store and let the catalog govern object reclamation.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, IndexError> {
        let root = path.as_ref();
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(root)
            .map_err(|error| storage_error(&error))?;
        validate_private_directory(root)?;
        if let Some(parent) = root.parent() {
            let parent = if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            };
            sync_parent_directory(parent)?;
        }
        let objects = root.join("objects");
        match fs::DirBuilder::new().mode(0o700).create(&objects) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(storage_error(&error)),
        }
        validate_private_directory(&objects)?;
        sync_directory(root)?;
        Ok(Self {
            objects,
            temp_sequence: AtomicU64::new(0),
        })
    }

    pub(crate) fn path_for(&self, reference: ObjectRef) -> PathBuf {
        let bytes = (reference.0).0;
        let mut name = String::with_capacity(64);
        for byte in bytes {
            name.push(hex_digit(byte >> 4));
            name.push(hex_digit(byte & 0x0f));
        }
        let shard = format!("{:02x}", bytes.first().copied().unwrap_or_default());
        self.objects.join(shard).join(name)
    }

    pub(crate) fn scratch_root(&self) -> Result<PathBuf, IndexError> {
        self.validate_store_root()?;
        let root = self
            .objects
            .parent()
            .ok_or_else(|| IndexError::Storage("object store has no root".into()))?;
        let scratch = root.join("scratch");
        ensure_private_directory(&scratch)?;
        Ok(scratch)
    }

    pub(crate) fn prepare_object(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<PreparedObject, IndexError> {
        if payload.len() > MAX_PAYLOAD || children.len() > MAX_CHILDREN {
            return Err(IndexError::InvalidInput(
                "object exceeds physical size limit".into(),
            ));
        }
        let wire_len = HEADER_LEN
            .checked_add(
                children
                    .len()
                    .checked_mul(32)
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .and_then(|len| len.checked_add(payload.len()))
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: u64::try_from(wire_len)
                .map_err(|_conversion_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let mut wire = Vec::with_capacity(wire_len);
        wire.extend_from_slice(MAGIC);
        wire.extend_from_slice(
            &u32::try_from(payload.len())
                .map_err(|_conversion_error| IndexError::ResourceLimited)?
                .to_be_bytes(),
        );
        wire.extend_from_slice(
            &u32::try_from(children.len())
                .map_err(|_conversion_error| IndexError::ResourceLimited)?
                .to_be_bytes(),
        );
        for child in children {
            wire.extend_from_slice(&(child.0).0);
        }
        wire.extend_from_slice(payload);
        let registration = RegisteredObject {
            reference: digest_wire(&wire),
            bytes: u64::try_from(wire_len)
                .map_err(|_conversion_error| IndexError::ResourceLimited)?,
            children: children.to_vec(),
        };
        Ok(PreparedObject { registration, wire })
    }

    pub(crate) fn install_prepared_batch(
        &self,
        objects: &[&PreparedObject],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.install_prepared_batch_with_barrier(objects, sink, budget, |file| {
            #[cfg(target_os = "macos")]
            {
                rustix::fs::fcntl_fullfsync(file).map_err(|error| {
                    IndexError::Storage(format!("full sync immutable object batch: {error}"))
                })
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _unused_file = file;
                Ok(())
            }
        })
    }

    fn install_prepared_batch_with_barrier(
        &self,
        objects: &[&PreparedObject],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
        barrier: impl FnOnce(&File) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        if objects.is_empty() {
            return Ok(());
        }
        if objects.len() > MAX_BATCH_INPUTS {
            return Err(IndexError::ResourceLimited);
        }
        let mut total_bytes = 0u64;
        for object in objects {
            total_bytes = total_bytes
                .checked_add(object.registration.bytes)
                .ok_or(IndexError::ResourceLimited)?;
        }
        if total_bytes > MAX_BATCH_BYTES {
            return Err(IndexError::ResourceLimited);
        }
        let count = u64::try_from(objects.len())
            .map_err(|_conversion_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            writes: count,
            bytes: total_bytes
                .checked_add(count.checked_mul(256).ok_or(IndexError::ResourceLimited)?)
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let mut all_refs = BTreeSet::new();
        for object in objects {
            if !all_refs.insert(object.registration.reference) {
                return Err(IndexError::BackendViolation(
                    "duplicate object in staged batch".into(),
                ));
            }
        }
        let mut seen = BTreeSet::new();
        for object in objects {
            for child in &object.registration.children {
                if all_refs.contains(child) && !seen.contains(child) {
                    return Err(IndexError::BackendViolation(
                        "staged batch is not child-first".into(),
                    ));
                }
                if !all_refs.contains(child) {
                    budget.charge(WorkUsage {
                        reads: 1,
                        ..WorkUsage::default()
                    })?;
                    self.validate_child_path(*child)?;
                }
            }
            let _was_new = seen.insert(object.registration.reference);
        }
        let registrations: Vec<RegisteredObject> = objects
            .iter()
            .map(|object| object.registration.clone())
            .collect();
        sink.reserve_batch(&registrations)?;

        #[cfg(target_os = "macos")]
        let batch_device = {
            self.validate_store_root()?;
            fs::symlink_metadata(&self.objects)
                .map_err(|error| storage_error(&error))?
                .dev()
        };
        #[cfg(target_os = "macos")]
        let mut barrier_file: Option<File> = None;
        let mut referenced_shards = BTreeSet::new();
        for object in objects {
            budget.checkpoint()?;
            self.validate_store_root()?;
            let final_path = self.path_for(object.registration.reference);
            let shard = final_path
                .parent()
                .ok_or_else(|| IndexError::Storage("object path has no parent".into()))?;
            ensure_private_directory(shard)?;
            #[cfg(target_os = "macos")]
            ensure_same_device(
                batch_device,
                fs::symlink_metadata(shard)
                    .map_err(|error| storage_error(&error))?
                    .dev(),
            )?;
            if final_path.exists() {
                self.verify_existing(object.registration.reference, &object.wire, budget)?;
                #[cfg(target_os = "macos")]
                {
                    let existing = open_private_regular_file(&final_path)?;
                    ensure_same_device(
                        batch_device,
                        existing
                            .metadata()
                            .map_err(|error| storage_error(&error))?
                            .dev(),
                    )?;
                    rustix::fs::fsync(&existing).map_err(|error| {
                        IndexError::Storage(format!("sync existing immutable object: {error}"))
                    })?;
                    if barrier_file.is_none() {
                        barrier_file = Some(existing);
                    }
                }
                let _was_new = referenced_shards.insert(shard.to_path_buf());
                continue;
            }
            let (mut file, temp_path) = self.create_temp(shard)?;
            let mut temp_guard = TempFile(temp_path);
            #[cfg(target_os = "macos")]
            ensure_same_device(
                batch_device,
                file.metadata()
                    .map_err(|error| storage_error(&error))?
                    .dev(),
            )?;
            file.write_all(&object.wire)
                .map_err(|error| storage_error(&error))?;
            #[cfg(target_os = "macos")]
            rustix::fs::fsync(&file)
                .map_err(|error| IndexError::Storage(format!("sync immutable object: {error}")))?;
            #[cfg(not(target_os = "macos"))]
            file.sync_all().map_err(|error| storage_error(&error))?;
            let written = file.metadata().map_err(|error| storage_error(&error))?;
            validate_private_file(&written)?;
            match publish_temp_no_replace(&mut temp_guard, &final_path, &written) {
                Ok(true) => {
                    validate_private_file(
                        &file.metadata().map_err(|error| storage_error(&error))?,
                    )?;
                    #[cfg(target_os = "macos")]
                    {
                        if barrier_file.is_none() {
                            barrier_file = Some(file);
                        }
                    }
                    #[cfg(not(target_os = "macos"))]
                    drop(file);
                    let _was_new = referenced_shards.insert(shard.to_path_buf());
                }
                Ok(false) => {
                    drop(file);
                    self.verify_existing(object.registration.reference, &object.wire, budget)?;
                    #[cfg(target_os = "macos")]
                    {
                        let existing = open_private_regular_file(&final_path)?;
                        ensure_same_device(
                            batch_device,
                            existing
                                .metadata()
                                .map_err(|error| storage_error(&error))?
                                .dev(),
                        )?;
                        rustix::fs::fsync(&existing).map_err(|error| {
                            IndexError::Storage(format!(
                                "sync concurrent immutable object: {error}"
                            ))
                        })?;
                        if barrier_file.is_none() {
                            barrier_file = Some(existing);
                        }
                    }
                    let _was_new = referenced_shards.insert(shard.to_path_buf());
                }
                Err(error) => return Err(error),
            }
        }
        // A concurrently installed matching object may not have had its directory
        // entry synced yet. Every referenced shard must be durable before custody
        // confirmation, including already-existing objects.
        #[cfg(target_os = "macos")]
        {
            sync_directory_plain(&self.objects, batch_device)?;
            for shard in referenced_shards {
                sync_directory_plain(&shard, batch_device)?;
            }
            budget.checkpoint()?;
            let file = barrier_file.ok_or_else(|| {
                IndexError::BackendViolation("nonempty object batch has no barrier file".into())
            })?;
            barrier(&file)?;
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _unused_barrier = barrier;
            sync_directory(&self.objects)?;
            for shard in referenced_shards {
                sync_directory(&shard)?;
            }
        }
        budget.checkpoint()?;
        sink.installed_batch(&registrations)
    }

    pub(crate) fn put_inner(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: Option<&dyn ObjectWriteSink>,
        budget: &mut WorkBudget,
    ) -> Result<(RegisteredObject, bool), IndexError> {
        let PreparedObject { registration, wire } =
            self.prepare_object(payload, children, budget)?;
        budget.charge(WorkUsage {
            writes: 1,
            bytes: registration.bytes,
            ..WorkUsage::default()
        })?;
        for child in children {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            self.validate_child_path(*child)?;
        }

        let reference = registration.reference;
        if let Some(sink) = sink {
            sink.reserve(&registration)?;
        }
        self.validate_store_root()?;
        let final_path = self.path_for(reference);
        let shard = final_path
            .parent()
            .ok_or_else(|| IndexError::Storage("object path has no parent".into()))?;
        ensure_private_directory(shard)?;
        if final_path.exists() {
            self.verify_existing(reference, &wire, budget)?;
            if let Some(sink) = sink {
                sync_directory(&self.objects)?;
                sync_directory(shard)?;
                sink.installed(&registration)?;
            }
            return Ok((registration, false));
        }
        sync_directory(&self.objects)?;
        let (mut file, temp_path) = self.create_temp(shard)?;
        let mut temp_guard = TempFile(temp_path);
        file.write_all(&wire)
            .map_err(|error| storage_error(&error))?;
        file.sync_all().map_err(|error| storage_error(&error))?;
        let written = file.metadata().map_err(|error| storage_error(&error))?;
        validate_private_file(&written)?;
        match publish_temp_no_replace(&mut temp_guard, &final_path, &written) {
            Ok(true) => {
                sync_directory(shard)?;
                validate_private_file(&file.metadata().map_err(|error| storage_error(&error))?)?;
                drop(file);
                if let Some(sink) = sink {
                    sink.installed(&registration)?;
                }
                Ok((registration, true))
            }
            Ok(false) => {
                drop(file);
                self.verify_existing(reference, &wire, budget)?;
                if let Some(sink) = sink {
                    sync_directory(&self.objects)?;
                    sync_directory(shard)?;
                    sink.installed(&registration)?;
                }
                Ok((registration, false))
            }
            Err(error) => Err(error),
        }
    }

    fn create_temp(&self, shard: &Path) -> Result<(File, PathBuf), IndexError> {
        validate_private_directory(shard)?;
        for _ in 0..16 {
            let sequence = self.temp_sequence.fetch_add(1, Ordering::Relaxed);
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| {
                    IndexError::Storage(format!("system clock before epoch: {error}"))
                })?
                .as_nanos();
            let path = shard.join(format!(".tmp-{}-{sequence}-{nonce}", std::process::id()));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(nofollow_flag()?)
                .open(&path)
            {
                Ok(file) => {
                    if let Err(error) = file
                        .metadata()
                        .map_err(|error| storage_error(&error))
                        .and_then(|metadata| validate_private_file(&metadata))
                    {
                        drop(file);
                        drop(fs::remove_file(&path));
                        return Err(error);
                    }
                    return Ok((file, path));
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(storage_error(&error)),
            }
        }
        Err(IndexError::Storage(
            "temporary object name exhausted".into(),
        ))
    }

    fn validate_child_path(&self, reference: ObjectRef) -> Result<(), IndexError> {
        self.validate_store_root()?;
        let path = self.path_for(reference);
        let shard = path
            .parent()
            .ok_or_else(|| IndexError::Storage("object path has no parent".into()))?;
        validate_private_shard(
            shard,
            IndexError::InvalidInput("object references a missing child".into()),
        )?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                IndexError::InvalidInput("object references a missing child".into())
            } else {
                storage_error(&error)
            }
        })?;
        validate_private_file(&metadata)
    }

    fn validate_store_root(&self) -> Result<(), IndexError> {
        let root = self
            .objects
            .parent()
            .ok_or_else(|| IndexError::Storage("object store has no root".into()))?;
        validate_private_directory(root)?;
        validate_private_directory(&self.objects)
    }

    fn verify_existing(
        &self,
        reference: ObjectRef,
        expected: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let stored = self.read_wire(reference, budget)?;
        if stored != expected {
            return Err(IndexError::CorruptObject(
                "content-addressed path has different bytes".into(),
            ));
        }
        Ok(())
    }

    fn read_wire(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Vec<u8>, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        self.validate_store_root()?;
        let path = self.path_for(reference);
        let shard = path
            .parent()
            .ok_or_else(|| IndexError::Storage("object path has no parent".into()))?;
        validate_private_shard(
            shard,
            IndexError::CorruptObject("referenced object is absent".into()),
        )?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                IndexError::CorruptObject("referenced object is absent".into())
            } else {
                storage_error(&error)
            }
        })?;
        validate_private_file(&metadata)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(nofollow_flag()?)
            .open(&path)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    IndexError::CorruptObject("referenced object is absent".into())
                } else {
                    storage_error(&error)
                }
            })?;
        let opened = file.metadata().map_err(|error| storage_error(&error))?;
        validate_private_file(&opened)?;
        if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
            return Err(IndexError::CorruptObject(
                "object changed during open".into(),
            ));
        }
        let file_len = usize::try_from(opened.len()).map_err(|_conversion_error| {
            IndexError::CorruptObject("object length overflows address space".into())
        })?;
        if !(HEADER_LEN..=MAX_WIRE).contains(&file_len) {
            return Err(IndexError::CorruptObject(
                "object length is out of bounds".into(),
            ));
        }
        budget.charge(WorkUsage {
            bytes: u64::try_from(file_len)
                .map_err(|_conversion_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let mut wire = Vec::with_capacity(file_len);
        let limit = u64::try_from(file_len)
            .map_err(|_conversion_error| IndexError::ResourceLimited)?
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let _read_bytes = file
            .take(limit)
            .read_to_end(&mut wire)
            .map_err(|error| storage_error(&error))?;
        if wire.len() != file_len || digest_wire(&wire) != reference {
            return Err(IndexError::CorruptObject(
                "object digest or size mismatch".into(),
            ));
        }
        Ok(wire)
    }
}

impl ImmutableObjectPort for FileObjectStore {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.prepare_object(payload, children, budget)
            .map(|prepared| prepared.registration)
    }

    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.put_inner(payload, children, None, budget)
            .map(|(registration, _)| registration)
    }

    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.put_inner(payload, children, Some(sink), budget)
            .map(|(registration, _)| registration)
    }

    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<Vec<RegisteredObject>, IndexError> {
        if inputs.len() > MAX_BATCH_INPUTS {
            return Err(IndexError::ResourceLimited);
        }
        let mut aggregate = 0u64;
        for input in inputs {
            if input.payload.len() > MAX_PAYLOAD || input.children.len() > MAX_CHILDREN {
                return Err(IndexError::ResourceLimited);
            }
            let wire_len = HEADER_LEN
                .checked_add(
                    input
                        .children
                        .len()
                        .checked_mul(32)
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .and_then(|size| size.checked_add(input.payload.len()))
                .ok_or(IndexError::ResourceLimited)?;
            aggregate = aggregate
                .checked_add(
                    u64::try_from(wire_len)
                        .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                )
                .ok_or(IndexError::ResourceLimited)?;
        }
        if aggregate > MAX_BATCH_BYTES {
            return Err(IndexError::ResourceLimited);
        }
        let count =
            u64::try_from(inputs.len()).map_err(|_conversion_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: aggregate
                .checked_add(count.checked_mul(256).ok_or(IndexError::ResourceLimited)?)
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let mut unique = Vec::<PreparedObject>::with_capacity(inputs.len());
        let mut seen = BTreeMap::<ObjectRef, usize>::new();
        let mut ordered = Vec::with_capacity(inputs.len());
        for input in inputs {
            budget.checkpoint()?;
            let prepared = self.prepare_object(&input.payload, &input.children, budget)?;
            let reference = prepared.registration.reference;
            if let Some(index) = seen.get(&reference) {
                let existing = unique
                    .get(*index)
                    .ok_or_else(|| IndexError::BackendViolation("batch index missing".into()))?;
                if existing.wire != prepared.wire {
                    return Err(IndexError::CorruptObject(
                        "same object digest has different batch wire".into(),
                    ));
                }
            } else {
                let index = unique.len();
                let _prior = seen.insert(reference, index);
                unique.push(prepared);
            }
            let registration = unique
                .get(*seen.get(&reference).ok_or_else(|| {
                    IndexError::BackendViolation("batch reference missing".into())
                })?)
                .ok_or_else(|| IndexError::BackendViolation("batch object missing".into()))?
                .registration
                .clone();
            ordered.push(registration);
        }
        let batch: Vec<_> = unique.iter().collect();
        self.install_prepared_batch(&batch, sink, budget)?;
        Ok(ordered)
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        let wire = self.read_wire(reference, budget)?;
        decode_stored_wire(reference, &wire, budget)
    }
}

pub(crate) fn decode_stored_wire(
    reference: ObjectRef,
    wire: &[u8],
    budget: &mut WorkBudget,
) -> Result<StoredObject, IndexError> {
    if !(HEADER_LEN..=MAX_WIRE).contains(&wire.len()) {
        return Err(IndexError::CorruptObject(
            "scratch object length is out of bounds".into(),
        ));
    }
    if digest_wire(wire) != reference {
        return Err(IndexError::CorruptObject(
            "scratch object digest mismatch".into(),
        ));
    }
    if wire.get(..8) != Some(MAGIC.as_slice()) {
        return Err(IndexError::CorruptObject("object magic mismatch".into()));
    }
    let payload_len = usize::try_from(read_u32(wire, 8)?)
        .map_err(|_conversion_error| IndexError::ResourceLimited)?;
    let children_len = usize::try_from(read_u32(wire, 12)?)
        .map_err(|_conversion_error| IndexError::ResourceLimited)?;
    if payload_len > MAX_PAYLOAD || children_len > MAX_CHILDREN {
        return Err(IndexError::CorruptObject(
            "encoded lengths exceed physical limits".into(),
        ));
    }
    let payload_at = HEADER_LEN
        .checked_add(
            children_len
                .checked_mul(32)
                .ok_or(IndexError::ResourceLimited)?,
        )
        .ok_or(IndexError::ResourceLimited)?;
    if payload_at.checked_add(payload_len) != Some(wire.len()) {
        return Err(IndexError::CorruptObject(
            "encoded lengths do not match object size".into(),
        ));
    }
    let decoded_bytes = children_len
        .checked_mul(std::mem::size_of::<ObjectRef>())
        .and_then(|children_bytes| children_bytes.checked_add(payload_len))
        .ok_or(IndexError::ResourceLimited)?;
    let decoded_bytes =
        u64::try_from(decoded_bytes).map_err(|_conversion_error| IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes: decoded_bytes,
        ..WorkUsage::default()
    })?;
    let mut children = Vec::with_capacity(children_len);
    for index in 0..children_len {
        let start = index
            .checked_mul(32)
            .and_then(|offset| HEADER_LEN.checked_add(offset))
            .ok_or(IndexError::ResourceLimited)?;
        let end = start.checked_add(32).ok_or(IndexError::ResourceLimited)?;
        let bytes = wire
            .get(start..end)
            .ok_or_else(|| IndexError::CorruptObject("truncated child reference".into()))?;
        let digest: [u8; 32] = bytes.try_into().map_err(|_conversion_error| {
            IndexError::CorruptObject("invalid child reference".into())
        })?;
        children.push(ObjectRef(Digest(digest)));
    }
    let payload = wire
        .get(payload_at..)
        .ok_or_else(|| IndexError::CorruptObject("truncated payload".into()))?
        .to_vec();
    Ok(StoredObject {
        registration: RegisteredObject {
            reference,
            bytes: u64::try_from(wire.len())
                .map_err(|_conversion_error| IndexError::ResourceLimited)?,
            children,
        },
        payload,
    })
}

fn read_u32(wire: &[u8], at: usize) -> Result<u32, IndexError> {
    let end = at.checked_add(4).ok_or_else(|| {
        IndexError::CorruptObject("object header offset overflows address space".into())
    })?;
    let bytes: [u8; 4] = wire
        .get(at..end)
        .ok_or_else(|| IndexError::CorruptObject("truncated object header".into()))?
        .try_into()
        .map_err(|_conversion_error| IndexError::CorruptObject("invalid object header".into()))?;
    Ok(u32::from_be_bytes(bytes))
}

fn digest_wire(wire: &[u8]) -> ObjectRef {
    let mut hasher = Sha256::new();
    hasher.update(HASH_DOMAIN);
    hasher.update(wire);
    ObjectRef(Digest(hasher.finalize().into()))
}

pub(crate) fn storage_error(error: &std::io::Error) -> IndexError {
    IndexError::Storage(error.to_string())
}

fn validate_private_directory(path: &Path) -> Result<(), IndexError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| storage_error(&error))?;
    validate_private_directory_metadata(path, &metadata)
}

fn validate_private_shard(path: &Path, missing: IndexError) -> Result<(), IndexError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            missing
        } else {
            storage_error(&error)
        }
    })?;
    validate_private_directory_metadata(path, &metadata)
}

fn validate_private_directory_metadata(
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<(), IndexError> {
    if !metadata.file_type().is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(IndexError::Storage(format!(
            "storage directory is not owner-private: {}",
            path.display()
        )));
    }
    Ok(())
}

fn ensure_private_directory(path: &Path) -> Result<(), IndexError> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(storage_error(&error)),
    }
    validate_private_directory(path)
}

fn nofollow_flag() -> Result<i32, IndexError> {
    i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits())
        .map_err(|_conversion_error| IndexError::ResourceLimited)
}

fn validate_private_file(metadata: &fs::Metadata) -> Result<(), IndexError> {
    if !metadata.file_type().is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(IndexError::CorruptObject(
            "object is not an owner-private single-link regular file".into(),
        ));
    }
    Ok(())
}

fn publish_temp_no_replace(
    temp: &mut TempFile,
    final_path: &Path,
    source: &fs::Metadata,
) -> Result<bool, IndexError> {
    // The temporary file and canonical name are in the same shard. A
    // no-replace rename never exposes the canonical inode with two links.
    match rustix::fs::renameat_with(
        rustix::fs::CWD,
        temp.0.as_path(),
        rustix::fs::CWD,
        final_path,
        rustix::fs::RenameFlags::NOREPLACE,
    ) {
        Ok(()) => {
            // Rename consumed the temporary name. Never unlink the canonical
            // name from TempFile's cleanup path if validation now fails.
            temp.0.clear();
            validate_installed_link(final_path, source)?;
            Ok(true)
        }
        Err(rustix::io::Errno::EXIST) => Ok(false),
        Err(error) => Err(IndexError::Storage(format!(
            "publish immutable object without replacement: {error}"
        ))),
    }
}

fn validate_installed_link(path: &Path, source: &fs::Metadata) -> Result<(), IndexError> {
    let linked = fs::symlink_metadata(path).map_err(|error| storage_error(&error))?;
    if !linked.file_type().is_file()
        || linked.dev() != source.dev()
        || linked.ino() != source.ino()
        || linked.uid() != rustix::process::geteuid().as_raw()
        || linked.mode() & 0o7777 != 0o600
        || linked.nlink() != 1
    {
        return Err(IndexError::CorruptObject(
            "installed link does not match private temporary inode".into(),
        ));
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), IndexError> {
    validate_private_directory(path)?;
    let file = File::open(path).map_err(|error| storage_error(&error))?;
    let metadata = file.metadata().map_err(|error| storage_error(&error))?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(IndexError::Storage(
            "storage directory changed during sync".into(),
        ));
    }
    file.sync_all().map_err(|error| storage_error(&error))
}

#[cfg(target_os = "macos")]
fn ensure_same_device(expected: u64, actual: u64) -> Result<(), IndexError> {
    if expected != actual {
        return Err(IndexError::Storage(
            "immutable object batch crosses filesystem devices".into(),
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn open_private_regular_file(path: &Path) -> Result<File, IndexError> {
    let linked = fs::symlink_metadata(path).map_err(|error| storage_error(&error))?;
    validate_private_file(&linked)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nofollow_flag()?)
        .open(path)
        .map_err(|error| storage_error(&error))?;
    let opened = file.metadata().map_err(|error| storage_error(&error))?;
    validate_private_file(&opened)?;
    if opened.dev() != linked.dev() || opened.ino() != linked.ino() {
        return Err(IndexError::CorruptObject(
            "immutable object changed during batch sync".into(),
        ));
    }
    Ok(file)
}

#[cfg(target_os = "macos")]
fn sync_directory_plain(path: &Path, expected_device: u64) -> Result<(), IndexError> {
    validate_private_directory(path)?;
    let file = File::open(path).map_err(|error| storage_error(&error))?;
    let metadata = file.metadata().map_err(|error| storage_error(&error))?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(IndexError::Storage(
            "storage directory changed during batch sync".into(),
        ));
    }
    ensure_same_device(expected_device, metadata.dev())?;
    rustix::fs::fsync(&file)
        .map_err(|error| IndexError::Storage(format!("sync immutable object directory: {error}")))
}

fn sync_parent_directory(path: &Path) -> Result<(), IndexError> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| storage_error(&error))
}

fn hex_digit(nibble: u8) -> char {
    if nibble < 10 {
        char::from(b'0'.wrapping_add(nibble))
    } else {
        char::from(b'a'.wrapping_add(nibble.wrapping_sub(10)))
    }
}

struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            drop(fs::remove_file(&self.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, Instant},
    };

    use quanta_index_core::index::WorkLimits;

    use super::*;

    static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let sequence = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
            Self(
                std::env::temp_dir()
                    .join(format!("qidx-identify-{}-{sequence}", std::process::id())),
            )
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            drop(fs::remove_dir_all(&self.0));
        }
    }

    struct TestSink;

    impl ObjectWriteSink for TestSink {
        fn reserve(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
            Ok(())
        }
        fn installed(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
            Ok(())
        }
        fn reserve_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
            Ok(())
        }
        fn installed_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct CountingSink(AtomicU64, AtomicU64);

    impl ObjectWriteSink for CountingSink {
        fn reserve(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected scalar reserve".into(),
            ))
        }
        fn installed(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected scalar install".into(),
            ))
        }
        fn reserve_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError> {
            assert_eq!(objects.len(), 2);
            let _prior = self.0.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn installed_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError> {
            assert_eq!(objects.len(), 2);
            let _prior = self.1.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    fn budget() -> WorkBudget {
        WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            Instant::now() + Duration::from_secs(30),
        )
    }

    fn shard_count(directory: &TestDir) -> Result<usize, std::io::Error> {
        fs::read_dir(directory.0.join("objects")).map(Iterator::count)
    }

    fn object_file_count(directory: &TestDir) -> Result<usize, std::io::Error> {
        let mut count = 0usize;
        for shard in fs::read_dir(directory.0.join("objects"))? {
            count += fs::read_dir(shard?.path())?.count();
        }
        Ok(count)
    }

    #[test]
    fn object_batch_deduplicates_identical_wire_and_confirms_once()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget();
        let child = store.identify(b"same", &[], &mut work)?.reference;
        let sink = CountingSink::default();
        let inputs = [
            ObjectBatchInput {
                payload: b"same".to_vec(),
                children: Vec::new(),
            },
            ObjectBatchInput {
                payload: b"same".to_vec(),
                children: Vec::new(),
            },
            ObjectBatchInput {
                payload: b"parent".to_vec(),
                children: vec![child],
            },
        ];
        let registrations = ImmutableObjectPort::install_batch(&store, &inputs, &sink, &mut work)?;
        assert_eq!(registrations.first(), registrations.get(1));
        assert_eq!(registrations.len(), 3);
        assert_eq!(sink.0.load(Ordering::Relaxed), 1);
        assert_eq!(sink.1.load(Ordering::Relaxed), 1);
        let parent = registrations.get(2).ok_or("parent registration missing")?;
        assert_eq!(
            store
                .get(parent.reference, &mut work)?
                .registration
                .children,
            vec![child]
        );
        let physical_count = object_file_count(&directory)?;
        let replay = ImmutableObjectPort::install_batch(&store, &inputs, &sink, &mut work)?;
        assert_eq!(replay, registrations);
        assert_eq!(object_file_count(&directory)?, physical_count);
        assert_eq!(sink.0.load(Ordering::Relaxed), 2);
        assert_eq!(sink.1.load(Ordering::Relaxed), 2);
        Ok(())
    }

    #[test]
    fn internal_batch_limits_reject_before_reservation_or_physical_install()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = CountingSink::default();
        let mut work = budget();
        let tiny = store.prepare_object(b"tiny", &[], &mut work)?;
        let over_count = vec![
            &tiny;
            MAX_BATCH_INPUTS
                .checked_add(1)
                .ok_or("test batch count overflow")?
        ];
        assert!(matches!(
            store.install_prepared_batch(&over_count, &sink, &mut work),
            Err(IndexError::ResourceLimited)
        ));
        assert_eq!(sink.0.load(Ordering::Relaxed), 0);
        assert_eq!(sink.1.load(Ordering::Relaxed), 0);
        assert_eq!(object_file_count(&directory)?, 0);

        let mut large_work = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 128 * 1024 * 1024,
                mutations: 100,
            },
            Instant::now() + Duration::from_secs(30),
        );
        let large_payload = vec![7u8; MAX_PAYLOAD];
        let large = store.prepare_object(&large_payload, &[], &mut large_work)?;
        let over_bytes = [&large; 4];
        assert!(matches!(
            store.install_prepared_batch(&over_bytes, &sink, &mut large_work),
            Err(IndexError::ResourceLimited)
        ));
        assert_eq!(sink.0.load(Ordering::Relaxed), 0);
        assert_eq!(sink.1.load(Ordering::Relaxed), 0);
        assert_eq!(object_file_count(&directory)?, 0);
        Ok(())
    }

    #[test]
    fn identify_matches_durable_install_without_touching_storage()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget();
        let child = store.put(b"child", &[], &mut work)?.reference;
        let before = shard_count(&directory)?;
        let identified = store.identify(b"parent", &[child], &mut work)?;
        assert_eq!(shard_count(&directory)?, before);
        assert!(!store.path_for(identified.reference).exists());
        let installed = store.install(b"parent", &[child], &TestSink, &mut work)?;
        assert_eq!(identified, installed);
        assert_eq!(
            store.get(identified.reference, &mut work)?.registration,
            identified
        );
        Ok(())
    }

    #[test]
    fn identify_missing_child_is_pure_but_install_refuses_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget();
        let missing = ObjectRef(Digest([99; 32]));
        let identified = store.identify(b"pending", &[missing], &mut work)?;
        assert_eq!(identified.children, vec![missing]);
        assert_eq!(shard_count(&directory)?, 0);
        let mut denied = WorkBudget::new(
            WorkLimits {
                reads: 0,
                writes: 0,
                bytes: 0,
                mutations: 0,
            },
            Instant::now() + Duration::from_secs(30),
        );
        assert!(matches!(
            store.identify(b"pending", &[missing], &mut denied),
            Err(IndexError::ResourceLimited)
        ));
        assert_eq!(shard_count(&directory)?, 0);
        assert!(
            store
                .install(b"pending", &[missing], &TestSink, &mut work)
                .is_err()
        );
        assert_eq!(shard_count(&directory)?, 0);
        Ok(())
    }

    #[test]
    fn absent_shard_and_empty_shard_have_the_same_object_error_class()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget();
        let missing = ObjectRef(Digest([99; 32]));
        let path = store.path_for(missing);
        let shard = path.parent().ok_or("object shard missing")?;
        assert!(!shard.exists());
        assert!(matches!(
            store.get(missing, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
        assert!(matches!(
            store.install(b"parent", &[missing], &TestSink, &mut work),
            Err(IndexError::InvalidInput(_))
        ));

        ensure_private_directory(shard)?;
        assert!(!path.exists());
        assert!(matches!(
            store.get(missing, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
        assert!(matches!(
            store.install(b"parent", &[missing], &TestSink, &mut work),
            Err(IndexError::InvalidInput(_))
        ));

        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o755))?;
        assert!(matches!(
            store.get(missing, &mut work),
            Err(IndexError::Storage(_))
        ));
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }

    #[test]
    fn no_replace_publication_is_single_link_before_directory_sync()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let writer = FileObjectStore::open(&directory.0)?;
        let reader = FileObjectStore::open(&directory.0)?;
        let mut work = budget();
        let prepared = writer.prepare_object(b"child", &[], &mut work)?;
        let reference = prepared.registration.reference;
        let final_path = writer.path_for(reference);
        let shard = final_path.parent().ok_or("object shard missing")?;
        ensure_private_directory(shard)?;

        let (mut file, temp_path) = writer.create_temp(shard)?;
        file.write_all(&prepared.wire)?;
        file.sync_all()?;
        let source = file.metadata()?;
        validate_private_file(&source)?;
        let mut temp = TempFile(temp_path.clone());
        assert!(publish_temp_no_replace(&mut temp, &final_path, &source)?);
        assert!(!temp_path.exists());
        assert_eq!(fs::symlink_metadata(&final_path)?.nlink(), 1);

        // The other store reads the canonical name while the writer still
        // holds its file descriptor and before the shard directory is synced.
        assert_eq!(reader.get(reference, &mut work)?.payload, b"child");
        let parent = reader.put(b"parent", &[reference], &mut work)?;
        assert_eq!(
            reader
                .get(parent.reference, &mut work)?
                .registration
                .children,
            vec![reference]
        );

        let winner = fs::symlink_metadata(&final_path)?;
        let (mut loser_file, loser_path) = reader.create_temp(shard)?;
        loser_file.write_all(&prepared.wire)?;
        loser_file.sync_all()?;
        let loser_source = loser_file.metadata()?;
        let mut loser = TempFile(loser_path.clone());
        assert!(!publish_temp_no_replace(
            &mut loser,
            &final_path,
            &loser_source,
        )?);
        assert!(loser_path.exists());
        let surviving = fs::symlink_metadata(&final_path)?;
        assert_eq!(
            (surviving.dev(), surviving.ino(), surviving.nlink()),
            (winner.dev(), winner.ino(), 1)
        );
        assert_eq!(reader.get(reference, &mut work)?.payload, b"child");
        drop(loser_file);
        drop(loser);
        assert!(!loser_path.exists());
        drop(file);
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn batch_barrier_failure_leaves_reserved_objects_unconfirmed()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget();
        let child = store.prepare_object(b"child", &[], &mut work)?;
        let parent = store.prepare_object(b"parent", &[child.registration.reference], &mut work)?;
        let objects = [&child, &parent];
        let sink = CountingSink::default();
        let barrier_calls = AtomicU64::new(0);
        let result =
            store.install_prepared_batch_with_barrier(&objects, &sink, &mut work, |file| {
                assert!(
                    file.metadata()
                        .map_err(|error| storage_error(&error))?
                        .is_file()
                );
                for object in objects {
                    let path = store.path_for(object.registration.reference);
                    assert_eq!(
                        fs::read(path).map_err(|error| storage_error(&error))?,
                        object.wire
                    );
                }
                let _prior = barrier_calls.fetch_add(1, Ordering::Relaxed);
                Err(IndexError::Storage("injected batch barrier failure".into()))
            });
        assert!(matches!(result, Err(IndexError::Storage(_))));
        assert_eq!(barrier_calls.load(Ordering::Relaxed), 1);
        assert_eq!(sink.0.load(Ordering::Relaxed), 1);
        assert_eq!(sink.1.load(Ordering::Relaxed), 0);
        assert_eq!(object_file_count(&directory)?, 2);
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn existing_batch_uses_regular_file_barrier_and_rejects_wrong_device()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget();
        let child = store.prepare_object(b"child", &[], &mut work)?;
        let parent = store.prepare_object(b"parent", &[child.registration.reference], &mut work)?;
        let objects = [&child, &parent];
        store.install_prepared_batch(&objects, &CountingSink::default(), &mut work)?;
        let physical_count = object_file_count(&directory)?;

        let device = fs::symlink_metadata(&store.objects)?.dev();
        let other_device = device.checked_add(1).ok_or("test device overflow")?;
        assert!(matches!(
            sync_directory_plain(&store.objects, other_device),
            Err(IndexError::Storage(_))
        ));

        let replay_sink = CountingSink::default();
        let barrier_calls = AtomicU64::new(0);
        store.install_prepared_batch_with_barrier(&objects, &replay_sink, &mut work, |file| {
            let metadata = file.metadata().map_err(|error| storage_error(&error))?;
            assert!(metadata.is_file());
            assert_eq!(metadata.dev(), device);
            rustix::fs::fcntl_fullfsync(file).map_err(|error| {
                IndexError::Storage(format!("full sync existing batch: {error}"))
            })?;
            let _prior = barrier_calls.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })?;
        assert_eq!(barrier_calls.load(Ordering::Relaxed), 1);
        assert_eq!(replay_sink.0.load(Ordering::Relaxed), 1);
        assert_eq!(replay_sink.1.load(Ordering::Relaxed), 1);
        assert_eq!(object_file_count(&directory)?, physical_count);
        Ok(())
    }
}
