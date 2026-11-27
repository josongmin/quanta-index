//! Actual read handles stay owned through read, validation and batch barriers.

use std::{
    ffi::OsStr,
    fs::{self, File},
    num::NonZeroUsize,
    os::unix::{
        ffi::OsStrExt as _,
        fs::{MetadataExt as _, OpenOptionsExt as _},
    },
    path::{Path, PathBuf},
    time::Instant,
};

use quanta_index_core::RequestBudgetV1;
use quanta_index_core::index::{
    Digest, IndexError, MemoryLease, MemoryLifetime, NativeProtectedSnapshotPageV1, ObjectReadFile,
    ObjectReadIoPort, ObjectRef, RegisteredObject, StoredObject, WorkBudget, WorkUsage,
};
use rustix::fs::OFlags;
use sha2::{Digest as _, Sha256};

use super::{
    HEADER_LEN, MAX_WIRE, charge_decoded_wire, decode_wire_registration, digest_wire, hex_digit,
    namespace::NamespaceLease, parse_wire_header, storage_error, validate_private_directory,
    validate_private_file, validate_private_shard, wire_hasher,
};

const STREAM_BYTES: usize = 64 * 1024;
pub(super) const SMALL_STORED_WIRE_BYTES: usize = 512;

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
    expected: &fs::Metadata,
    budget: &mut WorkBudget,
) -> Result<ObjectIoFile, IndexError> {
    if let Some(io) = io {
        return io
            .open_after_path_metadata(path, expected, budget)
            .map(ObjectIoFile::Admitted);
    }
    // A FIFO substituted after the pathname stat must not block the
    // actual FD type and identity checks at the caller.
    let flags = i32::try_from((OFlags::NOFOLLOW | OFlags::NONBLOCK).bits())
        .map_err(|_error| IndexError::Storage("object read open flags overflow".into()))?;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags)
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
    let ObjectRef(Digest(bytes)) = reference;
    let mut name = [0; 64];
    for (digit, nibble) in name
        .iter_mut()
        .zip(bytes.into_iter().flat_map(|byte| [byte >> 4, byte & 0x0f]))
    {
        *digit = hex_digit(nibble);
    }
    let [first, second, ..] = name;
    let shard = [first, second];
    let suffix_bytes = shard.len().saturating_add(name.len()).saturating_add(2);
    let mut path = PathBuf::with_capacity(objects.as_os_str().len().saturating_add(suffix_bytes));
    path.push(objects);
    path.push(OsStr::from_bytes(&shard));
    path.push(OsStr::from_bytes(&name));
    path
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
    if file_len <= SMALL_STORED_WIRE_BYTES {
        return read_small_stored(owner, file_len, reference, budget);
    }
    let (registration, (payload, memory)) = read_object_from_open(
        owner,
        file_len,
        reference,
        budget,
        |file, payload_len, hasher, budget| {
            let memory = budget.reserve_memory(
                StoredObject::payload_allocation_bytes(payload_len)?,
                MemoryLifetime::Retained,
            )?;
            let payload = read_buffer(file, payload_len)?;
            hasher.update(&payload);
            Ok((payload, memory))
        },
    )?;
    Ok(StoredObject::new(registration, payload, memory))
}

fn read_small_stored(
    owner: ObjectIoFile,
    file_len: usize,
    reference: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<StoredObject, IndexError> {
    // One bounded read covers the header, child references and payload. The
    // admitted file owner stays live through the digest and EOF witness.
    let mut file = owner.file()?;
    let mut wire = [0; SMALL_STORED_WIRE_BYTES];
    let wire = wire
        .get_mut(..file_len)
        .ok_or(IndexError::ResourceLimited)?;
    read_small_wire(&mut file, wire, reference, || budget.checkpoint())?;
    let (payload_len, child_bytes) = parse_wire_header(
        wire.get(..HEADER_LEN).ok_or(IndexError::ResourceLimited)?,
        file_len,
    )?;
    let child_end = HEADER_LEN
        .checked_add(child_bytes)
        .ok_or(IndexError::ResourceLimited)?;
    let children = wire
        .get(HEADER_LEN..child_end)
        .ok_or(IndexError::ResourceLimited)?;
    let payload_bytes = wire.get(child_end..).ok_or(IndexError::ResourceLimited)?;
    if payload_bytes.len() != payload_len {
        return Err(IndexError::CorruptObject(
            "encoded lengths do not match object size".into(),
        ));
    }
    let memory = budget.reserve_memory(
        StoredObject::payload_allocation_bytes(payload_len)?,
        MemoryLifetime::Retained,
    )?;
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(payload_len)
        .map_err(|_allocation_error| IndexError::ResourceLimited)?;
    payload.extend_from_slice(payload_bytes);
    charge_decoded_wire(payload_len, child_bytes, budget)?;
    let registration = decode_wire_registration(reference, file_len, children, budget)?;
    Ok(StoredObject::new(registration, payload, memory))
}

fn read_small_wire(
    file: &mut impl std::io::Read,
    wire: &mut [u8],
    reference: ObjectRef,
    checkpoint: impl FnOnce() -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    read_exact(file, wire)?;
    checkpoint()?;
    verify_wire_end(file, reference, || digest_wire(wire))
}

/// Authenticate the whole object with bounded payload scratch and retain
/// just its registered edges for closure traversal.
pub(super) fn read_registration(
    objects: &Path,
    namespace: &NamespaceLease,
    io: &dyn ObjectReadIoPort,
    reference: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<RegisteredObject, IndexError> {
    let (registration, ()) = read_object(
        objects,
        Some(namespace),
        Some(io),
        reference,
        budget,
        |file, payload_len, hasher, budget| {
            if payload_len == 0 {
                return Ok(());
            }
            let scratch_len = payload_len.min(STREAM_BYTES);
            let _memory = budget.reserve_memory(
                u64::try_from(scratch_len).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?;
            let mut scratch = zeroed_buffer(scratch_len)?;
            hash_wire_tail(file, payload_len, &mut scratch, hasher, budget)
        },
    )?;
    Ok(registration)
}

struct ProtectedSnapshotPage<'a> {
    owner: ObjectIoFile,
    registration: RegisteredObject,
    reference: ObjectRef,
    payload_len: usize,
    remaining: usize,
    hasher: Sha256,
    objects: &'a Path,
    namespace: Option<&'a NamespaceLease>,
    request: RequestBudgetV1,
    checkpoint_allowance: usize,
    // The page and its admitted FD retire before this backing loan.
    _memory: Option<MemoryLease>,
}

impl ProtectedSnapshotPage<'_> {
    fn checkpoint(&self) -> Result<(), IndexError> {
        if self.request.is_cancelled() {
            Err(IndexError::Cancelled)
        } else if Instant::now() >= self.request.deadline() {
            Err(IndexError::ResourceLimited)
        } else {
            Ok(())
        }
    }
}

impl std::io::Read for ProtectedSnapshotPage<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() || self.remaining == 0 {
            return Ok(0);
        }
        if self.checkpoint_allowance == 0 {
            self.checkpoint().map_err(std::io::Error::other)?;
            self.checkpoint_allowance = STREAM_BYTES;
        }
        let width = buffer
            .len()
            .min(self.remaining)
            .min(self.checkpoint_allowance);
        let mut file = self.owner.file().map_err(std::io::Error::other)?;
        let count = loop {
            match file.read(&mut buffer[..width]) {
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                    // An interrupted read consumes no bytes and cannot reach
                    // the next size-based checkpoint by itself.
                    self.checkpoint().map_err(std::io::Error::other)?;
                }
                Err(error) => return Err(std::io::Error::other(storage_error(&error))),
                Ok(count) => break count,
            }
        };
        if count == 0 {
            return Err(std::io::Error::other(IndexError::CorruptObject(
                "object truncated during read".into(),
            )));
        }
        self.hasher.update(&buffer[..count]);
        self.remaining -= count;
        self.checkpoint_allowance -= count;
        Ok(count)
    }
}

impl NativeProtectedSnapshotPageV1 for ProtectedSnapshotPage<'_> {
    fn registration(&self) -> &RegisteredObject {
        &self.registration
    }

    fn payload_len(&self) -> usize {
        self.payload_len
    }

    fn finish(self: Box<Self>) -> Result<(), IndexError> {
        let page = *self;
        page.checkpoint()?;
        if page.remaining != 0 {
            return Err(IndexError::CorruptObject(
                "object payload not fully consumed".into(),
            ));
        }
        let mut file = page.owner.file()?;
        verify_wire_end(&mut file, page.reference, || finish_digest(page.hasher))?;
        validate_stream_namespace(page.objects, page.namespace)
    }
}

fn validate_stream_namespace(
    objects: &Path,
    namespace: Option<&NamespaceLease>,
) -> Result<(), IndexError> {
    if let Some(namespace) = namespace {
        let _binding = namespace.validate_objects(objects)?;
    } else {
        let root = objects
            .parent()
            .ok_or_else(|| IndexError::Storage("object store has no root".into()))?;
        validate_private_directory(root)?;
        validate_private_directory(objects)?;
    }
    Ok(())
}

/// Read one physical snapshot page through the same admitted inode and digest
/// as a regular object read. The caller must consume it and call `finish`
/// before using any decoded data.
pub(super) fn open_protected_snapshot_page<'a>(
    objects: &'a Path,
    namespace: Option<&'a NamespaceLease>,
    io: Option<&dyn ObjectReadIoPort>,
    reference: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<Box<dyn NativeProtectedSnapshotPageV1 + 'a>, IndexError> {
    let memory = budget.reserve_memory(
        u64::try_from(std::mem::size_of::<ProtectedSnapshotPage<'a>>())
            .map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let prefix = open_wire_prefix(objects, namespace, io, reference, budget)?;
    charge_decoded_wire(prefix.payload_len, prefix.child_bytes, budget)?;
    let registration =
        decode_wire_registration(reference, prefix.file_len, &prefix.children, budget)?;
    let request = budget.request().clone();
    let WirePrefix {
        owner,
        payload_len,
        hasher,
        ..
    } = prefix;
    Ok(Box::new(ProtectedSnapshotPage {
        owner,
        registration,
        reference,
        payload_len,
        remaining: payload_len,
        hasher,
        objects,
        namespace,
        request,
        checkpoint_allowance: 0,
        _memory: memory,
    }))
}

fn read_object<T>(
    objects: &Path,
    namespace: Option<&NamespaceLease>,
    io: Option<&dyn ObjectReadIoPort>,
    reference: ObjectRef,
    budget: &mut WorkBudget,
    read_payload: impl FnOnce(&mut &File, usize, &mut Sha256, &WorkBudget) -> Result<T, IndexError>,
) -> Result<(RegisteredObject, T), IndexError> {
    let (owner, file_len) = open_wire_reader(objects, namespace, io, reference, budget)?;
    read_object_from_open(owner, file_len, reference, budget, read_payload)
}

fn read_object_from_open<T>(
    owner: ObjectIoFile,
    file_len: usize,
    reference: ObjectRef,
    budget: &mut WorkBudget,
    read_payload: impl FnOnce(&mut &File, usize, &mut Sha256, &WorkBudget) -> Result<T, IndexError>,
) -> Result<(RegisteredObject, T), IndexError> {
    let mut prefix = open_wire_prefix_from_open(owner, file_len, reference, budget)?;
    let mut file = prefix.owner.file()?;
    let payload = read_payload(&mut file, prefix.payload_len, &mut prefix.hasher, budget)?;
    verify_wire_end(&mut file, reference, || {
        finish_digest(std::mem::take(&mut prefix.hasher))
    })?;
    charge_decoded_wire(prefix.payload_len, prefix.child_bytes, budget)?;
    let registration =
        decode_wire_registration(reference, prefix.file_len, &prefix.children, budget)?;
    Ok((registration, payload))
}

// Own the child wire bytes and their loan until the caller either decodes
// registration or refuses. Fields drop in order, so bytes retire before loan.
struct WirePrefix {
    owner: ObjectIoFile,
    file_len: usize,
    payload_len: usize,
    child_bytes: usize,
    children: Vec<u8>,
    _child_wire_memory: Option<MemoryLease>,
    hasher: Sha256,
}

fn open_wire_prefix(
    objects: &Path,
    namespace: Option<&NamespaceLease>,
    io: Option<&dyn ObjectReadIoPort>,
    reference: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<WirePrefix, IndexError> {
    let (owner, file_len) = open_wire_reader(objects, namespace, io, reference, budget)?;
    open_wire_prefix_from_open(owner, file_len, reference, budget)
}

fn open_wire_prefix_from_open(
    owner: ObjectIoFile,
    file_len: usize,
    reference: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<WirePrefix, IndexError> {
    let mut file = owner.file()?;
    let mut header = [0; HEADER_LEN];
    read_exact(&mut file, &mut header)?;
    let mut hasher = wire_hasher();
    hasher.update(header);
    let (payload_len, child_bytes) = match parse_wire_header(&header, file_len) {
        Ok(layout) => layout,
        Err(header_error) => {
            // Lengths from a bad header cannot size allocations. Keep the
            // physical digest/EOF error ahead of the format error.
            let mut scratch = [0; 4096];
            hash_wire_tail(
                &mut file,
                file_len.saturating_sub(HEADER_LEN),
                &mut scratch,
                &mut hasher,
                budget,
            )?;
            verify_wire_end(&mut file, reference, || finish_digest(hasher))?;
            return Err(header_error);
        }
    };

    let child_wire_memory = if child_bytes == 0 {
        None
    } else {
        budget.reserve_memory(
            u64::try_from(child_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
    };
    let children = read_buffer(&mut file, child_bytes)?;
    hasher.update(&children);
    Ok(WirePrefix {
        owner,
        file_len,
        payload_len,
        child_bytes,
        children,
        _child_wire_memory: child_wire_memory,
        hasher,
    })
}

fn hash_wire_tail(
    file: &mut &File,
    remaining: usize,
    scratch: &mut [u8],
    hasher: &mut Sha256,
    budget: &WorkBudget,
) -> Result<(), IndexError> {
    let chunk_size = NonZeroUsize::new(scratch.len())
        .ok_or(IndexError::ResourceLimited)?
        .get();
    for offset in (0..remaining).step_by(chunk_size) {
        budget.checkpoint()?;
        let len = remaining.saturating_sub(offset).min(chunk_size);
        let chunk = scratch.get_mut(..len).ok_or(IndexError::ResourceLimited)?;
        read_exact(file, chunk)?;
        hasher.update(chunk);
    }
    Ok(())
}

fn finish_digest(hasher: Sha256) -> ObjectRef {
    ObjectRef(Digest(hasher.finalize().into()))
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
    let owner = open_reader(io, &path, &metadata, budget)?;
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
    let mut bytes = zeroed_buffer(len)?;
    read_exact(file, &mut bytes)?;
    Ok(bytes)
}

fn zeroed_buffer(len: usize) -> Result<Vec<u8>, IndexError> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(len)
        .map_err(|_allocation_error| IndexError::ResourceLimited)?;
    bytes.resize(len, 0);
    Ok(bytes)
}

fn read_exact(file: &mut impl std::io::Read, bytes: &mut [u8]) -> Result<(), IndexError> {
    file.read_exact(bytes).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            IndexError::CorruptObject("object truncated during read".into())
        } else {
            storage_error(&error)
        }
    })
}

fn verify_wire_end(
    file: &mut impl std::io::Read,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_wire_plain_reader_opens_with_nonblocking_flag() -> Result<(), IndexError> {
        let file = tempfile::NamedTempFile::new().map_err(|error| storage_error(&error))?;
        let metadata = file
            .as_file()
            .metadata()
            .map_err(|error| storage_error(&error))?;
        let mut budget = WorkBudget::with_request(
            quanta_index_core::index::WorkLimits {
                reads: 0,
                writes: 0,
                bytes: 0,
                mutations: 0,
            },
            RequestBudgetV1::unbounded(),
        );
        let owner = open_reader(None, file.path(), &metadata, &mut budget)?;
        let flags = rustix::fs::fcntl_getfl(owner.file()?).map_err(|error| {
            storage_error(&std::io::Error::from_raw_os_error(error.raw_os_error()))
        })?;
        assert!(flags.contains(OFlags::NONBLOCK));
        Ok(())
    }

    struct CountedWire<'a> {
        bytes: &'a [u8],
        calls: usize,
    }

    impl std::io::Read for CountedWire<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.calls += 1;
            let count = buffer.len().min(self.bytes.len());
            buffer[..count].copy_from_slice(&self.bytes[..count]);
            self.bytes = &self.bytes[count..];
            Ok(count)
        }
    }

    #[test]
    fn small_wire_uses_one_data_read_and_one_eof_read() -> Result<(), IndexError> {
        let wire = b"small authenticated wire";
        let reference = digest_wire(wire);
        let mut reader = CountedWire {
            bytes: wire,
            calls: 0,
        };
        let mut observed = [0; 24];
        read_small_wire(&mut reader, &mut observed, reference, || Ok(()))?;
        assert_eq!(reader.calls, 2);
        assert_eq!(observed, *wire);

        let mut appended = CountedWire {
            bytes: b"small authenticated wire!",
            calls: 0,
        };
        assert!(matches!(
            read_small_wire(&mut appended, &mut observed, reference, || Ok(())),
            Err(IndexError::CorruptObject(message)) if message == "object digest or size mismatch"
        ));
        let mut truncated = CountedWire {
            bytes: b"small authenticated wir",
            calls: 0,
        };
        assert!(matches!(
            read_small_wire(&mut truncated, &mut observed, reference, || Ok(())),
            Err(IndexError::CorruptObject(message)) if message == "object truncated during read"
        ));
        let mut damaged = CountedWire {
            bytes: b"small authenticated WIRE",
            calls: 0,
        };
        assert!(matches!(
            read_small_wire(&mut damaged, &mut observed, reference, || Ok(())),
            Err(IndexError::CorruptObject(message)) if message == "object digest or size mismatch"
        ));
        Ok(())
    }
}
