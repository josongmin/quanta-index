//! Linux installed Product authority. Caller environment and CLI arguments
//! never select the trust key, active generation, or executable image.

use std::fs::File;
#[cfg(target_os = "linux")]
use std::fs::{Metadata, OpenOptions};
#[cfg(target_os = "linux")]
use std::io::Read;
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::path::{Component, Path};

#[cfg(target_os = "linux")]
use ed25519_dalek::{Signature, VerifyingKey};
#[cfg(target_os = "linux")]
use serde_json::Value;
#[cfg(target_os = "linux")]
use sha2::{Digest as _, Sha256};

#[cfg(target_os = "linux")]
const TRUST_ROOT_V1: &str = "/etc/semantica-codegraph-v2/trust";
#[cfg(target_os = "linux")]
const ACTIVE_DOMAIN_V1: &[u8] = b"semantica.installed-product-active.v1\n";
#[cfg(target_os = "linux")]
const ACTIVE_SCHEMA_V1: &str = "semantica-installed-product-active/v1";
#[cfg(target_os = "linux")]
const MAX_ACTIVE_BYTES_V1: u64 = 8 * 1024;
#[cfg(target_os = "linux")]
const MAX_AG5_RECORD_BYTES_V1: u64 = 256 * 1024;
#[cfg(target_os = "linux")]
const MAX_MANIFEST_BYTES_V1: u64 = 256 * 1024;


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstalledRoleV1 {
    Cli,
    Server,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstalledTrustErrorV1 {
    UnsupportedHost,
    MissingOrUnsafeTrustRoot,
    InvalidSignature,
    InvalidActiveGeneration,
    ForeignImage,
    ChangedImage,
    RotationInProgress,
    Io,
    AdmissionRefused,
}

impl std::fmt::Display for InstalledTrustErrorV1 {
    fn fmt(&self, formatter_v1: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter_v1, "{self:?}")
    }
}

impl std::error::Error for InstalledTrustErrorV1 {}

/// The caller retains the exact request budget and real memory leases.
/// Every method must refuse before the corresponding read, hash, or allocation.
pub trait InstalledArtifactTrustAdmissionV1 {
    type RefusalV1;
    type MemoryLeaseV1;

    fn checkpoint_v1(&mut self) -> Result<(), Self::RefusalV1>;
    fn charge_io_v1(&mut self, operations_v1: u64) -> Result<(), Self::RefusalV1>;
    fn charge_read_v1(&mut self, bytes_v1: u64) -> Result<(), Self::RefusalV1>;
    fn charge_hash_v1(&mut self, bytes_v1: u64) -> Result<(), Self::RefusalV1>;
    fn reserve_memory_v1(&mut self, bytes_v1: usize) -> Result<Self::MemoryLeaseV1, Self::RefusalV1>;
}

#[derive(Debug)]
pub enum InstalledArtifactPeerErrorV1<E> {
    Trust(InstalledTrustErrorV1),
    Admission(E),
}

impl<E: std::fmt::Display> std::fmt::Display for InstalledArtifactPeerErrorV1<E> {
    fn fmt(&self, formatter_v1: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Trust(error_v1) => write!(formatter_v1, "installed trust: {error_v1}"),
            Self::Admission(error_v1) => write!(formatter_v1, "installed trust admission: {error_v1}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for InstalledArtifactPeerErrorV1<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Trust(error_v1) => Some(error_v1),
            Self::Admission(error_v1) => Some(error_v1),
        }
    }
}

#[cfg(all(target_os = "linux", feature = "s-product-startup-v1"))]
struct UnmeteredInstalledAdmissionV1;

#[cfg(all(target_os = "linux", feature = "s-product-startup-v1"))]
impl InstalledArtifactTrustAdmissionV1 for UnmeteredInstalledAdmissionV1 {
    type RefusalV1 = std::convert::Infallible;
    type MemoryLeaseV1 = ();

    fn checkpoint_v1(&mut self) -> Result<(), Self::RefusalV1> { Ok(()) }
    fn charge_io_v1(&mut self, _operations_v1: u64) -> Result<(), Self::RefusalV1> { Ok(()) }
    fn charge_read_v1(&mut self, _bytes_v1: u64) -> Result<(), Self::RefusalV1> { Ok(()) }
    fn charge_hash_v1(&mut self, _bytes_v1: u64) -> Result<(), Self::RefusalV1> { Ok(()) }
    fn reserve_memory_v1(&mut self, _bytes_v1: usize) -> Result<(), Self::RefusalV1> { Ok(()) }
}

#[cfg(target_os = "linux")]
struct VerifyContextV1<'a, A: InstalledArtifactTrustAdmissionV1> {
    admission_v1: &'a mut A,
    leases_v1: Vec<A::MemoryLeaseV1>,
    refusal_v1: Option<A::RefusalV1>,
}

#[cfg(target_os = "linux")]
impl<A: InstalledArtifactTrustAdmissionV1> VerifyContextV1<'_, A> {
    fn admit_v1(&mut self, result_v1: Result<(), A::RefusalV1>) -> Result<(), InstalledTrustErrorV1> {
        result_v1.map_err(|refusal_v1| {
            self.refusal_v1 = Some(refusal_v1);
            InstalledTrustErrorV1::AdmissionRefused
        })
    }

    fn checkpoint_v1(&mut self) -> Result<(), InstalledTrustErrorV1> {
        let result_v1 = self.admission_v1.checkpoint_v1();
        self.admit_v1(result_v1)
    }

    fn read_v1(&mut self, bytes_v1: u64) -> Result<(), InstalledTrustErrorV1> {
        self.checkpoint_v1()?;
        let result_v1 = self.admission_v1.charge_read_v1(bytes_v1);
        self.admit_v1(result_v1)
    }

    fn io_v1(&mut self) -> Result<(), InstalledTrustErrorV1> {
        self.checkpoint_v1()?;
        let result_v1 = self.admission_v1.charge_io_v1(1);
        self.admit_v1(result_v1)
    }

    fn hash_v1(&mut self, bytes_v1: u64) -> Result<(), InstalledTrustErrorV1> {
        self.checkpoint_v1()?;
        let result_v1 = self.admission_v1.charge_hash_v1(bytes_v1);
        self.admit_v1(result_v1)
    }

    fn memory_v1(&mut self, bytes_v1: usize) -> Result<(), InstalledTrustErrorV1> {
        self.checkpoint_v1()?;
        let lease_v1 = self.admission_v1.reserve_memory_v1(bytes_v1).map_err(|refusal_v1| {
            self.refusal_v1 = Some(refusal_v1);
            InstalledTrustErrorV1::AdmissionRefused
        })?;
        self.leases_v1.push(lease_v1);
        Ok(())
    }

    fn open_v1(&mut self, anchor_v1: &Path, path_v1: &Path, owner_uid_v1: u32, executable_v1: bool) -> Result<File, InstalledTrustErrorV1> {
        opened_protected_file_with_io_v1(anchor_v1, path_v1, owner_uid_v1, executable_v1, &mut || self.io_v1())
    }

    fn protected_chain_v1(&mut self, anchor_v1: &Path, path_v1: &Path, owner_uid_v1: u32) -> Result<(), InstalledTrustErrorV1> {
        let path_bytes_v1 = anchor_v1.as_os_str().len()
            .checked_add(path_v1.as_os_str().len())
            .and_then(|bytes_v1| bytes_v1.checked_add(1))
            .and_then(|bytes_v1| bytes_v1.checked_mul(8))
            .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?;
        self.memory_v1(path_bytes_v1)?;
        protected_chain_with_io_v1(anchor_v1, path_v1, owner_uid_v1, &mut || self.io_v1())
    }

    fn bounded_read_v1(&mut self, file_v1: &File, limit_v1: u64) -> Result<Vec<u8>, InstalledTrustErrorV1> {
        self.io_v1()?;
        let size_v1 = file_v1.metadata().map_err(|_| InstalledTrustErrorV1::Io)?.len();
        if size_v1 > limit_v1 {
            return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
        }
        let size_v1 = usize::try_from(size_v1).map_err(|_| InstalledTrustErrorV1::InvalidActiveGeneration)?;
        // Retain the grant throughout the verified callback. JSON object
        // decoding and canonical re-encoding can exceed source byte size.
        let grant_v1 = size_v1.checked_mul(128).and_then(|size_v1| size_v1.checked_add(1))
            .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?;
        self.memory_v1(grant_v1)?;
        self.read_v1(u64::try_from(size_v1).map_err(|_| InstalledTrustErrorV1::InvalidActiveGeneration)?
            .checked_add(1).ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?)?;
        self.io_v1()?; // exact-length read
        self.io_v1()?; // trailing-byte probe
        bounded_read_expected_v1(file_v1, limit_v1, Some(size_v1))
    }

    fn digest_v1(&mut self, file_v1: &File, limit_v1: u64) -> Result<String, InstalledTrustErrorV1> {
        digest_file_with_admission_v1(file_v1, limit_v1, |read_v1, hashed_v1| {
            if read_v1 != 0 { self.io_v1()?; }
            self.read_v1(read_v1)?;
            if hashed_v1 != 0 { self.hash_v1(hashed_v1)?; }
            Ok(())
        })
    }

    fn canonical_v1(&mut self, value_v1: &Value, encoded_upper_bound_v1: usize) -> Result<Vec<u8>, InstalledTrustErrorV1> {
        self.memory_v1(encoded_upper_bound_v1.checked_mul(2).and_then(|value_v1| value_v1.checked_add(1))
            .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?)?;
        canonical_value_v1(value_v1)
    }
}


#[derive(Debug)]
pub struct VerifiedInstalledArtifactsV2 {
    pub cli_image_v2: File,
    pub generation_v1: u64,
    pub release_manifest_sha256_v1: String,
    pub server_path_v1: PathBuf,
    pub server_sha256_v1: String,
    pub sql_worker_path_v1: PathBuf,
    pub sql_worker_sha256_v1: String,
    pub parser_worker_path_v1: PathBuf,
    pub parser_worker_sha256_v1: String,
    pub server_image_v1: File,
    pub release_manifest_image_v1: File,
    pub release_manifest_path_v1: PathBuf,
    pub sql_worker_image_v1: File,
    pub parser_worker_image_v1: File,
    pub z3_solver_image_v1: Option<File>,
    pub z3_solver_sha256_v1: Option<String>,
    pub z3_solver_reported_version_v1: Option<String>,
    pub release_root_v1: PathBuf,
    pub release_manifest_digest_v1: String,
    pub owner_uid_v1: u32,
    pub _rotation_lock_v1: File,
}

#[cfg(target_os = "linux")]

fn same_image_v1(left_v1: &Metadata, right_v1: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left_v1.dev() == right_v1.dev()
        && left_v1.ino() == right_v1.ino()
        && left_v1.len() == right_v1.len()
}

#[cfg(target_os = "linux")]
fn protected_chain_with_io_v1(
    anchor_v1: &Path, path_v1: &Path, owner_uid_v1: u32,
    io_v1: &mut impl FnMut() -> Result<(), InstalledTrustErrorV1>,
) -> Result<(), InstalledTrustErrorV1> {
    use std::os::unix::fs::MetadataExt;
    let relative_v1 = path_v1
        .strip_prefix(anchor_v1)
        .map_err(|_| InstalledTrustErrorV1::MissingOrUnsafeTrustRoot)?;
    let mut cursor_v1 = anchor_v1.to_path_buf();
    for part_v1 in std::iter::once(Component::CurDir).chain(relative_v1.components()) {
        match part_v1 {
            Component::CurDir => {}
            Component::Normal(name_v1) => cursor_v1.push(name_v1),
            _ => return Err(InstalledTrustErrorV1::MissingOrUnsafeTrustRoot),
        }
        io_v1()?;
        let meta_v1 = cursor_v1
            .symlink_metadata()
            .map_err(|_| InstalledTrustErrorV1::MissingOrUnsafeTrustRoot)?;
        if !meta_v1.is_dir()
            || meta_v1.file_type().is_symlink()
            || meta_v1.uid() != owner_uid_v1
            || meta_v1.mode() & 0o022 != 0
        {
            return Err(InstalledTrustErrorV1::MissingOrUnsafeTrustRoot);
        }
    }
    Ok(())
}

#[cfg(all(target_os = "linux", feature = "s-product-startup-v1"))]
fn opened_protected_file_v1(
    anchor_v1: &Path,
    path_v1: &Path,
    owner_uid_v1: u32,
    executable_v1: bool,
) -> Result<File, InstalledTrustErrorV1> {
    opened_protected_file_with_io_v1(anchor_v1, path_v1, owner_uid_v1, executable_v1, &mut || Ok(()))
}

#[cfg(target_os = "linux")]
fn opened_protected_file_with_io_v1(
    anchor_v1: &Path, path_v1: &Path, owner_uid_v1: u32,
    executable_v1: bool,
    io_v1: &mut impl FnMut() -> Result<(), InstalledTrustErrorV1>,
) -> Result<File, InstalledTrustErrorV1> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let parent_v1 = path_v1
        .parent()
        .ok_or(InstalledTrustErrorV1::MissingOrUnsafeTrustRoot)?;
    protected_chain_with_io_v1(anchor_v1, parent_v1, owner_uid_v1, io_v1)?;
    io_v1()?;
    let file_v1 = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path_v1)
        .map_err(|_| InstalledTrustErrorV1::MissingOrUnsafeTrustRoot)?;
    io_v1()?;
    let opened_v1 = file_v1.metadata().map_err(|_| InstalledTrustErrorV1::Io)?;
    io_v1()?;
    let named_v1 = path_v1
        .symlink_metadata()
        .map_err(|_| InstalledTrustErrorV1::MissingOrUnsafeTrustRoot)?;
    if !opened_v1.is_file()
        || named_v1.file_type().is_symlink()
        || !same_image_v1(&opened_v1, &named_v1)
        || opened_v1.uid() != owner_uid_v1
        || opened_v1.mode() & 0o022 != 0
        || (executable_v1 && opened_v1.mode() & 0o222 != 0)
        || (executable_v1 && opened_v1.mode() & 0o111 == 0)
    {
        return Err(InstalledTrustErrorV1::MissingOrUnsafeTrustRoot);
    }
    Ok(file_v1)
}

#[cfg(all(target_os = "linux", feature = "s-product-startup-v1"))]
fn bounded_read_v1(file_v1: &File, limit_v1: u64) -> Result<Vec<u8>, InstalledTrustErrorV1> {
    bounded_read_expected_v1(file_v1, limit_v1, None)
}

#[cfg(target_os = "linux")]
fn bounded_read_expected_v1(mut file_v1: &File, limit_v1: u64, expected_v1: Option<usize>) -> Result<Vec<u8>, InstalledTrustErrorV1> {
    let length_v1 = file_v1.metadata().map_err(|_| InstalledTrustErrorV1::Io)?.len();
    if length_v1 > limit_v1 {
        return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
    }
    let size_v1 = usize::try_from(length_v1).map_err(|_| InstalledTrustErrorV1::InvalidActiveGeneration)?;
    if expected_v1.is_some_and(|expected_v1| expected_v1 != size_v1) {
        return Err(InstalledTrustErrorV1::ChangedImage);
    }
    let mut data_v1 = vec![0u8; size_v1];
    file_v1.read_exact(&mut data_v1).map_err(|_| InstalledTrustErrorV1::Io)?;
    let mut extra_v1 = [0u8; 1];
    if file_v1.read(&mut extra_v1).map_err(|_| InstalledTrustErrorV1::Io)? != 0 {
        return Err(InstalledTrustErrorV1::ChangedImage);
    }
    Ok(data_v1)
}

#[cfg(target_os = "linux")]
fn sha256_v1(bytes_v1: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes_v1))
}

#[cfg(target_os = "linux")]
fn hex_bytes_v1<const N: usize>(value_v1: &str) -> Result<[u8; N], InstalledTrustErrorV1> {
    if value_v1.len() != 2 * N
        || !value_v1.bytes().all(|byte_v1| byte_v1.is_ascii_hexdigit())
        || value_v1.bytes().any(|byte_v1| byte_v1.is_ascii_uppercase())
    {
        return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
    }
    let mut bytes_v1 = [0u8; N];
    for (index_v1, byte_v1) in bytes_v1.iter_mut().enumerate() {
        *byte_v1 = u8::from_str_radix(
            value_v1
                .get(index_v1 * 2..index_v1 * 2 + 2)
                .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?,
            16,
        )
        .map_err(|_| InstalledTrustErrorV1::InvalidActiveGeneration)?;
    }
    Ok(bytes_v1)
}

#[cfg(target_os = "linux")]
fn field_v1<'a>(payload_v1: &'a Value, name_v1: &str) -> Result<&'a str, InstalledTrustErrorV1> {
    payload_v1
        .get(name_v1)
        .and_then(Value::as_str)
        .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)
}

#[cfg(target_os = "linux")]
fn canonical_value_v1(value_v1: &Value) -> Result<Vec<u8>, InstalledTrustErrorV1> {
    fn append_sorted_v1(
        value_v1: &Value,
        bytes_v1: &mut Vec<u8>,
    ) -> Result<(), InstalledTrustErrorV1> {
        match value_v1 {
            Value::Object(fields_v1) => {
                bytes_v1.push(b'{');
                let mut names_v1: Vec<&String> = fields_v1.keys().collect();
                names_v1.sort_unstable();
                for (index_v1, name_v1) in names_v1.into_iter().enumerate() {
                    if index_v1 != 0 {
                        bytes_v1.push(b',');
                    }
                    bytes_v1.extend(
                        serde_json::to_vec(name_v1)
                            .map_err(|_| InstalledTrustErrorV1::InvalidActiveGeneration)?,
                    );
                    bytes_v1.push(b':');
                    append_sorted_v1(
                        fields_v1
                            .get(name_v1)
                            .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?,
                        bytes_v1,
                    )?;
                }
                bytes_v1.push(b'}');
            }
            Value::Array(items_v1) => {
                bytes_v1.push(b'[');
                for (index_v1, item_v1) in items_v1.iter().enumerate() {
                    if index_v1 != 0 {
                        bytes_v1.push(b',');
                    }
                    append_sorted_v1(item_v1, bytes_v1)?;
                }
                bytes_v1.push(b']');
            }
            scalar_v1 => bytes_v1.extend(
                serde_json::to_vec(scalar_v1)
                    .map_err(|_| InstalledTrustErrorV1::InvalidActiveGeneration)?,
            ),
        }
        Ok(())
    }
    let mut bytes_v1 = Vec::new();
    append_sorted_v1(value_v1, &mut bytes_v1)?;
    // AG5 uses Python ensure_ascii=True. Restrict this authority version to
    // ASCII so the signed canonical bytes agree across both implementations.
    if !bytes_v1.is_ascii() {
        return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
    }
    Ok(bytes_v1)
}

#[cfg(all(target_os = "linux", feature = "s-product-startup-v1"))]
fn digest_file_v1(file_v1: &File, limit_v1: u64) -> Result<String, InstalledTrustErrorV1> {
    digest_file_with_admission_v1(file_v1, limit_v1, |_, _| Ok(()))
}

#[cfg(target_os = "linux")]
fn digest_file_with_admission_v1(
    file_v1: &File, limit_v1: u64,
    mut admit_v1: impl FnMut(u64, u64) -> Result<(), InstalledTrustErrorV1>,
) -> Result<String, InstalledTrustErrorV1> {
    use std::os::unix::fs::FileExt;
    let mut hash_v1 = Sha256::new();
    let mut total_v1 = 0u64;
    let mut chunk_v1 = [0u8; 64 * 1024];
    loop {
        admit_v1(chunk_v1.len() as u64, 0)?;
        // read_at leaves the retained executable FD at offset zero for the
        // later FD-backed transport; try_clone would share its open-file offset.
        let count_v1 = file_v1
            .read_at(&mut chunk_v1, total_v1)
            .map_err(|_| InstalledTrustErrorV1::Io)?;
        if count_v1 == 0 {
            break;
        }
        total_v1 = total_v1
            .checked_add(count_v1 as u64)
            .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?;
        if total_v1 > limit_v1 {
            return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
        }
        admit_v1(0, count_v1 as u64)?;
        hash_v1.update(chunk_v1.get(..count_v1).ok_or(InstalledTrustErrorV1::Io)?);
    }
    Ok(format!("{:x}", hash_v1.finalize()))
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct VerifiedSignedInstallV1 {
    envelope_v1: Value,
    manifest_payload_v1: Value,
    manifest_v1: File,
    manifest_path_v1: PathBuf,
    release_root_v1: PathBuf,
    generation_v1: u64,
    rotation_lock_v1: File,
    manifest_digest_v1: String,
}

#[cfg(target_os = "linux")]
fn verify_signed_install_v1<A: InstalledArtifactTrustAdmissionV1>(
    context_v1: &mut VerifyContextV1<'_, A>,
    trust_root_v1: &Path,
    anchor_v1: &Path,
    owner_uid_v1: u32,
) -> Result<VerifiedSignedInstallV1, InstalledTrustErrorV1> {
    context_v1.protected_chain_v1(anchor_v1, trust_root_v1, owner_uid_v1)?;
    let rotation_lock_v1 = context_v1.open_v1(
        anchor_v1,
        &trust_root_v1.join("rotation.lock"),
        owner_uid_v1,
        false,
    )?;
    {
        context_v1.io_v1()?;
        fs2::FileExt::try_lock_shared(&rotation_lock_v1).map_err(|error_v1| {
            if error_v1.kind() == std::io::ErrorKind::WouldBlock {
                InstalledTrustErrorV1::RotationInProgress
            } else {
                InstalledTrustErrorV1::Io
            }
        })?;
    }
    let key_v1 = context_v1.open_v1(
        anchor_v1,
        &trust_root_v1.join("deployment-public-key.bin"),
        owner_uid_v1,
        false,
    )?;
    let key_bytes_v1: [u8; 32] = context_v1.bounded_read_v1(&key_v1, 32)?
        .try_into()
        .map_err(|_| InstalledTrustErrorV1::InvalidActiveGeneration)?;
    let active_v1 = context_v1.open_v1(
        anchor_v1,
        &trust_root_v1.join("active-generation.json"),
        owner_uid_v1,
        false,
    )?;
    let raw_v1 = context_v1.bounded_read_v1(&active_v1, MAX_ACTIVE_BYTES_V1)?;
    context_v1.checkpoint_v1()?;
    let envelope_v1: Value = serde_json::from_slice(&raw_v1)
        .map_err(|_| InstalledTrustErrorV1::InvalidActiveGeneration)?;
    let object_v1 = envelope_v1
        .as_object()
        .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?;
    if object_v1.len() != 2
        || !object_v1.contains_key("payload")
        || !object_v1.contains_key("signature_hex")
        || [context_v1.canonical_v1(&envelope_v1, MAX_ACTIVE_BYTES_V1 as usize)?, b"\n".to_vec()].concat() != raw_v1
    {
        return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
    }
    let payload_v1 = envelope_v1
        .get("payload")
        .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?;
    let fields_v1 = payload_v1
        .as_object()
        .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?;
    let expected_v1 = [
        "schema_version",
        "install_root",
        "generation",
        "ag5_active_record_sha256",
        "release_manifest_sha256",
        "cli_sha256",
        "server_sha256",
        "sql_rulepack_worker_sha256",
        "parser_native_worker_sha256",
    ];
    if fields_v1.len() != expected_v1.len()
        || expected_v1
            .iter()
            .any(|name_v1| !fields_v1.contains_key(*name_v1))
        || field_v1(payload_v1, "schema_version")? != ACTIVE_SCHEMA_V1
    {
        return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
    }
    let signature_v1 = hex_bytes_v1::<64>(
        envelope_v1
            .get("signature_hex")
            .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?
            .as_str()
            .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?,
    )?;
    let key_v1 = VerifyingKey::from_bytes(&key_bytes_v1)
        .map_err(|_| InstalledTrustErrorV1::InvalidSignature)?;
    let mut signed_v1 = ACTIVE_DOMAIN_V1.to_vec();
    signed_v1.extend(context_v1.canonical_v1(payload_v1, MAX_ACTIVE_BYTES_V1 as usize)?);
    context_v1.hash_v1(signed_v1.len() as u64)?;
    key_v1
        .verify_strict(&signed_v1, &Signature::from_bytes(&signature_v1))
        .map_err(|_| InstalledTrustErrorV1::InvalidSignature)?;
    let generation_v1 = payload_v1
        .get("generation")
        .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?
        .as_u64()
        .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?;
    let install_root_v1 = PathBuf::from(field_v1(payload_v1, "install_root")?);
    context_v1.protected_chain_v1(anchor_v1, &install_root_v1, owner_uid_v1)?;
    let active_record_v1 = context_v1.open_v1(
        anchor_v1,
        &install_root_v1.join("records/active.json"),
        owner_uid_v1,
        false,
    )?;
    let active_bytes_v1 = context_v1.bounded_read_v1(&active_record_v1, MAX_AG5_RECORD_BYTES_V1)?;
    context_v1.hash_v1(active_bytes_v1.len() as u64)?;
    if sha256_v1(&active_bytes_v1) != field_v1(payload_v1, "ag5_active_record_sha256")? {
        return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
    }
    context_v1.checkpoint_v1()?;
    let ag5_v1: Value = serde_json::from_slice(&active_bytes_v1)
        .map_err(|_| InstalledTrustErrorV1::InvalidActiveGeneration)?;
    let ag5_payload_v1 = ag5_v1
        .get("payload")
        .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?;
    if ag5_payload_v1.get("generation").and_then(Value::as_u64) != Some(generation_v1)
        || field_v1(ag5_payload_v1, "active_manifest_sha256")?
            != field_v1(payload_v1, "release_manifest_sha256")?
        || field_v1(ag5_payload_v1, "install_root")?
            != install_root_v1
                .to_str()
                .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?
    {
        return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
    }
    let manifest_digest_v1 = field_v1(payload_v1, "release_manifest_sha256")?;
    let _ = hex_bytes_v1::<32>(manifest_digest_v1)?;
    let release_root_v1 = install_root_v1.join("releases").join(manifest_digest_v1);
    context_v1.protected_chain_v1(anchor_v1, &release_root_v1, owner_uid_v1)?;
    let manifest_path_v1 = release_root_v1.join("share/quanta/release-manifest.json");
    let manifest_v1 = context_v1.open_v1(
        anchor_v1,
        &manifest_path_v1,
        owner_uid_v1,
        false,
    )?;
    if context_v1.digest_v1(&manifest_v1, MAX_MANIFEST_BYTES_V1)? != manifest_digest_v1 {
        return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
    }
    context_v1.checkpoint_v1()?;
    let manifest_payload_v1: Value =
        serde_json::from_slice(&context_v1.bounded_read_v1(&manifest_v1, MAX_MANIFEST_BYTES_V1)?)
            .map_err(|_| InstalledTrustErrorV1::InvalidActiveGeneration)?;
    if manifest_payload_v1.get("schema_version").and_then(Value::as_str)
        != Some("quanta-release-bundle-v4")
    {
        return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
    }
    let manifest_digest_v1 = manifest_digest_v1.to_owned();
    Ok(VerifiedSignedInstallV1 {
        envelope_v1,
        manifest_payload_v1,
        manifest_v1,
        manifest_path_v1,
        release_root_v1,
        generation_v1,
        rotation_lock_v1,
        manifest_digest_v1,
    })
}

/// Signed server peer and its already opened manifest, executable and
/// installation rotation lock. The retained memory grants cover verification.
pub struct VerifiedInstalledServerPeerV1<L> {
    server_image_v1: File,
    manifest_image_v1: File,
    _rotation_lock_v1: File,
    server_path_v1: PathBuf,
    manifest_path_v1: PathBuf,
    generation_v1: u64,
    manifest_sha256_v1: String,
    server_sha256_v1: [u8; 32],
    _memory_leases_v1: Vec<L>,
}

impl<L> VerifiedInstalledServerPeerV1<L> {
    pub fn server_image_v1(&self) -> &File { &self.server_image_v1 }
    pub fn manifest_image_v1(&self) -> &File { &self.manifest_image_v1 }
    pub fn server_path_v1(&self) -> &std::path::Path { &self.server_path_v1 }
    pub fn manifest_path_v1(&self) -> &std::path::Path { &self.manifest_path_v1 }
    pub fn generation_v1(&self) -> u64 { self.generation_v1 }
    pub fn installed_generation_v1(&self) -> u64 { self.generation_v1 }
    pub fn manifest_sha256_v1(&self) -> &str { &self.manifest_sha256_v1 }
    pub fn release_manifest_digest_v1(&self) -> &str { &self.manifest_sha256_v1 }
    pub fn server_image_sha256_bytes_v1(&self) -> &[u8; 32] { &self.server_sha256_v1 }
}

#[cfg(target_os = "linux")]
fn verify_server_peer_with_context_v1<A: InstalledArtifactTrustAdmissionV1>(
    context_v1: &mut VerifyContextV1<'_, A>,
    peer_image_v1: &File,
    trust_root_v1: &Path,
    anchor_v1: &Path,
    owner_uid_v1: u32,
) -> Result<VerifiedInstalledServerPeerV1<A::MemoryLeaseV1>, InstalledTrustErrorV1> {
    let selected_v1 = verify_signed_install_v1(context_v1, trust_root_v1, anchor_v1, owner_uid_v1)?;
    let payload_v1 = selected_v1.envelope_v1.get("payload")
        .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?;
    let expected_v1 = field_v1(payload_v1, "server_sha256")?;
    let expected_bytes_v1 = hex_bytes_v1::<32>(expected_v1)?;
    let server_path_v1 = selected_v1.release_root_v1.join("libexec/quanta/quanta-cli-server");
    let server_image_v1 = context_v1.open_v1(anchor_v1, &server_path_v1, owner_uid_v1, true)?;
    if context_v1.digest_v1(&server_image_v1, 2 * 1024 * 1024 * 1024)? != expected_v1 {
        return Err(InstalledTrustErrorV1::ChangedImage);
    }
    context_v1.io_v1()?;
    let peer_meta_v1 = peer_image_v1.metadata().map_err(|_| InstalledTrustErrorV1::Io)?;
    context_v1.io_v1()?;
    let server_meta_v1 = server_image_v1.metadata().map_err(|_| InstalledTrustErrorV1::Io)?;
    if !same_image_v1(
        &peer_meta_v1,
        &server_meta_v1,
    ) {
        return Err(InstalledTrustErrorV1::ForeignImage);
    }
    let artifact_rows_v1 = selected_v1.manifest_payload_v1.get("artifacts")
        .and_then(Value::as_array)
        .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?;
    let mut server_row_v1 = None;
    for row_v1 in artifact_rows_v1 {
        context_v1.checkpoint_v1()?;
        if row_v1.get("role").and_then(Value::as_str) == Some("server") {
            if server_row_v1.replace(row_v1).is_some() {
                return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
            }
        }
    }
    let server_row_v1 = server_row_v1.ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?;
    if field_v1(server_row_v1, "relative_path")? != "libexec/quanta/quanta-cli-server"
        || field_v1(server_row_v1, "sha256")? != expected_v1
        || server_row_v1.get("size_bytes").and_then(Value::as_u64)
            != Some(server_meta_v1.len())
    {
        return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
    }
    Ok(VerifiedInstalledServerPeerV1 {
        server_image_v1,
        manifest_image_v1: selected_v1.manifest_v1,
        _rotation_lock_v1: selected_v1.rotation_lock_v1,
        server_path_v1,
        manifest_path_v1: selected_v1.manifest_path_v1,
        generation_v1: selected_v1.generation_v1,
        manifest_sha256_v1: selected_v1.manifest_digest_v1,
        server_sha256_v1: expected_bytes_v1,
        _memory_leases_v1: std::mem::take(&mut context_v1.leases_v1),
    })
}

/// Verify one already opened peer server image against the fixed installation.
/// The callback keeps both the real caller budget and all verification grants.
pub fn with_verified_installed_server_peer_v1<A, R>(
    peer_image_v1: &File,
    admission_v1: &mut A,
    use_verified_v1: impl FnOnce(&VerifiedInstalledServerPeerV1<A::MemoryLeaseV1>, &mut A) -> R,
) -> Result<R, InstalledArtifactPeerErrorV1<A::RefusalV1>>
where
    A: InstalledArtifactTrustAdmissionV1,
{
    #[cfg(target_os = "linux")]
    {
        with_verified_installed_server_peer_at_v1(
            peer_image_v1, admission_v1, Path::new(TRUST_ROOT_V1), Path::new("/"), 0,
            use_verified_v1,
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _peer_image_v1 = peer_image_v1;
        let _admission_v1 = admission_v1;
        drop(use_verified_v1);
        Err(InstalledArtifactPeerErrorV1::Trust(InstalledTrustErrorV1::UnsupportedHost))
    }
}

#[cfg(target_os = "linux")]
fn with_verified_installed_server_peer_at_v1<A, R>(
    peer_image_v1: &File,
    admission_v1: &mut A,
    trust_root_v1: &Path,
    anchor_v1: &Path,
    owner_uid_v1: u32,
    use_verified_v1: impl FnOnce(&VerifiedInstalledServerPeerV1<A::MemoryLeaseV1>, &mut A) -> R,
) -> Result<R, InstalledArtifactPeerErrorV1<A::RefusalV1>>
where
    A: InstalledArtifactTrustAdmissionV1,
{
    let mut context_v1 = VerifyContextV1 {
        admission_v1,
        leases_v1: Vec::new(),
        refusal_v1: None,
    };
    let verified_v1 = match verify_server_peer_with_context_v1(
        &mut context_v1, peer_image_v1, trust_root_v1, anchor_v1, owner_uid_v1,
    ) {
        Ok(verified_v1) => verified_v1,
        Err(InstalledTrustErrorV1::AdmissionRefused) => {
            return Err(InstalledArtifactPeerErrorV1::Admission(
                context_v1.refusal_v1.take().expect("admission refusal retained"),
            ));
        }
        Err(error_v1) => return Err(InstalledArtifactPeerErrorV1::Trust(error_v1)),
    };
    Ok(use_verified_v1(&verified_v1, context_v1.admission_v1))
}

#[cfg(all(target_os = "linux", feature = "fixture-trust-root-v1"))]
pub fn with_verified_installed_server_peer_fixture_v1<A, R>(
    peer_image_v1: &File, admission_v1: &mut A, trust_root_v1: &Path,
    anchor_v1: &Path, owner_uid_v1: u32,
    use_verified_v1: impl FnOnce(&VerifiedInstalledServerPeerV1<A::MemoryLeaseV1>, &mut A) -> R,
) -> Result<R, InstalledArtifactPeerErrorV1<A::RefusalV1>>
where A: InstalledArtifactTrustAdmissionV1,
{
    with_verified_installed_server_peer_at_v1(
        peer_image_v1, admission_v1, trust_root_v1, anchor_v1, owner_uid_v1, use_verified_v1,
    )
}

#[cfg(all(target_os = "linux", feature = "s-product-startup-v1"))]
fn verify_at_v1(
    trust_root_v1: &Path,
    anchor_v1: &Path,
    owner_uid_v1: u32,
) -> Result<VerifiedInstalledArtifactsV2, InstalledTrustErrorV1> {
    let mut admission_v1 = UnmeteredInstalledAdmissionV1;
    let mut context_v1 = VerifyContextV1 {
        admission_v1: &mut admission_v1,
        leases_v1: Vec::new(),
        refusal_v1: None,
    };
    let selected_v1 = verify_signed_install_v1(&mut context_v1, trust_root_v1, anchor_v1, owner_uid_v1)?;
    let payload_v1 = selected_v1.envelope_v1.get("payload")
        .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?;
    let manifest_payload_v1 = &selected_v1.manifest_payload_v1;
    let manifest_v1 = selected_v1.manifest_v1;
    let manifest_path_v1 = selected_v1.manifest_path_v1;
    let release_root_v1 = selected_v1.release_root_v1;
    let generation_v1 = selected_v1.generation_v1;
    let rotation_lock_v1 = selected_v1.rotation_lock_v1;
    let manifest_digest_v1 = selected_v1.manifest_digest_v1;
    let z3_solver_v1 = if let Some(reference_v1) = manifest_payload_v1.get("z3_solver") {
        if manifest_payload_v1
            .get("schema_version")
            .and_then(Value::as_str)
            != Some("quanta-release-bundle-v4")
        {
            return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
        }
        let fields_v1 = reference_v1
            .as_object()
            .filter(|fields_v1| fields_v1.len() == 5)
            .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?;
        for field_name_v1 in [
            "relative_path",
            "sha256",
            "size_bytes",
            "reported_version",
            "closure_policy",
        ] {
            if !fields_v1.contains_key(field_name_v1) {
                return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
            }
        }
        if field_v1(reference_v1, "relative_path")? != "libexec/quanta/z3-static"
            || field_v1(reference_v1, "closure_policy")? != "linux-x86_64-static-elf64-exec/v1"
        {
            return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
        }
        let digest_v1 = field_v1(reference_v1, "sha256")?;
        let _ = hex_bytes_v1::<32>(digest_v1)?;
        let size_v1 = reference_v1
            .get("size_bytes")
            .and_then(Value::as_u64)
            .filter(|size_v1| (64..=1024 * 1024 * 1024).contains(size_v1))
            .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?;
        let version_v1 = field_v1(reference_v1, "reported_version")?;
        if !version_v1.starts_with("Z3 version ")
            || version_v1.len() > 128
            || !version_v1
                .bytes()
                .all(|byte_v1| (0x20..=0x7e).contains(&byte_v1))
        {
            return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
        }
        let image_v1 = opened_protected_file_v1(
            anchor_v1,
            &release_root_v1.join("libexec/quanta/z3-static"),
            owner_uid_v1,
            true,
        )?;
        if image_v1
            .metadata()
            .map_err(|_| InstalledTrustErrorV1::Io)?
            .len()
            != size_v1
            || digest_file_v1(&image_v1, 1024 * 1024 * 1024)? != digest_v1
        {
            return Err(InstalledTrustErrorV1::ChangedImage);
        }
        quanta_explicit_fd_process::verify_static_linux_x86_64_elf_v1(&image_v1)
            .map_err(|_| InstalledTrustErrorV1::InvalidActiveGeneration)?;
        Some((image_v1, digest_v1.to_owned(), version_v1.to_owned()))
    } else {
        None
    };
    let roles_v1 = [
        ("bin/quanta", "cli_sha256"),
        ("libexec/quanta/quanta-cli-server", "server_sha256"),
        ("libexec/quanta/quanta-sql-rulepack-worker", "sql_rulepack_worker_sha256"),
        ("libexec/quanta/quanta_native_worker_v3", "parser_native_worker_sha256"),
    ];
    let mut images_v1 = Vec::new();
    for (relative_v1, digest_field_v1) in roles_v1 {
        let path_v1 = release_root_v1.join(relative_v1);
        let file_v1 = opened_protected_file_v1(anchor_v1, &path_v1, owner_uid_v1, true)?;
        let expected_v1 = field_v1(payload_v1, digest_field_v1)?;
        let _ = hex_bytes_v1::<32>(expected_v1)?;
        if digest_file_v1(&file_v1, 2 * 1024 * 1024 * 1024)? != expected_v1 {
            return Err(InstalledTrustErrorV1::ChangedImage);
        }
        images_v1.push((path_v1, file_v1));
    }
    let mut iter_v1 = images_v1.into_iter();
    let (_, cli_image_v2) = iter_v1
        .next()
        .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?;
    let (server_path_v1, server_image_v1) = iter_v1
        .next()
        .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?;
    let (sql_worker_path_v1, sql_worker_image_v1) = iter_v1
        .next()
        .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?;
    let (parser_worker_path_v1, parser_worker_image_v1) = iter_v1
        .next()
        .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?;
    let (z3_solver_image_v1, z3_solver_sha256_v1, z3_solver_reported_version_v1) =
        match z3_solver_v1 {
            Some((image_v1, digest_v1, version_v1)) => {
                (Some(image_v1), Some(digest_v1), Some(version_v1))
            }
            None => (None, None, None),
        };
    Ok(VerifiedInstalledArtifactsV2 {
        cli_image_v2,
        generation_v1,
        release_manifest_sha256_v1: manifest_digest_v1.to_owned(),
        server_path_v1,
        server_sha256_v1: field_v1(payload_v1, "server_sha256")?.to_owned(),
        sql_worker_path_v1,
        sql_worker_sha256_v1: field_v1(payload_v1, "sql_rulepack_worker_sha256")?.to_owned(),
        parser_worker_path_v1,
        parser_worker_sha256_v1: field_v1(payload_v1, "parser_native_worker_sha256")?.to_owned(),
        server_image_v1,
        release_manifest_image_v1: manifest_v1,
        release_manifest_path_v1: manifest_path_v1,
        sql_worker_image_v1,
        parser_worker_image_v1,
        z3_solver_image_v1,
        z3_solver_sha256_v1,
        z3_solver_reported_version_v1,
        release_root_v1,
        release_manifest_digest_v1: manifest_digest_v1.to_owned(),
        owner_uid_v1,
        _rotation_lock_v1: rotation_lock_v1,
    })
}

#[cfg(all(target_os = "linux", feature = "s-product-startup-v1"))]
fn verify_full_role_v1(
    trust_root_v1: &Path, anchor_v1: &Path, owner_uid_v1: u32,
    role_v1: InstalledRoleV1, running_image_v1: &File,
) -> Result<VerifiedInstalledArtifactsV2, InstalledTrustErrorV1> {
    let artifacts_v1 = verify_at_v1(trust_root_v1, anchor_v1, owner_uid_v1)?;
    let selected_v1 = match role_v1 {
        InstalledRoleV1::Cli => &artifacts_v1.cli_image_v2,
        InstalledRoleV1::Server => &artifacts_v1.server_image_v1,
    };
    if !same_image_v1(
        &running_image_v1.metadata().map_err(|_| InstalledTrustErrorV1::Io)?,
        &selected_v1.metadata().map_err(|_| InstalledTrustErrorV1::Io)?,
    ) {
        return Err(InstalledTrustErrorV1::ForeignImage);
    }
    Ok(artifacts_v1)
}

#[cfg(all(target_os = "linux", feature = "s-product-startup-v1"))]
pub fn verify_full_installed_product_v1(
    role_v1: InstalledRoleV1, running_image_v1: &File,
) -> Result<VerifiedInstalledArtifactsV2, InstalledTrustErrorV1> {
    verify_full_role_v1(Path::new(TRUST_ROOT_V1), Path::new("/"), 0, role_v1, running_image_v1)
}

#[cfg(all(target_os = "linux", feature = "s-product-startup-v1"))]
pub fn verify_full_installed_artifacts_v1() -> Result<VerifiedInstalledArtifactsV2, InstalledTrustErrorV1> {
    verify_at_v1(Path::new(TRUST_ROOT_V1), Path::new("/"), 0)
}

#[cfg(all(target_os = "linux", feature = "fixture-trust-root-v1", feature = "s-product-startup-v1"))]
pub fn verify_full_installed_product_fixture_v1(
    trust_root_v1: &Path, anchor_v1: &Path, owner_uid_v1: u32,
    role_v1: InstalledRoleV1, running_image_v1: &File,
) -> Result<VerifiedInstalledArtifactsV2, InstalledTrustErrorV1> {
    verify_full_role_v1(trust_root_v1, anchor_v1, owner_uid_v1, role_v1, running_image_v1)
}

#[cfg(all(target_os = "linux", feature = "fixture-trust-root-v1", feature = "s-product-startup-v1"))]
pub fn verify_full_installed_artifacts_fixture_v1(
    trust_root_v1: &Path, anchor_v1: &Path, owner_uid_v1: u32,
) -> Result<VerifiedInstalledArtifactsV2, InstalledTrustErrorV1> {
    verify_at_v1(trust_root_v1, anchor_v1, owner_uid_v1)
}

#[cfg(all(target_os = "linux", feature = "s-product-startup-v1"))]
pub mod s_build_context_helpers_v1 {
    use super::*;

    pub fn opened_protected_file_v1(
        anchor_v1: &Path, path_v1: &Path, owner_uid_v1: u32, executable_v1: bool,
    ) -> Result<File, InstalledTrustErrorV1> {
        super::opened_protected_file_v1(anchor_v1, path_v1, owner_uid_v1, executable_v1)
    }
    pub fn bounded_read_v1(file_v1: &File, limit_v1: u64) -> Result<Vec<u8>, InstalledTrustErrorV1> {
        super::bounded_read_v1(file_v1, limit_v1)
    }
    pub fn sha256_v1(bytes_v1: &[u8]) -> String { super::sha256_v1(bytes_v1) }
    pub fn hex_bytes_v1<const N: usize>(value_v1: &str) -> Result<[u8; N], InstalledTrustErrorV1> {
        super::hex_bytes_v1::<N>(value_v1)
    }
    pub fn field_v1<'a>(payload_v1: &'a Value, name_v1: &str) -> Result<&'a str, InstalledTrustErrorV1> {
        super::field_v1(payload_v1, name_v1)
    }
    pub fn canonical_value_v1(value_v1: &Value) -> Result<Vec<u8>, InstalledTrustErrorV1> {
        super::canonical_value_v1(value_v1)
    }
    pub fn digest_file_v1(file_v1: &File, limit_v1: u64) -> Result<String, InstalledTrustErrorV1> {
        super::digest_file_v1(file_v1, limit_v1)
    }
}

#[cfg(all(test, target_os = "linux", feature = "fixture-trust-root-v1"))]
mod tests;
