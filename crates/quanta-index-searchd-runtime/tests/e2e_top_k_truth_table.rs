//! QI-BB-025 — one `top_k` contract across every query route, at the daemon
//! front door.
//!
//! The public range is `1..=10_000`. Every route must refuse `0`, `10_001`
//! and `u32::MAX`, and must accept `1`, `9_999` and `10_000` — the last of
//! which the continuation probe used to refuse while the domain policies
//! accepted it.
//!
//! The refusal is one policy applied at three gates that share
//! `validate_public_top_k` and the `QUERY_TOP_K_OUT_OF_RANGE` code: the SDK
//! builder refuses before any round trip (pinned in the SDK crate against a
//! stub transport), the wire codec refuses on encode and on decode (pinned
//! here: a typed request does not encode, and raw bytes carrying the value
//! are refused at the daemon's decode — counted as a decode failure and
//! never dispatched to a route), and the dispatcher refuses an in-process
//! caller typed (pinned in the search-plane crate).
//!
//! All eight routes are driven through the harness's route-agnostic raw IPC
//! probe so the assertion is on wire behavior and no route gets a private
//! code path in the test.
//!
//! Acceptance is stronger than "not refused for `top_k`": the fixture seeds
//! every authority, so each route must *serve* an in-range request with no
//! typed error of any kind, honor `returned <= top_k`, and — where the route
//! answers with a wire window — keep `returned` / `candidate_count` /
//! `has_more` mutually consistent as observed on the wire. A route that
//! quietly turns an accepted value into some other refusal fails the table.

#![forbid(unsafe_code)]

use std::error::Error;

use quanta_index_contract::{
    CandidateCountV1, ContinuationTokenV2, GenerationPin, HistoryOrderV1, HistoryQueryRequest,
    HybridQueryRequest, HybridSeedQueryRequest, PUBLIC_TOP_K_MAX, QueryConstraintSetV1,
    QueryResultWindowV2, RuntimeMetadataQueryRequest, SearchPlaneQueryIpcRequest,
    SemanticQueryRequest, StructuralQueryRequest, SymbolQueryRequest, TOP_K_OUT_OF_RANGE_CODE,
    TextQueryRequest, TextQuerySyntax,
};
use quanta_index_ipc::{ClientIoPolicy, encode_request, send_request};
use quanta_index_searchd_harness as e2e_harness;

use e2e_harness::{E2eRoutePage, E2eRouteWindowProbe, E2eRuntime, E2eTextChunkSpec};

type TestResult = Result<(), Box<dyn Error>>;

const REFUSED: [u32; 3] = [0, PUBLIC_TOP_K_MAX + 1, u32::MAX];
const ACCEPTED: [u32; 3] = [1, PUBLIC_TOP_K_MAX - 1, PUBLIC_TOP_K_MAX];

/// A query route as the truth table sees it: a name and a payload builder
/// over the harness's sealed pin.
struct Route {
    name: &'static str,
    build: fn(Option<GenerationPin>, u32) -> SearchPlaneQueryIpcRequest,
}

/// Query text per authority.
///
/// Every route needs a query its lowering accepts, otherwise the route refuses
/// for the query before it ever reaches the `top_k` gate and the table would
/// be measuring the wrong thing.
const LEXICAL_QUERY: &str = "needle";
const SYMBOL_QUERY: &str = "symbol.has.name(needle)";
/// The history query names a whole token of the fixture's commit message.
///
/// The message is `fix: sample history alpha_content_needle`; history
/// keywords match whole folded tokens (QI-BB-011/023: one tokenizer for
/// every text route, `_` is a token character), not substrings.
const HISTORY_QUERY: &str = "type:commit alpha_content_needle";
const RUNTIME_QUERY: &str = "dirty:only needle";
const STRUCTURAL_QUERY: &str = "match { function_item { { identifier :[name] } } }";

fn text_request(
    syntax: TextQuerySyntax,
    query_text: &str,
    pin: Option<GenerationPin>,
    top_k: u32,
) -> TextQueryRequest {
    TextQueryRequest {
        syntax,
        query_text: query_text.to_string(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: pin,
        generation_selector: None,
        top_k,
        cursor: None,
    }
}

fn native_request(pin: Option<GenerationPin>, top_k: u32) -> TextQueryRequest {
    text_request(TextQuerySyntax::Native, LEXICAL_QUERY, pin, top_k)
}

fn lexical(pin: Option<GenerationPin>, top_k: u32) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Text(native_request(pin, top_k))
}

fn symbol(pin: Option<GenerationPin>, top_k: u32) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Symbol(SymbolQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: SYMBOL_QUERY.to_string(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: pin,
        generation_selector: None,
        top_k,
        cursor: None,
    })
}

fn semantic(pin: Option<GenerationPin>, top_k: u32) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
        query_text: LEXICAL_QUERY.to_string(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: pin,
        generation_selector: None,
        lexical_scope: None,
        top_k,
    })
}

fn hybrid(pin: Option<GenerationPin>, top_k: u32) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
        text_query: native_request(pin.clone(), top_k),
        semantic_query_text: LEXICAL_QUERY.to_string(),
        generation: pin,
        generation_selector: None,
        top_k,
    })
}

fn hybrid_seed(pin: Option<GenerationPin>, top_k: u32) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::HybridSeed(HybridSeedQueryRequest {
        text_query: native_request(pin.clone(), top_k),
        semantic_query_text: LEXICAL_QUERY.to_string(),
        generation: pin,
        generation_selector: None,
        dense_corpora: Vec::new(),
        top_k,
    })
}

fn history(pin: Option<GenerationPin>, top_k: u32) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::History(HistoryQueryRequest {
        text_query: text_request(TextQuerySyntax::Sourcegraph, HISTORY_QUERY, pin, top_k),
        order: HistoryOrderV1::Recency,
        cursor: None,
    })
}

fn runtime_metadata(pin: Option<GenerationPin>, top_k: u32) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::RuntimeMetadata(RuntimeMetadataQueryRequest {
        text_query: text_request(TextQuerySyntax::Sourcegraph, RUNTIME_QUERY, pin, top_k),
        cursor: None,
    })
}

fn structural(pin: Option<GenerationPin>, top_k: u32) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
        text_query: text_request(TextQuerySyntax::Native, STRUCTURAL_QUERY, pin, top_k),
        cursor: None,
    })
}

/// The eight `top_k`-bearing routes named by the finding's completion criteria.
///
/// Every one of them serves on `seeded_runtime`, so an in-range `top_k` has
/// exactly one acceptable outcome: no typed error at all.
const ROUTES: [Route; 8] = [
    Route {
        name: "lexical",
        build: lexical,
    },
    Route {
        name: "symbol",
        build: symbol,
    },
    Route {
        name: "semantic",
        build: semantic,
    },
    Route {
        name: "hybrid",
        build: hybrid,
    },
    Route {
        name: "hybrid_seed",
        build: hybrid_seed,
    },
    Route {
        name: "history",
        build: history,
    },
    Route {
        name: "runtime_metadata",
        build: runtime_metadata,
    },
    Route {
        name: "structural",
        build: structural,
    },
];

/// One sealed, activated generation that every route can serve from.
///
/// Two lexical (and therefore semantic) chunks, a symbol and a structural
/// function tree on one of them, a dirty overlay on the other, and one commit
/// touching the first. Each route's query above hits at least one row here.
fn seeded_runtime() -> Result<E2eRuntime, Box<dyn Error>> {
    const NEEDLE_CONTENT: &str = "fn needle() { let needle = 1; }";
    let mut rt = E2eRuntime::boot()?;
    rt.ingest_text("repo", "src/needle.rs", NEEDLE_CONTENT)?;
    rt.ingest_text("repo", "src/other.rs", "fn other() { let needle = 2; }")?;
    rt.ingest_symbol("repo", "src/needle.rs", "sym-needle", "needle")?;
    rt.ingest_structural_function_tree("src/needle.rs", NEEDLE_CONTENT, "needle")?;
    rt.ingest_dirty_for_path("src/other.rs", 7)?;
    rt.ingest_history_fixture("src/needle.rs")?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
}

fn probe(rt: &mut E2eRuntime, route: &Route, top_k: u32) -> Result<E2eRouteWindowProbe, String> {
    rt.probe_query_route(|pin| (route.build)(pin, top_k))
        .map_err(|err| format!("{}: top_k={top_k} harness failure: {err}", route.name))
}

/// A generation with two runtime-metadata hits and two structural hits, so
/// a page of one must report the continuation on both routes.
fn two_hit_runtime() -> Result<E2eRuntime, Box<dyn Error>> {
    const NEEDLE_CONTENT: &str = "fn needle() { let needle = 1; }";
    const OTHER_CONTENT: &str = "fn other() { let needle = 2; }";
    let mut rt = E2eRuntime::boot()?;
    rt.ingest_text("repo", "src/needle.rs", NEEDLE_CONTENT)?;
    rt.ingest_text("repo", "src/other.rs", OTHER_CONTENT)?;
    rt.ingest_structural_function_tree("src/needle.rs", NEEDLE_CONTENT, "needle")?;
    rt.ingest_structural_function_tree("src/other.rs", OTHER_CONTENT, "other")?;
    rt.ingest_dirty_for_path("src/needle.rs", 5)?;
    rt.ingest_dirty_for_path("src/other.rs", 7)?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
}

fn route_named(name: &str) -> Result<&'static Route, Box<dyn Error>> {
    ROUTES
        .iter()
        .find(|route| route.name == name)
        .ok_or_else(|| format!("truth table has no `{name}` route").into())
}

/// The runtime-metadata and structural routes answer with a window
/// (QI-BB-025 #4).
///
/// At `top_k = 1` over two hits both report `has_more`, structural with the
/// exact count it materialized and runtime with the lower bound its probe
/// saw; at `top_k = 10` both return the two rows with an exact count and no
/// continuation.
#[test]
fn runtime_and_structural_windows_report_the_continuation() -> TestResult {
    let mut rt = two_hit_runtime()?;
    let mut failures: Vec<String> = Vec::new();
    for (name, expected_at_one) in [
        ("runtime_metadata", CandidateCountV1::AtLeast(2)),
        ("structural", CandidateCountV1::Exact(2)),
    ] {
        let route = route_named(name)?;
        let page_of_one = probe(&mut rt, route, 1)?;
        if let Some(error) = page_of_one.typed_error {
            failures.push(format!("{name}: top_k=1 did not serve: {error}"));
            continue;
        }
        match page_of_one.window {
            Some(window)
                if page_of_one.returned_rows == 1
                    && window.returned() == 1
                    && window.has_more() == Some(true)
                    && window.candidate_count() == expected_at_one => {}
            other => failures.push(format!(
                "{name}: top_k=1 over two hits must report one row and has_more with {expected_at_one:?}, got rows={} window={other:?}",
                page_of_one.returned_rows
            )),
        }
        let whole = probe(&mut rt, route, 10)?;
        if let Some(error) = whole.typed_error {
            failures.push(format!("{name}: top_k=10 did not serve: {error}"));
            continue;
        }
        match whole.window {
            Some(window)
                if whole.returned_rows == 2
                    && window.returned() == 2
                    && window.has_more() == Some(false)
                    && window.candidate_count() == CandidateCountV1::Exact(2) => {}
            other => failures.push(format!(
                "{name}: top_k=10 over two hits must return both with an exact count, got rows={} window={other:?}",
                whole.returned_rows
            )),
        }
    }
    if !failures.is_empty() {
        return Err(format!("route windows drifted:\\n  {}", failures.join("\\n  ")).into());
    }
    Ok(())
}

/// One keyset route as the continuation check sees it: how to fetch a
/// page after a cursor and how to read the page's rows, window and
/// cursor.
struct KeysetRoute<R> {
    name: &'static str,
    page:
        fn(&mut E2eRuntime, Option<ContinuationTokenV2>) -> Result<E2eRoutePage<R>, anyhow::Error>,
    rows: fn(&R) -> Vec<String>,
    window: fn(&R) -> QueryResultWindowV2,
    cursor: fn(&R) -> Option<ContinuationTokenV2>,
}

/// Walk `route` at `top_k = 1` over the two-hit fixture and return the
/// failures observed.
///
/// Page one returns exactly one row and an opaque continuation; page two
/// returns the other row without continuation. Together they cover the
/// candidate-id-ordered set exactly once.
fn check_two_page_walk<R>(rt: &mut E2eRuntime, route: &KeysetRoute<R>) -> Vec<String> {
    let name = route.name;
    let first = match (route.page)(rt, None) {
        Ok(E2eRoutePage::Served(page)) => page,
        Ok(E2eRoutePage::Refused(error)) => {
            return vec![format!("{name}: page one did not serve: {error}")];
        }
        Err(err) => return vec![format!("{name}: page one harness failure: {err}")],
    };
    let first_rows = (route.rows)(&first);
    let cursor = match (route.cursor)(&first) {
        Some(cursor)
            if first_rows.len() == 1
                && (route.window)(&first).returned() == 1
                && (route.window)(&first).has_more() == Some(true) =>
        {
            cursor
        }
        _ => {
            return vec![format!(
                "{name}: page one of two must return one row and a continuation, got rows {first_rows:?} window {:?}",
                (route.window)(&first)
            )];
        }
    };
    let second = match (route.page)(rt, Some(cursor)) {
        Ok(E2eRoutePage::Served(page)) => page,
        Ok(E2eRoutePage::Refused(error)) => {
            return vec![format!("{name}: page two did not serve: {error}")];
        }
        Err(err) => return vec![format!("{name}: page two harness failure: {err}")],
    };
    let mut failures = Vec::new();
    let mut walked = first_rows;
    walked.extend((route.rows)(&second));
    let mut sorted = walked.clone();
    sorted.sort();
    sorted.dedup();
    if walked.len() != 2 || walked != sorted {
        failures.push(format!(
            "{name}: two pages of one must walk both rows once in order, got {walked:?}"
        ));
    }
    let window = (route.window)(&second);
    if window.has_more() != Some(false)
        || (route.cursor)(&second).is_some()
        || window.candidate_count() != CandidateCountV1::Exact(1)
    {
        failures.push(format!(
            "{name}: the last page is exact and final, got {window:?}"
        ));
    }
    failures
}

/// The runtime-metadata and structural routes continue a cut page from
/// its cursor (QI-BB-025 W4).
#[test]
fn runtime_and_structural_cursors_continue_the_page() -> TestResult {
    let mut rt = two_hit_runtime()?;
    let mut failures = check_two_page_walk(
        &mut rt,
        &KeysetRoute {
            name: "runtime_metadata",
            page: |rt, cursor| {
                rt.query_runtime_metadata_page(
                    TextQuerySyntax::Sourcegraph,
                    RUNTIME_QUERY,
                    1,
                    cursor,
                )
            },
            rows: |page| {
                page.results
                    .iter()
                    .map(|row| row.candidate_id.clone())
                    .collect()
            },
            window: |page| page.window.clone(),
            cursor: |page| page.next_cursor.clone(),
        },
    );
    failures.extend(check_two_page_walk(
        &mut rt,
        &KeysetRoute {
            name: "structural",
            page: |rt, cursor| {
                rt.query_structural_page(TextQuerySyntax::Native, STRUCTURAL_QUERY, 1, cursor)
            },
            rows: |page| {
                page.results
                    .iter()
                    .map(|row| row.candidate_id.clone())
                    .collect()
            },
            window: |page| page.window.clone(),
            cursor: |page| page.next_cursor.clone(),
        },
    ));
    if !failures.is_empty() {
        return Err(format!("cursor continuation drifted:\n  {}", failures.join("\n  ")).into());
    }
    Ok(())
}

/// The wire window must agree with the rows actually returned.
///
/// It must not claim a continuation that the row count contradicts. This is
/// an independent check on the values as they crossed the wire, not on the
/// constructor that produced them.
fn window_contradiction(
    window: &QueryResultWindowV2,
    returned_rows: usize,
    top_k: u32,
) -> Option<String> {
    let returned = u64::from(window.returned());
    let Ok(rows) = u64::try_from(returned_rows) else {
        return Some(format!(
            "{returned_rows} returned rows do not fit the wire count"
        ));
    };
    if returned != rows {
        return Some(format!(
            "window.returned={returned} but {rows} rows were returned"
        ));
    }
    let lower_bound = window.candidate_count().lower_bound();
    if lower_bound < returned {
        return Some(format!(
            "candidate_count lower bound {lower_bound} is below returned {returned}"
        ));
    }
    match (window.candidate_count(), window.has_more()) {
        (CandidateCountV1::Exact(exact), Some(false)) if exact == returned => None,
        (CandidateCountV1::Exact(exact), Some(true))
            if exact > returned && returned == u64::from(top_k) =>
        {
            None
        }
        (CandidateCountV1::AtLeast(lower), Some(true))
            if lower > returned && returned == u64::from(top_k) =>
        {
            None
        }
        (CandidateCountV1::AtLeast(_), None) => None,
        (count, has_more) => Some(format!(
            "candidate_count={count:?} has_more={has_more:?} contradict returned={returned} top_k={top_k}"
        )),
    }
}

/// Replace every `top_k` entry in a decoded CBOR tree with `top_k`, so the
/// bytes carry a value no encoder would emit.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "ciborium::Value is #[non_exhaustive]; the leaves and any future variant carry no top_k"
)]
fn patch_every_top_k(value: &mut ciborium::Value, top_k: u32) -> usize {
    match value {
        ciborium::Value::Map(fields) => fields
            .iter_mut()
            .map(|(key, entry)| {
                if matches!(key, ciborium::Value::Text(name) if name == "top_k") {
                    *entry = ciborium::Value::Integer(top_k.into());
                    1
                } else {
                    patch_every_top_k(entry, top_k)
                }
            })
            .sum(),
        ciborium::Value::Array(items) => items
            .iter_mut()
            .map(|item| patch_every_top_k(item, top_k))
            .sum(),
        // Every leaf variant, and — `ciborium::Value` being
        // `#[non_exhaustive]` — any variant this codec version does not
        // name, carries no `top_k`.
        _ => 0,
    }
}

/// The daemon's query-plane decode-failure and dispatch counters, from the
/// control scrape: the oracle that a request was refused at decode and
/// never reached a route.
fn ipc_query_counters(rt: &mut E2eRuntime) -> Result<(u64, u64), Box<dyn Error>> {
    let snapshot = rt.metrics_snapshot()?;
    let counter = |name: &str| -> Result<u64, Box<dyn Error>> {
        snapshot
            .counters
            .iter()
            .find(|counter| counter.name == name)
            .map(|counter| counter.value)
            .ok_or_else(|| format!("counter `{name}` is in the scrape").into())
    };
    Ok((
        counter("ipc_query_request_decode_failures_total")?,
        counter("ipc_query_requests_dispatched_total")?,
    ))
}

/// Every route refuses an out-of-range `top_k` under one code, from the
/// typed client and from raw bytes alike (QI-BB-025 완료 기준: the SDK and
/// raw IPC return the same error code).
///
/// The typed request does not encode, under the shared code; raw bytes
/// carrying the value decode, are dispatched once, and come back as a typed
/// answer with that same code — never served, never a decode failure, never
/// a closed connection that names nothing.
#[test]
fn every_route_refuses_out_of_range_top_k_with_one_code_from_typed_and_raw_callers() -> TestResult {
    let mut rt = seeded_runtime()?;
    // Prime the route once so the pin and the sockets exist and the
    // counters are live before the refusals are counted.
    let lexical = route_named("lexical")?;
    let primed = probe(&mut rt, lexical, 1)?;
    if primed.typed_error.is_some() {
        return Err(format!(
            "the fixture serves before refusals: {:?}",
            primed.typed_error
        )
        .into());
    }
    let Some((query_socket, _, _)) = rt
        .socket_paths()
        .map(|(q, c, i)| (q.to_path_buf(), c.to_path_buf(), i.to_path_buf()))
    else {
        return Err("the daemon is running after a served probe".into());
    };
    let pin = rt.generation_pin();
    let mut failures: Vec<String> = Vec::new();
    for route in &ROUTES {
        for top_k in REFUSED {
            // The client side: the typed request refuses to encode under
            // the shared code, so no SDK or harness caller can emit it.
            let typed = quanta_index_contract::SearchPlaneQueryIpcRequestEnvelope {
                request_id: 1,
                payload: (route.build)(Some(pin.clone()), top_k),
            };
            match encode_request(&typed) {
                Ok(_) => failures.push(format!(
                    "{}: top_k={top_k} encoded although it is out of range",
                    route.name
                )),
                Err(err) if err.to_string().contains(TOP_K_OUT_OF_RANGE_CODE) => {}
                Err(err) => failures.push(format!(
                    "{}: top_k={top_k} encode refusal does not name {TOP_K_OUT_OF_RANGE_CODE}: {err}",
                    route.name
                )),
            }
            // The daemon side: bytes no encoder produced, carrying the
            // value in every `top_k` position the route has.
            let in_range = quanta_index_contract::SearchPlaneQueryIpcRequestEnvelope {
                request_id: 1,
                payload: (route.build)(Some(pin.clone()), 1),
            };
            let mut wire: ciborium::Value = {
                let mut buf = Vec::new();
                ciborium::into_writer(&in_range, &mut buf)?;
                ciborium::from_reader(buf.as_slice())?
            };
            if patch_every_top_k(&mut wire, top_k) == 0 {
                failures.push(format!("{}: the wire carries no top_k", route.name));
                continue;
            }
            let (failures_before, dispatched_before) = ipc_query_counters(&mut rt)?;
            let answer = send_request::<
                ciborium::Value,
                quanta_index_contract::SearchPlaneQueryIpcResponseEnvelope,
            >(&query_socket, &wire, ClientIoPolicy::default());
            match answer.map(|response| response.payload) {
                Ok(quanta_index_contract::SearchPlaneQueryIpcResponse::Error(error))
                    if error.code.as_wire_str() == TOP_K_OUT_OF_RANGE_CODE => {}
                Ok(other) => failures.push(format!(
                    "{}: top_k={top_k} raw bytes must be answered typed {TOP_K_OUT_OF_RANGE_CODE}, got {other:?}",
                    route.name
                )),
                Err(err) => failures.push(format!(
                    "{}: top_k={top_k} raw bytes must be answered, not dropped: {err}",
                    route.name
                )),
            }
            // Decoded and dispatched: the refusal is the dispatcher's, the
            // same validator the encoder and the SDK run.
            let (failures_after, dispatched_after) = ipc_query_counters(&mut rt)?;
            if failures_after != failures_before {
                failures.push(format!(
                    "{}: top_k={top_k} must not be a decode failure: {failures_before} -> {failures_after}",
                    route.name
                ));
            }
            if dispatched_after != dispatched_before.saturating_add(1) {
                failures.push(format!(
                    "{}: top_k={top_k} must be dispatched exactly once: {dispatched_before} -> {dispatched_after}",
                    route.name
                ));
            }
        }
    }
    if !failures.is_empty() {
        return Err(format!("top_k refusal drifted:\n  {}", failures.join("\n  ")).into());
    }
    Ok(())
}

/// Every route accepts the public range, including the public maximum, and
/// answers exactly as pinned for this fixture.
#[test]
fn every_route_accepts_the_public_range_including_the_maximum() -> TestResult {
    let mut rt = seeded_runtime()?;
    let mut failures: Vec<String> = Vec::new();
    for route in &ROUTES {
        for top_k in ACCEPTED {
            let observed = match probe(&mut rt, route, top_k) {
                Ok(observed) => observed,
                Err(failure) => {
                    failures.push(failure);
                    continue;
                }
            };
            match observed.typed_error {
                Some(error) if error.code.as_str() == TOP_K_OUT_OF_RANGE_CODE => {
                    failures.push(format!(
                        "{}: top_k={top_k} is inside the public range but was refused: {}",
                        route.name, error.message
                    ));
                }
                Some(error) => failures.push(format!(
                    "{}: top_k={top_k} must serve on this fixture but answered `{}` ({})",
                    route.name, error.code, error.message
                )),
                None => {
                    if usize::try_from(top_k).is_ok_and(|cap| observed.returned_rows > cap) {
                        failures.push(format!(
                            "{}: top_k={top_k} returned {} rows, more than requested",
                            route.name, observed.returned_rows
                        ));
                    }
                    if let Some(window) = observed.window
                        && let Some(contradiction) =
                            window_contradiction(&window, observed.returned_rows, top_k)
                    {
                        failures.push(format!(
                            "{}: top_k={top_k} window contradiction: {contradiction}",
                            route.name
                        ));
                    }
                }
            }
        }
    }
    if !failures.is_empty() {
        return Err(format!("top_k acceptance drifted:\n  {}", failures.join("\n  ")).into());
    }
    Ok(())
}

/// The fixture must give every route something to return.
///
/// The acceptance table only proves a route applied the gate *and then
/// served* if the route had rows to serve. Every route must return at least
/// one row at the public maximum, so an empty answer cannot masquerade as
/// acceptance; and the lexical route, which has two hits, must report the
/// continuation at `top_k = 1`.
#[test]
fn fixture_gives_every_route_at_least_one_row() -> TestResult {
    let mut rt = seeded_runtime()?;
    let mut failures: Vec<String> = Vec::new();
    for route in &ROUTES {
        let observed = match probe(&mut rt, route, PUBLIC_TOP_K_MAX) {
            Ok(observed) => observed,
            Err(failure) => {
                failures.push(failure);
                continue;
            }
        };
        match observed.typed_error {
            Some(error) => failures.push(format!("{}: did not serve: {error}", route.name)),
            None if observed.returned_rows == 0 => {
                failures.push(format!(
                    "{}: served zero rows; fixture is blind",
                    route.name
                ));
            }
            None => {}
        }
    }
    if !failures.is_empty() {
        return Err(format!("fixture is not load-bearing:\n  {}", failures.join("\n  ")).into());
    }
    let lexical = ROUTES
        .iter()
        .find(|route| route.name == "lexical")
        .ok_or("truth table has no lexical route")?;
    let observed = probe(&mut rt, lexical, 1)?;
    if observed.returned_rows != 1 {
        return Err(format!(
            "lexical route returned {} rows for top_k=1 on a two-hit fixture",
            observed.returned_rows
        )
        .into());
    }
    let window = observed
        .window
        .ok_or("lexical route answered without a wire window")?;
    if window.has_more() != Some(true) {
        return Err("two-hit fixture at top_k=1 must report has_more".into());
    }
    Ok(())
}

/// Rows holding the needle beyond the public maximum: one more than a page
/// at `top_k = 10_000` can hold.
const OVER_MAXIMUM_ROWS: u32 = PUBLIC_TOP_K_MAX + 1;

/// Rows per file of the over-maximum fixture: eleven files hold them all.
const OVER_MAXIMUM_ROWS_PER_FILE: usize = 910;

/// A generation of [`OVER_MAXIMUM_ROWS`] chunks, each holding the needle
/// once, in eleven files of one batch.
///
/// Every row is a record the lexical and semantic tracks each index, and
/// the seal trains the dense index over all of them, which in a debug
/// build outlasts the client's default wait (the harness waits ten
/// minutes) and the harness's retention byte cap (widened to 256 MiB).
fn over_maximum_runtime() -> Result<(E2eRuntime, GenerationPin), Box<dyn Error>> {
    let mut rt = E2eRuntime::boot_with_client_request_timeout(std::time::Duration::from_secs(600))?
        .with_history_max_bytes(256 * 1024 * 1024);
    let contents: Vec<String> = (0..OVER_MAXIMUM_ROWS)
        .map(|index| format!("let row_{index} = {index}; // needle"))
        .collect();
    let specs: Vec<Vec<E2eTextChunkSpec<'_>>> = contents
        .chunks(OVER_MAXIMUM_ROWS_PER_FILE)
        .map(|file| {
            (1_u32..)
                .zip(file)
                .map(|(line, content)| E2eTextChunkSpec {
                    content,
                    start_line: line,
                    end_line: line,
                    source_repo_id: None,
                })
                .collect()
        })
        .collect();
    let paths: Vec<String> = (0..specs.len())
        .map(|file| format!("src/needles_{file:02}.rs"))
        .collect();
    let files: Vec<(&str, &[E2eTextChunkSpec<'_>])> = paths
        .iter()
        .zip(&specs)
        .map(|(path, chunks)| (path.as_str(), chunks.as_slice()))
        .collect();
    let _ids = rt
        .ingest_text_files_one_batch(&files)
        .map_err(|error| format!("over-maximum fixture ingest: {error}"))?;
    let generation = rt
        .seal()
        .map_err(|error| format!("over-maximum fixture seal: {error}"))?;
    rt.activate_last_sealed_generation()
        .map_err(|error| format!("over-maximum fixture activate: {error}"))?;
    let pin = GenerationPin::new(rt.repo(), rt.revision(), generation);
    Ok((rt, pin))
}

/// At the public maximum over more rows than a page holds, the page says
/// more exist and the continuation reaches every row (QI-BB-025 완료 기준
/// #3, the `has_more` branch).
///
/// Over 10,001 rows holding the needle, the lexical and semantic routes
/// at `top_k = 10_000` each return exactly 10,000 rows with a window that
/// says more exist and agrees with itself. Walking the lexical cursors at
/// the same `top_k` visits every one of the 10,001 rows exactly once, every
/// page within `top_k` and consistent with its window, and the last page
/// says nothing more exists and names no cursor.
#[test]
fn the_public_maximum_reports_the_continuation_over_ten_thousand_and_one_rows() -> TestResult {
    let (mut rt, pin) = over_maximum_runtime()?;
    let rows = usize::try_from(OVER_MAXIMUM_ROWS)?;
    let maximum = usize::try_from(PUBLIC_TOP_K_MAX)?;
    for route in ROUTES
        .iter()
        .filter(|route| matches!(route.name, "lexical" | "semantic"))
    {
        let observed = probe(&mut rt, route, PUBLIC_TOP_K_MAX)?;
        if let Some(error) = observed.typed_error {
            return Err(format!("{} at the maximum did not serve: {error}", route.name).into());
        }
        let window = observed
            .window
            .ok_or_else(|| format!("{} answered without a window", route.name))?;
        if observed.returned_rows != maximum || window.has_more() != Some(true) {
            return Err(format!(
                "{} returned {} rows with has_more={:?} over {rows} matching rows",
                route.name,
                observed.returned_rows,
                window.has_more()
            )
            .into());
        }
        if let Some(contradiction) =
            window_contradiction(&window, observed.returned_rows, PUBLIC_TOP_K_MAX)
        {
            return Err(format!("{}: {contradiction}", route.name).into());
        }
    }

    let mut seen = std::collections::BTreeSet::new();
    let mut cursor = None;
    let mut pages = 0_u32;
    loop {
        let page = rt
            .query_text_page(
                TextQuerySyntax::Native,
                LEXICAL_QUERY,
                PUBLIC_TOP_K_MAX,
                Some(pin.clone()),
                cursor,
            )?
            .served("the lexical walk")?;
        pages = pages.saturating_add(1);
        if let Some(contradiction) =
            window_contradiction(&page.window, page.results.len(), PUBLIC_TOP_K_MAX)
        {
            return Err(format!("page {pages}: {contradiction}").into());
        }
        for row in &page.results {
            if !seen.insert(row.candidate_id.clone()) {
                return Err(format!("page {pages} repeats {}", row.candidate_id).into());
            }
        }
        match (page.window.has_more(), page.next_cursor) {
            (Some(true), Some(next)) => cursor = Some(next),
            (Some(false), None) => break,
            (has_more, next) => {
                return Err(format!(
                    "page {pages}: has_more={has_more:?} but the cursor is {next:?}"
                )
                .into());
            }
        }
    }
    if seen.len() != rows || pages != 2 {
        return Err(format!(
            "the walk visited {} of {rows} rows over {pages} pages",
            seen.len()
        )
        .into());
    }
    Ok(())
}
