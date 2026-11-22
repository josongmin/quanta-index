//! Existing committed bytes only. This opens no serving engine or publisher.
use super::{
    coverage_root,
    errors::sidecar_corrupt,
    file_root::AuthorityRoot,
    manifest::LexicalSealedManifest,
    names,
    native_root::NativeRoot,
    object_binary,
    object_closure::{self, ClosureControl},
    object_codec::ObjectRef,
    overlay_names::OverlayFamily,
};
use quanta_index_contract::{GenerationSnapshot, SearchPlaneErrorCodeV2, SearchPlaneTrackKind};
use quanta_index_core::{
    CoreError, ExistingTreeEntryKindV1, ExistingTreeMetadataIoPortV1,
    index::{
        IndexError, MemoryLease, MemoryLifetime, ObjectReadIoPort, WorkBudget, WorkUsage,
        existing_file::{
            ExistingFileBytesV1, ExistingFileReadFailureV1 as Failure,
            read_existing_committed_file_v1, read_existing_file_v1,
            read_existing_file_with_oversize_error_v1, verify_existing_committed_files_v1,
        },
    },
};
use std::{mem::size_of, path::Path};

const FILE_AUTHORITY_ROOT_DIR: &str = "file-authority";
const TEXT_AUTHORITY_ROOT_DIR: &str = "text-authority";

fn memory(budget: &WorkBudget, bytes: u64) -> Result<MemoryLease, Failure> {
    budget
        .reserve_memory(bytes, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited.into())
}
fn width(n: usize) -> Result<u64, Failure> {
    u64::try_from(n).map_err(|_| IndexError::ResourceLimited.into())
}
fn corrupt(root: &Path, name: &str, reason: &str) -> Failure {
    sidecar_corrupt(root, name, reason).into()
}
fn manifest_missing(generation: &Path) -> Failure {
    let path = generation.join(super::manifest::LEXICAL_SEALED_MANIFEST_FILE_NAME);
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::GenerationManifestMissing,
        message: format!(
            "lexical: sealed generation has no content manifest at {}; it predates the sealed-manifest format and must be rebuilt from its producer",
            path.display()
        ),
    }
    .into()
}

#[derive(Clone, Copy, Default)]
struct GenerationLayout {
    manifest: Option<ExistingTreeEntryKindV1>,
    file_authority: Option<ExistingTreeEntryKindV1>,
    text_authority: Option<ExistingTreeEntryKindV1>,
}

fn inspect_generation_layout(
    generation: &Path,
    metadata: &dyn ExistingTreeMetadataIoPortV1,
    budget: &mut WorkBudget,
) -> Result<GenerationLayout, Failure> {
    let mut layout = GenerationLayout::default();
    metadata.visit_directory_v1(generation, budget, &mut |name, kind, _bytes, _budget| {
        if name == names::LEXICAL_QUARANTINE_RECEIPT_FILE_NAME {
            return Err(IndexError::RebuildRequired.into());
        }
        if name == super::manifest::LEXICAL_SEALED_MANIFEST_FILE_NAME {
            if kind != ExistingTreeEntryKindV1::File {
                return Err(corrupt(
                    generation,
                    super::manifest::LEXICAL_SEALED_MANIFEST_FILE_NAME,
                    "is not a regular file within its generation",
                ));
            }
            layout.manifest = Some(kind);
        } else if name == FILE_AUTHORITY_ROOT_DIR {
            if kind != ExistingTreeEntryKindV1::Directory {
                return Err(corrupt(
                    generation,
                    FILE_AUTHORITY_ROOT_DIR,
                    "is not a directory within its generation",
                ));
            }
            layout.file_authority = Some(kind);
        } else if name == TEXT_AUTHORITY_ROOT_DIR {
            if kind != ExistingTreeEntryKindV1::Directory {
                return Err(corrupt(
                    generation,
                    TEXT_AUTHORITY_ROOT_DIR,
                    "is not a directory within its generation",
                ));
            }
            layout.text_authority = Some(kind);
        }
        Ok(())
    })?;
    Ok(layout)
}

fn validate_generation_layout(generation: &Path, layout: GenerationLayout) -> Result<(), Failure> {
    match layout.manifest {
        Some(ExistingTreeEntryKindV1::File) => {}
        None => return Err(manifest_missing(generation)),
        Some(_) => {
            return Err(corrupt(
                generation,
                super::manifest::LEXICAL_SEALED_MANIFEST_FILE_NAME,
                "is not a regular file within its generation",
            ));
        }
    }
    match layout.file_authority {
        Some(ExistingTreeEntryKindV1::Directory) => {}
        Some(_) => {
            return Err(corrupt(
                generation,
                FILE_AUTHORITY_ROOT_DIR,
                "is not a directory within its generation",
            ));
        }
        None => {
            return Err(corrupt(
                generation,
                FILE_AUTHORITY_ROOT_DIR,
                "directory is missing from its generation",
            ));
        }
    }
    match layout.text_authority {
        Some(ExistingTreeEntryKindV1::Directory) => {}
        Some(_) => {
            return Err(corrupt(
                generation,
                TEXT_AUTHORITY_ROOT_DIR,
                "is not a directory within its generation",
            ));
        }
        None => {
            return Err(corrupt(
                generation,
                TEXT_AUTHORITY_ROOT_DIR,
                "directory is missing from its generation",
            ));
        }
    }
    Ok(())
}

fn validate_authority_root_entry(
    generation: &Path,
    directory: &str,
    file: &str,
    commitment_name: &str,
    metadata: &dyn ExistingTreeMetadataIoPortV1,
    budget: &mut WorkBudget,
) -> Result<(), Failure> {
    let mut present = false;
    metadata.visit_directory_v1(
        &generation.join(directory),
        budget,
        &mut |name, kind, _bytes, _budget| {
            if name != file {
                return Ok(());
            }
            if kind != ExistingTreeEntryKindV1::File {
                return Err(corrupt(
                    generation,
                    commitment_name,
                    "is not a regular file within its authority directory",
                ));
            }
            present = true;
            Ok(())
        },
    )?;
    if !present {
        return Err(corrupt(
            generation,
            commitment_name,
            "is missing from its authority directory",
        ));
    }
    Ok(())
}
pub(super) fn decoder_memory(
    bytes: usize,
    budget: &mut WorkBudget,
) -> Result<MemoryLease, Failure> {
    // Typed compact roots can encounter one cautious 1MiB Vec preallocation
    // before a malformed field fails. Prior completed fields are bounded by
    // actual input. Cover that transient, ordinary typed/round-trip copies and
    // fixed control state before entering the canonical decoder. Not RSS.
    let grant = memory(
        budget,
        width(bytes)?
            .checked_mul(128)
            .and_then(|n| n.checked_add(2 << 20))
            .ok_or(IndexError::ResourceLimited)?,
    )?;
    budget.charge(WorkUsage {
        bytes: width(bytes)?
            .checked_mul(32)
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    Ok(grant)
}
struct Dependencies {
    values: Vec<ObjectRef>,
    _decoded: MemoryLease,
    _input: ExistingFileBytesV1,
}
impl AsRef<[ObjectRef]> for Dependencies {
    fn as_ref(&self) -> &[ObjectRef] {
        &self.values
    }
}
struct AdmittedGraph<'a> {
    root: &'a Path,
    owner: u32,
    io: &'a dyn ObjectReadIoPort,
    budget: &'a mut WorkBudget,
}
impl ClosureControl for AdmittedGraph<'_> {
    type Error = Failure;
    type Grant = MemoryLease;
    type Dependencies = Dependencies;
    fn checkpoint(&mut self) -> Result<(), Failure> {
        Ok(self.budget.checkpoint()?)
    }
    fn reserve(&mut self, bytes: u64) -> Result<MemoryLease, Failure> {
        memory(self.budget, bytes)
    }
    fn dependencies(&mut self, reference: &ObjectRef) -> Result<Dependencies, Failure> {
        if reference.bytes > width(object_binary::MAX_OBJECT_BYTES)? {
            return Err(
                object_binary::corrupt("object reference exceeds encoded byte bound").into(),
            );
        }
        let _paths = memory(
            self.budget,
            width(self.root.as_os_str().len())?
                .checked_add(512)
                .ok_or(IndexError::ResourceLimited)?,
        )?;
        let hex = object_binary::hex_digest(&reference.sha256);
        let prefix = hex.get(..2).ok_or(IndexError::DigestConflict)?;
        let artifact = quanta_index_core::SealedArtifactCommitmentV1 {
            name: format!("{prefix}/{hex}"),
            bytes: reference.bytes,
            sha256: reference.sha256,
        };
        let input = read_existing_committed_file_v1(
            self.root,
            &artifact,
            self.owner,
            self.io,
            self.budget,
        )?;
        // The binary codec itself checks both fixed maxima before allocating.
        // Admit its maximum bounded dependency Vec and payload separately from
        // the returned input lease; this is not a whole-corpus row prepayment.
        let decoded = memory(
            self.budget,
            width(
                std::alloc::Layout::array::<ObjectRef>(object_binary::MAX_OBJECT_DEPS)
                    .map_err(|_| IndexError::ResourceLimited)?
                    .size(),
            )?
            .checked_add(width(object_binary::MAX_OBJECT_PAYLOAD)?)
            .ok_or(IndexError::ResourceLimited)?,
        )?;
        self.budget.charge(WorkUsage {
            bytes: width(input.bytes_v1().len())?,
            ..Default::default()
        })?;
        let (kind, values, payload) = object_binary::decode_object(input.bytes_v1())?;
        if kind != reference.kind {
            return Err(object_binary::corrupt("object kind differs from reference").into());
        }
        drop(payload);
        Ok(Dependencies {
            values,
            _decoded: decoded,
            _input: input,
        })
    }
    fn visit(&mut self, _reference: &ObjectRef) -> Result<(), Failure> {
        Ok(())
    }
}

/// Authenticate the current canonical lexical control rows, all outer committed
/// bytes, every reachable pooled object and all committed coverage pages.
///
/// `lexical_track_root` is the actual indexes/lexical root; `generation` is its
/// existing generation path. Caller IO must carry that same genuinely inventoried
/// Legacy root, with original WorkBudget/memory and root lease held through its
/// transaction. `maximum_objects` is its actual observed-file bound, not a grant.
///
/// This proves byte custody and canonical control shape. It does not recompute
/// Tantivy/live-schema/query results, Source parity, installation or publication
/// permission. The caller must check actual physical retirement before publish.
pub fn verify_existing_lexical_generation_read_only(
    lexical_track_root: &Path,
    expected: &GenerationSnapshot,
    owner: u32,
    maximum_objects: usize,
    metadata: &dyn ExistingTreeMetadataIoPortV1,
    io: &dyn ObjectReadIoPort,
    budget: &mut WorkBudget,
) -> Result<(), Failure> {
    budget.checkpoint()?;
    if expected.track != SearchPlaneTrackKind::Lexical {
        return Err(IndexError::DigestConflict.into());
    }
    let _paths = memory(
        budget,
        width(lexical_track_root.as_os_str().len())?
            .checked_mul(64)
            .and_then(|n| n.checked_add(65536))
            .ok_or(IndexError::ResourceLimited)?,
    )?;
    budget.charge(WorkUsage {
        bytes: width(expected.repo_id.as_str().len())?
            .checked_add(width(expected.revision_id.as_str().len())?)
            .and_then(|n| n.checked_add(64))
            .ok_or(IndexError::ResourceLimited)?,
        ..Default::default()
    })?;
    let generation = quanta_index_core::GenerationStorageKeyV1::for_repo_revision(
        &expected.repo_id,
        &expected.revision_id,
    )
    .generation_dir(lexical_track_root, expected.manifest_generation);
    let generation = generation.as_path();
    let identity_path = generation.join(names::LEXICAL_SEALED_IDENTITY_FILE_NAME);
    let manifest_path = generation.join(super::manifest::LEXICAL_SEALED_MANIFEST_FILE_NAME);
    let identity = read_existing_file_v1(
        &identity_path,
        owner,
        width(names::MAX_SEALED_IDENTITY_BYTES)?,
        io,
        budget,
    )?;
    let observed = crate::channel_payloads::decode_cbor_exact(identity.bytes_v1())
        .map_err(|error| corrupt(generation, names::LEXICAL_SEALED_IDENTITY_FILE_NAME, &error))?;
    super::identity::validate_lexical_sealed_identity(&observed, expected)?;
    let input = read_existing_file_with_oversize_error_v1(
        &manifest_path,
        owner,
        width(super::manifest::MAX_SEALED_MANIFEST_BYTES)?,
        io,
        budget,
        |path, length, limit| {
            Failure::Canonical(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
                message: format!(
                    "lexical: sealed generation manifest {} is {length} bytes, above the {limit}-byte limit; the generation must be rebuilt",
                    path.display()
                ),
            })
        },
    );
    let input = match input {
        Ok(input) => input,
        Err(
            error @ Failure::Canonical(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
                ..
            }),
        ) => return Err(error),
        Err(error) => {
            let layout = inspect_generation_layout(generation, metadata, budget)?;
            return match layout.manifest {
                None => Err(manifest_missing(generation)),
                Some(ExistingTreeEntryKindV1::File) => Err(error),
                Some(_) => Err(corrupt(
                    generation,
                    super::manifest::LEXICAL_SEALED_MANIFEST_FILE_NAME,
                    "is not a regular file within its generation",
                )),
            };
        }
    };
    // Canonical preflight bounds complete Value members before construction.
    // This conservative actual-byte envelope covers their nodes, bounded row
    // vectors and typed copies; it grants no new resource or dataset limit.
    let _manifest_decoder = memory(
        budget,
        width(input.bytes_v1().len())?
            .checked_mul(512)
            .and_then(|n| n.checked_add(1 << 20))
            .ok_or(IndexError::ResourceLimited)?,
    )?;
    let manifest = LexicalSealedManifest::decode(input.bytes_v1(), &manifest_path)?;
    if manifest.manifest_digest != expected.manifest_digest {
        return Err(corrupt(
            generation,
            super::manifest::LEXICAL_SEALED_MANIFEST_FILE_NAME,
            "manifest digest differs from sealed identity",
        ));
    }
    validate_generation_layout(
        generation,
        inspect_generation_layout(generation, metadata, budget)?,
    )?;
    validate_authority_root_entry(
        generation,
        FILE_AUTHORITY_ROOT_DIR,
        "root.cbor",
        crate::lexical_root_names::FILE_ROOT_NAME,
        metadata,
        budget,
    )?;
    validate_authority_root_entry(
        generation,
        TEXT_AUTHORITY_ROOT_DIR,
        "manifest.cbor",
        crate::lexical_root_names::NATIVE_ROOT_NAME,
        metadata,
        budget,
    )?;
    // The canonical manifest commits index-meta referenced files, not every
    // directory entry. Old unreferenced segment leftovers are not extra files.
    for artifact in manifest.all_commitments().filter(|a| {
        OverlayFamily::from_file_name(&a.name).is_none()
            && a.name != crate::lexical_root_names::FILE_ROOT_NAME
            && a.name != crate::lexical_root_names::NATIVE_ROOT_NAME
            && a.name != names::SOURCE_FILE_COVERAGE_FILE_NAME
    }) {
        verify_existing_committed_files_v1(
            generation,
            "",
            std::slice::from_ref(artifact),
            owner,
            io,
            budget,
        )?;
    }
    for (_, artifact) in manifest.overlay_commitments() {
        verify_existing_committed_files_v1(
            generation,
            "",
            std::slice::from_ref(artifact),
            owner,
            io,
            budget,
        )?;
    }
    let file = manifest
        .file_authority
        .first()
        .ok_or(IndexError::DigestConflict)?;
    let native = manifest
        .text_authority
        .as_ref()
        .and_then(|files| files.first())
        .ok_or(IndexError::DigestConflict)?;
    let file_input = read_existing_committed_file_v1(generation, file, owner, io, budget)?;
    let native_input = read_existing_committed_file_v1(generation, native, owner, io, budget)?;
    let _file_decoder = decoder_memory(file_input.bytes_v1().len(), budget)?;
    let _native_decoder = decoder_memory(native_input.bytes_v1().len(), budget)?;
    let file_root = AuthorityRoot::decode(file_input.bytes_v1())?;
    let native_root: NativeRoot =
        crate::channel_payloads::decode_cbor_exact(native_input.bytes_v1())
            .map_err(|error| corrupt(generation, &native.name, &error))?;
    native_root.validate()?;
    if crate::channel_payloads::encode_cbor(&native_root, "Native root")? != native_input.bytes_v1()
    {
        return Err(corrupt(generation, &native.name, "noncanonical root"));
    }
    // Fixed compact roots contain at most 12+9 map descriptors. Their small
    // root-reference Vec growth is funded before either canonical helper runs.
    let _root_refs = memory(
        budget,
        width(size_of::<ObjectRef>())?
            .checked_mul(64)
            .ok_or(IndexError::ResourceLimited)?,
    )?;
    let mut roots = file_root.object_roots();
    let native_roots = native_root.root_objects();
    roots
        .try_reserve_exact(native_roots.len())
        .map_err(|_| IndexError::ResourceLimited)?;
    roots.extend(native_roots);
    let pool = lexical_track_root
        .join(object_binary::POOL_DIR)
        .join(object_binary::OBJECTS_DIR);
    object_closure::walk(
        &roots,
        maximum_objects,
        &mut AdmittedGraph {
            root: &pool,
            owner,
            io,
            budget,
        },
    )?;
    if let Some(coverage) = manifest.source_coverage.as_ref() {
        let coverage_input =
            read_existing_committed_file_v1(generation, coverage, owner, io, budget)?;
        let _decoder = decoder_memory(coverage_input.bytes_v1().len(), budget)?;
        let expected_pages =
            coverage_root::page_commitments(coverage_input.bytes_v1(), generation, expected)?;
        check_directory(
            generation,
            &manifest,
            Some(&expected_pages),
            metadata,
            budget,
        )?;
        for page in &expected_pages {
            verify_existing_committed_files_v1(
                generation,
                "",
                std::slice::from_ref(page),
                owner,
                io,
                budget,
            )?;
        }
        let coverage_after =
            read_existing_committed_file_v1(generation, coverage, owner, io, budget)?;
        if coverage_after.bytes_v1() != coverage_input.bytes_v1() {
            return Err(IndexError::DigestConflict.into());
        }
        check_directory(
            generation,
            &manifest,
            Some(&expected_pages),
            metadata,
            budget,
        )?;
    } else {
        check_directory(generation, &manifest, None, metadata, budget)?;
    }
    let file_after = read_existing_committed_file_v1(generation, file, owner, io, budget)?;
    let native_after = read_existing_committed_file_v1(generation, native, owner, io, budget)?;
    if file_after.bytes_v1() != file_input.bytes_v1()
        || native_after.bytes_v1() != native_input.bytes_v1()
    {
        return Err(IndexError::DigestConflict.into());
    }
    drop((file_after, native_after));
    // Revalidate controls after the streamed content walk. No repair/quarantine
    // write is performed, and raw path opens never replace the supplied port.
    let after = read_existing_file_v1(
        &identity_path,
        owner,
        width(names::MAX_SEALED_IDENTITY_BYTES)?,
        io,
        budget,
    )?;
    if after.bytes_v1() != identity.bytes_v1() {
        return Err(IndexError::DigestConflict.into());
    }
    let after = read_existing_file_v1(
        &manifest_path,
        owner,
        width(super::manifest::MAX_SEALED_MANIFEST_BYTES)?,
        io,
        budget,
    )?;
    if after.bytes_v1() != input.bytes_v1() {
        return Err(IndexError::DigestConflict.into());
    }
    budget.checkpoint()?;
    Ok(())
}
fn check_directory(
    root: &Path,
    manifest: &LexicalSealedManifest,
    coverage: Option<&[quanta_index_core::SealedArtifactCommitmentV1]>,
    metadata: &dyn ExistingTreeMetadataIoPortV1,
    budget: &mut WorkBudget,
) -> Result<(), Failure> {
    let mut has_file_authority = false;
    let mut has_text_authority = false;
    metadata.visit_directory_v1(root, budget, &mut |name, kind, _bytes, budget| {
        budget.checkpoint()?;
        if name == names::LEXICAL_QUARANTINE_RECEIPT_FILE_NAME {
            return Err(IndexError::RebuildRequired.into());
        }
        if name == FILE_AUTHORITY_ROOT_DIR {
            if kind != ExistingTreeEntryKindV1::Directory {
                return Err(corrupt(
                    root,
                    FILE_AUTHORITY_ROOT_DIR,
                    "is not a directory within its generation",
                ));
            }
            has_file_authority = true;
        } else if name == TEXT_AUTHORITY_ROOT_DIR {
            if kind != ExistingTreeEntryKindV1::Directory {
                return Err(corrupt(
                    root,
                    TEXT_AUTHORITY_ROOT_DIR,
                    "is not a directory within its generation",
                ));
            }
            has_text_authority = true;
        }
        let entry = name.as_encoded_bytes();
        if entry.starts_with(names::RANKED_KEYS_PREFIX.as_bytes())
            && entry.ends_with(names::RANKED_KEYS_SUFFIX.as_bytes())
            && !name.to_str().is_some_and(|name| {
                manifest
                    .ranked_keys
                    .binary_search_by(|artifact| artifact.name.as_str().cmp(name))
                    .is_ok()
            })
        {
            return Err(corrupt(
                root,
                &name.to_string_lossy(),
                "uncommitted ranked-key file",
            ));
        }
        let Some(text) = name.to_str() else {
            if name
                .as_encoded_bytes()
                .starts_with(coverage_root::PAGE_PREFIX.as_bytes())
            {
                return Err(corrupt(
                    root,
                    names::SOURCE_FILE_COVERAGE_FILE_NAME,
                    "non-UTF8 uncommitted coverage page",
                ));
            }
            return Ok(());
        };
        if let Some(family) = OverlayFamily::from_file_name(text)
            && manifest.overlay(family).is_none()
        {
            return Err(corrupt(root, text, "uncommitted overlay"));
        }
        if name == names::SOURCE_FILE_COVERAGE_FILE_NAME && coverage.is_none() {
            return Err(corrupt(
                root,
                text,
                "coverage exists without a manifest commitment",
            ));
        }
        if coverage_root::is_coverage_page(text)
            && !coverage.is_some_and(|pages| pages.iter().any(|page| page.name == text))
        {
            return Err(corrupt(root, text, "uncommitted coverage page"));
        }
        Ok(())
    })?;
    if !has_file_authority {
        return Err(corrupt(
            root,
            FILE_AUTHORITY_ROOT_DIR,
            "directory is missing from its generation",
        ));
    }
    if !has_text_authority {
        return Err(corrupt(
            root,
            TEXT_AUTHORITY_ROOT_DIR,
            "directory is missing from its generation",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::index::{
        MemoryLeaseOwner, MemoryScope, MemoryScopePort, ObjectReadFile, WorkLimits,
    };
    use std::{
        fs::File,
        os::unix::fs::MetadataExt as _,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };
    // Finite DATA-test allocations only. No actual Legacy root binder, Source
    // actor, FD3–6, installed image, or publication permission is manufactured.
    #[derive(Default)]
    struct Live {
        bytes: AtomicU64,
    }
    impl MemoryLeaseOwner for Live {
        fn release(&self, bytes: u64) {
            let _old = self.bytes.fetch_sub(bytes, Ordering::SeqCst);
        }
    }
    struct Finite(Arc<Live>);
    impl MemoryScopePort for Finite {
        fn reserve(
            &self,
            bytes: u64,
            _lifetime: MemoryLifetime,
        ) -> Result<MemoryLease, IndexError> {
            let _prior_usage = self
                .0
                .bytes
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |old| {
                    old.checked_add(bytes).filter(|next| *next <= 8 << 20)
                })
                .map_err(|_| IndexError::ResourceLimited)?;
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }
    #[derive(Debug, Default)]
    struct DataIo {
        opens: AtomicU64,
    }
    struct DataFile {
        file: File,
        _memory: MemoryLease,
    }
    impl ObjectReadFile for DataFile {
        fn file(&self) -> Result<&File, IndexError> {
            Ok(&self.file)
        }
    }
    impl ObjectReadIoPort for DataIo {
        fn open(
            &self,
            path: &Path,
            budget: &mut WorkBudget,
        ) -> Result<Box<dyn ObjectReadFile>, IndexError> {
            let memory = budget
                .reserve_memory(size_of::<DataFile>() as u64, MemoryLifetime::Temporary)?
                .ok_or(IndexError::ResourceLimited)?;
            let _old = self.opens.fetch_add(1, Ordering::SeqCst);
            let file = File::open(path).map_err(|e| IndexError::Storage(e.to_string()))?;
            Ok(Box::new(DataFile {
                file,
                _memory: memory,
            }))
        }
    }
    fn work(funded: bool) -> Result<(WorkBudget, Arc<Live>), Box<dyn std::error::Error>> {
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 10_000,
                writes: 0,
                bytes: 64 << 20,
                mutations: 0,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or("test deadline overflow")?,
        );
        let live = Arc::new(Live::default());
        if funded {
            budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(Finite(live.clone())))));
        }
        Ok((budget, live))
    }
    fn golden_ref() -> ObjectRef {
        ObjectRef {
            kind: super::super::object_codec::ObjectKind::NativeBody,
            bytes: 24,
            sha256: [
                0x7d, 0xd7, 0x42, 0x79, 0xfa, 0xf1, 0xc9, 0xc4, 0x0c, 0x50, 0x56, 0x1f, 0xdd, 0x0d,
                0xa5, 0xae, 0x48, 0xf1, 0xf4, 0x41, 0x60, 0xe1, 0xad, 0xbc, 0xc2, 0xbc, 0x28, 0x4b,
                0x9e, 0x9e, 0x8d, 0x16,
            ],
        }
    }
    #[test]
    fn lexical_existing_pool_fixed_golden_deduplicates_and_refuses_rewrite_and_kind()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let reference = golden_ref();
        let name = object_binary::hex_digest(&reference.sha256);
        let shard = root.path().join(name.get(..2).ok_or("prefix absent")?);
        std::fs::create_dir(&shard)?;
        let path = shard.join(&name);
        let bytes = b"QLSTOR01\x00\x01\x03\x00\x00\x00\x00\x00\x00\x00\x00\x00\x03abc";
        std::fs::write(&path, bytes)?;
        let owner = std::fs::metadata(&path)?.uid();
        let io = DataIo::default();
        let (mut budget, live) = work(true)?;
        object_closure::walk(
            &[reference.clone(), reference.clone()],
            1,
            &mut AdmittedGraph {
                root: root.path(),
                owner,
                io: &io,
                budget: &mut budget,
            },
        )?;
        // One actual read + its same-port post-hash reopen, despite two edges.
        assert_eq!(io.opens.load(Ordering::SeqCst), 2);
        assert_eq!(live.bytes.load(Ordering::SeqCst), 0);
        let mut corrupt_bytes = bytes.to_vec();
        *corrupt_bytes.last_mut().ok_or("empty golden")? = b'd';
        std::fs::write(&path, &corrupt_bytes)?;
        assert!(
            object_closure::walk(
                std::slice::from_ref(&reference),
                1,
                &mut AdmittedGraph {
                    root: root.path(),
                    owner,
                    io: &io,
                    budget: &mut budget
                }
            )
            .is_err()
        );
        std::fs::write(&path, bytes)?;
        let mut wrong = reference;
        wrong.kind = super::super::object_codec::ObjectKind::FileBody;
        assert!(
            object_closure::walk(
                &[wrong],
                1,
                &mut AdmittedGraph {
                    root: root.path(),
                    owner,
                    io: &io,
                    budget: &mut budget
                }
            )
            .is_err()
        );
        assert_eq!(live.bytes.load(Ordering::SeqCst), 0);
        assert_eq!(budget.usage().writes, 0);
        assert_eq!(budget.usage().mutations, 0);
        Ok(())
    }
    #[test]
    fn lexical_existing_pool_requires_original_memory_before_open()
    -> Result<(), Box<dyn std::error::Error>> {
        let (mut budget, _live) = work(false)?;
        let io = DataIo::default();
        let result = object_closure::walk(
            &[golden_ref()],
            1,
            &mut AdmittedGraph {
                root: Path::new("never-opened"),
                owner: 0,
                io: &io,
                budget: &mut budget,
            },
        );
        assert!(matches!(
            result,
            Err(Failure::AdmissionOrIo(IndexError::ResourceLimited))
        ));
        assert_eq!(io.opens.load(Ordering::SeqCst), 0);
        Ok(())
    }
    fn malformed_compact_root_prefix(native: bool) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        // Fixed wire contracts: Native V8 has13 tuple fields, File V22 has16.
        // Both reach a PageRef's first Vec before any domain validation.
        let mut bytes = vec![
            if native { 0x8d } else { 0x90 },
            if native { 8 } else { 22 },
        ];
        if native {
            let normalizer = crate::normalize::TEXT_NORMALIZER_VERSION;
            ciborium::into_writer(&(normalizer.major, normalizer.minor), &mut bytes)?;
        } else {
            ciborium::into_writer(&[1u8; 32], &mut bytes)?;
        }
        ciborium::into_writer(&[2u8; 32], &mut bytes)?;
        // MapRoot, PageRef, ObjectRef and its fixed32-byte digest.
        bytes.extend_from_slice(&[0x82, 0x86, 0x83, 8, 0x18, 24]);
        ciborium::into_writer(&[3u8; 32], &mut bytes)?;
        // Definite array length u64::MAX, followed by EOF. A scalar tiny
        // input must never turn its header into an unbounded reservation.
        bytes.extend_from_slice(&[0x9b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
        Ok(bytes)
    }
    #[test]
    fn lexical_existing_compact_roots_refuse_huge_vec_headers_under_original_grant()
    -> Result<(), Box<dyn std::error::Error>> {
        for native in [false, true] {
            let bytes = malformed_compact_root_prefix(native)?;
            assert!(bytes.len() < 128);
            let (mut budget, live) = work(true)?;
            {
                let _decoder = decoder_memory(bytes.len(), &mut budget)?;
                assert!(live.bytes.load(Ordering::SeqCst) > 0);
                if native {
                    assert!(
                        crate::channel_payloads::decode_cbor_exact::<NativeRoot>(&bytes).is_err()
                    );
                } else {
                    assert!(AuthorityRoot::decode(&bytes).is_err());
                }
            }
            assert_eq!(live.bytes.load(Ordering::SeqCst), 0);
            assert_eq!(budget.usage().writes, 0);
            assert_eq!(budget.usage().mutations, 0);
        }
        Ok(())
    }
    #[test]
    fn lexical_existing_compact_roots_refuse_exhausted_original_before_decode()
    -> Result<(), Box<dyn std::error::Error>> {
        let bytes = malformed_compact_root_prefix(true)?;
        let (mut budget, live) = work(true)?;
        let occupied = budget
            .reserve_memory(7 << 20, MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited)?;
        assert!(matches!(
            decoder_memory(bytes.len(), &mut budget),
            Err(Failure::AdmissionOrIo(IndexError::ResourceLimited))
        ));
        assert_eq!(live.bytes.load(Ordering::SeqCst), 7 << 20);
        drop(occupied);
        assert_eq!(live.bytes.load(Ordering::SeqCst), 0);
        Ok(())
    }
}
