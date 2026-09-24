//! Shared, digest-bound cold/warm query schedule for qualified speed runs.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::canonical::canonical_json;
use crate::{BenchError, BenchResult, sha256_hex};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QueryProtocol {
    pub schema_version: u32,
    pub seed: u64,
    pub task_ids: Vec<String>,
    pub cold_probe_task_id: String,
    pub warmup_schedules: Vec<Vec<String>>,
    pub measurement_schedules: Vec<Vec<String>>,
    pub sha256: String,
}

#[derive(Serialize)]
struct ProtocolCore<'a> {
    schema_version: u32,
    seed: u64,
    task_ids: &'a [String],
    cold_probe_task_id: &'a str,
    warmup_schedules: &'a [Vec<String>],
    measurement_schedules: &'a [Vec<String>],
}

impl QueryProtocol {
    pub fn load(path: &Path, expected_task_ids: &[String]) -> BenchResult<Self> {
        let bytes = fs::read(path).map_err(|err| BenchError::Io {
            path: path.display().to_string(),
            message: err.to_string(),
        })?;
        let protocol: Self = serde_json::from_slice(&bytes).map_err(|err| BenchError::Json {
            path: path.display().to_string(),
            message: err.to_string(),
        })?;
        protocol.validate(expected_task_ids)?;
        Ok(protocol)
    }

    pub fn validate(&self, expected_task_ids: &[String]) -> BenchResult<()> {
        if self.schema_version != 1 {
            return Err(BenchError::Protocol(
                "query protocol schema version mismatch".into(),
            ));
        }
        if self.task_ids != expected_task_ids {
            return Err(BenchError::Protocol(
                "query protocol task order differs from query pack".into(),
            ));
        }
        let expected: BTreeSet<&str> = expected_task_ids.iter().map(String::as_str).collect();
        if expected.len() != expected_task_ids.len()
            || !expected.contains(self.cold_probe_task_id.as_str())
        {
            return Err(BenchError::Protocol(
                "query protocol task ids or cold probe are invalid".into(),
            ));
        }
        for (label, schedules) in [
            ("warmup", &self.warmup_schedules),
            ("measurement", &self.measurement_schedules),
        ] {
            if label == "measurement" && schedules.is_empty() {
                return Err(BenchError::Protocol(format!(
                    "query protocol {label} schedules are empty"
                )));
            }
            for schedule in schedules {
                let observed: BTreeSet<&str> = schedule.iter().map(String::as_str).collect();
                if schedule.len() != expected_task_ids.len() || observed != expected {
                    return Err(BenchError::Protocol(format!(
                        "query protocol {label} schedule is not an exact task permutation"
                    )));
                }
            }
        }
        let core = ProtocolCore {
            schema_version: self.schema_version,
            seed: self.seed,
            task_ids: &self.task_ids,
            cold_probe_task_id: &self.cold_probe_task_id,
            warmup_schedules: &self.warmup_schedules,
            measurement_schedules: &self.measurement_schedules,
        };
        let value: Value = serde_json::to_value(core).map_err(|err| {
            BenchError::Protocol(format!("query protocol cannot be canonicalized: {err}"))
        })?;
        let observed = sha256_hex(canonical_json(&value)?.as_bytes());
        if self.sha256.len() != 64 || self.sha256 != observed {
            return Err(BenchError::Protocol(
                "query protocol digest mismatch".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> QueryProtocol {
        let mut protocol = QueryProtocol {
            schema_version: 1,
            seed: 7,
            task_ids: vec!["a".into(), "b".into()],
            cold_probe_task_id: "b".into(),
            warmup_schedules: vec![vec!["b".into(), "a".into()]],
            measurement_schedules: vec![vec!["a".into(), "b".into()]],
            sha256: String::new(),
        };
        let core = ProtocolCore {
            schema_version: protocol.schema_version,
            seed: protocol.seed,
            task_ids: &protocol.task_ids,
            cold_probe_task_id: &protocol.cold_probe_task_id,
            warmup_schedules: &protocol.warmup_schedules,
            measurement_schedules: &protocol.measurement_schedules,
        };
        let value = serde_json::to_value(core).expect("serialize core");
        protocol.sha256 = sha256_hex(canonical_json(&value).expect("canonical").as_bytes());
        protocol
    }

    #[test]
    fn accepts_digest_bound_permutations() {
        fixture()
            .validate(&["a".into(), "b".into()])
            .expect("valid");
    }

    #[test]
    fn rejects_mutated_schedule() {
        let mut protocol = fixture();
        *protocol
            .measurement_schedules
            .get_mut(0)
            .and_then(|schedule| schedule.get_mut(1))
            .expect("fixture has a measurement schedule with two tasks") = "a".into();
        assert!(protocol.validate(&["a".into(), "b".into()]).is_err());
    }
}
