//! Read only the Native BOOT selector from retained deployment selection bytes.
//! The caller retains and hashes the original bytes. This is comparison DATA;
//! it neither decodes the deployment command nor authorizes a root switch.

use super::{NativeCutoverDigest64V1, NativeCutoverEvidenceSyntaxErrorV1, OperationKey};
use crate::index::{Digest, ObjectRef, RetiredProjectionPolicyRefV1};
use quanta_index_contract_base::ClosedFieldKeySeedV1;
use serde::de::{self, MapAccess, Visitor};

const MAX_SELECTION_BYTES_V1: usize = 64 * 1024;
const SELECTION_FIELDS_V1: &[&str] = &[
    "binary", "binary_sha256", "environment", "mode", "schema_version",
    "state_root", "native_boot_receipt", "native_config", "native_config_sha256",
];
const BOOT_FIELDS_V1: &[&str] = &[
    "root_nonce_hex", "manifest_operation", "manifest_object_hex",
    "source_cut_digest_hex", "install_catalog_digest_hex",
    "protected_source_routes_hex", "retired_policy_object_hex",
    "retired_policy_sha256_hex", "retired_policy_identity_count",
];

fn digest64_v1<E: de::Error>(text: &str, require_nonzero: bool) -> Result<NativeCutoverDigest64V1, E> {
    let bytes: [u8; 64] = text.as_bytes().try_into()
        .map_err(|_| E::custom("Native receipt digest must contain 64 lowercase hex bytes"))?;
    if !bytes.iter().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        || (require_nonzero && bytes.iter().all(|byte| *byte == b'0'))
    {
        return Err(E::custom("Native receipt digest is not canonical or is zero"));
    }
    Ok(NativeCutoverDigest64V1(bytes))
}

fn policy_digest_v1<E: de::Error>(text: &str) -> Result<Digest, E> {
    let _canonical = digest64_v1::<E>(text, false)?;
    let mut digest = [0; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).map_err(E::custom)?;
    }
    Ok(Digest(digest))
}

struct BootSelectorV1 {
    nonce: NativeCutoverDigest64V1,
    operation: OperationKey,
    has_retired_policy: bool,
}

struct BootVisitorV1;

impl<'de> Visitor<'de> for BootVisitorV1 {
    type Value = BootSelectorV1;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a closed schema-2 or schema-3 Native BOOT receipt")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut seen = 0u16;
        let mut nonce = None;
        let mut operation = None;
        let mut policy_object = None;
        let mut policy_sha256 = None;
        let mut policy_count = None;
        while let Some(key) = map.next_key_seed(ClosedFieldKeySeedV1(BOOT_FIELDS_V1))? {
            let bit = match key {
                "root_nonce_hex" => 1,
                "manifest_operation" => 2,
                "manifest_object_hex" => 4,
                "source_cut_digest_hex" => 8,
                "install_catalog_digest_hex" => 16,
                "protected_source_routes_hex" => 32,
                "retired_policy_object_hex" => 64,
                "retired_policy_sha256_hex" => 128,
                "retired_policy_identity_count" => 256,
                _ => unreachable!("closed field seed"),
            };
            if seen & bit != 0 {
                return Err(de::Error::duplicate_field(key));
            }
            seen |= bit;
            match key {
                "root_nonce_hex" => {
                    let text: &str = map.next_value()?;
                    nonce = Some(digest64_v1::<M::Error>(text, true)?);
                }
                "manifest_operation" => operation = Some(map.next_value::<OperationKey>()?),
                "manifest_object_hex"
                | "install_catalog_digest_hex"
                | "protected_source_routes_hex" => {
                    let text: &str = map.next_value()?;
                    let _ = digest64_v1::<M::Error>(text, false)?;
                }
                "source_cut_digest_hex" => {
                    let text: &str = map.next_value()?;
                    let _ = digest64_v1::<M::Error>(text, true)?;
                }
                "retired_policy_object_hex" => {
                    policy_object = Some(ObjectRef(policy_digest_v1::<M::Error>(map.next_value()?)?));
                }
                "retired_policy_sha256_hex" => {
                    policy_sha256 = Some(policy_digest_v1::<M::Error>(map.next_value()?)?);
                }
                "retired_policy_identity_count" => policy_count = Some(map.next_value::<u32>()?),
                _ => unreachable!("closed field seed"),
            }
        }
        if seen & 0b1_1111 != 0b1_1111 {
            return Err(de::Error::custom("Native BOOT receipt fields differ"));
        }
        let has_retired_policy = match (policy_object, policy_sha256, policy_count) {
            (None, None, None) => false,
            (Some(object), Some(sha256), Some(identity_count)) => {
                RetiredProjectionPolicyRefV1 {
                    object,
                    sha256,
                    identity_count,
                }
                .validate_v1()
                .map_err(de::Error::custom)?;
                true
            }
            _ => return Err(de::Error::custom("Native BOOT retirement policy is incomplete")),
        };
        Ok(BootSelectorV1 {
            nonce: nonce.expect("presence mask"),
            operation: operation.expect("presence mask"),
            has_retired_policy,
        })
    }
}

struct SelectionVisitorV1;

impl<'de> Visitor<'de> for SelectionVisitorV1 {
    type Value = Option<(NativeCutoverDigest64V1, OperationKey)>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a closed deployment selection in schema 2 or 3")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut seen = 0u16;
        let mut mode = None;
        let mut schema = None;
        let mut boot = None;
        while let Some(key) = map.next_key_seed(ClosedFieldKeySeedV1(SELECTION_FIELDS_V1))? {
            let bit = match key {
                "binary" => 1,
                "binary_sha256" => 2,
                "environment" => 4,
                "mode" => 8,
                "schema_version" => 16,
                "state_root" => 32,
                "native_boot_receipt" => 64,
                "native_config" => 128,
                "native_config_sha256" => 256,
                _ => unreachable!("closed field seed"),
            };
            if seen & bit != 0 {
                return Err(de::Error::duplicate_field(key));
            }
            seen |= bit;
            match key {
                "mode" => mode = Some(map.next_value::<&str>()?),
                "schema_version" => schema = Some(map.next_value::<u64>()?),
                "native_boot_receipt" => boot = Some(map.next_value_seed(BootSeedV1)?),
                _ => { let _: de::IgnoredAny = map.next_value()?; }
            }
        }
        match (mode, schema, boot) {
            (Some("legacy"), Some(2), None) if seen == 0b00_0011_1111 => Ok(None),
            (Some("native"), Some(version @ (2 | 3)), Some(boot))
                if seen == 0b01_1111_1111 && boot.has_retired_policy == (version == 3) =>
            {
                Ok(Some((boot.nonce, boot.operation)))
            }
            _ => Err(de::Error::custom("selection version, mode or fields differ")),
        }
    }
}

struct BootSeedV1;

impl<'de> de::DeserializeSeed<'de> for BootSeedV1 {
    type Value = BootSelectorV1;

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(BootVisitorV1)
    }
}

/// Project the saved BOOT operation and root nonce from already retained,
/// hash-checked selection bytes. Legacy schema-2 returns `None`. Native
/// schema-3 requires the complete retirement policy reference. The final Q
/// deployment decoder and physical Native root verification remain the
/// authority for selection.
pub fn read_native_cutover_before_native_selector_v1(
    bytes: &[u8],
) -> Result<Option<(NativeCutoverDigest64V1, OperationKey)>, NativeCutoverEvidenceSyntaxErrorV1> {
    if bytes.is_empty() {
        return Err(NativeCutoverEvidenceSyntaxErrorV1::Empty);
    }
    if bytes.len() > MAX_SELECTION_BYTES_V1 {
        return Err(NativeCutoverEvidenceSyntaxErrorV1::TooLarge);
    }
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let selected = serde::Deserializer::deserialize_map(&mut decoder, SelectionVisitorV1)
        .map_err(|_| NativeCutoverEvidenceSyntaxErrorV1::NonCanonical)?;
    decoder.end().map_err(|_| NativeCutoverEvidenceSyntaxErrorV1::TrailingData)?;
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPERATION: &str = "{\"epoch\":1,\"producer\":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],\"sequence\":1,\"stream\":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],\"stream_epoch\":1,\"workspace\":{\"collection\":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],\"slot\":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],\"tenant\":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],\"worktree\":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1]}}";

    fn selection(mode: &str, operation: &str) -> String {
        let common = format!("\"binary\":\"/bin/searchd\",\"binary_sha256\":\"{}\",\"environment\":{{\"LANG\":\"ko_KR.UTF-8\"}},\"mode\":\"{}\",\"schema_version\":2,\"state_root\":\"/state\"", "a".repeat(64), mode);
        if mode == "legacy" {
            format!("{{{common}}}")
        } else {
            format!("{{{common},\"native_boot_receipt\":{{\"root_nonce_hex\":\"{}\",\"manifest_operation\":{},\"manifest_object_hex\":\"{}\",\"source_cut_digest_hex\":\"{}\",\"install_catalog_digest_hex\":\"{}\"}},\"native_config\":\"/state/config.json\",\"native_config_sha256\":\"{}\"}}", "1".repeat(64), operation, "2".repeat(64), "3".repeat(64), "4".repeat(64), "5".repeat(64))
        }
    }

    #[test]
    fn schema2_legacy_and_native_project_only_saved_boot_identity() {
        assert_eq!(read_native_cutover_before_native_selector_v1(selection("legacy", OPERATION).as_bytes()), Ok(None));
        let (nonce, operation) = read_native_cutover_before_native_selector_v1(selection("native", OPERATION).as_bytes())
            .expect("closed schema-2 Native selector").expect("Native selector");
        assert_eq!(nonce.0, [b'1'; 64]);
        assert_eq!(operation.sequence, 1);
    }

    #[test]
    fn native_schema3_requires_exact_retirement_fields_and_validates_optional_route() {
        let read = |wire: &serde_json::Value| {
            read_native_cutover_before_native_selector_v1(&serde_json::to_vec(wire).unwrap())
        };
        let mut native: serde_json::Value =
            serde_json::from_str(&selection("native", OPERATION)).unwrap();
        let saved = read(&native).expect("plain schema-2 Native selector");
        native["native_boot_receipt"]["protected_source_routes_hex"] =
            serde_json::json!("6".repeat(64));
        assert_eq!(read(&native), Ok(saved));
        native["native_boot_receipt"]["protected_source_routes_hex"] =
            serde_json::json!("A".repeat(64));
        assert!(read(&native).is_err());
        native["native_boot_receipt"]["protected_source_routes_hex"] =
            serde_json::json!("6".repeat(64));
        native["native_boot_receipt"]["retired_policy_object_hex"] =
            serde_json::json!("7".repeat(64));
        native["native_boot_receipt"]["retired_policy_sha256_hex"] =
            serde_json::json!("8".repeat(64));
        native["native_boot_receipt"]["retired_policy_identity_count"] = serde_json::json!(0);
        assert!(read(&native).is_err(), "schema-2 cannot carry retirement");

        native["schema_version"] = serde_json::json!(3);
        assert_eq!(read(&native), Ok(saved));
        let mut unrouted = native.clone();
        assert!(
            unrouted["native_boot_receipt"]
                .as_object_mut()
                .unwrap()
                .remove("protected_source_routes_hex")
                .is_some()
        );
        assert_eq!(read(&unrouted), Ok(saved));
        let duplicate = serde_json::to_string(&native).unwrap().replacen(
            "\"retired_policy_identity_count\":0",
            "\"retired_policy_identity_count\":0,\"retired_policy_identity_count\":0",
            1,
        );
        assert!(read_native_cutover_before_native_selector_v1(duplicate.as_bytes()).is_err());
        let mut partial = native.clone();
        assert!(
            partial["native_boot_receipt"]
                .as_object_mut()
                .unwrap()
                .remove("retired_policy_sha256_hex")
                .is_some()
        );
        assert!(read(&partial).is_err(), "policy tuple is required");
        let mut zero_object = native.clone();
        zero_object["native_boot_receipt"]["retired_policy_object_hex"] =
            serde_json::json!("0".repeat(64));
        assert!(read(&zero_object).is_err());
        native["native_boot_receipt"]["retired_policy_identity_count"] =
            serde_json::json!(u32::MAX);
        assert!(read(&native).is_err(), "policy count is bounded");

        let legacy =
            selection("legacy", OPERATION).replace("\"schema_version\":2", "\"schema_version\":3");
        assert!(read_native_cutover_before_native_selector_v1(legacy.as_bytes()).is_err());
    }

    #[test]
    fn foreign_schema_duplicate_and_missing_fields_refuse() {
        let native = selection("native", OPERATION);
        assert!(read_native_cutover_before_native_selector_v1(native.replace("\"schema_version\":2", "\"schema_version\":3").as_bytes()).is_err());
        assert!(read_native_cutover_before_native_selector_v1(native.replace("\"mode\":\"native\"", "\"mode\":\"native\",\"mode\":\"native\"").as_bytes()).is_err());
        assert!(read_native_cutover_before_native_selector_v1(native.replace(",\"native_config_sha256\":\"", ",\"wrong_config_sha256\":\"").as_bytes()).is_err());
        assert!(read_native_cutover_before_native_selector_v1(native.replace("\"source_cut_digest_hex\":\"3333333333333333333333333333333333333333333333333333333333333333\",", "").as_bytes()).is_err());
    }

    #[test]
    fn full_u64_operation_sequence_and_foreign_receipt_refuse() {
        let operation = OPERATION.replace("\"sequence\":1", "\"sequence\":18446744073709551615");
        let native = selection("native", &operation);
        let (_, selected) = read_native_cutover_before_native_selector_v1(native.as_bytes())
            .expect("u64 max is exact JSON integer").expect("Native selector");
        assert_eq!(selected.sequence, u64::MAX);
        assert!(read_native_cutover_before_native_selector_v1(native.replace(&"1".repeat(64), &"0".repeat(64)).as_bytes()).is_err());
        assert!(read_native_cutover_before_native_selector_v1(native.replace("\"manifest_operation\":", "\"foreign_operation\":").as_bytes()).is_err());
    }
}
