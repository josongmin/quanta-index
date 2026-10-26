//! Live UDS overlay observations reach the existing Auto signal and policy ports.

use super::*;
use std::sync::Mutex;

use quanta_index_contract::native_protocol::{
    NativeEffortDone, NativeEffortReason, NativeOverlayCorpusStamp, NativeOverlayDocument,
    NativeOverlayEffortContext, NativeOverlayReply, NativeOverlayRequest, NativeOverlayRevision,
    NativeOverlaySession, NativeOverlayTextResult,
};
use quanta_index_core::index::effort::{
    DecisionFrame, EffortError, EffortPolicy, PolicyProposal, PolicyState, StageCostEstimator,
};
use quanta_index_searchd_runtime::{
    native::{NativeAutoSignals, NativeInteractionActivityPort, NativeInteractionRecorder},
    native_effort::NativeEffortDocumentContext,
};

#[derive(Clone)]
struct DocumentObservation {
    captured_at: Instant,
    session: NativeOverlaySession,
    identity: Digest,
    revisions: Vec<NativeOverlayRevision>,
    unchanged_for: Option<Duration>,
    signals: EffortSignals,
}

struct RecordingAutoSignals {
    live: NativeAutoSignals,
    seen: Arc<Mutex<Vec<DocumentObservation>>>,
    poisoned: Arc<AtomicBool>,
}

impl NativeEffortSignalPort for RecordingAutoSignals {
    fn sample(&self, now_ms: u64) -> EffortSignals {
        self.live.sample(now_ms)
    }

    fn sample_for_document(
        &self,
        now_ms: u64,
        context: &NativeEffortDocumentContext<'_>,
    ) -> EffortSignals {
        let signals = self.live.sample_for_document(now_ms, context);
        let mut seen = match self.seen.lock() {
            Ok(seen) => seen,
            Err(poison) => {
                self.poisoned.store(true, Ordering::SeqCst);
                poison.into_inner()
            }
        };
        seen.push(DocumentObservation {
            captured_at: Instant::now(),
            session: context.session,
            identity: context.identity,
            revisions: context.revisions.to_vec(),
            unchanged_for: context.unchanged_for,
            signals,
        });
        signals
    }
}

#[derive(Clone, Copy)]
struct PolicyObservation {
    preference: EffortPreference,
    before: PolicyState,
    after: PolicyState,
}

struct RecordingPolicy {
    policy: HysteresisPolicy,
    seen: Arc<Mutex<Vec<PolicyObservation>>>,
}

impl EffortPolicy for RecordingPolicy {
    fn propose(
        &self,
        frame: &DecisionFrame<'_>,
        state: PolicyState,
        estimator: &dyn StageCostEstimator,
    ) -> Result<PolicyProposal, EffortError> {
        let proposal = self.policy.propose(frame, state, estimator)?;
        self.seen
            .lock()
            .map_err(|_poison| EffortError::InvalidProposal)?
            .push(PolicyObservation {
                preference: frame.preference,
                before: state,
                after: proposal.next_state,
            });
        Ok(proposal)
    }
}

fn captured<T: Clone>(seen: &Arc<Mutex<Vec<T>>>) -> TestResult<Vec<T>> {
    Ok(seen
        .lock()
        .map_err(|_poison| "test capture poisoned")?
        .clone())
}

fn overlay_fast(
    client: &NativeUdsClient,
    selected: ViewRef,
    session: NativeOverlaySession,
    expected_corpus: Option<Digest>,
) -> TestResult<(NativeOverlayTextResult, NativeEffortDone)> {
    let mut query = stream_query(selected);
    query.preference = NativeEffortPreference::Fast;
    query.overlay = Some(NativeOverlayEffortContext {
        session,
        expected_corpus,
    });
    let mut fast = None;
    let done = client.query_effort_stream(query, &budget(), &mut |stage| match stage {
        NativeEffortStreamStage::OverlayFast(result) if fast.is_none() => {
            fast = Some(result.clone());
            Ok(())
        }
        NativeEffortStreamStage::Fast(_)
        | NativeEffortStreamStage::Deep(_)
        | NativeEffortStreamStage::OverlayFast(_)
        | NativeEffortStreamStage::OverlayDeep(_) => Err(NativeUdsError::Protocol(
            "unexpected stage in overlay Fast query".into(),
        )),
    })?;
    Ok((fast.ok_or("overlay Fast absent")?, done))
}

fn overlay_auto(
    client: &NativeUdsClient,
    selected: ViewRef,
    session: NativeOverlaySession,
) -> TestResult<(NativeOverlayTextResult, NativeEffortDone)> {
    let mut query = stream_query(selected);
    query.preference = NativeEffortPreference::Auto;
    query.overlay = Some(NativeOverlayEffortContext {
        session,
        expected_corpus: None,
    });
    let mut fast = None;
    let mut deep_corpus = None;
    let done = client.query_effort_stream(query, &budget(), &mut |stage| match stage {
        NativeEffortStreamStage::OverlayFast(result) if fast.is_none() => {
            fast = Some(result.clone());
            Ok(())
        }
        NativeEffortStreamStage::OverlayDeep(result) if fast.is_some() && deep_corpus.is_none() => {
            deep_corpus = Some(result.corpus.clone());
            Ok(())
        }
        NativeEffortStreamStage::Fast(_)
        | NativeEffortStreamStage::Deep(_)
        | NativeEffortStreamStage::OverlayFast(_)
        | NativeEffortStreamStage::OverlayDeep(_) => Err(NativeUdsError::Protocol(
            "unexpected stage in overlay Auto query".into(),
        )),
    })?;
    let fast = fast.ok_or("overlay Auto Fast absent")?;
    assert_eq!(done.deep_delivered, deep_corpus.is_some());
    if let Some(corpus) = deep_corpus {
        assert_eq!(corpus, fast.corpus);
    }
    Ok((fast, done))
}

fn assert_document_sample(
    samples: &[DocumentObservation],
    corpus: &NativeOverlayCorpusStamp,
) -> TestResult {
    assert!(!samples.is_empty(), "document signal port was not sampled");
    for sample in samples {
        assert_eq!(sample.session, corpus.session);
        assert_eq!(sample.identity, corpus.identity);
        assert_eq!(sample.revisions, corpus.revisions);
        let unchanged = sample
            .unchanged_for
            .ok_or("live corpus update age absent")?;
        let Signal::Known(idle) = sample.signals.interaction_idle_ms else {
            return Err("recorded foreground activity did not reach Auto".into());
        };
        assert!(idle.valid_for_ms > 0);
        assert_eq!(idle.value, u64::try_from(unchanged.as_millis())?);
        assert!(matches!(
            sample.signals.quality_confidence_bps,
            Signal::Unknown(UnknownReason::Unsupported)
        ));
        assert!(matches!(
            sample.signals.provider,
            Signal::Unknown(UnknownReason::Unsupported)
        ));
        assert!(matches!(sample.signals.index_backlog, Signal::Known(_)));
        assert!(matches!(
            sample.signals.interactive_pressure_bps,
            Signal::Known(_)
        ));
        assert!(matches!(
            sample.signals.memory_headroom_bytes,
            Signal::Known(_)
        ));
    }
    Ok(())
}

#[test]
fn uds_auto_samples_authenticated_overlay_and_resets_revision_clear_window() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, semantic) = service(root.path(), &socket, Arc::clone(&calls))?;
    let recorder = Arc::new(NativeInteractionRecorder::new());
    let signals = Arc::new(Mutex::new(Vec::new()));
    let signals_poisoned = Arc::new(AtomicBool::new(false));
    let policy_states = Arc::new(Mutex::new(Vec::new()));
    let activity: Arc<dyn NativeInteractionActivityPort> = recorder.clone();
    let live = service.auto_signals(
        Some(activity),
        Duration::from_secs(1),
        Duration::from_secs(5),
    )?;
    service.configure_interaction_activity(recorder)?;
    service.configure_overlay(2, 4, 1024, 4096, TEXT_INSTANCE)?;
    service.configure_effort(NativeEffortConfiguration {
        history: Arc::new(NativeEffortObservationHistory::new()),
        policy: Arc::new(RecordingPolicy {
            policy: policy()?,
            seen: Arc::clone(&policy_states),
        }),
        estimator: Arc::new(DeclaredCostEstimator),
        signals: Arc::new(RecordingAutoSignals {
            live,
            seen: Arc::clone(&signals),
            poisoned: Arc::clone(&signals_poisoned),
        }),
    })?;
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (selected, _) = setup(&client, semantic)?;
    let setup_provider_calls = calls.load(Ordering::SeqCst);

    let publisher = client.begin_interaction_publisher(Duration::from_secs(60), &budget())?;
    client.report_interaction_activity(publisher, 1, &budget())?;
    let session = NativeOverlaySession {
        workspace: workspace(),
        publisher,
        epoch: 1,
    };
    assert_eq!(
        client.overlay(NativeOverlayRequest::Begin { session }, &budget())?,
        NativeOverlayReply::Begun(session)
    );
    let document = NativeOverlayDocument {
        session,
        owner: owner(),
    };
    assert_eq!(
        client.update_overlay_at_path(
            document,
            None,
            1,
            Some("beta".into()),
            Some("a.rs".into()),
            &budget(),
        )?,
        NativeOverlayReply::Updated {
            document,
            revision: 1,
        }
    );

    let (first, first_done) = overlay_fast(&client, selected, session, None)?;
    assert_eq!(first.view, selected);
    assert_eq!(first_done.reason, NativeEffortReason::ExplicitFast);
    assert!(!first_done.deep_delivered);
    assert_eq!(first_done.corpus.as_ref(), Some(&first.corpus));
    assert_document_sample(&captured(&signals)?, &first.corpus)?;
    let prior_policy = captured(&policy_states)?;
    assert!(
        prior_policy
            .last()
            .is_some_and(|entry| entry.after.clear_since_ms.is_some()),
        "first corpus did not retain an observable clear window"
    );

    let first_signal_count = captured(&signals)?.len();
    let first_policy_count = prior_policy.len();
    let update_started = Instant::now();
    assert_eq!(
        client.update_overlay_at_path(
            document,
            Some(1),
            2,
            Some("alpha".into()),
            Some("a.rs".into()),
            &budget(),
        )?,
        NativeOverlayReply::Updated {
            document,
            revision: 2,
        }
    );
    let (second, second_done) = overlay_auto(&client, selected, session)?;
    assert_eq!(second.view, selected);
    assert_eq!(second_done.corpus.as_ref(), Some(&second.corpus));
    assert!(!second_done.deep_delivered);
    assert_eq!(calls.load(Ordering::SeqCst), setup_provider_calls);
    assert!(matches!(
        second_done.reason,
        NativeEffortReason::Dwell | NativeEffortReason::ProviderUnavailable
    ));
    assert_ne!(first.corpus.identity, second.corpus.identity);
    assert_eq!(second.corpus.session, session);
    assert!(second.corpus.revisions.iter().any(|row| {
        row.owner == document.owner && row.revision == 2 && !row.deleted && !row.retired
    }));
    let later_signals = captured(&signals)?;
    let changed_samples = later_signals
        .get(first_signal_count..)
        .ok_or("changed corpus signal capture boundary absent")?;
    assert_document_sample(changed_samples, &second.corpus)?;
    for sample in changed_samples {
        assert!(
            sample.unchanged_for.ok_or("revision age absent")?
                <= sample.captured_at.duration_since(update_started),
            "revision inherited an older corpus update timestamp"
        );
    }
    let later_policy = captured(&policy_states)?;
    let changed = later_policy
        .get(first_policy_count)
        .ok_or("changed corpus did not reach effort policy")?;
    assert_eq!(changed.preference, EffortPreference::Auto);
    assert_eq!(changed.before.clear_since_ms, None);
    assert_eq!(changed.before.clear_valid_until_ms, None);
    if signals_poisoned.load(Ordering::SeqCst) {
        return Err("document signal capture lock was poisoned".into());
    }

    client.end_interaction_publisher(publisher, 2, &budget())?;
    running.stop()?;
    Ok(())
}
