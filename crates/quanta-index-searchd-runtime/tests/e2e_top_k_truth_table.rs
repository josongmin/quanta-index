//! QI-BB-025 — one `top_k` contract across every query route, at the daemon
//! front door.
//!
//! The public range is `1..=10_000`. Every route must refuse `0`, `10_001`
//! and `u32::MAX` with the shared `QUERY_TOP_K_OUT_OF_RANGE` code, and must
//! accept `1`, `9_999` and `10_000` — the last of which the continuation
//! probe used to refuse while the domain policies accepted it.
//!
//! All eight routes are driven through the harness's route-agnostic raw IPC
//! probe so the assertion is on wire behavior and no route gets a private
//! code path in the test. The SDK builder's local refusal, which must carry
//! the same code, is pinned in the SDK crate's own tests against a stub
//! transport.
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
    CandidateCountV1, GenerationPin, HistoryQueryRequest, HybridQueryRequest,
    HybridSeedQueryRequest, PUBLIC_TOP_K_MAX, QueryConstraintSetV1, QueryResultWindowV1,
    RuntimeMetadataQueryRequest, SearchPlaneQueryIpcRequest, SemanticQueryRequest,
    StructuralQueryRequest, SymbolQueryRequest, TOP_K_OUT_OF_RANGE_CODE, TextQueryRequest,
    TextQuerySyntax,
};
use quanta_index_searchd_harness as e2e_harness;

use e2e_harness::{E2eRouteWindowProbe, E2eRuntime};

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
const HISTORY_QUERY: &str = "type:commit needle";
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
        cursor: None,
    })
}

fn runtime_metadata(pin: Option<GenerationPin>, top_k: u32) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::RuntimeMetadata(RuntimeMetadataQueryRequest {
        text_query: text_request(TextQuerySyntax::Sourcegraph, RUNTIME_QUERY, pin, top_k),
    })
}

fn structural(pin: Option<GenerationPin>, top_k: u32) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
        text_query: text_request(TextQuerySyntax::Native, STRUCTURAL_QUERY, pin, top_k),
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

/// The wire window must agree with the rows actually returned.
///
/// It must not claim a continuation that the row count contradicts. This is
/// an independent check on the values as they crossed the wire, not on the
/// constructor that produced them.
fn window_contradiction(
    window: QueryResultWindowV1,
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
        (CandidateCountV1::Exact(exact), false) if exact == returned => None,
        (CandidateCountV1::Exact(exact), true)
            if exact > returned && returned == u64::from(top_k) =>
        {
            None
        }
        (CandidateCountV1::AtLeast(lower), true)
            if lower > returned && returned == u64::from(top_k) =>
        {
            None
        }
        (count, has_more) => Some(format!(
            "candidate_count={count:?} has_more={has_more} contradict returned={returned} top_k={top_k}"
        )),
    }
}

/// Every route refuses an out-of-range `top_k` with the one shared code.
#[test]
fn every_route_refuses_out_of_range_top_k_with_the_shared_code() -> TestResult {
    let mut rt = seeded_runtime()?;
    let mut failures: Vec<String> = Vec::new();
    for route in &ROUTES {
        for top_k in REFUSED {
            let observed = match probe(&mut rt, route, top_k) {
                Ok(observed) => observed,
                Err(failure) => {
                    failures.push(failure);
                    continue;
                }
            };
            match observed.typed_error {
                Some(error) if error.code == TOP_K_OUT_OF_RANGE_CODE => {}
                Some(error) => failures.push(format!(
                    "{}: top_k={top_k} refused with `{}` instead of `{TOP_K_OUT_OF_RANGE_CODE}` ({})",
                    route.name, error.code, error.message
                )),
                None => failures.push(format!(
                    "{}: top_k={top_k} was not refused (returned {} rows)",
                    route.name, observed.returned_rows
                )),
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
                Some(error) if error.code == TOP_K_OUT_OF_RANGE_CODE => {
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
                            window_contradiction(window, observed.returned_rows, top_k)
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
    if !window.has_more() {
        return Err("two-hit fixture at top_k=1 must report has_more".into());
    }
    Ok(())
}
