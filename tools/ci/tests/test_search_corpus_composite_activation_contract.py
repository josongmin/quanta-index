"""Static regression fence for the composite search-corpus activation cutover."""

from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
CONTRACT_CONTROL = ROOT / "crates/quanta-index-contract/src/ipc/control.rs"
CONTRACT_SPLIT = ROOT / "crates/quanta-index-contract/src/ipc/split.rs"
DISPATCHER = ROOT / "crates/quanta-index-search-plane/src/control_dispatcher.rs"
SDK_CLIENT = ROOT / "crates/quanta-index-sdk/src/client.rs"
SDK_CORPUS = ROOT / "crates/quanta-index-sdk/src/lexical.rs"


def read_source(path: Path) -> str:
    return path.read_text(encoding="utf-8")


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
