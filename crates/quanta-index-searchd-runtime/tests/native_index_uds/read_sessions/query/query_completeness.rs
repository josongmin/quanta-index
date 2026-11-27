//! Native LQ query completeness (09.1d) through the public SDK and daemon.
//!
//! Expected owner sets were derived by hand from `CORPUS` and checked with
//! `rg -lw` (word terms), `rg -l` (the adjacent phrase and `/./`) and set
//! operations over the same bytes; none comes from the Native implementation.

use super::*;

const CORPUS: [(&str, &str); 5] = [
    ("src/a.rs", "alpha beta gamma\n"),
    ("src/b.rs", "alpha delta\n"),
    ("src/c.rs", "beta delta\n"),
    ("src/d.rs", "gamma epsilon\n"),
    ("src/e.rs", "beta alpha\n"),
];

fn publish_corpus<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
) -> TestResult<quanta_index_contract::index::ViewRef> {
    let configured = create_text_workspace(client)?;
    let ingress = client.begin_input(operation(2), &budget())?;
    let mut mutations = Vec::new();
    for (stable_id, (path, text)) in (1_u8..).zip(CORPUS) {
        let source = stage_text_source_in_ingress(client, ingress, path, text, None)?;
        mutations.push(OwnerMutation {
            owner: OwnerKey {
                producer: ingress.operation.producer,
                stable_id: StableOwnerId([stable_id; 16]),
            },
            expected: None,
            next: Some(source),
        });
    }
    let request = prepare_text_mutations_in_ingress(client, configured, ingress, mutations)?;
    published(&client.submit(request, &budget())?)
}

/// Native LQ queries and their owner sets, derived by hand from `CORPUS`.
const GOLDENS: [(&str, &[&str]); 10] = [
    ("alpha AND beta", &["src/a.rs", "src/e.rs"]),
    (
        "alpha OR gamma",
        &["src/a.rs", "src/b.rs", "src/d.rs", "src/e.rs"],
    ),
    ("alpha AND NOT beta", &["src/b.rs"]),
    // NOT > AND > OR: parentheses change the answer.
    (
        "beta AND (alpha OR delta)",
        &["src/a.rs", "src/c.rs", "src/e.rs"],
    ),
    (
        "beta AND alpha OR delta",
        &["src/a.rs", "src/b.rs", "src/c.rs", "src/e.rs"],
    ),
    (
        "(beta AND alpha) OR delta",
        &["src/a.rs", "src/b.rs", "src/c.rs", "src/e.rs"],
    ),
    // Adjacent, ordered tokens only.
    ("\"alpha beta\"", &["src/a.rs"]),
    ("\"beta alpha\"", &["src/e.rs"]),
    // No positive term: admitted over the finite Source universe.
    ("NOT alpha", &["src/c.rs", "src/d.rs"]),
    // A regex matching every non-empty Source: admitted and exact.
    (
        "/./",
        &["src/a.rs", "src/b.rs", "src/c.rs", "src/d.rs", "src/e.rs"],
    ),
];

fn sorted_paths(result: &quanta_index_contract::native_protocol::NativeTextResult) -> Vec<&str> {
    let mut paths = result
        .hits
        .iter()
        .map(|hit| hit.path.as_str())
        .collect::<Vec<_>>();
    paths.sort_unstable();
    paths
}

#[test]
fn boolean_precedence_phrase_and_unanchored_queries_match_independent_goldens() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(30),
    )?);
    let primary = (|| -> TestResult {
        let client = NativeUdsClient::new(socket.clone());
        let view = publish_corpus(&client)?;
        let selection = ViewSelection::Exact(view);
        let instance = native_text_spec().instance;
        for (query, expected) in GOLDENS {
            let result = client.search_text(
                selection,
                instance,
                native_text_request(query, 10),
                &budget(),
            )?;
            assert!(result.exhaustive, "{query}");
            assert_eq!(sorted_paths(&result), expected, "{query}");
            assert_eq!(result.matched, u64::try_from(expected.len())?, "{query}");
        }

        // top_k limits returned rows; matched stays the full verified count.
        let limited = client.search_text(
            selection,
            instance,
            native_text_request("alpha", 1),
            &budget(),
        )?;
        assert_eq!((limited.matched, limited.hits.len()), (3, 1));
        assert!(limited.exhaustive);

        // Unbounded and zero counts parse but have no Native plan: typed
        // refusals before I/O.
        for query in ["alpha count:all", "alpha count:0"] {
            let refused = client.search_text(
                selection,
                instance,
                native_text_request(query, 10),
                &budget(),
            );
            assert!(
                matches!(
                    &refused,
                    Err(NativeUdsError::QueryRejected {
                        code: quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                            quanta_index_contract::lex::LexicalErrorCode::UnsupportedCombo
                        ),
                        ..
                    })
                ),
                "{query}: {refused:?}"
            );
        }

        // The CLI reproduces every golden and the same typed refusals.
        let socket_text = socket.to_str().ok_or("socket path is not UTF-8")?;
        let workspace_hex = format!(
            "{}{}{}{}",
            "01".repeat(16),
            "02".repeat(16),
            "03".repeat(16),
            "04".repeat(16)
        );
        let instance_hex = "47".repeat(16);
        let cli = |query: &str| {
            let args = [
                "search-text",
                "--socket",
                socket_text,
                "--workspace",
                &workspace_hex,
                "--epoch",
                "1",
                "--instance",
                &instance_hex,
                "--query-text",
                query,
                "--top-k",
                "10",
                "--output",
                "json",
            ];
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = quanta_index_searchctl::run(args, &mut stdout, &mut stderr);
            (exit, stdout, stderr)
        };
        for (query, expected) in GOLDENS {
            let (exit, stdout, stderr) = cli(query);
            assert_eq!(
                exit,
                std::process::ExitCode::SUCCESS,
                "{query}: {}",
                String::from_utf8_lossy(&stderr)
            );
            let result: quanta_index_contract::native_protocol::NativeTextResult =
                serde_json::from_slice(&stdout)?;
            assert!(result.exhaustive, "CLI {query}");
            assert_eq!(sorted_paths(&result), expected, "CLI {query}");
        }
        for query in ["alpha count:all", "alpha count:0"] {
            let (exit, _, stderr) = cli(query);
            let stderr = String::from_utf8(stderr)?;
            // Exit 3 is the CLI's remote refusal; the message names the code.
            assert_eq!(exit, std::process::ExitCode::from(3), "{query}: {stderr}");
            assert!(stderr.contains("UNSUPPORTED_COMBO"), "{query}: {stderr}");
        }

        // An expired or cancelled request never reports an empty complete page.
        let expired = client.search_text(
            selection,
            instance,
            native_text_request("alpha", 10),
            &RequestBudgetV1::for_duration(Duration::ZERO),
        );
        assert!(expired.is_err(), "{expired:?}");
        let cancelled_budget = budget();
        cancelled_budget.cancel_handle().cancel();
        let cancelled = client.search_text(
            selection,
            instance,
            native_text_request("alpha", 10),
            &cancelled_budget,
        );
        assert!(cancelled.is_err(), "{cancelled:?}");
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped
}

#[test]
fn exhausted_work_budget_is_a_typed_refusal_not_an_empty_page() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(30),
    )?);
    let published_view = publish_corpus(&NativeUdsClient::new(socket.clone()));
    let stopped = running.stop();
    let view = published_view?;
    stopped?;

    // One map read cannot reach any posting of the selected member.
    let limited = WorkLimits {
        reads: 1,
        ..work_limits()
    };
    let reopened = NativeServiceGuard::start(read_session_service_with_retention_and_work_limits(
        root.path(),
        &socket,
        Duration::from_secs(30),
        None,
        limited,
    )?);
    // The disjunction needs postings; the wide regex needs every Source.
    let refused = ["alpha OR gamma", "/./"].map(|query| {
        (
            query,
            NativeUdsClient::new(&socket).search_text(
                ViewSelection::Exact(view),
                native_text_spec().instance,
                native_text_request(query, 10),
                &budget(),
            ),
        )
    });
    let stopped = reopened.stop();
    for (query, refused) in refused {
        assert!(
            matches!(&refused, Err(NativeUdsError::Refused(_))),
            "{query}: {refused:?}"
        );
    }
    stopped
}

/// Object reads of one Text query, and the read at which to cancel it.
#[derive(Default)]
struct ReadCounter {
    reads: AtomicU64,
    /// Cancel the request when the read count reaches this; zero never.
    cancel_at: AtomicU64,
}

impl ReadCounter {
    fn count(&self, budget: &WorkBudget) {
        let read = self.reads.fetch_add(1, Ordering::SeqCst).saturating_add(1);
        if read == self.cancel_at.load(Ordering::SeqCst) {
            budget.request().cancel_handle().cancel();
        }
    }
}

/// The Native Text reader with every object read of a query counted, so a
/// test can cancel the admitted request between any two reads.
struct CancellingTextReader {
    inner: NativeTextReader,
    counter: Arc<ReadCounter>,
}

struct CountedObjects<'a> {
    inner: &'a dyn ImmutableObjectPort,
    counter: &'a ReadCounter,
}

impl ImmutableObjectPort for CountedObjects<'_> {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.inner.identify(payload, children, budget)
    }

    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.inner.put(payload, children, budget)
    }

    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.inner.install(payload, children, sink, budget)
    }

    fn install_batch(
        &self,
        inputs: &[quanta_index_core::index::ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
        self.inner.install_batch(inputs, sink, budget)
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::StoredObject, IndexError> {
        let stored = self.inner.get(reference, budget)?;
        self.counter.count(budget);
        Ok(stored)
    }

    fn visit_utf8_leaf(
        &self,
        reference: ObjectRef,
        byte_len: u64,
        budget: &mut WorkBudget,
        visit: &mut dyn FnMut(&str, &mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        self.inner
            .visit_utf8_leaf(reference, byte_len, budget, &mut |text, budget| {
                self.counter.count(budget);
                visit(text, budget)
            })
    }
}

impl IndexQueryPort<PlannedTextRead, TextReadResult> for CancellingTextReader {
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

    fn reservation(&self, request: &PlannedTextRead) -> Result<ResourceNeed, IndexError> {
        self.inner.reservation(request)
    }

    fn query(
        &self,
        request: &PlannedTextRead,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<TextReadResult>, IndexError> {
        let objects = CountedObjects {
            inner: context.objects,
            counter: &self.counter,
        };
        let mut counted = QueryReadContext {
            ticket: context.ticket,
            instance: context.instance,
            objects: &objects,
            maps: context.maps,
            budget: &mut *context.budget,
        };
        self.inner.query(request, &mut counted)
    }
}

/// A request cancelled mid-scan never returns a partial page.
///
/// Cancelled after admission, after any object read of its scan, it is
/// refused whole or answers the whole page. Only a
/// cancellation after the scan's last checkpoint may answer, so the refused
/// cancellation points are a non-empty prefix of the reads.
#[test]
fn cancellation_mid_scan_never_returns_a_partial_page() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let counter = Arc::new(ReadCounter::default());
    let running = NativeServiceGuard::start(read_session_service_with_text_port(
        root.path(),
        &socket,
        Duration::from_secs(30),
        None,
        work_limits(),
        test_reclaim_config(),
        |inner| CancellingTextReader {
            inner,
            counter: Arc::clone(&counter),
        },
    )?);
    let primary = (|| -> TestResult {
        let client = NativeUdsClient::new(socket.clone());
        let view = publish_corpus(&client)?;
        let instance = native_text_spec().instance;
        // The wide regex reads every Source; the disjunction reads postings.
        for (query, expected) in [
            (
                "/./",
                &["src/a.rs", "src/b.rs", "src/c.rs", "src/d.rs", "src/e.rs"][..],
            ),
            (
                "alpha OR gamma",
                &["src/a.rs", "src/b.rs", "src/d.rs", "src/e.rs"],
            ),
        ] {
            let search = || {
                counter.reads.store(0, Ordering::SeqCst);
                client.search_text(
                    ViewSelection::Exact(view),
                    instance,
                    native_text_request(query, 10),
                    &budget(),
                )
            };
            counter.cancel_at.store(0, Ordering::SeqCst);
            let whole = search()?;
            assert_eq!(sorted_paths(&whole), expected, "{query}");
            let reads = counter.reads.load(Ordering::SeqCst);
            assert!(reads > 1, "{query} reads {reads} objects");
            let mut refused = Vec::new();
            for cancel_at in 1..=reads {
                counter.cancel_at.store(cancel_at, Ordering::SeqCst);
                match search() {
                    Err(NativeUdsError::Refused(_)) => refused.push(cancel_at),
                    Ok(page) => assert_eq!(page, whole, "{query} cancelled at read {cancel_at}"),
                    Err(other) => {
                        return Err(
                            format!("{query} cancelled at read {cancel_at}: {other:?}").into()
                        );
                    }
                }
            }
            let prefix = (1..=u64::try_from(refused.len())?).collect::<Vec<_>>();
            assert!(!refused.is_empty(), "{query}: no cancellation was observed");
            assert_eq!(refused, prefix, "{query}: refusals of {reads} reads");
        }
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped
}
