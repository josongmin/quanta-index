//! Release proof cases for activation, retained keysets, and SDK query binding.

use std::{path::PathBuf, sync::mpsc};

use quanta_index_contract::{
    index::{ObjectRef, ViewRef},
    native_protocol::{
        NativeDslKindV1, NativeReadSessionRequest, NativeReadSessionState, NativeSemanticResult,
    },
};
use quanta_index_sdk::{ConnectOptions, NativeQueryError, QuantaIndex, SdkError};

use super::{
    read_view_lifetime::{TextOwnerUpdate, publish_text, stage_text_update},
    *,
};

struct ReleaseProcess {
    _home: tempfile::TempDir,
    root: PathBuf,
    config: PathBuf,
    socket: PathBuf,
    binary: PathBuf,
}

impl ReleaseProcess {
    fn new() -> TestResult<Self> {
        Self::with_config(|root, socket| {
            let mut settings = config_with_reclaim(root, socket)?;
            let _prior = settings
                .as_object_mut()
                .ok_or("Native config must be an object")?
                .insert(
                    "read_sessions".into(),
                    serde_json::json!({
                        "global_limit": 2,
                        "per_uid_limit": 2,
                        "terminal_ttl_millis": 300_000_u64,
                        "sweep_limit": 2
                    }),
                );
            Ok(settings)
        })
    }

    fn with_config(
        settings: impl FnOnce(&Path, &Path) -> TestResult<serde_json::Value>,
    ) -> TestResult<Self> {
        let home = quanta_index_searchd_harness::private_tempdir()?;
        let root = home.path().join("state");
        let socket = home.path().join("native.sock");
        let config = home.path().join("native-config.json");
        fs::write(&config, serde_json::to_vec(&settings(&root, &socket)?)?)?;
        let binary = match std::env::var_os("QUANTA_INDEX_SEARCHD_BIN") {
            Some(path) => PathBuf::from(path),
            None if std::env::var_os("QUANTA_PROOF_DAEMON_WITNESS_DIR").is_some() => {
                return Err("release proof requires QUANTA_INDEX_SEARCHD_BIN".into());
            }
            None => PathBuf::from(env!("CARGO_BIN_EXE_quanta-index-searchd")),
        };
        require(
            binary.is_absolute(),
            "Native release daemon path must be absolute",
        )?;
        Ok(Self {
            _home: home,
            root,
            config,
            socket,
            binary,
        })
    }

    fn start(&self) -> TestResult<Daemon> {
        Daemon::start_binary(&self.binary, &self.config, &self.socket, None, None)
    }

    fn client(&self) -> NativeUdsClient {
        NativeUdsClient::new(self.socket.clone())
    }
}

fn configure_text(client: &NativeUdsClient) -> TestResult<ViewRef> {
    configure(client, vec![text_spec()])
}

fn configure(client: &NativeUdsClient, instances: Vec<InstanceSpec>) -> TestResult<ViewRef> {
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("Native workspace creation returned an unexpected reply".into());
    };
    let operation = operation(1);
    let ingress = client.begin_input(operation, &budget())?;
    let profile = client.stage_profile(ingress, IndexProfile { instances }, &budget())?;
    published(&client.submit(
        UpdateRequest {
            operation,
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)
}

fn query(needle: &str, top_k: u32) -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: needle.into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        top_k,
        cursor: None,
    }
}

fn latest() -> ViewSelection {
    ViewSelection::Latest {
        workspace: workspace(),
        epoch: 1,
    }
}

fn check_one(
    result: &NativeTextResult,
    view: ViewRef,
    path: &str,
    source: ObjectRef,
) -> TestResult {
    let [hit] = result.hits.as_slice() else {
        return Err("expected exactly one Native text hit".into());
    };
    require_equal(&result.view, &view, "ranked view")?;
    require_equal(&result.matched, &1, "total matches")?;
    require_equal(&hit.path.as_str(), &path, "hit path")?;
    require_equal(&hit.source, &source, "hit source")
}

fn record_launches<const N: usize>(
    observations: [Option<read_view_daemon_witness::Observation>; N],
    file: &str,
    case: &str,
) -> TestResult {
    read_view_daemon_witness::write(
        observations,
        file,
        &nextest_test_event(module_path!(), case),
        None,
    )
}

const ACTIVATION_OWNERS: [u8; 2] = [1, 2];
const ACTIVATION_NEEDLE: &str = "activationneedle";

fn activation_updates(
    paths: [&str; 2],
    expected: [Option<ObjectRef>; 2],
) -> [TextOwnerUpdate<'_>; 2] {
    std::array::from_fn(|index| TextOwnerUpdate {
        owner: ACTIVATION_OWNERS[index],
        expected: expected[index],
        path: paths[index],
        text: ACTIVATION_NEEDLE,
    })
}

fn require_complete_activation(
    result: &NativeTextResult,
    view: ViewRef,
    paths: [&str; 2],
    sources: [ObjectRef; 2],
) -> TestResult {
    require_equal(&result.view, &view, "activation view")?;
    require_equal(
        &(result.hits.len(), result.matched),
        &(2, 2),
        "activation membership",
    )?;
    for ((owner, path), source) in ACTIVATION_OWNERS.into_iter().zip(paths).zip(sources) {
        require(
            result.hits.iter().any(|hit| {
                hit.owner.stable_id == StableOwnerId([owner; 16])
                    && hit.path.as_str() == path
                    && hit.source == source
            }),
            format!("activation view omitted or mixed the expected {path} Source"),
        )?;
    }
    Ok(())
}

#[test]
fn activation_readers_see_complete_views_and_reopen_the_published_head() -> TestResult {
    let old_paths = ["old-a.rs", "old-b.rs"];
    let next_paths = ["new-a.rs", "new-b.rs"];
    let fixture = ReleaseProcess::new()?;
    let first = fixture.start()?;
    let client = fixture.client();
    let configured = configure_text(&client)?;
    let (initial, old_sources) = stage_text_update(
        &client,
        configured,
        1,
        activation_updates(old_paths, [None; 2]),
    )?;
    let old = published(&client.submit(initial, &budget())?)?;
    let (candidate, next_sources) = stage_text_update(
        &client,
        old,
        2,
        activation_updates(next_paths, old_sources.map(Some)),
    )?;
    let request = query(ACTIVATION_NEEDLE, 4);
    let (ready, before_publish) = mpsc::channel();
    let (new, seen) = thread::scope(|scope| -> TestResult<_> {
        let reader = fixture.client();
        let request = request.clone();
        let observed = scope.spawn(move || -> Result<NativeTextResult, String> {
            let limit = Instant::now() + Duration::from_secs(10);
            let request_budget = RequestBudgetV1::for_duration(Duration::from_secs(10));
            let read = || {
                reader
                    .search_text(
                        latest(),
                        text_spec().instance,
                        request.clone(),
                        &request_budget,
                    )
                    .map_err(|error| error.to_string())
            };
            let initial = read()?;
            require_complete_activation(&initial, old, old_paths, old_sources)
                .map_err(|error| error.to_string())?;
            ready.send(()).map_err(|error| error.to_string())?;
            while Instant::now() < limit {
                let result = read()?;
                if result.view != old {
                    return Ok(result);
                }
                require_complete_activation(&result, old, old_paths, old_sources)
                    .map_err(|error| error.to_string())?;
                thread::sleep(NATIVE_LIFECYCLE_POLL);
            }
            Err("Latest never reached the newly published view".into())
        });
        before_publish.recv_timeout(Duration::from_secs(10))?;
        let new = published(&client.submit(candidate, &budget())?)?;
        let seen = observed
            .join()
            .map_err(|_| "Native activation reader panicked")??;
        Ok((new, seen))
    })?;
    require_complete_activation(&seen, new, next_paths, next_sources)?;
    require(new != old, "publication did not advance the Native head")?;
    let stale = operation(4);
    let ingress = client.begin_input(stale, &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![text_spec()],
        },
        &budget(),
    )?;
    require_equal(
        &client.submit(
            UpdateRequest {
                operation: stale,
                expected_view: old,
                kind: UpdateKind::Reconfigure { profile },
            },
            &budget(),
        )?,
        &SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict)),
        "stale publication compare-and-swap",
    )?;
    let first_elf = read_view_daemon_witness::observe(first.0.id())?;
    first.stop()?;

    let reopened = fixture.start()?;
    require_equal(
        &client.search_text(latest(), text_spec().instance, request.clone(), &budget())?,
        &seen,
        "durable active head and rank after restart",
    )?;
    require_complete_activation(
        &client.search_text(
            ViewSelection::Exact(old),
            text_spec().instance,
            request,
            &budget(),
        )?,
        old,
        old_paths,
        old_sources,
    )?;
    let second_elf = read_view_daemon_witness::observe(reopened.0.id())?;
    reopened.stop()?;
    record_launches(
        [first_elf, second_elf],
        "p03-native-activation.json",
        "activation_readers_see_complete_views_and_reopen_the_published_head",
    )
}

#[test]
fn retained_text_keysets_preserve_order_and_refuse_a_different_view_cursor() -> TestResult {
    let fixture = ReleaseProcess::new()?;
    let daemon = fixture.start()?;
    let client = fixture.client();
    let configured = configure_text(&client)?;
    let (one, first_source) = publish_text(&client, configured, 1, 1, None, "a.rs", "pageanchor")?;
    let (two, second_source) = publish_text(&client, one, 2, 3, None, "b.rs", "pageanchor")?;
    let (held, third_source) = publish_text(&client, two, 3, 4, None, "c.rs", "pageanchor")?;
    let session = client.open_read_session(ViewSelection::Exact(held), [63; 32], &budget())?;
    let instance = text_spec().instance;
    let request = query("pageanchor", 1);
    let first = client.search_text_page_in_read_session(
        &session,
        instance,
        request.clone(),
        None,
        &budget(),
    )?;
    let first_anchor = first.after.ok_or("first ranked page omitted its cursor")?;

    let (head, _) = publish_text(&client, held, 4, 2, None, "added.rs", "pageanchor")?;
    let second = client.search_text_page_in_read_session(
        &session,
        instance,
        request.clone(),
        Some(first_anchor),
        &budget(),
    )?;
    let second_anchor = second
        .after
        .ok_or("second ranked page omitted its cursor")?;
    let third = client.search_text_page_in_read_session(
        &session,
        instance,
        request.clone(),
        Some(second_anchor),
        &budget(),
    )?;
    for (page, remaining, owner, path, source) in [
        (&first, 3, 1, "a.rs", first_source),
        (&second, 2, 3, "b.rs", second_source),
        (&third, 1, 4, "c.rs", third_source),
    ] {
        require_equal(
            &(page.view, page.matched, page.remaining),
            &(held, 3, remaining),
            "retained keyset totals and view",
        )?;
        let [hit] = page.hits.as_slice() else {
            return Err("retained keyset returned an unexpected page size".into());
        };
        require_equal(
            &(hit.owner.stable_id, hit.path.as_str(), hit.source),
            &(StableOwnerId([owner; 16]), path, source),
            "expected immutable rank and source",
        )?;
    }
    require_equal(&third.after, &None, "last ranked page cursor")?;

    let fresh = client.open_read_session(latest(), [64; 32], &budget())?;
    require(
        matches!(
            client.request(
                NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchTextPage {
                    session: fresh.session, instance, query: request, after: Some(first_anchor),
                }),
                &budget(),
            ),
            Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::InvalidInput
        ),
        "a different view accepted the retained cursor",
    )?;
    let current = client.search_text_page_in_read_session(
        &fresh,
        instance,
        query("pageanchor", 4),
        None,
        &budget(),
    )?;
    let current_paths: Vec<_> = current.hits.iter().map(|hit| hit.path.as_str()).collect();
    require_equal(
        &(current.view, current.matched, current_paths.as_slice()),
        &(head, 4, ["a.rs", "added.rs", "b.rs", "c.rs"].as_slice()),
        "fresh keyset after publication",
    )?;
    let _released = client.release_read_session(&session, &budget())?;
    let _released = client.release_read_session(&fresh, &budget())?;
    let elf = read_view_daemon_witness::observe(daemon.0.id())?;
    daemon.stop()?;
    let projection = |kind| -> TestResult<_> {
        let fixture = ReleaseProcess::with_config(|root, socket| {
            Ok(
                quanta_index_searchd_harness::native_index::native_dsl_history_config(
                    root, socket,
                )?,
            )
        })?;
        let daemon = fixture.start()?;
        quanta_index_searchd_harness::native_index::verify_native_dsl_retained_keyset(
            &fixture.client(),
            kind,
        )?;
        let executable = read_view_daemon_witness::observe(daemon.0.id())?;
        daemon.stop()?;
        Ok(executable)
    };
    let structural = projection(NativeDslKindV1::Structural)?;
    let runtime = projection(NativeDslKindV1::RuntimeMetadata)?;
    record_launches(
        [elf, structural, runtime],
        "p05-native-query-keysets.json",
        "retained_text_keysets_preserve_order_and_refuse_a_different_view_cursor",
    )
}

#[test]
fn sdk_query_reply_stays_bound_to_the_configured_view_and_instance() -> TestResult {
    let fixture = ReleaseProcess::new()?;
    let daemon = fixture.start()?;
    let client = fixture.client();
    let configured = configure_text(&client)?;
    let (held, source) = publish_text(&client, configured, 1, 1, None, "held.rs", "bindingneedle")?;
    let (new, _) = publish_text(
        &client,
        held,
        2,
        1,
        Some(source),
        "head.rs",
        "bindingneedle",
    )?;
    let sdk = QuantaIndex::connect_query_only(ConnectOptions::from_native_socket(
        &fixture.socket,
        ViewSelection::Exact(held),
    ))?;
    require(
        matches!(
            sdk.producer().native(),
            Err(SdkError::PlaneUnavailable {
                plane: "native-write"
            })
        ),
        "query-only SDK exposed a Native write capability",
    )?;
    let reader = sdk.reader().native()?;
    require(
        matches!(
            reader.open_exact_in_scope_v1(new, [65; 32], &budget()),
            Err(NativeQueryError::ProfileBindingMismatch)
        ),
        "SDK accepted another view in an exact query scope",
    )?;
    let mut session = reader.open([65; 32], &budget())?;
    let profile = session.load_profile(&budget())?;
    require_equal(
        &profile.view,
        &client.read_view(ViewSelection::Exact(held), &budget())?,
        "authenticated SDK view",
    )?;
    let spec = text_spec();
    let mut wrong_spec = spec.clone();
    wrong_spec.schema = SchemaId([99; 16]);
    let request = query("bindingneedle", 2);
    require(
        matches!(
            session.text(&wrong_spec, request.clone(), &budget()),
            Err(NativeQueryError::InstanceMismatch)
        ),
        "SDK admitted an instance outside its captured profile",
    )?;
    let ranked = session.text(&spec, request.clone(), &budget())?;
    check_one(&ranked, held, "held.rs", source)?;
    require_equal(
        &ranked,
        &client.search_text(
            ViewSelection::Exact(held),
            spec.instance,
            request,
            &budget(),
        )?,
        "SDK result and exact daemon rank",
    )?;
    let _released = session.release(&budget())?;
    let elf = read_view_daemon_witness::observe(daemon.0.id())?;
    daemon.stop()?;
    record_launches(
        [elf],
        "p06-native-sdk-binding.json",
        "sdk_query_reply_stays_bound_to_the_configured_view_and_instance",
    )
}

#[test]
fn native_provider_enforces_approved_budget_and_cancellation() -> TestResult {
    use sha2::{Digest as _, Sha256};

    let loopback = check_loopback_provider_budget()?;
    let Some(pin_path) = std::env::var_os("QUANTA_PROOF_P07_PROVIDER_PIN") else {
        return Ok(());
    };
    let witness_directory = std::env::var_os("QUANTA_PROOF_DAEMON_WITNESS_DIR")
        .ok_or("P07 provider proof requires its scoped witness directory")?;
    let witness_directory = PathBuf::from(witness_directory).canonicalize()?;
    let pin_path = PathBuf::from(pin_path);
    require(
        cfg!(target_os = "linux")
            && pin_path.is_absolute()
            && pin_path == witness_directory.join("p07-provider-config.json"),
        "P07 requires its private config pin and Linux executable observation",
    )?;
    let mut approval = Vec::new();
    let _read = fs::File::open(pin_path)?
        .take(16_385)
        .read_to_end(&mut approval)?;
    require(
        approval.len() <= 16_384,
        "P07 provider config exceeds its bound",
    )?;
    let approval_sha256 = format!("{:x}", Sha256::digest(&approval));
    require_equal(
        &approval_sha256,
        &std::env::var("QUANTA_PROOF_P07_PROVIDER_SHA256")?,
        "P07 approved provider config",
    )?;
    let release = check_approved_provider_budget(&approval)?;
    read_view_daemon_witness::write(
        [loopback, release],
        "p07-native-provider.json",
        &nextest_test_event(
            module_path!(),
            "native_provider_enforces_approved_budget_and_cancellation",
        ),
        Some(&approval_sha256),
    )
}

fn provider_fixture_rank(
    client: &NativeUdsClient,
    semantic: InstanceSpec,
) -> TestResult<(ViewSelection, SemanticQueryRequest, NativeSemanticResult)> {
    let instance = semantic.instance;
    let configured = configure(client, vec![text_spec(), semantic])?;
    let (source_view, _) = publish_text(
        client,
        configured,
        1,
        1,
        None,
        "provider.rs",
        "sourceneedle",
    )?;
    let request = SemanticQueryRequest {
        query_text: "providerneedle".into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        lexical_scope: None,
        top_k: 1,
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    let head = loop {
        let head = client.read_view(latest(), &budget())?.identity;
        if head.sequence > source_view.sequence {
            break head;
        }
        require(
            Instant::now() < deadline,
            "Native provider fixture did not materialize",
        )?;
        thread::sleep(NATIVE_LIFECYCLE_POLL);
    };
    let selection = ViewSelection::Exact(head);
    let result = client.search_semantic(selection, instance, request.clone(), &budget())?;
    require_equal(&result.view, &head, "provider result view")?;
    require_equal(
        &result.coverage.status,
        &CoverageStatus::Complete,
        "provider coverage",
    )?;
    let [hit] = result.hits.as_slice() else {
        return Err("provider result did not contain the sole fixture owner".into());
    };
    require_equal(
        &hit.owner,
        &OwnerKey {
            producer: operation(2).producer,
            stable_id: StableOwnerId([1; 16]),
        },
        "provider result owner",
    )?;
    Ok((selection, request, result))
}

fn check_approved_provider_budget(
    approval: &[u8],
) -> TestResult<Option<read_view_daemon_witness::Observation>> {
    use quanta_index_semantic::native::{
        EmbeddingInputPolicy, EmbeddingRecipe, VectorNormalization,
    };

    let provider: serde_json::Value = serde_json::from_slice(approval)?;
    let field = |key: &str| -> TestResult<&str> {
        provider
            .get(key)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("P07 approved provider lacks {key}").into())
    };
    let url = reqwest::Url::parse(field("base_url")?)?;
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_end_matches('.')
        .trim_matches(['[', ']']);
    let local = host == "localhost"
        || host.ends_with(".localhost")
        || host.parse::<std::net::IpAddr>().is_ok_and(|address| {
            let address = address.to_canonical();
            address.is_loopback() || address.is_unspecified()
        });
    require(
        field("kind")? == "open_ai" && url.scheme() == "https" && !host.is_empty() && !local,
        "P07 requires an approved HTTPS OpenAI provider",
    )?;
    let dimension = u32::try_from(
        provider
            .get("dimension")
            .and_then(serde_json::Value::as_u64)
            .ok_or("P07 approved provider lacks dimension")?,
    )?;
    let recipe = EmbeddingRecipe::from_text_model(
        &format!("openai:{}", field("model")?),
        field("model_revision")?,
        dimension,
        VectorNormalization::L2Unit,
        EmbeddingInputPolicy::SourceUtf8,
        None,
        1,
        4096,
    )?;
    let fixture = ReleaseProcess::with_config(|root, socket| {
        let mut settings = config_with_openai_semantic_dimension(root, socket, dimension)?;
        set_fixture_json(&mut settings, "/semantic/provider", provider.clone())?;
        configure_owned_semantic(&mut settings)?;
        Ok(settings)
    })?;
    let daemon = fixture.start()?;
    let client = fixture.client();
    let semantic = semantic_fixture_spec(recipe.fingerprint());
    let instance = semantic.instance;
    let (view, mut request, _rank) = provider_fixture_rank(&client, semantic)?;
    request.query_text = "exhaustedneedle".into();
    require(
        matches!(
            client.search_semantic(view, instance, request, &budget()),
            Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::ResourceLimited
        ),
        "the approved provider accepted a third paid attempt",
    )?;
    let elf = read_view_daemon_witness::observe(daemon.0.id())?;
    daemon.stop()?;
    Ok(elf)
}

fn check_loopback_provider_budget() -> TestResult<Option<read_view_daemon_witness::Observation>> {
    let stub = EmbeddingStub::start_fixture(EmbeddingFixture::HoldCancelledQuery)?;
    let base_url = stub.base_url();
    let fixture = ReleaseProcess::with_config(|root, socket| {
        let mut settings = config_with_openai_semantic(root, socket)?;
        configure_semantic_endpoint(&mut settings, &base_url)?;
        configure_openai_fixture_budget(&mut settings, 4)?;
        configure_owned_semantic(&mut settings)?;
        set_fixture_json(
            &mut settings,
            "/semantic/read/max_query_bytes",
            serde_json::json!(32),
        )?;
        Ok(settings)
    })?;
    let daemon = Daemon::start_binary(
        &fixture.binary,
        &fixture.config,
        &fixture.socket,
        Some("loopback-fixture-key"),
        None,
    )?;
    let client = fixture.client();
    let semantic = openai_semantic_spec(&base_url)?;
    let instance = semantic.instance;
    let (view, request, expected) = provider_fixture_rank(&client, semantic)?;
    let calls = stub.observed_inputs()?;
    require_equal(
        &calls.iter().map(String::as_str).collect::<Vec<_>>(),
        &vec!["sourceneedle", "providerneedle"],
        "source and query shared the initial two provider charges",
    )?;
    let mut over_budget = request.clone();
    over_budget.query_text = "budget ".repeat(8);
    require(
        matches!(
            client.search_semantic(view, instance, over_budget, &budget()),
            Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::InvalidInput
        ),
        "Semantic query crossed its configured byte limit",
    )?;
    let pre_cancelled = budget();
    pre_cancelled.cancel_handle().cancel();
    require(
        matches!(
            client.search_semantic(view, instance, request.clone(), &pre_cancelled),
            Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::Cancelled
        ),
        "pre-cancelled Semantic query was admitted",
    )?;
    require_equal(
        &stub.observed_inputs()?,
        &calls,
        "refused requests must cost zero provider calls",
    )?;

    let mut cancelled = request.clone();
    cancelled.query_text = "cancelledneedle".into();
    let in_flight = budget();
    thread::scope(|scope| -> TestResult {
        let query = scope.spawn(|| client.search_semantic(view, instance, cancelled, &in_flight));
        let observed = (|| -> TestResult {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if stub
                    .observed_inputs()?
                    .get(calls.len())
                    .is_some_and(|input| input == "cancelledneedle")
                {
                    return Ok(());
                }
                require(
                    Instant::now() < deadline,
                    "provider did not receive the cancellable attempt",
                )?;
                thread::sleep(NATIVE_LIFECYCLE_POLL);
            }
        })();
        in_flight.cancel_handle().cancel();
        let terminal = query
            .join()
            .map_err(|_| "Native provider request panicked")?;
        observed?;
        require(
            matches!(terminal, Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::Cancelled),
            "in-flight provider request did not settle as cancelled",
        )
    })?;
    stub.wait_until_idle()?;
    let mut once = calls;
    once.push("cancelledneedle".into());
    require_equal(
        &stub.observed_inputs()?,
        &once,
        "cancelled attempt must not retry",
    )?;
    let mut resumed = request;
    resumed.query_text = "resumedneedle".into();
    require_equal(
        &client.search_semantic(view, instance, resumed.clone(), &budget())?,
        &expected,
        "next query on the same result and provider budget",
    )?;
    once.push("resumedneedle".into());
    require_equal(
        &stub.observed_inputs()?,
        &once,
        "next query acquired provider custody once",
    )?;
    resumed.query_text = "exhaustedneedle".into();
    require(
        matches!(
            client.search_semantic(view, instance, resumed, &budget()),
            Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::ResourceLimited
        ),
        "the shared Source and query budget admitted a fifth HTTP attempt",
    )?;
    require_equal(
        &stub.observed_inputs()?,
        &once,
        "cost ceiling rejected before HTTP",
    )?;
    let elf = read_view_daemon_witness::observe(daemon.0.id())?;
    daemon.stop()?;
    stub.finish()?;
    Ok(elf)
}

#[test]
fn native_signals_drain_peer_fds_and_required_maintenance_loss_stops_serving() -> TestResult {
    use std::io::ErrorKind;

    use rustix::process::Signal;

    let fixture = ReleaseProcess::new()?;
    let client = fixture.client();
    let mut daemon = fixture.start()?;
    let view = configure_text(&client)?;
    let session = client.open_read_session(ViewSelection::Exact(view), [71; 32], &budget())?;
    let mut peer = UnixStream::connect(&fixture.socket)?;
    peer.set_read_timeout(Some(Duration::from_secs(1)))?;
    let first = read_view_daemon_witness::observe(daemon.0.id())?;
    daemon.stop_after_signal(Signal::INT, None)?;
    require(
        match peer.read(&mut [0_u8; 1]) {
            Ok(0) => true,
            Err(error) if error.kind() == ErrorKind::ConnectionReset => true,
            _ => false,
        },
        "shutdown left an open connection to the old daemon",
    )?;
    require(
        !fixture.socket.exists(),
        "SIGINT left the served socket behind",
    )?;

    let mut daemon = fixture.start()?;
    require_equal(
        &client.read_view(latest(), &budget())?.identity,
        &view,
        "SIGINT left the persisted view or state-root lease unavailable",
    )?;
    require(
        match client.read_session_status(&session, &budget()) {
            Ok(status) => status.state != NativeReadSessionState::Active,
            Err(NativeUdsError::Refused(error)) => {
                error.code == NativeIndexErrorCode::SnapshotUnavailable
            }
            Err(_) => false,
        },
        "an old peer session survived the process shutdown",
    )?;
    let second = read_view_daemon_witness::observe(daemon.0.id())?;
    daemon.stop_after_signal(Signal::TERM, None)?;
    require(
        !fixture.socket.exists(),
        "SIGTERM left the served socket behind",
    )?;

    let mut daemon = fixture.start()?;
    require_equal(
        &client.read_view(latest(), &budget())?.identity,
        &view,
        "SIGTERM left the persisted view or state-root lease unavailable",
    )?;
    let third = read_view_daemon_witness::observe(daemon.0.id())?;
    let displaced = fixture.root.with_extension("displaced");
    fs::rename(&fixture.root, &displaced)?;
    let terminal = (|| -> TestResult<_> {
        let deadline = Instant::now() + NATIVE_SHUTDOWN_LIMIT;
        loop {
            if let Some(status) = daemon.0.try_wait()? {
                return Ok(status);
            }
            require(
                Instant::now() < deadline,
                "required maintenance loss left the service alive",
            )?;
            thread::sleep(NATIVE_LIFECYCLE_POLL);
        }
    })();
    fs::rename(displaced, &fixture.root)?;
    require_equal(
        &terminal?.code(),
        &Some(70),
        "required maintenance loss exit",
    )?;
    let mut diagnostics = String::new();
    let _bytes = daemon
        .stderr_reader()?
        .take(STDERR_OBSERVATION_BYTES)
        .read_to_string(&mut diagnostics)?;
    require(
        diagnostics.contains("native owned pump failed"),
        format!("terminal did not identify the required maintenance child: {diagnostics}"),
    )?;
    require(
        !fixture.socket.exists(),
        "required child failure left the served socket behind",
    )?;
    record_launches(
        [first, second, third],
        "p08-native-supervisor.json",
        "native_signals_drain_peer_fds_and_required_maintenance_loss_stops_serving",
    )
}

#[test]
fn candidate_readiness_refuses_lost_root_with_bounded_diagnostics() -> TestResult {
    let fixture = ReleaseProcess::with_config(|root, socket| Ok(config(root, socket)))?;
    let mut daemon = fixture.start()?;
    let client = fixture.client();
    let ready = || {
        client.request(
            NativeIndexRequest::ObserveCandidateInstallationV1,
            &budget(),
        )
    };
    let NativeIndexResponse::CandidateInstallationV1(expected) = ready()? else {
        return Err("Native installation observation did not return its authority".into());
    };
    let elf = read_view_daemon_witness::observe(daemon.0.id())?;
    let displaced = fixture.root.with_extension("displaced");
    fs::rename(&fixture.root, &displaced)?;
    let lost = ready();
    fs::rename(displaced, &fixture.root)?;
    let Err(NativeUdsError::Refused(error)) = lost else {
        return Err("live listener reported Native readiness without its state root".into());
    };
    require_equal(
        &error.code,
        &NativeIndexErrorCode::Storage,
        "lost Native root refusal",
    )?;
    require(
        error
            .message
            .contains("root-scoped view identity unavailable")
            && error.message.len() <= 128,
        "readiness diagnostic exposed unbounded host details",
    )?;
    require(
        daemon.0.try_wait()?.is_none(),
        "readiness check restarted or lost the daemon",
    )?;
    require_equal(
        &ready()?,
        &NativeIndexResponse::CandidateInstallationV1(expected),
        "restored root regained a different installation identity",
    )?;
    daemon.stop()?;
    record_launches(
        [elf],
        "p09-native-readiness.json",
        "candidate_readiness_refuses_lost_root_with_bounded_diagnostics",
    )
}
