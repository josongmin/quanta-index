//! Local RCA only: independent exhaustive cosine versus the served index API.
//! No encoder, ranker, ANN policy, corpus-wide or executable-custody claim.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use quanta_index_contract::{
    BatchIngestMode, EmbeddingRecord, ExactRepoRelativePathV1, GenerationSnapshot,
    QueryConstraintSetV1, SearchPlaneTrackKind, SemanticCorpusKindV1, SemanticIngestBatch,
    lex::LanguageCode,
};
use quanta_index_core::{
    CoreError, MetricSourcePort, MetricValueV1, RequestBudgetV1, SemanticIndexOpenPort,
    SemanticIngestHeaderV1, SemanticPolicy, SemanticScopeStreamBuildPort, SemanticSearchHitV1,
    SemanticStreamWindowPolicy,
};
use serde::de::{DeserializeOwned, Error as _, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::{SemanticAdapter, run_blocking};

/// Maximum bytes read by the proof-only CLI before rejecting input.
pub const MAX_INPUT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 32 * 1024 * 1024;
const MAX_ROWS: usize = 16_384;
const MAX_QUERIES: usize = 16;
const MAX_K: u32 = 128;

fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::InvalidContract(message.into())
}

/// Unlike Value deserialization, this rejects duplicate keys at every depth.
struct StrictJson(Value);
impl<'de> Deserialize<'de> for StrictJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct JsonVisitor;
        impl<'de> Visitor<'de> for JsonVisitor {
            type Value = StrictJson;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("finite JSON without duplicate keys")
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<StrictJson, E> {
                Ok(StrictJson(Value::Bool(value)))
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<StrictJson, E> {
                Ok(StrictJson(Value::from(value)))
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<StrictJson, E> {
                Ok(StrictJson(Value::from(value)))
            }
            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<StrictJson, E> {
                serde_json::Number::from_f64(value)
                    .map(|number| StrictJson(Value::Number(number)))
                    .ok_or_else(|| E::custom("nonfinite JSON number"))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<StrictJson, E> {
                Ok(StrictJson(Value::from(value)))
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<StrictJson, E> {
                Ok(StrictJson(Value::from(value)))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<StrictJson, E> {
                Ok(StrictJson(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<StrictJson, A::Error> {
                let mut values = Vec::new();
                while let Some(StrictJson(value)) = seq.next_element()? {
                    values.push(value);
                }
                Ok(StrictJson(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<StrictJson, A::Error> {
                let mut values = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(A::Error::custom("duplicate JSON key"));
                    }
                    let StrictJson(value) = map.next_value()?;
                    let _old = values.insert(key, value);
                }
                Ok(StrictJson(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(JsonVisitor)
    }
}

struct Query {
    id: String,
    vector: Vec<f32>,
    k: u32,
    constraints: QueryConstraintSetV1,
    corpus: Option<SemanticCorpusKindV1>,
}
struct Plan {
    batch: SemanticIngestBatch,
    queries: Vec<Query>,
    raw: Value,
}

fn object(value: Value, fields: &[&str]) -> Result<Map<String, Value>, CoreError> {
    let Value::Object(map) = value else {
        return Err(invalid("expected an object"));
    };
    if map.len() != fields.len() || fields.iter().any(|key| !map.contains_key(*key)) {
        return Err(invalid("missing or unknown proof field"));
    }
    Ok(map)
}
fn take<T: DeserializeOwned>(map: &mut Map<String, Value>, key: &str) -> Result<T, CoreError> {
    serde_json::from_value(
        map.remove(key)
            .ok_or_else(|| invalid(format!("missing {key}")))?,
    )
    .map_err(|error| invalid(format!("{key}: {error}")))
}
struct BoundedSerializedSize(usize);
impl std::io::Write for BoundedSerializedSize {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .filter(|size| *size <= MAX_OUTPUT_BYTES)
            .ok_or_else(|| std::io::Error::other("typed batch exceeds output budget"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn parse(bytes: &[u8]) -> Result<Plan, CoreError> {
    if bytes.len() > usize::try_from(MAX_INPUT_BYTES).map_err(|error| invalid(error.to_string()))? {
        return Err(invalid("proof input exceeds 8 MiB"));
    }
    let StrictJson(raw) =
        serde_json::from_slice(bytes).map_err(|error| invalid(error.to_string()))?;
    let mut map = object(
        raw.clone(),
        &[
            "schema_version",
            "row_count",
            "query_count",
            "batch",
            "queries",
        ],
    )?;
    if take::<u32>(&mut map, "schema_version")? != 1 {
        return Err(invalid("wrong proof schema"));
    }
    let rows: usize = take(&mut map, "row_count")?;
    let count: usize = take(&mut map, "query_count")?;
    let batch: SemanticIngestBatch = take(&mut map, "batch")?;
    let mut queries = Vec::new();
    for value in take::<Vec<Value>>(&mut map, "queries")? {
        let mut query = object(
            value,
            &[
                "case_id",
                "vector",
                "k",
                "language_any_of",
                "repo_relative_path_exact",
                "corpus_kind",
            ],
        )?;
        let languages: Vec<LanguageCode> = take(&mut query, "language_any_of")?;
        if languages.windows(2).any(|pair| pair.first() >= pair.get(1)) {
            return Err(invalid("languages must be sorted and unique"));
        }
        queries.push(Query {
            id: take(&mut query, "case_id")?,
            vector: take(&mut query, "vector")?,
            k: take(&mut query, "k")?,
            corpus: take(&mut query, "corpus_kind")?,
            constraints: QueryConstraintSetV1 {
                language_any_of: languages.into_iter().collect(),
                repo_relative_path_exact: take::<Option<ExactRepoRelativePathV1>>(
                    &mut query,
                    "repo_relative_path_exact",
                )?,
            },
        });
    }
    validate_plan(&batch, &queries, rows, count, bytes.len())?;
    Ok(Plan {
        batch,
        queries,
        raw,
    })
}

fn validate_plan(
    batch: &SemanticIngestBatch,
    queries: &[Query],
    rows: usize,
    count: usize,
    input_bytes: usize,
) -> Result<(), CoreError> {
    let observed_rows = batch
        .replace_scopes
        .iter()
        .try_fold(0_usize, |sum, scope| {
            sum.checked_add(scope.embeddings.len())
                .ok_or_else(|| invalid("row count overflow"))
        })?;
    // f32 -> JSON may expand compact input numbers substantially. Count the
    // typed representation without allocating it or touching diagnostic state.
    let mut typed_size = BoundedSerializedSize(0);
    serde_json::to_writer(&mut typed_size, batch).map_err(|error| invalid(error.to_string()))?;
    let projected = typed_size
        .0
        .checked_mul(
            queries
                .len()
                .checked_add(2)
                .ok_or_else(|| invalid("output bound overflow"))?,
        )
        .and_then(|size| size.checked_add(input_bytes))
        // Each bounded query can repeat its expanded vector plus counters and
        // fixed metadata. Served payload is covered by the typed-batch factor.
        .and_then(|size| size.checked_add(MAX_QUERIES * (4096 * 32 + 65_536) + 65_536))
        .ok_or_else(|| invalid("output bound overflow"))?;
    if rows == 0
        || rows > MAX_ROWS
        || rows != observed_rows
        || count == 0
        || count > MAX_QUERIES
        || count != queries.len()
        || projected > MAX_OUTPUT_BYTES
    {
        return Err(invalid(
            "declared counts/output budget do not bind bounded inputs",
        ));
    }
    if !batch.seal
        || batch.mode != BatchIngestMode::ReplaceGeneration
        || batch.base_generation.is_some()
        || !batch.tombstone_scopes.is_empty()
        || !batch.clear_surfaces.is_empty()
        || batch.model_contract.dimension > 4096
    {
        return Err(invalid(
            "proof requires one fresh bounded sealed generation",
        ));
    }
    let dimension = SemanticIngestHeaderV1::of_batch(batch).dimension()?;
    let mut ids = BTreeSet::new();
    for record in batch
        .replace_scopes
        .iter()
        .flat_map(|scope| &scope.embeddings)
    {
        if !ids.insert(record.embedding_id.as_str()) {
            return Err(invalid("duplicate embedding id"));
        }
    }
    let mut case_ids = BTreeSet::new();
    for query in queries {
        if query.id.is_empty() || !case_ids.insert(&query.id) || query.k == 0 || query.k > MAX_K {
            return Err(invalid("duplicate/empty query id or unbounded k"));
        }
        SemanticPolicy::validate_fetch_size(query.k)?;
        SemanticPolicy::validate_embedding_vector_v1(
            &query.vector,
            dimension,
            batch.model_contract.normalization,
        )?;
    }
    crate::build::proof_validate_resident_batch_v1(batch, SemanticStreamWindowPolicy::DEFAULT)
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn eligible(record: &EmbeddingRecord, query: &Query) -> bool {
    query.corpus.is_none_or(|kind| record.corpus_kind == kind)
        && (query.constraints.language_any_of.is_empty()
            || query.constraints.language_any_of.contains(&record.language))
        && query
            .constraints
            .repo_relative_path_exact
            .as_ref()
            .is_none_or(|path| path.as_str() == record.repo_relative_path.as_str())
}
fn cosine(left: &[f32], right: &[f32]) -> Result<f64, CoreError> {
    if left.len() != right.len() || left.is_empty() {
        return Err(invalid("oracle dimension mismatch"));
    }
    let mut dot = 0.0;
    let mut left_norm = 0.0;
    let mut right_norm = 0.0;
    for (&a, &b) in left.iter().zip(right) {
        if !a.is_finite() || !b.is_finite() {
            return Err(invalid("oracle nonfinite vector"));
        }
        dot += f64::from(a) * f64::from(b);
        left_norm += f64::from(a) * f64::from(a);
        right_norm += f64::from(b) * f64::from(b);
    }
    let score = dot / (left_norm.sqrt() * right_norm.sqrt());
    if !score.is_finite() {
        return Err(invalid("oracle zero/nonfinite norm"));
    }
    Ok(score)
}
fn exact<'a>(
    records: &[&'a EmbeddingRecord],
    query: &Query,
) -> Result<Vec<(&'a EmbeddingRecord, f64)>, CoreError> {
    let mut scores = records
        .iter()
        .filter(|record| eligible(record, query))
        .map(|record| Ok((*record, cosine(&record.vector, &query.vector)?)))
        .collect::<Result<Vec<_>, CoreError>>()?;
    scores.sort_unstable_by(|(left, a), (right, b)| {
        b.total_cmp(a)
            .then_with(|| left.embedding_id.as_str().cmp(right.embedding_id.as_str()))
    });
    Ok(scores)
}
fn counters(adapter: &SemanticAdapter) -> Result<BTreeMap<String, u64>, CoreError> {
    Ok(adapter
        .scrape()?
        .into_iter()
        .filter_map(|point| match point.value {
            MetricValueV1::Counter(value) => Some((point.name, value)),
            MetricValueV1::Gauge(_) => None,
        })
        .collect())
}
fn external_state(state: &Path) -> Result<(), CoreError> {
    if !state.is_absolute() || state.file_name().is_none() {
        return Err(invalid("absolute external fresh state required"));
    }
    let parent = state
        .parent()
        .ok_or_else(|| invalid("missing state parent"))?
        .canonicalize()
        .map_err(|error| invalid(error.to_string()))?;
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| invalid("repository root missing"))?
        .canonicalize()
        .map_err(|error| invalid(error.to_string()))?;
    if parent.starts_with(repo) {
        return Err(invalid("proof state must be outside source repository"));
    }
    std::fs::create_dir(state).map_err(|error| invalid(format!("fresh state refused: {error}")))
}

fn verified_rows<'a>(
    records: &[&EmbeddingRecord],
    stored: &'a Value,
) -> Result<BTreeMap<&'a str, &'a Value>, CoreError> {
    let dimension = records
        .first()
        .ok_or_else(|| invalid("empty corpus"))?
        .vector
        .len();
    let schema = crate::layout::semantic_schema(
        i32::try_from(dimension).map_err(|error| invalid(error.to_string()))?,
    );
    let fields = schema
        .fields()
        .iter()
        .map(|field| field.name().clone())
        .collect::<BTreeSet<_>>();
    let rows = stored
        .get("semantic")
        .and_then(|table| table.get("rows"))
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("full corpus absent"))?;
    if rows.len() != records.len() {
        return Err(invalid("stored corpus is partial"));
    }
    let mut full_rows = BTreeMap::new();
    for row in rows {
        if row
            .as_object()
            .ok_or_else(|| invalid("stored row is not object"))?
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>()
            != fields
        {
            return Err(invalid("stored row is missing or adds logical columns"));
        }
        let id = row
            .get("embedding_id")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("stored row id absent"))?;
        let record = records
            .iter()
            .find(|record| record.embedding_id.as_str() == id)
            .ok_or_else(|| invalid("stored row not in input"))?;
        let expected = serde_json::to_value(record).map_err(|error| invalid(error.to_string()))?;
        for (key, value) in row
            .as_object()
            .ok_or_else(|| invalid("stored row is not object"))?
        {
            if expected.get(key) != Some(value) {
                return Err(invalid(format!("stored full row differs: {id}/{key}")));
            }
        }
        if full_rows.insert(id, row).is_some() {
            return Err(invalid("duplicate stored row"));
        }
    }
    Ok(full_rows)
}

fn counter_delta(
    before: &BTreeMap<String, u64>,
    after: &BTreeMap<String, u64>,
) -> Result<BTreeMap<String, u64>, CoreError> {
    if before.keys().ne(after.keys()) {
        return Err(invalid("dense counter set changed"));
    }
    before
        .iter()
        .filter(|(key, _)| key.starts_with("semantic_dense_"))
        .map(|(key, value)| {
            Ok((
                key.clone(),
                after
                    .get(key)
                    .ok_or_else(|| invalid("counter absent"))?
                    .checked_sub(*value)
                    .ok_or_else(|| invalid("counter regressed"))?,
            ))
        })
        .collect()
}

fn served_output(
    query: &Query,
    records: &[&EmbeddingRecord],
    full_rows: &BTreeMap<&str, &Value>,
    hits: &[SemanticSearchHitV1],
    batch: &SemanticIngestBatch,
) -> Result<Vec<Value>, CoreError> {
    if hits.len() > usize::try_from(query.k).map_err(|error| invalid(error.to_string()))? {
        return Err(invalid("served result count exceeds requested k"));
    }
    let mut seen = BTreeSet::new();
    let mut served = Vec::new();
    for hit in hits {
        let id = hit.candidate.candidate_id.as_str();
        let record = records
            .iter()
            .find(|record| record.embedding_id.as_str() == id)
            .ok_or_else(|| invalid("served id absent from corpus"))?;
        if !seen.insert(id)
            || !eligible(record, query)
            || hit.record_id != record.record_id.as_ref()
            || hit.owner_id != record.owner_id.as_ref()
            || hit.owner_kind != record.owner_kind
            || hit.corpus_kind != Some(record.corpus_kind)
            || hit.authority_digest != record.authority_digest.as_ref()
            || hit.candidate.repo_relative_path != record.repo_relative_path
            || hit.candidate.start_line != record.start_line
            || hit.candidate.end_line != record.end_line
            || hit.candidate.snippet != record.snippet.as_ref()
            || hit.candidate.repo_id != batch.repo_id
            || hit.candidate.revision_id != batch.revision_id
            || hit.candidate.manifest_generation != batch.generation
            || !hit.candidate.score.is_finite()
        {
            return Err(invalid(
                "served full candidate identity/payload/filters invalid",
            ));
        }
        let score = cosine(&record.vector, &query.vector)?;
        served.push(serde_json::json!({"candidate": hit.candidate, "record_id": hit.record_id,
            "owner_id": hit.owner_id, "owner_kind": hit.owner_kind, "corpus_kind": hit.corpus_kind,
            "authority_digest": hit.authority_digest, "score_f64_for_same_row": score,
            "absolute_score_error": (f64::from(hit.candidate.score) - score).abs(),
            "full_row_sha256": sha(&serde_json::to_vec(full_rows.get(id).ok_or_else(|| invalid("served full row absent"))?).map_err(|error| invalid(error.to_string()))?)}));
    }
    Ok(served)
}

/// Returns bounded raw JSON, not a pass flag. Corpus/query hashes are data
/// bindings only; source/executable custody must be provided by the caller.
pub fn exact_vs_served_v1(bytes: &[u8], state: &Path) -> Result<Vec<u8>, CoreError> {
    let plan = parse(bytes)?;
    external_state(state)?;
    let adapter = SemanticAdapter::with_state_root(state.to_path_buf())?;
    let header = SemanticIngestHeaderV1::of_batch(&plan.batch);
    let mut source = quanta_index_core::ResidentScopeSource::new(
        &plan.batch.replace_scopes,
        adapter.window_policy(),
    )?;
    let (tally, stages) = adapter.build_stream(&header, &mut source)?;
    let stored = run_blocking(
        &adapter.runtime,
        crate::search::proof_rows_v1(state, &plan.batch),
    )?;
    let records = plan
        .batch
        .replace_scopes
        .iter()
        .flat_map(|scope| &scope.embeddings)
        .collect::<Vec<_>>();
    let full_rows = verified_rows(&records, &stored)?;
    let candidate = GenerationSnapshot {
        repo_id: plan.batch.repo_id.clone(),
        revision_id: plan.batch.revision_id.clone(),
        track: SearchPlaneTrackKind::Semantic,
        manifest_generation: plan.batch.generation,
        manifest_digest: plan.batch.manifest_digest.clone(),
    };
    let searcher = adapter.open_proven(&candidate)?;
    let mut cases = Vec::new();
    for query in &plan.queries {
        let exact_scores = exact(&records, query)?;
        let k = usize::try_from(query.k).map_err(|error| invalid(error.to_string()))?;
        let expected = exact_scores.iter().take(k).map(|(record, score)| Ok(serde_json::json!({
            "embedding_id": record.embedding_id.as_str(), "record_id": record.record_id, "score_f64": score,
            "full_row_sha256": sha(&serde_json::to_vec(full_rows.get(record.embedding_id.as_str()).ok_or_else(|| invalid("oracle row absent"))?).map_err(|error| invalid(error.to_string()))?),
        }))).collect::<Result<Vec<_>, CoreError>>()?;
        let before = counters(&adapter)?;
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(60));
        let hits = match query.corpus {
            Some(kind) => searcher.search_hits_for_corpus_constrained(
                &query.vector,
                kind,
                &query.constraints,
                query.k,
                &budget,
            )?,
            None => searcher.search_hits_constrained(
                &query.vector,
                &query.constraints,
                query.k,
                &budget,
            )?,
        };
        let after = counters(&adapter)?;
        let delta = counter_delta(&before, &after)?;
        let served = served_output(query, &records, &full_rows, &hits, &plan.batch)?;
        let expected_ids = exact_scores
            .iter()
            .take(k)
            .map(|(record, _)| record.embedding_id.as_str())
            .collect::<Vec<_>>();
        let served_ids = hits
            .iter()
            .map(|hit| hit.candidate.candidate_id.as_str())
            .collect::<Vec<_>>();
        let overlap = served_ids
            .iter()
            .filter(|id| expected_ids.contains(id))
            .count();
        let boundary = exact_scores
            .iter()
            .take(k)
            .next_back()
            .map(|(_, score)| *score);
        let tie_aware = hits
            .iter()
            .filter(|hit| {
                exact_scores.iter().any(|(record, score)| {
                    record.embedding_id.as_str() == hit.candidate.candidate_id
                        && boundary.is_some_and(|cut| *score >= cut)
                })
            })
            .count();
        cases.push(serde_json::json!({"case_id": query.id, "vector": query.vector, "k": query.k,
            "constraints": query.constraints, "corpus_kind": query.corpus, "eligible_rows": exact_scores.len(),
            "exact": expected, "served": served, "exact_order_equal": expected_ids == served_ids,
            "top_k_overlap": overlap, "tie_aware_top_k_overlap": tie_aware, "exact_boundary_score_f64": boundary,
            "exact_result_count": expected_ids.len(), "served_result_count": served_ids.len(),
            "dense_counters_before": before, "dense_counters_after": after, "dense_execution_delta": delta}));
    }
    let output = serde_json::json!({"schema_version": 1, "claim_scope": "local exact-vs-served RCA only; not ANN qualification",
        "input_sha256": sha(bytes), "input": plan.raw, "stored_corpus": stored,
        "stored_corpus_sha256": sha(&serde_json::to_vec(&stored).map_err(|error| invalid(error.to_string()))?),
        "dense_lane_contract": searcher.dense_lane().trace_detail(), "build": {"tally": {"windows": tally.windows, "replace_scopes": tally.replace_scopes, "rows": tally.rows}, "stages": stages},
        "oracle": {"metric": "cosine over every eligible input f32 vector, accumulated in f64", "tie_order": "score_f64 descending, embedding_id ascending", "served_order": "unaltered production order", "score_errors": "raw f32 served score versus f64 same-row oracle; no pass tolerance"},
        "cases": cases});
    let bytes = serde_json::to_vec(&output).map_err(|error| invalid(error.to_string()))?;
    if bytes.len() > MAX_OUTPUT_BYTES {
        return Err(invalid("proof output exceeds 32 MiB"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Value {
        let record = crate::legacy_chunk_embedding_record_v1("a", "src/a.rs", vec![1.0, 0.0])
            .expect("record");
        let batch = crate::sealed_replace_batch_v1(
            quanta_index_contract::RepoId::new("repo").expect("repo"),
            quanta_index_contract::RevisionId::new("rev").expect("revision"),
            quanta_index_contract::ManifestGeneration::new(1),
            "src/a.rs",
            vec![record],
            2,
        );
        serde_json::json!({"schema_version":1,"row_count":1,"query_count":1,"batch":batch,"queries":[{
            "case_id":"a","vector":[1.0,0.0],"k":1,"language_any_of":[],"repo_relative_path_exact":null,"corpus_kind":null}]})
    }
    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "independent exact basis vectors have representable cosine zero/one"
    )]
    fn ann_proof_exact_oracle_pins_cosine_filters_and_ties() {
        assert_eq!(cosine(&[1.0, 0.0], &[1.0, 0.0]).expect("cosine"), 1.0);
        assert_eq!(cosine(&[1.0, 0.0], &[0.0, 1.0]).expect("cosine"), 0.0);
        assert!(cosine(&[0.0, 0.0], &[1.0, 0.0]).is_err());
        assert!(cosine(&[f32::NAN, 0.0], &[1.0, 0.0]).is_err());
        let plan = parse(&serde_json::to_vec(&fixture()).expect("JSON")).expect("plan");
        let first = plan
            .batch
            .replace_scopes
            .first()
            .expect("scope")
            .embeddings
            .first()
            .expect("row");
        let mut second = first.clone();
        second.embedding_id = quanta_index_contract::EmbeddingId::new("z");
        let ranked =
            exact(&[&second, first], plan.queries.first().expect("query")).expect("ranked");
        assert_eq!(ranked.first().expect("first").0.embedding_id.as_str(), "a");
        let mut plan = parse(&serde_json::to_vec(&fixture()).expect("JSON")).expect("plan");
        let query = plan.queries.first_mut().expect("query");
        query.constraints.repo_relative_path_exact =
            Some(ExactRepoRelativePathV1::new("src/missing.rs").expect("path"));
        assert!(!eligible(first, query));
        query.constraints.repo_relative_path_exact = None;
        let _inserted = query
            .constraints
            .language_any_of
            .insert(LanguageCode::new("python").expect("language"));
        assert!(!eligible(first, query));
        query.constraints.language_any_of.clear();
        query.corpus = Some(SemanticCorpusKindV1::ModuleCard);
        assert!(!eligible(first, query));
    }
    #[test]
    fn ann_proof_full_row_oracle_refuses_missing_and_forged_payload() {
        let plan = parse(&serde_json::to_vec(&fixture()).expect("JSON")).expect("plan");
        let record = plan
            .batch
            .replace_scopes
            .first()
            .expect("scope")
            .embeddings
            .first()
            .expect("record");
        let serialized = serde_json::to_value(record).expect("record JSON");
        let schema = crate::layout::semantic_schema(2);
        let row = Value::Object(
            schema
                .fields()
                .iter()
                .map(|field| {
                    (
                        field.name().clone(),
                        serialized.get(field.name()).expect("column").clone(),
                    )
                })
                .collect(),
        );
        let stored = serde_json::json!({"semantic":{"count":1,"rows":[row]}});
        assert!(verified_rows(&[record], &stored).is_ok());
        for field in ["snippet", "vector", "authority_digest"] {
            let mut changed = stored.clone();
            let row = changed
                .get_mut("semantic")
                .expect("semantic")
                .get_mut("rows")
                .expect("rows")
                .as_array_mut()
                .expect("array")
                .first_mut()
                .expect("row")
                .as_object_mut()
                .expect("object");
            assert!(row.remove(field).is_some());
            assert!(verified_rows(&[record], &changed).is_err());
        }
        let mut forged = stored;
        let row = forged
            .get_mut("semantic")
            .expect("semantic")
            .get_mut("rows")
            .expect("rows")
            .as_array_mut()
            .expect("array")
            .first_mut()
            .expect("row")
            .as_object_mut()
            .expect("object");
        let _previous = row.insert("snippet".into(), Value::from("forged snippet"));
        assert!(verified_rows(&[record], &forged).is_err());
    }
    #[test]
    fn ann_proof_state_is_external_and_exclusive() {
        let temp = tempfile::tempdir().expect("temp");
        let state = temp.path().join("fresh");
        assert!(external_state(Path::new("relative-state")).is_err());
        external_state(&state).expect("first exclusive directory");
        assert!(external_state(&state).is_err());
        assert!(
            external_state(&Path::new(env!("CARGO_MANIFEST_DIR")).join("no-proof-write")).is_err()
        );
    }
    #[test]
    #[expect(
        clippy::indexing_slicing,
        reason = "test mutates JSON fixture paths known by construction to probe one invalid field"
    )]
    fn ann_proof_refuses_bad_inputs_before_state_write() {
        let temp = tempfile::tempdir().expect("temp");
        let state = temp.path().join("fresh");
        for (field, value) in [
            ("row_count", serde_json::json!(true)),
            ("query_count", serde_json::json!(1.0)),
            ("row_count", serde_json::json!(2)),
            ("schema_version", serde_json::json!(2)),
        ] {
            let mut payload = fixture();
            payload[field] = value;
            assert!(
                exact_vs_served_v1(&serde_json::to_vec(&payload).expect("JSON"), &state).is_err()
            );
            assert!(!state.exists());
        }
        for value in [
            serde_json::json!([true, 0.0]),
            serde_json::json!([0.0, 0.0]),
            serde_json::json!([1.0]),
        ] {
            let mut payload = fixture();
            payload["queries"][0]["vector"] = value;
            assert!(parse(&serde_json::to_vec(&payload).expect("JSON")).is_err());
        }
        let encoded = serde_json::to_string(&fixture()).expect("JSON");
        assert!(
            parse(
                encoded
                    .replacen(
                        "\"schema_version\":1",
                        "\"schema_version\":1,\"schema_version\":1",
                        1
                    )
                    .as_bytes()
            )
            .is_err()
        );
        assert!(
            parse(&vec![
                b' ';
                usize::try_from(MAX_INPUT_BYTES)
                    .expect("bound")
                    .checked_add(1)
                    .expect("bound")
            ])
            .is_err()
        );
        let mut payload = fixture();
        let duplicate = payload["queries"][0].clone();
        payload["queries"]
            .as_array_mut()
            .expect("array")
            .push(duplicate);
        payload["query_count"] = serde_json::json!(2);
        assert!(parse(&serde_json::to_vec(&payload).expect("JSON")).is_err());
        let mut payload = fixture();
        let duplicate = payload["batch"]["replace_scopes"][0]["embeddings"][0].clone();
        payload["batch"]["replace_scopes"][0]["embeddings"]
            .as_array_mut()
            .expect("array")
            .push(duplicate);
        payload["row_count"] = serde_json::json!(2);
        assert!(parse(&serde_json::to_vec(&payload).expect("JSON")).is_err());
    }
}
