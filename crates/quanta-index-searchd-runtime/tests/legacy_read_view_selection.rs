//! Public selection after physical retirement in a pinned Legacy process.

#![forbid(unsafe_code)]

use std::error::Error;

use quanta_index_contract::{
    GenerationPin, GenerationSelector, ManifestGeneration, QueryConstraintSetV1, RepoId,
    RevisionId, SearchPlaneErrorCodeV2, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::GenerationStorageKeyV1;
use quanta_index_ipc::{ClientIoPolicy, send_request};
use quanta_index_searchd_harness::legacy_reference::{
    LegacySearchdBinary, daemon_socket_paths, publish_legacy_source,
};

#[path = "support/read_view_daemon_witness.rs"]
mod read_view_daemon_witness;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

const PATH: &str = "src/selection.rs";
const G1_BODY: &str = "fn selected_g1() {}";
const G2_BODY: &str = "fn selected_g2() {}";
const G3_BODY: &str = "fn selected_g3() {}";

/// A public resolve selects G1. Two newer generations physically retire it;
/// Pinned and stale ResolvedActive must refuse, while fresh Active serves G3.
/// The dispatcher race between selection and view acquisition has a separate
/// process test with a barrier at that exact interval.
#[test]
fn retired_selected_generation_refuses_before_open_while_fresh_active_serves_g3() -> TestResult {
    let parent = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let process = binary.start(&state_root, 2)?;
    let outcome = (|| -> TestResult<Option<read_view_daemon_witness::Observation>> {
        let client = process.connect()?;
        let repo = RepoId::new("repo-e2e")?;
        let revision = RevisionId::new("rev-e2e")?;
        let (selected, _) = publish_legacy_source(
            &client,
            &repo,
            &revision,
            1,
            None,
            &[(PATH, G1_BODY, 1)],
            None,
        )?;
        let g1 = ManifestGeneration::new(1);
        if selected.generation.lexical.manifest_generation != g1 {
            return Err("public selection did not name G1".into());
        }
        let selected_pin = GenerationPin::new(repo.clone(), revision.clone(), g1);
        let storage = GenerationStorageKeyV1::for_repo_revision(&repo, &revision);
        let root = std::fs::canonicalize(&state_root)?;
        let old_lexical = storage.generation_dir(&root.join("indexes/lexical"), g1);
        let old_semantic = storage.generation_dir(&root.join("indexes/semantic"), g1);
        if !old_lexical.is_dir() || !old_semantic.is_dir() {
            return Err("G1 physical track roots were absent before retention".into());
        }

        let (g2, _) = publish_legacy_source(
            &client,
            &repo,
            &revision,
            2,
            Some((PATH, G1_BODY, 1)),
            &[(PATH, G2_BODY, 2)],
            Some(selected.clone()),
        )?;
        let (current, _) = publish_legacy_source(
            &client,
            &repo,
            &revision,
            3,
            Some((PATH, G2_BODY, 2)),
            &[(PATH, G3_BODY, 3)],
            Some(g2),
        )?;
        if old_lexical.exists() || old_semantic.exists() {
            return Err("retention did not physically retire G1 from both tracks".into());
        }
        let g3 = ManifestGeneration::new(3);
        if current.generation.lexical.manifest_generation != g3
            || current.activation_token == selected.activation_token
        {
            return Err("G3 did not advance the active generation and token".into());
        }

        let query_socket = daemon_socket_paths(&state_root)[0].clone();
        let send = |request_id, generation, selector| -> Result<_, Box<dyn Error>> {
            let response: SearchPlaneQueryIpcResponseEnvelope = send_request(
                &query_socket,
                &SearchPlaneQueryIpcRequestEnvelope {
                    diagnostic_attempt: quanta_index_contract::DiagnosticAttemptFieldV1::Absent,
                    request_id,
                    payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                        syntax: TextQuerySyntax::Native,
                        query_text: "selected_g3".to_owned(),
                        constraints: QueryConstraintSetV1::unconstrained(),
                        generation,
                        generation_selector: selector,
                        top_k: 5,
                        cursor: None,
                    }),
                },
                ClientIoPolicy::default(),
            )?;
            if response.request_id != request_id {
                return Err("public query response request ID changed".into());
            }
            Ok(response.payload)
        };
        let stale = send(
            0x3101,
            Some(selected_pin.clone()),
            Some(GenerationSelector::Pinned(selected_pin.clone())),
        )?;
        if !matches!(
            &stale,
            SearchPlaneQueryIpcResponse::Error(error)
                if error.code == SearchPlaneErrorCodeV2::UnknownGeneration
        ) {
            return Err(format!("retired explicit G1 did not refuse unknown: {stale:?}").into());
        }
        let tokened = send(
            0x3102,
            Some(selected_pin),
            Some(GenerationSelector::ResolvedActive {
                repo_id: repo.clone(),
                revision_id: revision.clone(),
                activation_token: selected.activation_token,
            }),
        )?;
        if !matches!(
            &tokened,
            SearchPlaneQueryIpcResponse::Error(error)
                if error.code == SearchPlaneErrorCodeV2::NotReady
        ) {
            return Err(format!("stale activation token was served: {tokened:?}").into());
        }
        let active = send(
            0x3103,
            None,
            Some(GenerationSelector::Active {
                repo_id: repo,
                revision_id: revision,
            }),
        )?;
        let SearchPlaneQueryIpcResponse::Text(page) = active else {
            return Err(format!("fresh Active did not serve text: {active:?}").into());
        };
        if page.selected_active_head.as_ref() != Some(&current) {
            return Err("fresh Active response did not bind the selected G3 head and token".into());
        }
        if page.generation.manifest_generation != g3
            || page.results.len() != 1
            || page.results.iter().any(|row| {
                row.manifest_generation != g3
                    || row.candidate_id.as_str() != "e2e-3-src/selection.rs"
            })
        {
            return Err(format!("fresh Active did not bind complete G3 rows: {page:?}").into());
        }
        read_view_daemon_witness::observe(process.process_id())
    })();
    let stopped = process.stop();
    let witness = outcome?;
    stopped?;
    binary.verify()?;
    read_view_daemon_witness::write(
        [witness],
        "p04-read-view-selection.json",
        concat!(
            env!("CARGO_PKG_NAME"),
            "::process_readiness_owner_v1$",
            "legacy_read_view_selection::retired_selected_generation_refuses_before_open_while_fresh_active_serves_g3",
        ),
        None,
    )
}
