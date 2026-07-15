"""Static TDD fence for harness generation activation authority.

The reusable E2E harness must exercise the same control UDS and composite CAS
contract as production.  A direct ``ActivationCatalog`` mutation would make
daemon tests green while bypassing ledger readiness and transport validation.
"""

from pathlib import Path
import re


ROOT = Path(__file__).resolve().parents[3]
HARNESS = ROOT / "crates/quanta-index-searchd-harness/src/harness.rs"


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
    activate = rust_item_body(source, "activate_last_sealed_generation(")

    assert "Option<SearchCorpusGenerationIdentityV1>" in runtime
    assert "SearchPlaneIngestIpcResponse::SearchCorpusReceipt(receipt)" in seal
    for receipt_field in ("receipt.sealed", "receipt.generation", "receipt.manifest_digest"):
        assert receipt_field in seal, receipt_field

    stored_identity = re.search(
        r"self\.(?P<field>[A-Za-z0-9_]*sealed[A-Za-z0-9_]*)\s*=\s*Some\(",
        seal,
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
    activate = rust_item_body(source, "activate_last_sealed_generation(")

    assert "response.request_id != request_id" in dispatch_control
    assert "SearchPlaneControlIpcResponse::Error" in dispatch_control
    assert "SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(ack)" in activate
    assert re.search(r"ack\.active\s*!=\s*candidate", activate)
    assert re.search(
        r"ack\.previous_sealed_active\s*!=\s*expected_active",
        activate,
    )
