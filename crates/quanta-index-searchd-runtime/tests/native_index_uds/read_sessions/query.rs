//! Retained page behavior through the actual Native SDK and UDS dispatcher.

use super::*;

// History fixtures publish through a pinned external Legacy daemon; the
// current test binary and Native service do not embed that engine.
#[path = "query/history.rs"]
mod history;
#[path = "query/hybrid.rs"]
mod hybrid;
#[path = "query/preview.rs"]
mod preview;
#[path = "query/profile.rs"]
mod profile;
#[path = "query/regex_alternation.rs"]
mod regex_alternation;
#[path = "query/repo_predicates.rs"]
mod repo_predicates;
#[path = "query/repository_query_contract.rs"]
mod repository_query_contract;
#[path = "query/text_query_contract.rs"]
mod text_query_contract;
use quanta_index_contract::native_protocol::NativeReadSessionRequest;

#[test]
fn global_text_pages_keep_original_view_across_publication_and_release() -> TestResult {
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
        let (one, _) = publish_text_owner(&client, configured, 2, 1, "a.rs", "alpha")?;
        let (two, _) = publish_text_owner(&client, one, 3, 2, "b.rs", "alpha")?;
        let (held, _) = publish_text_owner(&client, two, 4, 3, "c.rs", "alpha")?;
        let session = client.open_read_session(ViewSelection::Exact(held), [41; 32], &budget())?;
        let instance = native_text_spec().instance;
        let first = client.search_text_page_in_read_session(
            &session,
            instance,
            native_text_request("alpha", 1),
            None,
            &budget(),
        )?;
        assert_eq!(first.view, held);
        assert_eq!((first.matched, first.remaining), (3, 3));
        assert_eq!(
            first
                .hits
                .iter()
                .map(|hit| hit.owner.stable_id)
                .collect::<Vec<_>>(),
            [StableOwnerId([1; 16])]
        );
        assert!(first.after.is_some());
        let (latest, _) = publish_text_owner(&client, held, 5, 4, "d.rs", "alpha")?;
        assert_ne!(latest, held);
        let second = client.search_text_page_in_read_session(
            &session,
            instance,
            native_text_request("alpha", 1),
            first.after,
            &budget(),
        )?;
        assert_eq!(
            (second.view, second.matched, second.remaining),
            (held, 3, 2)
        );
        assert_eq!(
            second
                .hits
                .iter()
                .map(|hit| hit.owner.stable_id)
                .collect::<Vec<_>>(),
            [StableOwnerId([2; 16])]
        );
        let third = client.search_text_page_in_read_session(
            &session,
            instance,
            native_text_request("alpha", 1),
            second.after,
            &budget(),
        )?;
        assert_eq!((third.matched, third.remaining), (3, 1));
        assert_eq!(
            third
                .hits
                .iter()
                .map(|hit| hit.owner.stable_id)
                .collect::<Vec<_>>(),
            [StableOwnerId([3; 16])]
        );
        assert_eq!(third.after, None);
        let pins = view_pin_count(root.path(), held.sequence)?;
        assert!(pins > 0);
        // Bypass the SDK's own binding guard to exercise server-side refusal.
        let mut foreign = first.after.ok_or("page oracle lost first anchor")?;
        foreign.view = latest;
        let refusal = client.request(
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchTextPage {
                session: session.session,
                instance,
                query: native_text_request("alpha", 1),
                after: Some(foreign),
            }),
            &budget(),
        );
        assert!(
            matches!(refusal,Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::InvalidInput)
        );
        assert_eq!(view_pin_count(root.path(), held.sequence)?, pins);
        let mut forged_artifact = first.after.ok_or("page oracle lost artifact anchor")?;
        forged_artifact.artifact = ObjectRef(Digest([99; 32]));
        let refusal = client.request(
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchTextPage {
                session: session.session,
                instance,
                query: native_text_request("alpha", 1),
                after: Some(forged_artifact),
            }),
            &budget(),
        );
        assert!(
            matches!(refusal, Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::InvalidInput)
        );
        assert_eq!(view_pin_count(root.path(), held.sequence)?, pins);
        let (released, _) = client.release_read_session(&session, &budget())?;
        assert_eq!(released.state, NativeReadSessionState::Released);
        assert!(
            matches!(client.search_text_page_in_read_session(&session,instance,native_text_request("alpha",1),None,&budget()),
            Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::SnapshotUnavailable)
        );
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped?;
    Ok(())
}

#[test]
fn text_page_refusals_preserve_session_pin_and_original_deadline() -> TestResult {
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
        let (view, _) = publish_text_owner(&client, configured, 2, 1, "a.rs", "alpha")?;
        let session = client.open_read_session(ViewSelection::Exact(view), [42; 32], &budget())?;
        let pins = view_pin_count(root.path(), view.sequence)?;
        let instance = native_text_spec().instance;
        let refusal = client.request(
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchTextPage {
                session: session.session,
                instance,
                query: native_text_request("alpha count:1", 1),
                after: None,
            }),
            &budget(),
        );
        assert!(
            matches!(refusal,Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::InvalidInput)
        );
        assert_eq!(view_pin_count(root.path(), view.sequence)?, pins);
        let expired = RequestBudgetV1::for_duration(Duration::ZERO);
        assert!(
            client
                .search_text_page_in_read_session(
                    &session,
                    instance,
                    native_text_request("alpha", 1),
                    None,
                    &expired
                )
                .is_err()
        );
        assert_eq!(view_pin_count(root.path(), view.sequence)?, pins);
        let page = client.search_text_page_in_read_session(
            &session,
            instance,
            native_text_request("alpha", 1),
            None,
            &budget(),
        )?;
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.view, view);
        let mut exact = native_text_request("alpha", 1);
        exact.constraints = QueryConstraintSetV1::from_exact_repo_relative_path(
            quanta_index_contract::ExactRepoRelativePathV1::new("a.rs")?,
        );
        let selected = client.search_text_page_in_read_session(
            &session,
            instance,
            exact.clone(),
            None,
            &budget(),
        )?;
        assert_eq!(selected.hits, page.hits);
        exact.constraints = QueryConstraintSetV1::from_exact_repo_relative_path(
            quanta_index_contract::ExactRepoRelativePathV1::new("b.rs")?,
        );
        let excluded =
            client.search_text_page_in_read_session(&session, instance, exact, None, &budget())?;
        assert_eq!((excluded.matched, excluded.remaining), (0, 0));
        assert!(excluded.hits.is_empty());
        assert_eq!(excluded.after, None);
        let empty = client.search_text_page_in_read_session(
            &session,
            instance,
            native_text_request("absent_term", 1),
            None,
            &budget(),
        )?;
        assert_eq!((empty.matched, empty.remaining), (0, 0));
        assert!(empty.hits.is_empty());
        assert_eq!(empty.after, None);
        assert_eq!(view_pin_count(root.path(), view.sequence)?, pins);
        let (released, _) = client.release_read_session(&session, &budget())?;
        assert_eq!(released.state, NativeReadSessionState::Released);
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped?;
    Ok(())
}
