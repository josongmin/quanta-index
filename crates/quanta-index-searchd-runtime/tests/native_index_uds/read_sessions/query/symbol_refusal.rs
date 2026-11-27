//! Native Text has no symbol-definition index. Until that product decision is
//! made, a symbol predicate must reach the SDK caller as a typed refusal and
//! never as an empty (silently unsupported) result.

use super::*;

/// The SDK's typed pre-I/O rejection for a query Native Text cannot plan: a
/// shape refusal is `UnsupportedCombo`, never a parse failure.
fn sdk_rejected<T>(result: &Result<T, NativeUdsError>) -> bool {
    matches!(
        result,
        Err(NativeUdsError::QueryRejected {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                quanta_index_contract::lex::LexicalErrorCode::UnsupportedCombo
            ),
            message,
        }) if message.contains("native BM25 requires Content keywords or phrases")
    )
}

#[test]
fn native_text_symbol_predicate_is_refused_as_invalid_input_not_empty() -> TestResult {
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
        let (view, _) =
            publish_text_owner(&client, configured, 2, 1, "src/lib.rs", "fn needle() {}")?;
        let instance = native_text_spec().instance;
        let session = client.open_read_session(ViewSelection::Exact(view), [78; 32], &budget())?;

        // Control: the same view answers an ordinary Content query, so a
        // refusal below is not an artifact of an empty corpus.
        let control = client.search_text_page_in_read_session(
            &session,
            instance,
            native_text_request("needle", 1),
            None,
            &budget(),
        )?;
        assert_eq!(control.matched, 1);

        // The SDK refuses before any I/O with a typed query rejection, and
        // the daemon refuses the same raw request with a typed InvalidInput.
        for query in [
            "symbol.has.name(needle)",
            "needle AND symbol.has.name(needle)",
            "needle OR symbol.has.name(needle)",
        ] {
            let in_session = client.search_text_page_in_read_session(
                &session,
                instance,
                native_text_request(query, 1),
                None,
                &budget(),
            );
            assert!(
                sdk_rejected(&in_session),
                "read-session query {query:?} was not a typed rejection: {in_session:?}"
            );
            let one_shot = client.search_text(
                ViewSelection::Exact(view),
                instance,
                native_text_request(query, 1),
                &budget(),
            );
            assert!(
                sdk_rejected(&one_shot),
                "one-shot query {query:?} was not a typed rejection: {one_shot:?}"
            );
            let daemon = client.request(
                NativeIndexRequest::SearchText {
                    selection: ViewSelection::Exact(view),
                    instance,
                    query: native_text_request(query, 1),
                },
                &budget(),
            );
            assert!(
                matches!(
                    &daemon,
                    Err(NativeUdsError::Refused(error))
                        if error.code == NativeIndexErrorCode::InvalidInput
                ),
                "daemon query {query:?} was not refused as InvalidInput: {daemon:?}"
            );
        }

        // A refusal leaves the retained session usable.
        let after = client.search_text_page_in_read_session(
            &session,
            instance,
            native_text_request("needle", 1),
            None,
            &budget(),
        )?;
        assert_eq!(after.matched, 1);
        let (released, _) = client.release_read_session(&session, &budget())?;
        assert_eq!(released.state, NativeReadSessionState::Released);
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped?;
    Ok(())
}
