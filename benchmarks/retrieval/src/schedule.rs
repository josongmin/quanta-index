//! Shared, digest-bound cold/warm query schedule for qualified speed runs.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};
use serde_json::Value;

use crate::canonical::canonical_json;
use crate::{BenchError, BenchResult, sha256_hex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryProtocol {
    pub schema_version: u32,
    pub seed: u64,
    pub task_ids: Vec<String>,
    pub cold_probe_task_id: String,
    pub warmup_schedules: Vec<Vec<String>>,
    pub measurement_schedules: Vec<Vec<String>>,
    pub sha256: String,
}

struct ProtocolCore<'a> {
    schema_version: u32,
    seed: u64,
    task_ids: &'a [String],
    cold_probe_task_id: &'a str,
    warmup_schedules: &'a [Vec<String>],
    measurement_schedules: &'a [Vec<String>],
}

const QUERY_PROTOCOL_FIELDS: &[&str] = &[
    "schema_version",
    "seed",
    "task_ids",
    "cold_probe_task_id",
    "warmup_schedules",
    "measurement_schedules",
    "sha256",
];

impl Serialize for QueryProtocol {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state =
            serializer.serialize_struct("QueryProtocol", QUERY_PROTOCOL_FIELDS.len())?;
        state.serialize_field("schema_version", &self.schema_version)?;
        state.serialize_field("seed", &self.seed)?;
        state.serialize_field("task_ids", &self.task_ids)?;
        state.serialize_field("cold_probe_task_id", &self.cold_probe_task_id)?;
        state.serialize_field("warmup_schedules", &self.warmup_schedules)?;
        state.serialize_field("measurement_schedules", &self.measurement_schedules)?;
        state.serialize_field("sha256", &self.sha256)?;
        state.end()
    }
}

struct QueryProtocolVisitor;

impl<'de> Visitor<'de> for QueryProtocolVisitor {
    type Value = QueryProtocol;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a query protocol with exactly the declared fields")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut schema_version = None;
        let mut seed = None;
        let mut task_ids = None;
        let mut cold_probe_task_id = None;
        let mut warmup_schedules = None;
        let mut measurement_schedules = None;
        let mut sha256 = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "schema_version" => {
                    if schema_version.is_some() {
                        return Err(de::Error::duplicate_field("schema_version"));
                    }
                    schema_version = Some(map.next_value()?);
                }
                "seed" => {
                    if seed.is_some() {
                        return Err(de::Error::duplicate_field("seed"));
                    }
                    seed = Some(map.next_value()?);
                }
                "task_ids" => {
                    if task_ids.is_some() {
                        return Err(de::Error::duplicate_field("task_ids"));
                    }
                    task_ids = Some(map.next_value()?);
                }
                "cold_probe_task_id" => {
                    if cold_probe_task_id.is_some() {
                        return Err(de::Error::duplicate_field("cold_probe_task_id"));
                    }
                    cold_probe_task_id = Some(map.next_value()?);
                }
                "warmup_schedules" => {
                    if warmup_schedules.is_some() {
                        return Err(de::Error::duplicate_field("warmup_schedules"));
                    }
                    warmup_schedules = Some(map.next_value()?);
                }
                "measurement_schedules" => {
                    if measurement_schedules.is_some() {
                        return Err(de::Error::duplicate_field("measurement_schedules"));
                    }
                    measurement_schedules = Some(map.next_value()?);
                }
                "sha256" => {
                    if sha256.is_some() {
                        return Err(de::Error::duplicate_field("sha256"));
                    }
                    sha256 = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, QUERY_PROTOCOL_FIELDS)),
            }
        }
        Ok(QueryProtocol {
            schema_version: schema_version
                .ok_or_else(|| de::Error::missing_field("schema_version"))?,
            seed: seed.ok_or_else(|| de::Error::missing_field("seed"))?,
            task_ids: task_ids.ok_or_else(|| de::Error::missing_field("task_ids"))?,
            cold_probe_task_id: cold_probe_task_id
                .ok_or_else(|| de::Error::missing_field("cold_probe_task_id"))?,
            warmup_schedules: warmup_schedules
                .ok_or_else(|| de::Error::missing_field("warmup_schedules"))?,
            measurement_schedules: measurement_schedules
                .ok_or_else(|| de::Error::missing_field("measurement_schedules"))?,
            sha256: sha256.ok_or_else(|| de::Error::missing_field("sha256"))?,
        })
    }
}

impl<'de> Deserialize<'de> for QueryProtocol {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "QueryProtocol",
            QUERY_PROTOCOL_FIELDS,
            QueryProtocolVisitor,
        )
    }
}

impl Serialize for ProtocolCore<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ProtocolCore", 6)?;
        state.serialize_field("schema_version", &self.schema_version)?;
        state.serialize_field("seed", &self.seed)?;
        state.serialize_field("task_ids", self.task_ids)?;
        state.serialize_field("cold_probe_task_id", self.cold_probe_task_id)?;
        state.serialize_field("warmup_schedules", self.warmup_schedules)?;
        state.serialize_field("measurement_schedules", self.measurement_schedules)?;
        state.end()
    }
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

    #[test]
    fn decoding_rejects_unknown_duplicate_and_missing_fields() {
        let encoded = serde_json::to_string(&fixture()).expect("serialize protocol fixture");
        let duplicate = encoded.replacen("\"sha256\":", "\"sha256\":\"x\",\"sha256\":", 1);
        assert!(serde_json::from_str::<QueryProtocol>(&duplicate).is_err());

        let mut unknown: Value = serde_json::from_str(&encoded).expect("parse fixture");
        unknown
            .as_object_mut()
            .expect("protocol fixture is an object")
            .insert("unexpected".to_string(), Value::Bool(true));
        assert!(serde_json::from_value::<QueryProtocol>(unknown).is_err());

        let mut missing: Value = serde_json::from_str(&encoded).expect("parse fixture");
        drop(
            missing
                .as_object_mut()
                .expect("protocol fixture is an object")
                .remove("seed"),
        );
        assert!(serde_json::from_value::<QueryProtocol>(missing).is_err());
    }
}
