//! Pinned old-Q process regressions for composite generation authority.

#![forbid(unsafe_code)]

use std::error::Error;
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_contract::{
    SearchCorpusActiveHeadV1, SearchPlaneErrorCodeV2,
    SearchPlaneRollbackSearchCorpusGenerationCasRequest,
};
use quanta_index_core::GenerationStorageKeyV1;
use quanta_index_sdk::{
    ChunkId, ChunkRecord, GenerationPin, LanguageCode, QuantaIndex, RepoId, RepoRelativePath,
    RevisionId, SdkError, SearchCorpusBatch, SourceFileKey,
};
use quanta_index_searchd_harness::{
    fixture_source_scope_v1, legacy_reference::LegacySearchdBinary,
};

use crate::legacy_start_probe;

#[path = "composite_generation_authority_restart/fixture.rs"]
mod fixture;
use fixture::*;

type TestResult = Result<(), Box<dyn Error>>;
const G0: u64 = 40;
const G0_DIGEST: &str = "manifest:composite-restart:g0";
const ERR_ROLLBACK_CAS_CONFLICT: SearchPlaneErrorCodeV2 =
    SearchPlaneErrorCodeV2::RollbackCasConflict;
const ERR_STATE_ROOT_IN_USE: SearchPlaneErrorCodeV2 = SearchPlaneErrorCodeV2::StateRootInUse;

fn batch_for(
    repo_id: &str,
    revision_id: &str,
    raw_generation: u64,
    digest: &str,
    source_parent_v1: Option<u64>,
) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let path = format!("src/{repo_id}_generation_{raw_generation}.rs");
    let text = format!("fn generation_{raw_generation}() {{}}");
    let end_byte = u32::try_from(text.len())?;
    let source_repo_v1 = RepoId::new(repo_id)?;
    let revision_v1 = RevisionId::new(revision_id)?;
    let scope_v1 = fixture_source_scope_v1(
        SourceFileKey {
            source_repo_id: source_repo_v1.clone(),
            repo_relative_path: RepoRelativePath::new(path.clone()),
        },
        revision_v1.clone(),
        vec![ChunkRecord {
            chunk_id: ChunkId::new(format!("chunk-generation-{raw_generation}")),
            repo_relative_path: RepoRelativePath::new(path),
            language: LanguageCode::new("rust")?,
            start_byte: 0,
            end_byte,
            start_line: 1,
            end_line: 1,
            text: text.into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        }],
        Vec::new(),
    )?;
    Ok(SearchCorpusBatch::replace_generation(
        source_repo_v1,
        revision_v1,
        generation(raw_generation),
        digest,
    )
    .source_event(fixture_event_v1(repo_id, raw_generation, source_parent_v1))
    .replace_scope(
        scope_v1.coverage,
        scope_v1.source_bytes,
        scope_v1.chunks,
        scope_v1.symbols,
    ))
}

/// Observe the released binary only through the SDK query front door.
fn assert_lexical_generation(
    client: &QuantaIndex,
    raw_generation: u64,
    pinned: bool,
) -> TestResult {
    let started = Instant::now();
    let mut last_error = None;
    while started.elapsed() < Duration::from_secs(30) {
        let query = client
            .lexical()
            .query()
            .native(format!("generation_{raw_generation}"));
        let query = if pinned {
            query.pinned(GenerationPin::new(
                repo(),
                revision(),
                generation(raw_generation),
            ))
        } else {
            query.active(repo(), revision())
        };
        match query.top_k(5).execute() {
            Ok(response) => {
                let expected_pin =
                    GenerationPin::new(repo(), revision(), generation(raw_generation));
                let expected_candidate = format!("chunk-generation-{raw_generation}");
                if response.generation != expected_pin
                    || response
                        .results
                        .first()
                        .map(|candidate| candidate.candidate_id.as_str())
                        != Some(expected_candidate.as_str())
                {
                    return Err(format!(
                        "SDK query observed wrong generation or candidate: {response:?}"
                    )
                    .into());
                }
                return Ok(());
            }
            Err(error) => last_error = Some(error),
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err(format!("SDK query did not become ready: {last_error:?}").into())
}

fn publish_three_generations(client: &QuantaIndex) -> Result<Activated, Box<dyn Error>> {
    let g0 = publish_generation(client, G0, G0_DIGEST, None)?;
    let g1 = publish_generation(client, G1, G1_DIGEST, Some(g0.clone()))?;
    let g2 = publish_generation(client, G2, G2_DIGEST, Some(g1.clone()))?;
    Ok(Activated {
        g0: Some(g0),
        g1,
        g2,
    })
}

fn publish_and_activate_for(
    client: &QuantaIndex,
    repo_id: &str,
    revision_id: &str,
    raw_generation: u64,
    digest: &str,
    expected_active: Option<SearchCorpusActiveHeadV1>,
) -> Result<SearchCorpusActiveHeadV1, Box<dyn Error>> {
    let (receipt, activation) = client.search_corpus().publish_and_activate(
        &batch_for(
            repo_id,
            revision_id,
            raw_generation,
            digest,
            expected_active
                .as_ref()
                .map(|head_v1| head_v1.generation.lexical.manifest_generation.get()),
        )?,
        expected_active,
    )?;
    let expected = composite_identity_for(repo_id, revision_id, raw_generation, digest);
    if activation.active.generation.lexical != expected.lexical
        || activation.active.generation.semantic != expected.semantic
        || Some(&activation.active.generation.semantic_content) != receipt.semantic_content.as_ref()
    {
        return Err(format!(
            "publish-and-activate selected a foreign composite identity: expected tracks={expected:?} receipt roots={:?} observed={:?}",
            receipt.semantic_content, activation.active
        )
        .into());
    }
    Ok(activation.active)
}

#[test]
fn rollback_rejects_stale_expected_active_and_preserves_current_composite_v1() -> TestResult {
    let directory = quanta_index_searchd_harness::private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let first_process = binary.start(directory.path(), 8)?;
    let first_client = first_process.connect()?;
    let activated = publish_three_generations(&first_client)?;
    let g0 = activated
        .g0
        .clone()
        .ok_or("three generations were published")?;

    let rollback =
        first_client
            .generations()
            .rollback(SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                expected_active: activated.g1.clone(),
                target: g0.generation,
            });
    let Err(SdkError::Remote { code, .. }) = rollback else {
        return Err(format!(
            "rollback with stale expected active did not return a typed remote error: {rollback:?}"
        )
        .into());
    };
    if code != ERR_ROLLBACK_CAS_CONFLICT {
        return Err(format!(
            "stale expected active must report {ERR_ROLLBACK_CAS_CONFLICT}, got {code}"
        )
        .into());
    }

    let current = current_composite(&first_client)?;
    current
        .validate_v1()
        .map_err(|error| format!("stale rollback split active authority: {error}"))?;
    if current != activated.g2.generation {
        return Err(format!("stale rollback changed active G2: {current:?}").into());
    }
    drop(first_client);
    first_process.stop()?;

    let second_process = binary.start(directory.path(), 8)?;
    let second_client = second_process.connect()?;
    let reopened = current_composite(&second_client)?;
    if reopened != activated.g2.generation {
        return Err(format!("stale rollback changed reopened G2: {reopened:?}").into());
    }
    drop(second_client);
    second_process.stop()?;
    binary.verify()
}

#[test]
fn real_child_process_restart_preserves_and_rolls_back_composite_generation_v1() -> TestResult {
    let directory = quanta_index_searchd_harness::private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;

    let first_process = binary.start(directory.path(), 8)?;
    let first_client = first_process.connect()?;
    let activated = publish_two_generations(&first_client)?;
    if current_composite(&first_client)? != activated.g2.generation {
        return Err("child process did not activate exact G2 composite identity".into());
    }
    assert_lexical_generation(&first_client, G2, false)?;
    assert_lexical_generation(&first_client, G1, true)?;
    drop(first_client);
    first_process.stop()?;

    let second_process = binary.start(directory.path(), 8)?;
    let second_client = second_process.connect()?;
    if current_composite(&second_client)? != activated.g2.generation {
        return Err("child process restart did not recover exact G2 composite identity".into());
    }
    assert_lexical_generation(&second_client, G2, false)?;
    assert_lexical_generation(&second_client, G1, true)?;
    rollback_g2_to_g1(&second_client, &activated)?;
    if current_composite(&second_client)? != activated.g1.generation {
        return Err("child process rollback did not activate exact G1 composite identity".into());
    }
    assert_lexical_generation(&second_client, G1, false)?;
    assert_lexical_generation(&second_client, G2, true)?;
    drop(second_client);
    second_process.stop()?;

    let third_process = binary.start(directory.path(), 8)?;
    let third_client = third_process.connect()?;
    if current_composite(&third_client)? != activated.g1.generation {
        return Err(
            "second child process restart did not preserve rolled-back G1 authority".into(),
        );
    }
    assert_lexical_generation(&third_client, G1, false)?;
    assert_lexical_generation(&third_client, G2, true)?;
    drop(third_client);
    third_process.stop()?;
    binary.verify()
}

#[test]
fn real_child_process_cross_repo_restart_retains_and_rolls_back_each_composite_v1() -> TestResult {
    const REPO_B: &str = "repo-composite-restart-b";
    const REVISION_B: &str = "revision-composite-restart-b";
    const A0_DIGEST: &str = "manifest:cross-repo:a:g0";
    const A1_DIGEST: &str = "manifest:cross-repo:a:g1";
    const A2_DIGEST: &str = "manifest:cross-repo:a:g2";
    const B0_DIGEST: &str = "manifest:cross-repo:b:g0";
    const B1_DIGEST: &str = "manifest:cross-repo:b:g1";
    const B2_DIGEST: &str = "manifest:cross-repo:b:g2";

    let directory = quanta_index_searchd_harness::private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let first_process = binary.start(directory.path(), 2)?;
    let first_client = first_process.connect()?;

    let a0 = publish_and_activate_for(&first_client, REPO, REVISION, G0, A0_DIGEST, None)?;
    let b0 = publish_and_activate_for(&first_client, REPO_B, REVISION_B, G0, B0_DIGEST, None)?;
    let a1 = publish_and_activate_for(&first_client, REPO, REVISION, G1, A1_DIGEST, Some(a0))?;
    let b1 = publish_and_activate_for(&first_client, REPO_B, REVISION_B, G1, B1_DIGEST, Some(b0))?;
    let a2 = publish_and_activate_for(
        &first_client,
        REPO,
        REVISION,
        G2,
        A2_DIGEST,
        Some(a1.clone()),
    )?;
    let b2 = publish_and_activate_for(
        &first_client,
        REPO_B,
        REVISION_B,
        G2,
        B2_DIGEST,
        Some(b1.clone()),
    )?;

    if current_composite_for(&first_client, REPO, REVISION)? != a2.generation
        || current_composite_for(&first_client, REPO_B, REVISION_B)? != b2.generation
    {
        return Err("cross-repo setup did not preserve two independent active composites".into());
    }
    let rejected = legacy_start_probe::require_start_failure(&binary, directory.path(), 2)?;
    let rejection_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    if !rejection_text.contains(ERR_STATE_ROOT_IN_USE.as_wire_str()) {
        return Err(format!(
            "second child did not reject the live cross-repo state root with {ERR_STATE_ROOT_IN_USE}: {rejection_text}"
        )
        .into());
    }
    drop(first_client);
    first_process.stop()?;

    let second_process = binary.start(directory.path(), 2)?;
    let second_client = second_process.connect()?;
    if current_composite_for(&second_client, REPO, REVISION)? != a2.generation
        || current_composite_for(&second_client, REPO_B, REVISION_B)? != b2.generation
    {
        return Err("restart aliased or lost one repo's active composite".into());
    }

    let a_rollback = second_client.generations().rollback(
        SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: a2.clone(),
            target: a1.generation.clone(),
        },
    )?;
    if a_rollback.active.generation != a1.generation || a_rollback.previous_sealed_active != a2 {
        return Err("repo A rollback ack did not bind the exact CAS transition".into());
    }
    if current_composite_for(&second_client, REPO, REVISION)? != a1.generation
        || current_composite_for(&second_client, REPO_B, REVISION_B)? != b2.generation
    {
        return Err("repo A rollback changed repo B or failed to select retained A1".into());
    }
    let b_rollback = second_client.generations().rollback(
        SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: b2.clone(),
            target: b1.generation.clone(),
        },
    )?;
    if b_rollback.active.generation != b1.generation || b_rollback.previous_sealed_active != b2 {
        return Err("repo B rollback ack did not bind the exact CAS transition".into());
    }
    if current_composite_for(&second_client, REPO, REVISION)? != a1.generation
        || current_composite_for(&second_client, REPO_B, REVISION_B)? != b1.generation
    {
        return Err("repo B rollback changed repo A or failed to select retained B1".into());
    }
    drop(second_client);
    second_process.stop()?;

    let third_process = binary.start(directory.path(), 2)?;
    let third_client = third_process.connect()?;
    if current_composite_for(&third_client, REPO, REVISION)? != a1.generation
        || current_composite_for(&third_client, REPO_B, REVISION_B)? != b1.generation
    {
        return Err(
            "second restart did not preserve both independently rolled-back composites".into(),
        );
    }
    drop(third_client);
    third_process.stop()?;
    binary.verify()
}

enum MissingTargetTrack {
    Lexical,
    Semantic,
}

fn assert_missing_target_rejected(track: MissingTargetTrack) -> TestResult {
    let directory = quanta_index_searchd_harness::private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let first_process = binary.start(directory.path(), 8)?;
    let first_client = first_process.connect()?;
    let activated = publish_two_generations(&first_client)?;
    drop(first_client);
    first_process.stop()?;

    let track_root = match track {
        MissingTargetTrack::Lexical => directory.path().join("indexes/lexical"),
        MissingTargetTrack::Semantic => {
            quanta_index_semantic::semantic_state_root(directory.path())
        }
    };
    let target_root = GenerationStorageKeyV1::for_repo_revision(&repo(), &revision())
        .generation_dir(&track_root, generation(G1));
    std::fs::remove_dir_all(&target_root)?;

    let second_process = binary.start(directory.path(), 8)?;
    let second_client = second_process.connect()?;
    let rollback = rollback_g2_to_g1(&second_client, &activated);
    let Err(SdkError::Remote {
        code: SearchPlaneErrorCodeV2::RollbackTargetUnopenable,
        ..
    }) = rollback
    else {
        return Err(format!(
            "rollback with missing target {target_root:?} did not return RollbackTargetUnopenable: {rollback:?}"
        )
        .into());
    };
    let current = current_composite(&second_client)?;
    current
        .validate_v1()
        .map_err(|error| format!("failed rollback split active authority: {error}"))?;
    if current != activated.g2.generation {
        return Err(format!("failed rollback changed active G2: {current:?}").into());
    }
    drop(second_client);
    second_process.stop()?;
    binary.verify()
}

#[test]
fn rollback_rejects_missing_lexical_target_and_preserves_active_composite() -> TestResult {
    assert_missing_target_rejected(MissingTargetTrack::Lexical)
}

#[test]
fn rollback_rejects_missing_semantic_target_and_preserves_active_composite() -> TestResult {
    assert_missing_target_rejected(MissingTargetTrack::Semantic)
}

#[test]
fn real_child_process_state_root_lease_rejects_second_owner_and_releases_v1() -> TestResult {
    let directory = quanta_index_searchd_harness::private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let first_process = binary.start(directory.path(), 8)?;

    let rejected = legacy_start_probe::require_start_failure(&binary, directory.path(), 8)?;
    let rejection_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    if !rejection_text.contains(ERR_STATE_ROOT_IN_USE.as_wire_str()) {
        return Err(format!(
            "second child process did not report {ERR_STATE_ROOT_IN_USE}: {rejection_text}"
        )
        .into());
    }

    drop(first_process);
    let admitted_after_release = binary.start(directory.path(), 8)?;
    admitted_after_release.stop()?;
    binary.verify()
}
