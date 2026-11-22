//! Repository file predicate through the published Native source cut and SDK.

use super::*;
use quanta_index_contract::{
    RepoId,
    native_protocol::{NativeReadSessionStatus, NativeTextPage},
};

// A repository identity is attached by StageRepositorySources, not inferred
// from the relative path or from the source body at query time.
fn publish_repository_sources<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
    expected: quanta_index_contract::index::ViewRef,
    sequence: u64,
    sources: &[(&RepoId, u8, &str, &str, Option<&str>)],
) -> TestResult<quanta_index_contract::index::ViewRef> {
    let operation = operation(sequence);
    let ingress = client.begin_input(operation, &budget())?;
    let mut mutations = Vec::with_capacity(sources.len());
    for (repository, stable_id, path, body, language) in sources {
        let response = client.request(
            NativeIndexRequest::StageRepositorySources {
                ingress,
                repository: (*repository).clone(),
                inputs: vec![SourceInput {
                    path: (*path).into(),
                    bytes: body.as_bytes().to_vec(),
                    text: SourceTextAdmission::Utf8 {
                        policy: Digest([31; 32]),
                    },
                    language: language
                        .map(quanta_index_contract::lex::LanguageCode::new)
                        .transpose()?,
                }],
            },
            &budget(),
        )?;
        let NativeIndexResponse::StagedSources(records) = response else {
            return Err(format!("repository source staging differs: {response:?}").into());
        };
        let [source] = records.as_slice() else {
            return Err("repository source staging returned unexpected count".into());
        };
        mutations.push(OwnerMutation {
            owner: OwnerKey {
                producer: operation.producer,
                stable_id: StableOwnerId([*stable_id; 16]),
            },
            expected: None,
            next: Some(*source),
        });
    }
    let prior = client.read_view(ViewSelection::Exact(expected), &budget())?;
    let owners = client.stage_mutations(ingress, mutations, &budget())?;
    let event_sequence = sequence
        .checked_sub(1)
        .ok_or("repository source event sequence underflow")?;
    let mut transition = SourceTransition {
        expected_source: prior.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation.producer,
        stream: operation.stream,
        stream_epoch: 1,
        predecessor: event_sequence
            .checked_sub(1)
            .ok_or("repository source event predecessor underflow")?,
        sequence: event_sequence,
        payload: transition.owner_digest()?,
    });
    published(&client.submit(
        UpdateRequest {
            operation,
            expected_view: expected,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)
}

fn page<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
    session: &NativeReadSessionStatus,
    query: &str,
    top_k: u32,
) -> TestResult<NativeTextPage> {
    let result = client.search_text_page_in_read_session(
        session,
        native_text_spec().instance,
        native_text_request(query, top_k),
        None,
        &budget(),
    )?;
    assert_eq!(result.view, session.view);
    Ok(result)
}

fn owners(page: &NativeTextPage) -> Vec<StableOwnerId> {
    page.hits.iter().map(|hit| hit.owner.stable_id).collect()
}

#[test]
fn repository_file_gate_uses_same_cut_witness_before_ranking_and_keeps_held_view() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(30),
    )?);
    let primary = (|| -> TestResult {
        let client = NativeUdsClient::new(socket);
        let configured = create_text_workspace(&client)?;
        let corp_a = RepoId::try_from("corp-a")?;
        let corp_b = RepoId::try_from("corp-b")?;
        let held_view = publish_repository_sources(
            &client,
            configured,
            2,
            &[
                (&corp_a, 1, "body/shared.rs", "two", Some("rust")),
                (
                    &corp_b,
                    2,
                    "body/shared.rs",
                    "two two two two",
                    Some("rust"),
                ),
                (&corp_a, 3, "src/lib.rs", "needle", Some("python")),
                (&corp_b, 4, "src/lib.rs", "other", Some("rust")),
                (&corp_a, 5, "src/split.rs", "other", Some("rust")),
                (&corp_a, 6, "src/other.rs", "needle", Some("rust")),
            ],
        )?;
        let held =
            client.open_read_session(ViewSelection::Exact(held_view), [73; 32], &budget())?;

        let baseline = page(&client, &held, "two", 1)?;
        assert_eq!(baseline.matched, 2);
        assert_eq!(owners(&baseline), [StableOwnerId([2; 16])]);

        let gated = page(
            &client,
            &held,
            "two AND repo:has.file(path:src/lib.rs, content:needle)",
            1,
        )?;
        assert_eq!(gated.matched, 1);
        assert_eq!(owners(&gated), [StableOwnerId([1; 16])]);
        assert_eq!(gated.remaining, 1);

        let language_query = "two AND repo:has.file(path:src/lib.rs, lang:PyThOn, content:needle)";
        let by_language = page(&client, &held, language_query, 1)?;
        assert_eq!(by_language.matched, 1);
        assert_eq!(owners(&by_language), [StableOwnerId([1; 16])]);
        assert_eq!(
            page(
                &client,
                &held,
                "two AND repo:has.file(path:src/lib.rs, lang:rust, content:needle)",
                1
            )?
            .matched,
            0
        );

        let mut sourcegraph = native_text_request("repo:has.file(path:src/lib.rs) needle", 10);
        sourcegraph.syntax = quanta_index_contract::TextQuerySyntax::Sourcegraph;
        let sourcegraph = client.search_text_page_in_read_session(
            &held,
            native_text_spec().instance,
            sourcegraph,
            None,
            &budget(),
        )?;
        assert_eq!(sourcegraph.view, held_view);
        assert_eq!(sourcegraph.matched, 2);
        let mut sourcegraph_owners = owners(&sourcegraph);
        sourcegraph_owners.sort_unstable_by_key(|owner| owner.0);
        assert_eq!(
            sourcegraph_owners,
            [StableOwnerId([3; 16]), StableOwnerId([6; 16])]
        );

        let split = page(
            &client,
            &held,
            "two AND repo:has.file(path:src/split.rs, content:needle)",
            1,
        )?;
        assert_eq!(split.matched, 0);
        assert!(split.hits.is_empty());

        let regex = page(
            &client,
            &held,
            r"two AND repo:has.file(path:src/.*, name:lib\.rs, content:needle)",
            1,
        )?;
        assert_eq!(regex.matched, 1);
        assert_eq!(owners(&regex), [StableOwnerId([1; 16])]);

        let complement = page(
            &client,
            &held,
            "two AND NOT repo:has.file(path:src/lib.rs, content:needle)",
            1,
        )?;
        assert_eq!(complement.matched, 1);
        assert_eq!(owners(&complement), [StableOwnerId([2; 16])]);

        let disjunction = page(
            &client,
            &held,
            "two OR repo:has.file(path:src/lib.rs, content:needle)",
            10,
        )?;
        assert_eq!(disjunction.matched, 5);
        let mut actual = owners(&disjunction);
        actual.sort_unstable_by_key(|owner| owner.0);
        assert_eq!(actual, [1_u8, 2, 3, 5, 6].map(|id| StableOwnerId([id; 16])));

        // The newly published witness changes the latest cut only. The held
        // read session continues to query its original source view.
        let latest = publish_repository_sources(
            &client,
            held_view,
            3,
            &[(&corp_b, 7, "src/new.rs", "needle", None)],
        )?;
        assert_ne!(latest, held_view);
        let held_again = page(&client, &held, "two AND repo:has.file(content:needle)", 2)?;
        assert_eq!(held_again.matched, 1);
        assert_eq!(owners(&held_again), [StableOwnerId([1; 16])]);
        let current =
            client.open_read_session(ViewSelection::Exact(latest), [74; 32], &budget())?;
        let current_page = page(
            &client,
            &current,
            "two AND repo:has.file(content:needle)",
            2,
        )?;
        assert_eq!(current_page.matched, 2);
        let mut current_owners = owners(&current_page);
        current_owners.sort_unstable_by_key(|owner| owner.0);
        assert_eq!(
            current_owners,
            [StableOwnerId([1; 16]), StableOwnerId([2; 16])]
        );

        // Removing the sole authenticated language witness affects only the
        // successor cut; a held session retains the earlier published record.
        let witness = OwnerKey {
            producer: operation(2).producer,
            stable_id: StableOwnerId([3; 16]),
        };
        let lookup =
            client.lookup_source_owners(ViewSelection::Exact(latest), vec![witness], &budget())?;
        let witness_ref = lookup
            .rows
            .as_slice()
            .first()
            .and_then(|row| row.source)
            .ok_or("language witness is absent")?;
        let delete_operation = operation(4);
        let ingress = client.begin_input(delete_operation, &budget())?;
        let changes = client.stage_mutations(
            ingress,
            [OwnerMutation {
                owner: witness,
                expected: Some(witness_ref),
                next: None,
            }],
            &budget(),
        )?;
        let mut transition = SourceTransition {
            expected_source: lookup.source,
            owners: changes,
            events: Vec::new(),
        };
        transition.events.push(SourceEvent {
            producer: delete_operation.producer,
            stream: delete_operation.stream,
            stream_epoch: 1,
            predecessor: 2,
            sequence: 3,
            payload: transition.owner_digest()?,
        });
        let without_witness = published(&client.submit(
            UpdateRequest {
                operation: delete_operation,
                expected_view: latest,
                kind: UpdateKind::SourceAdvance(transition),
            },
            &budget(),
        )?)?;
        let deleted =
            client.open_read_session(ViewSelection::Exact(without_witness), [76; 32], &budget())?;
        assert_eq!(page(&client, &deleted, language_query, 1)?.matched, 0);
        assert_eq!(page(&client, &held, language_query, 1)?.matched, 1);
        let (released, _) = client.release_read_session(&deleted, &budget())?;
        assert_eq!(released.state, NativeReadSessionState::Released);

        let (released, _) = client.release_read_session(&held, &budget())?;
        assert_eq!(released.state, NativeReadSessionState::Released);
        let (released, _) = client.release_read_session(&current, &budget())?;
        assert_eq!(released.state, NativeReadSessionState::Released);
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped?;
    Ok(())
}

#[test]
fn repository_language_gate_refuses_missing_metadata_over_uds() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(30),
    )?);
    let primary = (|| -> TestResult {
        let client = NativeUdsClient::new(socket);
        let configured = create_text_workspace(&client)?;
        let repository = RepoId::try_from("corp-a")?;
        let view = publish_repository_sources(
            &client,
            configured,
            2,
            &[(&repository, 1, "body/old.rs", "two", None)],
        )?;
        let session = client.open_read_session(ViewSelection::Exact(view), [77; 32], &budget())?;
        for query in [
            "two AND repo:has.file(lang:rust)",
            "two OR repo:has.file(lang:rust)",
        ] {
            assert!(matches!(client.search_text_page_in_read_session(
                &session, native_text_spec().instance, native_text_request(query, 1), None, &budget(),
            ), Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::InvalidInput));
        }
        assert_eq!(
            page(
                &client,
                &session,
                "two AND repo:has.file(path:body/old.rs)",
                1
            )?
            .matched,
            1
        );
        let (released, _) = client.release_read_session(&session, &budget())?;
        assert_eq!(released.state, NativeReadSessionState::Released);
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped?;
    Ok(())
}

#[test]
fn repository_file_gate_refuses_old_cut_without_repository_identity() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(30),
    )?);
    let primary = (|| -> TestResult {
        let client = NativeUdsClient::new(socket);
        let configured = create_text_workspace(&client)?;
        let (view, _) = publish_text_owner(&client, configured, 2, 11, "body/old.rs", "two")?;
        let session = client.open_read_session(ViewSelection::Exact(view), [75; 32], &budget())?;
        let refusal = client.search_text_page_in_read_session(
            &session,
            native_text_spec().instance,
            native_text_request("two AND repo:has.file(path:body/old.rs)", 1),
            None,
            &budget(),
        );
        assert!(matches!(
            refusal,
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::InvalidInput
        ));
        let (released, _) = client.release_read_session(&session, &budget())?;
        assert_eq!(released.state, NativeReadSessionState::Released);
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped?;
    Ok(())
}
