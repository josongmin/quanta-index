//! Actual read handles stay owned through read, validation and batch barriers.

use std::{
    fs::{self, File},
    io::Read as _,
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
};

use quanta_index_core::index::{
    IndexError, ObjectReadFile, ObjectReadIoPort, ObjectRef, WorkBudget, WorkUsage,
};

use super::{
    FileObjectStore, HEADER_LEN, MAX_WIRE, ReadWire, digest_wire, hex_digit,
    namespace::NamespaceLease, nofollow_flag, storage_error, validate_private_directory,
    validate_private_file, validate_private_shard,
};

pub(super) enum ObjectIoFile {
    Plain(File),
    Admitted(Box<dyn ObjectReadFile>),
    #[cfg(target_os = "macos")]
    Writer(super::write_io::ObjectWriter),
}

impl ObjectIoFile {
    pub(super) fn file(&self) -> Result<&File, IndexError> {
        match self {
            Self::Plain(file) => Ok(file),
            Self::Admitted(owner) => owner.file(),
            #[cfg(target_os = "macos")]
            Self::Writer(owner) => owner.file(),
        }
    }
}

impl FileObjectStore {
    #[cfg(target_os = "macos")]
    pub(super) fn open_reader(
        &self,
        path: &Path,
        budget: &mut WorkBudget,
    ) -> Result<ObjectIoFile, IndexError> {
        open_reader(self.read_io.as_deref(), path, budget)
    }

    #[cfg(target_os = "macos")]
    pub(super) fn open_private_regular_file(
        &self,
        path: &Path,
        budget: &mut WorkBudget,
    ) -> Result<ObjectIoFile, IndexError> {
        use std::os::unix::fs::MetadataExt as _;
        let linked = std::fs::symlink_metadata(path).map_err(|error| storage_error(&error))?;
        validate_private_file(&linked)?;
        let owner = self.open_reader(path, budget)?;
        let opened = owner
            .file()?
            .metadata()
            .map_err(|error| storage_error(&error))?;
        validate_private_file(&opened)?;
        if opened.dev() != linked.dev() || opened.ino() != linked.ino() {
            return Err(IndexError::CorruptObject(
                "immutable object changed during batch sync".into(),
            ));
        }
        Ok(owner)
    }
}

fn open_reader(
    io: Option<&dyn ObjectReadIoPort>,
    path: &Path,
    budget: &mut WorkBudget,
) -> Result<ObjectIoFile, IndexError> {
    if let Some(io) = io {
        return io.open(path, budget).map(ObjectIoFile::Admitted);
    }
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(nofollow_flag()?)
        .open(path)
        .map(ObjectIoFile::Plain)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                IndexError::CorruptObject("referenced object is absent".into())
            } else {
                storage_error(&error)
            }
        })
}

pub(super) fn object_path(objects: &Path, reference: ObjectRef) -> PathBuf {
    let bytes = (reference.0).0;
    let mut name = String::with_capacity(64);
    for byte in bytes {
        name.push(hex_digit(byte >> 4));
        name.push(hex_digit(byte & 0x0f));
    }
    let shard = format!("{:02x}", bytes.first().copied().unwrap_or_default());
    objects.join(shard).join(name)
}

pub(super) fn read_wire(
    objects: &Path,
    namespace: Option<&NamespaceLease>,
    io: Option<&dyn ObjectReadIoPort>,
    reference: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<ReadWire, IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let root = objects
        .parent()
        .ok_or_else(|| IndexError::Storage("object store has no root".into()))?;
    validate_private_directory(root)?;
    validate_private_directory(objects)?;
    if let Some(namespace) = namespace {
        let _binding = namespace.validate_objects(objects)?;
    }
    let path_bytes = u64::try_from(objects.as_os_str().len())
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_mul(3)
        .ok_or(IndexError::ResourceLimited)?
        .checked_add(512)
        .ok_or(IndexError::ResourceLimited)?;
    let _path_memory = budget.reserve_memory(
        path_bytes,
        quanta_index_core::index::MemoryLifetime::Temporary,
    )?;
    let path = object_path(objects, reference);
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
    let owner = open_reader(io, &path, budget)?;
    let mut file = owner.file()?;
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
        bytes: u64::try_from(file_len).map_err(|_conversion_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let memory = budget.reserve_memory(
        u64::try_from(file_len).map_err(|_error| IndexError::ResourceLimited)?,
        quanta_index_core::index::MemoryLifetime::Temporary,
    )?;
    let mut wire = Vec::new();
    wire.try_reserve_exact(file_len)
        .map_err(|_allocation_error| IndexError::ResourceLimited)?;
    wire.resize(file_len, 0);
    file.read_exact(&mut wire).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            IndexError::CorruptObject("object truncated during read".into())
        } else {
            storage_error(&error)
        }
    })?;
    // Verify EOF without allowing read_to_end to grow the charged buffer.
    let mut extra = [0];
    let extra_bytes = file
        .read(&mut extra)
        .map_err(|error| storage_error(&error))?;
    if extra_bytes != 0 || digest_wire(&wire) != reference {
        return Err(IndexError::CorruptObject(
            "object digest or size mismatch".into(),
        ));
    }
    Ok(ReadWire {
        bytes: wire,
        _memory: memory,
    })
}
