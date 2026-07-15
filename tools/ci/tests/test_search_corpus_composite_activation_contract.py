"""Static regression fence for the composite search-corpus activation cutover."""

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
CONTRACT_CONTROL = ROOT / "crates/quanta-index-contract/src/ipc/control.rs"
CONTRACT_SPLIT = ROOT / "crates/quanta-index-contract/src/ipc/split.rs"
DISPATCHER = ROOT / "crates/quanta-index-search-plane/src/control_dispatcher.rs"
READINESS = ROOT / "crates/quanta-index-search-plane/src/readiness.rs"
SDK_CLIENT = ROOT / "crates/quanta-index-sdk/src/client.rs"
SDK_CORPUS = ROOT / "crates/quanta-index-sdk/src/lexical.rs"
SDK_GENERATIONS = ROOT / "crates/quanta-index-sdk/src/generations.rs"
SDK_LIB = ROOT / "crates/quanta-index-sdk/src/lib.rs"
SDK_REPOMAP = ROOT / "crates/quanta-index-sdk/src/repomap.rs"
SDK_TESTS = ROOT / "crates/quanta-index-sdk/src/tests.rs"
ROLLBACK_PRODUCTION_ROOTS = (
    ROOT / "crates/quanta-index-contract/src",
    ROOT / "crates/quanta-index-sdk/src",
    ROOT / "crates/quanta-index-search-plane/src",
    ROOT / "crates/quanta-index-searchd-harness/src",
    ROOT / "crates/quanta-index-searchd-runtime/src",
    ROOT / "crates/quanta-index-searchctl/src",
)
IPC_REQUEST_FUZZ = (
    ROOT / "crates/quanta-index-contract/fuzz/fuzz_targets/ipc_request_decode.rs"
)
IPC_RESPONSE_FUZZ = (
    ROOT / "crates/quanta-index-contract/fuzz/fuzz_targets/ipc_response_decode.rs"
)
COMPOSITE_CONTROL_FUZZ_DICTIONARY = (
    ROOT
    / "crates/quanta-index-contract/fuzz/dictionaries/composite_search_corpus_control.dict"
)
JUSTFILE = ROOT / "Justfile"


def read_source(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def read_rust_corpus(roots: tuple[Path, ...]) -> str:
    return "\n".join(
        read_source(path)
        for root in roots
        for path in sorted(root.rglob("*.rs"))
    )


def test_single_track_activation_contract_and_dispatch_are_removed_v1() -> None:
    control = read_source(CONTRACT_CONTROL)
    split = read_source(CONTRACT_SPLIT)
    dispatcher = read_source(DISPATCHER)

    for legacy_symbol in (
        "SearchPlaneActivateGenerationRequest",
        "SearchPlaneActivateGenerationCasRequest",
        "SearchPlaneActivationAck",
        "SearchPlaneActivationCasAck",
    ):
        assert legacy_symbol not in control, legacy_symbol

    for legacy_variant_prefix in (
        "ActivateGeneration(",
        "ActivateGenerationCas(",
        "ActivationAck(",
        "ActivationCasAck(",
    ):
        assert not any(
            line.strip().startswith(legacy_variant_prefix)
            for line in split.splitlines()
        ), legacy_variant_prefix

    assert "ActivateGenerationCas(request)" not in dispatcher
    assert "ActivateGeneration(_request)" not in dispatcher


def test_sdk_only_emits_composite_activation_ingress_v1() -> None:
    client = read_source(SDK_CLIENT)
    corpus = read_source(SDK_CORPUS)

    assert "SearchCorpusGenerationIdentityV1" in client
    assert "SearchPlaneSearchCorpusActivationCasAck" in client
    assert "ActivateSearchCorpusGenerationCas" in corpus
    assert "SearchPlaneActivateGenerationCasRequest" not in corpus
    assert "SearchPlaneActivateGenerationRequest" not in corpus


def test_rollback_contract_is_composite_only_v1() -> None:
    control = read_source(CONTRACT_CONTROL)
    split = read_source(CONTRACT_SPLIT)
    generations = read_source(SDK_GENERATIONS)
    sdk_sources = "\n".join(
        read_source(path)
        for path in (SDK_CLIENT, SDK_CORPUS, SDK_GENERATIONS, SDK_LIB, SDK_REPOMAP)
    )
    production_sources = read_rust_corpus(ROLLBACK_PRODUCTION_ROOTS)

    for legacy_symbol in (
        "SearchPlaneRollbackGenerationRequest",
        "SearchPlaneRollbackGenerationAck",
    ):
        assert legacy_symbol not in control, legacy_symbol
        assert legacy_symbol not in sdk_sources, legacy_symbol
        assert legacy_symbol not in production_sources, legacy_symbol

    for legacy_variant_prefix in ("RollbackGeneration(", "RollbackAck("):
        assert not any(
            line.strip().startswith(legacy_variant_prefix)
            for line in split.splitlines()
        ), legacy_variant_prefix
        assert legacy_variant_prefix not in production_sources, legacy_variant_prefix

    for legacy_scalar_field in (
        "expected_active_generation",
        "expected_active_manifest_digest",
        "target_generation",
        "target_manifest_digest",
        "previous_generation",
        "previous_manifest_digest",
    ):
        assert legacy_scalar_field not in control, legacy_scalar_field
        assert legacy_scalar_field not in production_sources, legacy_scalar_field

    assert "SearchPlaneRollbackSearchCorpusGenerationCasRequest" in control
    assert "SearchPlaneSearchCorpusRollbackCasAck" in control
    assert "SearchCorpusRollbackValidationErrorV1" in control
    assert "impl SearchPlaneRollbackSearchCorpusGenerationCasRequest" in control
    assert "RollbackSearchCorpusGenerationCas" in split
    assert "SearchCorpusRollbackCasAck" in split
    assert "RollbackSearchCorpusGenerationCas" in sdk_sources
    assert "SearchCorpusRollbackCasAck" in sdk_sources

    for runtime_owner in (DISPATCHER, READINESS):
        source = read_source(runtime_owner)
        assert "request.validate_v1()" in source, runtime_owner
        assert "expected_active.repo_id() != target.repo_id()" not in source, runtime_owner
        assert "target.manifest_generation().get() >=" not in source, runtime_owner


def test_sdk_binds_composite_success_acks_to_requests_v1() -> None:
    corpus = read_source(SDK_CORPUS)
    generations = read_source(SDK_GENERATIONS)

    assert "validate_composite_activation_ack_v1" in corpus
    assert "validate_composite_rollback_request_v1" in generations
    assert "validate_composite_rollback_ack_v1" in generations
    assert "request.validate_v1()" in generations


def test_activation_relation_validation_is_contract_owned_v1() -> None:
    control = read_source(CONTRACT_CONTROL)
    corpus = read_source(SDK_CORPUS)

    assert "SearchCorpusActivationValidationErrorV1" in control
    assert "impl SearchPlaneActivateSearchCorpusGenerationCasRequest" in control
    for invariant in (
        "CandidateIdentity",
        "ExpectedActiveIdentity",
        "RepoMismatch",
        "RevisionMismatch",
        "CandidateGenerationMustAdvanceExpectedActive",
    ):
        assert invariant in control, invariant
    assert "request.validate_v1()" in corpus
    assert "validate_expected_search_corpus_identity_v1" not in corpus


def test_sdk_search_corpus_receipt_is_exact_bound_before_activation_v1() -> None:
    corpus = read_source(SDK_CORPUS)
    tests = read_source(SDK_TESTS)

    assert "validate_search_corpus_publish_receipt_v1" in corpus
    for field in (
        "generation",
        "manifest_digest",
        "sealed",
        "accepted_replace_scopes",
        "accepted_tombstone_scopes",
        "accepted_clear_surfaces",
    ):
        assert f"receipt.{field}" in corpus, field
    assert "producer_client_rejects_each_search_corpus_receipt_mismatch_before_activation_v1" in tests
    assert "control request" in tests


def test_sdk_control_request_id_mismatch_is_a_protocol_error_v1() -> None:
    tests = read_source(SDK_TESTS)

    assert "with_request_id_offset" in tests
    assert "control_request_id_mismatch_is_rejected_for_activation_and_rollback_v1" in tests


def test_existing_ipc_fuzz_targets_decode_composite_control_dtos_directly_v1() -> None:
    request_fuzz = read_source(IPC_REQUEST_FUZZ)
    response_fuzz = read_source(IPC_RESPONSE_FUZZ)
    for request_type in (
        "SearchCorpusGenerationIdentityV1",
        "SearchPlaneActivateSearchCorpusGenerationCasRequest",
        "SearchPlaneRollbackSearchCorpusGenerationCasRequest",
    ):
        assert re.search(
            rf"from_reader::<\s*{request_type}\s*,", request_fuzz
        ), request_type

    for response_type in (
        "SearchPlaneSearchCorpusActivationCasAck",
        "SearchPlaneSearchCorpusRollbackCasAck",
    ):
        assert re.search(
            rf"from_reader::<\s*{response_type}\s*,", response_fuzz
        ), response_type

    assert request_fuzz.count(".validate_v1()") >= 3
    assert response_fuzz.count(".validate_v1()") >= 4

    dictionary = read_source(COMPOSITE_CONTROL_FUZZ_DICTIONARY)
    for token in (
        "ActivateSearchCorpusGenerationCas",
        "RollbackSearchCorpusGenerationCas",
        "SearchCorpusActivationCasAck",
        "SearchCorpusRollbackCasAck",
        "expected_active",
        "candidate",
        "target",
    ):
        assert token in dictionary, token

    justfile = read_source(JUSTFILE)
    dictionary_flag = "-dict=dictionaries/composite_search_corpus_control.dict"
    for target in ("ipc_request_decode", "ipc_response_decode"):
        assert re.search(
            rf"cargo \+nightly fuzz run {target} -- {re.escape(dictionary_flag)} ",
            justfile,
        ), target
