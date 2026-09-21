//! Immutable content-addressed object store and quarantine projections
//! for sealed `RepoMap` candidates (SEP-21 S21-01B/S21-02, P03).
//!
//! The filesystem holds immutable derived projections only: the `SQLite`
//! catalog is the sole visibility authority. Every object lives at the
//! P01A content address (`objects/sha256/aa/bb/<60hex>.cbor`), written
//! once with create-new/no-clobber semantics and verified on every read
//! through the full chain: raw address digest → lstat/no-follow open /
//! fstat security observations (inode/device, expected uid, exact mode,
//! regular file, `nlink == 1`) → strict canonical CBOR decode → exact
//! re-encode → payload identity/commitment match.
//!
//! Quarantine evidence is projected the same way: the canonical incident
//! envelope at `quarantine/incidents/sha256/aa/bb/<60hex>.cbor` and the
//! readable raw payload bytes at `quarantine/payloads/sha256/aa/bb/
//! <60hex>.bin`, create-new with exact-byte replay and file/directory
//! fsync. The original source object is unlinked only after both
//! projections are durable, and the source directory is fsynced after the
//! unlink.

use std::fs::{self, File, OpenOptions};
use std::io::Read as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest as _, Sha256};

use quanta_index_contract::{
    CandidateObjectDigestV1, CanonicalRepoMapCodecErrorV1, QuarantineIncidentDigestV1,
    QuarantineIncidentV1, QuarantinePayloadDigestV1, QuarantineReasonCodeV1,
    RepoMapCandidateEnvelopeV1, StateRootUuidCommitmentV1,
};
use quanta_index_core::CoreError;

use crate::layout_v3::{
    CandidateObjectAddressV1, ObservedFileKindV1, ObservedFileMetadataV1, SecureMetadataPairV1,
    StateRootSecurityContextV1,
};

/// The root-identity file: 16 bytes bound once at first open.
///
/// The commitment is a layout identity binding, not a secret: it is derived
/// from the root path, the creation instant and the creating process so
/// two distinct roots never share a commitment in practice.
const ROOT_UUID_FILE_NAME: &str = "root-uuid.bin";
const ROOT_UUID_BYTES: usize = 16;

/// Names of the legacy V1 layout this owner refuses to mutate (P10 owns
/// the offline importer for them).
pub(crate) const LEGACY_V1_DIR_NAMES: [&str; 2] = ["activations", "snapshots"];

pub(crate) const CRASH_BOUNDARY_ENV: &str = "QUANTA_INDEX_REPOMAP_CRASH_BOUNDARY";
pub(crate) const CRASH_EXIT_CODE: i32 = 87;
/// The seal protocol's crash points, named for the crash matrix.
pub(crate) const AFTER_OBJECT_SYNC: &str = "after-object-sync";
pub(crate) const AFTER_CATALOG_COMMIT: &str = "after-catalog-commit";

/// The seal protocol's crash-point hook, mirroring the search-plane
/// `QUANTA_INDEX_CRASH_POINT` precedent.
///
/// A no-op unless the environment names this exact boundary, in which case
/// the process stops dead — the crash matrix's subprocess boundary, never a
/// release behavior.
#[expect(
    clippy::exit,
    reason = "the subprocess-only crash matrix must terminate without unwinding, exactly as a crash would"
)]
pub(crate) fn exit_at_crash_boundary(boundary: &str) {
    if std::env::var(CRASH_BOUNDARY_ENV).is_ok_and(|configured| configured == boundary) {
        std::process::exit(CRASH_EXIT_CODE);
    }
}

fn storage_error(action: &str, path: &Path, err: &dyn std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("repomap object store failed to {action} {}: {err}", path.display()))
}

fn typed_refusal(
    code: quanta_index_contract::SearchPlaneErrorCodeV2,
    message: String,
) -> CoreError {
    CoreError::Typed { code, message }
}

/// The immutable object store under one layout-V3 root.
#[derive(Debug)]
pub(crate) struct RepoMapObjectStore {
    root: PathBuf,
    security: StateRootSecurityContextV1,
    uuid_commitment: StateRootUuidCommitmentV1,
}

/// What a seal left durable: the address, the exact canonical bytes, and
/// whether the object already existed (an exact replay).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SealedObjectV1 {
    pub address: CandidateObjectAddressV1,
    pub bytes: Arc<Vec<u8>>,
    pub replayed: bool,
}

/// Why a candidate object could not be verified, with the raw bytes when
/// they were readable at all — the quarantine projection's input.
#[derive(Clone, Debug)]
pub(crate) struct ObjectVerificationFailureV1 {
    pub reason: QuarantineReasonCodeV1,
    pub detail: String,
    pub raw_bytes: Option<Vec<u8>>,
    pub observed_byte_size: Option<u64>,
}

impl RepoMapObjectStore {
    /// Open (creating if needed) the layout-V3 root and its security
    /// context. The effective uid is taken from the root directory itself
    /// (this process created or adopted it); the uuid commitment is bound
    /// once at first open and reused on every later open.
    pub(crate) fn open(root: impl AsRef<Path>) -> Result<Self, CoreError> {
        let root = root.as_ref();
        let root_metadata = match fs::symlink_metadata(root) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(root)
                    .map_err(|err| storage_error("create layout root", root, &err))?;
                fs::symlink_metadata(root)
                    .map_err(|err| storage_error("inspect layout root", root, &err))?
            }
            Err(err) => return Err(storage_error("inspect layout root", root, &err)),
        };
        if root_metadata.is_symlink() {
            return Err(typed_refusal(
                quanta_index_contract::SearchPlaneErrorCodeV2::StateRootInsecure,
                format!("repomap object store refuses a symlinked layout root: {}", root.display()),
            ));
        }
        if !root_metadata.is_dir() {
            return Err(storage_error(
                "open layout root (not a directory)",
                root,
                &"path exists and is not a directory",
            ));
        }
        set_directory_mode(root)?;
        let root_metadata = fs::symlink_metadata(root)
            .map_err(|err| storage_error("inspect layout root", root, &err))?;
        let expected_uid = observed_metadata(root, &root_metadata)
            .map_err(|detail| storage_error("observe layout root", root, &detail))?
            .uid;
        let uuid_commitment = Self::load_or_bind_uuid(root)?;
        let security = StateRootSecurityContextV1::new(expected_uid, uuid_commitment);
        Ok(Self {
            root: root.to_path_buf(),
            security,
            uuid_commitment,
        })
    }

    pub(crate) const fn uuid_commitment(&self) -> StateRootUuidCommitmentV1 {
        self.uuid_commitment
    }

    /// Bind (first open) or load (every later open) the root uuid
    /// commitment. Created with create-new semantics: a concurrent creator
    /// wins and both read the same bytes.
    fn load_or_bind_uuid(root: &Path) -> Result<StateRootUuidCommitmentV1, CoreError> {
        let path = root.join(ROOT_UUID_FILE_NAME);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.is_file() {
                    return Err(storage_error(
                        "inspect root uuid (not a regular file)",
                        &path,
                        &"unexpected kind",
                    ));
                }
                let bytes =
                    fs::read(&path).map_err(|err| storage_error("read root uuid", &path, &err))?;
                let uuid = <[u8; ROOT_UUID_BYTES]>::try_from(bytes.as_slice()).map_err(
                    |_wrong_length| {
                        storage_error(
                            "read root uuid (wrong length)",
                            &path,
                            &format!("{} bytes, expected {ROOT_UUID_BYTES}", bytes.len()),
                        )
                    },
                )?;
                Ok(StateRootUuidCommitmentV1::for_uuid_bytes(uuid))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let uuid = derive_uuid_bytes(root);
                let mut file = open_create_new(&path)?;
                file.write_all(uuid.as_slice())
                    .map_err(|err| storage_error("write root uuid", &path, &err))?;
                file.sync_all()
                    .map_err(|err| storage_error("sync root uuid", &path, &err))?;
                drop(file);
                sync_directory(root)?;
                Ok(StateRootUuidCommitmentV1::for_uuid_bytes(uuid))
            }
            Err(err) => Err(storage_error("inspect root uuid", &path, &err)),
        }
    }

    /// Seal one candidate envelope at its content address, create-new.
    ///
    /// An existing object at the address is an exact-byte replay when its
    /// bytes match (content-addressed: same digest, same bytes) and a
    /// typed corruption refusal when they do not — never an overwrite.
    pub(crate) fn seal(
        &self,
        envelope: &RepoMapCandidateEnvelopeV1,
    ) -> Result<SealedObjectV1, CoreError> {
        let bytes = envelope
            .encode_canonical()
            .map_err(|err| object_codec_error("encode sealed candidate", &err))?;
        let digest = envelope
            .object_digest()
            .map_err(|err| object_codec_error("derive object address", &err))?;
        let address = CandidateObjectAddressV1::new(digest);
        let relative = address.relative_path();
        let final_path = self.root.join(&relative);
        match fs::symlink_metadata(&final_path) {
            Ok(_) => {
                let existing = self.read_raw_verified(&relative)?;
                if existing.as_slice() == bytes.as_slice() {
                    return Ok(SealedObjectV1 {
                        address,
                        bytes: Arc::new(bytes),
                        replayed: true,
                    });
                }
                return Err(typed_refusal(
                    quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                    format!(
                        "repomap object store: an object already sits at {} but its bytes do \
                         not match the address digest",
                        final_path.display()
                    ),
                ));
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(storage_error("inspect object", &final_path, &err)),
        }
        let traversed = self.ensure_fanout_directories(&relative)?;
        let mut file = open_create_new(&final_path)?;
        let written = file
            .write_all(bytes.as_slice())
            .and_then(|()| file.sync_all());
        written.map_err(|err| storage_error("write sealed object", &final_path, &err))?;
        drop(file);
        exit_at_crash_boundary(AFTER_OBJECT_SYNC);
        for directory in traversed.iter().rev() {
            sync_directory(directory)?;
        }
        Ok(SealedObjectV1 {
            address,
            bytes: Arc::new(bytes),
            replayed: false,
        })
    }

    /// Verify one sealed candidate object end to end: security metadata,
    /// strict canonical decode, exact re-encode, address digest and
    /// commitment match against the catalog row.
    pub(crate) fn verify(
        &self,
        digest: CandidateObjectDigestV1,
        expected_commitment: &[u8; 32],
    ) -> Result<RepoMapCandidateEnvelopeV1, ObjectVerificationFailureV1> {
        let address = CandidateObjectAddressV1::new(digest);
        let relative = address.relative_path();
        let raw = self
            .read_raw_verified(&relative)
            .map_err(|err| ObjectVerificationFailureV1 {
                reason: QuarantineReasonCodeV1::SecureIoUnavailable,
                detail: err.to_string(),
                raw_bytes: None,
                observed_byte_size: None,
            })?;
        let raw_bytes = raw.clone();
        #[expect(
            clippy::disallowed_methods,
            reason = "usize-to-u64 cannot fail on supported targets; the Option marks the byte size as observed-or-not"
        )]
        let observed_byte_size = u64::try_from(raw.len()).ok();
        let envelope =
            RepoMapCandidateEnvelopeV1::decode_canonical(raw_bytes.as_slice()).map_err(|err| {
                ObjectVerificationFailureV1 {
                    reason: match err {
                        CanonicalRepoMapCodecErrorV1::TrailingBytes
                        | CanonicalRepoMapCodecErrorV1::NonCanonicalInteger => {
                            QuarantineReasonCodeV1::NonCanonicalEnvelope
                        }
                        CanonicalRepoMapCodecErrorV1::ArtifactBindingMismatch => {
                            QuarantineReasonCodeV1::AddressDigestMismatch
                        }
                        CanonicalRepoMapCodecErrorV1::UnexpectedEnd
                        | CanonicalRepoMapCodecErrorV1::WrongType(_)
                        | CanonicalRepoMapCodecErrorV1::InvalidValue(_)
                        | CanonicalRepoMapCodecErrorV1::InvalidUtf8
                        | CanonicalRepoMapCodecErrorV1::Identity(_)
                        | CanonicalRepoMapCodecErrorV1::EvidenceBindingMismatch
                        | CanonicalRepoMapCodecErrorV1::LengthOutOfRange
                        | CanonicalRepoMapCodecErrorV1::InvalidDigestText => {
                            QuarantineReasonCodeV1::EnvelopeDecodeFailed
                        }
                    },
                    detail: err.to_string(),
                    raw_bytes: Some(raw_bytes.clone()),
                    observed_byte_size,
                }
            })?;
        // `decode_canonical` enforces decode → exact re-encode equality
        // internally, so the payload identity check is the commitment.
        let commitment = envelope
            .commitment()
            .map_err(|err| ObjectVerificationFailureV1 {
                reason: QuarantineReasonCodeV1::AddressDigestMismatch,
                detail: err.to_string(),
                raw_bytes: Some(raw_bytes.clone()),
                observed_byte_size,
            })?;
        if commitment.as_bytes() != expected_commitment {
            return Err(ObjectVerificationFailureV1 {
                reason: QuarantineReasonCodeV1::AddressDigestMismatch,
                detail: format!(
                    "sealed object commitment {commitment} does not match the catalog row commitment"
                ),
                raw_bytes: Some(raw_bytes),
                observed_byte_size,
            });
        }
        let object_digest =
            envelope
                .object_digest()
                .map_err(|err| ObjectVerificationFailureV1 {
                    reason: QuarantineReasonCodeV1::AddressDigestMismatch,
                    detail: err.to_string(),
                    raw_bytes: Some(raw_bytes.clone()),
                    observed_byte_size,
                })?;
        if object_digest != digest {
            return Err(ObjectVerificationFailureV1 {
                reason: QuarantineReasonCodeV1::AddressDigestMismatch,
                detail: format!(
                    "sealed object address digest {digest} does not match its envelope"
                ),
                raw_bytes: Some(raw_bytes),
                observed_byte_size,
            });
        }
        Ok(envelope)
    }

    /// Read the bytes at a content address after full security
    /// verification of every traversed directory and the leaf file.
    fn read_raw_verified(&self, relative: &Path) -> Result<Vec<u8>, CoreError> {
        let final_path = self.root.join(relative);
        let traversed = traversed_directories(&self.root, relative)?;
        let mut directory_pairs = Vec::new();
        for directory in &traversed {
            let pair = observe_directory(directory)?;
            directory_pairs.push(pair);
        }
        let leaf_metadata = fs::symlink_metadata(&final_path)
            .map_err(|err| storage_error("inspect object", &final_path, &err))?;
        if leaf_metadata.is_symlink() {
            return Err(typed_refusal(
                quanta_index_contract::SearchPlaneErrorCodeV2::StateRootInsecure,
                format!(
                    "repomap object store refuses a symlinked object: {}",
                    final_path.display()
                ),
            ));
        }
        let lstat = observed_metadata(&final_path, &leaf_metadata)
            .map_err(|detail| storage_error("observe object", &final_path, &detail))?;
        let mut file = File::open(&final_path)
            .map_err(|err| storage_error("open object", &final_path, &err))?;
        let fstat_metadata = file
            .metadata()
            .map_err(|err| storage_error("fstat object", &final_path, &err))?;
        let fstat = observed_metadata(&final_path, &fstat_metadata)
            .map_err(|detail| storage_error("fstat object", &final_path, &detail))?;
        let leaf = SecureMetadataPairV1 {
            lstat,
            fstat: Some(fstat),
        };
        if let Err(error) = self.security.verify_opened_file(&directory_pairs, leaf) {
            return Err(typed_refusal(
                quanta_index_contract::SearchPlaneErrorCodeV2::StateRootInsecure,
                format!("repomap object store refused {}: {error}", final_path.display()),
            ));
        }
        let mut bytes = Vec::new();
        let _read = file
            .read_to_end(&mut bytes)
            .map_err(|err| storage_error("read object", &final_path, &err))?;
        Ok(bytes)
    }

    /// Create every fanout directory of a content address (0700, exact),
    /// returning them innermost-last for fsync.
    fn ensure_fanout_directories(&self, relative: &Path) -> Result<Vec<PathBuf>, CoreError> {
        let mut created = Vec::new();
        let mut current = self.root.clone();
        let components = relative.components().collect::<Vec<_>>();
        // The leaf is a file; every component before it is a directory.
        let directory_components = components
            .split_last()
            .map(|(_leaf, dirs)| dirs)
            .unwrap_or_default();
        for component in directory_components {
            let name = component.as_os_str();
            current.push(name);
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => {
                    return Err(storage_error(
                        "create fanout directory (path is not a directory)",
                        &current,
                        &"unexpected kind",
                    ));
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir(&current)
                        .map_err(|err| storage_error("create fanout directory", &current, &err))?;
                }
                Err(err) => {
                    return Err(storage_error("inspect fanout directory", &current, &err));
                }
            }
            set_directory_mode(&current)?;
            created.push(current.clone());
        }
        Ok(created)
    }

    /// Project one quarantine incident durably: the canonical incident
    /// envelope at its content address and (when raw bytes were readable)
    /// the payload at its content address, create-new with exact-byte
    /// replay and file/directory fsync.
    pub(crate) fn project_quarantine(
        &self,
        incident: &QuarantineIncidentV1,
        payload_bytes: Option<&[u8]>,
    ) -> Result<(QuarantineIncidentDigestV1, Option<QuarantinePayloadDigestV1>), CoreError> {
        let envelope_bytes = incident
            .encode_canonical()
            .map_err(|err| object_codec_error("encode quarantine incident", &err))?;
        let incident_digest = incident
            .digest()
            .map_err(|err| object_codec_error("derive incident digest", &err))?;
        let incident_address = crate::layout_v3::QuarantineIncidentAddressV1::new(incident_digest);
        write_content_addressed(
            &self.root,
            &incident_address.relative_path(),
            envelope_bytes.as_slice(),
            "quarantine incident",
        )?;
        let payload_digest = payload_bytes.map(|bytes| {
            let mut hasher = Sha256::new();
            hasher.update(b"quanta-index/quarantine-payload/v1\0");
            hasher.update(bytes);
            QuarantinePayloadDigestV1::from_bytes(hasher.finalize().into())
        });
        if let (Some(bytes), Some(digest)) = (payload_bytes, payload_digest) {
            let address = crate::layout_v3::QuarantinePayloadAddressV1::new(digest);
            write_content_addressed(
                &self.root,
                &address.relative_path(),
                bytes,
                "quarantine payload",
            )?;
        }
        Ok((incident_digest, payload_digest))
    }

    /// Reclaim one projected quarantine payload (online discard): the
    /// incident/event row stays durable in the catalog; only the payload
    /// projection goes. The directory is fsynced after the unlink.
    pub(crate) fn reclaim_quarantine_payload(
        &self,
        digest: QuarantinePayloadDigestV1,
    ) -> Result<QuarantinePayloadDigestV1, CoreError> {
        let address = crate::layout_v3::QuarantinePayloadAddressV1::new(digest);
        let relative = address.relative_path();
        let path = self.root.join(&relative);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(storage_error("reclaim quarantine payload", &path, &err)),
        }
        // A payload projection that never existed (the incident recorded
        // no readable bytes) has no directory to sync either.
        if let Some(parent) = path.parent()
            && parent.is_dir()
        {
            sync_directory(parent)?;
        }
        Ok(digest)
    }

    /// Unlink one source object after its quarantine projections are
    /// durable, then fsync the source directory so the removal survives a
    /// crash.
    pub(crate) fn unlink_object(&self, digest: CandidateObjectDigestV1) -> Result<(), CoreError> {
        let address = CandidateObjectAddressV1::new(digest);
        let relative = address.relative_path();
        let path = self.root.join(&relative);
        match fs::remove_file(&path) {
            Ok(()) => {}
            // Already gone: the durable incident row is the authority, so
            // an absent source is a completed projection, not an error.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(storage_error("unlink quarantined object", &path, &err));
            }
        }
        if let Some(parent) = path.parent() {
            sync_directory(parent)?;
        }
        Ok(())
    }

    /// The raw bytes at a quarantine payload projection, for exact-byte
    /// replay checks in tests and the discard path.
    pub(crate) fn read_quarantine_payload(
        &self,
        digest: QuarantinePayloadDigestV1,
    ) -> Result<Option<Vec<u8>>, CoreError> {
        let address = crate::layout_v3::QuarantinePayloadAddressV1::new(digest);
        let path = self.root.join(address.relative_path());
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(storage_error("read quarantine payload", &path, &err)),
        }
    }
}

/// Write bytes at a content address, create-new, with exact-byte replay.
fn write_content_addressed(
    root: &Path,
    relative: &Path,
    bytes: &[u8],
    label: &str,
) -> Result<(), CoreError> {
    let final_path = root.join(relative);
    match fs::symlink_metadata(&final_path) {
        Ok(_) => {
            let existing = fs::read(&final_path)
                .map_err(|err| storage_error("read existing {label}", &final_path, &err))?;
            if existing.as_slice() == bytes {
                // Exact-byte replay: the projection is already durable.
                return Ok(());
            }
            return Err(typed_refusal(
                quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                format!(
                    "repomap object store: the {label} projection at {} does not hold the \
                     exact bytes its digest names",
                    final_path.display()
                ),
            ));
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(storage_error(&format!("inspect {label}"), &final_path, &err));
        }
    }
    // Build the fanout directories with exact modes before the leaf.
    let mut current = root.to_path_buf();
    let components = relative.components().collect::<Vec<_>>();
    let (_, dirs) = components
        .split_last()
        .ok_or_else(|| storage_error("split {label} path", &final_path, &"empty path"))?;
    for component in dirs {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(storage_error(
                    &format!("create {label} fanout directory"),
                    &current,
                    &"path exists and is not a directory",
                ));
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|err| {
                    storage_error(&format!("create {label} fanout directory"), &current, &err)
                })?;
            }
            Err(err) => {
                return Err(storage_error(
                    &format!("inspect {label} fanout directory"),
                    &current,
                    &err,
                ));
            }
        }
        set_directory_mode(&current)?;
    }
    let mut file = open_create_new(&final_path)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|err| storage_error("write {label} projection", &final_path, &err))?;
    drop(file);
    // fsync every fanout directory, innermost first.
    let mut directory = final_path.parent().map(Path::to_path_buf);
    while let Some(current) = directory {
        sync_directory(&current)?;
        directory = current.parent().map(Path::to_path_buf);
        if directory.as_ref().is_some_and(|parent| parent == root) {
            // The root itself is synced by its own writer; stop at it.
            break;
        }
    }
    Ok(())
}

fn traversed_directories(root: &Path, relative: &Path) -> Result<Vec<PathBuf>, CoreError> {
    let mut directories = Vec::new();
    let mut current = root.to_path_buf();
    let components = relative.components().collect::<Vec<_>>();
    let Some((_, dirs)) = components.split_last() else {
        return Err(CoreError::Storage(format!(
            "repomap object store: relative path {} has no leaf",
            relative.display()
        )));
    };
    for component in dirs {
        current.push(component.as_os_str());
        directories.push(current.clone());
    }
    Ok(directories)
}

fn observe_directory(path: &Path) -> Result<SecureMetadataPairV1, CoreError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|err| storage_error("inspect directory", path, &err))?;
    if metadata.is_symlink() {
        return Err(typed_refusal(
            quanta_index_contract::SearchPlaneErrorCodeV2::StateRootInsecure,
            format!("repomap object store refuses a symlinked directory: {}", path.display()),
        ));
    }
    let lstat = observed_metadata(path, &metadata)
        .map_err(|detail| storage_error("observe directory", path, &detail))?;
    let directory = File::open(path).map_err(|err| storage_error("open directory", path, &err))?;
    let fstat_metadata = directory
        .metadata()
        .map_err(|err| storage_error("fstat directory", path, &err))?;
    let fstat = observed_metadata(path, &fstat_metadata)
        .map_err(|detail| storage_error("fstat directory", path, &detail))?;
    Ok(SecureMetadataPairV1 {
        lstat,
        fstat: Some(fstat),
    })
}

/// Reduce platform metadata to the immutable-policy fields. Permission
/// bits only; type bits are carried by `kind`.
#[expect(
    clippy::unnecessary_wraps,
    reason = "the non-unix build arm returns Err, so the Result wrapper is load-bearing there"
)]
fn observed_metadata(
    _path: &Path,
    metadata: &std::fs::Metadata,
) -> Result<ObservedFileMetadataV1, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let file_type = metadata.file_type();
        let kind = if file_type.is_symlink() {
            ObservedFileKindV1::Symlink
        } else if file_type.is_dir() {
            ObservedFileKindV1::Directory
        } else if file_type.is_file() {
            ObservedFileKindV1::RegularFile
        } else {
            ObservedFileKindV1::Other
        };
        Ok(ObservedFileMetadataV1 {
            device: metadata.dev(),
            inode: metadata.ino(),
            uid: metadata.uid(),
            mode: metadata.mode() & 0o777,
            link_count: metadata.nlink(),
            kind,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (path, metadata);
        Err("platform metadata is unavailable outside unix".to_string())
    }
}

fn open_create_new(path: &Path) -> Result<File, CoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|err| storage_error("create-new", path, &err))
    }
    #[cfg(not(unix))]
    {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|err| storage_error("create-new", path, &err))
    }
}

fn set_directory_mode(path: &Path) -> Result<(), CoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|err| storage_error("set directory mode", path, &err))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn sync_directory(path: &Path) -> Result<(), CoreError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|err| storage_error("sync directory", path, &err))
}

fn object_codec_error(action: &str, error: &CanonicalRepoMapCodecErrorV1) -> CoreError {
    CoreError::Storage(format!("repomap object store {action}: {error}"))
}

/// Derive 16 uuid bytes for a fresh root: a uniqueness binding over the
/// root path, the creation instant and the creating process.
///
/// It is not a secret and not a content identity; two distinct roots never
/// share it in practice.
fn derive_uuid_bytes(root: &Path) -> [u8; ROOT_UUID_BYTES] {
    let mut hasher = Sha256::new();
    hasher.update(b"quanta-index/repomap-root-uuid/v1\0");
    hasher.update(root.to_string_lossy().as_bytes());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    hasher.update(nanos.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let (head, _tail) = digest.split_at(ROOT_UUID_BYTES);
    let mut uuid = [0_u8; ROOT_UUID_BYTES];
    uuid.copy_from_slice(head);
    uuid
}
