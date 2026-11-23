//! Facts FULL authority is issued only from its dedicated signed active domain.

#[cfg(target_os = "linux")]
use super::{
    hex_bytes_v1, InstalledArtifactTrustAdmissionV1, InstalledTrustErrorV1, VerifyContextV1,
};
#[cfg(target_os = "linux")]
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledNativeFactsPublicationV3(NativeInstallAxesV3);

impl InstalledNativeFactsPublicationV3 {
    pub fn repository_root_v3(&self) -> &str {
        &self.0.repository_root_v3
    }

    pub fn q_image_sha256_v3(&self) -> &[u8; 32] {
        &self.0.q_image_sha256_v3
    }

    pub fn q_config_sha256_v3(&self) -> &[u8; 32] {
        &self.0.q_config_sha256_v3
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NativeInstallAxesV3 {
    pub(super) repository_root_v3: String,
    pub(super) q_image_sha256_v3: [u8; 32],
    pub(super) q_config_sha256_v3: [u8; 32],
}

#[cfg(target_os = "linux")]
pub(super) fn parse_native_facts_publication_v3<A: InstalledArtifactTrustAdmissionV1>(
    context_v3: &mut VerifyContextV1<'_, A>,
    payload_v3: &Value,
) -> Result<InstalledNativeFactsPublicationV3, InstalledTrustErrorV1> {
    let fields_v3 = payload_v3
        .get("native_facts_publication")
        .and_then(Value::as_object)
        .filter(|fields_v3| fields_v3.len() == 3)
        .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)?;
    parse_native_install_axes_v3(context_v3, fields_v3).map(InstalledNativeFactsPublicationV3)
}

#[cfg(target_os = "linux")]
pub(super) fn parse_native_install_axes_v3<A: InstalledArtifactTrustAdmissionV1>(
    context_v3: &mut VerifyContextV1<'_, A>,
    fields_v3: &Map<String, Value>,
) -> Result<NativeInstallAxesV3, InstalledTrustErrorV1> {
    let field_v3 = |name_v3| {
        fields_v3
            .get(name_v3)
            .and_then(Value::as_str)
            .ok_or(InstalledTrustErrorV1::InvalidActiveGeneration)
    };
    let root_v3 = field_v3("repository_root")?;
    if root_v3.len() <= 1
        || !root_v3.starts_with('/')
        || !root_v3.is_ascii()
        || root_v3.bytes().any(|byte_v3| byte_v3.is_ascii_control())
        || root_v3
            .split('/')
            .skip(1)
            .any(|part_v3| part_v3.is_empty() || part_v3 == "." || part_v3 == "..")
    {
        return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
    }
    let q_image_sha256_v3 = hex_bytes_v1::<32>(field_v3("q_image_sha256")?)?;
    let q_config_sha256_v3 = hex_bytes_v1::<32>(field_v3("q_config_sha256")?)?;
    if q_image_sha256_v3 == [0; 32] || q_config_sha256_v3 == [0; 32] {
        return Err(InstalledTrustErrorV1::InvalidActiveGeneration);
    }
    context_v3.memory_v1(root_v3.len())?;
    Ok(NativeInstallAxesV3 {
        repository_root_v3: root_v3.to_owned(),
        q_image_sha256_v3,
        q_config_sha256_v3,
    })
}
