use std::path::PathBuf;

use anyhow::{Result, bail};
use quanta_index_contract::index::{Digest, ObjectRef, OperationKey, RetiredProjectionPolicyRefV1};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeCommandModeV1 {
    Serve,
    Verify,
    VerifyCandidate,
    Select,
    SelectCandidate {
        root_nonce: [u8; 32],
        source_cut_digest: Digest,
        protected_source_routes: Option<ObjectRef>,
        retired_projection_policy: Option<RetiredProjectionPolicyRefV1>,
    },
    ServeCandidate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeBootIdentityV1 {
    pub root_nonce: [u8; 32],
    pub manifest_operation: OperationKey,
    pub manifest_object: ObjectRef,
    pub source_cut_digest: Digest,
    pub protected_source_routes: Option<ObjectRef>,
    pub retired_projection_policy: Option<RetiredProjectionPolicyRefV1>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeStateCommandV1 {
    pub mode: NativeCommandModeV1,
    pub config_path: PathBuf,
    pub identity: Option<NativeBootIdentityV1>,
    pub config_digest: Option<Digest>,
    pub install_catalog_digest: Option<Digest>,
    pub select_operation: Option<OperationKey>,
    pub receipt_path: Option<PathBuf>,
}

impl NativeStateCommandV1 {
    /// Enforce the typed command contract for direct Runtime callers as well
    /// as CLI input. This check performs no filesystem access.
    pub fn validate(&self) -> Result<()> {
        if !self.config_path.is_absolute() {
            bail!("native --config must be an absolute path");
        }
        if self
            .identity
            .is_some_and(|identity| identity.root_nonce == [0; 32])
        {
            bail!("native root nonce must be nonzero");
        }
        if self.identity.is_some_and(|identity| {
            identity
                .protected_source_routes
                .is_some_and(|reference| reference.0 == Digest([0; 32]))
        }) {
            bail!("protected Source route reference must be nonzero");
        }
        if let Some(policy) = self
            .identity
            .and_then(|identity| identity.retired_projection_policy)
        {
            policy
                .validate_v1()
                .map_err(|reason| anyhow::anyhow!(reason))?;
        }
        match self.mode {
            NativeCommandModeV1::Select | NativeCommandModeV1::SelectCandidate { .. } => {
                if self.identity.is_some()
                    || self.install_catalog_digest.is_some()
                    || self.select_operation.is_none()
                    || self.receipt_path.is_none()
                {
                    bail!("select-native-state requires operation and receipt only");
                }
                if !self
                    .receipt_path
                    .as_ref()
                    .is_some_and(|path| path.is_absolute())
                {
                    bail!("select-native-state --receipt must be an absolute path");
                }
                if let NativeCommandModeV1::SelectCandidate {
                    root_nonce,
                    source_cut_digest,
                    protected_source_routes,
                    retired_projection_policy,
                } = self.mode
                    && (self.config_digest.is_none()
                        || root_nonce == [0; 32]
                        || source_cut_digest == Digest([0; 32])
                        || protected_source_routes
                            .is_some_and(|reference| reference.0 == Digest([0; 32]))
                        || retired_projection_policy
                            .is_some_and(|reference| reference.validate_v1().is_err()))
                {
                    bail!(
                        "select-native-candidate requires pinned config, root and SourceCut digests"
                    );
                }
            }
            NativeCommandModeV1::Verify | NativeCommandModeV1::VerifyCandidate => {
                if self.identity.is_none()
                    || self.config_digest.is_none()
                    || self.install_catalog_digest.is_none()
                    || self.select_operation.is_some()
                    || self.receipt_path.is_some()
                {
                    bail!("native verification requires identity and install digests only");
                }
            }
            NativeCommandModeV1::Serve => {
                if self.install_catalog_digest.is_some()
                    || self.select_operation.is_some()
                    || self.receipt_path.is_some()
                    || self.identity.is_some() != self.config_digest.is_some()
                {
                    bail!(
                        "serve-native requires either plain config or selected identity and config digest"
                    );
                }
            }
            NativeCommandModeV1::ServeCandidate => {
                if self.identity.is_some()
                    || self.config_digest.is_none()
                    || self.install_catalog_digest.is_some()
                    || self.select_operation.is_some()
                    || self.receipt_path.is_some()
                {
                    bail!("serve-native-candidate requires only config and config digest");
                }
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct NativeCommandInput {
    pub config_path: Option<PathBuf>,
    pub root_nonce: Option<String>,
    pub manifest_operation: Option<String>,
    pub manifest_object: Option<String>,
    pub source_cut_digest: Option<String>,
    pub protected_source_routes: Option<String>,
    pub retired_policy_object: Option<String>,
    pub retired_policy_sha256: Option<String>,
    pub retired_policy_identity_count: Option<String>,
    pub config_digest: Option<String>,
    pub install_catalog_digest: Option<String>,
    pub receipt_path: Option<PathBuf>,
}

impl NativeCommandInput {
    pub(super) fn has_any(&self) -> bool {
        self.config_path.is_some()
            || self.root_nonce.is_some()
            || self.manifest_operation.is_some()
            || self.manifest_object.is_some()
            || self.source_cut_digest.is_some()
            || self.protected_source_routes.is_some()
            || self.retired_policy_object.is_some()
            || self.retired_policy_sha256.is_some()
            || self.retired_policy_identity_count.is_some()
            || self.config_digest.is_some()
            || self.install_catalog_digest.is_some()
            || self.receipt_path.is_some()
    }

    pub(super) fn finish_candidate(self) -> Result<NativeStateCommandV1> {
        let mut input = self;
        let root_nonce = parse_hex32(
            &input
                .root_nonce
                .take()
                .ok_or_else(|| anyhow::anyhow!("select-native-candidate requires --root-nonce"))?,
            "--root-nonce",
        )?;
        let source_cut_digest = parse_digest(
            &input.source_cut_digest.take().ok_or_else(|| {
                anyhow::anyhow!("select-native-candidate requires --source-cut-digest")
            })?,
            "--source-cut-digest",
        )?;
        let protected_source_routes = input
            .protected_source_routes
            .take()
            .map(|value| parse_digest(&value, "--protected-source-routes").map(ObjectRef))
            .transpose()?;
        let retired_projection_policy = parse_retired_policy(
            input.retired_policy_object.take(),
            input.retired_policy_sha256.take(),
            input.retired_policy_identity_count.take(),
        )?;
        input.finish(NativeCommandModeV1::SelectCandidate {
            root_nonce,
            source_cut_digest,
            protected_source_routes,
            retired_projection_policy,
        })
    }

    pub(super) fn finish(self, mode: NativeCommandModeV1) -> Result<NativeStateCommandV1> {
        let config_path = self
            .config_path
            .ok_or_else(|| anyhow::anyhow!("native command requires --config PATH"))?;
        if !config_path.is_absolute() {
            bail!("native --config must be an absolute path");
        }
        let manifest_operation = self
            .manifest_operation
            .map(|value| parse_operation(&value))
            .transpose()?;
        let config_digest = self
            .config_digest
            .map(|value| parse_digest(&value, "--config-digest"))
            .transpose()?;

        if matches!(
            mode,
            NativeCommandModeV1::Select | NativeCommandModeV1::SelectCandidate { .. }
        ) {
            if self.root_nonce.is_some()
                || self.manifest_object.is_some()
                || self.source_cut_digest.is_some()
                || self.protected_source_routes.is_some()
                || self.retired_policy_object.is_some()
                || self.retired_policy_sha256.is_some()
                || self.retired_policy_identity_count.is_some()
                || self.install_catalog_digest.is_some()
            {
                bail!(
                    "select-native-state accepts only the manifest operation, config digest and receipt"
                );
            }
            let select_operation = manifest_operation.ok_or_else(|| {
                anyhow::anyhow!("select-native-state requires --manifest-operation JSON")
            })?;
            let receipt_path = self
                .receipt_path
                .ok_or_else(|| anyhow::anyhow!("select-native-state requires --receipt ABS"))?;
            if !receipt_path.is_absolute() {
                bail!("select-native-state --receipt must be an absolute path");
            }
            let command = NativeStateCommandV1 {
                mode,
                config_path,
                identity: None,
                config_digest,
                install_catalog_digest: None,
                select_operation: Some(select_operation),
                receipt_path: Some(receipt_path),
            };
            command.validate()?;
            return Ok(command);
        }

        if self.receipt_path.is_some() {
            bail!("--receipt is only valid for select-native-state");
        }
        if matches!(
            mode,
            NativeCommandModeV1::Serve | NativeCommandModeV1::ServeCandidate
        ) && self.install_catalog_digest.is_some()
        {
            bail!("--catalog-content-digest is only valid for verify-native-state");
        }
        let has_identity = self.root_nonce.is_some()
            || manifest_operation.is_some()
            || self.manifest_object.is_some()
            || self.source_cut_digest.is_some()
            || self.protected_source_routes.is_some()
            || self.retired_policy_object.is_some()
            || self.retired_policy_sha256.is_some()
            || self.retired_policy_identity_count.is_some();
        if mode == NativeCommandModeV1::ServeCandidate && has_identity {
            bail!("serve-native-candidate cannot name an installed BOOT identity");
        }
        if matches!(
            mode,
            NativeCommandModeV1::Serve | NativeCommandModeV1::ServeCandidate
        ) && !has_identity
        {
            let command = NativeStateCommandV1 {
                mode,
                config_path,
                identity: None,
                config_digest,
                install_catalog_digest: None,
                select_operation: None,
                receipt_path: None,
            };
            command.validate()?;
            return Ok(command);
        }

        let root_nonce = parse_hex32(
            &self
                .root_nonce
                .ok_or_else(|| anyhow::anyhow!("native identity requires --root-nonce"))?,
            "--root-nonce",
        )?;
        if root_nonce == [0; 32] {
            bail!("--root-nonce must be nonzero");
        }
        let manifest_operation = manifest_operation
            .ok_or_else(|| anyhow::anyhow!("native identity requires --manifest-operation JSON"))?;
        let manifest_object = ObjectRef(parse_digest(
            &self
                .manifest_object
                .ok_or_else(|| anyhow::anyhow!("native identity requires --manifest-object"))?,
            "--manifest-object",
        )?);
        let source_cut_digest = parse_digest(
            &self
                .source_cut_digest
                .ok_or_else(|| anyhow::anyhow!("native identity requires --source-cut-digest"))?,
            "--source-cut-digest",
        )?;
        let config_digest = config_digest
            .ok_or_else(|| anyhow::anyhow!("selected native command requires --config-digest"))?;
        let install_catalog_digest = if matches!(
            mode,
            NativeCommandModeV1::Verify | NativeCommandModeV1::VerifyCandidate
        ) {
            Some(parse_digest(
                &self.install_catalog_digest.ok_or_else(|| {
                    anyhow::anyhow!("verify-native-state requires --catalog-content-digest")
                })?,
                "--catalog-content-digest",
            )?)
        } else {
            None
        };
        let command = NativeStateCommandV1 {
            mode,
            config_path,
            identity: Some(NativeBootIdentityV1 {
                root_nonce,
                manifest_operation,
                manifest_object,
                source_cut_digest,
                protected_source_routes: self
                    .protected_source_routes
                    .map(|value| parse_digest(&value, "--protected-source-routes").map(ObjectRef))
                    .transpose()?,
                retired_projection_policy: parse_retired_policy(
                    self.retired_policy_object,
                    self.retired_policy_sha256,
                    self.retired_policy_identity_count,
                )?,
            }),
            config_digest: Some(config_digest),
            install_catalog_digest,
            select_operation: None,
            receipt_path: None,
        };
        command.validate()?;
        Ok(command)
    }
}

fn parse_retired_policy(
    object: Option<String>,
    sha256: Option<String>,
    count: Option<String>,
) -> Result<Option<RetiredProjectionPolicyRefV1>> {
    let (object, sha256, count) = match (object, sha256, count) {
        (None, None, None) => return Ok(None),
        (Some(object), Some(sha256), Some(count)) => (object, sha256, count),
        _ => bail!("retired policy requires object, SHA-256 and identity count together"),
    };
    let reference = RetiredProjectionPolicyRefV1 {
        object: ObjectRef(parse_digest(&object, "--retired-policy-object")?),
        sha256: parse_digest(&sha256, "--retired-policy-sha256")?,
        identity_count: count.parse()?,
    };
    reference
        .validate_v1()
        .map_err(|reason| anyhow::anyhow!(reason))?;
    Ok(Some(reference))
}

fn parse_operation(value: &str) -> Result<OperationKey> {
    // The canonical OperationKey decoder rejects duplicate, missing and unknown
    // fields, including those inside WorkspaceKey. Do not parse through Value.
    serde_json::from_str(value)
        .map_err(|error| anyhow::anyhow!("invalid --manifest-operation: {error}"))
}

pub(super) fn parse_digest(value: &str, flag: &str) -> Result<Digest> {
    parse_hex32(value, flag).map(Digest)
}

fn parse_hex32(value: &str, flag: &str) -> Result<[u8; 32]> {
    let bytes = value.as_bytes();
    if bytes.len() != 64 {
        bail!("{flag} requires exactly 64 lowercase hex digits");
    }
    let mut decoded = [0; 32];
    for (slot, pair) in decoded.iter_mut().zip(bytes.chunks_exact(2)) {
        let &[hi_byte, lo_byte] = pair else {
            bail!("{flag} requires exactly 64 lowercase hex digits");
        };
        let hi =
            hex_nibble(hi_byte).ok_or_else(|| anyhow::anyhow!("{flag} requires lowercase hex"))?;
        let lo =
            hex_nibble(lo_byte).ok_or_else(|| anyhow::anyhow!("{flag} requires lowercase hex"))?;
        *slot = (hi << 4) | lo;
    }
    Ok(decoded)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => byte.checked_sub(b'0'),
        b'a'..=b'f' => byte
            .checked_sub(b'a')
            .and_then(|value| value.checked_add(10)),
        _ => None,
    }
}
