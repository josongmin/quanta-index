//! Actual read handles stay owned through read, validation and batch barriers.

use std::{
    fs::{self, File},
    io::Read as _,
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
};

use quanta_index_core::index::{
    Digest, IndexError, MemoryLease, MemoryLifetime, ObjectReadFile, ObjectReadIoPort, ObjectRef,
    StoredObject, WorkBudget, WorkUsage,
};
use sha2::Digest as _;

use super::{
    HEADER_LEN, MAX_WIRE, charge_decoded_wire, decode_wire_registration, digest_wire,
    digest_wire_parts, hex_digit, namespace::NamespaceLease, nofollow_flag, parse_wire_header,
    storage_error, validate_private_directory, validate_private_file, validate_private_shard,
    wire_hasher,
};

/// Only the completed file read constructs this immutable, reference-bound wire.
pub(super) struct ReadWire {
    bytes: Vec<u8>,
    // Drop the bytes before releasing their live allocation custody.
    _memory: Option<MemoryLease>,
}

impl ReadWire {
    pub(super) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

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
    let (wire, _owner) = read_wire_with_owner(objects, namespace, io, reference, budget)?;
    Ok(wire)
}

pub(super) fn path_memory_bytes(objects: &Path) -> Result<u64, IndexError> {
    u64::try_from(objects.as_os_str().len())
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_mul(3)
        .and_then(|bytes| bytes.checked_add(512))
        .ok_or(IndexError::ResourceLimited)
}

pub(super) fn read_wire_with_owner(
    objects: &Path,
    namespace: Option<&NamespaceLease>,
    io: Option<&dyn ObjectReadIoPort>,
    reference: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<(ReadWire, ObjectIoFile), IndexError> {
    let (owner, file_len) = open_wire_reader(objects, namespace, io, reference, budget)?;
    let mut file = owner.file()?;
    let memory = budget.reserve_memory(
        u64::try_from(file_len).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let wire = read_buffer(&mut file, file_len)?;
    verify_wire_end(&mut file, reference, || digest_wire(&wire))?;
    Ok((
        ReadWire {
            bytes: wire,
            _memory: memory,
        },
        owner,
    ))
}

/// Read payload bytes into their retained backing. The full wire buffer is
/// only needed by callers that compare an existing file byte for byte.
pub(super) fn read_stored(
    objects: &Path,
    namespace: Option<&NamespaceLease>,
    io: Option<&dyn ObjectReadIoPort>,
    reference: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<StoredObject, IndexError> {
    let (owner, file_len) = open_wire_reader(objects, namespace, io, reference, budget)?;
    let mut file = owner.file()?;
    let mut header = [0; HEADER_LEN];
    read_exact(&mut file, &mut header)?;
    let (payload_len, child_bytes) = match parse_wire_header(&header, file_len) {
        Ok(layout) => layout,
        Err(header_error) => {
            // Lengths from a bad header cannot size allocations. Keep the
            // physical digest/EOF error ahead of the format error.
            verify_wire_after_bad_header(&mut file, &header, file_len, reference)?;
            return Err(header_error);
        }
    };

    let memory = budget.reserve_memory(
        StoredObject::payload_allocation_bytes(payload_len)?,
        MemoryLifetime::Retained,
    )?;
    let _child_wire_memory = if child_bytes == 0 {
        None
    } else {
        budget.reserve_memory(
            u64::try_from(child_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
    };
    let children = read_buffer(&mut file, child_bytes)?;
    let payload = read_buffer(&mut file, payload_len)?;
    verify_wire_end(&mut file, reference, || {
        digest_wire_parts(&[&header, &children, &payload])
    })?;
    charge_decoded_wire(payload_len, child_bytes, budget)?;
    let registration = decode_wire_registration(reference, file_len, &children, budget)?;
    Ok(StoredObject::new(registration, payload, memory))
}

fn verify_wire_after_bad_header(
    file: &mut &File,
    header: &[u8; HEADER_LEN],
    file_len: usize,
    reference: ObjectRef,
) -> Result<(), IndexError> {
    let mut hasher = wire_hasher();
    hasher.update(header);
    let mut scratch = [0; 4096];
    let chunk_size = scratch.len();
    for offset in (HEADER_LEN..file_len).step_by(chunk_size) {
        let len = file_len.saturating_sub(offset).min(chunk_size);
        let chunk = scratch.get_mut(..len).ok_or(IndexError::ResourceLimited)?;
        read_exact(file, chunk)?;
        hasher.update(chunk);
    }
    verify_wire_end(file, reference, || {
        ObjectRef(Digest(hasher.finalize().into()))
    })
}

fn open_wire_reader(
    objects: &Path,
    namespace: Option<&NamespaceLease>,
    io: Option<&dyn ObjectReadIoPort>,
    reference: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<(ObjectIoFile, usize), IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    if let Some(namespace) = namespace {
        // The held lease validates the live root, lease and objects names and
        // their private metadata together. Repeat the generic path checks only
        // for legacy reads without a namespace binding.
        let _binding = namespace.validate_objects(objects)?;
    } else {
        let root = objects
            .parent()
            .ok_or_else(|| IndexError::Storage("object store has no root".into()))?;
        validate_private_directory(root)?;
        validate_private_directory(objects)?;
    }
    let path_bytes = path_memory_bytes(objects)?;
    let _path_memory = budget.reserve_memory(path_bytes, MemoryLifetime::Temporary)?;
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
    let file = owner.file()?;
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
    Ok((owner, file_len))
}

fn read_buffer(file: &mut &File, len: usize) -> Result<Vec<u8>, IndexError> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(len)
        .map_err(|_allocation_error| IndexError::ResourceLimited)?;
    bytes.resize(len, 0);
    read_exact(file, &mut bytes)?;
    Ok(bytes)
}

fn read_exact(file: &mut &File, bytes: &mut [u8]) -> Result<(), IndexError> {
    file.read_exact(bytes).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            IndexError::CorruptObject("object truncated during read".into())
        } else {
            storage_error(&error)
        }
    })
}

fn verify_wire_end(
    file: &mut &File,
    reference: ObjectRef,
    digest: impl FnOnce() -> ObjectRef,
) -> Result<(), IndexError> {
    // Verify EOF without allowing read_to_end to grow the charged buffer.
    let mut extra = [0];
    let extra_bytes = file
        .read(&mut extra)
        .map_err(|error| storage_error(&error))?;
    if extra_bytes != 0 || digest() != reference {
        return Err(IndexError::CorruptObject(
            "object digest or size mismatch".into(),
        ));
    }
    Ok(())
}
