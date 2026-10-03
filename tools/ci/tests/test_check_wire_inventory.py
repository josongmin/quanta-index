"""Tests for tools/ci/lint/check-wire-inventory.py."""

from __future__ import annotations

import importlib.util
import sys
import textwrap
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools" / "ci" / "lint" / "check-wire-inventory.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("check_wire_inventory", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["check_wire_inventory"] = module
    spec.loader.exec_module(module)
    return module


MODULE = _load_module()


SPLIT_RS = """
    #[derive(Clone, Debug, PartialEq)]
    pub enum SearchPlaneQueryIpcRequest {
        Text(TextQueryRequest),
        /// Read-only lookup.
        Explain(SearchPlaneExplainQueryRequest),
    }

    #[derive(Clone, Debug, PartialEq)]
    pub enum SearchPlaneQueryIpcResponse {
        Text(TextQueryResponse),
        Explain(SearchPlaneExplainQueryResponse),
        Error(SearchPlaneIpcError),
    }

    pub enum NotAnOpcode {
        Whatever(u8),
    }
"""

INGEST_RS = """
    pub enum SearchPlaneIngestIpcRequest {
        PublishSearchCorpusBatch(SearchCorpusIngestBatch),
    }

    pub enum SearchPlaneIngestIpcResponse {
        SearchCorpusReceipt(BatchPublishReceipt),
        Error(SearchPlaneIpcError),
    }
"""

LEXICAL_LIB_RS = """
    const LEXICAL_SEALED_MANIFEST_FORMAT_VERSION: u32 = 3;
    const MAX_WRITERS: u32 = 12;
    pub const ANN_LIBRARY_VERSION: &str = "0.30.0";
    #[cfg(test)]
    pub(crate) const LEGACY_FIXTURE_FORMAT_VERSION: u32 = 5;
    pub(crate) const TEXT_NORMALIZER_VERSION: TextNormalizerVersion =
        TextNormalizerVersion { major: 2, minor: 0 };
"""

INVENTORY_OK = """
    schema = 2

    [[ipc]]
    enum = "SearchPlaneQueryIpcRequest"
    file = "crates/quanta-index-contract/src/ipc/split.rs"
    plane = "query"
    direction = "request"
    variants = ["Text", "Explain"]

    [[ipc]]
    enum = "SearchPlaneQueryIpcResponse"
    file = "crates/quanta-index-contract/src/ipc/split.rs"
    plane = "query"
    direction = "response"
    variants = ["Text", "Explain", "Error"]

    [[ipc]]
    enum = "SearchPlaneIngestIpcRequest"
    file = "crates/quanta-index-contract/src/ipc/ingest/envelope.rs"
    plane = "ingest"
    direction = "request"
    variants = ["PublishSearchCorpusBatch"]

    [[ipc]]
    enum = "SearchPlaneIngestIpcResponse"
    file = "crates/quanta-index-contract/src/ipc/ingest/envelope.rs"
    plane = "ingest"
    direction = "response"
    variants = ["SearchCorpusReceipt", "Error"]

    [[artifact]]
    id = "lexical-sealed-manifest"
    owner = "quanta-index-lexical"
    path = "state_root/indexes/lexical/g<N>/search-corpus-generation-manifest.cbor"
    reproduction = "producer-rebuild"
    notes = "seal commitment"
    [[artifact.version_constants]]
    file = "crates/quanta-index-lexical/src/lib.rs"
    name = "LEXICAL_SEALED_MANIFEST_FORMAT_VERSION"
    value = 3

    [[artifact]]
    id = "lexical-text-normalizer-stamp"
    owner = "quanta-index-lexical"
    path = "(stamped)"
    reproduction = "producer-rebuild"
    notes = "normalizer contract"
    [[artifact.version_constants]]
    file = "crates/quanta-index-lexical/src/lib.rs"
    name = "TEXT_NORMALIZER_VERSION"
    value = "2.0"
"""


def _write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(textwrap.dedent(content), encoding="utf-8")


def make_workspace(tmp_path: Path) -> Path:
    root = tmp_path / "ws"
    _write(
        root / "Cargo.toml",
        """
        [workspace]
        members = ["crates/quanta-index-contract", "crates/quanta-index-lexical"]
        """,
    )
    _write(root / "crates/quanta-index-contract/Cargo.toml", "[package]\nname = 'c'\n")
    _write(root / "crates/quanta-index-lexical/Cargo.toml", "[package]\nname = 'l'\n")
    _write(root / "crates/quanta-index-contract/src/ipc/split.rs", SPLIT_RS)
    _write(root / "crates/quanta-index-contract/src/ipc/ingest/envelope.rs", INGEST_RS)
    _write(root / "crates/quanta-index-lexical/src/lib.rs", LEXICAL_LIB_RS)
    # A tests/ tree must be ignored even when it declares a format constant.
    _write(
        root / "crates/quanta-index-lexical/src/tests/fixture.rs",
        "const FIXTURE_FORMAT_VERSION: u32 = 99;\n",
    )
    return root


def load(text: str) -> dict:
    return MODULE.tomllib.loads(textwrap.dedent(text))


def messages(findings) -> list[str]:
    return [f.render() for f in findings]


def test_exact_inventory_passes(tmp_path: Path):
    root = make_workspace(tmp_path)
    assert MODULE.check(load(INVENTORY_OK), root) == []


def test_tools_owned_schema_version_is_checked(tmp_path: Path):
    root = make_workspace(tmp_path)
    _write(root / "tools/ci/owner.py", "# owner\n")
    _write(root / "tools/ci/decoder.py", "# decoder\n")
    _write(
        root / "tools/ci/schema.json",
        '{"properties":{"schema_version":{"const":2}}}\n',
    )
    inventory = load(INVENTORY_OK)
    inventory["tool_artifact"] = [
        {
            "id": "proof",
            "owner": "tools/ci/owner.py",
            "path": "artifacts/proof.json",
            "producer": "fixture",
            "consumers": ["fixture"],
            "version": 1,
            "schema_file": "tools/ci/schema.json",
            "decoder": "tools/ci/decoder.py",
            "compatibility": "refuse old",
            "migration_fixture": "old_refused",
            "notes": "fixture",
        }
    ]
    found = messages(MODULE.check(inventory, root))
    assert any("inventory version 1 differs from schema const 2" in message for message in found)


def test_multiversion_receipt_schema_requires_both_inventory_rows():
    inventory = MODULE.load_inventory()
    inventory["tool_artifact"] = [
        row
        for row in inventory["tool_artifact"]
        if row["id"] != "verification-receipt-v2-retrieval"
    ]
    findings = messages(MODULE.check_tool_artifacts(inventory, REPO_ROOT))
    assert any(
        "schema version 2 has no inventory row" in message
        and "verification-receipt.schema.json" in message
        for message in findings
    )


def test_malformed_tool_version_is_a_finding_not_a_checker_crash():
    inventory = MODULE.load_inventory()
    row = next(
        row
        for row in inventory["tool_artifact"]
        if row["id"] == "verification-receipt-v2-retrieval"
    )
    row["version"] = []
    findings = messages(MODULE.check_tool_artifacts(inventory, REPO_ROOT))
    assert any("`version` must be a positive integer" in message for message in findings)
    assert any("schema version 2 has no inventory row" in message for message in findings)


def test_multiversion_receipt_rejects_duplicate_version_claim():
    inventory = MODULE.load_inventory()
    row = next(
        row
        for row in inventory["tool_artifact"]
        if row["id"] == "verification-receipt-v2-retrieval"
    )
    inventory["tool_artifact"].append({**row, "id": "duplicate-v2"})
    findings = messages(MODULE.check_tool_artifacts(inventory, REPO_ROOT))
    assert any("schema version 2 is listed twice" in message for message in findings)


def test_ipc_enum_parser_reads_only_opcode_enums():
    enums = MODULE.parse_ipc_enums(textwrap.dedent(SPLIT_RS))
    assert enums == {
        "SearchPlaneQueryIpcRequest": ["Text", "Explain"],
        "SearchPlaneQueryIpcResponse": ["Text", "Explain", "Error"],
    }


def test_ipc_enum_parser_refuses_a_variant_shape_it_does_not_understand():
    source = """
        pub enum SearchPlaneControlIpcRequest {
            Ping,
        }
    """
    try:
        MODULE.parse_ipc_enums(textwrap.dedent(source))
    except ValueError as err:
        assert "unrecognized variant line" in str(err)
    else:
        raise AssertionError("a unit variant must be a finding, not a skipped line")


def test_a_code_variant_missing_from_the_inventory_fails(tmp_path: Path):
    root = make_workspace(tmp_path)
    inventory = load(INVENTORY_OK)
    inventory["ipc"][0]["variants"] = ["Text"]
    found = messages(MODULE.check(inventory, root))
    assert any("variant `Explain` exists in the code but is not listed" in m for m in found)


def test_an_inventory_variant_the_code_lacks_fails(tmp_path: Path):
    root = make_workspace(tmp_path)
    inventory = load(INVENTORY_OK)
    inventory["ipc"][0]["variants"] = ["Text", "Explain", "Ghost"]
    found = messages(MODULE.check(inventory, root))
    assert any("variant `Ghost` is listed but the enum has no such variant" in m for m in found)


def test_an_opcode_enum_without_an_inventory_row_fails(tmp_path: Path):
    root = make_workspace(tmp_path)
    inventory = load(INVENTORY_OK)
    inventory["ipc"] = [
        row for row in inventory["ipc"] if row["enum"] != "SearchPlaneIngestIpcResponse"
    ]
    found = messages(MODULE.check(inventory, root))
    assert any("`SearchPlaneIngestIpcResponse` has no [[ipc]] row" in m for m in found)


def test_a_wrong_file_for_an_enum_fails(tmp_path: Path):
    root = make_workspace(tmp_path)
    inventory = load(INVENTORY_OK)
    inventory["ipc"][2]["file"] = "crates/quanta-index-contract/src/ipc/split.rs"
    found = messages(MODULE.check(inventory, root))
    assert any(
        "the enum lives in 'crates/quanta-index-contract/src/ipc/ingest/envelope.rs'" in m for m in found
    )


def test_format_constants_are_read_with_test_and_non_format_names_excluded():
    constants = MODULE.parse_format_constants(
        "crates/x/src/lib.rs", textwrap.dedent(LEXICAL_LIB_RS)
    )
    assert [(c.name, c.value) for c in constants] == [
        ("LEXICAL_SEALED_MANIFEST_FORMAT_VERSION", "3"),
        ("TEXT_NORMALIZER_VERSION", "2.0"),
    ]


def test_a_format_constant_the_inventory_does_not_name_fails(tmp_path: Path):
    root = make_workspace(tmp_path)
    _write(
        root / "crates/quanta-index-lexical/src/extra.rs",
        "pub(crate) const HISTORY_EPOCH_FORMAT_VERSION: u32 = 1;\n",
    )
    found = messages(MODULE.check(load(INVENTORY_OK), root))
    assert any(
        "`HISTORY_EPOCH_FORMAT_VERSION` = 1 is not named by any [[artifact]]" in m for m in found
    )


def test_a_bumped_constant_without_an_inventory_update_fails(tmp_path: Path):
    root = make_workspace(tmp_path)
    _write(
        root / "crates/quanta-index-lexical/src/lib.rs",
        LEXICAL_LIB_RS.replace("FORMAT_VERSION: u32 = 3;", "FORMAT_VERSION: u32 = 4;"),
    )
    found = messages(MODULE.check(load(INVENTORY_OK), root))
    assert any("inventory says 3; the code declares '4'" in m for m in found)


def test_an_inventory_constant_the_code_lacks_fails(tmp_path: Path):
    root = make_workspace(tmp_path)
    inventory = load(INVENTORY_OK)
    inventory["artifact"][0]["version_constants"][0]["name"] = "GONE_FORMAT_VERSION"
    found = messages(MODULE.check(inventory, root))
    assert any("no format-version constant `GONE_FORMAT_VERSION`" in m for m in found)
    # The real constant is now unclaimed, which is its own finding.
    assert any("`LEXICAL_SEALED_MANIFEST_FORMAT_VERSION` = 3 is not named" in m for m in found)


def test_a_constant_claimed_twice_fails(tmp_path: Path):
    root = make_workspace(tmp_path)
    inventory = load(INVENTORY_OK)
    inventory["artifact"][1]["version_constants"].append(
        dict(inventory["artifact"][0]["version_constants"][0])
    )
    found = messages(MODULE.check(inventory, root))
    assert any("already claimed by artifact 'lexical-sealed-manifest'" in m for m in found)


def test_an_unknown_reproduction_class_fails(tmp_path: Path):
    root = make_workspace(tmp_path)
    inventory = load(INVENTORY_OK)
    inventory["artifact"][0]["reproduction"] = "somehow"
    found = messages(MODULE.check(inventory, root))
    assert any("`reproduction` must be one of" in m for m in found)


def test_retired_importer_reproduction_classes_fail(tmp_path: Path):
    root = make_workspace(tmp_path)
    for retired in ("offline-importer", "migration-input-only"):
        inventory = load(INVENTORY_OK)
        inventory["artifact"][0]["reproduction"] = retired
        found = messages(MODULE.check(inventory, root))
        assert any("`reproduction` must be one of" in m for m in found), retired


def test_an_unknown_owner_crate_fails(tmp_path: Path):
    root = make_workspace(tmp_path)
    inventory = load(INVENTORY_OK)
    inventory["artifact"][0]["owner"] = "quanta-index-nowhere"
    found = messages(MODULE.check(inventory, root))
    assert any("owner crate `quanta-index-nowhere` does not exist" in m for m in found)


def test_the_committed_inventory_matches_the_repository():
    assert MODULE.check(MODULE.load_inventory(), REPO_ROOT) == []
