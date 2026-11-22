//! Public Native Text equivalence at an immutable segment boundary.

use super::*;
use quanta_index_contract::index::{IngressRef, ViewRef};

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

fn fixture_config(root: &Path, socket: &Path) -> TestResult<serde_json::Value> {
    let mut configuration = config(root, socket);
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
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![text_instance()],
        },
        &budget(),
    )?;
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
    let sequence = ingress.operation.sequence;
    let before = client.read_view(ViewSelection::Exact(prior), &budget())?;
    let owners = client.stage_mutations(ingress, mutations, &budget())?;
    let event_sequence = sequence.checked_sub(1).ok_or("source event underflow")?;
    let mut transition = SourceTransition {
        expected_source: before.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation(sequence).producer,
        stream: operation(sequence).stream,
        stream_epoch: 1,
        predecessor: event_sequence.checked_sub(1).unwrap_or(0),
        sequence: event_sequence,
        payload: transition.owner_digest()?,
    });
    published(&client.submit(
        UpdateRequest {
            operation: operation(sequence),
            expected_view: prior,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)
}

fn observed_owners(
    client: &NativeUdsClient,
    view: ViewRef,
    query: &str,
) -> TestResult<Vec<(OwnerKey, String)>> {
    let result = client.search_text(
        ViewSelection::Exact(view),
        text_instance().instance,
        TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: query.into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            top_k: 256,
            cursor: None,
        },
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
    let mut owners = result
        .hits
        .into_iter()
        .map(|hit| (hit.owner, hit.path))
        .collect::<Vec<_>>();
    owners.sort_unstable_by_key(|(owner, _)| owner.stable_id.0);
    Ok(owners)
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

#[test]
fn native_boundary_delta_matches_independent_full_and_cold_reopen() -> TestResult {
    // wire::MAX_DOCS is 128. The 130-owner FULL and final corpus require
    // at least two physical document partitions in segmented-layered-v2.
    require(BASE_OWNERS > 128, "boundary fixture fits one Text segment")?;
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
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
