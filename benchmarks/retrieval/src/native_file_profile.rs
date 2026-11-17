//! Same-view file pools captured through the real Native read-session SDK.

use std::{
    collections::BTreeSet,
    path::PathBuf,
    time::{Duration, Instant},
};

use quanta_index_contract::{
    QueryConstraintSetV1, SemanticQueryRequest, TextQueryRequest, TextQuerySyntax,
    index::{IndexInstanceId, ViewRef, ViewSelection},
    native_protocol::{
        MAX_NATIVE_SOURCE_OWNER_LOOKUP, MAX_NATIVE_TEXT_TOP_K, NativeHybridFileResult,
        NativeTextHit,
    },
};
use quanta_index_retrieval_bench::{
    BenchError, BenchResult,
    canonical::canonical_json,
    query_plan::{NlPlanConfig, QueryInputPolicy},
    record::load_query_pack_with_file_digest,
    sha256_hex,
};
use quanta_index_sdk::native_uds::{NativeUdsClient, NativeUdsError};
use serde_json::{Value, json};

use super::{Args, optional_u64, reject_unknown, required, runner_binary_digest, write_json_bound};

fn sdk(error: NativeUdsError) -> BenchError {
    BenchError::Sdk(format!("{error:?}"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn fixed_hex<const N: usize>(raw: &str, name: &str) -> BenchResult<[u8; N]> {
    if raw.len() != N * 2
        || !raw
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(BenchError::Config(format!(
            "{name} must be {} lowercase hex digits",
            N * 2
        )));
    }
    let mut bytes = [0; N];
    for (slot, pair) in bytes.iter_mut().zip(raw.as_bytes().chunks_exact(2)) {
        let pair =
            std::str::from_utf8(pair).map_err(|error| BenchError::Config(error.to_string()))?;
        *slot =
            u8::from_str_radix(pair, 16).map_err(|error| BenchError::Config(error.to_string()))?;
    }
    Ok(bytes)
}

fn file_identity(hit: &NativeTextHit) -> Value {
    json!({
        "producer": hex(&hit.owner.producer.0),
        "owner": hex(&hit.owner.stable_id.0),
        "source": hex(&hit.source.0.0),
        "path": hit.path,
        "score_bits": format!("{:016x}", hit.score.get().to_bits()),
    })
}

fn pool_hash(lane: &str, rows: &Value) -> BenchResult<String> {
    let material = canonical_json(rows)?;
    let mut bytes = b"qidx.native.file.pool.v1\n".to_vec();
    bytes.extend_from_slice(lane.as_bytes());
    bytes.push(b'\n');
    bytes.extend_from_slice(material.as_bytes());
    Ok(sha256_hex(&bytes))
}

fn pool_hashes(result: &NativeHybridFileResult) -> BenchResult<Value> {
    let lexical = Value::Array(result.lexical_pool.iter().map(file_identity).collect());
    let dense = Value::Array(
        result
            .dense_pool
            .iter()
            .map(|hit| {
                json!({
                            "producer": hex(&hit.owner.producer.0),
                            "owner": hex(&hit.owner.stable_id.0),
                            "source": hex(&hit.source.0.0),
                            "path": hit.path,
                            "score_bits": format!("{:016x}", hit.score.get().to_bits()),
                            "projection_producer": hex(&hit.projection_owner.producer.0),
                            "projection_owner": hex(&hit.projection_owner.stable_id.0),
                            "vector_id": hex(&hit.vector_id),
                "chunk": hit.chunk,
                        })
            })
            .collect(),
    );
    Ok(json!({
        "scheme": "qidx.native.file.pool.v1",
        "lexical": pool_hash("lexical", &lexical)?,
        "dense": pool_hash("dense", &dense)?,
    }))
}

pub(super) fn run(args: &Args) -> BenchResult<()> {
    reject_unknown(
        args,
        &[
            "query-pack",
            "source-universe-sha256",
            "source-binding-sha256",
            "native-socket",
            "daemon-binary",
            "daemon-config",
            "decision-policy",
            "view-json",
            "open-key",
            "text-instance",
            "semantic-instance",
            "file-instance",
            "lane-top-k",
            "file-top-k",
            "io-timeout-secs",
            "out",
        ],
    )?;
    let (pack, pack_file_sha256) =
        load_query_pack_with_file_digest(&PathBuf::from(required(args, "query-pack")?))?;
    let source_binding = match (
        args.flags.get("source-universe-sha256"),
        args.flags.get("source-binding-sha256"),
    ) {
        (Some(universe), Some(binding)) => Some((
            hex(&fixed_hex::<32>(universe, "source-universe-sha256")?),
            hex(&fixed_hex::<32>(binding, "source-binding-sha256")?),
        )),
        (None, None) => None,
        _ => {
            return Err(BenchError::Config(
                "composite source digests must be supplied together".into(),
            ));
        }
    };
    let view: ViewRef = serde_json::from_str(&required(args, "view-json")?)
        .map_err(|error| BenchError::Config(format!("invalid view-json: {error}")))?;
    let text_instance = IndexInstanceId(fixed_hex(
        &required(args, "text-instance")?,
        "text-instance",
    )?);
    let semantic_instance = IndexInstanceId(fixed_hex(
        &required(args, "semantic-instance")?,
        "semantic-instance",
    )?);
    let file_instance = IndexInstanceId(fixed_hex(
        &required(args, "file-instance")?,
        "file-instance",
    )?);
    let open_key = fixed_hex(&required(args, "open-key")?, "open-key")?;
    if open_key == [0; 32]
        || text_instance == semantic_instance
        || text_instance == file_instance
        || semantic_instance == file_instance
    {
        return Err(BenchError::Config(
            "open-key must be nonzero and lane/file instances distinct".into(),
        ));
    }
    let lane_top_k = u32::try_from(optional_u64(args, "lane-top-k", 0)?)
        .map_err(|error| BenchError::Config(error.to_string()))?;
    let file_top_k = u32::try_from(optional_u64(args, "file-top-k", 0)?)
        .map_err(|error| BenchError::Config(error.to_string()))?;
    let seconds = optional_u64(args, "io-timeout-secs", 60)?;
    if !(1..=MAX_NATIVE_TEXT_TOP_K).contains(&lane_top_k)
        || !(10..=lane_top_k).contains(&file_top_k)
        || lane_top_k != pack.contract_top_k
        || seconds == 0
        || seconds > 60
    {
        return Err(BenchError::Config("Native file capture requires pack lane depth, 10<=file K<=lane K<=1024 and 1..=60s SDK deadline".into()));
    }
    let socket = PathBuf::from(required(args, "native-socket")?);
    if !socket.is_absolute() {
        return Err(BenchError::Config("native-socket must be absolute".into()));
    }
    let budget = || quanta_index_ipc::RequestBudgetV1::for_duration(Duration::from_secs(seconds));
    let runtime = super::native_file_runtime::RuntimeCapture::start(args, &socket, &budget())?;
    let client = NativeUdsClient::new(socket);
    let client = match runtime.as_ref() {
        Some(binding) => client.with_observed_query_peer_v1(binding.peer()),
        None => client,
    };
    let manifest = client
        .read_view(ViewSelection::Exact(view), &budget())
        .map_err(sdk)?;
    let mut session = client
        .open_read_session(ViewSelection::Exact(view), open_key, &budget())
        .map_err(sdk)?;
    let primary = (|| -> BenchResult<(Vec<Value>, Value, Value)> {
        let selected_profile = client
            .read_session_profile(&session, &budget())
            .map_err(sdk)?;
        if selected_profile.session != session.session || selected_profile.view != manifest {
            return Err(BenchError::Protocol(
                "Native retained profile session or CatalogView differs".into(),
            ));
        }
        for instance in [text_instance, semantic_instance, file_instance] {
            if selected_profile
                .profile
                .instances
                .iter()
                .filter(|spec| spec.instance == instance)
                .count()
                != 1
            {
                return Err(BenchError::Protocol(
                    "Native retained profile lacks unique lane/file instance".into(),
                ));
            }
        }
        // The retained session keeps the exact view readable across independent
        // path and owner requests. Each request still selects that view exactly.
        let mut paths = Vec::with_capacity(pack.file_universe.len());
        let mut after: Option<String> = None;
        loop {
            let page = client
                .query_paths(
                    ViewSelection::Exact(view),
                    file_instance,
                    "",
                    after.as_deref(),
                    128,
                    &budget(),
                )
                .map_err(sdk)?;
            if page.view != view || (page.has_more && page.entries.is_empty()) {
                return Err(BenchError::Protocol(
                    "Native path page view or continuation differs".into(),
                ));
            }
            for entry in page.entries {
                if after
                    .as_deref()
                    .is_some_and(|prior| entry.path.as_str() <= prior)
                {
                    return Err(BenchError::Protocol(
                        "Native indexed path page is unordered or repeated".into(),
                    ));
                }
                after = Some(entry.path.clone());
                paths.push(entry);
                if paths.len() > pack.file_universe.len() {
                    return Err(BenchError::Protocol(
                        "Native indexed path count exceeds frozen file universe".into(),
                    ));
                }
            }
            if !page.has_more {
                break;
            }
        }
        let expected_paths: BTreeSet<_> = pack
            .file_universe
            .iter()
            .map(|(path, _digest)| path.as_str())
            .collect();
        let actual_paths: BTreeSet<_> = paths.iter().map(|entry| entry.path.as_str()).collect();
        if paths.len() != pack.file_universe.len() || actual_paths != expected_paths {
            return Err(BenchError::Protocol(
                "Native indexed paths differ from frozen file universe".into(),
            ));
        }
        let mut owners: Vec<_> = paths.iter().map(|entry| entry.owner).collect();
        owners.sort_unstable();
        if owners.windows(2).any(|pair| pair[0] == pair[1])
            || owners
                .first()
                .is_some_and(|first| owners.iter().any(|owner| owner.producer != first.producer))
        {
            return Err(BenchError::Protocol(
                "Native frozen benchmark requires one producer and one path per owner".into(),
            ));
        }
        let mut owner_lookups = Vec::new();
        for chunk in owners.chunks(MAX_NATIVE_SOURCE_OWNER_LOOKUP) {
            let lookup = client
                .lookup_source_owners(ViewSelection::Exact(view), chunk.to_vec(), &budget())
                .map_err(sdk)?;
            if lookup.view != view || lookup.source != manifest.source {
                return Err(BenchError::Protocol(
                    "Native source-owner lookup view or SourceCut differs".into(),
                ));
            }
            owner_lookups.push(json!(lookup));
        }
        let indexed_universe = json!({
            "view": view,
            "source_cut": manifest.source,
            "file_instance": file_instance,
            "entries": paths,
            "owner_lookups": owner_lookups,
        });
        let mut tasks = Vec::with_capacity(pack.tasks.len());
        for task in &pack.tasks {
            session = client
                .renew_read_session(&session, &budget())
                .map_err(sdk)?;
            let plan = task
                .plan(
                    QueryInputPolicy::NaturalLanguageFile,
                    &NlPlanConfig::default(),
                )
                .map_err(|error| BenchError::Config(error.to_string()))?;
            let text = TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: plan.lexical_request.clone(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: None,
                generation_selector: None,
                top_k: lane_top_k,
                cursor: None,
            };
            let semantic = SemanticQueryRequest {
                query_text: plan.semantic_text.clone(),
                constraints: text.constraints.clone(),
                generation: None,
                generation_selector: None,
                lexical_scope: None,
                top_k: lane_top_k,
            };
            let start = Instant::now();
            let lexical = client
                .search_text_in_read_session(&session, text_instance, text.clone(), &budget())
                .map_err(sdk)?;
            let lexical_micros = start.elapsed().as_micros();
            let start = Instant::now();
            let hybrid = client
                .search_hybrid_file_in_read_session(
                    &session,
                    text_instance,
                    semantic_instance,
                    text.clone(),
                    semantic.clone(),
                    file_top_k,
                    &budget(),
                )
                .map_err(sdk)?;
            let hybrid_micros = start.elapsed().as_micros();
            if lexical.view != view
                || hybrid.view != view
                || lexical.artifact != hybrid.lexical_artifact
                || lexical.matched != hybrid.lexical_matched
                || lexical.hits != hybrid.lexical_pool
            {
                return Err(BenchError::Protocol(
                    "Native standalone lexical and Hybrid lexical pool differ".into(),
                ));
            }
            tasks.push(json!({
                "task_id": task.task_id,
                "query_identity": {
                    "original_query_sha256": plan.original_query_sha256,
                    "effective_lexical_request_sha256": plan.effective_lexical_request_sha256,
                    "semantic_text_sha256": plan.semantic_text_sha256,
                    "policy_config_sha256": plan.policy_config_sha256,
                },
                "text_request": text, "semantic_request": semantic,
                "lexical_micros": lexical_micros, "hybrid_micros": hybrid_micros,
                "pool_hashes": pool_hashes(&hybrid)?,
                "lexical": lexical, "hybrid": hybrid,
            }));
        }
        Ok((tasks, indexed_universe, json!(selected_profile)))
    })();
    // Cleanup has its own bounded control deadline and never extends a query.
    // Always release a successfully opened pin, including a failed capture.
    let released = client
        .release_read_session(&session, &budget())
        .map_err(sdk);
    let ((tasks, indexed_universe, selected_profile), released) = match (primary, released) {
        (Ok(capture), Ok((released, _))) => (capture, released),
        (Err(primary), Ok(_)) => return Err(primary),
        (Ok(_), Err(cleanup)) => return Err(cleanup),
        (Err(primary), Err(cleanup)) => {
            return Err(BenchError::Protocol(format!(
                "capture: {primary}; release: {cleanup}"
            )));
        }
    };
    let settlement = client
        .read_session_settlement(&released, &budget())
        .map_err(sdk)?;
    let capture = json!({
        "schema_version": 2, "profile": "native_file", "engine": "native",
        "runner_binary_sha256": runner_binary_digest()?,
        "query_pack_sha256": pack.pack_sha256,
        "query_pack_file_sha256": pack_file_sha256,
        "suite_id": pack.suite_id, "suite_commitment_sha256": pack.suite_commitment_sha256,
        "repository_commit": pack.repository_commit,
        "file_universe_digest": pack.file_universe_digest,
        "view": view, "selected_manifest": manifest,
        "selected_profile": selected_profile, "indexed_universe": indexed_universe,
        "text_instance": text_instance, "semantic_instance": semantic_instance,
        "lane_top_k": lane_top_k, "file_top_k": file_top_k,
        "query_deadline_seconds": seconds,
        "released_session": released, "settlement": settlement, "tasks": tasks,
    });
    let mut capture = capture;
    if let Some(binding) = runtime {
        capture["runtime_binding"] = binding.finish()?;
    }
    if let Some((universe, binding)) = source_binding {
        capture["source_universe_sha256"] = json!(universe);
        capture["source_binding_sha256"] = json!(binding);
    }
    let _digest = write_json_bound(&PathBuf::from(required(args, "out")?), &capture)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_hash_uses_independent_fixed_gold_and_binds_lane_order_source_and_score() {
        let row = json!({
            "producer": "00".repeat(16), "owner": "01".repeat(16),
            "source": "02".repeat(32), "path": "src/alpha.rs",
            "score_bits": "3ff0000000000000",
        });
        let rows = json!([row.clone()]);
        assert_eq!(
            pool_hash("lexical", &rows).expect("fixed lexical hash"),
            "34ab76cf9c57709c0a345e91ee50a61652636b8455edddbf0e26b48cacb68fe0",
        );
        assert_eq!(
            pool_hash("dense", &rows).expect("fixed dense hash"),
            "6f0ae5530df5b475b7a5763259f2d8f1cc77dc123805534b59a5a31b94ba30b2",
        );
        assert_ne!(
            pool_hash("lexical", &rows).expect("valid fixed pool"),
            pool_hash("dense", &rows).expect("valid fixed pool")
        );
        let mut changed = row.clone();
        changed["source"] = json!("03".repeat(32));
        assert_ne!(
            pool_hash("lexical", &rows).expect("valid fixed pool"),
            pool_hash("lexical", &json!([changed])).expect("valid fixed pool")
        );
        let mut changed_score = row.clone();
        changed_score["score_bits"] = json!("3fe0000000000000");
        assert_ne!(
            pool_hash("lexical", &rows).expect("fixed lexical hash"),
            pool_hash("lexical", &json!([changed_score])).expect("changed score hash"),
        );
        let mut other = row.clone();
        other["owner"] = json!("04".repeat(16));
        assert_ne!(
            pool_hash("lexical", &json!([row.clone(), other.clone()])).expect("valid fixed pool"),
            pool_hash("lexical", &json!([other, row])).expect("valid fixed pool")
        );
    }

    #[test]
    fn fixed_native_identity_refuses_malformed_hex_before_any_sdk_open() {
        assert!(fixed_hex::<16>("AB", "instance").is_err());
        assert!(fixed_hex::<16>(&"gg".repeat(16), "instance").is_err());
        assert!(fixed_hex::<16>(&"00".repeat(15), "instance").is_err());
        assert_eq!(
            fixed_hex::<16>(&"01".repeat(16), "instance").expect("valid fixed pool"),
            [1; 16]
        );
    }
}
