//! Selected-view replacement, deletion, path, and overlay quality checks.

use super::*;
use std::collections::BTreeSet;

use quanta_index_contract::{
    ExactRepoRelativePathV1,
    native_protocol::{NativeOverlayEffortContext, NativeOverlaySemanticResult},
};

pub(super) fn check_exact_path_query(
    client: &NativeUdsClient,
    selected: ViewRef,
    instance: IndexInstanceId,
    config: &AnnRailConfig,
    path: &str,
    expected: Option<(OwnerKey, &[f32])>,
    deadline: Instant,
) -> TestResult {
    let selected_view = client.read_view(
        ViewSelection::Exact(selected),
        &budget().capped_at(deadline),
    )?;
    let mut query = quality_query(0, config)?;
    query.top_k = 1;
    query.constraints =
        QueryConstraintSetV1::from_exact_repo_relative_path(ExactRepoRelativePathV1::new(path)?);
    let result = client.search_semantic(
        ViewSelection::Exact(selected),
        instance,
        query,
        &budget().capped_at(deadline),
    )?;
    require_equal(&result.view, &selected, "ANN exact-path selected view")?;
    require_equal(&result.instance, &instance, "ANN exact-path instance")?;
    require_equal(
        &result.coverage.status,
        &CoverageStatus::Complete,
        "ANN exact-path source coverage",
    )?;
    require_equal(
        &result.coverage.covered,
        &Some(result.coverage.desired),
        "ANN exact-path covered binding",
    )?;
    require_equal(
        &result.coverage.desired.root,
        &selected_view.source.owners,
        "ANN exact-path selected owner root",
    )?;
    require(
        result.scanned_owners <= u32::try_from(QUALITY_MAX_OWNERS)?,
        "ANN exact-path scan exceeded the original candidate cap",
    )?;
    match expected {
        Some((owner, vector)) => {
            let [hit] = result.hits.as_slice() else {
                return Err(
                    format!("ANN exact-path {path} did not return one eligible row").into(),
                );
            };
            require_equal(&hit.owner, &owner, "ANN exact-path owner")?;
            require_equal(
                &hit.vector_id.as_slice(),
                &b"whole-file".as_slice(),
                "ANN exact-path vector ID",
            )?;
            let direction = query_direction(0, config)?;
            let gold_score: f64 = vector
                .iter()
                .zip(&direction)
                .map(|(left, right)| f64::from(*left) * f64::from(*right))
                .sum();
            require(
                (hit.score.get() - gold_score).abs() <= EXACT_SCORE_TOLERANCE,
                format!("ANN exact-path {path} score differs from selected source"),
            )
        }
        None => require(
            result.hits.is_empty(),
            format!("ANN exact-path {path} returned a deleted or foreign owner"),
        ),
    }?;
    require(
        Instant::now() < deadline,
        "ANN exact-path quality watchdog expired after query",
    )
}

pub(super) struct QualityDelta {
    pub(super) source_view: ViewRef,
    pub(super) materialized_sources: [(OwnerKey, ObjectRef); 1],
    pub(super) sources: Vec<(OwnerKey, ObjectRef)>,
    pub(super) rows: Vec<(String, Vec<f32>)>,
}

pub(super) fn stage_quality_delta(
    client: &NativeUdsClient,
    selected: ViewRef,
    original_sources: &[(OwnerKey, ObjectRef)],
    original_rows: &[(String, Vec<f32>)],
    config: &AnnRailConfig,
    deadline: Instant,
) -> TestResult<QualityDelta> {
    // One changed source and one deleted owner exercise the incremental path.
    // The replacement reuses a fixture direction, so the provider and exact
    // oracle stay independent without adding another synthetic vector engine.
    let replaced = quality_owner(0, config)?;
    let deleted = quality_owner(122, config)?;
    let old_replaced = original_sources
        .iter()
        .find(|(owner, _source)| *owner == replaced)
        .map(|(_owner, source)| *source)
        .ok_or("ANN replacement prior source absent")?;
    let old_deleted = original_sources
        .iter()
        .find(|(owner, _source)| *owner == deleted)
        .map(|(_owner, source)| *source)
        .ok_or("ANN deletion prior source absent")?;
    let key = operation(3);
    let ingress = client.begin_input(key, &budget().capped_at(deadline))?;
    let staged = client.stage_sources(
        ingress,
        vec![SourceInput {
            path: "src/0000.rs".into(),
            bytes: b"row-122".to_vec(),
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
        }],
        &budget().capped_at(deadline),
    )?;
    let [replacement] = staged.as_slice() else {
        return Err("ANN replacement source staging cardinality differs".into());
    };
    require(
        *replacement != old_replaced,
        "ANN replacement did not change authenticated source",
    )?;
    let mutations = [
        OwnerMutation {
            owner: replaced,
            expected: Some(old_replaced),
            next: Some(*replacement),
        },
        OwnerMutation {
            owner: deleted,
            expected: Some(old_deleted),
            next: None,
        },
    ];
    let first = mutations.first().ok_or("ANN first mutation absent")?;
    let second = mutations.get(1).ok_or("ANN second mutation absent")?;
    require(
        first.owner < second.owner,
        "ANN delta mutation order differs",
    )?;
    let owners = client.stage_mutations(ingress, mutations, &budget().capped_at(deadline))?;
    let prior = client.read_view(
        ViewSelection::Exact(selected),
        &budget().capped_at(deadline),
    )?;
    let mut transition = SourceTransition {
        expected_source: prior.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: key.producer,
        stream: key.stream,
        stream_epoch: key.stream_epoch,
        predecessor: 1,
        sequence: 2,
        payload: transition.owner_digest()?,
    });
    let source_view = published(&client.submit(
        UpdateRequest {
            operation: key,
            expected_view: selected,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget().capped_at(deadline),
    )?)?;
    let mut sources = original_sources.to_vec();
    let (_owner, source) = sources
        .iter_mut()
        .find(|(owner, _source)| *owner == replaced)
        .ok_or("ANN replacement source gold absent")?;
    *source = *replacement;
    sources.retain(|(owner, _source)| *owner != deleted);
    require_equal(
        &sources.len(),
        &4095_usize,
        "ANN changed source owner count",
    )?;
    let mut rows = original_rows.to_vec();
    let (_id, vector) = rows
        .iter_mut()
        .find(|(id, _vector)| id == "row-0")
        .ok_or("ANN replacement row gold absent")?;
    *vector = unit_direction(122, config.width);
    rows.retain(|(id, _vector)| id != "row-122");
    require_equal(
        &rows.len(),
        &4095_usize,
        "ANN changed eligible vector count",
    )?;
    require(
        Instant::now() < deadline,
        "ANN delta watchdog expired after SourceAdvance",
    )?;
    Ok(QualityDelta {
        source_view,
        materialized_sources: [(replaced, *replacement)],
        sources,
        rows,
    })
}

pub(super) fn check_delta_provider_inputs(inputs: &[String]) -> TestResult {
    let corpus_inputs: Vec<&str> = inputs
        .iter()
        .filter_map(|input| input.starts_with("row-").then_some(input.as_str()))
        .collect();
    require_equal(
        &corpus_inputs,
        &vec!["row-122"],
        "ANN delta provider input text must contain one row-122",
    )
}

fn overlay_quality_query(
    client: &NativeUdsClient,
    selection: &QualitySelection<'_>,
    session: NativeOverlaySession,
    seed: u64,
    expected_corpus: Option<Digest>,
    path: Option<&str>,
    deadline: Instant,
) -> TestResult<NativeOverlaySemanticResult> {
    let selected = selection.view;
    let instance = selection.instance;
    let config = selection.config;
    let constraints = match path {
        Some(value) => QueryConstraintSetV1::from_exact_repo_relative_path(
            ExactRepoRelativePathV1::new(value)?,
        ),
        None => QueryConstraintSetV1::unconstrained(),
    };
    let top_k = if path.is_some() {
        1
    } else {
        u32::try_from(config.k)?
    };
    let mut semantic_query = quality_query(seed, config)?;
    semantic_query.constraints = constraints.clone();
    semantic_query.top_k = top_k;
    let mut fast_corpus = None;
    let mut deep = None;
    let done = client.query_effort_stream(
        NativeEffortStreamQuery {
            overlay: Some(NativeOverlayEffortContext {
                session,
                expected_corpus,
            }),
            selection: ViewSelection::Exact(selected),
            text_instance: text_spec().instance,
            semantic_instance: instance,
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "row-0".into(),
                constraints,
                generation: None,
                generation_selector: None,
                top_k,
                cursor: None,
            },
            semantic_query,
            preference: NativeEffortPreference::Deep,
        },
        &budget().capped_at(deadline),
        &mut |stage| {
            match stage {
                NativeEffortStreamStage::OverlayFast(fast) if fast_corpus.is_none() => {
                    fast_corpus = Some(fast.corpus.clone());
                }
                NativeEffortStreamStage::OverlayDeep(result)
                    if fast_corpus.is_some() && deep.is_none() =>
                {
                    deep = Some(result.clone());
                }
                NativeEffortStreamStage::Fast(_)
                | NativeEffortStreamStage::Deep(_)
                | NativeEffortStreamStage::OverlayFast(_)
                | NativeEffortStreamStage::OverlayDeep(_) => {
                    return Err(NativeUdsError::Protocol(
                        "ANN overlay stage order or origin differs".into(),
                    ));
                }
            }
            Ok(())
        },
    )?;
    let fast_corpus = fast_corpus.ok_or("ANN overlay Fast absent")?;
    let result = deep.ok_or("ANN overlay Deep absent")?;
    require(done.deep_delivered, "ANN overlay Deep was not delivered")?;
    require_equal(&done.view, &selected, "ANN overlay Done view")?;
    require_equal(
        &done.corpus,
        &Some(fast_corpus.clone()),
        "ANN overlay Done corpus",
    )?;
    require_equal(&result.corpus, &fast_corpus, "ANN overlay stage corpus")?;
    require_equal(
        &result.corpus.session,
        &session,
        "ANN overlay corpus session",
    )?;
    require(
        Instant::now() < deadline,
        "ANN overlay quality watchdog expired after streamed query",
    )?;
    Ok(result)
}

fn check_overlay_quality_result(
    result: &NativeOverlaySemanticResult,
    selection: &QualitySelection<'_>,
    seed: u64,
    overlay_owner: OwnerKey,
    deleted_owner: OwnerKey,
    revision: u64,
) -> TestResult<usize> {
    require_equal(&result.view, &selection.view, "ANN overlay selected view")?;
    require_equal(
        &result.instance,
        &selection.instance,
        "ANN overlay instance",
    )?;
    require_equal(
        &result.coverage.status,
        &CoverageStatus::Complete,
        "ANN overlay source coverage",
    )?;
    require_equal(
        &result.coverage.covered,
        &Some(result.coverage.desired),
        "ANN overlay covered input binding",
    )?;
    require_equal(
        &result.coverage.desired.root,
        &selection.source_owners,
        "ANN overlay selected source root",
    )?;
    require(
        matches!(
            result.traversal,
            NativeSemanticTraversal::Approximate { .. }
        ) && !result.exhaustive,
        "ANN overlay query did not use bounded approximate traversal",
    )?;
    require_equal(
        &result.hits.len(),
        &selection.config.k,
        "ANN overlay top-k page length",
    )?;
    require(
        result.scanned_owners <= u32::try_from(QUALITY_MAX_OWNERS)?,
        "ANN overlay scanned beyond fixed candidate bound",
    )?;
    require(
        result.corpus.revisions.iter().any(|row| {
            row.owner == overlay_owner && row.revision == revision && !row.deleted && !row.retired
        }),
        "ANN overlay corpus does not bind the unsaved revision",
    )?;
    require(
        result.corpus.revisions.iter().any(|row| {
            row.owner == deleted_owner && row.revision == revision && row.deleted && !row.retired
        }),
        "ANN overlay corpus does not bind the active tombstone",
    )?;
    let deleted_id = quality_row_id(deleted_owner, selection.config)?;
    require(
        selection.rows.iter().all(|(id, _vector)| id != &deleted_id),
        "ANN tombstoned owner remained eligible in exact oracle",
    )?;
    let direction = query_direction(seed, selection.config)?;
    let expected = exact_top_k(selection.rows, &direction, selection.config.k);
    require_equal(
        &expected.len(),
        &selection.config.k,
        "ANN overlay exact golden length",
    )?;
    let mut unique_owners = BTreeSet::new();
    let mut matches = 0_usize;
    for hit in &result.hits {
        require(
            hit.owner != deleted_owner,
            "ANN tombstoned durable owner leaked into overlay hits",
        )?;
        require_equal(
            &hit.vector_id.as_slice(),
            &b"whole-file".as_slice(),
            "ANN overlay whole-file vector ID",
        )?;
        let id = check_quality_hit(
            hit.owner,
            hit.score.get(),
            selection,
            &direction,
            &mut unique_owners,
        )?;
        if hit.owner == overlay_owner {
            require(
                hit.overlay
                    .is_some_and(|origin| origin.revision == revision)
                    && hit.chunk.is_none(),
                "ANN masked owner leaked its durable vector",
            )?;
        } else {
            require(hit.overlay.is_none(), "ANN unexpected unsaved origin")?;
        }
        if expected.contains(&id) {
            matches = matches.checked_add(1).ok_or("ANN overlay match overflow")?;
        }
    }
    Ok(matches)
}

pub(super) fn overlay_quality_sweep(
    client: &NativeUdsClient,
    selected: ViewRef,
    instance: IndexInstanceId,
    session: NativeOverlaySession,
    config: &AnnRailConfig,
    rows: &[(String, Vec<f32>)],
    owners: (OwnerKey, OwnerKey),
    deadline: Instant,
) -> TestResult {
    let (overlay_owner, deleted_owner) = owners;
    let view = client.read_view(
        ViewSelection::Exact(selected),
        &budget().capped_at(deadline),
    )?;
    let selection = QualitySelection {
        view: selected,
        source_owners: view.source.owners,
        instance,
        config,
        rows,
    };
    let mut matches = 0_usize;
    let mut corpus_identity = None;
    let mut publisher_sequence = 0_u64;
    let mut overlay_owner_seen = false;
    for seed in 0..config.queries {
        require(
            Instant::now() < deadline,
            format!("ANN overlay quality watchdog expired at seed {seed}"),
        )?;
        publisher_sequence = publisher_sequence
            .checked_add(1)
            .ok_or("ANN overlay publisher sequence overflow")?;
        client.renew_interaction_publisher(
            session.publisher,
            publisher_sequence,
            &budget().capped_at(deadline),
        )?;
        let result = overlay_quality_query(
            client,
            &selection,
            session,
            seed,
            corpus_identity,
            None,
            deadline,
        )?;
        if let Some(expected) = corpus_identity {
            require_equal(
                &result.corpus.identity,
                &expected,
                "ANN overlay corpus changed during quality sweep",
            )?;
        } else {
            corpus_identity = Some(result.corpus.identity);
        }
        overlay_owner_seen |= result.hits.iter().any(|hit| hit.owner == overlay_owner);
        matches = matches
            .checked_add(check_overlay_quality_result(
                &result,
                &selection,
                seed,
                overlay_owner,
                deleted_owner,
                1,
            )?)
            .ok_or("ANN overlay total recall overflow")?;
    }
    require(overlay_owner_seen, "ANN unsaved replacement never ranked")?;
    let corpus_identity = corpus_identity.ok_or("ANN overlay corpus identity absent")?;
    for (path, expected_owner) in [
        ("src/0000.rs", Some(overlay_owner)),
        ("src/0001.rs", None),
        ("src/0122.rs", None),
    ] {
        require(
            Instant::now() < deadline,
            format!("ANN overlay exact-path watchdog expired at {path}"),
        )?;
        publisher_sequence = publisher_sequence
            .checked_add(1)
            .ok_or("ANN overlay exact-path publisher sequence overflow")?;
        client.renew_interaction_publisher(
            session.publisher,
            publisher_sequence,
            &budget().capped_at(deadline),
        )?;
        let result = overlay_quality_query(
            client,
            &selection,
            session,
            0,
            Some(corpus_identity),
            Some(path),
            deadline,
        )?;
        require_equal(&result.view, &selected, "ANN overlay path selected view")?;
        require_equal(&result.instance, &instance, "ANN overlay path instance")?;
        require_equal(
            &result.coverage.status,
            &CoverageStatus::Complete,
            "ANN overlay path source coverage",
        )?;
        require_equal(
            &result.coverage.covered,
            &Some(result.coverage.desired),
            "ANN overlay path covered binding",
        )?;
        require_equal(
            &result.coverage.desired.root,
            &selection.source_owners,
            "ANN overlay path selected source root",
        )?;
        require_equal(
            &result.corpus.identity,
            &corpus_identity,
            "ANN overlay path same-session corpus",
        )?;
        require(
            result.scanned_owners <= u32::try_from(QUALITY_MAX_OWNERS)?,
            "ANN overlay path scanned beyond fixed candidate bound",
        )?;
        match expected_owner {
            Some(owner) => {
                let [hit] = result.hits.as_slice() else {
                    return Err("ANN overlay exact path did not select one unsaved owner".into());
                };
                require_equal(&hit.owner, &owner, "ANN overlay exact-path owner")?;
                require_equal(
                    &hit.vector_id.as_slice(),
                    &b"whole-file".as_slice(),
                    "ANN overlay exact-path vector ID",
                )?;
                require(
                    hit.overlay.is_some_and(|origin| origin.revision == 1) && hit.chunk.is_none(),
                    "ANN overlay exact path exposed durable owner or wrong revision",
                )?;
                let direction = query_direction(0, config)?;
                let vector = unit_direction(0, config.width);
                let gold_score: f64 = vector
                    .iter()
                    .zip(&direction)
                    .map(|(left, right)| f64::from(*left) * f64::from(*right))
                    .sum();
                require(
                    (hit.score.get() - gold_score).abs() <= EXACT_SCORE_TOLERANCE,
                    "ANN overlay exact-path score differs from unsaved source",
                )?;
            }
            None => require(
                result.hits.is_empty(),
                "ANN overlay deleted exact path returned a hit",
            )?,
        }
    }
    let possible = usize::try_from(config.queries)?
        .checked_mul(config.k)
        .ok_or("ANN overlay recall denominator overflow")?;
    let recall = f64::from(u32::try_from(matches)?) / f64::from(u32::try_from(possible)?);
    require(
        recall >= config.recall_floor,
        format!(
            "ANN overlay selected recall@{} {recall:.6} < {:.6} ({matches}/{possible})",
            config.k, config.recall_floor
        ),
    )?;
    require(
        Instant::now() < deadline,
        "ANN overlay quality watchdog expired after final path",
    )
}
