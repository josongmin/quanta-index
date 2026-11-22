//! Native SDK observations for the fixed snippet-quality corpus.
//!
//! The shared Native fixture owns daemon custody and publication. This leaf
//! supplies only the snippet sources and queries, then checks cold replay.

use std::{fs, os::unix::fs::PermissionsExt as _, path::Path};

use anyhow::{Context as _, Result, ensure};
use quanta_index_contract::{
    QueryConstraintSetV1, TextQueryRequest, TextQuerySyntax,
    index::{CatalogView, Digest, SourceCut, SourceTextAdmission, ViewRef, ViewSelection},
    native_protocol::{NativeTextCoverage, NativeTextPreviewResult},
};
use quanta_index_sdk::native_uds::{
    BorrowedSourceInput, MAX_NATIVE_STAGE_SOURCES_INPUTS, NativeUdsClient,
};

use super::workload::{FIXED_FOREGROUND_WORK_LIMITS, NativeForegroundWorkload, NativeWorkloadMode};
use super::{
    NativeDaemon, NativeExecutable, PublicationAudit, PublicationLabel, advance_staged, budget,
    cold_source_mutations, configure_text_index, create_workspace, native_config, operation, owner,
    text_spec,
};
use crate::snippet_fixture::{SNIPPET_CORPUS, SNIPPET_QUERIES, TOP_K};

/// Product observations and the exact daemon/config/source binding that made them.
pub struct NativeSnippetObservations {
    pub rows: Vec<NativeTextPreviewResult>,
    pub published_view: ViewRef,
    pub source_cut: SourceCut,
    pub config_sha256: String,
    pub config_template_sha256: String,
    pub daemon_sha256: String,
}

fn observe_queries<P: AsRef<Path>>(
    client: &NativeUdsClient<P>,
    view: ViewRef,
    refs: &[quanta_index_contract::index::ObjectRef],
) -> Result<Vec<NativeTextPreviewResult>> {
    let mut rows = Vec::with_capacity(SNIPPET_QUERIES.len());
    for query in SNIPPET_QUERIES {
        let row = client.search_text_preview(
            ViewSelection::Exact(view),
            text_spec().instance,
            TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: query.query.into(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: None,
                generation_selector: None,
                top_k: TOP_K,
                cursor: None,
            },
            &budget(),
        )?;
        ensure!(
            row.ranking.view == view
                && row.ranking.instance == text_spec().instance
                && row.ranking.coverage == NativeTextCoverage::CoreRequired
                && row.ranking.exhaustive
                && row.ranking.hits.len() == row.previews.len()
                && u64::try_from(row.ranking.hits.len())? == row.ranking.matched,
            "Native snippet preview ranking or coverage differs for {}",
            query.id
        );
        for (hit, preview) in row.ranking.hits.iter().zip(&row.previews) {
            let index = SNIPPET_CORPUS
                .iter()
                .position(|fixture| fixture.path == hit.path)
                .with_context(|| format!("Native snippet hit has unknown path: {}", hit.path))?;
            let reference = refs
                .get(index)
                .context("Native snippet source reference missing")?;
            ensure!(
                hit.owner == owner(index)?
                    && hit.source == *reference
                    && preview.owner == hit.owner
                    && preview.source == hit.source,
                "Native snippet preview source binding differs for {}",
                query.id
            );
        }
        rows.push(row);
    }
    Ok(rows)
}

fn stop_with_result<T>(daemon: NativeDaemon, run: Result<T>) -> Result<T> {
    match (run, daemon.stop(&mut None)) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(cause), Ok(())) | (Ok(_), Err(cause)) => Err(cause),
        (Err(cause), Err(drain)) => {
            Err(cause.context(format!("Native snippet daemon drain failed: {drain:#}")))
        }
    }
}

/// Observe the four fixed snippet queries through the product Native UDS SDK.
///
/// The returned rows are the first daemon's unmodified SDK results. Each of
/// two replacement processes must recover the head and reproduce every row.
pub fn run_native_snippet_observations(daemon_bin: &Path) -> Result<NativeSnippetObservations> {
    let executable = NativeExecutable::capture(daemon_bin)?;
    ensure!(
        !SNIPPET_CORPUS.is_empty() && SNIPPET_CORPUS.len() <= MAX_NATIVE_STAGE_SOURCES_INPUTS,
        "Native snippet fixture exceeds one source staging page"
    );
    let temp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()?;
    let state = temp.path().join("state");
    let socket = temp.path().join("native.sock");
    let config = temp.path().join("native.json");
    let workload = NativeForegroundWorkload {
        limits: FIXED_FOREGROUND_WORK_LIMITS,
        mode: NativeWorkloadMode::Fixed,
    };
    let config_bytes = serde_json::to_vec(&native_config(&state, &socket, workload))?;
    fs::write(&config, &config_bytes)?;
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600))?;

    let first = NativeDaemon::start(
        &executable,
        &config,
        &socket,
        None,
        0,
        &temp.path().join("first.stderr"),
    )?;
    let first_run = (|| -> Result<_> {
        let client = NativeUdsClient::new(socket.clone());
        let mut publication = PublicationAudit::default();
        let genesis = create_workspace(&client)?;
        let configured = configure_text_index(&client, genesis, &mut publication)?;
        let ingress = client.begin_input(operation(2), &budget())?;
        let sources = SNIPPET_CORPUS
            .iter()
            .map(|fixture| BorrowedSourceInput {
                path: fixture.path,
                bytes: fixture.content.as_bytes(),
                text: SourceTextAdmission::Utf8 {
                    policy: Digest([31; 32]),
                },
                language: None,
            })
            .collect::<Vec<_>>();
        let refs = client.stage_sources_borrowed(ingress, &sources, &budget())?;
        ensure!(
            refs.len() == SNIPPET_CORPUS.len(),
            "Native snippet fixture staging omitted source references"
        );
        let view = advance_staged(
            &client,
            ingress,
            configured,
            cold_source_mutations(&refs)?,
            1,
            PublicationLabel {
                phase: "snippet_full_event_ack",
                cycle: None,
            },
            &mut publication,
        )?;
        let selected = client.read_view(ViewSelection::Exact(view), &budget())?;
        ensure!(
            selected.identity == view,
            "Native snippet selected view differs"
        );
        let rows = observe_queries(&client, view, &refs)?;
        Ok((view, selected, refs, rows))
    })();
    let (view, selected, refs, rows) = stop_with_result(first, first_run)?;

    for cycle in 1..=2 {
        let reopened = NativeDaemon::start(
            &executable,
            &config,
            &socket,
            None,
            0,
            &temp.path().join(format!("reopen-{cycle}.stderr")),
        )?;
        let replay = (|| -> Result<()> {
            let client = NativeUdsClient::new(socket.as_path());
            let latest = client.read_view(
                ViewSelection::Latest {
                    workspace: view.workspace,
                    epoch: view.epoch,
                },
                &budget(),
            )?;
            let exact = client.read_view(ViewSelection::Exact(view), &budget())?;
            ensure!(
                latest.identity == view
                    && exact.identity == view
                    && same_source_and_profile(&selected, &latest)
                    && same_source_and_profile(&selected, &exact),
                "Native snippet reopen {cycle} changed the published view or source/profile"
            );
            let replayed = observe_queries(&client, view, &refs)?;
            ensure!(
                replayed == rows,
                "Native snippet exact-view rows changed on reopen {cycle}"
            );
            Ok(())
        })();
        stop_with_result(reopened, replay)?;
    }
    ensure!(
        fs::read(&config)? == config_bytes,
        "Native snippet daemon config changed during observation"
    );
    Ok(NativeSnippetObservations {
        rows,
        published_view: view,
        source_cut: selected.source,
        config_sha256: super::hex_sha256(&config_bytes),
        config_template_sha256: super::hex_sha256(&serde_json::to_vec(&native_config(
            Path::new("/fixture-state"),
            Path::new("/fixture.sock"),
            workload,
        ))?),
        daemon_sha256: executable.sha256().to_owned(),
    })
}

fn same_source_and_profile(left: &CatalogView, right: &CatalogView) -> bool {
    left.source == right.source && left.profile == right.profile
}
