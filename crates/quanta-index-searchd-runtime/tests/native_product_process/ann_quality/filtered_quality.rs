//! Public Native lexical-scope ranking over a deterministic two-region ANN view.
//!
//! A lexical scope takes the authenticated finite exact path in `ann_query.rs`;
//! this checks filtered selected-set quality, not graph-level filtered recall.

use super::*;
use quanta_index_contract::{TextQueryRequest, TextQuerySyntax};
use std::collections::BTreeSet;

const FILTERED_WATCHDOG: Duration = Duration::from_secs(300);
const WIDE: &str = "eligiblewide";
const NARROW: &str = "eligiblenarrow";

pub(crate) fn source_text(seed: u64) -> String {
    if seed.is_multiple_of(16) {
        format!("row-{seed} {WIDE} {NARROW}")
    } else {
        format!("row-{seed} {WIDE}")
    }
}

fn eligible_rows(rows: &[(String, Vec<f32>)], narrow: bool) -> TestResult<Vec<(String, Vec<f32>)>> {
    rows.iter()
        .enumerate()
        .filter(|(seed, _row)| !narrow || seed.is_multiple_of(16))
        .map(|(seed, row)| {
            require_equal(
                &row.0,
                &format!("row-{seed}"),
                "fixed lexical source identity",
            )?;
            Ok(row.clone())
        })
        .collect()
}

fn scoped_query(seed: u64, token: &str) -> TestResult<SemanticQueryRequest> {
    let mut query = quality_query(seed, &FILTERED_CONFIG)?;
    query.lexical_scope = Some(TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: token.to_owned(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        top_k: u32::try_from(FILTERED_CONFIG.rows)?,
        cursor: None,
    });
    Ok(query)
}

fn verify_scope(
    client: &NativeUdsClient,
    selected: ViewRef,
    instance: IndexInstanceId,
    rows: &[(String, Vec<f32>)],
    sources: &[(OwnerKey, ObjectRef)],
    narrow: bool,
    deadline: Instant,
) -> TestResult {
    let token = if narrow { NARROW } else { WIDE };
    let eligible = eligible_rows(rows, narrow)?;
    require_equal(
        &eligible.len(),
        &if narrow { 12_usize } else { 192_usize },
        "fixed lexical membership count",
    )?;
    let scope = scoped_query(0, token)?
        .lexical_scope
        .ok_or("filtered scope missing")?;
    let lexical = client.search_text(
        ViewSelection::Exact(selected),
        text_spec().instance,
        scope,
        &budget().capped_at(deadline),
    )?;
    require_equal(&lexical.view, &selected, "selected lexical view")?;
    require_equal(
        &lexical.instance,
        &text_spec().instance,
        "selected lexical instance",
    )?;
    require(
        lexical.exhaustive,
        "selected lexical scope is not exhaustive",
    )?;
    require_equal(
        &usize::try_from(lexical.matched)?,
        &eligible.len(),
        "selected lexical matched count",
    )?;
    require_equal(
        &lexical.hits.len(),
        &eligible.len(),
        "selected lexical hit count",
    )?;
    let expected_owners = (0..FILTERED_CONFIG.rows)
        .filter(|seed| !narrow || seed.is_multiple_of(16))
        .map(|seed| quality_owner(seed, &FILTERED_CONFIG))
        .collect::<TestResult<BTreeSet<_>>>()?;
    let observed_owners: BTreeSet<_> = lexical.hits.iter().map(|hit| hit.owner).collect();
    require_equal(
        &observed_owners,
        &expected_owners,
        "selected lexical token owners",
    )?;
    for hit in &lexical.hits {
        let expected_source = sources
            .iter()
            .find(|(owner, _source)| *owner == hit.owner)
            .map(|(_owner, source)| source)
            .ok_or("lexical hit has no staged source")?;
        require_equal(&hit.source, expected_source, "selected lexical source")?;
    }
    let selected_view = client.read_view(
        ViewSelection::Exact(selected),
        &budget().capped_at(deadline),
    )?;
    let selection = QualitySelection {
        view: selected,
        source_owners: selected_view.source.owners,
        instance,
        config: &FILTERED_CONFIG,
        rows: &eligible,
    };
    if narrow {
        // Query 2 is near row 122 in the full corpus. Its global winner is
        // intentionally outside the twelve predeclared lexical members.
        let global_first = exact_top_k(rows, &query_direction(2, &FILTERED_CONFIG)?, 1);
        require_equal(
            &global_first.first().map(String::as_str),
            &Some("row-122"),
            "independent unfiltered top-1 for the narrow query",
        )?;
        require(
            !eligible.iter().any(|(id, _vector)| id == "row-122"),
            "narrow lexical source set admitted the global top-1",
        )?;
    }
    let mut total_overlap = 0_usize;
    for seed in 0..FILTERED_CONFIG.queries {
        require(
            Instant::now() < deadline,
            "filtered quality watchdog expired",
        )?;
        let result = client.search_semantic(
            ViewSelection::Exact(selected),
            instance,
            scoped_query(seed, token)?,
            &budget().capped_at(deadline),
        )?;
        require_equal(&result.view, &selected, "filtered selected view")?;
        require_equal(&result.instance, &instance, "filtered selected instance")?;
        require_equal(
            &result.coverage.status,
            &CoverageStatus::Complete,
            "filtered coverage",
        )?;
        require_equal(
            &result.coverage.covered,
            &Some(result.coverage.desired),
            "filtered covered binding",
        )?;
        require_equal(
            &result.coverage.desired.root,
            &selection.source_owners,
            "filtered selected source root",
        )?;
        require_equal(
            &result.traversal,
            &NativeSemanticTraversal::ExactExhausted,
            "finite lexical traversal",
        )?;
        require(result.exhaustive, "finite lexical scope is not exhaustive")?;
        require_equal(&result.skipped_stale, &0, "filtered stale owners")?;
        require_equal(
            &usize::try_from(result.scanned_owners)?,
            &eligible.len(),
            "filtered scanned owners",
        )?;
        require_equal(
            &result.hits.len(),
            &FILTERED_CONFIG.k,
            "filtered top-k after selection",
        )?;
        let direction = query_direction(seed, &FILTERED_CONFIG)?;
        let expected = exact_top_k(&eligible, &direction, FILTERED_CONFIG.k);
        require_equal(
            &expected.len(),
            &FILTERED_CONFIG.k,
            "independent filtered golden length",
        )?;
        let mut seen = BTreeSet::new();
        let mut actual = Vec::new();
        for hit in &result.hits {
            require_equal(
                &hit.vector_id.as_slice(),
                &b"whole-file".as_slice(),
                "filtered vector ID",
            )?;
            actual.push(check_quality_hit(
                hit.owner,
                hit.score.get(),
                &selection,
                &direction,
                &mut seen,
            )?);
        }
        total_overlap = total_overlap
            .checked_add(actual.iter().filter(|id| expected.contains(id)).count())
            .ok_or("filtered recall overlap overflow")?;
        require_equal(&actual, &expected, "filtered exact cosine top-k")?;
    }
    let possible = usize::try_from(FILTERED_CONFIG.queries)?
        .checked_mul(FILTERED_CONFIG.k)
        .ok_or("filtered recall denominator overflow")?;
    let recall = f64::from(u32::try_from(total_overlap)?) / f64::from(u32::try_from(possible)?);
    require(
        recall >= FILTERED_CONFIG.recall_floor,
        format!(
            "filtered recall@{} {recall} below {}",
            FILTERED_CONFIG.k, FILTERED_CONFIG.recall_floor
        ),
    )?;
    require(
        Instant::now() < deadline,
        "filtered quality watchdog expired after queries",
    )
}

#[test]
fn product_native_ann_selected_lexical_scopes_match_independent_filtered_gold() -> TestResult {
    let config = FILTERED_CONFIG;
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let root = temp.path().join("state");
    let socket = temp.path().join("native.sock");
    let config_path = temp.path().join("native-ann-filtered.json");
    let stub = EmbeddingStub::start_ann_filtered()?;
    let base_url = stub.base_url();
    let mut configuration =
        config_with_openai_semantic_dimension(&root, &socket, u32::try_from(config.width)?)?;
    set_fixture_json(
        &mut configuration,
        "/semantic/read/max_owners",
        serde_json::json!(QUALITY_MAX_OWNERS),
    )?;
    configure_semantic_endpoint(&mut configuration, &base_url)?;
    configure_owned_semantic(&mut configuration)?;
    fs::write(&config_path, serde_json::to_vec(&configuration)?)?;
    let semantic = openai_semantic_spec_dimension(&base_url, u32::try_from(config.width)?)?;
    let mut daemon = Daemon::start_with_api_key(&config_path, &socket, Some("test-key"))?;
    let client = NativeUdsClient::new(socket);
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("filtered ANN workspace creation failed".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![text_spec(), semantic.clone()],
        },
        &budget(),
    )?;
    let configured = published(&client.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)?;
    let deadline = Instant::now()
        .checked_add(FILTERED_WATCHDOG)
        .ok_or("filtered quality watchdog overflow")?;
    let staged = stage_quality_sources(&client, configured, &config, deadline, source_text)?;
    let selected = wait_for_complete(
        &client,
        &mut daemon,
        staged.view,
        work_status::CompletionProgress::new(&staged.sources, staged.view, &semantic)?,
        &config,
        Some(config.rows),
        deadline,
    )?;
    verify_selected_sources(&client, selected, &staged.sources, deadline)?;
    let rows = corpus(&config);
    // The unscoped query confirms this selected view uses a sealed Native ANN
    // directory. Lexical-scope queries below deliberately take its exact path.
    let unscoped = client.search_semantic(
        ViewSelection::Exact(selected),
        semantic.instance,
        quality_query(0, &config)?,
        &budget().capped_at(deadline),
    )?;
    require(
        matches!(
            unscoped.traversal,
            NativeSemanticTraversal::Approximate { .. }
        ) && unscoped.coverage.status == CoverageStatus::Complete,
        "filtered fixture did not reach sealed Native ANN coverage",
    )?;
    verify_scope(
        &client,
        selected,
        semantic.instance,
        &rows,
        &staged.sources,
        false,
        deadline,
    )?;
    verify_scope(
        &client,
        selected,
        semantic.instance,
        &rows,
        &staged.sources,
        true,
        deadline,
    )?;
    quality_stop("filtered selected daemon shutdown", daemon)?;
    stub.finish()
}
