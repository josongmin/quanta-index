//! Metadata capture must use the same retained profile as subsequent queries.

use super::*;

#[test]
fn retained_profile_keeps_captured_configuration_across_reconfigure_and_release() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(30),
    )?);
    let primary = (|| -> TestResult {
        let client = NativeUdsClient::new(socket);
        let held = create_text_workspace(&client)?;
        let old_view = client.read_view(ViewSelection::Exact(held), &budget())?;
        let session = client.open_read_session(ViewSelection::Exact(held), [61; 32], &budget())?;
        // Change the actual profile object, not just a source/head sequence.
        // The latest profile has no query roles; looking up Latest cannot pass.
        let ingress = client.begin_input(operation(2), &budget())?;
        let new_profile =
            client.stage_profile(ingress, IndexProfile { instances: vec![] }, &budget())?;
        let latest = published(&client.submit(
            UpdateRequest {
                operation: operation(2),
                expected_view: held,
                kind: UpdateKind::Reconfigure {
                    profile: new_profile,
                },
            },
            &budget(),
        )?)?;
        assert_ne!(latest, held);
        assert_ne!(new_profile, old_view.profile);
        let pins = view_pin_count(root.path(), held.sequence)?;
        let captured = client.read_session_profile(&session, &budget())?;
        assert_eq!(captured.session, session.session);
        assert_eq!(captured.view, old_view);
        assert_eq!(
            captured.profile,
            IndexProfile {
                instances: vec![native_text_spec()]
            }
        );
        assert_eq!(view_pin_count(root.path(), held.sequence)?, pins);
        // Exercise both the metadata consumer and an actual reader on that view.
        let page = client.search_text_page_in_read_session(
            &session,
            native_text_spec().instance,
            native_text_request("alpha", 1),
            None,
            &budget(),
        )?;
        assert_eq!(page.view, held);
        assert_eq!(page.matched, 0);
        let latest_session =
            client.open_read_session(ViewSelection::Exact(latest), [62; 32], &budget())?;
        let latest_capture = client.read_session_profile(&latest_session, &budget())?;
        assert_eq!(latest_capture.view.identity, latest);
        assert_eq!(latest_capture.view.profile, new_profile);
        assert!(latest_capture.profile.instances.is_empty());
        let (released, _) = client.release_read_session(&session, &budget())?;
        assert_eq!(released.state, NativeReadSessionState::Released);
        // Bypass the local Active check: the real dispatcher must reject too.
        assert!(matches!(client.request(NativeIndexRequest::ReadSession(
            NativeReadSessionRequest::ReadProfile { session: session.session }), &budget()),
            Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::SnapshotUnavailable));
        let (released_latest, _) = client.release_read_session(&latest_session, &budget())?;
        assert_eq!(released_latest.state, NativeReadSessionState::Released);
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped?;
    Ok(())
}
