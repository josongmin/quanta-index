//! Public Native repository-query contracts retained from the Legacy text suite.

use super::*;
use quanta_index_contract::{
    RepoId, SearchPlaneErrorCodeV2, lex::LexicalErrorCode, native_protocol::NativeReadSessionStatus,
};

fn publish_repo_owner<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
    expected: quanta_index_contract::index::ViewRef,
    sequence: u64,
    stable_id: u8,
    repository: &RepoId,
    path: &str,
    body: &str,
) -> TestResult<quanta_index_contract::index::ViewRef> {
    let ingress = client.begin_input(operation(sequence), &budget())?;
    let (request, _) = prepare_text_owner_in_repository(
        client,
        expected,
        ingress,
        stable_id,
        path,
        body,
        Some(repository.clone()),
    )?;
    published(&client.submit(request, &budget())?)
}

fn matching_owners<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
    session: &NativeReadSessionStatus,
    query: &str,
) -> TestResult<Vec<StableOwnerId>> {
    let page = client.search_text_page_in_read_session(
        session,
        native_text_spec().instance,
        native_text_request(query, 10),
        None,
        &budget(),
    )?;
    assert_eq!(page.view, session.view);
    let mut owners = page
        .hits
        .iter()
        .map(|hit| hit.owner.stable_id)
        .collect::<Vec<_>>();
    owners.sort_unstable_by_key(|owner| owner.0);
    Ok(owners)
}

#[test]
fn repository_path_aliases_and_scalar_file_matchers_gate_the_source_repository() -> TestResult {
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
        let one = publish_repo_owner(
            &client,
            configured,
            2,
            1,
            &corp_a,
            "src/doc-a.rs",
            "shared_oracle_needle",
        )?;
        let two = publish_repo_owner(
            &client,
            one,
            3,
            2,
            &corp_a,
            "src/gate-a.rs",
            "shared_oracle_needle",
        )?;
        let view = publish_repo_owner(
            &client,
            two,
            4,
            3,
            &corp_b,
            "src/doc-b.rs",
            "shared_oracle_needle",
        )?;
        let session = client.open_read_session(ViewSelection::Exact(view), [84; 32], &budget())?;
        let expected = [StableOwnerId([1; 16]), StableOwnerId([2; 16])];
        for query in [
            "shared_oracle_needle AND repo:has.file(src/gate-a.rs)",
            "shared_oracle_needle AND repo:has.file(name:gate-a.rs)",
            "shared_oracle_needle AND repo:has.path(src/gate-a.rs)",
            "shared_oracle_needle AND repo:contains.path(src/gate-a.rs)",
        ] {
            assert_eq!(
                matching_owners(&client, &session, query)?,
                expected,
                "{query}"
            );
        }
        for query in [
            "shared_oracle_needle AND repo:has.file(src/missing.rs)",
            "shared_oracle_needle AND repo:has.file(name:missing.rs)",
        ] {
            assert!(
                matching_owners(&client, &session, query)?.is_empty(),
                "{query}"
            );
        }
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
fn legacy_repo_metadata_and_repo_surface_are_explicitly_unavailable_on_native_text() -> TestResult {
    // `repo:<pattern>` is an executable Native filter over authenticated
    // Source repository metadata (09.1c). The other Legacy repository
    // surfaces have no Native producer and stay explicit refusals.
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
        let repo = RepoId::try_from("corp-a")?;
        let view = publish_repo_owner(&client, configured, 2, 1, &repo, "src/lib.rs", "needle")?;
        let session = client.open_read_session(ViewSelection::Exact(view), [85; 32], &budget())?;
        for query in [
            "needle fork:only",
            "needle archived:no",
            "needle visibility:private",
            "needle context:team-search",
            "needle select:repo",
            "needle type:repo",
        ] {
            let result = client.search_text_page_in_read_session(
                &session,
                native_text_spec().instance,
                native_text_request(query, 10),
                None,
                &budget(),
            );
            assert!(
                matches!(&result, Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::InvalidInput),
                "Native text must refuse unsupported Legacy repository query {query}: {result:?}"
            );
        }
        for query in [
            "needle AND repo.has.content(needle)",
            "needle AND repo:has.content(needle)",
        ] {
            let result = client.search_text_page_in_read_session(
                &session,
                native_text_spec().instance,
                native_text_request(query, 10),
                None,
                &budget(),
            );
            // The Native content grammar admits only its listed predicates;
            // any other predicate is a typed unsupported combination.
            assert!(
                matches!(&result, Err(NativeUdsError::QueryRejected { code, .. })
                    if *code == SearchPlaneErrorCodeV2::Lexical(LexicalErrorCode::UnsupportedCombo)),
                "Native SDK must reject the unsupported repository predicate {query}: {result:?}"
            );
        }
        assert_eq!(
            matching_owners(&client, &session, "needle")?,
            [StableOwnerId([1; 16])]
        );
        // The repository filter applies to the record's own repository.
        assert_eq!(
            matching_owners(&client, &session, "needle repo:corp-a")?,
            [StableOwnerId([1; 16])]
        );
        assert!(matching_owners(&client, &session, "needle repo:corp-b")?.is_empty());
        let (released, _) = client.release_read_session(&session, &budget())?;
        assert_eq!(released.state, NativeReadSessionState::Released);

        // A selected Source record without repository metadata cannot prove
        // it is outside the requested repository: refused, never dropped.
        let (legacy, _) =
            publish_text_owner(&client, view, 3, 2, "src/legacy.rs", "needle legacy")?;
        let session =
            client.open_read_session(ViewSelection::Exact(legacy), [86; 32], &budget())?;
        let result = client.search_text_page_in_read_session(
            &session,
            native_text_spec().instance,
            native_text_request("needle repo:corp-a", 10),
            None,
            &budget(),
        );
        assert!(
            matches!(&result, Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::InvalidInput
                    && error.message.contains("repository metadata")),
            "a record without repository metadata must refuse repo:: {result:?}"
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
