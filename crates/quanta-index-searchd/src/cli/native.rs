use std::path::PathBuf;

use anyhow::{Result, bail};
use quanta_index_contract::index::{Digest, ObjectRef, OperationKey};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeCommandModeV1 {
    Serve,
    Verify,
    Select,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeBootIdentityV1 {
    pub root_nonce: [u8; 32],
    pub manifest_operation: OperationKey,
    pub manifest_object: ObjectRef,
    pub source_cut_digest: Digest,
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
        match self.mode {
            NativeCommandModeV1::Select => {
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
            }
            NativeCommandModeV1::Verify => {
                if self.identity.is_none()
                    || self.config_digest.is_none()
                    || self.install_catalog_digest.is_none()
                    || self.select_operation.is_some()
                    || self.receipt_path.is_some()
                {
                    bail!("verify-native-state requires identity and install digests only");
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
            || self.config_digest.is_some()
            || self.install_catalog_digest.is_some()
            || self.receipt_path.is_some()
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

        if mode == NativeCommandModeV1::Select {
            if self.root_nonce.is_some()
                || self.manifest_object.is_some()
                || self.source_cut_digest.is_some()
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
        if mode == NativeCommandModeV1::Serve && self.install_catalog_digest.is_some() {
            bail!("--catalog-content-digest is only valid for verify-native-state");
        }
        let has_identity = self.root_nonce.is_some()
            || manifest_operation.is_some()
            || self.manifest_object.is_some()
            || self.source_cut_digest.is_some();
        if mode == NativeCommandModeV1::Serve && !has_identity {
            if config_digest.is_some() {
                bail!("selected serve-native requires a complete identity with --config-digest");
            }
            let command = NativeStateCommandV1 {
                mode,
                config_path,
                identity: None,
                config_digest: None,
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
        let install_catalog_digest = if mode == NativeCommandModeV1::Verify {
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

fn parse_operation(value: &str) -> Result<OperationKey> {
    // The canonical OperationKey decoder rejects duplicate, missing and unknown
    // fields, including those inside WorkspaceKey. Do not parse through Value.
    serde_json::from_str(value)
        .map_err(|error| anyhow::anyhow!("invalid --manifest-operation: {error}"))
}

fn parse_digest(value: &str, flag: &str) -> Result<Digest> {
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
