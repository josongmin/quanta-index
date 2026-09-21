from __future__ import annotations

import hashlib
import importlib.util
import json
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "tools/ci/check-search-plane-error-codes.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("check_search_plane_error_codes", SCRIPT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _copy_inputs(tmp_path: Path) -> Path:
    for relative in [
        "crates/quanta-index-contract/src/ipc/error.rs",
        "crates/quanta-index-contract/src/lex/error_code.rs",
        "crates/quanta-index-contract-base/src/query/top_k.rs",
        "tools/ci/search-plane-error-code-table.schema.json",
    ]:
        source = ROOT / relative
        target = tmp_path / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(source.read_bytes())
    return tmp_path


def test_source_table_is_sorted_unique_and_contains_mandatory_codes() -> None:
    module = _load_module()
    codes = module.source_codes(ROOT)
    assert codes == sorted(set(codes))
    assert {
        "BAD_REQUEST",
        "IDENTITY_EMPTY",
        "IDENTITY_CONTROL_CHARACTER",
        "SEQUENCE_EXHAUSTED",
        "OPERATION_REPLAY_FLOOR",
        "STATE_ROOT_SECURITY_POLICY_UNSUPPORTED",
        "PROTOCOL_VERSION_UNSUPPORTED",
    } - set(codes) == {"BAD_REQUEST"}


def test_enum_source_digest_uses_frozen_domain_and_payload_framing() -> None:
    module = _load_module()
    relative = module.ENUM_SOURCE.as_posix().encode()
    content = (ROOT / module.ENUM_SOURCE).read_bytes()
    domain = b"quanta-index/search-plane-error-enum-source/v2"
    expected = hashlib.sha256(
        len(domain).to_bytes(4, "big")
        + domain
        + len(relative).to_bytes(4, "big")
        + relative
        + len(content).to_bytes(8, "big")
        + content
    ).hexdigest()
    assert module.enum_source_digest(ROOT) == f"sha256:{expected}"


def test_committed_table_check_rejects_stale_content(tmp_path: Path) -> None:
    module = _load_module()
    root = _copy_inputs(tmp_path)
    module.write_table(root)
    table_path = root / module.TABLE
    table = json.loads(table_path.read_text(encoding="utf-8"))
    table["codes"] = table["codes"][:-1]
    table["cardinality"] -= 1
    table_path.write_text(json.dumps(table), encoding="utf-8")
    with pytest.raises(module.TableError, match="differs"):
        module.check_table(root)
