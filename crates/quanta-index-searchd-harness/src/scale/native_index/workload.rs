//! Explicit foreground work funding for the index-native scale fixture.
//!
//! These are cumulative logical-work limits. They do not replace the daemon's
//! execution grants, physical capacity, maintenance limits, or request deadline.

use anyhow::{Context as _, Result};
use quanta_index_core::index::WorkLimits;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{ScaleTier, params_for};

const WORK_LIMITS_ENV: &str = "QUANTA_INDEX_NATIVE_WORK_LIMITS_JSON";

pub(super) const FIXED_FOREGROUND_WORK_LIMITS: WorkLimits = WorkLimits {
    reads: 1_000_000,
    writes: 1_000_000,
    bytes: 536_870_912,
    mutations: 1_000_000,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NativeWorkloadMode {
    Fixed,
    Explicit,
}

impl NativeWorkloadMode {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::Explicit => "explicit",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct NativeForegroundWorkload {
    pub(super) limits: WorkLimits,
    pub(super) mode: NativeWorkloadMode,
}

impl NativeForegroundWorkload {
    /// Exact selected fixture input for benchmark metadata and source binding.
    pub(super) fn metadata(&self) -> Value {
        json!({
            "mode": self.mode.as_str(),
            "work_limits": {
                "reads": self.limits.reads,
                "writes": self.limits.writes,
                "bytes": self.limits.bytes,
                "mutations": self.limits.mutations,
            },
        })
    }
}

// Only the external JSON boundary uses this decoder shape. The internal limits
// remain the Core WorkLimits type consumed by the daemon configuration.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExternalWorkLimits {
    reads: u64,
    writes: u64,
    bytes: u64,
    mutations: u64,
}

impl From<ExternalWorkLimits> for WorkLimits {
    fn from(value: ExternalWorkLimits) -> Self {
        Self {
            reads: value.reads,
            writes: value.writes,
            bytes: value.bytes,
            mutations: value.mutations,
        }
    }
}

/// Select a fixed or separately declared foreground profile before fixture
/// setup. The generator floor is necessary admission, not sufficient funding.
pub(super) fn selected_workload_from_env(tier: ScaleTier) -> Result<NativeForegroundWorkload> {
    match std::env::var(WORK_LIMITS_ENV) {
        Ok(raw) => selected_workload_from_json(tier, Some(&raw)),
        Err(std::env::VarError::NotPresent) => selected_workload_from_json(tier, None),
        Err(std::env::VarError::NotUnicode(_)) => {
            anyhow::bail!("{WORK_LIMITS_ENV} must be UTF-8")
        }
    }
}

pub(super) fn selected_workload_from_json(
    tier: ScaleTier,
    raw: Option<&str>,
) -> Result<NativeForegroundWorkload> {
    let selected = match raw {
        Some(raw) => NativeForegroundWorkload {
            limits: serde_json::from_str::<ExternalWorkLimits>(raw)
                .with_context(|| {
                    format!("{WORK_LIMITS_ENV} must contain exactly four unsigned integer fields")
                })?
                .into(),
            mode: NativeWorkloadMode::Explicit,
        },
        None => NativeForegroundWorkload {
            limits: FIXED_FOREGROUND_WORK_LIMITS,
            mode: NativeWorkloadMode::Fixed,
        },
    };
    let validation = (|| -> Result<()> {
        anyhow::ensure!(
            selected.limits.reads > 0
                && selected.limits.writes > 0
                && selected.limits.bytes > 0
                && selected.limits.mutations > 0,
            "{WORK_LIMITS_ENV} work limits must all be positive"
        );
        require_generator_mutation_floor(tier, selected.limits)
    })();
    validation.map_err(|error| {
        let reason = error.to_string();
        error.context(format!(
            "{reason}; selected native foreground funding={}",
            selected.metadata()
        ))
    })?;
    Ok(selected)
}

fn require_generator_mutation_floor(tier: ScaleTier, limits: WorkLimits) -> Result<()> {
    let params = params_for(tier);
    let files = u64::from(params.repo_count)
        .checked_mul(u64::from(params.files_per_repo))
        .context("native workload file count overflow")?;
    // Each generated line has at least two indexable tokens. The guaranteed
    // hit adds three, and the scoped repo/file anchors add two each.
    let minimum_mutations = u64::from(params.avg_file_lines)
        .checked_mul(2)
        .and_then(|tokens| tokens.checked_add(7))
        .and_then(|tokens| tokens.checked_mul(files))
        .context("native workload mutation floor overflow")?;
    anyhow::ensure!(
        minimum_mutations <= limits.mutations,
        "native foreground profile cannot admit tier={}: minimum token mutations={} exceed configured mutations={}; a separately specified workload profile is required before daemon execution",
        tier.as_str(),
        minimum_mutations,
        limits.mutations
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_defaults_keep_exact_small_profile_and_refuse_impossible_tiers() -> Result<()> {
        for tier in [ScaleTier::Small, ScaleTier::Medium] {
            let selected = selected_workload_from_json(tier, None)?;
            anyhow::ensure!(selected.mode == NativeWorkloadMode::Fixed);
            anyhow::ensure!(selected.limits == FIXED_FOREGROUND_WORK_LIMITS);
            anyhow::ensure!(
                selected.metadata()
                    == json!({
                        "mode":"fixed",
                        "work_limits":{"reads":1_000_000,"writes":1_000_000,
                            "bytes":536_870_912,"mutations":1_000_000}
                    })
            );
        }
        for (tier, minimum) in [
            (ScaleTier::Large, "1011712"),
            (ScaleTier::Xlarge, "13336576"),
        ] {
            let error = selected_workload_from_json(tier, None)
                .err()
                .context("fixed profile admitted impossible tier")?;
            let message = error.to_string();
            anyhow::ensure!(message.contains(tier.as_str()));
            anyhow::ensure!(message.contains(minimum));
            anyhow::ensure!(message.contains("configured mutations=1000000"));
        }
        Ok(())
    }

    #[test]
    fn external_json_is_exact_and_positive() -> Result<()> {
        for raw in [
            "{}",
            "[]",
            r#"{"reads":1,"writes":1,"bytes":1}"#,
            r#"{"reads":1,"writes":1,"bytes":1,"mutations":1000000,"extra":1}"#,
            r#"{"reads":1,"reads":2,"writes":1,"bytes":1,"mutations":1000000}"#,
            r#"{"reads":0,"writes":1,"bytes":1,"mutations":1000000}"#,
            r#"{"reads":-1,"writes":1,"bytes":1,"mutations":1000000}"#,
            r#"{"reads":1.5,"writes":1,"bytes":1,"mutations":1000000}"#,
            r#"{"reads":18446744073709551616,"writes":1,"bytes":1,"mutations":1000000}"#,
            "",
        ] {
            anyhow::ensure!(
                selected_workload_from_json(ScaleTier::Small, Some(raw)).is_err(),
                "invalid external work limits accepted: {raw}"
            );
        }
        let zero = r#"{"reads":0,"writes":3,"bytes":4,"mutations":1000000}"#;
        let error = selected_workload_from_json(ScaleTier::Small, Some(zero))
            .err()
            .context("decoded zero-value profile was accepted")?;
        let message = error.to_string();
        anyhow::ensure!(message.contains("must all be positive"));
        anyhow::ensure!(
            message.contains(
                &json!({
                    "mode":"explicit",
                    "work_limits":{
                        "reads":0,"writes":3,"bytes":4,"mutations":1_000_000
                    }
                })
                .to_string()
            ),
            "decoded positive-value refusal omitted exact selected funding: {message}"
        );
        Ok(())
    }

    #[test]
    fn explicit_large_floor_is_necessary_only() -> Result<()> {
        let insufficient = r#"{"reads":2,"writes":3,"bytes":4,"mutations":1011711}"#;
        let error = selected_workload_from_json(ScaleTier::Large, Some(insufficient))
            .err()
            .context("explicit profile admitted below-floor mutations")?;
        let message = error.to_string();
        anyhow::ensure!(message.contains("minimum token mutations=1011712"));
        anyhow::ensure!(
            message.contains(
                &json!({
                    "mode":"explicit",
                    "work_limits":{
                        "reads":2,"writes":3,"bytes":4,"mutations":1_011_711
                    }
                })
                .to_string()
            ),
            "decoded refusal omitted exact selected funding: {message}"
        );

        let declared = r#"{"reads":1,"writes":1,"bytes":1,"mutations":1011712}"#;
        let selected = selected_workload_from_json(ScaleTier::Large, Some(declared))?;
        anyhow::ensure!(selected.mode == NativeWorkloadMode::Explicit);
        anyhow::ensure!(selected.limits.mutations == 1_011_712);
        anyhow::ensure!(selected.limits.reads == 1);
        anyhow::ensure!(selected.limits.writes == 1);
        anyhow::ensure!(selected.limits.bytes == 1);
        anyhow::ensure!(selected.metadata().get("mode") == Some(&json!("explicit")));
        Ok(())
    }
}
