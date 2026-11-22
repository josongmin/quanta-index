//! Effective limits for scale and open-loop process fixtures.

use std::time::Duration;

use anyhow::Result as AnyResult;
use quanta_index_ipc::ServerAdmissionPolicy;
use serde_json::{Value, json};

use crate::scale_disk_census::DiskObservationMode;
use crate::source_fixture::{SCALE_HISTORY_MAX_BYTES, SCALE_HISTORY_MAX_TOTAL_BYTES};

pub use crate::legacy_reference::DEFAULT_HISTORY_MAX_GENERATIONS;
/// Default harness retention in revision pairs.
pub const HARNESS_HISTORY_MAX_REVISION_PAIRS: usize = 128;

pub(super) const SCALE_INGEST_TEXT_BYTES: u64 = 128 * 1024 * 1024;
pub(super) const SCALE_INGEST_MAX_RECORDS: usize = 100_000;
pub(super) const SCALE_PROCESS_MEMORY_CEILING_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub(super) const SCALE_INGEST_VECTOR_BYTES: u64 = 256 * 1024 * 1024;
pub(super) const SCALE_CLIENT_IO_TIMEOUT: Duration = Duration::from_secs(600);
pub(super) const SCALE_QUERY_DISPATCH_BUDGET: Duration = Duration::from_secs(600);

pub(super) fn scale_query_admission_policy() -> AnyResult<ServerAdmissionPolicy> {
    let default = ServerAdmissionPolicy::DEFAULT;
    Ok(ServerAdmissionPolicy::new(
        default.max_connections(),
        default.dispatch_slots(),
        default.max_in_flight_per_repo(),
        default.queue_wait(),
        SCALE_QUERY_DISPATCH_BUDGET,
        default.io_timeout(),
    )?)
}

/// Explicit runtime policy for a scale measurement. Requested values remain
/// separate from the effective defaults in both success and refusal records.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScaleRuntimeConfig {
    pub client_timeout: Option<Duration>,
    pub history_max_bytes: Option<u64>,
    pub history_max_total_bytes: Option<u64>,
    pub disk_observation: DiskObservationMode,
}

impl ScaleRuntimeConfig {
    pub fn daemon_history_environment(self) -> AnyResult<[(&'static str, String); 2]> {
        Ok([
            (
                "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES",
                self.effective_history_max_bytes()?.to_string(),
            ),
            (
                "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES",
                self.effective_history_max_total_bytes()?.to_string(),
            ),
        ])
    }

    pub fn effective_timeout_ms(self) -> AnyResult<u64> {
        timeout_ms(self.client_timeout)
    }

    pub fn history_policy_id(self) -> AnyResult<&'static str> {
        match (self.history_max_bytes, self.history_max_total_bytes) {
            (None, None) => Ok("scale-supported-v1"),
            (Some(_), None) => Ok("explicit-pair-default-total-v1"),
            (Some(_), Some(_)) => Ok("explicit-pair-total-diagnostic-v1"),
            (None, Some(_)) => {
                anyhow::bail!("scale: total history override requires an explicit pair bound")
            }
        }
    }

    pub fn effective_history_max_total_bytes(self) -> AnyResult<u64> {
        let _policy = self.history_policy_id()?;
        let bytes = self
            .history_max_total_bytes
            .unwrap_or(SCALE_HISTORY_MAX_TOTAL_BYTES);
        if bytes == 0 {
            anyhow::bail!("scale: total history max bytes must be positive");
        }
        Ok(bytes)
    }

    pub fn effective_history_max_bytes(self) -> AnyResult<u64> {
        let bytes = self.history_max_bytes.unwrap_or(SCALE_HISTORY_MAX_BYTES);
        let total = self.effective_history_max_total_bytes()?;
        if !(1..=total).contains(&bytes) {
            anyhow::bail!(
                "scale: history max bytes must be in 1..={total} (effective total retention cap)"
            );
        }
        Ok(bytes)
    }

    pub fn execution_json(self) -> AnyResult<Value> {
        with_capacity_profile(json!({
            "disk_observation_mode": self.disk_observation.as_str(),
            "client_request_timeout_ms": self.effective_timeout_ms()?,
            "requested_client_request_timeout_ms": self.client_timeout.map(|_| self.effective_timeout_ms()).transpose()?,
            "history_max_generations": 2,
            "history_policy_id": self.history_policy_id()?,
            "history_max_bytes": self.effective_history_max_bytes()?,
            "requested_history_max_bytes": self.history_max_bytes,
            "history_max_revision_pairs": HARNESS_HISTORY_MAX_REVISION_PAIRS,
            "history_max_total_bytes": self.effective_history_max_total_bytes()?,
            "requested_history_max_total_bytes": self.history_max_total_bytes,
        }))
    }
}

pub fn daemon_scale_capacity_environment() -> [(&'static str, String); 6] {
    [
        (
            "QUANTA_INDEX_INGEST_MAX_RECORDS",
            SCALE_INGEST_MAX_RECORDS.to_string(),
        ),
        (
            "QUANTA_INDEX_INGEST_MAX_TEXT_BYTES",
            SCALE_INGEST_TEXT_BYTES.to_string(),
        ),
        (
            "QUANTA_INDEX_INGEST_MAX_VECTOR_BYTES",
            SCALE_INGEST_VECTOR_BYTES.to_string(),
        ),
        (
            "QUANTA_INDEX_SOURCE_PUBLICATION_MAX_BYTES",
            quanta_index_contract::SOURCE_PUBLICATION_UPLOAD_MAX_BYTES.to_string(),
        ),
        (
            "QUANTA_INDEX_QUERY_DISPATCH_BUDGET_MS",
            SCALE_QUERY_DISPATCH_BUDGET.as_millis().to_string(),
        ),
        (
            "QUANTA_INDEX_PROCESS_MEMORY_CEILING_BYTES",
            SCALE_PROCESS_MEMORY_CEILING_BYTES.to_string(),
        ),
    ]
}

pub(super) fn with_capacity_profile(mut value: Value) -> AnyResult<Value> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("scale: capacity profile requires a JSON object"))?;
    for (name, field) in [
        (
            "query_dispatch_budget_ms",
            json!(u64::try_from(
                scale_query_admission_policy()?
                    .dispatch_budget()
                    .as_millis()
            )?),
        ),
        ("ingest_max_records", json!(SCALE_INGEST_MAX_RECORDS)),
        ("ingest_max_text_bytes", json!(SCALE_INGEST_TEXT_BYTES)),
        ("ingest_max_vector_bytes", json!(SCALE_INGEST_VECTOR_BYTES)),
        (
            "coverage_max_encoded_bytes",
            json!(quanta_index_lexical::FILE_COVERAGE_ENCODED_BYTES_LIMIT),
        ),
        (
            "coverage_max_decode_heap_bytes",
            json!(quanta_index_lexical::FILE_COVERAGE_DECODE_HEAP_BYTES_LIMIT),
        ),
        (
            "source_publication_part_bytes",
            json!(quanta_index_contract::SOURCE_PUBLICATION_UPLOAD_PART_BYTES),
        ),
        (
            "source_publication_max_bytes",
            json!(quanta_index_contract::SOURCE_PUBLICATION_UPLOAD_MAX_BYTES),
        ),
        (
            "process_memory_ceiling_bytes",
            json!(SCALE_PROCESS_MEMORY_CEILING_BYTES),
        ),
    ] {
        if object.insert(name.into(), field).is_some() {
            anyhow::bail!("scale: capacity profile field {name} is already present");
        }
    }
    Ok(value)
}

pub(super) fn timeout_ms(client_timeout: Option<Duration>) -> AnyResult<u64> {
    let duration = client_timeout.unwrap_or(SCALE_CLIENT_IO_TIMEOUT);
    let millis = u64::try_from(duration.as_millis())?;
    if !(1..=600_000).contains(&millis) || Duration::from_millis(millis) != duration {
        anyhow::bail!("scale: client request timeout must be whole milliseconds in 1..=600000");
    }
    Ok(millis)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use anyhow::Context as _;

    use super::*;

    #[test]
    fn process_capacity_and_report_use_the_same_effective_values() -> AnyResult<()> {
        let policy = ScaleRuntimeConfig {
            history_max_bytes: Some(64 * 1024 * 1024),
            history_max_total_bytes: Some(256 * 1024 * 1024),
            ..ScaleRuntimeConfig::default()
        };
        let report = policy.execution_json()?;
        let environment: BTreeMap<_, _> = daemon_scale_capacity_environment()
            .into_iter()
            .chain(policy.daemon_history_environment()?)
            .collect();
        for (field, name) in [
            (
                "history_max_bytes",
                "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES",
            ),
            (
                "history_max_total_bytes",
                "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES",
            ),
            ("ingest_max_records", "QUANTA_INDEX_INGEST_MAX_RECORDS"),
            (
                "ingest_max_text_bytes",
                "QUANTA_INDEX_INGEST_MAX_TEXT_BYTES",
            ),
            (
                "ingest_max_vector_bytes",
                "QUANTA_INDEX_INGEST_MAX_VECTOR_BYTES",
            ),
            (
                "source_publication_max_bytes",
                "QUANTA_INDEX_SOURCE_PUBLICATION_MAX_BYTES",
            ),
            (
                "query_dispatch_budget_ms",
                "QUANTA_INDEX_QUERY_DISPATCH_BUDGET_MS",
            ),
            (
                "process_memory_ceiling_bytes",
                "QUANTA_INDEX_PROCESS_MEMORY_CEILING_BYTES",
            ),
        ] {
            let documented = report.get(field).and_then(Value::as_u64).context(field)?;
            let launched = environment.get(name).context(name)?.parse::<u64>()?;
            anyhow::ensure!(documented == launched, "{field} differs from {name}");
        }
        Ok(())
    }
}
