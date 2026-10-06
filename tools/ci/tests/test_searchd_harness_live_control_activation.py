"""Static TDD fence for harness generation activation authority.

The reusable E2E harness must exercise the same control UDS and composite CAS
contract as production.  A direct ``ActivationCatalog`` mutation would make
daemon tests green while bypassing ledger readiness and transport validation.
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
HARNESS = ROOT / "crates/quanta-index-searchd-harness/src/harness.rs"
SOURCE_PUBLICATION = ROOT / "crates/quanta-index-searchd-harness/src/harness/source_publication.rs"


def read_harness() -> str:
    return HARNESS.read_text(encoding="utf-8")


def rust_item_body(source: str, marker: str) -> str:
    """Return one Rust item's brace-delimited body for narrow source checks."""

    start = source.find(marker)
    assert start >= 0, f"missing Rust item marker: {marker}"
    open_brace = source.find("{", start)
    assert open_brace >= 0, f"missing opening brace after: {marker}"

    depth = 0
    for index in range(open_brace, len(source)):
        token = source[index]
        if token == "{":
            depth += 1
        elif token == "}":
            depth -= 1
            if depth == 0:
                return source[open_brace : index + 1]
    raise AssertionError(f"unterminated Rust item after: {marker}")


def test_harness_activation_has_no_direct_catalog_backdoor_v1() -> None:
    source = read_harness()

    for forbidden in (
        "ActivationCatalog",
        "PreparedSearchCorpusGenerationV1",
        "SearchCorpusGenerationV1",
        'state_root.join("activations")',
        "activate_prepared_search_corpus_generation_v1",
    ):
        assert forbidden not in source, forbidden

    assert "SearchPlaneActivateSearchCorpusGenerationCasRequest" in source
    assert "SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas" in source


def test_harness_driver_owns_and_waits_for_control_socket_v1() -> None:
    source = read_harness()
    driver = rust_item_body(source, "struct DriverState")
    start_driver = rust_item_body(source, "fn start_driver(")

    assert "control_socket: PathBuf" in driver
    assert "runtime.control_server.socket_path()" in start_driver
    for socket in ("query_socket", "control_socket", "ingest_socket"):
        assert f"{socket}.exists()" in start_driver, socket
    assert re.search(
        r"Ok\(\(\s*query_socket,\s*control_socket,\s*ingest_socket,",
        start_driver,
    )


def test_sealed_ingest_receipt_identity_reaches_composite_cas_v1() -> None:
    source = read_harness()
    runtime = rust_item_body(source, "pub struct E2eRuntime {")
    seal = rust_item_body(source, "pub fn seal_lexical_generation_for_tracks(")
    publish = rust_item_body(source, "pub fn publish_search_corpus_batch(")
    dispatch = rust_item_body(source, "fn dispatch_ingest_response(")
    activate = rust_item_body(source, "activate_last_sealed_generation(")
    publication = SOURCE_PUBLICATION.read_text(encoding="utf-8")

    assert "Option<SearchCorpusGenerationIdentityV1>" in runtime
    assert "self.publish_search_corpus_batch(batch)?" in seal
    assert "SearchPlaneIngestIpcResponse::SearchCorpusReceipt(outcome)" in publish
    assert "self.search_corpus_identity_from_sealed_receipt(" in publish
    assert "outcome.publication.target.manifest_generation" in publish
    assert "&outcome.receipt" in publish
    assert "validate_receipt(requested, *sealed, &outcome.receipt)" in dispatch
    assert "self.source_publication.accept(&batch, &outcome)?" in publish
    for receipt_field in (
        "receipt.accepted_clear_surfaces",
        "receipt.accepted_replace_scopes",
        "receipt.accepted_tombstone_scopes",
    ):
        assert receipt_field in publication, receipt_field

    stored_identity = re.search(
        r"self\.(?P<field>[A-Za-z0-9_]*sealed[A-Za-z0-9_]*)\s*=\s*Some\(",
        publish,
    )
    assert stored_identity, "seal must retain the validated composite receipt identity"
    assert re.search(
        rf"self\s*\.\s*{re.escape(stored_identity.group('field'))}",
        activate,
    )
    assert "SearchPlaneActivateSearchCorpusGenerationCasRequest" in activate
    assert "candidate" in activate
    assert "expected_active" in activate
    assert "GenerationSnapshot {" not in activate


def test_control_response_and_composite_activation_ack_are_request_bound_v1() -> None:
    source = read_harness()
    dispatch_control = rust_item_body(source, "fn dispatch_control(")
    dispatch_control_response = rust_item_body(source, "fn dispatch_control_response_v1(")
    activate = rust_item_body(source, "activate_last_sealed_generation(")

    assert "response.request_id != request_id" in dispatch_control_response
    assert "SearchPlaneControlIpcResponse::Error" in dispatch_control
    assert "SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(ack)" in activate
    assert re.search(r"ack\.active\.generation\s*!=\s*candidate", activate)
    assert re.search(
        r"ack\.previous_sealed_active\s*!=\s*expected_active",
        activate,
    )


def test_reopen_surfaces_driver_join_failure_without_discard_v1() -> None:
    source = read_harness()
    reopen = rust_item_body(source, "pub fn reopen(")
    try_reopen = rust_item_body(source, "pub fn try_reopen_in_place(")
    stop_driver = rust_item_body(source, "fn stop_driver(")

    assert "drop(join.join())" not in source
    stop_start = source.find("fn stop_driver(")
    assert "AnyResult<()>" in source[stop_start : stop_start + 120]
    for outcome in ("Ok(Ok(()))", "Ok(Err(error))", "Err(panic)"):
        assert outcome in stop_driver, outcome
    assert "if let Err(error) = self.try_reopen_in_place()" in reopen
    assert "panic!" in reopen
    try_reopen_start = source.find("pub fn try_reopen_in_place(")
    assert "AnyResult<()>" in source[try_reopen_start : try_reopen_start + 120]
    assert re.fullmatch(r"\{\s*self\.stop_driver\(\)\s*\}", try_reopen)
    assert stop_driver.index("outcome?;") < stop_driver.index(
        "self.remove_socket_directory()"
    )


def test_activation_expectation_is_reloaded_from_daemon_control_authority_v1() -> None:
    source = read_harness()
    runtime = rust_item_body(source, "pub struct E2eRuntime {")
    activate = rust_item_body(source, "activate_last_sealed_generation(")
    current = rust_item_body(source, "fn current_search_corpus_head_from_control_v1(")

    assert "active_search_corpus_identity" not in runtime
    assert "current_search_corpus_head_from_control_v1" in activate
    assert "SearchPlaneControlIpcRequest::SearchCorpusActiveHead" in current
    assert "SearchPlaneControlIpcResponse::SearchCorpusActiveHeadObservation" in current
    assert "observation.repo_id() != repo_id" in current
    assert "observation.revision_id() != revision_id" in current
    assert "observation.into_head()" in current
    assert "active_search_corpus_identity" not in activate
