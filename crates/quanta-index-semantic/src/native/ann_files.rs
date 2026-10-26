//! Immutable, attempt-owned Lance file sets for one Native ANN region.
//!
//! Every file is split into ordinary managed objects. The descriptor lists
//! their exact references and is the sole parent registered with Catalog.
//! A selected reader keeps the returned `StoredObject`s and their memory
//! grants alive; no file or dataset is concatenated in memory here.

use std::io;

use quanta_index_core::index::{
    AttemptContext, Digest, IndexError, MemoryLease, MemoryLifetime, ObjectRef, StagedObjectBatch,
    StagedPayload, StoredObject, WorkBudget, WorkUsage, ports::ImmutableObjectPort,
};
use quanta_index_core::{
    DiagnosticAnnIndexKindV1, DiagnosticAnnRegionObservationV1, DiagnosticAnnRegionPhaseV1,
};
use sha2::{Digest as _, Sha256};

use crate::codec::cbor_serde;

use super::ann_lance::{MAX_REGION_CELLS, MAX_REGION_DIMENSION, MAX_REGION_ROWS, RegionSeal};
use crate::manifest::{VECTOR_INDEX_MODE_EXACT, VECTOR_INDEX_MODE_IVF_HNSW_SQ};

const FILE_SET_FORMAT: u32 = 1;
pub(super) const MAX_FILE_COUNT: usize = 256;
pub(super) const MAX_TOTAL_BYTES: usize = 33_554_432;
pub(super) const MAX_CHUNK_BYTES: usize = 1_048_576;
pub(super) const MAX_PATH_BYTES: usize = 256;
// Each nonempty file has at most one short final chunk beyond full MiB chunks.
const MAX_CHUNK_COUNT: usize = 288;
const MAX_MANIFEST_BYTES: usize = 524_288;
const MAX_INSTALL_METADATA_MEMORY_BYTES: u64 = 4_718_592;
const MAX_STAGED_CHUNKS: usize = 64;

#[derive(Clone, Copy)]
struct PendingChunk<'a> {
    file: usize,
    chunk: usize,
    bytes: &'a [u8],
}

struct FinalChunkBatch {
    staged: StagedObjectBatch,
    predicted: [Option<ObjectRef>; MAX_STAGED_CHUNKS],
}

fn install_pending_chunks(
    context: &mut AttemptContext<'_>,
    manifest: &mut AnnFileSet,
    pending: &mut [Option<PendingChunk<'_>>; MAX_STAGED_CHUNKS],
    pending_len: &mut usize,
) -> Result<(), IndexError> {
    if *pending_len == 0 {
        return Ok(());
    }
    if *pending_len == 1 {
        let first = pending.first().copied().flatten().ok_or_else(|| {
            IndexError::BackendViolation("ANN singleton chunk slot missing".into())
        })?;
        let reference = context.put(first.bytes, &[])?;
        assign_pending_chunks(manifest, pending, pending_len, &[reference])
    } else {
        let mut staged = StagedObjectBatch::new(*pending_len, context.budget)?;
        for slot in pending.iter().take(*pending_len) {
            let item = slot.ok_or_else(|| {
                IndexError::BackendViolation("ANN chunk batch slot missing".into())
            })?;
            staged.push(
                StagedPayload::copy(item.bytes, context.budget)?,
                &[],
                context.budget,
            )?;
        }
        let references = context.put_batch(staged.inputs())?;
        assign_pending_chunks(manifest, pending, pending_len, &references)
    }
}

fn assign_pending_chunks(
    manifest: &mut AnnFileSet,
    pending: &mut [Option<PendingChunk<'_>>; MAX_STAGED_CHUNKS],
    pending_len: &mut usize,
    references: &[ObjectRef],
) -> Result<(), IndexError> {
    if references.len() != *pending_len {
        return Err(IndexError::BackendViolation(
            "ANN chunk batch lost ordinal outputs".into(),
        ));
    }
    for (slot, reference) in pending.iter_mut().take(*pending_len).zip(references) {
        let item = slot.take().ok_or_else(|| {
            IndexError::BackendViolation("ANN chunk batch destination missing".into())
        })?;
        manifest
            .files
            .get_mut(item.file)
            .and_then(|file| file.chunks.get_mut(item.chunk))
            .ok_or_else(|| {
                IndexError::BackendViolation("ANN chunk batch destination missing".into())
            })?
            .reference = *reference;
    }
    *pending_len = 0;
    Ok(())
}

// The final bounded tail and its descriptor can share one child-first install.
// Keep the ordinary flush path for a full tail or one whose payload leaves no
// room for the descriptor under the existing MiB staging envelope.
fn prepare_final_chunk_batch(
    context: &mut AttemptContext<'_>,
    manifest: &mut AnnFileSet,
    pending: &[Option<PendingChunk<'_>>; MAX_STAGED_CHUNKS],
    pending_len: usize,
    pending_bytes: usize,
) -> Result<Option<FinalChunkBatch>, IndexError> {
    let max_tail_bytes = MAX_CHUNK_BYTES
        .checked_sub(MAX_MANIFEST_BYTES)
        .ok_or(IndexError::ResourceLimited)?;
    if pending_len == 0 || pending_len >= MAX_STAGED_CHUNKS || pending_bytes > max_tail_bytes {
        return Ok(None);
    }
    let count = pending_len
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    let mut staged = StagedObjectBatch::new(count, context.budget)?;
    let mut predicted = [None; MAX_STAGED_CHUNKS];
    for (ordinal, slot) in pending.iter().take(pending_len).enumerate() {
        let item = slot
            .ok_or_else(|| IndexError::BackendViolation("ANN final chunk slot missing".into()))?;
        let reference = context
            .objects
            .identify(item.bytes, &[], context.budget)?
            .reference;
        manifest
            .files
            .get_mut(item.file)
            .and_then(|file| file.chunks.get_mut(item.chunk))
            .ok_or_else(|| {
                IndexError::BackendViolation("ANN final chunk destination missing".into())
            })?
            .reference = reference;
        *predicted
            .get_mut(ordinal)
            .ok_or(IndexError::ResourceLimited)? = Some(reference);
        staged.push(
            StagedPayload::copy(item.bytes, context.budget)?,
            &[],
            context.budget,
        )?;
    }
    Ok(Some(FinalChunkBatch { staged, predicted }))
}

/// The caller must retain the admitted source bytes and their grant until
/// `install_fileset` returns. This function never takes ownership of them.
pub(super) struct AnnFileInput<'a> {
    pub path: &'a str,
    pub bytes: &'a [u8],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AnnChunk {
    pub reference: ObjectRef,
    pub length: u32,
}

cbor_serde!(AnnChunk {
    reference: ObjectRef,
    length: u32,
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AnnFile {
    pub path: String,
    pub length: u64,
    pub sha256: Digest,
    pub chunks: Vec<AnnChunk>,
}

cbor_serde!(AnnFile {
    path: String,
    length: u64,
    sha256: Digest,
    chunks: Vec<AnnChunk>,
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AnnFileSet {
    pub format: u32,
    pub recipe: Digest,
    pub seal: RegionSeal,
    pub total_length: u64,
    pub files: Vec<AnnFile>,
}

cbor_serde!(AnnFileSet {
    format: u32,
    recipe: Digest,
    seal: RegionSeal,
    total_length: u64,
    files: Vec<AnnFile>,
});

pub(super) struct LoadedAnnChunk {
    pub length: u32,
    pub object: StoredObject,
}

pub(super) struct LoadedAnnFile {
    pub path: String,
    pub length: u64,
    pub chunks: Vec<LoadedAnnChunk>,
}

/// Manifest authority only; physical chunk bodies have not been opened.
pub(super) struct LoadedAnnFileSetSeal {
    pub seal: RegionSeal,
    _descriptor: StoredObject,
    _metadata_memory: Option<MemoryLease>,
}

struct LoadedAnnManifest {
    manifest: AnnFileSet,
    descriptor: StoredObject,
    metadata_memory: Option<MemoryLease>,
}

pub(super) struct LoadedAnnFileSet {
    pub seal: RegionSeal,
    pub total_length: u64,
    pub files: Vec<LoadedAnnFile>,
    // The descriptor and decoded metadata remain funded alongside file data.
    _descriptor: StoredObject,
    _metadata_memory: Option<MemoryLease>,
}

fn invalid(message: &'static str) -> IndexError {
    IndexError::InvalidInput(message.into())
}

fn corrupt(message: &'static str) -> IndexError {
    IndexError::CorruptObject(message.into())
}

/// Accept only a portable lexical relative path. Restricting components to
/// printable ASCII makes the same byte spelling canonical on every host.
fn valid_relative_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH_BYTES
        && !path.starts_with('/')
        && path.bytes().all(|byte| {
            (b'!'..=b'~').contains(&byte)
                && !matches!(byte, b'\\' | b':' | b'?' | b'*' | b'<' | b'>' | b'"' | b'|')
        })
        && path
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn validate_seal(seal: &RegionSeal) -> Result<(), &'static str> {
    let row_limit = u64::try_from(MAX_REGION_ROWS).map_err(|_error| "ANN row limit overflow")?;
    let dimension_limit =
        u64::try_from(MAX_REGION_DIMENSION).map_err(|_error| "ANN dimension limit overflow")?;
    let cell_limit = u64::try_from(MAX_REGION_CELLS).map_err(|_error| "ANN cell limit overflow")?;
    if seal.generation == 0
        || seal.rows == 0
        || u64::from(seal.rows) > row_limit
        || seal.dimension == 0
        || u64::from(seal.dimension) > dimension_limit
        || u64::from(seal.rows)
            .checked_mul(u64::from(seal.dimension))
            .is_none_or(|cells| cells > cell_limit)
        || seal
            .vector_index
            .validate(u64::from(seal.rows), seal.generation)
            .is_err()
    {
        return Err("Native ANN file set has an invalid region seal");
    }
    Ok(())
}

pub(super) fn diagnostic_index_kind(
    seal: &RegionSeal,
) -> Result<DiagnosticAnnIndexKindV1, IndexError> {
    match seal.vector_index.mode.as_str() {
        VECTOR_INDEX_MODE_EXACT if seal.vector_index.ann.is_none() => {
            Ok(DiagnosticAnnIndexKindV1::Flat)
        }
        VECTOR_INDEX_MODE_IVF_HNSW_SQ if seal.vector_index.ann.is_some() => {
            Ok(DiagnosticAnnIndexKindV1::IvfHnswSq)
        }
        _ => Err(corrupt("Native ANN file-set index mode differs from seal")),
    }
}

fn validate_inputs(files: &[AnnFileInput<'_>], seal: &RegionSeal) -> Result<(), IndexError> {
    validate_seal(seal).map_err(invalid)?;
    if files.is_empty() || files.len() > MAX_FILE_COUNT {
        return Err(invalid(
            "Native ANN file count is outside the bounded policy",
        ));
    }
    let mut prior = None;
    let mut total = 0usize;
    for file in files {
        if !valid_relative_path(file.path) || prior.is_some_and(|path| path >= file.path) {
            return Err(invalid(
                "Native ANN file paths must be canonical and sorted",
            ));
        }
        prior = Some(file.path);
        total = total
            .checked_add(file.bytes.len())
            .filter(|total| *total <= MAX_TOTAL_BYTES)
            .ok_or(IndexError::ResourceLimited)?;
    }
    if total == 0 {
        return Err(invalid("Native ANN file set contains no file bytes"));
    }
    Ok(())
}

fn validate_manifest(manifest: &AnnFileSet) -> Result<(), &'static str> {
    if manifest.format != FILE_SET_FORMAT || manifest.recipe != manifest.seal.recipe {
        return Err("Native ANN file-set format or recipe mismatch");
    }
    validate_seal(&manifest.seal)?;
    if manifest.files.is_empty() || manifest.files.len() > MAX_FILE_COUNT {
        return Err("Native ANN file-set file count is invalid");
    }
    let mut prior = None;
    let mut total = 0u64;
    let mut chunk_count = 0usize;
    let chunk_limit =
        u32::try_from(MAX_CHUNK_BYTES).map_err(|_error| "Native ANN chunk limit overflow")?;
    let chunk_limit_u64 = u64::from(chunk_limit);
    let total_limit =
        u64::try_from(MAX_TOTAL_BYTES).map_err(|_error| "Native ANN total limit overflow")?;
    for file in &manifest.files {
        if !valid_relative_path(&file.path)
            || prior.is_some_and(|path: &str| path >= file.path.as_str())
        {
            return Err("Native ANN file-set path order is invalid");
        }
        prior = Some(file.path.as_str());
        if file.length > total_limit {
            return Err("Native ANN file length exceeds the bounded policy");
        }
        let expected = file.length.div_ceil(chunk_limit_u64);
        if u64::try_from(file.chunks.len()) != Ok(expected) {
            return Err("Native ANN file chunk count is not canonical");
        }
        let mut file_length = 0u64;
        for (ordinal, chunk) in file.chunks.iter().enumerate() {
            if chunk.length == 0
                || chunk.length > chunk_limit
                || (ordinal < file.chunks.len().saturating_sub(1) && chunk.length != chunk_limit)
            {
                return Err("Native ANN file chunk length is not canonical");
            }
            file_length = file_length
                .checked_add(u64::from(chunk.length))
                .ok_or("Native ANN file chunk length overflow")?;
        }
        if file_length != file.length {
            return Err("Native ANN file length differs from its chunks");
        }
        total = total
            .checked_add(file.length)
            .ok_or("Native ANN file-set total length overflow")?;
        chunk_count = chunk_count
            .checked_add(file.chunks.len())
            .ok_or("Native ANN file-set chunk count overflow")?;
        if total > total_limit || chunk_count > MAX_CHUNK_COUNT {
            return Err("Native ANN file set exceeds the bounded policy");
        }
    }
    if total == 0 || total != manifest.total_length {
        return Err("Native ANN file-set total length mismatch");
    }
    Ok(())
}

fn child_union(manifest: &AnnFileSet) -> Result<Vec<ObjectRef>, IndexError> {
    let count = manifest.files.iter().try_fold(0usize, |count, file| {
        count
            .checked_add(file.chunks.len())
            .ok_or(IndexError::ResourceLimited)
    })?;
    if count > MAX_CHUNK_COUNT {
        return Err(IndexError::ResourceLimited);
    }
    let mut children = Vec::new();
    children
        .try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    for file in &manifest.files {
        children.extend(file.chunks.iter().map(|chunk| chunk.reference));
    }
    children.sort_unstable();
    children.dedup();
    Ok(children)
}

struct BoundedManifestWriter {
    bytes: Vec<u8>,
    overflow: bool,
}

impl BoundedManifestWriter {
    fn new() -> Result<Self, IndexError> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(MAX_MANIFEST_BYTES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(Self {
            bytes,
            overflow: false,
        })
    }
}

impl io::Write for BoundedManifestWriter {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(input.len())
            .is_none_or(|next| next > MAX_MANIFEST_BYTES)
        {
            self.overflow = true;
            return Err(io::ErrorKind::InvalidData.into());
        }
        self.bytes.extend_from_slice(input);
        Ok(input.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encode_manifest(manifest: &AnnFileSet) -> Result<Vec<u8>, IndexError> {
    let mut writer = BoundedManifestWriter::new()?;
    let result = serde_json::to_writer(&mut writer, manifest);
    if writer.overflow {
        return Err(IndexError::ResourceLimited);
    }
    result.map_err(|error| IndexError::Storage(format!("Native ANN file-set encode: {error}")))?;
    Ok(writer.bytes)
}

fn read_metadata_memory_bytes(payload_bytes: usize) -> Result<u64, IndexError> {
    let bytes = payload_bytes
        .checked_mul(8)
        .and_then(|bytes| bytes.checked_add(MAX_MANIFEST_BYTES))
        .ok_or(IndexError::ResourceLimited)?;
    u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)
}

/// Install children first, then register one descriptor whose exact child
/// union is the complete physical file set.
///
/// An aborted attempt leaves only attempt-owned objects for the existing
/// Catalog reclaim path.
pub(super) fn install_fileset(
    context: &mut AttemptContext<'_>,
    seal: RegionSeal,
    files: &[AnnFileInput<'_>],
) -> Result<ObjectRef, IndexError> {
    let span = crate::build::begin_build_stage(
        context.budget.request().diagnostic_context(),
        quanta_index_core::DiagnosticStageV1::SemanticSeal,
    );
    let result = (|| -> Result<ObjectRef, IndexError> {
        validate_inputs(files, &seal)?;
        let _metadata_memory = context
            .budget
            .reserve_memory(MAX_INSTALL_METADATA_MEMORY_BYTES, MemoryLifetime::Temporary)?;
        let mut manifest = AnnFileSet {
            format: FILE_SET_FORMAT,
            recipe: seal.recipe,
            seal,
            total_length: 0,
            files: Vec::new(),
        };
        manifest
            .files
            .try_reserve_exact(files.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        // Borrow the caller-owned snapshot until each bounded group installs.
        // Only groups with multiple chunks copy admitted payload bytes.
        let mut pending = [None; MAX_STAGED_CHUNKS];
        let mut pending_len = 0usize;
        let mut pending_bytes = 0usize;
        for file in files {
            context.budget.checkpoint()?;
            let count = file.bytes.len().div_ceil(MAX_CHUNK_BYTES);
            let mut chunks = Vec::new();
            chunks
                .try_reserve_exact(count)
                .map_err(|_error| IndexError::ResourceLimited)?;
            let file_index = manifest.files.len();
            manifest.files.push(AnnFile {
                path: file.path.to_owned(),
                length: u64::try_from(file.bytes.len())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                sha256: Digest([0; 32]),
                chunks,
            });
            let mut hash = Sha256::new();
            for bytes in file.bytes.chunks(MAX_CHUNK_BYTES) {
                context.budget.charge(WorkUsage {
                    bytes: u64::try_from(bytes.len())
                        .map_err(|_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                hash.update(bytes);
                let next_bytes = pending_bytes
                    .checked_add(bytes.len())
                    .ok_or(IndexError::ResourceLimited)?;
                if pending_len == MAX_STAGED_CHUNKS || next_bytes > MAX_CHUNK_BYTES {
                    install_pending_chunks(context, &mut manifest, &mut pending, &mut pending_len)?;
                    pending_bytes = 0;
                }
                let file_entry = manifest.files.get_mut(file_index).ok_or_else(|| {
                    IndexError::BackendViolation("ANN file-set entry missing".into())
                })?;
                let chunk_index = file_entry.chunks.len();
                file_entry.chunks.push(AnnChunk {
                    reference: ObjectRef(Digest([0; 32])),
                    length: u32::try_from(bytes.len())
                        .map_err(|_error| IndexError::ResourceLimited)?,
                });
                *pending
                    .get_mut(pending_len)
                    .ok_or(IndexError::ResourceLimited)? = Some(PendingChunk {
                    file: file_index,
                    chunk: chunk_index,
                    bytes,
                });
                pending_len = pending_len
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                pending_bytes = pending_bytes
                    .checked_add(bytes.len())
                    .ok_or(IndexError::ResourceLimited)?;
            }
            let length =
                u64::try_from(file.bytes.len()).map_err(|_error| IndexError::ResourceLimited)?;
            manifest.total_length = manifest
                .total_length
                .checked_add(length)
                .ok_or(IndexError::ResourceLimited)?;
            manifest
                .files
                .get_mut(file_index)
                .ok_or_else(|| IndexError::BackendViolation("ANN file-set entry missing".into()))?
                .sha256 = Digest(hash.finalize().into());
        }
        let final_batch = prepare_final_chunk_batch(
            context,
            &mut manifest,
            &pending,
            pending_len,
            pending_bytes,
        )?;
        if final_batch.is_none() {
            install_pending_chunks(context, &mut manifest, &mut pending, &mut pending_len)?;
        }
        validate_manifest(&manifest).map_err(invalid)?;
        // Resolve fallible observation fields before the descriptor is written.
        let observation = if span.is_some() {
            let files = u32::try_from(manifest.files.len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            let chunks = manifest.files.iter().try_fold(0u32, |count, file| {
                count
                    .checked_add(
                        u32::try_from(file.chunks.len())
                            .map_err(|_error| IndexError::ResourceLimited)?,
                    )
                    .ok_or(IndexError::ResourceLimited)
            })?;
            Some((
                manifest
                    .seal
                    .generation
                    .checked_sub(1)
                    .ok_or(IndexError::ResourceLimited)?,
                diagnostic_index_kind(&manifest.seal)?,
                files,
                chunks,
            ))
        } else {
            None
        };
        let children = child_union(&manifest)?;
        let payload = encode_manifest(&manifest)?;
        context.budget.charge(WorkUsage {
            bytes: u64::try_from(payload.len()).map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let reference = if let Some(FinalChunkBatch {
            mut staged,
            predicted,
        }) = final_batch
        {
            let descriptor = context
                .objects
                .identify(&payload, &children, context.budget)?
                .reference;
            staged.push(
                StagedPayload::copy(&payload, context.budget)?,
                &children,
                context.budget,
            )?;
            let installed = context.put_batch(staged.inputs())?;
            let count = pending_len
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if installed.len() != count
                || installed
                    .iter()
                    .take(pending_len)
                    .zip(predicted.iter().take(pending_len))
                    .any(|(actual, expected)| Some(*actual) != *expected)
                || installed.get(pending_len) != Some(&descriptor)
            {
                return Err(IndexError::BackendViolation(
                    "ANN final chunk/descriptor batch identity or order differs".into(),
                ));
            }
            context.budget.checkpoint()?;
            descriptor
        } else {
            context.put(&payload, &children)?
        };
        if let (Some(diagnostic), Some(span), Some((ordinal, index_kind, files, chunks))) = (
            context.budget.request().diagnostic_context(),
            span.as_ref(),
            observation,
        ) {
            diagnostic.ann_region_observation(
                span,
                DiagnosticAnnRegionObservationV1 {
                    phase: DiagnosticAnnRegionPhaseV1::Installed,
                    fileset_ref: reference.0.0,
                    descriptor_ref: None,
                    ordinal,
                    rows: manifest.seal.rows,
                    index_kind,
                    files,
                    chunks,
                },
            );
        }
        Ok(reference)
    })();
    crate::build::finish_build_stage(span, result.is_ok());
    result
}

fn load_manifest(
    objects: &dyn ImmutableObjectPort,
    reference: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<LoadedAnnManifest, IndexError> {
    let descriptor = objects.get(reference, budget)?;
    if descriptor.registration.reference != reference
        || descriptor.payload.is_empty()
        || descriptor.payload.len() > MAX_MANIFEST_BYTES
    {
        return Err(corrupt(
            "Native ANN file-set descriptor reference or size is invalid",
        ));
    }
    let metadata_memory = budget.reserve_memory(
        read_metadata_memory_bytes(descriptor.payload.len())?,
        MemoryLifetime::Retained,
    )?;
    budget.charge(WorkUsage {
        bytes: u64::try_from(descriptor.payload.len())
            .map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let manifest: AnnFileSet = serde_json::from_slice(&descriptor.payload)
        .map_err(|error| IndexError::CorruptObject(format!("Native ANN file set: {error}")))?;
    validate_manifest(&manifest).map_err(corrupt)?;
    if encode_manifest(&manifest)? != descriptor.payload {
        return Err(corrupt("Native ANN file-set descriptor is not canonical"));
    }
    let children = child_union(&manifest)?;
    if descriptor.registration.children != children {
        return Err(corrupt("Native ANN file-set child graph mismatch"));
    }
    Ok(LoadedAnnManifest {
        manifest,
        descriptor,
        metadata_memory,
    })
}

/// Authenticate bounded manifest authority for region retirement.
///
/// This does not open or hash physical chunk bodies. Selected reads still require
/// `load_fileset`; this seal alone is not physical-integrity evidence.
pub(super) fn load_fileset_seal(
    objects: &dyn ImmutableObjectPort,
    reference: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<LoadedAnnFileSetSeal, IndexError> {
    let LoadedAnnManifest {
        manifest,
        descriptor,
        metadata_memory,
    } = load_manifest(objects, reference, budget)?;
    Ok(LoadedAnnFileSetSeal {
        seal: manifest.seal,
        _descriptor: descriptor,
        _metadata_memory: metadata_memory,
    })
}

/// Verify a selected immutable descriptor and retain each chunk's original
/// storage grant. A caller may replay the ordered chunks into its admitted
/// filesystem without assembling a second dataset buffer.
pub(super) fn load_fileset(
    objects: &dyn ImmutableObjectPort,
    reference: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<LoadedAnnFileSet, IndexError> {
    let LoadedAnnManifest {
        manifest,
        descriptor,
        metadata_memory,
    } = load_manifest(objects, reference, budget)?;
    let mut loaded = Vec::new();
    loaded
        .try_reserve_exact(manifest.files.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    for file in manifest.files {
        budget.checkpoint()?;
        let mut chunks = Vec::new();
        chunks
            .try_reserve_exact(file.chunks.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut hash = Sha256::new();
        for chunk in file.chunks {
            let object = objects.get(chunk.reference, budget)?;
            if object.registration.reference != chunk.reference
                || !object.registration.children.is_empty()
                || u64::try_from(object.payload.len())
                    .map_err(|_error| IndexError::ResourceLimited)?
                    != u64::from(chunk.length)
            {
                return Err(corrupt("Native ANN file-set chunk binding mismatch"));
            }
            budget.charge(WorkUsage {
                bytes: u64::from(chunk.length),
                ..WorkUsage::default()
            })?;
            hash.update(&object.payload);
            chunks.push(LoadedAnnChunk {
                length: chunk.length,
                object,
            });
        }
        if Digest(hash.finalize().into()) != file.sha256 {
            return Err(corrupt("Native ANN file-set file digest mismatch"));
        }
        loaded.push(LoadedAnnFile {
            path: file.path,
            length: file.length,
            chunks,
        });
    }
    Ok(LoadedAnnFileSet {
        seal: manifest.seal,
        total_length: manifest.total_length,
        files: loaded,
        _descriptor: descriptor,
        _metadata_memory: metadata_memory,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        time::{Duration, Instant},
    };

    use quanta_index_core::index::ports::ImmutableObjectPort;
    use quanta_index_core::index::{
        Digest, IndexError, ObjectBatchInput, ObjectRef, RegisteredObject, RegisteredObjectBatch,
        StoredObject, WorkBudget, WorkLimits, WorkUsage, ports::ObjectWriteSink,
    };
    use sha2::{Digest as _, Sha256};

    use super::super::ann_lance::RegionSeal;
    use super::{
        AnnChunk, AnnFile, AnnFileSet, FILE_SET_FORMAT, encode_manifest, load_fileset,
        valid_relative_path, validate_manifest,
    };
    use crate::manifest::VectorIndexSealV1;

    const ABC_SHA256: [u8; 32] = [
        0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22,
        0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00,
        0x15, 0xad,
    ];

    fn reference(byte: u8) -> ObjectRef {
        ObjectRef(Digest([byte; 32]))
    }

    fn sample() -> AnnFileSet {
        AnnFileSet {
            format: FILE_SET_FORMAT,
            recipe: Digest([9; 32]),
            seal: RegionSeal {
                generation: 1,
                recipe: Digest([9; 32]),
                dimension: 2,
                rows: 1,
                rows_digest: Digest([8; 32]),
                vector_index: VectorIndexSealV1 {
                    mode: "exact".into(),
                    library: "lancedb".into(),
                    library_version: "test".into(),
                    index_min_rows: 2,
                    ann: None,
                },
            },
            total_length: 3,
            files: vec![AnnFile {
                path: "data/part-1.lance".into(),
                length: 3,
                sha256: Digest(ABC_SHA256),
                chunks: vec![AnnChunk {
                    reference: reference(1),
                    length: 3,
                }],
            }],
        }
    }

    fn stored(reference: ObjectRef, bytes: Vec<u8>, children: Vec<ObjectRef>) -> StoredObject {
        StoredObject::new(
            RegisteredObject::unscoped(
                reference,
                u64::try_from(bytes.len()).expect("test bytes fit"),
                children,
            ),
            bytes,
            None,
        )
    }

    struct ReadStore(BTreeMap<ObjectRef, StoredObject>);

    impl ImmutableObjectPort for ReadStore {
        fn identify(
            &self,
            _payload: &[u8],
            _children: &[ObjectRef],
            _budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("read-only test store".into()))
        }

        fn put(
            &self,
            _payload: &[u8],
            _children: &[ObjectRef],
            _budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("read-only test store".into()))
        }

        fn install(
            &self,
            _payload: &[u8],
            _children: &[ObjectRef],
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("read-only test store".into()))
        }

        fn install_batch(
            &self,
            _inputs: &[ObjectBatchInput],
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<RegisteredObjectBatch, IndexError> {
            Err(IndexError::BackendViolation("read-only test store".into()))
        }

        fn get(
            &self,
            reference: ObjectRef,
            budget: &mut WorkBudget,
        ) -> Result<StoredObject, IndexError> {
            let object = self
                .0
                .get(&reference)
                .ok_or(IndexError::SnapshotUnavailable)?;
            budget.charge(WorkUsage {
                reads: 1,
                bytes: u64::try_from(object.payload.len())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            Ok(object.clone())
        }
    }

    fn budget() -> WorkBudget {
        WorkBudget::new(
            WorkLimits {
                reads: 16,
                writes: 0,
                bytes: 1_000_000,
                mutations: 0,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .expect("test deadline fits"),
        )
    }

    #[test]
    fn selected_file_set_replays_fixed_bytes_and_refuses_foreign_children() {
        let fixed_sha256: [u8; 32] = Sha256::digest(b"abc").into();
        assert_eq!(fixed_sha256, ABC_SHA256);
        let manifest = sample();
        assert!(validate_manifest(&manifest).is_ok());
        let payload = encode_manifest(&manifest).expect("bounded manifest");
        let descriptor = reference(2);
        let mut objects = BTreeMap::new();
        let _previous = objects.insert(
            descriptor,
            stored(descriptor, payload.clone(), vec![reference(1)]),
        );
        let _previous = objects.insert(reference(1), stored(reference(1), b"abc".to_vec(), vec![]));
        let selected = load_fileset(&ReadStore(objects.clone()), descriptor, &mut budget())
            .expect("selected file set");
        assert_eq!(selected.total_length, 3);
        let first_file = selected.files.first().expect("selected file");
        assert_eq!(first_file.path, "data/part-1.lance");
        let first_chunk = first_file.chunks.first().expect("selected chunk");
        assert_eq!(first_chunk.object.payload.as_slice(), b"abc");

        let _previous = objects.insert(descriptor, stored(descriptor, payload, vec![]));
        assert!(matches!(
            load_fileset(&ReadStore(objects), descriptor, &mut budget()),
            Err(IndexError::CorruptObject(_))
        ));
    }

    #[test]
    fn selected_file_set_refuses_missing_or_old_chunk() {
        let mut manifest = sample();
        let descriptor = reference(2);
        let mut objects = BTreeMap::new();
        let _previous = objects.insert(
            descriptor,
            stored(
                descriptor,
                encode_manifest(&manifest).expect("bounded manifest"),
                vec![reference(1)],
            ),
        );
        assert!(matches!(
            load_fileset(&ReadStore(objects.clone()), descriptor, &mut budget()),
            Err(IndexError::SnapshotUnavailable)
        ));

        manifest
            .files
            .first_mut()
            .expect("sample file")
            .chunks
            .first_mut()
            .expect("sample chunk")
            .reference = reference(3);
        let _previous = objects.insert(
            descriptor,
            stored(
                descriptor,
                encode_manifest(&manifest).expect("bounded manifest"),
                vec![reference(3)],
            ),
        );
        let _previous = objects.insert(reference(3), stored(reference(3), b"old".to_vec(), vec![]));
        assert!(matches!(
            load_fileset(&ReadStore(objects), descriptor, &mut budget()),
            Err(IndexError::CorruptObject(_))
        ));
    }

    #[test]
    fn descriptor_refuses_noncanonical_paths_lengths_and_recipe() {
        for path in ["/abs", "a/../b", "a//b", "a/./b", "C:/a", "a\\b", "a/é"] {
            assert!(!valid_relative_path(path), "{path}");
        }
        let mut manifest = sample();
        manifest.recipe = Digest([7; 32]);
        assert!(validate_manifest(&manifest).is_err());
        manifest.recipe = manifest.seal.recipe;
        manifest.files.first_mut().expect("sample file").length = 2;
        assert!(validate_manifest(&manifest).is_err());
        manifest.files.first_mut().expect("sample file").length = 3;
        let duplicate = manifest.files.first().expect("sample file").clone();
        manifest.files.push(duplicate);
        assert!(validate_manifest(&manifest).is_err());
    }
}
