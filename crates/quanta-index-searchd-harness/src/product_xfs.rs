//! Provisioned XFS inputs shared by product-daemon proof and scale harnesses.

use std::{
    fs, io,
    os::unix::fs::{DirBuilderExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use anyhow::{Context as _, Result, ensure};
use quanta_index_core::RequestBudgetV1;
use quanta_index_searchd::app::{
    runtime::StateRootLease,
    state_root_quota::{STOCK_SQLITE_TEMP_DIR_NAME, StateRootProjectQuotaV1},
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

/// Ceiling shared by stock product process fixtures.
pub const MAX_PROCESS_FDS: u32 = 4096;
/// Logical scratch reserved inside the provisioned XFS project.
pub const SCRATCH_RESERVED_BYTES: u64 = 1 << 30;

/// Inputs supplied by an already provisioned XFS project and verifier.
pub struct ProductXfsProfile {
    project_parent: PathBuf,
    verifier_socket: PathBuf,
    project_id: u32,
    hard_bytes: u64,
    hard_inodes: u64,
}

impl ProductXfsProfile {
    /// Read the same explicit inputs used by the stock SQLite process fixture.
    pub fn from_env() -> Result<Self> {
        let project_parent = PathBuf::from(
            std::env::var_os("QIDX_NATIVE_XFS_TEST_PARENT")
                .context("missing QIDX_NATIVE_XFS_TEST_PARENT for product daemon proof")?,
        )
        .canonicalize()?;
        ensure!(
            project_parent.is_dir(),
            "XFS project parent is not a directory"
        );
        Self::new(
            project_parent,
            PathBuf::from(
                std::env::var_os("QIDX_NATIVE_XFS_VERIFIER_SOCKET")
                    .context("missing QIDX_NATIVE_XFS_VERIFIER_SOCKET for product daemon proof")?,
            ),
            std::env::var("QIDX_NATIVE_XFS_PROJECT_ID")
                .context("missing QIDX_NATIVE_XFS_PROJECT_ID for product daemon proof")?
                .parse()?,
            std::env::var("QIDX_NATIVE_XFS_HARD_BYTES")
                .context("missing QIDX_NATIVE_XFS_HARD_BYTES for product daemon proof")?
                .parse()?,
            std::env::var("QIDX_NATIVE_XFS_HARD_INODES")
                .context("missing QIDX_NATIVE_XFS_HARD_INODES for product daemon proof")?
                .parse()?,
        )
    }

    fn new(
        project_parent: PathBuf,
        verifier_socket: PathBuf,
        project_id: u32,
        hard_bytes: u64,
        hard_inodes: u64,
    ) -> Result<Self> {
        let _quota = StateRootProjectQuotaV1::new(
            verifier_socket.clone(),
            project_id,
            hard_bytes,
            hard_inodes,
        )?;
        ensure!(
            hard_bytes >= SCRATCH_RESERVED_BYTES,
            "product proof XFS hard quota must fund 1 GiB scratch"
        );
        Ok(Self {
            project_parent,
            verifier_socket,
            project_id,
            hard_bytes,
            hard_inodes,
        })
    }

    /// Own a fresh candidate under the provisioned project; the caller owns its lifetime.
    pub fn tempdir(&self) -> Result<TempDir> {
        let directory = tempfile::Builder::new()
            .prefix("qidx-product-")
            .tempdir_in(&self.project_parent)?;
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))?;
        Ok(directory)
    }

    /// Give stock Catalog an empty, private directory containing only its UDS.
    pub fn service_socket(control: &Path, directory_name: &str) -> Result<PathBuf> {
        ensure!(
            !directory_name.is_empty()
                && directory_name.len() <= 24
                && directory_name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
            "invalid product service socket directory name"
        );
        let control_metadata = fs::symlink_metadata(control)?;
        ensure!(
            control.is_absolute()
                && control_metadata.file_type().is_dir()
                && control_metadata.permissions().mode() & 0o777 == 0o700
                && control.canonicalize()? == control,
            "product service socket control directory must be canonical and private"
        );
        let directory = control.join(directory_name);
        fs::DirBuilder::new().mode(0o700).create(&directory)?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        Ok(directory.join("native.sock"))
    }

    /// Configure the actual writer profile, independently of the fixture's read policy.
    pub fn configure(&self, config: &mut Value) -> Result<()> {
        let object = config
            .as_object()
            .context("product Native config must be an object")?;
        ensure!(
            !object.contains_key("state_root_quota")
                && !object.contains_key("stock_sqlite_max_process_fds"),
            "product Native config already owns a quota or FD profile"
        );
        let resources = config
            .get_mut("resources")
            .and_then(Value::as_object_mut)
            .context("product Native config is missing resources")?;
        let scratch = resources
            .get_mut("scratch_reserved_bytes")
            .context("product Native config is missing scratch_reserved_bytes")?;
        *scratch = json!(SCRATCH_RESERVED_BYTES);
        let object = config
            .as_object_mut()
            .context("product Native config must be an object")?;
        let _quota = object.insert(
            "state_root_quota".into(),
            json!({
                "verifier_socket": self.verifier_socket,
                "project_id": self.project_id,
                "hard_bytes": self.hard_bytes,
                "hard_inodes": self.hard_inodes,
            }),
        );
        let _fds = object.insert(
            "stock_sqlite_max_process_fds".into(),
            json!(MAX_PROCESS_FDS),
        );
        Ok(())
    }

    /// Verify the real kernel project before starting a product writer.
    pub fn prepare_root(&self, root: &Path, socket: &Path) -> Result<PathBuf> {
        let parent = root
            .parent()
            .context("product StateRoot has no parent")?
            .canonicalize()?;
        ensure!(
            parent.starts_with(&self.project_parent),
            "product StateRoot must be below the provisioned XFS project"
        );
        ensure_private_directory(root)?;
        let sqlite_temp = root.join(STOCK_SQLITE_TEMP_DIR_NAME);
        ensure_private_directory(&sqlite_temp)?;
        let lease = StateRootLease::acquire(root)?;
        let quota = StateRootProjectQuotaV1::new(
            self.verifier_socket.clone(),
            self.project_id,
            self.hard_bytes,
            self.hard_inodes,
        )?;
        quota.require(
            &lease,
            socket,
            4,
            &RequestBudgetV1::for_duration(Duration::from_secs(10)),
        )?;
        Ok(sqlite_temp)
    }

    /// Launch through the stock process boundary, including its exact FD cap.
    pub fn command(binary: &Path, sqlite_temp: &Path) -> Command {
        let mut command = Command::new("/bin/sh");
        let _launch = command
            .arg("-c")
            .arg(r#"ulimit -n "$1" && shift && exec "$@""#)
            .arg("qidx-product-xfs-launcher")
            .arg(MAX_PROCESS_FDS.to_string())
            .arg(binary)
            .env("SQLITE_TMPDIR", sqlite_temp);
        command
    }

    /// Exercise the installation writer admission against the same pinned bytes.
    pub fn preflight(binary: &Path, config: &Path, sqlite_temp: &Path) -> Result<()> {
        ensure!(
            binary.is_absolute() && config.is_absolute() && sqlite_temp.is_absolute(),
            "product writer preflight requires absolute executable, config and SQLite temp paths"
        );
        let bytes = fs::read(config)?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let output = Self::command(binary, sqlite_temp)
            .arg("preflight-native-writer")
            .arg("--config")
            .arg(config)
            .arg("--config-digest")
            .arg(digest)
            .output()
            .context("run product Native writer preflight")?;
        ensure!(
            output.status.success(),
            "product Native writer preflight failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => fs::set_permissions(path, fs::Permissions::from_mode(0o700))?,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(path)?;
            ensure!(
                metadata.file_type().is_dir() && metadata.permissions().mode() & 0o777 == 0o700,
                "product XFS directory must be an existing private directory: {}",
                path.display()
            );
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn product_profile_requires_bounded_quota_and_stock_launch_fields() -> Result<()> {
        let profile = ProductXfsProfile::new(
            "/tmp".into(),
            "/run/qidx-verifier.sock".into(),
            17,
            4_u64 << 30,
            4096,
        )?;
        let mut config = serde_json::json!({
            "resources": {"scratch_reserved_bytes": 17_179_869_184_u64}
        });
        profile.configure(&mut config)?;
        assert_eq!(config["resources"]["scratch_reserved_bytes"], 1_u64 << 30);
        assert_eq!(config["stock_sqlite_max_process_fds"], 4096);
        assert_eq!(config["state_root_quota"]["project_id"], 17);
        let pinned = config.clone();
        assert!(profile.configure(&mut config).is_err());
        assert_eq!(config, pinned);
        assert!(
            ProductXfsProfile::new(
                "/tmp".into(),
                "/run/qidx-verifier.sock".into(),
                17,
                SCRATCH_RESERVED_BYTES - 512,
                4096
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn product_directory_refuses_alias_before_changing_target_permissions() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let outside = temp.path().join("outside");
        fs::create_dir(&outside)?;
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o755))?;
        let alias = temp.path().join("state");
        symlink(&outside, &alias)?;
        assert!(ensure_private_directory(&alias).is_err());
        assert_eq!(fs::metadata(&outside)?.permissions().mode() & 0o777, 0o755);
        Ok(())
    }

    #[test]
    fn service_socket_requires_fresh_private_directory_with_no_control_files() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let control = temp.path().canonicalize()?;
        fs::set_permissions(&control, fs::Permissions::from_mode(0o700))?;
        let config = control.join("native.json");
        fs::write(&config, b"config outside uds")?;
        for name in ["", ".", "../escape", "uds/config", "name_with_underscore"] {
            assert!(ProductXfsProfile::service_socket(&control, name).is_err());
        }
        let socket = ProductXfsProfile::service_socket(&control, "uds")?;
        let directory = socket.parent().context("service socket has no parent")?;
        assert_eq!(
            socket.file_name().and_then(|name| name.to_str()),
            Some("native.sock")
        );
        assert_eq!(fs::metadata(directory)?.permissions().mode() & 0o777, 0o700);
        assert!(fs::read_dir(directory)?.next().is_none());
        assert_eq!(fs::read(&config)?.as_slice(), b"config outside uds");
        assert!(ProductXfsProfile::service_socket(&control, "uds").is_err());
        Ok(())
    }

    #[test]
    fn service_socket_rejects_public_or_aliased_control_before_creating_entries() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let control = temp.path().canonicalize()?;
        fs::set_permissions(&control, fs::Permissions::from_mode(0o755))?;
        assert!(ProductXfsProfile::service_socket(&control, "uds").is_err());
        assert!(!control.join("uds").exists());
        fs::set_permissions(&control, fs::Permissions::from_mode(0o700))?;
        let private = control.join("private");
        fs::create_dir(&private)?;
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700))?;
        let alias = private.join("alias");
        symlink(&control, &alias)?;
        assert!(ProductXfsProfile::service_socket(&alias, "uds").is_err());
        assert!(!control.join("uds").exists());
        Ok(())
    }
}
