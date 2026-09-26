//! Opt-in, local proof exporter. It builds actual fresh and delta generations
//! and reads every stored semantic and membership row after seal validation.
use std::path::Path;

use quanta_index_contract::{BatchIngestMode, SemanticIngestBatch};
use quanta_index_core::{CoreError, SemanticIngestHeaderV1, SemanticScopeStreamBuildPort};
use serde::Deserialize;
use serde::de::{Error as _, MapAccess, Visitor};

use crate::{SemanticAdapter, run_blocking};

struct Plan {
    schema_version: u32,
    cases: Vec<Mutation>,
}

struct Mutation {
    case_id: String,
    fresh: SemanticIngestBatch,
    before: SemanticIngestBatch,
    delta: SemanticIngestBatch,
}

fn take_field<'de, M: MapAccess<'de>, T: Deserialize<'de>>(
    slot: &mut Option<T>,
    map: &mut M,
    name: &'static str,
) -> Result<(), M::Error> {
    if slot.is_some() {
        return Err(M::Error::duplicate_field(name));
    }
    *slot = Some(map.next_value()?);
    Ok(())
}

impl<'de> Deserialize<'de> for Plan {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PlanVisitor;
        impl<'de> Visitor<'de> for PlanVisitor {
            type Value = Plan;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a complete incremental proof plan")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Plan, M::Error> {
                let (mut schema_version, mut cases) = (None, None);
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "schema_version" => {
                            take_field(&mut schema_version, &mut map, "schema_version")?;
                        }
                        "cases" => take_field(&mut cases, &mut map, "cases")?,
                        _ => {
                            return Err(M::Error::unknown_field(
                                &key,
                                &["schema_version", "cases"],
                            ));
                        }
                    }
                }
                Ok(Plan {
                    schema_version: schema_version
                        .ok_or_else(|| M::Error::missing_field("schema_version"))?,
                    cases: cases.ok_or_else(|| M::Error::missing_field("cases"))?,
                })
            }
        }
        deserializer.deserialize_map(PlanVisitor)
    }
}

impl<'de> Deserialize<'de> for Mutation {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MutationVisitor;
        impl<'de> Visitor<'de> for MutationVisitor {
            type Value = Mutation;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("one complete fresh/before/delta mutation case")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Mutation, M::Error> {
                let (mut case_id, mut fresh, mut before, mut delta) = (None, None, None, None);
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "case_id" => take_field(&mut case_id, &mut map, "case_id")?,
                        "fresh" => take_field(&mut fresh, &mut map, "fresh")?,
                        "before" => take_field(&mut before, &mut map, "before")?,
                        "delta" => take_field(&mut delta, &mut map, "delta")?,
                        _ => {
                            return Err(M::Error::unknown_field(
                                &key,
                                &["case_id", "fresh", "before", "delta"],
                            ));
                        }
                    }
                }
                Ok(Mutation {
                    case_id: case_id.ok_or_else(|| M::Error::missing_field("case_id"))?,
                    fresh: fresh.ok_or_else(|| M::Error::missing_field("fresh"))?,
                    before: before.ok_or_else(|| M::Error::missing_field("before"))?,
                    delta: delta.ok_or_else(|| M::Error::missing_field("delta"))?,
                })
            }
        }
        deserializer.deserialize_map(MutationVisitor)
    }
}

type Owner = (String, String, String);

fn owner(record: &quanta_index_contract::EmbeddingRecord) -> Owner {
    (
        record.corpus_kind.as_code_str().to_string(),
        record.owner_kind.as_code_str().to_string(),
        record.owner_id.to_string(),
    )
}

fn records(batch: &SemanticIngestBatch) -> Vec<&quanta_index_contract::EmbeddingRecord> {
    batch
        .replace_scopes
        .iter()
        .flat_map(|scope| &scope.embeddings)
        .collect()
}

fn encoded_records(
    records: &[&quanta_index_contract::EmbeddingRecord],
) -> Result<Vec<String>, CoreError> {
    let mut rows = records
        .iter()
        .map(|record| {
            serde_json::to_string(record)
                .map_err(|error| CoreError::InvalidContract(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    rows.sort_unstable();
    Ok(rows)
}

fn memberships(
    batch: &SemanticIngestBatch,
    keep: impl Fn(&quanta_index_contract::EmbeddingRecord) -> bool,
) -> Result<Vec<String>, CoreError> {
    let mut values = Vec::new();
    for scope in &batch.replace_scopes {
        for members in &scope.cluster_memberships {
            let record = scope
                .embeddings
                .iter()
                .find(|record| record.record_id.as_ref() == members.cluster_record_id)
                .ok_or_else(|| {
                    CoreError::InvalidContract("proof membership lacks its record".to_string())
                })?;
            if keep(record) {
                values.push(
                    serde_json::to_string(members)
                        .map_err(|error| CoreError::InvalidContract(error.to_string()))?,
                );
            }
        }
    }
    values.sort_unstable();
    Ok(values)
}

fn validate_operation(case: &Mutation) -> Result<(), CoreError> {
    use quanta_index_contract::SearchScopeSurface;
    use std::collections::BTreeSet;
    let before = records(&case.before);
    let delta = records(&case.delta);
    let previous = before
        .iter()
        .map(|record| owner(record))
        .collect::<BTreeSet<_>>();
    let replaced = delta
        .iter()
        .map(|record| owner(record))
        .collect::<BTreeSet<_>>();
    let removed = case
        .delta
        .tombstone_scopes
        .iter()
        .map(|item| {
            let scope = &item.semantic_scope;
            (
                scope.corpus_kind.as_code_str().to_string(),
                scope.owner_kind.as_code_str().to_string(),
                scope.owner_id.clone(),
            )
        })
        .collect::<BTreeSet<_>>();
    let cleared = &case.delta.clear_surfaces;
    let retained = |record: &quanta_index_contract::EmbeddingRecord| {
        let key = owner(record);
        !replaced.contains(&key)
            && !removed.contains(&key)
            && !cleared.contains(&SearchScopeSurface::for_semantic_owner_v1(
                record.owner_kind,
                record.corpus_kind,
            ))
    };
    let unaffected = before
        .iter()
        .copied()
        .filter(|record| retained(record))
        .collect::<Vec<_>>();
    let mut expected = unaffected.clone();
    expected.extend(&delta);
    let mut expected_members = memberships(&case.before, retained)?;
    expected_members.extend(memberships(&case.delta, |_| true)?);
    expected_members.sort_unstable();
    let shared = replaced.iter().any(|key| previous.contains(key));
    let valid = match case.case_id.as_str() {
        "append" => {
            !replaced.is_empty()
                && !shared
                && removed.is_empty()
                && cleared.is_empty()
                && delta.iter().all(|new| {
                    before
                        .iter()
                        .all(|old| old.embedding_id != new.embedding_id)
                })
        }
        "replace" | "membership_replace" => {
            !replaced.is_empty() && shared && removed.is_empty() && cleared.is_empty()
        }
        "tombstone" => {
            replaced.is_empty()
                && cleared.is_empty()
                && removed.iter().any(|key| previous.contains(key))
        }
        "clear_surface" => {
            replaced.is_empty()
                && removed.is_empty()
                && before.iter().any(|record| {
                    cleared.contains(&SearchScopeSurface::for_semantic_owner_v1(
                        record.owner_kind,
                        record.corpus_kind,
                    ))
                })
        }
        _ => false,
    };
    let changed = if case.case_id == "membership_replace" {
        memberships(&case.before, |_| true)? != expected_members
    } else {
        encoded_records(&before)? != encoded_records(&expected)?
    };
    if !valid
        || !changed
        || unaffected.is_empty()
        || encoded_records(&records(&case.fresh))? != encoded_records(&expected)?
        || memberships(&case.fresh, |_| true)? != expected_members
    {
        return Err(CoreError::InvalidContract(
            "proof mutation is vacuous or violates independent operation/unaffected-owner oracle"
                .to_string(),
        ));
    }
    Ok(())
}

fn build(
    adapter: &SemanticAdapter,
    batch: &SemanticIngestBatch,
) -> Result<serde_json::Value, CoreError> {
    let header = SemanticIngestHeaderV1::of_batch(batch);
    let mut source = quanta_index_core::ResidentScopeSource::new(
        &batch.replace_scopes,
        adapter.window_policy(),
    )?;
    let (tally, stages) = adapter.build_stream(&header, &mut source)?;
    Ok(serde_json::json!({
        "generation": batch.generation,
        "batch_digest": batch.batch_digest,
        "manifest_digest": batch.manifest_digest,
        "windows": tally.windows,
        "replace_scopes": tally.replace_scopes,
        "rows": tally.rows,
        "stages": stages,
    }))
}

/// Run a plan in a new state directory. Existing state is refused; neither a
/// capped nearest-neighbor result nor a status summary is a row-set oracle.
pub fn incremental_proof_v1(
    plan_bytes: &[u8],
    state: &Path,
) -> Result<serde_json::Value, CoreError> {
    if plan_bytes.len() > 32 * 1024 * 1024 {
        return Err(CoreError::InvalidContract(
            "proof plan exceeds 32 MiB".to_string(),
        ));
    }
    let plan: Plan = serde_json::from_slice(plan_bytes)
        .map_err(|error| CoreError::InvalidContract(format!("incremental proof plan: {error}")))?;
    if plan.schema_version != 1 || plan.cases.is_empty() || plan.cases.len() > 128 {
        return Err(CoreError::InvalidContract(
            "invalid proof version/case count".to_string(),
        ));
    }
    std::fs::create_dir(state).map_err(|error| {
        CoreError::Storage(format!("proof needs a fresh state directory: {error}"))
    })?;
    let mut cases = Vec::new();
    let mut previous = None;
    for (index, case) in plan.cases.iter().enumerate() {
        if case.case_id.is_empty()
            || previous.is_some_and(|id: &str| id >= case.case_id.as_str())
            || !case.fresh.seal
            || !case.before.seal
            || !case.delta.seal
            || case.fresh.mode != BatchIngestMode::ReplaceGeneration
            || case.before.mode != BatchIngestMode::ReplaceGeneration
            || case.fresh.base_generation.is_some()
            || case.before.base_generation.is_some()
            || case.delta.mode != BatchIngestMode::Delta
            || case.delta.base_generation != Some(case.before.generation)
            || case.delta.generation == case.before.generation
            || case.fresh.repo_id != case.before.repo_id
            || case.fresh.repo_id != case.delta.repo_id
            || case.fresh.revision_id != case.before.revision_id
            || case.fresh.revision_id != case.delta.revision_id
            || case.fresh.model_contract != case.before.model_contract
            || case.fresh.model_contract != case.delta.model_contract
        {
            return Err(CoreError::InvalidContract(
                "proof case identities/modes are inconsistent".to_string(),
            ));
        }
        validate_operation(case)?;
        previous = Some(&case.case_id);
        let fresh = SemanticAdapter::with_state_root(state.join(format!("case-{index}-fresh")))?;
        let incremental =
            SemanticAdapter::with_state_root(state.join(format!("case-{index}-incremental")))?;
        let fresh_receipt = build(&fresh, &case.fresh)?;
        let before_receipt = build(&incremental, &case.before)?;
        let before_rows = run_blocking(
            &incremental.runtime,
            crate::search::proof_rows_v1(&incremental.state_root, &case.before),
        )?;
        let delta_receipt = build(&incremental, &case.delta)?;
        let fresh_rows = run_blocking(
            &fresh.runtime,
            crate::search::proof_rows_v1(&fresh.state_root, &case.fresh),
        )?;
        let incremental_rows = run_blocking(
            &incremental.runtime,
            crate::search::proof_rows_v1(&incremental.state_root, &case.delta),
        )?;
        cases.push(serde_json::json!({
            "case_id": case.case_id,
            "fresh": fresh_rows,
            "before": before_rows,
            "incremental": incremental_rows,
            "receipts": {"fresh": fresh_receipt, "before": before_receipt, "delta": delta_receipt},
        }));
    }
    Ok(serde_json::json!({"schema_version": 1, "cases": cases}))
}
