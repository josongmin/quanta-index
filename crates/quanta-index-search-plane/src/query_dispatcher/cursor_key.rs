//! Persistent cursor signing-key custody for `CursorEnvelopeV2` (S21-06).
//!
//! One random 32-byte key per state root, stored as `id || key` under
//! `state_root/keys/` with mode `0600`, created on first open. Tokens minted
//! under it carry the standard TTL (15 minutes default, one hour maximum) and
//! every decode failure maps to an existing typed refusal code — never a raw
//! string and never a silent acceptance.

use std::fs;
use std::path::Path;

use quanta_index_contract::{
    CursorBindingV2, CursorEnvelopeError, CursorEnvelopeV2, CursorKeyV2, CursorTtlPolicyV2,
    SearchPlaneErrorCodeV2,
};
use quanta_index_core::CoreError;
use quanta_index_core::publication_admission::{
    DurableControlFileAdmissionPort, PublicationControlOpenTicket,
};

use crate::readiness::{create_cursor_key_admitted_v1, prepare_cursor_key_admitted_v1};

/// File mode the key file must have: owner read/write only.
const CURSOR_KEY_FILE_MODE: u32 = 0o600;

/// Bytes on disk: 8-byte big-endian key id followed by the 32-byte key.
const CURSOR_KEY_FILE_LEN: usize = 40;

/// The server-held cursor key plus the TTL policy it mints under.
///
/// Decode order is base64, canonical bytes, version, key lookup, HMAC,
/// expiry, then request-context binding — every step fail-closed before
/// any read-view acquisition. Mint and verify are the only token
/// operations; no route constructs or parses a token on its own (the
/// shared continuation authority lives in
/// `crate::query_dispatcher::continuation`).
#[derive(Clone, Debug)]
pub struct CursorKeyStore {
    key: CursorKeyV2,
    ttl: CursorTtlPolicyV2,
}

impl CursorKeyStore {
    /// Create a process-local signing authority. This is suitable for
    /// owner-local composition and tests; the product root must replace it
    /// with [`Self::open`] so continuations survive process restarts.
    pub fn ephemeral() -> Result<Self, CoreError> {
        let raw = fresh_entropy(CURSOR_KEY_FILE_LEN)?;
        let (id_bytes, key_bytes) = raw.split_at(8);
        let id = u64::from_be_bytes(
            id_bytes
                .try_into()
                .map_err(|err| invalid(&format!("cursor key id has the wrong length: {err}")))?,
        );
        let key = key_bytes
            .try_into()
            .map_err(|err| invalid(&format!("cursor key material has the wrong length: {err}")))?;
        Ok(Self {
            key: CursorKeyV2::new(id, key),
            ttl: CursorTtlPolicyV2::standard(),
        })
    }

    /// Load the key from `state_root/keys/cursor-v2.key`, creating it with
    /// fresh OS entropy on first open. Fails closed when the file exists
    /// with the wrong length or a looser mode, or when entropy or the
    /// filesystem is unavailable.
    pub fn open(state_root: &Path) -> Result<Self, CoreError> {
        let keys_dir = state_root.join("keys");
        let key_path = keys_dir.join("cursor-v2.key");
        if key_path.exists() {
            return Self::load(&key_path);
        }
        fs::create_dir_all(&keys_dir).map_err(|err| {
            CoreError::Storage(format!("cursor key: create keys directory: {err}"))
        })?;
        let raw = fresh_entropy(CURSOR_KEY_FILE_LEN)?;
        write_key_file(&key_path, &raw)?;
        Self::load(&key_path)
    }

    /// Open a service-owned cursor key through the canonical Control file
    /// authority. A new key is published only after both parents and the
    /// original writer have settled; an existing key is read under an owned
    /// reader lease on its actual file descriptor.
    pub fn open_with_durable_file_admission(
        state_root: &Path,
        admission: &dyn DurableControlFileAdmissionPort,
    ) -> Result<Self, CoreError> {
        let key_path = state_root.join("keys/cursor-v2.key");
        admission.reconcile_deletions()?;
        // Authenticate the exact root and key path before even creating a
        // directory. Only source absence authorizes the first-birth path.
        let prior = match admission.begin_reader_open(&key_path) {
            Ok(ticket) => Some(ticket),
            Err(CoreError::NotFound(_)) => None,
            Err(error) => return Err(error),
        };
        prepare_cursor_key_admitted_v1(state_root, admission)?;
        let ticket = if let Some(ticket) = prior {
            ticket
        } else {
            let entropy = fresh_entropy(CURSOR_KEY_FILE_LEN)?;
            let mut raw = [0u8; CURSOR_KEY_FILE_LEN];
            raw.copy_from_slice(&entropy);
            create_cursor_key_admitted_v1(&key_path, &raw, admission)?;
            admission.begin_reader_open(&key_path)?
        };
        Self::load_admitted(&key_path, ticket)
    }

    fn load(key_path: &Path) -> Result<Self, CoreError> {
        let raw = fs::read(key_path)
            .map_err(|err| CoreError::Storage(format!("cursor key: read: {err}")))?;
        if raw.len() != CURSOR_KEY_FILE_LEN {
            return Err(CoreError::Storage(format!(
                "cursor key: unexpected length {} (expected {CURSOR_KEY_FILE_LEN})",
                raw.len()
            )));
        }
        check_key_file_mode(key_path)?;
        Self::from_raw(&raw)
    }

    fn load_admitted(
        key_path: &Path,
        ticket: Box<dyn PublicationControlOpenTicket>,
    ) -> Result<Self, CoreError> {
        use std::io::Read as _;
        let file = open_cursor_key_nofollow(key_path)?;
        let mut reader = ticket.opened_owned(file)?;
        let loaded = (|| {
            let metadata = reader.metadata()?;
            check_admitted_key_metadata(&metadata)?;
            let mut raw = [0u8; CURSOR_KEY_FILE_LEN];
            reader
                .read_exact(&mut raw)
                .map_err(|error| cursor_io_error("read original FD", &error))?;
            let mut extra = [0u8; 1];
            if reader
                .read(&mut extra)
                .map_err(|error| cursor_io_error("check EOF", &error))?
                != 0
            {
                return Err(CoreError::Storage(
                    "cursor key: trailing bytes after fixed key".into(),
                ));
            }
            Self::from_raw(&raw)
        })();
        let retirement = reader.retire();
        match (loaded, retirement) {
            (Ok(key), Ok(())) => Ok(key),
            (Err(primary), Ok(())) => Err(primary),
            (Ok(_), Err(error)) => Err(error),
            (Err(mut primary), Err(error)) => {
                append_key_retirement_error(&mut primary, &error);
                Err(primary)
            }
        }
    }

    fn from_raw(raw: &[u8]) -> Result<Self, CoreError> {
        let (id_bytes, key_bytes) = raw
            .split_first_chunk::<8>()
            .and_then(|(id, rest)| rest.split_first_chunk::<32>().map(|(key, _)| (id, key)))
            .ok_or_else(|| {
                CoreError::Storage("cursor key: file does not hold id and key".to_string())
            })?;
        let id = u64::from_be_bytes(*id_bytes);
        let mut key = [0u8; 32];
        key.copy_from_slice(key_bytes);
        Ok(Self {
            key: CursorKeyV2::new(id, key),
            ttl: CursorTtlPolicyV2::standard(),
        })
    }

    /// The key id, for diagnostics.
    #[must_use]
    pub const fn key_id(&self) -> u64 {
        self.key.id()
    }
}

#[cfg(unix)]
fn open_cursor_key_nofollow(path: &Path) -> Result<fs::File, CoreError> {
    use rustix::fs::{Mode, OFlags, open, openat};

    let keys_dir = path
        .parent()
        .ok_or_else(|| CoreError::InvalidContract("cursor key: keys directory absent".into()))?;
    let state_root = keys_dir
        .parent()
        .ok_or_else(|| CoreError::InvalidContract("cursor key: state root absent".into()))?;
    let directory_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let root = fs::File::from(
        open(state_root, directory_flags, Mode::empty()).map_err(|error| {
            cursor_io_error("open held state root", &std::io::Error::from(error))
        })?,
    );
    verify_cursor_directory_identity(&root, state_root, false)?;
    let keys = fs::File::from(
        openat(&root, "keys", directory_flags, Mode::empty()).map_err(|error| {
            cursor_io_error("open held keys directory", &std::io::Error::from(error))
        })?,
    );
    verify_cursor_directory_identity(&keys, keys_dir, true)?;
    let fd = openat(
        &keys,
        "cursor-v2.key",
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|error| cursor_io_error("open original FD", &std::io::Error::from(error)))?;
    Ok(fs::File::from(fd))
}

#[cfg(unix)]
fn verify_cursor_directory_identity(
    directory: &fs::File,
    path: &Path,
    private: bool,
) -> Result<(), CoreError> {
    use std::os::unix::fs::MetadataExt as _;

    let held = directory
        .metadata()
        .map_err(|error| cursor_io_error("stat held directory", &error))?;
    let named = fs::symlink_metadata(path)
        .map_err(|error| cursor_io_error("stat named directory", &error))?;
    if !held.is_dir()
        || !named.is_dir()
        || held.dev() != named.dev()
        || held.ino() != named.ino()
        || (private
            && (held.uid() != rustix::process::geteuid().as_raw() || held.mode() & 0o022 != 0))
    {
        return Err(CoreError::InvalidContract(
            "cursor key: directory identity or permissions changed".into(),
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn open_cursor_key_nofollow(_path: &Path) -> Result<fs::File, CoreError> {
    Err(CoreError::InvalidContract(
        "cursor key: anchored nofollow open requires Unix".into(),
    ))
}

#[cfg(unix)]
fn check_admitted_key_metadata(metadata: &fs::Metadata) -> Result<(), CoreError> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    if !metadata.is_file()
        || metadata.len() != 40
        || metadata.permissions().mode() & 0o7777 != CURSOR_KEY_FILE_MODE
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
    {
        return Err(CoreError::Storage(
            "cursor key: original FD is not a private 40-byte regular key".into(),
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_admitted_key_metadata(_metadata: &fs::Metadata) -> Result<(), CoreError> {
    Err(CoreError::InvalidContract(
        "cursor key: metadata verification requires Unix".into(),
    ))
}

fn append_key_retirement_error(primary: &mut CoreError, retirement: &CoreError) {
    let message = match primary {
        CoreError::InvalidContract(message)
        | CoreError::Typed { message, .. }
        | CoreError::NotReady(message)
        | CoreError::NotImplemented(message)
        | CoreError::NotFound(message)
        | CoreError::Storage(message)
        | CoreError::StorageIo { message, .. } => message,
    };
    message.push_str("; additionally failed to retire cursor key reader: ");
    message.push_str(&retirement.to_string());
}

fn cursor_io_error(context: &str, error: &std::io::Error) -> CoreError {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(cause) = source {
        if let Some(primary) = cause.downcast_ref::<CoreError>() {
            return primary.clone();
        }
        if let Some(io) = cause.downcast_ref::<std::io::Error>()
            && io.raw_os_error().is_some()
        {
            return CoreError::storage_io(context, io);
        }
        source = cause
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
            .map(|inner| -> &(dyn std::error::Error + 'static) { inner })
            .or_else(|| cause.source());
    }
    CoreError::storage_io(context, error)
}

impl CursorKeyStore {
    /// Mint a continuation token for `binding` at `now_unix` under the
    /// standard TTL (or a clamped explicit request).
    pub fn mint(
        &self,
        binding: CursorBindingV2,
        boundary: &str,
        now_unix: u64,
        requested_ttl_secs: Option<u64>,
    ) -> Result<(String, CursorEnvelopeV2), CoreError> {
        let ttl_secs = match requested_ttl_secs {
            Some(requested) => self
                .ttl
                .clamp(requested)
                .ok_or_else(|| invalid("cursor ttl must be positive"))?,
            None => self.ttl.default_secs,
        };
        let expires = now_unix
            .checked_add(ttl_secs)
            .ok_or_else(|| invalid("cursor expiry overflow"))?;
        let envelope = CursorEnvelopeV2::mint(binding, boundary, &self.key, now_unix, expires)
            .map_err(map_envelope_error)?;
        let token = envelope.encode(&self.key).map_err(map_envelope_error)?;
        Ok((token, envelope))
    }

    /// Decode and verify a continuation token: signature, version, key id
    /// and expiry all fail closed before any resource acquisition.
    pub fn verify(&self, token: &str, now_unix: u64) -> Result<CursorEnvelopeV2, CoreError> {
        CursorEnvelopeV2::decode(token, &self.key, now_unix).map_err(map_envelope_error)
    }

    /// Fail-closed context check: the decoded envelope must bind to the
    /// executing request exactly, otherwise the continuation is refused
    /// with `CURSOR_CONTEXT_MISMATCH` before the query runs.
    pub fn require_binding(
        &self,
        envelope: &CursorEnvelopeV2,
        expected: &CursorBindingV2,
    ) -> Result<(), CoreError> {
        if envelope.matches(expected, self.key.id()) {
            Ok(())
        } else {
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CursorContextMismatch,
                message: "cursor continuation context does not match the executing request"
                    .to_string(),
            })
        }
    }
}

fn invalid(message: &str) -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::CursorInvalid,
        message: message.to_string(),
    }
}

/// Map envelope failures onto the existing stable refusal codes.
#[expect(
    clippy::needless_pass_by_value,
    reason = "the message is built from the owned error's Display output"
)]
fn map_envelope_error(error: CursorEnvelopeError) -> CoreError {
    let code = match &error {
        CursorEnvelopeError::Expired => SearchPlaneErrorCodeV2::CursorExpired,
        CursorEnvelopeError::MalformedToken
        | CursorEnvelopeError::Version { .. }
        | CursorEnvelopeError::Signature
        | CursorEnvelopeError::InvalidWindow => SearchPlaneErrorCodeV2::CursorInvalid,
    };
    CoreError::Typed {
        code,
        message: error.to_string(),
    }
}

fn fresh_entropy(len: usize) -> Result<Vec<u8>, CoreError> {
    use std::io::Read;
    let mut entropy = fs::File::open("/dev/urandom")
        .map_err(|err| CoreError::Storage(format!("cursor key: open entropy source: {err}")))?;
    let mut buffer = vec![0u8; len];
    entropy
        .read_exact(&mut buffer)
        .map_err(|err| CoreError::Storage(format!("cursor key: read entropy: {err}")))?;
    Ok(buffer)
}

fn write_key_file(path: &Path, raw: &[u8]) -> Result<(), CoreError> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|err| CoreError::Storage(format!("cursor key: create: {err}")))?;
    set_owner_only_mode(&file)?;
    file.write_all(raw)
        .and_then(|()| file.sync_all())
        .map_err(|err| CoreError::Storage(format!("cursor key: write: {err}")))?;
    Ok(())
}

#[cfg(unix)]
fn set_owner_only_mode(file: &fs::File) -> Result<(), CoreError> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(CURSOR_KEY_FILE_MODE))
        .map_err(|err| CoreError::Storage(format!("cursor key: set mode: {err}")))
}

#[cfg(unix)]
fn check_key_file_mode(path: &Path) -> Result<(), CoreError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)
        .map_err(|err| CoreError::Storage(format!("cursor key: stat: {err}")))?
        .permissions()
        .mode();
    if mode & 0o777 != CURSOR_KEY_FILE_MODE {
        return Err(CoreError::Storage(format!(
            "cursor key: mode {:o} is looser than {:o}",
            mode & 0o777,
            CURSOR_KEY_FILE_MODE
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_owner_only_mode(_file: &fs::File) -> Result<(), CoreError> {
    Err(CoreError::Storage(
        "cursor key: owner-only mode is not enforceable on this platform".to_string(),
    ))
}

#[cfg(not(unix))]
fn check_key_file_mode(_path: &Path) -> Result<(), CoreError> {
    Err(CoreError::Storage(
        "cursor key: mode enforcement is not available on this platform".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{CursorKeyStore, cursor_io_error};
    use quanta_index_contract::{
        CursorBindingV2, CursorRouteV2, GenerationPin, ManifestGeneration, RepoId, RevisionId,
        SearchPlaneErrorCodeV2,
    };
    use quanta_index_core::CoreError;

    #[test]
    fn admitted_reader_io_keeps_nested_typed_and_os_causes() {
        let budget = CoreError::Typed {
            code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
            message: "fixed reader budget refusal".into(),
        };
        let nested = cursor_io_error(
            "read original FD",
            &std::io::Error::other(std::io::Error::other(budget)),
        );
        assert!(matches!(nested, CoreError::Typed { code, message }
            if code == SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded
                && message == "fixed reader budget refusal"));
        let os = cursor_io_error(
            "read original FD",
            &std::io::Error::from_raw_os_error(rustix::io::Errno::NOSPC.raw_os_error()),
        );
        assert!(
            matches!(os, CoreError::StorageIo { raw_os_error: Some(code), .. }
            if code == rustix::io::Errno::NOSPC.raw_os_error())
        );
        let text = cursor_io_error(
            "read original FD",
            &std::io::Error::other("IngestResourceBudgetExceeded"),
        );
        assert!(matches!(
            text,
            CoreError::StorageIo {
                raw_os_error: None,
                ..
            }
        ));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("p05-cursor-key-{name}-{}", std::process::id()));
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(error) if matches!(error.kind(), std::io::ErrorKind::NotFound) => {}
            Err(error) => panic!("cleanup: {error}"),
        }
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn binding(repo: &str) -> CursorBindingV2 {
        CursorBindingV2 {
            route: CursorRouteV2::Lexical,
            pin: GenerationPin::new(
                RepoId::new(repo).expect("static fixture identity"),
                RevisionId::new("rev-a").expect("static fixture identity"),
                ManifestGeneration::new(3),
            ),
            plan_digest: [0x11; 32],
            query_digest: [0x22; 32],
            constraints_digest: [0x33; 32],
            order: "rank_desc".to_string(),
            cap: 20,
            aux_epochs: Vec::new(),
        }
    }

    #[test]
    fn key_is_persistent_and_owner_only() {
        let state_root = temp_dir("persist");
        let first = CursorKeyStore::open(&state_root).expect("first open");
        let second = CursorKeyStore::open(&state_root).expect("second open");
        assert_eq!(first.key_id(), second.key_id());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(state_root.join("keys/cursor-v2.key"))
                .expect("key file")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        if let Err(error) = std::fs::remove_dir_all(&state_root) {
            panic!("cleanup: {error}");
        }
    }

    #[test]
    fn minted_tokens_round_trip_and_refuse_foreign_contexts() {
        let state_root = temp_dir("mint");
        let store = CursorKeyStore::open(&state_root).expect("open");
        let (token, envelope) = store
            .mint(binding("repo-a"), "boundary-1", 1_000, None)
            .expect("mint");
        let decoded = store.verify(&token, 1_100).expect("verify");
        assert_eq!(decoded, envelope);
        // Expired.
        let expired = store.verify(&token, 1_000 + 15 * 60 + 1);
        assert_eq!(
            expired.expect_err("expired").into_search_plane_wire().0,
            SearchPlaneErrorCodeV2::CursorExpired
        );
        // Wrong context.
        match store.require_binding(&decoded, &binding("repo-b")) {
            Err(refused @ CoreError::Typed { .. }) => {
                assert_eq!(
                    refused.into_search_plane_wire().0,
                    SearchPlaneErrorCodeV2::CursorContextMismatch
                );
            }
            other => panic!("foreign repo must be refused typed, got {other:?}"),
        }
        store
            .require_binding(&decoded, &binding("repo-a"))
            .expect("matching context binds");
        // Tampered token.
        let tampered = format!("x{token}");
        assert_eq!(
            store
                .verify(&tampered, 1_100)
                .expect_err("tampered")
                .into_search_plane_wire()
                .0,
            SearchPlaneErrorCodeV2::CursorInvalid
        );
        if let Err(error) = std::fs::remove_dir_all(&state_root) {
            panic!("cleanup: {error}");
        }
    }
}
