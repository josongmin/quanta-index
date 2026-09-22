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
                .map_err(|_| invalid("cursor key id has the wrong length"))?,
        );
        let key = key_bytes
            .try_into()
            .map_err(|_| invalid("cursor key material has the wrong length"))?;
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

    use super::CursorKeyStore;
    use quanta_index_contract::{
        CursorBindingV2, CursorRouteV2, GenerationPin, ManifestGeneration, RepoId, RevisionId,
        SearchPlaneErrorCodeV2,
    };
    use quanta_index_core::CoreError;

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
