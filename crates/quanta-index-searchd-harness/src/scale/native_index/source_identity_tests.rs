//! Native lifecycle and fresh rebuild for the same path in different repositories.

use std::{fs, os::unix::fs::PermissionsExt as _, path::Path};

use anyhow::{Context as _, Result, ensure};
use quanta_index_contract::RepoId;
use quanta_index_contract::index::{
    Digest, IngressRef, ObjectRef, OwnerMutation, SourceInput, SourceTextAdmission, ViewRef,
    ViewSelection,
};
use quanta_index_contract::native_protocol::{
    NativeIndexRequest, NativeIndexResponse, NativeTextCoverage, NativeTextPathRankingV1,
};
use quanta_index_sdk::native_uds::NativeUdsClient;
use tempfile::TempDir;

use super::retained_read::text_query;
use super::workload::{FIXED_FOREGROUND_WORK_LIMITS, NativeForegroundWorkload, NativeWorkloadMode};
use super::{
    NativeDaemon, NativeExecutable, PublicationAudit, PublicationLabel, advance,
    advance_frontier_only, advance_staged, budget, configure_text_index, create_workspace,
    native_config, operation, owner, text_spec, workspace,
};

const PATH: &str = "src/shared.rs";
const COMMON: &str = "qidxsharedword";
const BEFORE: &str = "qidxrepo0before";
const AFTER: &str = "qidxrepo0after";
const SOURCES: [(&str, &[u8]); 3] = [
    ("repo0", b"// qidxsharedword qidxrepo0before\n"),
    ("repo1", b"// qidxsharedword qidxrepo1stable\n"),
    (
        "repo2",
        b"// qidxsharedword qidxsharedword qidxrepo2stable\n",
    ),
];

fn stage_repository_source(
    client: &NativeUdsClient,
    ingress: IngressRef,
    repository: &str,
    bytes: &[u8],
) -> Result<ObjectRef> {
    let response = client.request(
        NativeIndexRequest::StageRepositorySources {
            ingress,
            repository: RepoId::new(repository)?,
            inputs: vec![SourceInput {
                path: PATH.into(),
                bytes: bytes.to_vec(),
                text: SourceTextAdmission::Utf8 {
                    policy: Digest([31; 32]),
                },
                language: None,
            }],
        },
        &budget(),
    )?;
    let NativeIndexResponse::StagedSources(refs) = response else {
        anyhow::bail!("unexpected repository staging response: {response:?}");
    };
    let [source] = refs.as_slice() else {
        anyhow::bail!(
            "expected one repository source ref, received {}",
            refs.len()
        );
    };
    Ok(*source)
}

fn stage_full(
    client: &NativeUdsClient,
    first: usize,
    audit: &mut PublicationAudit,
) -> Result<(ViewRef, Vec<ObjectRef>)> {
    let genesis = create_workspace(client)?;
    let configured = configure_text_index(client, genesis, audit)?;
    let ingress = client.begin_input(operation(2), &budget())?;
    let mut refs = Vec::new();
    let mut mutations = Vec::new();
    for (index, &(repository, bytes)) in SOURCES.iter().enumerate().skip(first) {
        let source = stage_repository_source(client, ingress, repository, bytes)?;
        ensure!(!refs.contains(&source), "repositories share a source ref");
        refs.push(source);
        mutations.push(OwnerMutation {
            owner: owner(index)?,
            expected: None,
            next: Some(source),
        });
    }
    let full = advance_staged(
        client,
        ingress,
        configured,
        mutations,
        1,
        PublicationLabel {
            phase: "full_event_ack",
            cycle: None,
        },
        audit,
    )?;
    Ok((full, refs))
}

fn check_query(
    client: &NativeUdsClient,
    view: ViewRef,
    token: &str,
    expected: &[(usize, ObjectRef)],
) -> Result<NativeTextPathRankingV1> {
    let result = client.search_text_paths(
        ViewSelection::Exact(view),
        text_spec().instance,
        text_query(&format!("select:path {token}"), 10),
        &budget(),
    )?;
    let ranked = result.ranked;
    let ranking = &ranked.ranking;
    ensure!(
        ranking.view == view
            && ranking.instance == text_spec().instance
            && ranking.coverage == NativeTextCoverage::CoreRequired
            && ranking.exhaustive,
        "Native path query lost its pinned view or complete coverage"
    );
    ensure!(
        ranking.matched == u64::try_from(expected.len())?
            && ranking.hits.len() == expected.len()
            && ranked.repositories.len() == expected.len(),
        "Native path query returned incorrect membership: {ranked:?}"
    );
    for &(index, source) in expected {
        let repo = SOURCES[index].0;
        let expected_owner = owner(index)?;
        ensure!(
            ranking
                .hits
                .iter()
                .zip(&ranked.repositories)
                .any(|(hit, repository)| {
                    hit.owner == expected_owner
                        && hit.source == source
                        && hit.path == PATH
                        && repository.as_str() == repo
                }),
            "Native path query omitted {repo}, its owner or its source: {ranked:?}"
        );
    }
    Ok(ranked)
}

fn with_daemon<T>(
    executable: &NativeExecutable,
    directory: &Path,
    attempt: &str,
    run: impl FnOnce(&NativeUdsClient) -> Result<T>,
) -> Result<T> {
    let socket = directory.join("native.sock");
    let config = directory.join("native.json");
    let workload = NativeForegroundWorkload {
        limits: FIXED_FOREGROUND_WORK_LIMITS,
        mode: NativeWorkloadMode::Fixed,
    };
    fs::write(
        &config,
        serde_json::to_vec(&native_config(&directory.join("state"), &socket, workload))?,
    )?;
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600))?;
    let stderr = directory.join(format!("{attempt}.stderr"));
    let daemon = NativeDaemon::start(executable, &config, &socket, None, 0, &stderr)?;
    let result = run(&NativeUdsClient::new(socket));
    match (result, daemon.stop(&mut None)) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(primary), Err(cleanup)) => {
            Err(primary.context(format!("Native daemon drain also failed: {cleanup:#}")))
        }
    }
}

#[test]
#[ignore = "requires an explicitly built Native daemon via QUANTA_INDEX_SEARCHD_BIN"]
fn equal_relative_paths_remain_distinct_through_delta_delete_and_process_reopen() -> Result<()> {
    let binary = std::env::var_os("QUANTA_INDEX_SEARCHD_BIN")
        .context("explicit QUANTA_INDEX_SEARCHD_BIN is required")?;
    let binary = Path::new(&binary);
    ensure!(
        binary.is_absolute() && binary.is_file(),
        "daemon must be an absolute existing binary"
    );
    let executable = NativeExecutable::capture(binary)?;
    let live = TempDir::new()?;
    fs::set_permissions(live.path(), fs::Permissions::from_mode(0o700))?;
    let (ack, survivors, final_ranking) =
        with_daemon(&executable, live.path(), "first", |client| {
            let mut audit = PublicationAudit::default();
            let (full, sources) = stage_full(client, 0, &mut audit)?;
            let [first, second, third] = sources.as_slice() else {
                anyhow::bail!("expected three initial repository sources");
            };
            let survivors = [(1, *second), (2, *third)];
            let _full = check_query(
                client,
                full,
                COMMON,
                &[(0, *first), survivors[0], survivors[1]],
            )?;
            let _before = check_query(client, full, BEFORE, &[(0, *first)])?;

            let ingress = client.begin_input(operation(3), &budget())?;
            let replacement = stage_repository_source(
                client,
                ingress,
                SOURCES[0].0,
                b"// qidxsharedword qidxrepo0after\n",
            )?;
            let delta = advance_staged(
                client,
                ingress,
                full,
                [OwnerMutation {
                    owner: owner(0)?,
                    expected: Some(*first),
                    next: Some(replacement),
                }],
                2,
                PublicationLabel {
                    phase: "delta_event_ack",
                    cycle: None,
                },
                &mut audit,
            )?;
            let _stale = check_query(client, delta, BEFORE, &[])?;
            let _changed = check_query(client, delta, AFTER, &[(0, replacement)])?;
            let delta_refs = [(0, replacement), survivors[0], survivors[1]];
            let delta_ranking = check_query(client, delta, COMMON, &delta_refs)?;
            let noop = advance_frontier_only(client, delta, &mut audit)?;
            let noop_ranking = check_query(client, noop, COMMON, &delta_refs)?;
            ensure!(
                noop_ranking.ranking.hits == delta_ranking.ranking.hits
                    && noop_ranking.repositories == delta_ranking.repositories,
                "frontier-only update changed ranked repository paths"
            );

            let deleted = advance(
                client,
                operation(5),
                noop,
                [OwnerMutation {
                    owner: owner(0)?,
                    expected: Some(replacement),
                    next: None,
                }],
                4,
                &mut audit,
            )?;
            let _deleted = check_query(client, deleted, AFTER, &[])?;
            let ranking = check_query(client, deleted, COMMON, &survivors)?;
            Ok((deleted, survivors, ranking))
        })?;

    with_daemon(&executable, live.path(), "reopen", |client| {
        let head = client
            .read_view(
                ViewSelection::Latest {
                    workspace: workspace(),
                    epoch: 1,
                },
                &budget(),
            )?
            .identity;
        ensure!(head == ack, "reopened head differs from delete ACK");
        let _deleted = check_query(client, head, AFTER, &[])?;
        ensure!(
            check_query(client, head, COMMON, &survivors)? == final_ranking,
            "process reopen changed repository ranking"
        );
        Ok(())
    })?;

    let fresh = TempDir::new()?;
    fs::set_permissions(fresh.path(), fs::Permissions::from_mode(0o700))?;
    with_daemon(&executable, fresh.path(), "fresh", |client| {
        let (view, refs) = stage_full(client, 1, &mut PublicationAudit::default())?;
        ensure!(
            refs.iter()
                .copied()
                .eq(survivors.iter().map(|&(_, source)| source)),
            "fresh FULL changed source identity for the surviving input bytes"
        );
        let rebuilt = check_query(client, view, COMMON, &survivors)?;
        ensure!(
            rebuilt.ranking.hits == final_ranking.ranking.hits
                && rebuilt.repositories == final_ranking.repositories,
            "fresh FULL and incremental publication disagree on ranked repository paths"
        );
        Ok(())
    })
}
