//! Optional execution-image binding for Native quality captures.
//!
//! This observes the actual connected process; it grants no installed/root or
//! publication authority. Portable captures without this option stay diagnostic.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use quanta_index_ipc::{
    IndexError, KernelConnectedPeerProcessLeaseV1, MemoryLease, MemoryLeaseOwner, MemoryLifetime,
    MemoryScope, MemoryScopePort, RequestBudgetV1,
};
use quanta_index_retrieval_bench::{BenchError, BenchResult, sha256_hex};
use quanta_index_searchd::{SearchdCommand, cli::NativeCommandModeV1};
use serde::{Deserialize, Deserializer, de};
use serde_json::{Value, json};

use super::{Args, required, runner_binary_digest};

const PEER_BYTES: u64 = 4096;

struct PeerCharge(AtomicU64);
struct PeerMemory(Arc<PeerCharge>);

fn charge_bytes(bytes: u64) -> Result<u64, IndexError> {
    bytes
        .checked_add(
            u64::try_from(MemoryLease::allocation_backing_layout()?.size())
                .map_err(|_| IndexError::ResourceLimited)?,
        )
        .ok_or(IndexError::ResourceLimited)
}

impl MemoryLeaseOwner for PeerCharge {
    fn release(&self, bytes: u64) {
        let charge = charge_bytes(bytes).expect("admitted lease layout");
        assert!(
            self.0
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| live
                    .checked_sub(charge))
                .is_ok(),
            "peer grant accounting underflow"
        );
    }
}

impl MemoryScopePort for PeerMemory {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        if bytes == 0 {
            return Err(IndexError::ResourceLimited);
        }
        let charge = charge_bytes(bytes)?;
        let _previous = self
            .0
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                live.checked_add(charge).filter(|next| *next <= PEER_BYTES)
            })
            .map_err(|_| IndexError::ResourceLimited)?;
        Ok(MemoryLease::new(bytes, self.0.clone()))
    }
}

fn refuse(message: impl std::fmt::Display) -> BenchError {
    BenchError::Config(format!("Native execution binding: {message}"))
}

fn absolute_file(raw: &str) -> BenchResult<PathBuf> {
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(refuse("execution input paths must be absolute"));
    }
    path.canonicalize().map_err(refuse)
}

fn regular_file(path: &Path) -> BenchResult<File> {
    use rustix::fs::{Mode, OFlags, open};
    let file = File::from(
        open(
            path,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(refuse)?,
    );
    if !file.metadata().map_err(refuse)?.is_file() {
        return Err(refuse("execution input must be a regular file"));
    }
    Ok(file)
}

fn hash_file(path: &Path) -> BenchResult<String> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let _copied = std::io::copy(&mut regular_file(path)?, &mut hasher).map_err(refuse)?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn bounded_read(path: &Path, limit: u64) -> BenchResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let _read = regular_file(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(refuse)?;
    if u64::try_from(bytes.len()).map_err(refuse)? > limit {
        return Err(refuse("control file exceeds its fixed byte limit"));
    }
    Ok(bytes)
}

fn bytes_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn bound_command(raw: &[u8], config: &Path, digest: &str) -> BenchResult<(String, Option<Value>)> {
    if raw.last() != Some(&0) {
        return Err(refuse("incomplete process command line"));
    }
    let words: Vec<&str> = raw[..raw.len() - 1]
        .split(|byte| *byte == 0)
        .map(std::str::from_utf8)
        .collect::<Result<_, _>>()
        .map_err(refuse)?;
    let parsed =
        SearchdCommand::from_args(words.into_iter().skip(1).map(str::to_owned)).map_err(refuse)?;
    let native = parsed
        .native_command()
        .ok_or_else(|| refuse("peer is not a Native serving command"))?;
    let mode = match native.mode {
        NativeCommandModeV1::Serve => "serve-native",
        NativeCommandModeV1::ServeCandidate => "serve-native-candidate",
        _ => return Err(refuse("peer is not a Native serving command")),
    };
    if native.config_path.canonicalize().map_err(refuse)? != config {
        return Err(refuse("peer --config differs"));
    }
    let actual_digest = native
        .config_digest
        .ok_or_else(|| refuse("peer --config-digest is absent"))?;
    let mut actual_hex = String::with_capacity(64);
    for byte in actual_digest.0 {
        use std::fmt::Write as _;
        write!(&mut actual_hex, "{byte:02x}").map_err(refuse)?;
    }
    if actual_hex != digest {
        return Err(refuse("peer --config-digest differs"));
    }
    let boot_identity = native.identity.map(|identity| {
        json!({
            "root_nonce_hex": bytes_hex(&identity.root_nonce),
            "manifest_operation": identity.manifest_operation,
            "manifest_object_hex": bytes_hex(&identity.manifest_object.0.0),
            "source_cut_digest_hex": bytes_hex(&identity.source_cut_digest.0),
        })
    });
    Ok((mode.to_owned(), boot_identity))
}

struct SocketConfig {
    socket_path: PathBuf,
    state_root: PathBuf,
}

impl<'de> Deserialize<'de> for SocketConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const FIELDS: &[&str] = &["socket_path", "state_root"];

        struct SocketConfigVisitor;

        impl<'de> de::Visitor<'de> for SocketConfigVisitor {
            type Value = SocketConfig;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct SocketConfig")
            }

            fn visit_map<A: de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<SocketConfig, A::Error> {
                let mut socket_path = None;
                let mut state_root = None;
                while let Some(key) = map.next_key::<String>()? {
                    let (field, slot) = match key.as_str() {
                        "socket_path" => ("socket_path", &mut socket_path),
                        "state_root" => ("state_root", &mut state_root),
                        _ => {
                            let _ignored = map.next_value::<de::IgnoredAny>()?;
                            continue;
                        }
                    };
                    if slot.is_some() {
                        return Err(de::Error::duplicate_field(field));
                    }
                    *slot = Some(map.next_value()?);
                }
                Ok(SocketConfig {
                    socket_path: socket_path
                        .ok_or_else(|| de::Error::missing_field("socket_path"))?,
                    state_root: state_root.ok_or_else(|| de::Error::missing_field("state_root"))?,
                })
            }

            fn visit_seq<A: de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<SocketConfig, A::Error> {
                let socket_path = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let state_root = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                Ok(SocketConfig {
                    socket_path,
                    state_root,
                })
            }
        }

        deserializer.deserialize_struct("SocketConfig", FIELDS, SocketConfigVisitor)
    }
}

pub(super) struct RuntimeCapture {
    peer: KernelConnectedPeerProcessLeaseV1,
    binary: PathBuf,
    config: PathBuf,
    policy: PathBuf,
    binary_sha: String,
    config_sha: String,
    policy_sha: String,
    runner_sha: String,
    command: String,
    boot_identity: Option<Value>,
    command_line: Vec<u8>,
    socket: PathBuf,
}

impl RuntimeCapture {
    pub(super) fn start(
        args: &Args,
        socket: &Path,
        budget: &RequestBudgetV1,
    ) -> BenchResult<Option<Self>> {
        let fields = ["daemon-binary", "daemon-config", "decision-policy"];
        let count = fields
            .iter()
            .filter(|key| args.flags.contains_key(**key))
            .count();
        if count == 0 {
            return Ok(None);
        }
        if count != fields.len() {
            return Err(refuse(
                "daemon-binary, daemon-config and decision-policy are required together",
            ));
        }
        if !cfg!(target_os = "linux") {
            return Err(refuse(
                "connected image capture requires Linux peer descriptors",
            ));
        }
        let binary = absolute_file(&required(args, fields[0])?)?;
        let config = absolute_file(&required(args, fields[1])?)?;
        let policy = absolute_file(&required(args, fields[2])?)?;
        let raw_config = bounded_read(&config, 1024 * 1024)?;
        let parsed: SocketConfig = serde_json::from_slice(&raw_config).map_err(refuse)?;
        if parsed.socket_path != socket || !parsed.state_root.is_absolute() {
            return Err(refuse(
                "config socket differs or state root is not absolute",
            ));
        }
        let memory = MemoryScope::new(Arc::new(PeerMemory(Arc::new(PeerCharge(AtomicU64::new(
            0,
        ))))));
        let peer = quanta_index_sdk::native_uds::NativeUdsClient::new(socket)
            .capture_peer_process_admitted_v1(budget, &memory)
            .map_err(refuse)?
            .share_lease_v1();
        let pid = peer
            .credentials_v1()
            .pid
            .ok_or_else(|| refuse("peer PID is unavailable"))?;
        let command_line = bounded_read(Path::new(&format!("/proc/{pid}/cmdline")), 65536)?;
        let config_sha = sha256_hex(&raw_config);
        let (command, boot_identity) = bound_command(&command_line, &config, &config_sha)?;
        let captured = Self {
            peer,
            binary_sha: hash_file(&binary)?,
            policy_sha: sha256_hex(&bounded_read(&policy, 1024 * 1024)?),
            runner_sha: runner_binary_digest()?,
            binary,
            config,
            policy,
            config_sha,
            command,
            boot_identity,
            command_line,
            socket: socket.to_path_buf(),
        };
        captured.verify()?;
        Ok(Some(captured))
    }

    pub(super) fn peer(&self) -> &KernelConnectedPeerProcessLeaseV1 {
        &self.peer
    }

    fn verify(&self) -> BenchResult<()> {
        self.peer
            .with_live_executable_v1(|fd| -> BenchResult<()> {
                use sha2::{Digest, Sha256};
                let mut file = File::from(fd.try_clone_to_owned().map_err(refuse)?);
                let _position = file.seek(SeekFrom::Start(0)).map_err(refuse)?;
                let mut hasher = Sha256::new();
                let _copied = std::io::copy(&mut file, &mut hasher).map_err(refuse)?;
                if format!("{:x}", hasher.finalize()) != self.binary_sha {
                    return Err(refuse(
                        "connected executable differs from the pinned daemon",
                    ));
                }
                let pid = self
                    .peer
                    .credentials_v1()
                    .pid
                    .ok_or_else(|| refuse("peer PID absent"))?;
                if bounded_read(Path::new(&format!("/proc/{pid}/cmdline")), 65536)?
                    != self.command_line
                {
                    return Err(refuse("peer command line changed"));
                }
                Ok(())
            })
            .map_err(refuse)??;
        if hash_file(&self.binary)? != self.binary_sha
            || sha256_hex(&bounded_read(&self.config, 1024 * 1024)?) != self.config_sha
            || sha256_hex(&bounded_read(&self.policy, 1024 * 1024)?) != self.policy_sha
            || runner_binary_digest()? != self.runner_sha
        {
            return Err(refuse(
                "binary, config, policy or runner changed during capture",
            ));
        }
        Ok(())
    }

    pub(super) fn finish(&self) -> BenchResult<Value> {
        self.verify()?;
        let credentials = self.peer.credentials_v1();
        Ok(json!({
            "schema_version": 1, "kind": "linux_connected_peer_image_v1",
            "daemon_binary_sha256": self.binary_sha, "daemon_config_sha256": self.config_sha,
            "runner_binary_sha256": self.runner_sha, "decision_policy_sha256": self.policy_sha,
            "socket_path": self.socket, "config_path": self.config, "command": self.command,
            "boot_identity": self.boot_identity,
            "pid": credentials.pid, "uid": credentials.uid, "gid": credentials.gid,
            "peer_image_verified": true,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_config_allows_settings_but_rejects_duplicate_bindings() {
        for input in [
            r#"{"state_root":"/state","socket_path":"/sock","semantic":{"scopes":[]}}"#,
            r#"["/sock","/state"]"#,
        ] {
            let config: SocketConfig = serde_json::from_str(input).expect("socket bindings");
            assert_eq!(config.socket_path, Path::new("/sock"));
            assert_eq!(config.state_root, Path::new("/state"));
        }
        for input in [
            r#"{"socket_path":"/sock","socket_path":"/foreign","state_root":"/state"}"#,
            r#"{"state_root":"/state"}"#,
            r#"{"socket_path":null,"state_root":"/state"}"#,
            r#"["/sock","/state","/extra"]"#,
        ] {
            assert!(
                serde_json::from_str::<SocketConfig>(input).is_err(),
                "{input}"
            );
        }
    }

    #[test]
    fn execution_inputs_reject_special_files_and_enforce_byte_bounds() {
        let directory = tempfile::tempdir().expect("inputs");
        let file = directory.path().join("config");
        std::fs::write(&file, b"abc").expect("regular input");
        assert_eq!(
            hash_file(&file).expect("streamed bytes"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(bounded_read(&file, 3).expect("exact byte cap"), b"abc");
        assert!(bounded_read(&file, 2).is_err());
        let alias = directory.path().join("alias");
        std::os::unix::fs::symlink(&file, &alias).expect("path replacement");
        assert!(regular_file(&alias).is_err());
        assert!(regular_file(Path::new("/dev/null")).is_err());
        let fifo = directory.path().join("fifo");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .expect("FIFO fixture")
                .success()
        );
        assert!(bounded_read(&fifo, 1024).is_err());
    }

    #[test]
    fn execution_binding_is_optional_but_partial_pins_fail_before_socket_open() {
        let mut args = Args {
            positional: Vec::new(),
            flags: Default::default(),
            help: false,
        };
        let budget = RequestBudgetV1::for_duration(std::time::Duration::from_secs(1));
        let socket = Path::new("/missing-native-peer.sock");
        assert!(
            RuntimeCapture::start(&args, socket, &budget)
                .expect("diagnostic mode")
                .is_none()
        );
        let _previous = args
            .flags
            .insert("daemon-config".into(), "/missing-config".into());
        let error = RuntimeCapture::start(&args, socket, &budget)
            .err()
            .expect("partial pins refused");
        assert!(error.to_string().contains("required together"));
    }

    #[test]
    fn command_binding_requires_one_digest_and_the_actual_config() {
        let config = tempfile::NamedTempFile::new().expect("config");
        let path = config.path().canonicalize().expect("canonical config");
        let digest = "a".repeat(64);
        let valid = format!(
            "daemon\0serve-native-candidate\0--config\0{}\0--config-digest\0{digest}\0",
            path.display()
        );
        assert_eq!(
            bound_command(valid.as_bytes(), &path, &digest)
                .expect("bound argv")
                .0,
            "serve-native-candidate"
        );
        for invalid in [
            valid.replace("--config-digest", "--other-digest"),
            valid.replace(&digest, &"b".repeat(64)),
            format!("{valid}--config-digest\0{digest}\0"),
            valid.replace("serve-native-candidate", "verify-native-state"),
            valid.trim_end_matches('\0').to_owned(),
        ] {
            assert!(bound_command(invalid.as_bytes(), &path, &digest).is_err());
        }
    }

    #[test]
    fn command_binding_accepts_all_cli_orders_and_canonical_config_alias() {
        let config = tempfile::NamedTempFile::new().expect("config");
        let path = config.path().canonicalize().expect("canonical config");
        let digest = "a".repeat(64);
        let parent = path.parent().expect("config parent");
        let alias = parent
            .join("..")
            .join(parent.file_name().expect("parent name"))
            .join(path.file_name().expect("config name"));
        assert_eq!(alias.canonicalize().expect("alias resolves"), path);
        for argv in [
            format!(
                "daemon\0serve-native-candidate\0--config\0{}\0--config-digest\0{digest}\0",
                path.display()
            ),
            format!(
                "daemon\0--config\0{}\0serve-native-candidate\0--config-digest\0{digest}\0",
                path.display()
            ),
            format!(
                "daemon\0--config-digest\0{digest}\0--config\0{}\0serve-native-candidate\0",
                alias.display()
            ),
        ] {
            assert_eq!(
                bound_command(argv.as_bytes(), &path, &digest)
                    .expect("canonical daemon CLI")
                    .0,
                "serve-native-candidate"
            );
        }
    }

    #[test]
    fn selected_command_carries_exact_existing_boot_identity() {
        let config = tempfile::NamedTempFile::new().expect("config");
        let path = config.path().canonicalize().expect("canonical config");
        let digest = "a".repeat(64);
        let tenant = [1_u8; 16];
        let collection = [2_u8; 16];
        let worktree = [3_u8; 16];
        let slot = [4_u8; 16];
        let producer = [5_u8; 16];
        let stream = [6_u8; 16];
        let operation = json!({
            "workspace": {"tenant": tenant, "collection": collection,
                "worktree": worktree, "slot": slot},
            "epoch": 1, "producer": producer, "stream": stream,
            "stream_epoch": 1, "sequence": 7,
        });
        let argv = format!(
            "daemon\0serve-native\0--config\0{}\0--config-digest\0{digest}\0--root-nonce\0{}\0--manifest-operation\0{operation}\0--manifest-object\0{}\0--source-cut-digest\0{}\0",
            path.display(),
            "11".repeat(32),
            "22".repeat(32),
            "33".repeat(32)
        );
        let (mode, identity) =
            bound_command(argv.as_bytes(), &path, &digest).expect("selected command");
        assert_eq!(mode, "serve-native");
        assert_eq!(
            identity,
            Some(json!({
                "root_nonce_hex": "11".repeat(32),
                "manifest_operation": operation,
                "manifest_object_hex": "22".repeat(32),
                "source_cut_digest_hex": "33".repeat(32),
            }))
        );
    }

    #[test]
    fn command_binding_refuses_command_text_in_flag_value_or_duplicate_mode() {
        let config = tempfile::NamedTempFile::new().expect("config");
        let path = config.path().canonicalize().expect("canonical config");
        let digest = "a".repeat(64);
        for argv in [
            format!("daemon\0--config\0serve-native-candidate\0--config-digest\0{digest}\0"),
            format!(
                "daemon\0serve-native-candidate\0serve-native-candidate\0--config\0{}\0--config-digest\0{digest}\0",
                path.display()
            ),
            format!(
                "daemon\0serve-native-candidate\0--config\0{}\0--config\0{}\0--config-digest\0{digest}\0",
                path.display(),
                path.display()
            ),
        ] {
            assert!(bound_command(argv.as_bytes(), &path, &digest).is_err());
        }
    }

    #[test]
    fn peer_memory_is_bounded_until_the_last_shared_lease_drops() {
        let owner = PeerMemory(Arc::new(PeerCharge(AtomicU64::new(0))));
        let maximum = PEER_BYTES - charge_bytes(0).expect("lease backing");
        let lease = owner
            .reserve(maximum, MemoryLifetime::Retained)
            .expect("entire limit");
        let clone = lease.clone();
        drop(lease);
        assert!(owner.reserve(1, MemoryLifetime::Retained).is_err());
        drop(clone);
        assert!(
            owner
                .reserve(PEER_BYTES + 1, MemoryLifetime::Retained)
                .is_err()
        );
        assert!(owner.reserve(maximum, MemoryLifetime::Retained).is_ok());
    }
}
