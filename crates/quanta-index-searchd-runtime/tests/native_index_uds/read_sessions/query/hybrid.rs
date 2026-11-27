//! Real-reader Hybrid with publication paused between Text and dense execution.
//!
//! The paused provider and the threads that drive it hand off through
//! channels with no timeout: every wait ends when its peer sends, or when the
//! peer's side is dropped because that peer failed or never ran. A query that
//! returns without entering the provider takes its gate back and drops it,
//! which disconnects the entry sender and ends the waiting thread.

use super::*;
use quanta_index_contract::{index::CoverageStatus, native_protocol::NativeSemanticTraversal};
use quanta_index_core::index::WorkUsage;
use quanta_index_searchd_runtime::native::NativeReadSessionConfig;
use std::path::Path;

type ProviderGate = (&'static str, mpsc::SyncSender<()>, mpsc::Receiver<()>);
type Barrier = Arc<Mutex<Option<ProviderGate>>>;

struct InterleavedProvider {
    barrier: Barrier,
    attempts: Arc<quanta_index_embed::ProviderAttemptPool>,
}
impl TextEmbeddingProvider for InterleavedProvider {
    fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        NativeFixedTextProvider { calls: None }.embed_batch(texts)
    }
    fn embed_batch_within(
        &self,
        texts: &[&str],
        budget: &RequestBudgetV1,
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        budget.checkpoint("hybrid-test-provider")?;
        let gate = if texts == ["query"] {
            self.barrier
                .lock()
                .map_err(|error| CoreError::InvalidContract(error.to_string()))?
                .take()
        } else {
            None
        };
        let Some((phase, entered, release)) = gate else {
            return NativeFixedTextProvider { calls: None }.embed_batch_within(texts, budget);
        };
        let custody = budget.begin_provider_attempt_v1()?.ok_or_else(|| {
            CoreError::InvalidContract(format!(
                "{phase}: missing actual provider execution custody"
            ))
        })?;
        let memory = quanta_index_core::TextEmbeddingBatch::reserve_output(1, 2, budget)?;
        let (done, outcome) = mpsc::sync_channel(1);
        self.attempts.spawn_tracked_with_custody(
            "hybrid-test-provider",
            Some(custody),
            move || {
                let result = (|| {
                    entered.send(()).map_err(|error| {
                        CoreError::InvalidContract(format!("{phase}: provider entry: {error}"))
                    })?;
                    release.recv().map_err(|error| {
                        CoreError::InvalidContract(format!("{phase}: physical release: {error}"))
                    })?;
                    let vectors = NativeFixedTextProvider { calls: None }
                        .embed_batch(&["query"])?
                        .into_unscoped()?;
                    Ok(quanta_index_core::TextEmbeddingBatch::from_parts(
                        vectors, memory,
                    ))
                })();
                let _abandoned = done.send(result);
            },
        )?;
        loop {
            budget.checkpoint("hybrid-test-provider")?;
            match outcome.recv_timeout(Duration::from_millis(5)) {
                Ok(result) => {
                    budget.checkpoint("hybrid-test-provider")?;
                    return result;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(CoreError::InvalidContract(format!(
                        "{phase}: tracked provider ended without result"
                    )));
                }
            }
        }
    }
    fn model_id(&self) -> &'static str {
        "native-uds-fixed-model"
    }
    fn model_revision(&self) -> &'static str {
        "fixed-revision-1"
    }
    fn dimension(&self) -> usize {
        2
    }
    fn normalization(&self) -> EmbeddingNormalization {
        EmbeddingNormalization::L2Unit
    }
}

struct PrechargedReader<T> {
    inner: T,
    reads: u64,
}
impl<Q, R, T: IndexQueryPort<Q, R>> IndexQueryPort<Q, R> for PrechargedReader<T> {
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        self.inner.validate_instance(instance)
    }
    fn validate_searchable(
        &self,
        owner: OwnerKey,
        source: Option<ObjectRef>,
        context: &mut QueryReadContext<'_>,
    ) -> Result<(), IndexError> {
        self.inner.validate_searchable(owner, source, context)
    }
    fn reservation(&self, request: &Q) -> Result<ResourceNeed, IndexError> {
        self.inner.reservation(request)
    }
    fn query(
        &self,
        request: &Q,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<R>, IndexError> {
        context.budget.charge(WorkUsage {
            reads: self.reads,
            ..WorkUsage::default()
        })?;
        self.inner.query(request, context)
    }
}

fn hybrid_service(
    root: &Path,
    socket: &Path,
    barrier: Barrier,
    charge_reads: u64,
) -> TestResult<(
    NativeIndexService,
    InstanceSpec,
    Arc<quanta_index_embed::ProviderAttemptPool>,
)> {
    let attempts = Arc::new(quanta_index_embed::ProviderAttemptPool::new(2)?);
    let bridge = Arc::new(TextEmbeddingBridge::new(InterleavedProvider {
        barrier,
        attempts: Arc::clone(&attempts),
    })?);
    let recipe = bridge.recipe(1024)?;
    let spec = InstanceSpec {
        policy: recipe.fingerprint(),
        ..async_owner_spec()
    };
    let descriptor = AdapterDescriptor {
        kind: spec.kind,
        schema: spec.schema,
        format: spec.format,
    };
    let mut resources = read_session_resources(Duration::from_secs(30));
    resources.execution.interactive.provider_units = 1;
    resources.execution.enrichment.provider_units = 1;
    resources.execution.capacity.provider_units = 2;
    let need = resources.execution.interactive;
    let text_need = ResourceNeed {
        provider_units: 0,
        ..need
    };
    let text_recipe = TextRecipe::new(Digest([31; 32]));
    let text_spec = native_text_spec();
    let text_descriptor = AdapterDescriptor {
        kind: text_spec.kind,
        schema: text_spec.schema,
        format: text_spec.format,
    };
    let mut capabilities = reclaim_capabilities(None)?;
    capabilities.provider_attempt_pool = Some(Arc::clone(&attempts));
    capabilities.builders.register_owner(
        descriptor,
        NativeSemantic::new(Arc::clone(&bridge), recipe.clone())?,
    )?;
    capabilities.builders.register(
        text_descriptor,
        NativeTextIndex::new(text_recipe),
        |delta| Ok(delta.clone()),
    )?;
    capabilities.text.register(
        text_descriptor,
        text_need,
        PrechargedReader {
            inner: NativeTextReader::new(text_recipe, Bm25Policy { k1: 1.2, b: 0.75 }, text_need)?,
            reads: charge_reads,
        },
    )?;
    capabilities.semantic.register(
        descriptor,
        need,
        PrechargedReader {
            inner: NativeSemanticReader::new(
                bridge,
                recipe,
                NativeSemanticReadConfig {
                    page_size: 1,
                    max_owners: 16,
                    max_query_bytes: 1024,
                    need,
                },
            )?,
            reads: charge_reads,
        },
    )?;
    let service = NativeIndexService::bind_with_retention(
        root,
        socket,
        work_limits(),
        capabilities,
        &resources,
        Some(test_reclaim_config()),
        None,
    )?;
    service.configure_read_sessions(NativeReadSessionConfig::default())?;
    Ok((service, spec, attempts))
}

fn configure(
    client: &NativeUdsClient,
    spec: InstanceSpec,
) -> TestResult<quanta_index_contract::index::ViewRef> {
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("Hybrid genesis differs".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![native_text_spec(), spec],
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

fn publish_initial(
    client: &NativeUdsClient,
    configured: quanta_index_contract::index::ViewRef,
) -> TestResult<(quanta_index_contract::index::ViewRef, [ObjectRef; 2])> {
    let ingress = client.begin_input(operation(2), &budget())?;
    let sources = client.stage_sources(
        ingress,
        vec![
            SourceInput {
                path: "alpha.rs".into(),
                bytes: b"alpha".to_vec(),
                text: SourceTextAdmission::Utf8 {
                    policy: Digest([31; 32]),
                },
                language: None,
            },
            SourceInput {
                path: "beta.rs".into(),
                bytes: b"beta".to_vec(),
                text: SourceTextAdmission::Excluded,
                language: None,
            },
        ],
        &budget(),
    )?;
    let [alpha_source, beta_source] = sources.as_slice() else {
        return Err("Hybrid staged source cardinality differs".into());
    };
    let source_refs = [*alpha_source, *beta_source];
    let prior = client.read_view(ViewSelection::Exact(configured), &budget())?;
    let owners = client.stage_mutations(
        ingress,
        [1, 2]
            .into_iter()
            .zip(source_refs.iter().copied())
            .map(|(id, source)| OwnerMutation {
                owner: owner(id),
                expected: None,
                next: Some(source),
            }),
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: prior.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation(2).producer,
        stream: operation(2).stream,
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    let view = published(&client.submit(
        UpdateRequest {
            operation: operation(2),
            expected_view: configured,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)?;
    Ok((view, source_refs))
}

fn materialize(
    client: &NativeUdsClient,
    spec: IndexInstanceId,
    mut view: quanta_index_contract::index::ViewRef,
) -> TestResult<quanta_index_contract::index::ViewRef> {
    let NativeIndexResponse::ReadyWork(work) = client.request(
        NativeIndexRequest::ReadyWork {
            workspace: workspace(),
            epoch: 1,
            limit: 16,
        },
        &budget(),
    )?
    else {
        return Err("Hybrid ready work differs".into());
    };
    let work = work
        .into_iter()
        .filter(|intent| intent.instance == spec)
        .collect::<Vec<_>>();
    assert_eq!(work.len(), 2);
    for (index, intent) in work.into_iter().enumerate() {
        let NativeIndexResponse::Claimed(claim) = client.request(
            NativeIndexRequest::ClaimWork {
                intent,
                lease_millis: 60_000,
            },
            &budget(),
        )?
        else {
            return Err("Hybrid claim differs".into());
        };
        let key = OperationKey {
            producer: ProducerId([91; 16]),
            stream: StreamId([92; 16]),
            sequence: u64::try_from(index)? + 1,
            ..operation(1)
        };
        view = published(&client.submit(
            UpdateRequest {
                operation: key,
                expected_view: view,
                kind: UpdateKind::Materialize { claim },
            },
            &budget(),
        )?)?;
    }
    Ok(view)
}

#[test]
fn hybrid_dense_only_original_view_partial_coverage_and_provider_refusal_keep_one_pin() -> TestResult
{
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let barrier: Barrier = Arc::new(Mutex::new(None));
    let (service, spec, attempts) = hybrid_service(root.path(), &socket, Arc::clone(&barrier), 0)?;
    let running = NativeServiceGuard::start(service);
    let primary = (|| -> TestResult {
        let client = NativeUdsClient::new(socket.clone());
        let configured = configure(&client, spec.clone())?;
        let (pending, [alpha_source, beta_source]) = publish_initial(&client, configured)?;
        let pending_view = client.read_view(ViewSelection::Exact(pending), &budget())?;
        assert_eq!(pending_view.identity, pending);
        let old = client.open_read_session(ViewSelection::Exact(pending), [61; 32], &budget())?;
        let partial = client.search_hybrid_file_in_read_session(
            &old,
            native_text_spec().instance,
            spec.instance,
            native_text_request("alpha", 2),
            semantic_request("query"),
            2,
            &budget(),
        )?;
        assert_eq!(partial.view, pending);
        assert_eq!(partial.lexical_pool.len(), 1);
        assert!(partial.dense_pool.is_empty());
        assert!(!partial.semantic_exhaustive);
        assert_ne!(partial.semantic_coverage.status, CoverageStatus::Complete);
        assert_eq!(partial.semantic_coverage.instance, spec.instance);
        assert_eq!(
            partial.semantic_coverage.desired.root,
            pending_view.source.owners
        );
        let complete = materialize(&client, spec.instance, pending)?;
        let repeated = client.search_hybrid_file_in_read_session(
            &old,
            native_text_spec().instance,
            spec.instance,
            native_text_request("alpha", 2),
            semantic_request("query"),
            2,
            &budget(),
        )?;
        assert_eq!(repeated, partial);
        let (released, _) = client.release_read_session(&old, &budget())?;
        assert_eq!(released.state, NativeReadSessionState::Released);
        let session =
            client.open_read_session(ViewSelection::Exact(complete), [62; 32], &budget())?;
        assert_eq!(view_pin_count(root.path(), complete.sequence)?, 1);
        let complete_view = client.read_view(ViewSelection::Exact(complete), &budget())?;
        assert_eq!(complete_view.identity, complete);
        let (later_update, _later_source) =
            prepare_text_owner(&client, complete, 3, 3, "later.rs", "alpha")?;
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        *barrier.lock().map_err(|error| error.to_string())? =
            Some(("publication-between-lanes", entered_tx, release_rx));
        let publish_socket = socket.clone();
        let publisher = std::thread::spawn(
            move || -> Result<quanta_index_contract::index::ViewRef, String> {
                entered_rx.recv().map_err(|error| error.to_string())?;
                let result = NativeUdsClient::new(publish_socket)
                    .submit(later_update, &budget())
                    .map_err(|error| error.to_string())
                    .and_then(|outcome| published(&outcome).map_err(|error| error.to_string()));
                let released = release_tx.send(()).map_err(|error| error.to_string());
                match (result, released) {
                    (Ok(view), Ok(())) => Ok(view),
                    (publication, release) => Err(format!(
                        "publication={publication:?}; provider-release={release:?}"
                    )),
                }
            },
        );
        let result = client.search_hybrid_file_in_read_session(
            &session,
            native_text_spec().instance,
            spec.instance,
            native_text_request("alpha", 2),
            semantic_request("query"),
            2,
            &budget(),
        );
        drop(barrier.lock().map_err(|error| error.to_string())?.take());
        let published = publisher
            .join()
            .map_err(|_panic| "Hybrid publisher panicked")?;
        let (result, latest) = match (result, published) {
            (Ok(result), Ok(latest)) => (result, latest),
            (query, publication) => {
                return Err(format!(
                    "publication-between-lanes: query={query:?}; publication={publication:?}"
                )
                .into());
            }
        };
        assert_ne!(latest, complete);
        assert_eq!(result.view, complete);
        assert_eq!(result.semantic_coverage.status, CoverageStatus::Complete);
        assert_eq!(result.semantic_coverage.instance, spec.instance);
        assert_eq!(
            result.semantic_coverage.desired.root,
            complete_view.source.owners
        );
        assert_eq!(
            result.semantic_coverage.covered,
            Some(result.semantic_coverage.desired)
        );
        assert!(result.semantic_coverage.searchable.is_some());
        assert_eq!(
            result
                .lexical_pool
                .iter()
                .map(|hit| hit.owner)
                .collect::<Vec<_>>(),
            [owner(1)]
        );
        assert_eq!(
            result
                .dense_pool
                .iter()
                .map(|hit| hit.owner)
                .collect::<Vec<_>>(),
            [owner(1), owner(2)]
        );
        for (hit, (expected_owner, expected_source, expected_path, expected_score)) in
            result.dense_pool.iter().zip([
                (owner(1), alpha_source, "alpha.rs", 1.0),
                (owner(2), beta_source, "beta.rs", 0.0),
            ])
        {
            assert_eq!(hit.owner, expected_owner);
            assert_eq!(hit.source, expected_source);
            assert_eq!(hit.path, expected_path);
            assert_eq!(hit.projection_owner, expected_owner);
            assert_eq!(hit.vector_id.as_slice(), b"whole-file");
            assert_eq!(hit.chunk, None);
            assert_eq!(hit.score.get(), expected_score);
        }
        assert_eq!(
            result
                .hits
                .iter()
                .map(|hit| (hit.owner, hit.lexical_rank, hit.dense_rank, hit.score.get()))
                .collect::<Vec<_>>(),
            [
                (owner(1), Some(1), Some(1), 2.0 / 61.0),
                (owner(2), None, Some(2), 1.0 / 62.0),
            ]
        );
        let pins = view_pin_count(root.path(), complete.sequence)?;
        assert_eq!(pins, 1);
        let constraints = QueryConstraintSetV1::from_exact_repo_relative_path(
            quanta_index_contract::ExactRepoRelativePathV1::new("beta.rs")?,
        );
        let mut filtered_text = native_text_request("alpha", 2);
        filtered_text.constraints = constraints.clone();
        let mut filtered_semantic = semantic_request("query");
        filtered_semantic.constraints = constraints;
        let filtered = client.search_hybrid_file_in_read_session(
            &session,
            native_text_spec().instance,
            spec.instance,
            filtered_text.clone(),
            filtered_semantic,
            2,
            &budget(),
        )?;
        assert_eq!(filtered.view, complete);
        assert_eq!(filtered.semantic_coverage.status, CoverageStatus::Complete);
        assert_eq!(filtered.semantic_coverage.instance, spec.instance);
        assert_eq!(
            filtered.semantic_coverage.desired.root,
            complete_view.source.owners
        );
        assert_eq!(
            filtered.semantic_coverage.covered,
            Some(filtered.semantic_coverage.desired)
        );
        assert!(filtered.semantic_coverage.searchable.is_some());
        assert_eq!(
            filtered.semantic_traversal,
            NativeSemanticTraversal::ExactExhausted
        );
        assert!(filtered.semantic_exhaustive);
        assert_eq!(filtered.skipped_stale, 0);
        assert!(filtered.lexical_pool.is_empty());
        assert_eq!(
            filtered
                .dense_pool
                .iter()
                .map(|hit| hit.owner)
                .collect::<Vec<_>>(),
            [owner(2)]
        );
        let [filtered_dense] = filtered.dense_pool.as_slice() else {
            return Err("Hybrid filtered dense pool cardinality differs".into());
        };
        assert_eq!(filtered_dense.source, beta_source);
        assert_eq!(filtered_dense.path, "beta.rs");
        assert_eq!(filtered_dense.projection_owner, owner(2));
        assert_eq!(filtered_dense.vector_id.as_slice(), b"whole-file");
        assert_eq!(filtered_dense.chunk, None);
        assert_eq!(filtered_dense.score.get(), 0.0);
        assert_eq!(
            filtered
                .hits
                .iter()
                .map(|hit| (hit.owner, hit.lexical_rank, hit.dense_rank, hit.score.get()))
                .collect::<Vec<_>>(),
            [(owner(2), None, Some(1), 1.0 / 61.0)]
        );
        let mismatched = client.request(
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchHybridFile {
                session: session.session,
                text_instance: native_text_spec().instance,
                semantic_instance: spec.instance,
                text_query: filtered_text,
                semantic_query: semantic_request("query"),
                file_top_k: 2,
            }),
            &budget(),
        );
        assert!(
            matches!(mismatched,Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::InvalidInput)
        );
        assert_eq!(view_pin_count(root.path(), complete.sequence)?, pins);
        // Bypass SDK validation: inline lexical eligibility must not leave
        // the independent dense lane with a different source universe.
        let inline = client.request(
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchHybridFile {
                session: session.session,
                text_instance: native_text_spec().instance,
                semantic_instance: spec.instance,
                text_query: native_text_request("alpha file:alpha", 2),
                semantic_query: semantic_request("query"),
                file_top_k: 2,
            }),
            &budget(),
        );
        assert!(
            matches!(inline, Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::InvalidInput),
            "server accepted an inline filter on only its lexical lane"
        );
        assert_eq!(view_pin_count(root.path(), complete.sequence)?, pins);
        let provider_error = client.search_hybrid_file_in_read_session(
            &session,
            native_text_spec().instance,
            spec.instance,
            native_text_request("alpha", 2),
            semantic_request("provider-error"),
            2,
            &budget(),
        );
        assert!(
            matches!(&provider_error, Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::Storage),
            "provider error did not return a typed storage refusal: {provider_error:?}"
        );
        assert_eq!(view_pin_count(root.path(), complete.sequence)?, pins);
        let after_error = client.search_hybrid_file_in_read_session(
            &session,
            native_text_spec().instance,
            spec.instance,
            native_text_request("alpha", 2),
            semantic_request("query"),
            2,
            &budget(),
        )?;
        // Full equality preserves the selected coverage binding on retry.
        assert_eq!(after_error, result);
        let cancelled_budget = budget();
        let cancel = cancelled_budget.cancel_handle();
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        *barrier.lock().map_err(|error| error.to_string())? =
            Some(("client-cancellation", entered_tx, release_rx));
        let interrupter = std::thread::spawn(move || -> Result<mpsc::SyncSender<()>, String> {
            entered_rx.recv().map_err(|error| error.to_string())?;
            cancel.cancel();
            Ok(release_tx)
        });
        let cancelled = client.search_hybrid_file_in_read_session(
            &session,
            native_text_spec().instance,
            spec.instance,
            native_text_request("alpha", 2),
            semantic_request("query"),
            2,
            &cancelled_budget,
        );
        drop(barrier.lock().map_err(|error| error.to_string())?.take());
        let physical_release = interrupter
            .join()
            .map_err(|_panic| "Hybrid canceller panicked")??;
        assert!(
            matches!(cancelled, Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::Cancelled)
        );
        assert!(running.resources.observe()?.reserved.provider_units > 0);
        physical_release.send(())?;
        // Query completion does not join an abandoned physical attempt.
        // Observe the same tracked pool finish before checking its grant refund.
        let join_deadline = Instant::now() + Duration::from_secs(5);
        while attempts.live_attempts() != 0 {
            if Instant::now() >= join_deadline {
                return Err("client-cancellation: physical provider did not finish".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let active = client.renew_read_session(&session, &budget())?;
        assert_eq!(active.state, NativeReadSessionState::Active);
        assert_eq!(active.view, session.view);
        assert!(active.revision > session.revision);
        assert_eq!(running.resources.observe()?.reserved.provider_units, 0);
        assert_eq!(view_pin_count(root.path(), complete.sequence)?, pins);
        let deadline_budget = RequestBudgetV1::for_duration(Duration::from_millis(200));
        // The provider is released only once the request deadline itself has
        // passed, measured from the budget, not by a fixed margin.
        let expires_at = Instant::now()
            .checked_add(deadline_budget.remaining())
            .ok_or("original-deadline instant overflow")?;
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        *barrier.lock().map_err(|error| error.to_string())? =
            Some(("original-deadline", entered_tx, release_rx));
        let delayed = std::thread::spawn(move || -> Result<(), String> {
            // A deadline that passes before the query reaches the provider
            // leaves nothing to release: the query takes its gate back and
            // the entry sender disconnects.
            let Ok(()) = entered_rx.recv() else {
                return Ok(());
            };
            std::thread::sleep(expires_at.saturating_duration_since(Instant::now()));
            release_tx.send(()).map_err(|error| error.to_string())
        });
        let expired = client.search_hybrid_file_in_read_session(
            &session,
            native_text_spec().instance,
            spec.instance,
            native_text_request("alpha", 2),
            semantic_request("query"),
            2,
            &deadline_budget,
        );
        drop(barrier.lock().map_err(|error| error.to_string())?.take());
        delayed
            .join()
            .map_err(|_panic| "Hybrid deadline gate panicked")??;
        assert!(
            matches!(&expired, Err(NativeUdsError::Transport(message)) if message.starts_with("ipc Read timed out after ") && message.ends_with(" ms")),
            "in-flight Hybrid deadline did not expire in the IPC reply read: {expired:?}"
        );
        assert!(deadline_budget.remaining().is_zero());
        let active = client.read_session_status(&session, &budget())?;
        assert_eq!(active.state, NativeReadSessionState::Active);
        assert_eq!(view_pin_count(root.path(), complete.sequence)?, pins);
        let (released, remaining) = client.release_read_session(&session, &budget())?;
        assert_eq!(released.state, NativeReadSessionState::Released);
        assert!(!remaining);
        assert_eq!(view_pin_count(root.path(), complete.sequence)?, 0);
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped?;
    Ok(())
}

#[test]
fn hybrid_spends_one_cumulative_work_budget_across_both_actual_readers() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let (service, spec, _attempts) =
        hybrid_service(root.path(), &socket, Arc::new(Mutex::new(None)), 600_000)?;
    let running = NativeServiceGuard::start(service);
    let primary = (|| -> TestResult {
        let client = NativeUdsClient::new(socket);
        let configured = configure(&client, spec.clone())?;
        let (pending, _source_refs) = publish_initial(&client, configured)?;
        let view = materialize(&client, spec.instance, pending)?;
        let session = client.open_read_session(ViewSelection::Exact(view), [63; 32], &budget())?;
        let text = client.search_text_in_read_session(
            &session,
            native_text_spec().instance,
            native_text_request("alpha", 2),
            &budget(),
        )?;
        assert_eq!(text.hits.len(), 1);
        let dense = client.search_semantic(
            ViewSelection::Exact(view),
            spec.instance,
            semantic_request("query"),
            &budget(),
        )?;
        assert_eq!(dense.hits.len(), 2);
        let pins = view_pin_count(root.path(), view.sequence)?;
        for _ in 0..2 {
            let result = client.search_hybrid_file_in_read_session(
                &session,
                native_text_spec().instance,
                spec.instance,
                native_text_request("alpha", 2),
                semantic_request("query"),
                2,
                &budget(),
            );
            assert!(
                matches!(result,Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::ResourceLimited)
            );
            assert_eq!(view_pin_count(root.path(), view.sequence)?, pins);
            assert_eq!(
                client.read_session_status(&session, &budget())?.state,
                NativeReadSessionState::Active
            );
        }
        let (released, _) = client.release_read_session(&session, &budget())?;
        assert_eq!(released.state, NativeReadSessionState::Released);
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped?;
    Ok(())
}
