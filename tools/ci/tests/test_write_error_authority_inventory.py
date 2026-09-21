"""Owner-local tests for ErrorAuthorityInventoryV1."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
WRITER_PATH = REPO_ROOT / "tools/ci/write-error-authority-inventory.py"
SCHEMA_PATH = REPO_ROOT / "tools/ci/error-authority-inventory.schema.json"


def _load_module():
    spec = importlib.util.spec_from_file_location("write_error_authority_inventory", WRITER_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


WRITER = _load_module()


def _fixture(tmp_path: Path, source: str) -> Path:
    root = tmp_path / "repo"
    source_path = root / "crates/example/src/lib.rs"
    source_path.parent.mkdir(parents=True)
    source_path.write_text(source, encoding="utf-8")
    schema = root / "tools/ci/error-authority-inventory.schema.json"
    schema.parent.mkdir(parents=True)
    shutil.copyfile(SCHEMA_PATH, schema)
    subprocess.run(["git", "-C", str(root), "init", "-q"], check=True)
    subprocess.run(["git", "-C", str(root), "config", "user.name", "Fixture"], check=True)
    subprocess.run(
        ["git", "-C", str(root), "config", "user.email", "fixture@example.invalid"],
        check=True,
    )
    subprocess.run(["git", "-C", str(root), "add", "."], check=True)
    subprocess.run(["git", "-C", str(root), "commit", "-qm", "fixture"], check=True)
    return root


def test_inventory_is_deterministic_and_open_paths_are_exact(tmp_path: Path) -> None:
    root = _fixture(
        tmp_path,
        "pub struct Error { pub code: String }\n"
        'fn build(code: &str) { let _ = CoreError::Typed { code: format!("X_{code}") }; }\n',
    )
    output = root / "artifacts/sep-21/p00/error-authority-inventory.json"

    first_path, first_digest, first_closed = WRITER.publish_inventory(
        root=root, output=output, require_closed=False
    )
    first_bytes = first_path.read_bytes()
    second_path, second_digest, second_closed = WRITER.publish_inventory(
        root=root, output=output, require_closed=True
    )

    payload = json.loads(second_path.read_text(encoding="utf-8"))
    counts = {category["id"]: category["count"] for category in payload["categories"]}
    assert first_bytes == second_path.read_bytes()
    assert first_digest == second_digest
    assert not first_closed
    assert not second_closed
    assert counts["core-error-typed-constructor"] == 1
    assert counts["free-form-code-string-field"] == 1
    assert counts["dynamic-code-format"] == 1
    assert counts["code-str-parameter"] == 1
    assert all(
        occurrence["path"] == "crates/example/src/lib.rs"
        for category in payload["categories"]
        for occurrence in category["occurrences"]
    )


def test_require_closed_never_promotes_regex_discovery_to_semantic_closure(
    tmp_path: Path,
) -> None:
    root = _fixture(
        tmp_path,
        "enum SearchPlaneErrorCodeV2 { Invalid }\n"
        "fn build() { let _ = CoreError::Typed { code: SearchPlaneErrorCodeV2::Invalid }; }\n",
    )
    output = root / "artifacts/sep-21/p00/error-authority-inventory.json"

    _, _, closed = WRITER.publish_inventory(root=root, output=output, require_closed=True)

    payload = json.loads(output.read_text(encoding="utf-8"))
    assert not closed
    assert payload["closed"] is False
    assert payload["p01_completion_requirements"]["manual_gates"]


def test_inventory_source_digest_changes_with_raw_source_bytes(tmp_path: Path) -> None:
    root = _fixture(tmp_path, "fn first() {}\n")
    before = WRITER.build_inventory(root)
    (root / "crates/example/src/lib.rs").write_text("fn second() {}\n", encoding="utf-8")

    after = WRITER.build_inventory(root)

    assert before["source_head"] == after["source_head"]
    assert before["source_digest"] != after["source_digest"]


def test_literal_prefilter_preserves_every_category_and_unicode_columns(tmp_path: Path) -> None:
    source = (
        "α SearchPlaneIpcError\n"
        "CoreError::Typed {\n"
        "code: String\n"
        'code: format!("X")\n'
        "code: &str\n"
        'error_code.contains("bad")\n'
        'code == "X"\n'
        "BAD_REQUEST\n"
    )
    root = _fixture(tmp_path, source)

    payload = WRITER.build_inventory(root)
    categories = {category["id"]: category for category in payload["categories"]}

    assert {category_id: category["count"] for category_id, category in categories.items()} == {
        category.id: 1 for category in WRITER.CATEGORIES
    }
    reference = categories["search-plane-ipc-error-reference"]["occurrences"][0]
    assert reference["line"] == 1
    assert reference["column"] == 3
    assert (
        reference["line_sha256"] == hashlib.sha256("α SearchPlaneIpcError\n".encode()).hexdigest()
    )

    (root / "crates/example/src/lib.rs").write_bytes(source.replace("\n", "\r\n").encode())
    crlf_payload = WRITER.build_inventory(root)
    assert crlf_payload["categories"] == payload["categories"]
    assert crlf_payload["source_digest"] != payload["source_digest"]


def test_inventory_refuses_output_escape(tmp_path: Path) -> None:
    root = _fixture(tmp_path, "fn main() {}\n")
    with pytest.raises(ValueError, match="inside the repository"):
        WRITER.publish_inventory(
            root=root,
            output=tmp_path / "outside.json",
            require_closed=False,
        )
