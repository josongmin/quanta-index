//! Schema-2 wire codec. Native recovery does not introduce another persisted IR.
use super::{MAX_BYTES, charge, fingerprint, read_control};
use anyhow::{Result, anyhow, ensure};
use quanta_index_contract::index::{Digest, ObjectRef, OperationKey};
use quanta_index_core::index::WorkBudget;
use quanta_index_searchd::cli::{NativeBootIdentityV1, NativeCommandModeV1, NativeStateCommandV1};
use rustix::fs::{Mode, OFlags};
use serde::{
    Deserialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::{
    collections::BTreeMap,
    fmt, fs,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

mod environment;
const COMMON: &[&str] = &[
    "binary",
    "binary_sha256",
    "environment",
    "mode",
    "schema_version",
    "state_root",
];
const NATIVE: &[&str] = &[
    "native_boot_receipt",
    "native_config",
    "native_config_sha256",
];

/// Canonical persisted selection bytes plus projections into existing Q types.
/// No Debug/Serialize exposes the selected environment to terminal diagnostics.
pub(crate) struct DeploymentSelectionDocumentV1 {
    payload: Vec<u8>,
    fingerprint: Digest,
    state_root: PathBuf,
    binary: PathBuf,
    binary_digest: Digest,
    native: Option<NativeStateCommandV1>,
}
impl DeploymentSelectionDocumentV1 {
    pub(super) fn payload_v1(&self) -> &[u8] {
        &self.payload
    }
    pub(super) fn fingerprint_v1(&self) -> Digest {
        self.fingerprint
    }
    pub(crate) fn state_root_v1(&self) -> &Path {
        &self.state_root
    }
    pub(crate) fn native_command_v1(&self) -> Option<&NativeStateCommandV1> {
        self.native.as_ref()
    }
    pub(super) fn binary_v1(&self) -> &Path {
        &self.binary
    }
    pub(crate) fn legacy_physical_policy_v1(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_searchd::app::config::LegacyPublicationPhysicalPolicy> {
        use quanta_index_core::index::{IndexError, MemoryLifetime};
        ensure!(
            self.native.is_none(),
            "Legacy physical policy requires the saved Legacy selection"
        );
        let bytes = u64::try_from(self.payload.len())?
            .checked_mul(64)
            .and_then(|n| n.checked_add(65536))
            .ok_or(IndexError::ResourceLimited)?;
        let _memory = budget
            .reserve_memory(bytes, MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited)?;
        let wire = unique_json(&self.payload)?;
        let environment = wire
            .get("environment")
            .and_then(Value::as_object)
            .ok_or_else(|| anyhow!("saved Legacy environment is missing"))?;
        let policy=quanta_index_searchd::app::config::LegacyPublicationPhysicalPolicy::from_explicit_lookup_v1(
            &|name|Ok(environment.get(name).and_then(Value::as_str).map(str::to_owned)))?
            .ok_or_else(||anyhow!("saved Legacy selection has no complete physical policy"))?;
        budget.checkpoint()?;
        Ok(policy)
    }
    pub(crate) fn binary_sha256_v1(&self) -> Digest {
        self.binary_digest
    }
    pub(super) fn verify_binary_v1(&self, budget: &mut WorkBudget) -> Result<VerifiedBinary> {
        verify_binary(&self.binary, self.binary_digest, budget)
    }
}

pub(super) fn read_document(
    path: &Path,
    budget: &mut WorkBudget,
) -> Result<DeploymentSelectionDocumentV1> {
    let bytes = read_control(path, budget)?;
    decode(&bytes, budget)
}
pub(super) fn decode(
    bytes: &[u8],
    budget: &mut WorkBudget,
) -> Result<DeploymentSelectionDocumentV1> {
    let (wire, payload) = canonical_document(bytes)?;
    decode_document(&wire, payload, budget)
}

pub(super) fn canonical_document(bytes: &[u8]) -> Result<(Value, Vec<u8>)> {
    ensure!(bytes.len() <= MAX_BYTES, "selection exceeds byte bound");
    let wire = unique_json(bytes)?;
    let payload = canonical_payload(&wire)?;
    ensure!(
        payload.len() <= MAX_BYTES,
        "canonical selection exceeds byte bound"
    );
    Ok((wire, payload))
}

pub(super) fn decode_document(
    wire: &Value,
    payload: Vec<u8>,
    budget: &mut WorkBudget,
) -> Result<DeploymentSelectionDocumentV1> {
    ensure!(
        wire.get("schema_version").and_then(Value::as_u64) == Some(2),
        "unsupported selection format"
    );
    let native = match text(wire, "mode")? {
        "native" => true,
        "legacy" => false,
        _ => return Err(anyhow!("unsupported selection mode")),
    };
    exact_fields(wire, COMMON, if native { NATIVE } else { &[] })?;
    let binary = canonical_path(text(wire, "binary")?, false, budget)?;
    let state_root = canonical_path(text(wire, "state_root")?, true, budget)?;
    let binary_digest = hex(text(wire, "binary_sha256")?)?;
    let environment = wire
        .get("environment")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("environment must be a JSON object"))?;
    for (key, value) in environment {
        ensure!(
            environment::ALLOWED.contains(&key.as_str()),
            "unknown environment/config key"
        );
        ensure!(
            value.as_str().is_some_and(|s| !s.contains('\0')),
            "invalid environment value"
        );
    }
    let command = if native {
        let config = canonical_path(text(wire, "native_config")?, false, budget)?;
        let config_digest = hex(text(wire, "native_config_sha256")?)?;
        let config_bytes = read_control(&config, budget)?;
        ensure!(
            fingerprint(&config_bytes) == config_digest,
            "native config SHA-256 changed"
        );
        let config_wire = unique_json(&config_bytes)?;
        ensure!(
            config_wire.get("state_root").and_then(Value::as_str) == state_root.to_str(),
            "native config state_root differs from selected root"
        );
        let receipt = wire
            .get("native_boot_receipt")
            .ok_or_else(|| anyhow!("Native receipt missing"))?;
        exact_fields(
            receipt,
            &[
                "root_nonce_hex",
                "manifest_operation",
                "manifest_object_hex",
                "source_cut_digest_hex",
                "install_catalog_digest_hex",
            ],
            &["protected_source_routes_hex"],
        )?;
        let nonce = hex(text(receipt, "root_nonce_hex")?)?;
        let source = hex(text(receipt, "source_cut_digest_hex")?)?;
        ensure!(
            nonce != Digest([0; 32]) && source != Digest([0; 32]),
            "native boot receipt has a zero identity digest"
        );
        let operation = receipt
            .get("manifest_operation")
            .ok_or_else(|| anyhow!("manifest operation missing"))?;
        validate_operation(operation)?;
        let operation: OperationKey = serde_json::from_value(operation.clone())?;
        Some(NativeStateCommandV1 {
            mode: NativeCommandModeV1::Verify,
            config_path: config,
            identity: Some(NativeBootIdentityV1 {
                root_nonce: nonce.0,
                manifest_operation: operation,
                manifest_object: ObjectRef(hex(text(receipt, "manifest_object_hex")?)?),
                source_cut_digest: source,
                protected_source_routes: receipt.get("protected_source_routes_hex")
                    .map(|_| text(receipt, "protected_source_routes_hex").and_then(hex).map(ObjectRef))
                    .transpose()?,
            }),
            config_digest: Some(config_digest),
            install_catalog_digest: Some(hex(text(receipt, "install_catalog_digest_hex")?)?),
            select_operation: None,
            receipt_path: None,
        })
    } else {
        None
    };
    Ok(DeploymentSelectionDocumentV1 {
        fingerprint: fingerprint(&payload),
        payload,
        state_root,
        binary,
        binary_digest,
        native: command,
    })
}
fn exact_fields(value: &Value, fields: &[&str], extra: &[&str]) -> Result<()> {
    let map = value
        .as_object()
        .ok_or_else(|| anyhow!("selection record must be an object"))?;
    ensure!(
        map.len() == fields.len() + extra.len()
            && map
                .keys()
                .all(|k| fields.contains(&k.as_str()) || extra.contains(&k.as_str())),
        "selection fields differ from format version"
    );
    Ok(())
}
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("selection field must be a string"))
}
fn validate_operation(value: &Value) -> Result<()> {
    exact_fields(
        value,
        &[
            "workspace",
            "epoch",
            "producer",
            "stream",
            "stream_epoch",
            "sequence",
        ],
        &[],
    )?;
    let workspace = value
        .get("workspace")
        .ok_or_else(|| anyhow!("operation workspace missing"))?;
    exact_fields(
        workspace,
        &["tenant", "collection", "worktree", "slot"],
        &[],
    )?;
    for key in ["tenant", "collection", "worktree", "slot"] {
        bytes16(workspace.get(key))?;
    }
    for key in ["producer", "stream"] {
        bytes16(value.get(key))?;
    }
    for key in ["epoch", "stream_epoch", "sequence"] {
        ensure!(
            value.get(key).and_then(Value::as_u64).is_some(),
            "operation axis must be u64"
        );
    }
    Ok(())
}
fn bytes16(value: Option<&Value>) -> Result<()> {
    let bytes = value
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("operation ID must be an array"))?;
    ensure!(
        bytes.len() == 16 && bytes.iter().all(|b| b.as_u64().is_some_and(|n| n <= 255)),
        "operation ID must be exactly 16 bytes"
    );
    Ok(())
}
pub(super) fn hex(value: &str) -> Result<Digest> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "digest must be lowercase SHA-256"
    );
    let mut bytes = [0; 32];
    for (i, slot) in bytes.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16)?;
    }
    Ok(Digest(bytes))
}
fn canonical_path(raw: &str, directory: bool, budget: &mut WorkBudget) -> Result<PathBuf> {
    let path = Path::new(raw);
    ensure!(
        raw.len() <= 4096 && !raw.contains('\0') && path.is_absolute(),
        "selection path must be a bounded absolute path"
    );
    charge(budget, 2, 0, 0, 0)?;
    let canonical = fs::canonicalize(path)?;
    ensure!(
        canonical.to_str() == Some(raw),
        "selection paths are not canonical"
    );
    let metadata = fs::metadata(&canonical)?;
    ensure!(
        if directory {
            metadata.is_dir()
        } else {
            metadata.is_file()
        },
        "selection path has wrong file type"
    );
    Ok(canonical)
}
pub(super) struct VerifiedBinary {
    file: fs::File,
    hashed: fs::Metadata,
}

impl VerifiedBinary {
    pub(super) fn require_current(&self, path: &Path, budget: &mut WorkBudget) -> Result<()> {
        charge(budget, 3, 0, 0, 0)?;
        rustix::fs::access(path, rustix::fs::Access::EXEC_OK)?;
        ensure!(
            same_binary_version(&self.hashed, &self.file.metadata()?)
                && same_binary_version(&self.hashed, &fs::symlink_metadata(path)?),
            "selected binary changed after hashing"
        );
        Ok(())
    }
}

fn same_binary_version(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    after.is_file()
        && before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.len() == after.len()
        && before.mode() == after.mode()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.ctime() == after.ctime()
        && before.ctime_nsec() == after.ctime_nsec()
}

fn verify_binary(path: &Path, expected: Digest, budget: &mut WorkBudget) -> Result<VerifiedBinary> {
    charge(budget, 2, 0, 0, 0)?;
    let mut file = fs::File::from(rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let hashed = file.metadata()?;
    ensure!(hashed.is_file(), "selected binary is not regular");
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        charge(budget, 1, 0, u64::try_from(buffer.len())?, 0)?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    ensure!(
        Digest(hash.finalize().into()) == expected,
        "selected binary SHA-256 changed"
    );
    let verified = VerifiedBinary { file, hashed };
    verified.require_current(path, budget)?;
    Ok(verified)
}

/// Python json.dumps(sort_keys=True, separators=(",", ":"), ensure_ascii=True).
struct AsciiFormatter;
impl serde_json::ser::Formatter for AsciiFormatter {
    fn write_string_fragment<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> std::io::Result<()> {
        for ch in fragment.chars() {
            if ch.is_ascii() && ch != '\u{7f}' {
                writer.write_all(&[u8::try_from(u32::from(ch)).map_err(std::io::Error::other)?])?;
            } else {
                let mut units = [0; 2];
                for unit in ch.encode_utf16(&mut units).iter() {
                    write!(writer, "\\u{unit:04x}")?;
                }
            }
        }
        Ok(())
    }
}
pub(super) fn canonical_payload(value: &Value) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(MAX_BYTES + 1)?;
    // UniqueValue constructs every map in sorted order, including configurations
    // compiled with serde_json's preserve_order feature.
    let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, AsciiFormatter);
    serde::Serialize::serialize(value, &mut serializer)?;
    bytes.push(b'\n');
    Ok(bytes)
}

struct UniqueValue(Value);
impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: de::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueVisitor).map(Self)
    }
}
struct UniqueVisitor;
impl<'de> Visitor<'de> for UniqueVisitor {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("JSON without duplicate object fields")
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Value, E> {
        Ok(Value::Number(v.into()))
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Value, E> {
        Ok(Value::Number(v.into()))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Value, E> {
        serde_json::Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| E::custom("nonfinite JSON number"))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }
    fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Value, E> {
        Ok(Value::String(v))
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(UniqueValue(v)) = seq.next_element()? {
            values.push(v);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Value, A::Error> {
        let mut entries = BTreeMap::new();
        while let Some((key, UniqueValue(value))) = map.next_entry::<String, UniqueValue>()? {
            if entries.insert(key, value).is_some() {
                return Err(de::Error::custom("duplicate JSON object field"));
            }
        }
        Ok(Value::Object(entries.into_iter().collect()))
    }
}
pub(super) fn unique_json(bytes: &[u8]) -> Result<Value> {
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let value = UniqueValue::deserialize(&mut decoder)?.0;
    decoder.end()?;
    Ok(value)
}
