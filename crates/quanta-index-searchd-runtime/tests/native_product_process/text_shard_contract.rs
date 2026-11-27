//! Public Native Text equivalence at an immutable segment boundary.

use super::*;
use quanta_index_contract::index::{IngressRef, MapRoot, OwnerMutationSet, ViewRef};
use quanta_index_contract::native_protocol::{
    NativeIndexRequestEnvelope, NativeIndexResponseEnvelope, NativeReadSessionStatus,
    NativeTextCursor,
};
use quanta_index_core::index::OwnerMutationDigest;
use std::{collections::BTreeMap, os::unix::process::ExitStatusExt as _};

const REPOSITORY: &str = "text-boundary-contract";
const BASE_OWNERS: usize = 130;
const LOW: usize = 127;
const MID: usize = 128;
const HIGH: usize = 129;
const ADDED: usize = 137;

fn text_instance() -> InstanceSpec {
    let mut spec = text_spec();
    spec.policy = TextRecipe::segmented_layered(Digest([31; 32])).output_policy();
    spec.format = StorageFormatId([77; 16]);
    spec
}

fn literal_instance() -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([7; 16]),
        kind: IndexKind([8; 16]),
        group: PublicationGroupId([9; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: InputDomain::SourceLocal,
        schema: SchemaId([10; 16]),
        policy: Digest([11; 32]),
        format: StorageFormatId([12; 16]),
    }
}

fn fixture_config(root: &Path, socket: &Path) -> TestResult<serde_json::Value> {
    with_layered_text(config(root, socket))
}

fn with_layered_text(mut configuration: serde_json::Value) -> TestResult<serde_json::Value> {
    configuration["text"]["layout"] = serde_json::json!("segmented-layered-v2");
    set_fixture_json(
        &mut configuration,
        "/text/descriptor/format",
        serde_json::to_value(text_instance().format)?,
    )?;
    Ok(configuration)
}

fn fixture_owner(index: usize) -> TestResult<OwnerKey> {
    let id = u16::try_from(index.checked_add(1).ok_or("owner ID overflow")?)?;
    let mut stable = [0; 16];
    stable[..2].copy_from_slice(&id.to_be_bytes());
    Ok(OwnerKey {
        producer: operation(2).producer,
        stable_id: StableOwnerId(stable),
    })
}

fn fixture_path(index: usize) -> String {
    format!("src/boundary{index:03}.rs")
}

fn fixture_body(index: usize) -> String {
    let marker = match index {
        LOW => "edgelowmarker phraseanchor lowtail",
        MID => "edgemidmarker phraseanchor midtail",
        HIGH => "edgehighmarker phraseanchor hightail",
        _ => "plainfixture",
    };
    format!("commonanchor {marker}")
}

fn fixture_source(path: String, body: String) -> SourceInput {
    SourceInput {
        path,
        bytes: body.into_bytes(),
        text: SourceTextAdmission::Utf8 {
            policy: Digest([31; 32]),
        },
        language: None,
    }
}

fn configure(client: &NativeUdsClient) -> TestResult<ViewRef> {
    configure_instances(client, vec![literal_instance(), text_instance()])
}

fn configure_instances(
    client: &NativeUdsClient,
    instances: Vec<InstanceSpec>,
) -> TestResult<ViewRef> {
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("text boundary workspace creation failed".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(ingress, IndexProfile { instances }, &budget())?;
    published(&client.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)
}

fn stage(
    client: &NativeUdsClient,
    ingress: IngressRef,
    sources: Vec<SourceInput>,
) -> TestResult<Vec<ObjectRef>> {
    let mut staged = Vec::with_capacity(sources.len());
    let mut remaining = sources.into_iter();
    loop {
        let inputs = remaining
            .by_ref()
            .take(quanta_index_contract::index::MAX_NATIVE_STAGE_SOURCES_INPUTS)
            .collect::<Vec<_>>();
        if inputs.is_empty() {
            break;
        }
        let expected = inputs.len();
        let NativeIndexResponse::StagedSources(references) = client.request(
            NativeIndexRequest::StageRepositorySources {
                ingress,
                repository: quanta_index_contract::RepoId::try_from(REPOSITORY)?,
                inputs,
            },
            &budget(),
        )?
        else {
            return Err("text boundary sources were not staged".into());
        };
        require_equal(
            &references.len(),
            &expected,
            "text boundary staged batch count",
        )?;
        staged.extend(references);
    }
    Ok(staged)
}

fn advance(
    client: &NativeUdsClient,
    ingress: IngressRef,
    prior: ViewRef,
    mutations: impl IntoIterator<Item = OwnerMutation>,
) -> TestResult<ViewRef> {
    let request = prepare_advance(client, ingress, prior, mutations)?;
    submit_and_settle(client, &request)
}

fn submit_advance(
    client: &NativeUdsClient,
    request: UpdateRequest,
    publish_budget: &RequestBudgetV1,
) -> TestResult<ViewRef> {
    let operation = request.operation;
    let submitted = client.submit(request, publish_budget);
    if matches!(
        &submitted,
        Ok(SubmitOutcome::Unknown(key)) | Err(NativeUdsError::Unknown(key)) if *key == operation
    ) {
        let status = client.status(operation, &budget());
        let head = client
            .read_view(
                ViewSelection::Latest {
                    workspace: operation.workspace,
                    epoch: operation.epoch,
                },
                &budget(),
            )
            .map(|view| view.identity);
        return Err(format!(
            "source advance ACK unknown for {operation:?}, journal={status:?}, head={head:?}"
        )
        .into());
    }
    published(&submitted?)
}

fn prepare_advance(
    client: &NativeUdsClient,
    ingress: IngressRef,
    prior: ViewRef,
    mutations: impl IntoIterator<Item = OwnerMutation>,
) -> TestResult<UpdateRequest> {
    let owners = client.stage_mutations(ingress, mutations, &budget())?;
    prepare_sealed_advance(client, ingress.operation, prior, owners)
}

fn prepare_sealed_advance(
    client: &NativeUdsClient,
    operation: OperationKey,
    prior: ViewRef,
    owners: OwnerMutationSet,
) -> TestResult<UpdateRequest> {
    let sequence = operation.sequence;
    let before = client.read_view(ViewSelection::Exact(prior), &budget())?;
    let event_sequence = sequence.checked_sub(1).ok_or("source event underflow")?;
    let mut transition = SourceTransition {
        expected_source: before.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation.producer,
        stream: operation.stream,
        stream_epoch: operation.stream_epoch,
        predecessor: event_sequence.checked_sub(1).unwrap_or(0),
        sequence: event_sequence,
        payload: transition.owner_digest()?,
    });
    Ok(UpdateRequest {
        operation,
        expected_view: prior,
        kind: UpdateKind::SourceAdvance(transition),
    })
}

fn observed_owners(
    client: &NativeUdsClient,
    view: ViewRef,
    query: &str,
) -> TestResult<Vec<(OwnerKey, String)>> {
    let mut owners = ranked_files(client, view, query, 256)?
        .into_iter()
        .map(|(owner, path, _score)| (owner, path))
        .collect::<Vec<_>>();
    owners.sort_unstable_by_key(|(owner, _)| owner.stable_id.0);
    Ok(owners)
}

fn text_query(query: &str, top_k: u32) -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: query.into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        top_k,
        cursor: None,
    }
}

fn ranked_files(
    client: &NativeUdsClient,
    view: ViewRef,
    query: &str,
    top_k: u32,
) -> TestResult<Vec<(OwnerKey, String, u64)>> {
    let result = client.search_text(
        ViewSelection::Exact(view),
        text_instance().instance,
        text_query(query, top_k),
        &budget(),
    )?;
    require_equal(&result.view, &view, "text boundary view")?;
    require(
        result.exhaustive,
        format!("text boundary query truncated: {query}"),
    )?;
    require_equal(
        &result.matched,
        &u64::try_from(result.hits.len())?,
        "text boundary full result count",
    )?;
    Ok(result
        .hits
        .into_iter()
        .map(|hit| (hit.owner, hit.path, hit.score.get().to_bits()))
        .collect())
}

fn observe(
    client: &NativeUdsClient,
    view: ViewRef,
) -> TestResult<Vec<(&'static str, Vec<(OwnerKey, String)>)>> {
    [
        "edgelowmarker",
        "edgemidmarker",
        "edgehighmarker",
        "edgemovedmarker",
        "addedmarker",
        "rewrittenlow",
        "\"edgelowmarker phraseanchor\"",
        "\"edgemovedmarker phraseanchor\"",
        "\"addedmarker phraseanchor\"",
        "commonanchor",
        "/edge[a-z]+marker/",
        "file.contains('dedmarker phras')",
        "file.contains('ighmarker phras')",
    ]
    .into_iter()
    .map(|query| Ok((query, observed_owners(client, view, query)?)))
    .collect()
}

#[derive(Clone, Copy)]
struct ExpectedFile {
    path: &'static str,
    body: &'static str,
}

const MULTI_QUERIES: [(&str, &str, bool); 9] = [
    ("commonanchor", "commonanchor", false),
    ("alphaedge", "alphaedge", false),
    ("betaedge", "betaedge", false),
    ("gammaedge", "gammaedge", false),
    ("deltaedge", "deltaedge", false),
    ("newalphaedge", "newalphaedge", false),
    ("rebornedge", "rebornedge", false),
    ("newdeltaedge", "newdeltaedge", false),
    ("file.contains('alphaedge')", "alphaedge", true),
];

fn assert_expected_files(
    client: &NativeUdsClient,
    view: ViewRef,
    files: &[Option<ExpectedFile>],
) -> TestResult {
    for (query, needle, substring) in MULTI_QUERIES {
        let mut actual = ranked_files(client, view, query, 32)?
            .into_iter()
            .map(|(owner, path, _score)| (owner, path))
            .collect::<Vec<_>>();
        actual.sort_unstable_by_key(|(owner, _path)| owner.stable_id.0);
        let expected = files
            .iter()
            .enumerate()
            .filter_map(|(index, file)| {
                let file = file.as_ref()?;
                let found = if substring {
                    file.body.contains(needle)
                } else {
                    file.body
                        .split_ascii_whitespace()
                        .any(|token| token == needle)
                };
                found.then_some((index, file.path))
            })
            .map(|(index, path)| Ok((fixture_owner(index)?, path.to_owned())))
            .collect::<TestResult<Vec<_>>>()?;
        require_equal(
            &actual,
            &expected,
            &format!("multigeneration Text gold: {query}"),
        )?;
    }
    Ok(())
}

fn publish_expected_full(
    client: &NativeUdsClient,
    files: &[Option<ExpectedFile>],
) -> TestResult<(ViewRef, Vec<ObjectRef>)> {
    let configured = configure(client)?;
    let ingress = client.begin_input(operation(2), &budget())?;
    let described = files
        .iter()
        .enumerate()
        .filter_map(|(index, file)| file.map(|file| (index, file)))
        .collect::<Vec<_>>();
    let sources = stage(
        client,
        ingress,
        described
            .iter()
            .map(|(_, file)| fixture_source(file.path.into(), file.body.into()))
            .collect(),
    )?;
    require_equal(
        &sources.len(),
        &described.len(),
        "independent FULL source count",
    )?;
    let view = advance(
        client,
        ingress,
        configured,
        described
            .iter()
            .zip(&sources)
            .map(|((index, _file), source)| {
                Ok(OwnerMutation {
                    owner: fixture_owner(*index)?,
                    expected: None,
                    next: Some(*source),
                })
            })
            .collect::<TestResult<Vec<_>>>()?,
    )?;
    Ok((view, sources))
}

#[test]
fn native_multigeneration_delta_matches_model_and_independent_full_after_reopen() -> TestResult {
    const BASE: [Option<ExpectedFile>; 4] = [
        Some(ExpectedFile {
            path: "src/a.rs",
            body: "commonanchor alphaedge",
        }),
        Some(ExpectedFile {
            path: "src/b.rs",
            body: "commonanchor betaedge",
        }),
        Some(ExpectedFile {
            path: "src/c.rs",
            body: "commonanchor gammaedge",
        }),
        Some(ExpectedFile {
            path: "src/d.rs",
            body: "commonanchor deltaedge",
        }),
    ];
    const FIRST: [Option<ExpectedFile>; 4] = [
        Some(ExpectedFile {
            path: "src/a.rs",
            body: "commonanchor newalphaedge",
        }),
        Some(ExpectedFile {
            path: "src/b-renamed.rs",
            body: "commonanchor betaedge",
        }),
        None,
        BASE[3],
    ];
    const FINAL: [Option<ExpectedFile>; 4] = [
        FIRST[0],
        None,
        Some(ExpectedFile {
            path: "src/c.rs",
            body: "commonanchor rebornedge",
        }),
        Some(ExpectedFile {
            path: "src/d-renamed.rs",
            body: "commonanchor newdeltaedge",
        }),
    ];
    let temp = quanta_index_searchd_harness::private_tempdir()?;
    let delta_root = temp.path().join("multi-delta-state");
    fs::create_dir(&delta_root)?;
    fs::set_permissions(&delta_root, fs::Permissions::from_mode(0o700))?;
    let delta_socket = temp.path().join("multi-delta.sock");
    let delta_config = temp.path().join("multi-delta-config.json");
    fs::write(
        &delta_config,
        serde_json::to_vec(&fixture_config(&delta_root, &delta_socket)?)?,
    )?;
    let daemon = Daemon::start(&delta_config, &delta_socket)?;
    let client = NativeUdsClient::new(delta_socket.clone());
    let (full, base) = publish_expected_full(&client, &BASE)?;
    let [base_a, base_b, base_c, base_d] = base.as_slice() else {
        return Err("multigeneration base owner count differs".into());
    };
    assert_expected_files(&client, full, &BASE)?;

    let ingress = client.begin_input(operation(3), &budget())?;
    let changed = stage(
        &client,
        ingress,
        FIRST
            .iter()
            .take(2)
            .flatten()
            .map(|file| fixture_source(file.path.into(), file.body.into()))
            .collect(),
    )?;
    let [a, b] = changed.as_slice() else {
        return Err("first DELTA staged source count differs".into());
    };
    let first = advance(
        &client,
        ingress,
        full,
        [
            OwnerMutation {
                owner: fixture_owner(0)?,
                expected: Some(*base_a),
                next: Some(*a),
            },
            OwnerMutation {
                owner: fixture_owner(1)?,
                expected: Some(*base_b),
                next: Some(*b),
            },
            OwnerMutation {
                owner: fixture_owner(2)?,
                expected: Some(*base_c),
                next: None,
            },
            OwnerMutation {
                owner: fixture_owner(3)?,
                expected: Some(*base_d),
                next: Some(*base_d),
            },
        ],
    )?;
    require(full != first, "first DELTA did not advance the view")?;
    assert_expected_files(&client, first, &FIRST)?;
    assert_expected_files(&client, full, &BASE)?;

    let ingress = client.begin_input(operation(4), &budget())?;
    let changed = stage(
        &client,
        ingress,
        FINAL
            .iter()
            .skip(2)
            .flatten()
            .map(|file| fixture_source(file.path.into(), file.body.into()))
            .collect(),
    )?;
    let [c, d] = changed.as_slice() else {
        return Err("second DELTA staged source count differs".into());
    };
    let second = advance(
        &client,
        ingress,
        first,
        [
            OwnerMutation {
                owner: fixture_owner(0)?,
                expected: Some(*a),
                next: Some(*a),
            },
            OwnerMutation {
                owner: fixture_owner(1)?,
                expected: Some(*b),
                next: None,
            },
            OwnerMutation {
                owner: fixture_owner(2)?,
                expected: None,
                next: Some(*c),
            },
            OwnerMutation {
                owner: fixture_owner(3)?,
                expected: Some(*base_d),
                next: Some(*d),
            },
        ],
    )?;
    require(first != second, "second DELTA did not advance the view")?;
    assert_expected_files(&client, second, &FINAL)?;
    assert_expected_files(&client, first, &FIRST)?;
    daemon.stop()?;

    let reopened = Daemon::start(&delta_config, &delta_socket)?;
    let cold = NativeUdsClient::new(delta_socket);
    assert_expected_files(&cold, second, &FINAL)?;
    assert_expected_files(&cold, full, &BASE)?;
    let delta_ranked = MULTI_QUERIES
        .iter()
        .map(|(query, _, _)| ranked_files(&cold, second, query, 32))
        .collect::<TestResult<Vec<_>>>()?;
    reopened.stop()?;

    let full_root = temp.path().join("multi-full-state");
    fs::create_dir(&full_root)?;
    fs::set_permissions(&full_root, fs::Permissions::from_mode(0o700))?;
    let full_socket = temp.path().join("multi-full.sock");
    let full_config = temp.path().join("multi-full-config.json");
    fs::write(
        &full_config,
        serde_json::to_vec(&fixture_config(&full_root, &full_socket)?)?,
    )?;
    let full_daemon = Daemon::start(&full_config, &full_socket)?;
    let full_client = NativeUdsClient::new(full_socket);
    let (rebuilt, _sources) = publish_expected_full(&full_client, &FINAL)?;
    for ((query, _, _), delta) in MULTI_QUERIES.into_iter().zip(delta_ranked) {
        require_equal(
            &delta,
            &ranked_files(&full_client, rebuilt, query, 32)?,
            &format!("multigeneration DELTA versus independent FULL rank and score: {query}"),
        )?;
    }
    full_daemon.stop()
}

#[test]
fn native_boundary_delta_matches_independent_full_and_cold_reopen() -> TestResult {
    // wire::MAX_DOCS is 128. The 130-owner FULL and final corpus require
    // at least two physical document partitions in segmented-layered-v2.
    require(BASE_OWNERS > 128, "boundary fixture fits one Text segment")?;
    let temp = quanta_index_searchd_harness::private_tempdir()?;
    let delta_root = temp.path().join("delta-state");
    fs::create_dir(&delta_root)?;
    fs::set_permissions(&delta_root, fs::Permissions::from_mode(0o700))?;
    let delta_socket = temp.path().join("delta.sock");
    let delta_config = temp.path().join("delta-config.json");
    fs::write(
        &delta_config,
        serde_json::to_vec(&fixture_config(&delta_root, &delta_socket)?)?,
    )?;
    let daemon = Daemon::start(&delta_config, &delta_socket)?;
    let client = NativeUdsClient::new(delta_socket.clone());
    let configured = configure(&client)?;
    let ingress = client.begin_input(operation(2), &budget())?;
    let base_sources = (0..BASE_OWNERS)
        .map(|index| fixture_source(fixture_path(index), fixture_body(index)))
        .collect();
    let base = stage(&client, ingress, base_sources)?;
    require_equal(&base.len(), &BASE_OWNERS, "text boundary base source count")?;
    let full = advance(
        &client,
        ingress,
        configured,
        base.iter()
            .enumerate()
            .map(|(index, source)| {
                Ok(OwnerMutation {
                    owner: fixture_owner(index)?,
                    expected: None,
                    next: Some(*source),
                })
            })
            .collect::<TestResult<Vec<_>>>()?,
    )?;
    require_equal(
        &observed_owners(&client, full, "edgehighmarker")?,
        &vec![(fixture_owner(HIGH)?, fixture_path(HIGH))],
        "base high edge",
    )?;
    require_equal(
        &observed_owners(&client, full, "file.contains('ighmarker phras')")?,
        &vec![(fixture_owner(HIGH)?, fixture_path(HIGH))],
        "base high edge raw substring",
    )?;
    require_equal(
        &observed_owners(&client, full, "/edge[a-z]+marker/")?,
        &vec![
            (fixture_owner(LOW)?, fixture_path(LOW)),
            (fixture_owner(MID)?, fixture_path(MID)),
            (fixture_owner(HIGH)?, fixture_path(HIGH)),
        ],
        "base regex across segment boundaries",
    )?;

    let ingress = client.begin_input(operation(3), &budget())?;
    let changed = stage(
        &client,
        ingress,
        vec![
            fixture_source(
                fixture_path(LOW),
                "commonanchor edgelowmarker phraseanchor rewrittenlow".into(),
            ),
            fixture_source(
                "src/moved-mid.rs".into(),
                "commonanchor edgemovedmarker phraseanchor midtail".into(),
            ),
            fixture_source(
                fixture_path(ADDED),
                "commonanchor addedmarker phraseanchor addedtail".into(),
            ),
        ],
    )?;
    let [low, mid, added] = changed.as_slice() else {
        return Err("text boundary changed source count differs".into());
    };
    let delta = advance(
        &client,
        ingress,
        full,
        [
            OwnerMutation {
                owner: fixture_owner(LOW)?,
                expected: Some(base[LOW]),
                next: Some(*low),
            },
            OwnerMutation {
                owner: fixture_owner(MID)?,
                expected: Some(base[MID]),
                next: Some(*mid),
            },
            OwnerMutation {
                owner: fixture_owner(HIGH)?,
                expected: Some(base[HIGH]),
                next: None,
            },
            OwnerMutation {
                owner: fixture_owner(ADDED)?,
                expected: None,
                next: Some(*added),
            },
        ],
    )?;
    let incremental = observe(&client, delta)?;
    require_equal(
        &incremental[0].1,
        &vec![(fixture_owner(LOW)?, fixture_path(LOW))],
        "low edge",
    )?;
    require(
        incremental[1].1.is_empty() && incremental[2].1.is_empty(),
        "removed edge leaked",
    )?;
    require_equal(
        &incremental[3].1,
        &vec![(fixture_owner(MID)?, "src/moved-mid.rs".into())],
        "moved edge",
    )?;
    require_equal(
        &incremental[4].1,
        &vec![(fixture_owner(ADDED)?, fixture_path(ADDED))],
        "added edge",
    )?;
    require_equal(
        &incremental[5].1,
        &vec![(fixture_owner(LOW)?, fixture_path(LOW))],
        "rewritten edge",
    )?;
    require_equal(
        &incremental[6].1,
        &vec![(fixture_owner(LOW)?, fixture_path(LOW))],
        "low edge phrase",
    )?;
    require_equal(
        &incremental[7].1,
        &vec![(fixture_owner(MID)?, "src/moved-mid.rs".into())],
        "moved edge phrase",
    )?;
    require_equal(
        &incremental[8].1,
        &vec![(fixture_owner(ADDED)?, fixture_path(ADDED))],
        "added edge phrase",
    )?;
    let expected_live = (0..BASE_OWNERS)
        .filter(|index| *index != HIGH)
        .chain(std::iter::once(ADDED))
        .map(|index| {
            Ok((
                fixture_owner(index)?,
                if index == MID {
                    "src/moved-mid.rs".into()
                } else {
                    fixture_path(index)
                },
            ))
        })
        .collect::<TestResult<Vec<_>>>()?;
    require_equal(&incremental[9].1, &expected_live, "all live owners")?;
    require_equal(
        &incremental[10].1,
        &vec![
            (fixture_owner(LOW)?, fixture_path(LOW)),
            (fixture_owner(MID)?, "src/moved-mid.rs".into()),
        ],
        "regex across surviving segment boundaries",
    )?;
    require_equal(
        &incremental[11].1,
        &vec![(fixture_owner(ADDED)?, fixture_path(ADDED))],
        "added edge raw substring",
    )?;
    require(
        incremental[12].1.is_empty(),
        "removed edge raw substring leaked",
    )?;
    daemon.stop()?;
    let reopened = Daemon::start(&delta_config, &delta_socket)?;
    let cold = NativeUdsClient::new(delta_socket);
    require_equal(&observe(&cold, delta)?, &incremental, "cold delta results")?;
    reopened.stop()?;

    let rebuild_root = temp.path().join("rebuild-state");
    fs::create_dir(&rebuild_root)?;
    fs::set_permissions(&rebuild_root, fs::Permissions::from_mode(0o700))?;
    let rebuild_socket = temp.path().join("rebuild.sock");
    let rebuild_config = temp.path().join("rebuild-config.json");
    fs::write(
        &rebuild_config,
        serde_json::to_vec(&fixture_config(&rebuild_root, &rebuild_socket)?)?,
    )?;
    let rebuilt_daemon = Daemon::start(&rebuild_config, &rebuild_socket)?;
    let rebuild_client = NativeUdsClient::new(rebuild_socket);
    let configured = configure(&rebuild_client)?;
    let ingress = rebuild_client.begin_input(operation(2), &budget())?;
    let final_sources = (0..BASE_OWNERS)
        .filter(|index| *index != HIGH)
        .chain(std::iter::once(ADDED))
        .map(|index| match index {
            LOW => fixture_source(
                fixture_path(index),
                "commonanchor edgelowmarker phraseanchor rewrittenlow".into(),
            ),
            MID => fixture_source(
                "src/moved-mid.rs".into(),
                "commonanchor edgemovedmarker phraseanchor midtail".into(),
            ),
            ADDED => fixture_source(
                fixture_path(index),
                "commonanchor addedmarker phraseanchor addedtail".into(),
            ),
            _ => fixture_source(fixture_path(index), fixture_body(index)),
        })
        .collect();
    let final_refs = stage(&rebuild_client, ingress, final_sources)?;
    require_equal(
        &final_refs.len(),
        &BASE_OWNERS,
        "independent FULL source count",
    )?;
    let final_owners = (0..BASE_OWNERS)
        .filter(|index| *index != HIGH)
        .chain(std::iter::once(ADDED));
    let rebuilt = advance(
        &rebuild_client,
        ingress,
        configured,
        final_owners
            .zip(final_refs)
            .map(|(index, source)| {
                Ok(OwnerMutation {
                    owner: fixture_owner(index)?,
                    expected: None,
                    next: Some(source),
                })
            })
            .collect::<TestResult<Vec<_>>>()?,
    )?;
    require_equal(
        &observe(&rebuild_client, rebuilt)?,
        &incremental,
        "DELTA versus independent FULL",
    )?;
    rebuilt_daemon.stop()
}

type CrashFiles = BTreeMap<usize, (String, String)>;
type CrashEdit = (usize, Option<(&'static str, &'static str)>);

fn publish_crash_fixture_full(
    client: &NativeUdsClient,
    files: &CrashFiles,
) -> TestResult<(ViewRef, BTreeMap<usize, ObjectRef>)> {
    publish_crash_fixture_sources(client, configure(client)?, files)
}

fn publish_crash_fixture_sources(
    client: &NativeUdsClient,
    base: ViewRef,
    files: &CrashFiles,
) -> TestResult<(ViewRef, BTreeMap<usize, ObjectRef>)> {
    let ingress = client.begin_input(operation(2), &budget())?;
    let sources = stage(
        client,
        ingress,
        files
            .values()
            .map(|(path, body)| fixture_source(path.clone(), body.clone()))
            .collect(),
    )?;
    let refs: BTreeMap<_, _> = files.keys().copied().zip(sources).collect();
    let mutations = refs
        .iter()
        .map(|(&index, &source)| {
            Ok(OwnerMutation {
                owner: fixture_owner(index)?,
                expected: None,
                next: Some(source),
            })
        })
        .collect::<TestResult<Vec<_>>>()?;
    let request = prepare_advance(client, ingress, base, mutations)?;
    // Initial and reference FULLs are fixture setup for the crash tests.
    // Keep the bounded submit budgets on the tested DELTA and recovery paths.
    let setup_budget = RequestBudgetV1::for_duration(Duration::from_secs(120));
    let started = Instant::now();
    let view = submit_advance(client, request, &setup_budget)?;
    eprintln!(
        "crash fixture FULL with {} owners published in {:?}",
        files.len(),
        started.elapsed()
    );
    Ok((view, refs))
}

fn prepare_crash_fixture_delta(
    client: &NativeUdsClient,
    prior: ViewRef,
    sequence: u64,
    edits: &[CrashEdit],
    files: &mut CrashFiles,
    refs: &mut BTreeMap<usize, ObjectRef>,
) -> TestResult<UpdateRequest> {
    let ingress = client.begin_input(operation(sequence), &budget())?;
    let sources = stage(
        client,
        ingress,
        edits
            .iter()
            .filter_map(|(_, edit)| *edit)
            .map(|(path, body)| fixture_source(path.into(), body.into()))
            .collect(),
    )?;
    let mut sources = sources.into_iter();
    let mut mutations = Vec::with_capacity(edits.len());
    for &(index, edit) in edits {
        let expected = refs.remove(&index);
        let next = match edit {
            Some((path, body)) => {
                let object = sources.next().ok_or("staged edit lost a source")?;
                let _previous = refs.insert(index, object);
                let _previous = files.insert(index, (path.into(), body.into()));
                Some(object)
            }
            None => {
                require(
                    files.remove(&index).is_some(),
                    "deleting an absent fixture file",
                )?;
                None
            }
        };
        mutations.push(OwnerMutation {
            owner: fixture_owner(index)?,
            expected,
            next,
        });
    }
    require(sources.next().is_none(), "unused staged source")?;
    prepare_advance(client, ingress, prior, mutations)
}

fn verify_text_page(
    client: &NativeUdsClient,
    session: &NativeReadSessionStatus,
    query: &TextQueryRequest,
    prior: Option<NativeTextCursor>,
    expected: &[(OwnerKey, String, u64)],
    offset: usize,
) -> TestResult<Option<NativeTextCursor>> {
    let remaining = expected.get(offset..).ok_or("invalid held page offset")?;
    let page = client.search_text_page_in_read_session(
        session,
        text_instance().instance,
        query.clone(),
        prior,
        &budget(),
    )?;
    require_equal(&page.session, &session.session, "held page session")?;
    require_equal(&page.view, &session.view, "held page view")?;
    require_equal(
        &page.matched,
        &u64::try_from(expected.len())?,
        "held page total",
    )?;
    require_equal(
        &page.remaining,
        &u64::try_from(remaining.len())?,
        "held page remaining",
    )?;
    let actual: Vec<_> = page
        .hits
        .into_iter()
        .map(|hit| (hit.owner, hit.path, hit.score.get().to_bits()))
        .collect();
    let limit = usize::try_from(query.top_k)?;
    let count = remaining.len().min(limit);
    require_equal(
        &actual.as_slice(),
        &remaining.get(..count).ok_or("missing text page gold")?,
        "held page rank and score",
    )?;
    require_equal(
        &page.after.is_some(),
        &(remaining.len() > limit),
        "held page cursor presence",
    )?;
    Ok(page.after)
}

fn create_crash_fixture(
    directory: &Path,
    name: &str,
) -> TestResult<(std::path::PathBuf, std::path::PathBuf)> {
    let root = directory.join(name);
    fs::create_dir(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let socket = directory.join(format!("{name}.sock"));
    let config = directory.join(format!("{name}.json"));
    let mut configuration = with_layered_text(config_with_reclaim(&root, &socket)?)?;
    configuration["read_sessions"] = serde_json::json!({
        "global_limit": 2,
        "per_uid_limit": 2,
        "terminal_ttl_millis": 300_000_u64,
        "sweep_limit": 2
    });
    fs::write(&config, serde_json::to_vec(&configuration)?)?;
    Ok((config, socket))
}

pub(super) fn kill_fixture_daemon(mut daemon: Daemon) -> TestResult<u32> {
    let pid = daemon.0.id();
    daemon.0.kill()?;
    require_equal(
        &daemon.0.wait()?.signal(),
        &Some(rustix::process::Signal::KILL.as_raw()),
        "daemon hard exit",
    )?;
    Ok(pid)
}

#[test]
fn native_multipage_lexical_delta_survives_sigkill_and_matches_full() -> TestResult {
    const OWNERS: usize = 132;
    const ROUNDS: [&[CrashEdit]; 3] = [
        &[
            (
                0,
                Some(("src/boundary000.rs", "stressanchor firstwave firstwave")),
            ),
            (127, Some(("src/moved-127.rs", "stressanchor seedterm"))),
            (128, None),
            (140, Some(("src/added-140.rs", "stressanchor firstwave"))),
        ],
        &[
            (
                0,
                Some(("src/boundary000.rs", "stressanchor firstwave firstwave")),
            ),
            (128, Some(("src/reborn-128.rs", "stressanchor secondwave"))),
            (
                129,
                Some(("src/moved-129.rs", "stressanchor secondwave secondwave")),
            ),
            (140, None),
        ],
        &[
            (127, None),
            (130, Some(("src/moved-130.rs", "stressanchor thirdwave"))),
            (
                131,
                Some(("src/boundary131.rs", "stressanchor thirdwave thirdwave")),
            ),
            (
                141,
                Some(("src/added-141.rs", "stressanchor thirdwave seedterm")),
            ),
        ],
    ];
    const PROBES: [&str; 7] = [
        "stressanchor",
        "seedterm",
        "firstwave",
        "secondwave",
        "thirdwave",
        "\"stressanchor thirdwave\"",
        "file.contains('secondwave secondwave')",
    ];

    let temp = quanta_index_searchd_harness::private_tempdir()?;
    let (config, socket) = create_crash_fixture(temp.path(), "stress-delta")?;
    let daemon = Daemon::start(&config, &socket)?;
    let client = NativeUdsClient::new(socket.clone());
    let mut files: CrashFiles = (0..OWNERS)
        .map(|index| (index, (fixture_path(index), "stressanchor seedterm".into())))
        .collect();
    let (base, mut refs) = publish_crash_fixture_full(&client, &files)?;
    let mut held = client.open_read_session(ViewSelection::Exact(base), [91; 32], &budget())?;
    let page_query = text_query("stressanchor", 31);
    let zero_score_query = text_query("NOT seedterm", 2);
    require_equal(
        &verify_text_page(&client, &held, &zero_score_query, None, &[], 0)?,
        &None,
        "initial FULL has no negative matches",
    )?;
    let gold = ranked_files(&client, base, "stressanchor", 256)?;
    require_equal(&gold.len(), &OWNERS, "partition-crossing tie count")?;
    let (_, _, score) = gold.first().ok_or("missing initial tie")?;
    for (index, row) in gold.iter().enumerate() {
        require_equal(
            row,
            &(fixture_owner(index)?, fixture_path(index), *score),
            "fixed tie and ascending owner order",
        )?;
    }

    let after = verify_text_page(&client, &held, &page_query, None, &gold, 0)?;
    let first = prepare_crash_fixture_delta(&client, base, 3, ROUNDS[0], &mut files, &mut refs)?;
    let first_view = published(&client.submit(first.clone(), &budget())?)?;
    let after = verify_text_page(&client, &held, &page_query, after, &gold, 31)?;
    let second =
        prepare_crash_fixture_delta(&client, first_view, 4, ROUNDS[1], &mut files, &mut refs)?;
    require_equal(
        &client.status(second.operation, &budget())?,
        &OperationStatus::Unknown,
        "sealed DELTA must not be published before the crash",
    )?;
    let after = verify_text_page(&client, &held, &page_query, after, &gold, 62)?;

    let old_pid = kill_fixture_daemon(daemon)?;
    let reopened = Daemon::start(&config, &socket)?;
    require(
        reopened.0.id() != old_pid,
        "crash did not replace the daemon PID",
    )?;
    let client = NativeUdsClient::new(socket.clone());
    let latest = ViewSelection::Latest {
        workspace: workspace(),
        epoch: 1,
    };
    require_equal(
        &client.read_view(latest, &budget())?.identity,
        &first_view,
        "head after SIGKILL",
    )?;
    require_equal(
        &client.status(first.operation, &budget())?,
        &OperationStatus::Published(first_view),
        "durable published operation",
    )?;
    require_equal(
        &published(&client.submit(first, &budget())?)?,
        &first_view,
        "published DELTA replay",
    )?;
    require_equal(
        &client.status(second.operation, &budget())?,
        &OperationStatus::Unknown,
        "sealed operation is still unpublished",
    )?;
    held = client.open_read_session(ViewSelection::Exact(base), [92; 32], &budget())?;
    let after = verify_text_page(&client, &held, &page_query, after, &gold, 93)?;

    let second_view = published(&client.submit(second, &budget())?)?;
    let third =
        prepare_crash_fixture_delta(&client, second_view, 5, ROUNDS[2], &mut files, &mut refs)?;
    let final_operation = third.operation;
    let final_view = published(&client.submit(third, &budget())?)?;
    require_equal(
        &client.read_view(latest, &budget())?.identity,
        &final_view,
        "final DELTA is current",
    )?;
    require_equal(
        &verify_text_page(&client, &held, &page_query, after, &gold, 124)?,
        &None,
        "last held page across the partition boundary",
    )?;
    let _released = client.release_read_session(&held, &budget())?;

    require_equal(&files.len(), &OWNERS, "final oracle owner count")?;
    let expected = files
        .iter()
        .map(|(&index, (path, _))| Ok((fixture_owner(index)?, path.clone())))
        .collect::<TestResult<Vec<_>>>()?;
    require_equal(
        &observed_owners(&client, final_view, "stressanchor")?,
        &expected,
        "final live owners and paths",
    )?;
    let (full_config, full_socket) = create_crash_fixture(temp.path(), "stress-full")?;
    let full_daemon = Daemon::start(&full_config, &full_socket)?;
    let full_client = NativeUdsClient::new(full_socket);
    let (full_view, _) = publish_crash_fixture_full(&full_client, &files)?;
    for query in PROBES {
        let delta = ranked_files(&client, final_view, query, 256)?;
        let full = ranked_files(&full_client, full_view, query, 256)?;
        require_equal(
            &delta,
            &full,
            &format!("post-SIGKILL DELTA vs independent FULL rank and score bits: {query}"),
        )?;
    }
    let full_rank = ranked_files(&full_client, full_view, &page_query.query_text, 256)?;
    let zero_rank = [0, 128, 129, 130, 131]
        .into_iter()
        .map(|id| {
            let (path, _) = files
                .get(&id)
                .ok_or("negative match owner missing from model")?;
            Ok((fixture_owner(id)?, path.clone(), 0.0_f64.to_bits()))
        })
        .collect::<TestResult<Vec<_>>>()?;
    require_equal(
        &ranked_files(&full_client, full_view, &zero_score_query.query_text, 256)?,
        &zero_rank,
        "independent FULL negative-match score and owner order",
    )?;
    let selection = ViewSelection::Exact(final_view);
    let session = client.open_read_session(selection, [93; 32], &budget())?;
    let full_after = verify_text_page(&client, &session, &page_query, None, &full_rank, 0)?;
    let zero_after = verify_text_page(&client, &session, &zero_score_query, None, &zero_rank, 0)?;
    let reopened_pid = kill_fixture_daemon(reopened)?;
    let cold = Daemon::start(&config, &socket)?;
    require(
        cold.0.id() != reopened_pid,
        "second crash did not replace the daemon PID",
    )?;
    let cold_client = NativeUdsClient::new(socket);
    require_equal(
        &cold_client.read_view(latest, &budget())?.identity,
        &final_view,
        "final DELTA remains current after the second SIGKILL",
    )?;
    require_equal(
        &cold_client.status(final_operation, &budget())?,
        &OperationStatus::Published(final_view),
        "final operation remains published after the second SIGKILL",
    )?;
    let session = cold_client.open_read_session(selection, [94; 32], &budget())?;
    for (query, rank, mut after) in [
        (&page_query, &full_rank, full_after),
        (&zero_score_query, &zero_rank, zero_after),
    ] {
        let limit = usize::try_from(query.top_k)?;
        for offset in (limit..rank.len()).step_by(limit) {
            after = verify_text_page(&cold_client, &session, query, after, rank, offset)?;
        }
        require_equal(&after, &None, "last final DELTA page after cold reopen")?;
    }
    let _released = cold_client.release_read_session(&session, &budget())?;
    full_daemon.stop()?;
    cold.stop()
}

fn write_mutation_page(
    client: &NativeUdsClient,
    ingress: IngressRef,
    base: MapRoot,
    rows: &[OwnerMutation],
) -> Result<NativeIndexResponse, NativeUdsError> {
    client.request(
        NativeIndexRequest::StageMutationPage {
            ingress,
            base,
            mutations: rows.to_vec(),
        },
        &budget(),
    )
}

fn require_wire_refusal(
    reply: Result<NativeIndexResponse, NativeUdsError>,
    code: NativeIndexErrorCode,
    label: &str,
) -> TestResult {
    match reply {
        Err(NativeUdsError::Refused(error)) => require_equal(&error.code, &code, label),
        other => Err(format!("{label}: expected {code:?}, got {other:?}").into()),
    }
}

#[test]
fn native_lexical_partial_mutation_page_recovers_after_sigkill() -> TestResult {
    const BASE_OWNERS: usize = 144;
    const ADDED_OWNER: usize = 300;
    const REMOVED_OWNERS: [usize; 3] = [0, 128, 143];
    const MUTATIONS: u64 = 145;
    let temp = quanta_index_searchd_harness::private_tempdir()?;
    let (config, socket) = create_crash_fixture(temp.path(), "page-delta")?;
    let daemon = Daemon::start(&config, &socket)?;
    let client = NativeUdsClient::new(socket.clone());
    let initial = (0..BASE_OWNERS)
        .map(|index| (index, (fixture_path(index), "pageanchor oldneedle".into())))
        .collect();
    let (base, old_refs) = publish_crash_fixture_full(&client, &initial)?;

    let mut final_files: CrashFiles = (0..BASE_OWNERS)
        .filter(|index| !REMOVED_OWNERS.contains(index))
        .map(|index| {
            let path = if matches!(index, 16 | 127 | 129) {
                format!("src/moved-{index:03}.rs")
            } else {
                fixture_path(index)
            };
            let body = if index % 2 == 0 {
                "pageanchor evenwave tailterm"
            } else {
                "pageanchor oddwave oddwave"
            };
            (index, (path, body.into()))
        })
        .collect();
    let _previous = final_files.insert(
        ADDED_OWNER,
        (
            "src/recovered-last.rs".into(),
            "pageanchor tailproof".into(),
        ),
    );
    let key = operation(3);
    let ingress = client.begin_input(key, &budget())?;
    let sources = stage(
        &client,
        ingress,
        final_files
            .values()
            .map(|(path, body)| fixture_source(path.clone(), body.clone()))
            .collect(),
    )?;
    require_equal(&sources.len(), &final_files.len(), "successor source count")?;
    let next_refs: BTreeMap<_, _> = final_files.keys().copied().zip(sources).collect();
    let mutations = (0..BASE_OWNERS)
        .chain(std::iter::once(ADDED_OWNER))
        .map(|index| {
            Ok(OwnerMutation {
                owner: fixture_owner(index)?,
                expected: old_refs.get(&index).copied(),
                next: next_refs.get(&index).copied(),
            })
        })
        .collect::<TestResult<Vec<_>>>()?;
    let (lower, higher) = mutations
        .split_at_checked(17)
        .ok_or("mutation prefix split")?;
    require_equal(&higher.len(), &128, "first network page must be full")?;
    let NativeIndexResponse::MutationPageStaged(prefix) =
        write_mutation_page(&client, ingress, MapRoot(None), higher)?
    else {
        return Err("missing first mutation page ACK".into());
    };
    let prefix_ref = prefix.0.ok_or("first page ACK had no root")?;

    let old_pid = kill_fixture_daemon(daemon)?;
    let reopened = Daemon::start(&config, &socket)?;
    require(
        reopened.0.id() != old_pid,
        "restart did not replace the daemon PID",
    )?;
    let client = NativeUdsClient::new(socket);
    let latest = ViewSelection::Latest {
        workspace: workspace(),
        epoch: 1,
    };
    let foreign = client.begin_input(operation(4), &budget())?;
    let reopen = |operation| {
        client.request(
            NativeIndexRequest::ReopenInputWithRootForSourceStageV1 {
                operation,
                root: prefix_ref,
            },
            &budget(),
        )
    };
    require_wire_refusal(
        reopen(foreign.operation),
        NativeIndexErrorCode::InvalidInput,
        "foreign ingress must not acquire the ACK root",
    )?;
    client.discard_input(foreign, &budget())?;
    let NativeIndexResponse::InputOpened(recovered) = reopen(key)? else {
        return Err("confirmed prefix did not reopen".into());
    };
    require_equal(&recovered.operation, &key, "recovered operation")?;
    require(
        recovered.fence > ingress.fence,
        "reopened input did not advance its fence",
    )?;
    require_wire_refusal(
        write_mutation_page(&client, ingress, prefix, lower),
        NativeIndexErrorCode::FenceLost,
        "old fence must not write the tail",
    )?;
    require_wire_refusal(
        write_mutation_page(
            &client,
            recovered,
            prefix,
            higher.get(..1).ok_or("missing duplicate witness")?,
        ),
        NativeIndexErrorCode::Conflict,
        "duplicate owner across pages",
    )?;
    let seal = |root, count| {
        client.request(
            NativeIndexRequest::SealMutations {
                ingress: recovered,
                root,
                count,
            },
            &budget(),
        )
    };
    require_wire_refusal(
        seal(prefix, MUTATIONS),
        NativeIndexErrorCode::InvalidInput,
        "a prefix cannot seal as the whole set",
    )?;
    require_equal(
        &client.status(key, &budget())?,
        &OperationStatus::Unknown,
        "no operation admitted by stage or refusals",
    )?;
    require_equal(
        &client.read_view(latest, &budget())?.identity,
        &base,
        "partial page and refusals must preserve the head",
    )?;

    let NativeIndexResponse::MutationPageStaged(root) =
        write_mutation_page(&client, recovered, prefix, lower)?
    else {
        return Err("missing recovered tail ACK".into());
    };
    let NativeIndexResponse::MutationsStaged(sealed) = seal(root, MUTATIONS)? else {
        return Err("missing recovered mutation seal".into());
    };
    let mut ordered_digest = OwnerMutationDigest::default();
    for mutation in &mutations {
        ordered_digest.push(mutation)?;
    }
    require_equal(
        &sealed,
        &OwnerMutationSet {
            root,
            count: MUTATIONS,
            digest: ordered_digest.finish(),
        },
        "recovered mutation seal preserves sorted content",
    )?;

    let update = prepare_sealed_advance(&client, key, base, sealed)?;
    let published_view = published(&client.submit(update.clone(), &budget())?)?;
    require_equal(
        &published(&client.submit(update, &budget())?)?,
        &published_view,
        "exact submit replay",
    )?;
    require_equal(
        &client.read_view(latest, &budget())?.identity,
        &published_view,
        "only the complete DELTA is current",
    )?;
    require_equal(
        &ranked_files(&client, base, "oldneedle", 256)?.len(),
        &BASE_OWNERS,
        "old pinned generation retains deleted and replaced content",
    )?;
    require_equal(
        &observed_owners(&client, published_view, "tailproof")?,
        &vec![(fixture_owner(ADDED_OWNER)?, "src/recovered-last.rs".into())],
        "highest owner survives stage recovery",
    )?;

    let (full_config, full_socket) = create_crash_fixture(temp.path(), "page-full")?;
    let full_daemon = Daemon::start(&full_config, &full_socket)?;
    let full_client = NativeUdsClient::new(full_socket);
    let (rebuilt, _) = publish_crash_fixture_full(&full_client, &final_files)?;
    for query in [
        "pageanchor",
        "evenwave",
        "oddwave",
        "tailproof",
        "oldneedle",
        "\"pageanchor oddwave\"",
    ] {
        require_equal(
            &ranked_files(&client, published_view, query, 256)?,
            &ranked_files(&full_client, rebuilt, query, 256)?,
            &format!("recovered page DELTA versus independent FULL: {query}"),
        )?;
    }
    full_daemon.stop()?;
    reopened.stop()
}

fn fixture_body_ref(body: &str) -> TestResult<ObjectRef> {
    quanta_index_contract::index::native_immutable_object_ref_v1(body.as_bytes(), &[])
        .ok_or_else(|| "fixture body could not produce an immutable ref".into())
}

fn verify_commit_file_inventory(
    client: &NativeUdsClient,
    view: ViewRef,
    files: &CrashFiles,
    refs: &BTreeMap<usize, ObjectRef>,
) -> TestResult {
    let repository = quanta_index_contract::RepoId::try_from(REPOSITORY)?;
    let selection = ViewSelection::Exact(view);
    let mut expected = files
        .iter()
        .map(|(&index, (path, body))| {
            let source = refs.get(&index).ok_or("missing expected file source")?;
            Ok((
                path.clone(),
                fixture_owner(index)?,
                *source,
                fixture_body_ref(body)?,
                u64::try_from(body.len())?,
            ))
        })
        .collect::<TestResult<Vec<_>>>()?;
    expected.sort_unstable();
    let mut after = None;
    let mut actual = Vec::new();
    loop {
        let page = client.query_repository_paths(
            selection,
            file_spec().instance,
            String::new(),
            after,
            128,
            &budget(),
        )?;
        require_equal(&page.view, &view, "commit file inventory view")?;
        for entry in page.entries {
            require_equal(
                &entry.repository.as_ref(),
                &Some(&repository),
                "commit file inventory repository",
            )?;
            let entry = entry.file;
            actual.push((
                entry.path,
                entry.owner,
                entry.source,
                entry.bytes,
                entry.byte_len,
            ));
        }
        require(
            actual.len() <= expected.len(),
            "commit file inventory exceeded the model",
        )?;
        if page.next_after.is_none() {
            break;
        }
        after = page.next_after;
    }
    require_equal(&actual, &expected, "commit file inventory across pages")?;

    for pattern in ["commitanchor", "original", "carried", "newleaf"] {
        let expected = files
            .iter()
            .filter(|(_, (_, body))| body.contains(pattern))
            .map(|(&index, (path, body))| {
                Ok((
                    fixture_owner(index)?,
                    path.clone(),
                    fixture_body_ref(body)?,
                    u64::try_from(body.len())?,
                ))
            })
            .collect::<TestResult<Vec<_>>>()?;
        let mut after = None;
        let mut actual = Vec::new();
        loop {
            let page = client.search_literal(
                selection,
                literal_instance().instance,
                pattern.as_bytes(),
                after.as_deref(),
                128,
                &budget(),
            )?;
            require_equal(&page.view, &view, "commit literal view")?;
            for hit in page.hits {
                require_equal(
                    &hit.record.source_repo_id.as_ref(),
                    &Some(&repository),
                    "commit literal repository",
                )?;
                require_equal(
                    &hit.record.text,
                    &SourceTextAdmission::Utf8 {
                        policy: Digest([31; 32]),
                    },
                    "commit literal text admission",
                )?;
                require_equal(
                    &hit.record.language,
                    &None,
                    "commit literal source language",
                )?;
                actual.push((
                    hit.owner,
                    hit.record.path,
                    hit.record.bytes,
                    hit.record.byte_len,
                ));
            }
            require(
                actual.len() <= expected.len(),
                "commit literal exceeded the model",
            )?;
            if page.after.is_none() {
                break;
            }
            after = page.after;
        }
        require_equal(
            &actual,
            &expected,
            &format!("commit literal matches: {pattern}"),
        )?;
    }
    Ok(())
}

struct RecordedFrame<R> {
    reader: R,
    bytes: Vec<u8>,
}

impl<R: std::io::Read> std::io::Read for RecordedFrame<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.reader.read(buffer)?;
        self.bytes.extend_from_slice(&buffer[..read]);
        Ok(read)
    }
}

fn resume_with_truncated_publish_reply(
    proxy: &Path,
    socket: &Path,
    daemon: Daemon,
    key: OperationKey,
) -> TestResult<(u32, ViewRef)> {
    use std::io::{ErrorKind, Write as _};
    use std::net::Shutdown;

    let listener = UnixListener::bind(proxy)?;
    listener.set_nonblocking(true)?;
    thread::scope(|scope| -> TestResult<_> {
        let caller =
            scope.spawn(|| NativeUdsClient::new(proxy.to_path_buf()).resume(key, &budget()));
        let deadline = daemon_phase_deadline(Duration::from_secs(5), None)?;
        let mut peer = loop {
            match listener.accept() {
                Ok((peer, _)) => break peer,
                Err(error)
                    if error.kind() == ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::sleep(NATIVE_LIFECYCLE_POLL);
                }
                Err(error) => return Err(error.into()),
            }
        };
        peer.set_nonblocking(false)?;
        peer.set_read_timeout(Some(Duration::from_secs(5)))?;
        peer.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut request_frame = RecordedFrame {
            reader: &mut peer,
            bytes: Vec::new(),
        };
        let request: NativeIndexRequestEnvelope =
            quanta_index_ipc::decode_request(&mut request_frame)?;
        require_equal(
            &request.payload,
            &NativeIndexRequest::Resume(key),
            "proxied exact Resume",
        )?;

        let mut backend = UnixStream::connect(socket)?;
        backend.set_read_timeout(Some(Duration::from_secs(30)))?;
        backend.set_write_timeout(Some(Duration::from_secs(5)))?;
        backend.write_all(&request_frame.bytes)?;
        let mut reply_frame = RecordedFrame {
            reader: backend,
            bytes: Vec::new(),
        };
        let reply: NativeIndexResponseEnvelope =
            quanta_index_ipc::decode_response(&mut reply_frame)?;
        require_equal(
            &reply.request_id,
            &request.request_id,
            "forwarded request ID",
        )?;
        let NativeIndexResponse::Submitted(receipt) = &reply.payload else {
            return Err(format!("daemon did not return a publication receipt: {reply:?}").into());
        };
        require_equal(&receipt.operation, &key, "daemon receipt operation")?;
        let published = published(&receipt.payload)?;

        // Only the proxy receives the complete frame. Keep the SDK peer open
        // until after the real daemon dies, then expose EOF one byte early.
        let (_, prefix) = reply_frame
            .bytes
            .split_last()
            .ok_or("empty terminal response frame")?;
        peer.write_all(prefix)?;
        let pid = kill_fixture_daemon(daemon)?;
        peer.shutdown(Shutdown::Write)?;
        drop(peer);
        let result = caller.join().map_err(|_| "Resume client thread panicked")?;
        require(
            matches!(&result, Err(NativeUdsError::Unknown(operation)) if *operation == key),
            format!("partial terminal response must preserve the unknown operation: {result:?}"),
        )?;
        Ok((pid, published))
    })
}

#[test]
fn native_lexical_commit_rollback_and_truncated_published_ack_preserve_one_operation() -> TestResult
{
    const OWNERS: usize = 131;
    const EDITS: &[CrashEdit] = &[
        (
            0,
            Some(("src/boundary000.rs", "commitanchor rebound rebound")),
        ),
        (127, Some(("src/renamed-127.rs", "commitanchor carried"))),
        (128, None),
        (144, Some(("src/inserted-144.rs", "commitanchor newleaf"))),
    ];
    let temp = quanta_index_searchd_harness::private_tempdir()?;
    let (config, socket) = create_crash_fixture(temp.path(), "commit-delta")?;
    let catalog_path = temp
        .path()
        .join("commit-delta/catalog")
        .join(quanta_index_catalog::CATALOG_FILE_NAME);
    let first_daemon = Daemon::start(&config, &socket)?;
    let client = NativeUdsClient::new(socket.clone());
    let instances = vec![literal_instance(), file_spec(), text_instance()];
    let mut files = (0..OWNERS)
        .map(|index| {
            let body = format!("commitanchor original fixtureowner{index:03}");
            (index, (fixture_path(index), body))
        })
        .collect();
    let configured = configure_instances(&client, instances.clone())?;
    let (base, mut refs) = publish_crash_fixture_sources(&client, configured, &files)?;
    let original_files = files.clone();
    let original_refs = refs.clone();
    let original_source = client
        .read_view(ViewSelection::Exact(base), &budget())?
        .source;
    let initial_rank = ranked_files(&client, base, "original", 256)?;
    require_equal(&initial_rank.len(), &OWNERS, "initial rank count")?;
    let update = prepare_crash_fixture_delta(&client, base, 3, EDITS, &mut files, &mut refs)?;
    let key = update.operation;
    let next_sequence = base
        .sequence
        .checked_add(1)
        .ok_or("view sequence overflow")?;
    let latest = ViewSelection::Latest {
        workspace: workspace(),
        epoch: 1,
    };
    let initial_literal = client.search_literal(
        latest,
        literal_instance().instance,
        b"commitanchor",
        None,
        128,
        &budget(),
    )?;
    require_equal(
        &initial_literal.view,
        &base,
        "initial Literal selects the current view",
    )?;
    let old_literal_cursor = initial_literal
        .after
        .ok_or("missing initial Literal continuation")?;
    let initial_paths = client.query_repository_paths(
        latest,
        file_spec().instance,
        "src/".into(),
        None,
        128,
        &budget(),
    )?;
    require_equal(
        &initial_paths.view,
        &base,
        "initial path page selects the current view",
    )?;
    let old_path_cursor = initial_paths
        .next_after
        .ok_or("missing initial repository path continuation")?;
    let catalog_workspace = serde_json::to_vec(&base.workspace)?;
    let catalog_operation = serde_json::to_vec(&key)?;
    let catalog_epoch = i64::try_from(base.epoch)?;
    let catalog_sequence = i64::try_from(base.sequence)?;
    let read_catalog = || {
        rusqlite::Connection::open_with_flags(
            &catalog_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
    };
    let successor_rows = || -> TestResult<i64> {
        Ok(read_catalog()?.query_row(
            "SELECT COUNT(*) FROM native_view_v1 WHERE workspace=?1 AND epoch=?2 AND sequence>?3",
            rusqlite::params![&catalog_workspace, catalog_epoch, catalog_sequence],
            |row| row.get(0),
        )?)
    };
    let operation_row = || -> TestResult<i64> {
        Ok(read_catalog()?.query_row(
            "SELECT operation_id FROM native_operation_v1 WHERE operation=?1",
            [&catalog_operation],
            |row| row.get(0),
        )?)
    };

    {
        let catalog = rusqlite::Connection::open(&catalog_path)?;
        catalog.busy_timeout(Duration::from_secs(5))?;
        // Only the fixture's final operation write is refused. The matching
        // successor head is visible inside that transaction after write_view,
        // but the pending admission is already in a different transaction.
        catalog.execute_batch(&format!(
            "CREATE TRIGGER fixture_abort_terminal BEFORE UPDATE OF payload ON native_operation_v1
             WHEN EXISTS (SELECT 1 FROM native_workspace_v1
                          WHERE workspace=OLD.workspace AND epoch=OLD.epoch AND sequence={next_sequence})
             BEGIN SELECT RAISE(ABORT, 'fixture refuses the publication terminal write'); END;"
        ))?;
    }
    require_equal(
        &client.submit_borrowed(&update, &budget())?,
        &SubmitOutcome::Unknown(key),
        "injected commit fault leaves uncertainty",
    )?;
    let OperationStatus::Pending(attempt) = client.status(key, &budget())? else {
        return Err("failed publication did not retain its admitted operation".into());
    };
    require_equal(&attempt.operation, &key, "durable pending operation")?;
    let admitted_row = operation_row()?;
    require_equal(
        &client.read_view(latest, &budget())?.identity,
        &base,
        "failed terminal write rolled back the head",
    )?;
    require_equal(
        &successor_rows()?,
        &0,
        "failed terminal write rolled back the successor row",
    )?;
    verify_commit_file_inventory(&client, base, &original_files, &original_refs)?;
    let first_pid = kill_fixture_daemon(first_daemon)?;
    rusqlite::Connection::open(&catalog_path)?
        .execute_batch("DROP TRIGGER fixture_abort_terminal")?;

    let second_daemon = Daemon::start(&config, &socket)?;
    require(
        second_daemon.0.id() != first_pid,
        "pending recovery reused the old daemon PID",
    )?;
    let client = NativeUdsClient::new(socket.clone());
    require_equal(
        &client.status(key, &budget())?,
        &OperationStatus::Pending(attempt),
        "same attempt after failed commit and death",
    )?;
    require_equal(
        &client.submit_borrowed(&update, &budget())?,
        &SubmitOutcome::Accepted(attempt),
        "duplicate Submit does not create an attempt",
    )?;
    require_equal(
        &successor_rows()?,
        &0,
        "pending Submit replay does not publish a view",
    )?;
    let (second_pid, published_view) = resume_with_truncated_publish_reply(
        &temp.path().join("ack-proxy.sock"),
        &socket,
        second_daemon,
        key,
    )?;
    require_equal(
        &published_view.sequence,
        &next_sequence,
        "single publication sequence",
    )?;

    let third_daemon = Daemon::start(&config, &socket)?;
    require(
        third_daemon.0.id() != second_pid,
        "ACK-loss recovery reused the old daemon PID",
    )?;
    let client = NativeUdsClient::new(socket);
    let terminal = OperationStatus::Published(published_view);
    require_equal(
        &client.status(key, &budget())?,
        &terminal,
        "durable answer after the truncated ACK",
    )?;
    require_equal(
        &client.resume(key, &budget())?,
        &SubmitOutcome::Terminal(terminal.clone()),
        "Resume replays the same publication",
    )?;
    require_equal(
        &client.submit_borrowed(&update, &budget())?,
        &SubmitOutcome::Terminal(terminal),
        "Submit replays the same publication",
    )?;
    require_equal(
        &client.read_view(latest, &budget())?.identity,
        &published_view,
        "terminal retries preserve the exact head",
    )?;
    require_equal(
        &successor_rows()?,
        &1,
        "only one view survived the two faults and all retries",
    )?;
    require_equal(
        &operation_row()?,
        &admitted_row,
        "same admission row across recovery and retries",
    )?;
    require_equal(
        &ranked_files(&client, base, "original", 256)?,
        &initial_rank,
        "prior exact rank survives both SIGKILLs",
    )?;
    verify_commit_file_inventory(&client, base, &original_files, &original_refs)?;
    verify_commit_file_inventory(&client, published_view, &files, &refs)?;
    let exact_tail = client.search_literal(
        ViewSelection::Exact(base),
        literal_instance().instance,
        b"commitanchor",
        Some(&old_literal_cursor),
        128,
        &budget(),
    )?;
    let expected_tail = (128..OWNERS)
        .map(|id| Ok((fixture_owner(id)?, fixture_path(id))))
        .collect::<TestResult<Vec<_>>>()?;
    let actual_tail = exact_tail
        .hits
        .into_iter()
        .map(|hit| (hit.owner, hit.record.path))
        .collect::<Vec<_>>();
    require_equal(
        &actual_tail,
        &expected_tail,
        "old Literal cursor continues against its exact view",
    )?;
    require(
        !exact_tail.has_more,
        "old Literal cursor did not reach its last page",
    )?;
    require_wire_refusal(
        client.request(
            NativeIndexRequest::SearchLiteral {
                selection: latest,
                instance: literal_instance().instance,
                literal: b"commitanchor".to_vec(),
                after: Some(old_literal_cursor),
                limit: 128,
            },
            &budget(),
        ),
        NativeIndexErrorCode::InvalidInput,
        "Literal continuation refuses Latest after a publication",
    )?;
    let exact_paths = client.query_repository_paths(
        ViewSelection::Exact(base),
        file_spec().instance,
        "src/".into(),
        Some(old_path_cursor.clone()),
        128,
        &budget(),
    )?;
    let actual_paths = exact_paths
        .entries
        .into_iter()
        .map(|entry| (entry.file.owner, entry.file.path))
        .collect::<Vec<_>>();
    require_equal(
        &actual_paths,
        &expected_tail,
        "old path cursor continues against its exact view",
    )?;
    require(
        !exact_paths.has_more,
        "old path cursor did not reach its last page",
    )?;
    require_wire_refusal(
        client.request(
            NativeIndexRequest::QueryRepositoryPaths {
                selection: latest,
                instance: file_spec().instance,
                prefix: "src/".into(),
                after: Some(old_path_cursor),
                limit: 128,
            },
            &budget(),
        ),
        NativeIndexErrorCode::InvalidInput,
        "repository path continuation refuses Latest after a publication",
    )?;

    let (full_config, full_socket) = create_crash_fixture(temp.path(), "commit-full")?;
    let full_daemon = Daemon::start(&full_config, &full_socket)?;
    let full_client = NativeUdsClient::new(full_socket);
    let configured = configure_instances(&full_client, instances)?;
    let (full, full_refs) = publish_crash_fixture_sources(&full_client, configured, &files)?;
    verify_commit_file_inventory(&full_client, full, &files, &full_refs)?;
    for (query, expected_count) in [
        ("commitanchor", OWNERS),
        ("original", OWNERS - 3),
        ("carried", 1),
        ("newleaf", 1),
        ("\"commitanchor rebound\"", 1),
    ] {
        let delta_rank = ranked_files(&client, published_view, query, 256)?;
        require_equal(
            &delta_rank.len(),
            &expected_count,
            &format!("commit and ACK-loss DELTA match count: {query}"),
        )?;
        require_equal(
            &delta_rank,
            &ranked_files(&full_client, full, query, 256)?,
            &format!("commit and ACK-loss DELTA versus independent FULL: {query}"),
        )?;
    }
    // Use the current ViewRef and owner ref with the old SourceCut to isolate
    // the Source CAS from view, owner, and producer frontier checks.
    let ingress = client.begin_input(operation(4), &budget())?;
    let source = *refs.get(&0).ok_or("missing current owner ref")?;
    let mut stale = prepare_advance(
        &client,
        ingress,
        published_view,
        [OwnerMutation {
            owner: fixture_owner(0)?,
            expected: Some(source),
            next: None,
        }],
    )?;
    let UpdateKind::SourceAdvance(transition) = &mut stale.kind else {
        return Err("source CAS fixture did not prepare a SourceAdvance".into());
    };
    require(
        transition.expected_source != original_source,
        "source CAS fixture retained the old source",
    )?;
    transition.expected_source = original_source;
    require_equal(
        &client.submit(stale, &budget())?,
        &SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict)),
        "stale SourceCut is refused with a current view and owner ref",
    )?;
    require_equal(
        &client.read_view(latest, &budget())?.identity,
        &published_view,
        "stale SourceCut preserves the head",
    )?;
    require_equal(
        &successor_rows()?,
        &1,
        "stale SourceCut cannot add a successor row",
    )?;
    full_daemon.stop()?;
    third_daemon.stop()
}
