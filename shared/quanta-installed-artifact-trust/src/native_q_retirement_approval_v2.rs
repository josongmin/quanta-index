//! Authenticate a Q retirement approval under the fixed installed S trust root.
//! The signed payload remains data until Q compares it with its selected BOOT,
//! executable, config and physically verified retirement object.

use super::*;
use std::os::unix::fs::OpenOptionsExt;

const APPROVAL_DOMAIN_V2: &[u8] = b"semantica.installed-native-q-approval.v2\n";
const APPROVAL_SCHEMA_V2: &str = "semantica-installed-native-q-approval/v2";
const MAX_APPROVAL_BYTES_V2: u64 = 8 * 1024;

/// A signed receipt with the installation rotation lock retained for its user.
/// No caller-provided key or active generation can construct this capability.
pub struct VerifiedNativeQRetirementApprovalV2 {
    payload_v2: Value,
    _rotation_lock_v2: File,
}

impl VerifiedNativeQRetirementApprovalV2 {
    pub fn payload_v2(&self) -> &Value {
        &self.payload_v2
    }
}

/// `receipt_path_v2` is only a locator. Its signature, active generation and
/// installed manifest are checked under the fixed root before returning.
pub fn verify_native_q_retirement_approval_v2(
    receipt_path_v2: &Path,
) -> Result<VerifiedNativeQRetirementApprovalV2, InstalledTrustErrorV1> {
    let mut admission_v2 = UnmeteredInstalledAdmissionV1;
    let mut context_v2 = VerifyContextV1 {
        admission_v1: &mut admission_v2,
        leases_v1: Vec::new(),
        refusal_v1: None,
    };
    let trust_root_v2 = Path::new(TRUST_ROOT_V1);
    let anchor_v2 = Path::new("/");
    let selected_v2 = verify_signed_install_v1(&mut context_v2, trust_root_v2, anchor_v2, 0)?;
    let active_v2 = context_v2.open_v1(
        anchor_v2,
        &trust_root_v2.join("active-generation.json"),
        0,
        false,
    )?;
    let active_sha_v2 = sha256_v1(&context_v2.bounded_read_v1(&active_v2, MAX_ACTIVE_BYTES_V1)?);
    let key_v2 = context_v2.open_v1(
        anchor_v2,
        &trust_root_v2.join("deployment-public-key.bin"),
        0,
        false,
    )?;
    let key_bytes_v2: [u8; 32] = context_v2
        .bounded_read_v1(&key_v2, 32)?
        .try_into()
        .map_err(|_| InstalledTrustErrorV1::InvalidSignature)?;
    if !receipt_path_v2.is_absolute() {
        return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
    }
    context_v2.io_v1()?;
    let receipt_v2 = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(receipt_path_v2)
        .map_err(|_| InstalledTrustErrorV1::Io)?;
    context_v2.io_v1()?;
    if !receipt_v2
        .metadata()
        .map_err(|_| InstalledTrustErrorV1::Io)?
        .is_file()
    {
        return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
    }
    let raw_v2 = context_v2.bounded_read_v1(&receipt_v2, MAX_APPROVAL_BYTES_V2)?;
    let envelope_v2: Value = serde_json::from_slice(&raw_v2)
        .map_err(|_| InstalledTrustErrorV1::InvalidActiveGeneration)?;
    let fields_v2 = envelope_v2
        .as_object()
        .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?;
    if fields_v2.len() != 2
        || !fields_v2.contains_key("payload")
        || !fields_v2.contains_key("signature_hex")
    {
        return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
    }
    let payload_v2 = envelope_v2
        .get("payload")
        .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?;
    let payload_fields_v2 = payload_v2
        .as_object()
        .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?;
    let expected_fields_v2 = [
        "schema_version",
        "active_generation_sha256",
        "release_manifest_sha256",
        "q_elf_sha256",
        "selection_sha256",
        "selection_path",
        "state_root",
        "boot_receipt",
        "config_sha256",
        "decision_policy_sha256",
        "retired_projection_policy",
    ];
    if payload_fields_v2.len() != expected_fields_v2.len()
        || !expected_fields_v2
            .iter()
            .all(|name_v2| payload_fields_v2.contains_key(*name_v2))
        || field_v1(payload_v2, "schema_version")? != APPROVAL_SCHEMA_V2
        || field_v1(payload_v2, "active_generation_sha256")? != active_sha_v2
        || field_v1(payload_v2, "release_manifest_sha256")? != selected_v2.manifest_digest_v1
    {
        return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
    }
    for name_v2 in [
        "active_generation_sha256",
        "release_manifest_sha256",
        "q_elf_sha256",
        "selection_sha256",
        "config_sha256",
        "decision_policy_sha256",
    ] {
        let _ = hex_bytes_v1::<32>(field_v1(payload_v2, name_v2)?)?;
    }
    let signature_v2 = hex_bytes_v1::<64>(field_v1(&envelope_v2, "signature_hex")?)?;
    let canonical_v2 = canonical_value_v1(&envelope_v2)?;
    if raw_v2 != canonical_v2 && raw_v2 != [canonical_v2, b"\n".to_vec()].concat() {
        return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
    }
    let mut signed_v2 = APPROVAL_DOMAIN_V2.to_vec();
    signed_v2.extend(canonical_value_v1(payload_v2)?);
    VerifyingKey::from_bytes(&key_bytes_v2)
        .map_err(|_| InstalledTrustErrorV1::InvalidSignature)?
        .verify_strict(&signed_v2, &Signature::from_bytes(&signature_v2))
        .map_err(|_| InstalledTrustErrorV1::InvalidSignature)?;
    Ok(VerifiedNativeQRetirementApprovalV2 {
        payload_v2: payload_v2.clone(),
        _rotation_lock_v2: selected_v2.rotation_lock_v1,
    })
}
