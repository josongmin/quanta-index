use std::path::PathBuf;

use anyhow::{Result, bail};
use quanta_index_contract::RepoId;
use quanta_index_contract::index::AuxiliaryGenerationKeyV1;
use quanta_index_contract::index::Digest;
use quanta_index_contract::native_cutover_evidence_v1::NativeCutoverRecoveryModeV1;

use super::native::parse_digest;

/// Expected identities for the inherited-descriptor recovery protocol.
/// Parsing this command establishes no custody or publication authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeRecoveryCommandV1 {
    pub selection_path: PathBuf,
    pub expected_current_sha256: Digest,
    pub candidate_path: PathBuf,
    pub expected_candidate_sha256: Digest,
    pub mode: NativeCutoverRecoveryModeV1,
    /// Only reconcile an already published selection under the installed recovery verifier.
    pub settle_published_only: bool,
    pub config_path: PathBuf,
    pub config_digest: Digest,
    pub parent_executable: PathBuf,
    pub parent_executable_sha256: Digest,
}

/// Scope of the private History exporter. The all-repo modes name their source
/// explicitly; Legacy and Native readers retain separate physical custody.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeHistoryExportScopeV1 {
    Protected,
    Checkpoint(AuxiliaryGenerationKeyV1),
    Inventory(RepoId),
    CheckpointAll(AuxiliaryGenerationKeyV1),
    InventoryAll,
    NativeInventoryAll,
}

/// A private, read-only child request. Parsing never grants Legacy root custody.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeHistoryExportCommandV1 {
    pub selection_path: PathBuf,
    pub expected_current_sha256: Digest,
    pub scope: NativeHistoryExportScopeV1,
    pub config_path: PathBuf,
    pub config_digest: Digest,
}

/// Private complete Source export from one pinned active Legacy selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSourceExportCommandV1 {
    pub selection_path: PathBuf,
    pub expected_current_sha256: Digest,
    pub config_path: PathBuf,
    pub config_digest: Digest,
    /// Explicitly select the independently authenticated Native Source format.
    pub source_only_native: bool,
}

impl NativeSourceExportCommandV1 {
    pub fn validate(&self) -> Result<()> {
        if !self.selection_path.is_absolute() || !self.config_path.is_absolute() {
            bail!("Source exporter selection and config paths must be absolute");
        }
        Ok(())
    }
}

impl NativeHistoryExportCommandV1 {
    pub fn validate(&self) -> Result<()> {
        if !self.selection_path.is_absolute() || !self.config_path.is_absolute() {
            bail!("History exporter selection and config paths must be absolute");
        }
        Ok(())
    }
}

impl NativeRecoveryCommandV1 {
    /// Validate direct callers before opening any configuration or descriptor.
    pub fn validate(&self) -> Result<()> {
        for (flag, path) in [
            ("--selection-path", &self.selection_path),
            ("--candidate-path", &self.candidate_path),
            ("--config", &self.config_path),
            ("--parent-executable", &self.parent_executable),
        ] {
            if !path.is_absolute() {
                bail!("recover-native {flag} requires an absolute path");
            }
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
pub(super) struct NativeRecoveryInput {
    selection_path: Option<String>,
    expected_current_sha256: Option<String>,
    candidate_path: Option<String>,
    expected_candidate_sha256: Option<String>,
    mode: Option<String>,
    settle_published_only: bool,
    parent_executable: Option<String>,
    parent_executable_sha256: Option<String>,
}

impl NativeRecoveryInput {
    pub(super) fn finish_source_export(
        self,
        source_only_native: bool,
        config_path: Option<PathBuf>,
        config_digest: Option<String>,
    ) -> Result<NativeSourceExportCommandV1> {
        if self.candidate_path.is_some()
            || self.expected_candidate_sha256.is_some()
            || self.mode.is_some()
            || self.settle_published_only
            || self.parent_executable.is_some()
            || self.parent_executable_sha256.is_some()
        {
            bail!("Source exporter accepts only the active selection binding");
        }
        let command = NativeSourceExportCommandV1 {
            selection_path: PathBuf::from(
                self.selection_path
                    .ok_or_else(|| anyhow::anyhow!("Source exporter requires --selection-path"))?,
            ),
            expected_current_sha256: parse_digest(
                &self.expected_current_sha256.ok_or_else(|| {
                    anyhow::anyhow!("Source exporter requires --expected-current-sha256")
                })?,
                "--expected-current-sha256",
            )?,
            config_path: config_path
                .ok_or_else(|| anyhow::anyhow!("Source exporter requires --config"))?,
            config_digest: parse_digest(
                &config_digest
                    .ok_or_else(|| anyhow::anyhow!("Source exporter requires --config-digest"))?,
                "--config-digest",
            )?,
            source_only_native,
        };
        command.validate()?;
        Ok(command)
    }
    pub(super) fn finish_history_export(
        self,
        generation_json: Option<String>,
        inventory_repo_id: Option<String>,
        protected_native_heads: Option<String>,
        all_history_inventory: Option<String>,
        all_history_checkpoint_json: Option<String>,
        all_native_history_inventory: Option<String>,
        config_path: Option<PathBuf>,
        config_digest: Option<String>,
    ) -> Result<NativeHistoryExportCommandV1> {
        if self.candidate_path.is_some()
            || self.expected_candidate_sha256.is_some()
            || self.mode.is_some()
            || self.settle_published_only
            || self.parent_executable.is_some()
            || self.parent_executable_sha256.is_some()
        {
            bail!("History exporter accepts only the active selection binding");
        }
        let selected = [generation_json.is_some(), inventory_repo_id.is_some(),
            protected_native_heads.is_some(), all_history_inventory.is_some(),
            all_history_checkpoint_json.is_some(), all_native_history_inventory.is_some()]
            .into_iter().filter(|selected| *selected).count();
        if selected != 1 {
            bail!("History exporter requires exactly one export scope");
        }
        let scope = if let Some(value) = protected_native_heads {
            if value != "true" {
                bail!("Native protected export requires --protected-native-heads true");
            }
            NativeHistoryExportScopeV1::Protected
        } else if let Some(value) = all_history_inventory {
            if value != "true" {
                bail!("all-repository History inventory requires true");
            }
            NativeHistoryExportScopeV1::InventoryAll
        } else if let Some(value) = all_native_history_inventory {
            if value != "true" {
                bail!("all-repository Native History inventory requires true");
            }
            NativeHistoryExportScopeV1::NativeInventoryAll
        } else if let Some(raw) = all_history_checkpoint_json {
            if raw.len() > 4096 {
                bail!("History exporter generation exceeds input bound");
            }
            NativeHistoryExportScopeV1::CheckpointAll(serde_json::from_str(&raw)?)
        } else if let Some(raw) = generation_json {
            if raw.len() > 4096 {
                bail!("History exporter generation exceeds input bound");
            }
            NativeHistoryExportScopeV1::Checkpoint(serde_json::from_str(&raw)?)
        } else if let Some(raw) = inventory_repo_id {
            if raw.len() > 4096 {
                bail!("History exporter repository exceeds input bound");
            }
            NativeHistoryExportScopeV1::Inventory(RepoId::new(raw)?)
        } else {
            unreachable!("one export scope was checked above")
        };
        let command = NativeHistoryExportCommandV1 {
            selection_path: PathBuf::from(
                self.selection_path
                    .ok_or_else(|| anyhow::anyhow!("History exporter requires --selection-path"))?,
            ),
            expected_current_sha256: parse_digest(
                &self.expected_current_sha256.ok_or_else(|| {
                    anyhow::anyhow!("History exporter requires --expected-current-sha256")
                })?,
                "--expected-current-sha256",
            )?,
            scope,
            config_path: config_path
                .ok_or_else(|| anyhow::anyhow!("History exporter requires --config"))?,
            config_digest: parse_digest(
                &config_digest
                    .ok_or_else(|| anyhow::anyhow!("History exporter requires --config-digest"))?,
                "--config-digest",
            )?,
        };
        command.validate()?;
        Ok(command)
    }
    pub(super) fn has_any(&self) -> bool {
        self.selection_path.is_some()
            || self.expected_current_sha256.is_some()
            || self.candidate_path.is_some()
            || self.expected_candidate_sha256.is_some()
            || self.mode.is_some()
            || self.settle_published_only
            || self.parent_executable.is_some()
            || self.parent_executable_sha256.is_some()
    }

    pub(super) fn parse_flag(
        &mut self,
        flag: &str,
        args: &mut impl Iterator<Item = String>,
    ) -> Result<bool> {
        if flag == "--settle-published-only" {
            if std::mem::replace(&mut self.settle_published_only, true) {
                bail!("{flag} may be specified only once");
            }
            return Ok(true);
        }
        let target = match flag {
            "--selection-path" => &mut self.selection_path,
            "--expected-current-sha256" => &mut self.expected_current_sha256,
            "--candidate-path" => &mut self.candidate_path,
            "--expected-candidate-sha256" => &mut self.expected_candidate_sha256,
            "--mode" => &mut self.mode,
            "--parent-executable" => &mut self.parent_executable,
            "--parent-executable-sha256" => &mut self.parent_executable_sha256,
            _ => return Ok(false),
        };
        let value = args
            .next()
            .ok_or_else(|| anyhow::anyhow!("{flag} requires a value"))?;
        if target.replace(value).is_some() {
            bail!("{flag} may be specified only once");
        }
        Ok(true)
    }

    pub(super) fn finish(
        self,
        config_path: Option<PathBuf>,
        config_digest: Option<String>,
    ) -> Result<NativeRecoveryCommandV1> {
        let required = |value: Option<String>, flag: &str| {
            value.ok_or_else(|| anyhow::anyhow!("recover-native requires {flag}"))
        };
        let mode = match required(self.mode, "--mode")?.as_str() {
            "rollback-legacy" => NativeCutoverRecoveryModeV1::RollbackLegacy,
            "restore-forward" => NativeCutoverRecoveryModeV1::RestoreForward,
            _ => bail!("recover-native --mode requires rollback-legacy or restore-forward"),
        };
        let command = NativeRecoveryCommandV1 {
            selection_path: PathBuf::from(required(self.selection_path, "--selection-path")?),
            expected_current_sha256: parse_digest(
                &required(self.expected_current_sha256, "--expected-current-sha256")?,
                "--expected-current-sha256",
            )?,
            candidate_path: PathBuf::from(required(self.candidate_path, "--candidate-path")?),
            expected_candidate_sha256: parse_digest(
                &required(
                    self.expected_candidate_sha256,
                    "--expected-candidate-sha256",
                )?,
                "--expected-candidate-sha256",
            )?,
            mode,
            settle_published_only: self.settle_published_only,
            config_path: config_path
                .ok_or_else(|| anyhow::anyhow!("recover-native requires --config"))?,
            config_digest: parse_digest(
                &required(config_digest, "--config-digest")?,
                "--config-digest",
            )?,
            parent_executable: PathBuf::from(required(
                self.parent_executable,
                "--parent-executable",
            )?),
            parent_executable_sha256: parse_digest(
                &required(self.parent_executable_sha256, "--parent-executable-sha256")?,
                "--parent-executable-sha256",
            )?,
        };
        command.validate()?;
        Ok(command)
    }
}
