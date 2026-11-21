//! Pinned external Legacy searchd input for maintained migration tests.

use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, Metadata};
use std::io::{Read as _, Write};
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};

#[cfg(target_os = "linux")]
use nix::fcntl::{FcntlArg, SealFlag, fcntl};
use nix::fcntl::{OFlag, open};
#[cfg(target_os = "linux")]
use nix::sys::memfd::{MFdFlags, memfd_create};
use nix::sys::stat::Mode;

use quanta_index_searchd::app::SemanticEmbedderProfile;
use quanta_index_searchd::app::config::{
    OpenAiEmbedderTuning, ProviderEgressGrantConfig, ProviderWorkBudgetConfig,
};
use sha2::{Digest as _, Sha256};

use super::apply_searchd_env;
use super::environment::searchd_env;

const BINARY_ENV: &str = "QUANTA_INDEX_LEGACY_SEARCHD_BIN";
const SHA256_ENV: &str = "QUANTA_INDEX_LEGACY_SEARCHD_SHA256";
pub(crate) struct RelevanceProviderInputs {
    pub(crate) grant: ProviderEgressGrantConfig,
    pub(crate) budget: ProviderWorkBudgetConfig,
}

impl RelevanceProviderInputs {
    pub(crate) fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            grant: ProviderEgressGrantConfig::from_env()?,
            budget: ProviderWorkBudgetConfig::from_env()?,
        })
    }
}

#[derive(Debug)]
pub struct LegacySearchdBinary {
    path: PathBuf,
    sha256: String,
    identity: FileIdentity,
    launch: LaunchExecutable,
}

#[derive(Debug)]
enum LaunchExecutable {
    #[cfg(target_os = "linux")]
    Sealed(File),
    #[cfg(target_os = "macos")]
    PrivateFile {
        _directory: tempfile::TempDir,
        path: PathBuf,
        identity: FileIdentity,
    },
}

/// A command whose executable custody remains live through spawn.
pub struct LegacySearchdCommand<'binary> {
    binary: &'binary LegacySearchdBinary,
    command: Command,
}

impl LegacySearchdCommand<'_> {
    pub fn arg(&mut self, argument: impl AsRef<OsStr>) -> &mut Self {
        let _configured = self.command.arg(argument);
        self
    }

    pub fn args<I, S>(&mut self, arguments: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let _configured = self.command.args(arguments);
        self
    }

    pub fn env(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Self {
        let _configured = self.command.env(key, value);
        self
    }

    pub fn envs<I, K, V>(&mut self, environment: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        let _configured = self.command.envs(environment);
        self
    }

    pub fn env_clear(&mut self) -> &mut Self {
        let _configured = self.command.env_clear();
        self
    }

    pub fn env_remove(&mut self, key: impl AsRef<OsStr>) -> &mut Self {
        let _configured = self.command.env_remove(key);
        self
    }

    pub fn stdin(&mut self, stdin: impl Into<Stdio>) -> &mut Self {
        let _configured = self.command.stdin(stdin);
        self
    }

    pub fn stdout(&mut self, stdout: impl Into<Stdio>) -> &mut Self {
        let _configured = self.command.stdout(stdout);
        self
    }

    pub fn stderr(&mut self, stderr: impl Into<Stdio>) -> &mut Self {
        let _configured = self.command.stderr(stderr);
        self
    }

    pub fn get_envs(&self) -> impl Iterator<Item = (&OsStr, Option<&OsStr>)> {
        self.command.get_envs()
    }

    pub fn apply_searchd_env(&mut self, max_generations: usize) -> &mut Self {
        self.envs(searchd_env(max_generations))
    }

    pub fn spawn(&mut self) -> Result<Child, Box<dyn Error>> {
        self.binary.verify_launch_custody()?;
        let mut child = self.command.spawn()?;
        if let Err(error) = self.binary.verify_launch_custody() {
            drop(child.kill());
            drop(child.wait());
            return Err(error);
        }
        Ok(child)
    }

    pub fn output(&mut self) -> Result<Output, Box<dyn Error>> {
        self.binary.verify_launch_custody()?;
        let output = self.command.output()?;
        self.binary.verify_launch_custody()?;
        Ok(output)
    }

    pub fn status(&mut self) -> Result<ExitStatus, Box<dyn Error>> {
        self.binary.verify_launch_custody()?;
        let status = self.command.status()?;
        self.binary.verify_launch_custody()?;
        Ok(status)
    }
}

/// An ordinary, private, digest-verified file for the path-based installer.
/// Its directory stays live while a selection references the path.
pub struct VerifiedInstallerBinary {
    _directory: tempfile::TempDir,
    path: PathBuf,
    identity: FileIdentity,
    source_identity: FileIdentity,
    sha256: String,
}

impl VerifiedInstallerBinary {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn verify(&self) -> Result<(), Box<dyn Error>> {
        let (identity, sha256) = inspect(&self.path)?;
        if identity != self.identity || sha256 != self.sha256 {
            return Err("Legacy installer executable changed".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    length: u64,
    mode: u32,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

impl FileIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            mode: metadata.mode(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }
}

fn open_source(path: &Path) -> Result<(FileIdentity, File), Box<dyn Error>> {
    let path_metadata = fs::symlink_metadata(path)?;
    if !path_metadata.file_type().is_file() || path_metadata.mode() & 0o111 == 0 {
        return Err("Legacy executable must be a regular executable file, not a symlink".into());
    }
    let expected_identity = FileIdentity::from_metadata(&path_metadata);
    // O_NONBLOCK prevents a replaced FIFO from stalling before the metadata
    // comparison; O_NOFOLLOW rejects a final-component symlink replacement.
    let file = File::from(open(
        path,
        OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?);
    if FileIdentity::from_metadata(&file.metadata()?) != expected_identity {
        return Err("Legacy executable changed while opening".into());
    }
    Ok((expected_identity, file))
}

fn copy_verified(
    source: &Path,
    destination: &mut File,
    expected_sha256: &str,
) -> Result<FileIdentity, Box<dyn Error>> {
    let (identity, sha256) = inspect_into(source, destination)?;
    if sha256 != expected_sha256 {
        return Err("Legacy executable SHA-256 differs from the declared input".into());
    }
    Ok(identity)
}

fn inspect(path: &Path) -> Result<(FileIdentity, String), Box<dyn Error>> {
    inspect_into(path, std::io::sink())
}

fn inspect_into(
    path: &Path,
    mut destination: impl Write,
) -> Result<(FileIdentity, String), Box<dyn Error>> {
    let (identity, mut file) = open_source(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
        destination.write_all(&buffer[..read])?;
    }
    if FileIdentity::from_metadata(&file.metadata()?) != identity
        || FileIdentity::from_metadata(&fs::symlink_metadata(path)?) != identity
    {
        return Err("Legacy executable changed while hashing".into());
    }
    Ok((identity, format!("{:x}", hash.finalize())))
}

fn private_snapshot(path: &Path, sha256: &str) -> Result<VerifiedInstallerBinary, Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = crate::private_tempdir()?;
    let snapshot = directory.path().join("legacy-searchd");
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&snapshot)?;
    let source_identity = copy_verified(path, &mut output, sha256)?;
    output.set_permissions(fs::Permissions::from_mode(0o500))?;
    output.sync_all()?;
    let (identity, actual_sha256) = inspect(&snapshot)?;
    if actual_sha256 != sha256 {
        return Err("Legacy private executable SHA-256 changed".into());
    }
    Ok(VerifiedInstallerBinary {
        _directory: directory,
        path: snapshot,
        identity,
        source_identity,
        sha256: sha256.to_owned(),
    })
}

#[cfg(target_os = "linux")]
fn sealed_executable(path: &Path, sha256: &str) -> Result<(FileIdentity, File), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt as _;

    let descriptor = memfd_create(
        "quanta-legacy-searchd",
        MFdFlags::MFD_CLOEXEC | MFdFlags::MFD_ALLOW_SEALING,
    )?;
    let mut sealed = File::from(descriptor);
    let identity = copy_verified(path, &mut sealed, sha256)?;
    sealed.set_permissions(fs::Permissions::from_mode(0o500))?;
    sealed.sync_all()?;
    let required = SealFlag::F_SEAL_WRITE
        | SealFlag::F_SEAL_GROW
        | SealFlag::F_SEAL_SHRINK
        | SealFlag::F_SEAL_SEAL;
    fcntl(&sealed, FcntlArg::F_ADD_SEALS(required))?;
    verify_sealed(&sealed, identity.length, required)?;
    Ok((identity, sealed))
}

#[cfg(target_os = "linux")]
fn verify_sealed(sealed: &File, length: u64, required: SealFlag) -> Result<(), Box<dyn Error>> {
    let actual = SealFlag::from_bits_truncate(fcntl(sealed, FcntlArg::F_GET_SEALS)?);
    let metadata = sealed.metadata()?;
    if !actual.contains(required) || metadata.len() != length || metadata.mode() & 0o777 != 0o500 {
        return Err("Legacy sealed executable custody changed".into());
    }
    Ok(())
}

impl LegacySearchdBinary {
    pub fn from_environment() -> Result<Self, Box<dyn Error>> {
        Self::from_values(std::env::var_os(BINARY_ENV), std::env::var_os(SHA256_ENV))
    }

    fn from_values(
        binary: Option<OsString>,
        sha256: Option<OsString>,
    ) -> Result<Self, Box<dyn Error>> {
        let binary = binary.ok_or_else(|| format!("missing {BINARY_ENV}"))?;
        let sha256 = sha256.ok_or_else(|| format!("missing {SHA256_ENV}"))?;
        let sha256 = sha256
            .into_string()
            .map_err(|_| format!("{SHA256_ENV} must be UTF-8"))?;
        Self::from_path_and_sha(Path::new(&binary), &sha256)
    }

    pub fn from_path_and_sha(path: &Path, sha256: &str) -> Result<Self, Box<dyn Error>> {
        if !path.is_absolute() {
            return Err("Legacy executable path must be absolute".into());
        }
        if sha256.len() != 64
            || !sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("Legacy executable SHA-256 must be 64 lowercase hex characters".into());
        }
        #[cfg(target_os = "linux")]
        let (identity, launch) = {
            let (identity, sealed) = sealed_executable(path, sha256)?;
            (identity, LaunchExecutable::Sealed(sealed))
        };
        #[cfg(target_os = "macos")]
        let (identity, launch) = {
            let snapshot = private_snapshot(path, sha256)?;
            (
                snapshot.source_identity,
                LaunchExecutable::PrivateFile {
                    _directory: snapshot._directory,
                    path: snapshot.path,
                    identity: snapshot.identity,
                },
            )
        };
        Ok(Self {
            path: path.to_path_buf(),
            sha256: sha256.to_owned(),
            identity,
            launch,
        })
    }

    /// Digest of the executable pinned at admission.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub fn verify(&self) -> Result<(), Box<dyn Error>> {
        let (identity, sha256) = inspect(&self.path)?;
        if identity != self.identity || sha256 != self.sha256 {
            return Err("Legacy executable identity or SHA-256 changed".into());
        }
        self.verify_launch_custody()
    }

    pub(super) fn verify_launch_custody(&self) -> Result<(), Box<dyn Error>> {
        if FileIdentity::from_metadata(&fs::symlink_metadata(&self.path)?) != self.identity {
            return Err("Legacy executable identity or SHA-256 changed".into());
        }
        // Hash the verified copy at construction. Launch rechecks its seal or
        // private file identity without hashing the original for every spawn.
        match &self.launch {
            #[cfg(target_os = "linux")]
            LaunchExecutable::Sealed(file) => {
                let required = SealFlag::F_SEAL_WRITE
                    | SealFlag::F_SEAL_GROW
                    | SealFlag::F_SEAL_SHRINK
                    | SealFlag::F_SEAL_SEAL;
                verify_sealed(file, self.identity.length, required)
            }
            #[cfg(target_os = "macos")]
            LaunchExecutable::PrivateFile { path, identity, .. } => {
                if FileIdentity::from_metadata(&fs::symlink_metadata(path)?) != *identity {
                    return Err("Legacy private executable changed".into());
                }
                Ok(())
            }
        }
    }

    fn launch_path(&self) -> PathBuf {
        match &self.launch {
            #[cfg(target_os = "linux")]
            LaunchExecutable::Sealed(file) => PathBuf::from(format!(
                "/proc/{}/fd/{}",
                std::process::id(),
                file.as_raw_fd()
            )),
            #[cfg(target_os = "macos")]
            LaunchExecutable::PrivateFile { path, .. } => path.clone(),
        }
    }

    pub fn installer_binary(&self) -> Result<VerifiedInstallerBinary, Box<dyn Error>> {
        self.verify_launch_custody()?;
        let installer = private_snapshot(&self.path, &self.sha256)?;
        if installer.source_identity != self.identity {
            return Err("Legacy installer source identity changed".into());
        }
        self.verify_launch_custody()?;
        Ok(installer)
    }

    pub fn command(&self) -> Result<LegacySearchdCommand<'_>, Box<dyn Error>> {
        self.verify_launch_custody()?;
        Ok(LegacySearchdCommand {
            binary: self,
            command: Command::new(self.launch_path()),
        })
    }

    /// Configure a reproducible Legacy benchmark fixture without inheriting
    /// deployment knobs. Keep the host environment used for the socket paths.
    pub fn benchmark_command(
        &self,
        state_root: &Path,
        history_max_generations: usize,
    ) -> Result<LegacySearchdCommand<'_>, Box<dyn Error>> {
        let mut command = self.command()?;
        for (name, _) in std::env::vars_os() {
            if name.as_encoded_bytes().starts_with(b"QUANTA_INDEX_") {
                let _configured = command.env_remove(name);
            }
        }
        let _configured = command
            .arg("serve")
            .arg("--state-root")
            .arg(state_root)
            .apply_searchd_env(history_max_generations)
            .env(
                "QUANTA_INDEX_EMBED_DIM",
                quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_DIMENSION.to_string(),
            );
        Ok(command)
    }

    /// Run the corpus producer and query embedder under the same explicitly
    /// selected profile, with the same isolation as other reference benches.
    pub(crate) fn relevance_benchmark_command(
        &self,
        state_root: &Path,
        history_max_generations: usize,
        profile: &SemanticEmbedderProfile,
        provider_inputs: Option<&RelevanceProviderInputs>,
    ) -> anyhow::Result<LegacySearchdCommand<'_>> {
        let mut command = self
            .benchmark_command(state_root, history_max_generations)
            .map_err(|error| anyhow::anyhow!("configure Legacy relevance reference: {error}"))?;
        let _configured = command
            .env("QUANTA_INDEX_EMBEDDER", profile.selector())
            .env_remove("OPENAI_API_KEY");
        match profile {
            SemanticEmbedderProfile::Hash { dimension } => {
                let _configured = command.env("QUANTA_INDEX_EMBED_DIM", dimension.to_string());
            }
            SemanticEmbedderProfile::PotionCode { model_dir, .. } => {
                let _configured = command.env("QUANTA_INDEX_EMBED_MODEL_DIR", model_dir);
            }
            SemanticEmbedderProfile::LocalCode { binding, tuning } => {
                let _configured = command
                    .env("QUANTA_INDEX_LOCAL_CODE_ORIGIN", binding.origin())
                    .env("QUANTA_INDEX_LOCAL_CODE_MANIFEST", binding.manifest_path())
                    .env(
                        "QUANTA_INDEX_LOCAL_CODE_MANIFEST_SHA256",
                        binding.manifest_sha256(),
                    );
                configure_relevance_provider(&mut command, tuning, provider_inputs)?;
            }
            SemanticEmbedderProfile::OpenAi {
                model,
                model_revision,
                dimension,
                api_key,
                tuning,
            } => {
                let _configured = command
                    .env("QUANTA_INDEX_EMBED_MODEL", model)
                    .env("QUANTA_INDEX_EMBED_MODEL_REVISION", model_revision)
                    .env("QUANTA_INDEX_EMBED_DIM", dimension.to_string())
                    .env("OPENAI_API_KEY", api_key);
                configure_relevance_provider(&mut command, tuning, provider_inputs)?;
            }
            SemanticEmbedderProfile::Unavailable => {
                anyhow::bail!("Legacy relevance requires an available semantic embedder");
            }
        }
        Ok(command)
    }

    pub fn command_under_umask(
        &self,
        state_root: &Path,
        umask: &str,
        history_max_generations: usize,
    ) -> Result<LegacySearchdCommand<'_>, Box<dyn Error>> {
        self.verify_launch_custody()?;
        let mut command = Command::new("/bin/sh");
        let _configured = command
            .arg("-c")
            .arg(r#"umask "$1" && exec "$2" serve --state-root "$3""#)
            .arg("umask-hardening")
            .arg(umask)
            .arg(self.launch_path())
            .arg(state_root);
        apply_searchd_env(&mut command, history_max_generations);
        Ok(LegacySearchdCommand {
            binary: self,
            command,
        })
    }
}

fn configure_relevance_provider(
    command: &mut LegacySearchdCommand<'_>,
    tuning: &OpenAiEmbedderTuning,
    inputs: Option<&RelevanceProviderInputs>,
) -> anyhow::Result<()> {
    let ambient;
    let inputs = match inputs {
        Some(inputs) => inputs,
        None => {
            ambient = RelevanceProviderInputs::from_env()?;
            &ambient
        }
    };
    let RelevanceProviderInputs { grant, budget } = inputs;
    let cache = &tuning.cache_retention;
    anyhow::ensure!(
        tuning.timeout.subsec_nanos() == 0 && cache.max_entry_age().subsec_nanos() == 0,
        "Legacy relevance provider timeout and cache age require whole seconds"
    );
    let _configured = command.envs([
        ("QUANTA_INDEX_PROVIDER_TENANT", grant.tenant_id.clone()),
        ("QUANTA_INDEX_PROVIDER_ENDPOINT", grant.endpoint.clone()),
        ("QUANTA_INDEX_PROVIDER_REGION", grant.region.clone()),
        ("QUANTA_INDEX_PROVIDER_RETENTION", grant.retention.clone()),
        ("QUANTA_INDEX_PROVIDER_PROFILE", grant.profile.clone()),
        (
            "QUANTA_INDEX_PROVIDER_SOURCE_CONTENT_CONSENT",
            grant.source_content_consent.to_string(),
        ),
        ("QUANTA_INDEX_EMBED_BATCH", tuning.max_batch.to_string()),
        (
            "QUANTA_INDEX_EMBED_MAX_EST_TOKENS",
            tuning.max_estimated_tokens_per_request.to_string(),
        ),
        (
            "QUANTA_INDEX_EMBED_MAX_RETRIES",
            tuning.max_retries.to_string(),
        ),
        (
            "QUANTA_INDEX_EMBED_TIMEOUT_SECS",
            tuning.timeout.as_secs().to_string(),
        ),
        ("QUANTA_INDEX_EMBED_CACHE", tuning.cache_enabled.to_string()),
        (
            "QUANTA_INDEX_EMBED_CONCURRENCY",
            tuning.concurrency.to_string(),
        ),
        (
            "QUANTA_INDEX_EMBED_CACHE_MAX_ENTRIES",
            cache.max_entries().to_string(),
        ),
        (
            "QUANTA_INDEX_EMBED_CACHE_MAX_BYTES",
            cache.max_resident_bytes().to_string(),
        ),
        (
            "QUANTA_INDEX_EMBED_CACHE_MAX_NAMESPACES",
            cache.max_namespaces().to_string(),
        ),
        (
            "QUANTA_INDEX_EMBED_CACHE_MAX_AGE_SECS",
            cache.max_entry_age().as_secs().to_string(),
        ),
        (
            "QUANTA_INDEX_EMBED_CACHE_MAX_TOTAL_BYTES",
            cache.max_total_bytes().to_string(),
        ),
        (
            "QUANTA_INDEX_PROVIDER_MAX_INFLIGHT_REQUESTS",
            budget.inflight_requests_cap.to_string(),
        ),
        (
            "QUANTA_INDEX_PROVIDER_MAX_INFLIGHT_BYTES",
            budget.inflight_bytes_cap.to_string(),
        ),
        (
            "QUANTA_INDEX_PROVIDER_COST_CEILING_MICROS",
            budget.total_cost_micros_ceiling.to_string(),
        ),
        (
            "QUANTA_INDEX_PROVIDER_MAX_RETRY_ATTEMPTS",
            budget.retry_attempts_cap.to_string(),
        ),
    ]);
    if let Some(quote) = budget.openai_max_request_cost_micros {
        let _configured = command.env(
            "QUANTA_INDEX_PROVIDER_OPENAI_MAX_REQUEST_COST_MICROS",
            quote.to_string(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use std::os::unix::fs::{PermissionsExt as _, symlink};
    use std::path::Path;

    use sha2::{Digest as _, Sha256};

    use super::{LegacySearchdBinary, SemanticEmbedderProfile};

    fn executable(path: &Path, bytes: &[u8]) -> Result<String, Box<dyn std::error::Error>> {
        fs::write(path, bytes)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    #[test]
    fn accepts_regular_executable_and_rechecks_its_bytes() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("old-searchd");
        let sha = executable(&path, b"#!/bin/sh\nexit 0\n")?;
        let pinned = LegacySearchdBinary::from_path_and_sha(&path, &sha)?;
        pinned.verify()?;
        Ok(())
    }

    #[test]
    fn relevance_reference_uses_selected_model_and_removes_ambient_provider_key()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("old-searchd");
        let sha = executable(&path, b"#!/bin/sh\nexit 0\n")?;
        let pinned = LegacySearchdBinary::from_path_and_sha(&path, &sha)?;
        let model_dir = temp.path().join("model");
        let profile = SemanticEmbedderProfile::PotionCode {
            model_dir: model_dir.clone(),
            encoding: quanta_index_embed::PotionCodeEncodingPolicy::FullLengthV2,
        };
        let command = pinned.relevance_benchmark_command(temp.path(), 3, &profile, None)?;
        let env: std::collections::BTreeMap<_, _> = command.get_envs().collect();
        assert_eq!(
            env.get(OsStr::new("QUANTA_INDEX_EMBEDDER")).copied(),
            Some(Some(OsStr::new("potion-code-full-v2")))
        );
        assert_eq!(
            env.get(OsStr::new("QUANTA_INDEX_EMBED_MODEL_DIR")).copied(),
            Some(Some(model_dir.as_os_str()))
        );
        assert_eq!(env.get(OsStr::new("OPENAI_API_KEY")), Some(&None));
        let command = pinned.relevance_benchmark_command(
            temp.path(),
            3,
            &SemanticEmbedderProfile::Hash { dimension: 128 },
            None,
        )?;
        let env: std::collections::BTreeMap<_, _> = command.get_envs().collect();
        assert_eq!(
            env.get(OsStr::new("QUANTA_INDEX_EMBEDDER")).copied(),
            Some(Some(OsStr::new("hash-dev")))
        );
        assert_eq!(
            env.get(OsStr::new("QUANTA_INDEX_EMBED_DIM")).copied(),
            Some(Some(OsStr::new("128")))
        );
        Ok(())
    }

    #[test]
    fn relevance_external_provider_passes_the_frozen_grant_and_work_caps()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("old-searchd");
        let sha = executable(&path, b"#!/bin/sh\nexit 0\n")?;
        let pinned = LegacySearchdBinary::from_path_and_sha(&path, &sha)?;
        let inputs = super::RelevanceProviderInputs {
            grant: super::ProviderEgressGrantConfig {
                tenant_id: "approved-tenant".into(),
                endpoint: "https://api.openai.com/v1/embeddings".into(),
                source_content_consent: true,
                ..Default::default()
            },
            budget: super::ProviderWorkBudgetConfig {
                inflight_requests_cap: 5,
                inflight_bytes_cap: 8192,
                total_cost_micros_ceiling: 999,
                retry_attempts_cap: 1,
                openai_max_request_cost_micros: std::num::NonZeroU64::new(20),
            },
        };
        let command = pinned.relevance_benchmark_command(
            temp.path(),
            3,
            &SemanticEmbedderProfile::OpenAi {
                model: "fixture-model".into(),
                model_revision: "fixture-revision".into(),
                dimension: 8,
                api_key: "fixture-key".into(),
                tuning: Default::default(),
            },
            Some(&inputs),
        )?;
        let supplied = [
            ("QUANTA_INDEX_PROVIDER_MAX_INFLIGHT_REQUESTS", "5"),
            ("QUANTA_INDEX_PROVIDER_MAX_INFLIGHT_BYTES", "8192"),
            ("QUANTA_INDEX_PROVIDER_COST_CEILING_MICROS", "999"),
            ("QUANTA_INDEX_PROVIDER_MAX_RETRY_ATTEMPTS", "1"),
            ("QUANTA_INDEX_PROVIDER_OPENAI_MAX_REQUEST_COST_MICROS", "20"),
            ("QUANTA_INDEX_PROVIDER_TENANT", "approved-tenant"),
            (
                "QUANTA_INDEX_PROVIDER_ENDPOINT",
                "https://api.openai.com/v1/embeddings",
            ),
            ("QUANTA_INDEX_PROVIDER_SOURCE_CONTENT_CONSENT", "true"),
        ];
        let env: std::collections::BTreeMap<_, _> = command.get_envs().collect();
        for (key, value) in supplied {
            assert_eq!(
                env.get(OsStr::new(key)).copied(),
                Some(Some(OsStr::new(value)))
            );
        }
        assert!(!matches!(
            env.get(OsStr::new("QUANTA_INDEX_PROVIDER_UNRELATED")),
            Some(Some(_))
        ));

        Ok(())
    }

    #[test]
    fn deferred_command_refuses_replaced_source_before_launch()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("old-searchd");
        let marker = temp.path().join("launched");
        let bytes = b"#!/bin/sh\nprintf launched > \"$1\"\n";
        let sha = executable(&path, bytes)?;
        let pinned = LegacySearchdBinary::from_path_and_sha(&path, &sha)?;
        let mut command = pinned.command()?;
        let _configured = command.arg(&marker);
        let replacement = temp.path().join("replacement");
        assert_eq!(executable(&replacement, bytes)?, sha);
        fs::rename(&replacement, &path)?;
        assert!(command.status().is_err());
        assert!(!marker.exists());
        Ok(())
    }

    #[test]
    fn pinned_command_and_installer_snapshot_remain_executable()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("old-searchd");
        let sha = executable(&path, b"#!/bin/sh\nexit 0\n")?;
        let pinned = LegacySearchdBinary::from_path_and_sha(&path, &sha)?;
        #[cfg(target_os = "linux")]
        let super::LaunchExecutable::Sealed(file) = &pinned.launch;
        #[cfg(target_os = "linux")]
        super::verify_sealed(
            file,
            pinned.identity.length,
            nix::fcntl::SealFlag::F_SEAL_WRITE
                | nix::fcntl::SealFlag::F_SEAL_GROW
                | nix::fcntl::SealFlag::F_SEAL_SHRINK
                | nix::fcntl::SealFlag::F_SEAL_SEAL,
        )?;
        assert!(pinned.command()?.status()?.success());
        let installer = pinned.installer_binary()?;
        installer.verify()?;
        assert!(installer.path().is_file());
        assert!(
            std::process::Command::new(installer.path())
                .status()?
                .success()
        );
        Ok(())
    }

    #[test]
    fn rejects_absent_input_and_noncanonical_sha_without_environment_mutation()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("old-searchd");
        let sha = executable(&path, b"#!/bin/sh\nexit 0\n")?;
        assert!(LegacySearchdBinary::from_values(None, Some(OsString::from(&sha))).is_err());
        assert!(LegacySearchdBinary::from_values(Some(path.as_os_str().into()), None).is_err());
        for invalid in [
            "",
            "a",
            &format!("sha256:{sha}"),
            &sha.to_uppercase(),
            &"g".repeat(64),
        ] {
            assert!(LegacySearchdBinary::from_path_and_sha(&path, invalid).is_err());
        }
        assert!(
            LegacySearchdBinary::from_path_and_sha(Path::new("relative-searchd"), &sha).is_err()
        );
        assert!(
            LegacySearchdBinary::from_path_and_sha(&temp.path().join("missing"), &sha).is_err()
        );
        Ok(())
    }

    #[test]
    fn rejects_wrong_hash_nonexecutable_directory_and_symlink()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("old-searchd");
        let sha = executable(&path, b"#!/bin/sh\nexit 0\n")?;
        assert!(LegacySearchdBinary::from_path_and_sha(&path, &"0".repeat(64)).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        assert!(LegacySearchdBinary::from_path_and_sha(&path, &sha).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        assert!(LegacySearchdBinary::from_path_and_sha(temp.path(), &sha).is_err());
        let link = temp.path().join("old-searchd-link");
        symlink(&path, &link)?;
        assert!(LegacySearchdBinary::from_path_and_sha(&link, &sha).is_err());
        Ok(())
    }

    #[test]
    fn rejects_in_place_mutation_and_same_content_path_replacement()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("old-searchd");
        let bytes = b"#!/bin/sh\nexit 0\n";
        let sha = executable(&path, bytes)?;
        let pinned = LegacySearchdBinary::from_path_and_sha(&path, &sha)?;
        fs::write(&path, b"#!/bin/sh\nexit 1\n")?;
        assert!(pinned.verify().is_err());

        let sha = executable(&path, bytes)?;
        let pinned = LegacySearchdBinary::from_path_and_sha(&path, &sha)?;
        let replacement = temp.path().join("replacement");
        let same_sha = executable(&replacement, bytes)?;
        assert_eq!(same_sha, sha);
        fs::rename(&replacement, &path)?;
        assert!(pinned.verify().is_err());
        Ok(())
    }
}
