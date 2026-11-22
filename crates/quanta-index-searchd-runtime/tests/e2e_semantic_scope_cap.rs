//! QI-BB-004 — a semantic query's lexical scope honors `scope_top_k`.
//!
//! The contract says the scope request's `top_k` is the lexical candidate cap:
//! the semantic lane may only rank documents that the lexical lane placed in
//! its top `scope_top_k`. The dispatcher used to ignore the value and
//! materialize the scope query's full recall, so with a broad scope the
//! semantic answer could contain documents far outside the cap — and the
//! allowlist grew with the corpus.
//!
//! A pinned old-Q process publishes the original hundred files through the
//! public SDK. Oracle: the lexical route itself, asked for the same query at
//! `top_k = scope_top_k` on the same pinned generation. Whatever it ranks in
//! its top `scope_top_k` is the only set the scoped semantic answer may draw
//! from. The two routes share the lexical adapter but nothing else in the
//! path under test, so agreement is an observable property, not a mirror.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;
use std::path::Path;

use quanta_index_contract::{
    GenerationPin, ManifestGeneration, PUBLIC_TOP_K_MAX, PlannerStage, QueryConstraintSetV1,
    RepoId, RevisionId, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope,
    SearchPlaneQueryIpcResponse, SearchPlaneQueryIpcResponseEnvelope, SemanticQueryRequest,
    SemanticQueryResponse, TOP_K_OUT_OF_RANGE_CODE, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_ipc::{ClientIoPolicy, encode_request, send_request};
use quanta_index_sdk::QuantaIndex;
use quanta_index_searchd_harness::{
    legacy_reference::{LegacySearchdBinary, daemon_socket_paths, publish_legacy_source},
    private_tempdir,
};

type TestResult = Result<(), Box<dyn Error>>;

const MATCHING_FILES: u32 = 100;
const OUTER_TOP_K: u32 = 10;
const NARROW_SCOPE: u32 = 2;
const WIDE_SCOPE: u32 = MATCHING_FILES;
const QUERY: &str = "needle";

/// Publish the original file order and IDs through the fixed old-Q source
/// producer. The lexical route remains an independent ranking oracle.
fn with_published_reference(
    test: impl FnOnce(&QuantaIndex, &GenerationPin, &Path) -> TestResult,
) -> TestResult {
    let parent = private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let process = binary.start(&state_root, 8)?;
    let outcome = (|| -> TestResult {
        let client = process.connect()?;
        let repo = RepoId::new("repo-e2e")?;
        let revision = RevisionId::new("rev-e2e")?;
        let corpus = (0..MATCHING_FILES)
            .map(|index| -> Result<_, Box<dyn Error>> {
                let repeats = usize::try_from(index % 7)?.saturating_add(1);
                let needles = vec!["needle"; repeats].join(" ");
                let filler = "filler ".repeat(usize::try_from(index % 5)?.saturating_add(1));
                Ok((
                    format!("src/item_{index:03}.rs"),
                    format!("fn item_{index}() {{ {needles} {filler}}}"),
                    u64::from(index) + 1,
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let files = corpus
            .iter()
            .map(|(path, body, id)| (path.as_str(), body.as_str(), *id))
            .collect::<Vec<_>>();
        let (_head, _receipt) =
            publish_legacy_source(&client, &repo, &revision, 1, None, &files, None)?;
        let pin = GenerationPin::new(repo, revision, ManifestGeneration::new(1));
        let [query_socket, _, _] = daemon_socket_paths(&state_root);
        test(&client, &pin, &query_socket)
    })();
    let stopped = process.stop();
    let verified = binary.verify();
    outcome?;
    stopped?;
    verified
}

fn trace_details(result: &SemanticQueryResponse) -> Vec<(PlannerStage, String)> {
    result
        .explanation
        .planner_trace
        .iter()
        .map(|entry| (entry.stage, entry.detail.clone()))
        .collect()
}

/// With `scope_top_k = 2` and `top_k = 10`, the semantic answer never leaves
/// the lexical top-2, and the explanation names both cap and candidate count.
#[test]
fn scoped_semantic_results_stay_inside_the_lexical_top_scope() -> TestResult {
    with_published_reference(|client, pin, _socket| {
        let lexical_top = client
            .lexical()
            .query()
            .native(QUERY)
            .pinned(pin.clone())
            .top_k(NARROW_SCOPE)
            .execute()?;
        let allowed: BTreeSet<String> = lexical_top
            .results
            .iter()
            .map(|candidate| candidate.candidate_id.clone())
            .collect();
        if allowed.len() != usize::try_from(NARROW_SCOPE)? {
            return Err(format!(
                "lexical oracle returned {} ids for top_k={NARROW_SCOPE}; fixture is not load-bearing",
                allowed.len()
            )
            .into());
        }
        // The old fixture minted these IDs in ascending source publication
        // order, independently of the query response's BM25 ordering.
        let fixture_ids = (0..MATCHING_FILES)
            .map(|index| format!("e2e-{}-src/item_{index:03}.rs", index + 1))
            .collect::<BTreeSet<_>>();
        if !allowed.is_subset(&fixture_ids) {
            return Err(format!("lexical oracle returned an unpublished ID: {allowed:?}").into());
        }

        let scoped = client
            .semantic()
            .query()
            .text(QUERY)
            .scope_native(QUERY)
            .scope_top_k(NARROW_SCOPE)
            .pinned(pin.clone())
            .top_k(OUTER_TOP_K)
            .execute()?;
        if scoped.results.is_empty() {
            return Err("scoped semantic returned nothing; fixture proved nothing".into());
        }
        let escaped = scoped
            .results
            .iter()
            .map(|candidate| &candidate.candidate_id)
            .filter(|id| !allowed.contains(*id))
            .collect::<Vec<_>>();
        if !escaped.is_empty() {
            return Err(format!(
                "semantic results left the lexical top-{NARROW_SCOPE}: escaped={escaped:?} allowed={allowed:?}"
            )
            .into());
        }
        if scoped.results.len() > usize::try_from(NARROW_SCOPE)? {
            return Err(format!(
                "scoped semantic returned {} rows from a scope of {NARROW_SCOPE}",
                scoped.results.len()
            )
            .into());
        }

        let trace = trace_details(&scoped);
        let has_cap = trace.iter().any(|(stage, detail)| {
            *stage == PlannerStage::Plan && detail == &format!("semantic.scope.cap={NARROW_SCOPE}")
        });
        let has_count = trace.iter().any(|(stage, detail)| {
            *stage == PlannerStage::ExecFanout
                && detail == &format!("semantic.scope.text_candidates={NARROW_SCOPE}")
        });
        if !has_cap || !has_count {
            return Err(format!(
                "explanation does not name the scope cap and candidate count: {trace:?}"
            )
            .into());
        }
        Ok(())
    })
}

/// The cap is what narrows the answer: the same query with a scope wide
/// enough to admit every match fills the outer `top_k`.
#[test]
fn a_wide_scope_fills_the_outer_top_k() -> TestResult {
    with_published_reference(|client, pin, _socket| {
        let wide = client
            .semantic()
            .query()
            .text(QUERY)
            .scope_native(QUERY)
            .scope_top_k(WIDE_SCOPE)
            .pinned(pin.clone())
            .top_k(OUTER_TOP_K)
            .execute()?;
        if wide.results.len() != usize::try_from(OUTER_TOP_K)? {
            return Err(format!(
                "wide scope of {WIDE_SCOPE} over {MATCHING_FILES} matches returned {} rows, expected {OUTER_TOP_K}",
                wide.results.len()
            )
            .into());
        }
        Ok(())
    })
}

/// `scope_top_k` is a public `top_k` and shares the one gate.
///
/// Zero and above-maximum are refused under the shared code from both kinds
/// of caller (QI-BB-025): a typed client cannot encode the request, and raw
/// bytes whose *scope* `top_k` alone is out of range — the outer `top_k`
/// stays valid — are answered typed by the dispatcher. The maximum is
/// accepted.
#[test]
fn scope_top_k_shares_the_public_gate() -> TestResult {
    with_published_reference(|client, pin, query_socket| {
        let request = |scope_top_k: u32| SearchPlaneQueryIpcRequestEnvelope {
            diagnostic_attempt: quanta_index_contract::DiagnosticAttemptFieldV1::Absent,
            request_id: 7,
            payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
                query_text: QUERY.to_string(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: Some(pin.clone()),
                generation_selector: None,
                lexical_scope: Some(TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: QUERY.to_string(),
                    constraints: QueryConstraintSetV1::unconstrained(),
                    generation: Some(pin.clone()),
                    generation_selector: None,
                    top_k: scope_top_k,
                    cursor: None,
                }),
                top_k: OUTER_TOP_K,
            }),
        };
        for refused in [0, PUBLIC_TOP_K_MAX + 1, u32::MAX] {
            // The typed client: the request does not encode, under the code.
            match encode_request(&request(refused)) {
                Ok(_) => return Err(format!("scope_top_k={refused} encoded").into()),
                Err(err) if err.to_string().contains(TOP_K_OUT_OF_RANGE_CODE) => {}
                Err(err) => {
                    return Err(format!(
                        "scope_top_k={refused}: encode refusal names no code: {err}"
                    )
                    .into());
                }
            }
            // The raw caller: only the scope's `top_k` is out of range.
            let mut wire: ciborium::Value = {
                let mut buf = Vec::new();
                ciborium::into_writer(&request(1), &mut buf)?;
                ciborium::from_reader(buf.as_slice())?
            };
            if patch_scope_top_k(&mut wire, refused) != 1 {
                return Err("the wire carries exactly one scope top_k".into());
            }
            let answer = send_request::<ciborium::Value, SearchPlaneQueryIpcResponseEnvelope>(
                query_socket,
                &wire,
                ClientIoPolicy::default(),
            )?;
            let refused_typed = matches!(
                &answer.payload,
                SearchPlaneQueryIpcResponse::Error(error)
                    if error.code.as_wire_str() == TOP_K_OUT_OF_RANGE_CODE
            );
            if !refused_typed {
                return Err(format!(
                    "raw scope_top_k={refused} must be answered typed `{TOP_K_OUT_OF_RANGE_CODE}`, got {:?}",
                    answer.payload
                )
                .into());
            }
        }
        let at_max = client
            .semantic()
            .query()
            .text(QUERY)
            .scope_native(QUERY)
            .scope_top_k(PUBLIC_TOP_K_MAX)
            .pinned(pin.clone())
            .top_k(OUTER_TOP_K)
            .execute()?;
        if at_max.results.is_empty() {
            return Err("scope_top_k at the public maximum returned no candidates".into());
        }
        Ok(())
    })
}

/// Replace the `top_k` inside the `lexical_scope` map (and only there);
/// returns how many were replaced.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "ciborium::Value is #[non_exhaustive]; the leaves and any future variant carry no scope"
)]
fn patch_scope_top_k(value: &mut ciborium::Value, top_k: u32) -> usize {
    match value {
        ciborium::Value::Map(fields) => fields
            .iter_mut()
            .map(|(key, entry)| {
                if matches!(key, ciborium::Value::Text(name) if name == "lexical_scope") {
                    let ciborium::Value::Map(scope) = entry else {
                        return 0;
                    };
                    scope
                        .iter_mut()
                        .filter(|(field, _)| {
                            matches!(field, ciborium::Value::Text(name) if name == "top_k")
                        })
                        .map(|(_, slot)| {
                            *slot = ciborium::Value::Integer(top_k.into());
                            1
                        })
                        .sum()
                } else {
                    patch_scope_top_k(entry, top_k)
                }
            })
            .sum(),
        ciborium::Value::Array(items) => items
            .iter_mut()
            .map(|item| patch_scope_top_k(item, top_k))
            .sum(),
        _ => 0,
    }
}
