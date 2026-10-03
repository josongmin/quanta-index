use super::*;

// ---------------------------------------------------------------------------
// TH-1 adapter proofs (TOPT-06): the SDK wait adapters classify by wire
// code and fail closed. Scripted closures and a virtual clock exercise
// the real adapter without daemon startup or wall-clock waits.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ScriptedWaitTicker {
    elapsed: Cell<Duration>,
}

impl WaitTicker for ScriptedWaitTicker {
    fn now(&self) -> Duration {
        self.elapsed.get()
    }

    fn sleep(&self, duration: Duration) {
        self.elapsed
            .set(self.elapsed.get().saturating_add(duration));
    }
}

/// A scripted remote refusal behind no transport at all.
fn scripted_remote(code: SearchPlaneErrorCodeV2) -> SdkError {
    SdkError::Remote {
        code,
        message: "scripted".to_string(),
        repair: None,
    }
}

#[test]
fn sdk_wait_never_ready_script_returns_typed_timeout() {
    let ticker = ScriptedWaitTicker::default();
    let calls = AtomicU64::new(0);
    let error = wait_for_sdk_ready_with_ticker(&ticker, Duration::from_millis(25), || {
        let _prior = calls.fetch_add(1, Ordering::SeqCst);
        Err::<(), SdkError>(scripted_remote(SearchPlaneErrorCodeV2::NotReady))
    })
    .expect_err("a never-ready script fails");
    let WaitError::Timeout(timeout) = error else {
        panic!("a never-ready script times out typed, got {error:?}");
    };
    assert!(
        timeout.attempts > 1,
        "a virtual-clock NOT_READY script exercises retry"
    );
    assert_eq!(calls.load(Ordering::SeqCst), timeout.attempts);
    assert!(
        timeout.expected.contains(file!()),
        "the timeout names its wait call site: {}",
        timeout.expected
    );
    let last = timeout.last.expect("the last NOT_READY is evidence");
    assert!(
        last.contains("NotReady"),
        "the last observation names the code: {last}"
    );
}

#[test]
fn sdk_wait_non_retryable_error_returns_terminal_at_once() {
    let ticker = ScriptedWaitTicker::default();
    let calls = AtomicU64::new(0);
    let error = wait_for_sdk_ready_with_ticker(&ticker, Duration::from_millis(25), || {
        let _prior = calls.fetch_add(1, Ordering::SeqCst);
        Err::<(), SdkError>(scripted_remote(SearchPlaneErrorCodeV2::Lexical(
            LexicalErrorCode::QueryTimeout,
        )))
    })
    .expect_err("a terminal error fails");
    assert!(
        matches!(error, WaitError::Terminal(_)),
        "a non-retryable code is terminal, never retried: {error:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a terminal error returns without sleeping"
    );
}

#[test]
fn sdk_wait_not_ready_then_ready_recovers() {
    let ticker = ScriptedWaitTicker::default();
    let calls = AtomicU64::new(0);
    let value = wait_for_sdk_ready_with_ticker(&ticker, Duration::from_secs(5), || {
        let call = calls.fetch_add(1, Ordering::SeqCst).saturating_add(1);
        if call < 3 {
            Err(scripted_remote(SearchPlaneErrorCodeV2::NotReady))
        } else {
            Ok("ready")
        }
    })
    .expect("NOT_READY then ready returns the value");
    assert_eq!(value, "ready");
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}
