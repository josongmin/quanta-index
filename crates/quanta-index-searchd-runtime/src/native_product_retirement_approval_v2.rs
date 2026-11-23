//! Selected Q retirement approval consumer. Environment paths are locators;
//! only the fixed installed S trust key and selected BOOT can grant authority.

use super::{Digest, NativeStateCommandV1, ValidatedConfig};
#[cfg(target_os = "linux")]
use super::Sha256;
use anyhow::Result;
#[cfg(target_os = "linux")]
use anyhow::{anyhow, ensure};
#[cfg(not(target_os = "linux"))]
use anyhow::bail;
#[cfg(target_os = "linux")]
use sha2::Digest as _;
#[cfg(target_os = "linux")]
use std::{
    fs::{self, File},
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[cfg(target_os = "linux")]
fn same_file_version(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mode() == right.mode()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

#[cfg(target_os = "linux")]
pub(crate) struct VerifiedApprovalV2 {
    _signed: quanta_installed_artifact_trust::VerifiedNativeQRetirementApprovalV2,
    _deployment_lock: File,
}

#[cfg(not(target_os = "linux"))]
pub(crate) struct VerifiedApprovalV2;

#[cfg(target_os = "linux")]
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(char::from(DIGITS[usize::from(byte >> 4)]));
        result.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    result
}

#[cfg(target_os = "linux")]
fn read_control(path: &Path, maximum: u64) -> Result<Vec<u8>> {
    ensure!(
        path.is_absolute() && fs::canonicalize(path)? == path,
        "retirement control path is not canonical"
    );
    let mut file = File::from(rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?);
    let metadata = file.metadata()?;
    let named = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && same_file_version(&metadata, &named) && metadata.len() <= maximum,
        "retirement control file exceeds bound"
    );
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len())?);
    (&mut file).take(maximum + 1).read_to_end(&mut bytes)?;
    ensure!(
        u64::try_from(bytes.len())? == metadata.len()
            && same_file_version(&metadata, &file.metadata()?)
            && same_file_version(&metadata, &fs::symlink_metadata(path)?),
        "retirement control file changed"
    );
    Ok(bytes)
}

#[cfg(target_os = "linux")]
fn digest_file(path: &Path, maximum: u64) -> Result<String> {
    let selected_image = path != Path::new("/proc/self/exe");
    if selected_image {
        ensure!(
            path.is_absolute() && fs::canonicalize(path)? == path,
            "selected Q image path is not canonical"
        );
    }
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    if selected_image {
        ensure!(
            same_file_version(&metadata, &fs::symlink_metadata(path)?),
            "selected Q image changed"
        );
    }
    ensure!(
        metadata.is_file() && metadata.len() <= maximum,
        "Q image exceeds bound"
    );
    let mut hasher = Sha256::new();
    let mut bytes = [0; 64 * 1024];
    let mut total = 0u64;
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(u64::try_from(count)?)
            .ok_or_else(|| anyhow!("Q image size overflow"))?;
        ensure!(total <= maximum, "Q image exceeds bound");
        hasher.update(&bytes[..count]);
    }
    ensure!(
        total == metadata.len()
            && same_file_version(&metadata, &file.metadata()?)
            && (!selected_image || same_file_version(&metadata, &fs::symlink_metadata(path)?)),
        "Q image changed"
    );
    Ok(hex(&hasher.finalize()))
}

#[cfg(target_os = "linux")]
fn retain_selection_lock(selected_path: &Path) -> Result<File> {
    use fs2::FileExt as _;
    let raw = std::env::var("QUANTA_NATIVE_DEPLOYMENT_LOCK_FD_V2")?.parse::<i32>()?;
    ensure!(
        selected_path.is_absolute(),
        "selected Q path is not absolute"
    );
    let parent = selected_path
        .parent()
        .ok_or_else(|| anyhow!("selected Q parent missing"))?;
    ensure!(
        fs::canonicalize(parent)? == parent,
        "selected Q parent is not canonical"
    );
    let named = parent.join(".quanta-index-deployment.lock");
    let lock = quanta_explicit_fd_process::try_duplicate_inherited_regular_file_v1(raw)?;
    let held = lock.metadata()?;
    ensure!(
        held.is_file()
            && held.uid() == rustix::process::geteuid().as_raw()
            && held.mode() & 0o022 == 0
            && held.nlink() == 1
            && same_file_version(&held, &fs::symlink_metadata(&named)?),
        "inherited deployment lock differs from selected path"
    );
    lock.try_lock_exclusive()?;
    ensure!(
        same_file_version(&held, &lock.metadata()?)
            && same_file_version(&held, &fs::symlink_metadata(&named)?),
        "deployment lock changed"
    );
    Ok(lock)
}

#[cfg(target_os = "linux")]
pub(super) fn verify_selected_v2(
    command: &NativeStateCommandV1,
    config: &ValidatedConfig,
    config_sha256: Digest,
) -> Result<VerifiedApprovalV2> {
    let receipt_path = PathBuf::from(
        std::env::var_os("QUANTA_NATIVE_RETIREMENT_APPROVAL_PATH_V2")
            .ok_or_else(|| anyhow!("retired Native serve requires signed approval locator"))?,
    );
    let decision_path = PathBuf::from(
        std::env::var_os("QUANTA_NATIVE_RETIREMENT_DECISION_POLICY_PATH_V2")
            .ok_or_else(|| anyhow!("retired Native serve requires decision policy locator"))?,
    );
    let signed =
        quanta_installed_artifact_trust::verify_native_q_retirement_approval_v2(&receipt_path)?;
    let payload = signed.payload_v2();
    let field = |name: &str| -> Result<&str> {
        payload
            .get(name)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow!("signed Q approval field {name} differs"))
    };
    let selected_path = PathBuf::from(field("selection_path")?);
    let deployment_lock = retain_selection_lock(&selected_path)?;
    let selected_bytes = read_control(&selected_path, 64 * 1024)?;
    ensure!(
        hex(&Sha256::digest(&selected_bytes)) == field("selection_sha256")?,
        "signed Q approval selection changed"
    );
    let selected: serde_json::Value = serde_json::from_slice(&selected_bytes)?;
    ensure!(
        selected
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            == Some(3)
            && selected.get("mode").and_then(serde_json::Value::as_str) == Some("native"),
        "signed Q approval requires schema-3 Native selection"
    );
    let selected_field = |name: &str| -> Result<&str> {
        selected
            .get(name)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow!("selected Q field {name} differs"))
    };
    ensure!(
        selected_field("state_root")?
            == config
                .state_root
                .to_str()
                .ok_or_else(|| anyhow!("state root is not UTF-8"))?
            && selected_field("state_root")? == field("state_root")?
            && selected_field("native_config")?
                == command
                    .config_path
                    .to_str()
                    .ok_or_else(|| anyhow!("config path is not UTF-8"))?
            && selected_field("native_config_sha256")? == hex(&config_sha256.0)
            && field("config_sha256")? == hex(&config_sha256.0),
        "signed Q approval config or root differs"
    );
    let q_image_sha = field("q_elf_sha256")?;
    ensure!(
        selected_field("binary_sha256")? == q_image_sha
            && digest_file(Path::new(selected_field("binary")?), 2 * 1024 * 1024 * 1024)?
                == q_image_sha
            && digest_file(Path::new("/proc/self/exe"), 2 * 1024 * 1024 * 1024)? == q_image_sha,
        "signed Q approval executable differs"
    );
    let boot = payload
        .get("boot_receipt")
        .ok_or_else(|| anyhow!("signed BOOT receipt missing"))?;
    ensure!(
        selected.get("native_boot_receipt") == Some(boot),
        "signed BOOT receipt differs from selected BOOT"
    );
    let identity = command
        .identity
        .ok_or_else(|| anyhow!("selected BOOT identity missing"))?;
    ensure!(
        boot.get("root_nonce_hex")
            .and_then(serde_json::Value::as_str)
            == Some(hex(&identity.root_nonce).as_str())
            && boot
                .get("manifest_object_hex")
                .and_then(serde_json::Value::as_str)
                == Some(hex(&identity.manifest_object.0.0).as_str())
            && boot
                .get("source_cut_digest_hex")
                .and_then(serde_json::Value::as_str)
                == Some(hex(&identity.source_cut_digest.0).as_str())
            && boot.get("manifest_operation")
                == Some(&serde_json::to_value(identity.manifest_operation)?),
        "signed BOOT identity differs"
    );
    let expected_routes = identity
        .protected_source_routes
        .map(|reference| hex(&reference.0.0));
    ensure!(
        boot.get("protected_source_routes_hex")
            .and_then(serde_json::Value::as_str)
            == expected_routes.as_deref(),
        "signed BOOT Source routes differ"
    );
    let policy = identity
        .retired_projection_policy
        .ok_or_else(|| anyhow!("retired BOOT policy missing"))?;
    let signed_policy = payload
        .get("retired_projection_policy")
        .ok_or_else(|| anyhow!("signed policy missing"))?;
    ensure!(
        boot.get("retired_policy_object_hex")
            .and_then(serde_json::Value::as_str)
            == Some(hex(&policy.object.0.0).as_str())
            && boot
                .get("retired_policy_sha256_hex")
                .and_then(serde_json::Value::as_str)
                == Some(hex(&policy.sha256.0).as_str())
            && boot
                .get("retired_policy_identity_count")
                .and_then(serde_json::Value::as_u64)
                == Some(u64::from(policy.identity_count))
            && signed_policy
                .get("object_hex")
                .and_then(serde_json::Value::as_str)
                == Some(hex(&policy.object.0.0).as_str())
            && signed_policy
                .get("sha256_hex")
                .and_then(serde_json::Value::as_str)
                == Some(hex(&policy.sha256.0).as_str())
            && signed_policy
                .get("identity_count")
                .and_then(serde_json::Value::as_u64)
                == Some(u64::from(policy.identity_count)),
        "signed retirement policy differs"
    );
    ensure!(
        hex(&Sha256::digest(read_control(&decision_path, 1024 * 1024)?))
            == field("decision_policy_sha256")?,
        "signed decision policy changed"
    );
    Ok(VerifiedApprovalV2 {
        _signed: signed,
        _deployment_lock: deployment_lock,
    })
}

#[cfg(not(target_os = "linux"))]
pub(super) fn verify_selected_v2(
    _command: &NativeStateCommandV1,
    _config: &ValidatedConfig,
    _config_sha256: Digest,
) -> Result<VerifiedApprovalV2> {
    bail!("signed Q retirement approval requires Linux installed trust")
}
