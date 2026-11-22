"""Owner-local tests for the registry-driven ProofManifestV1 writer."""

from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import platform
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

import jsonschema
import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
WRITER_PATH = REPO_ROOT / "tools/ci/write-proof-manifest.py"
CHECKER_PATH = REPO_ROOT / "tools/ci/lint/check-proof-authority.py"
REGISTRY_PATH = REPO_ROOT / "tools/ci/proof-authority.toml"
SCHEMA_PATH = REPO_ROOT / "tools/ci/proof-manifest.schema.json"
AGGREGATE_SCHEMA_PATH = REPO_ROOT / "tools/ci/proof-aggregate.schema.json"
ERROR_INVENTORY_WRITER_PATH = REPO_ROOT / "tools/ci/write-error-authority-inventory.py"


def _load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


WRITER = _load_module("write_proof_manifest", WRITER_PATH)
CHECKER = _load_module("write_proof_manifest_checker", CHECKER_PATH)
ERROR_INVENTORY_WRITER = _load_module(
    "write_proof_manifest_error_inventory", ERROR_INVENTORY_WRITER_PATH
)


@pytest.mark.parametrize(
    "raw",
    [
        b'{"status":"failed","status":"passed"}',
        b'{"counts":{"failed":1,"failed":0}}',
        b'{"items":[{"proof_id":"wrong","proof_id":"right"}]}',
        b'{"value":NaN}',
        b'{"value":Infinity}',
        b'{"value":-Infinity}',
        b'{"value":1e9999}',
    ],
)
def test_terminal_json_refuses_ambiguous_or_nonfinite_bytes(raw: bytes) -> None:
    with pytest.raises(WRITER.ManifestRefused, match="valid JSON"):
        WRITER._json_object_bytes(raw, label="terminal input")


def test_terminal_json_preserves_unambiguous_values() -> None:
    assert WRITER._json_object_bytes(
        b'{"status":"failed","counts":{"failed":1},"items":[null,true,1.25]}',
        label="terminal input",
    ) == {"status": "failed", "counts": {"failed": 1}, "items": [None, True, 1.25]}


def test_cross_repo_hellgate_selects_live_repomap_terminal_target() -> None:
    command = subprocess.run(
        ["just", "--dry-run", "rust-verify-hellgate-cross-repo"],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    ).stderr
    assert "./scripts/verify-repomap-cross-repo.sh" in command
    script = REPO_ROOT / "scripts/verify-repomap-cross-repo.sh"
    subprocess.run(["bash", "-n", str(script)], check=True, capture_output=True, text=True)
    source = script.read_text()
    # The typed runner owns the exact list/run selectors. Its independent
    # literal and behavior oracles live in test_paired_r5_result.py.
    target = '"$paired_python" "$quanta_root/tools/ci/paired_r5_result.py"'
    assert source.count(target) == 2
    build = source.index("just rust-build-release-native-daemon")
    assert source.index("--verify-resolution-only") < build
    assert "rust-build-release-daemon-fresh" not in source
    recipes = subprocess.check_output(["just", "--summary"], cwd=REPO_ROOT, text=True).split()
    assert "rust-build-release-daemon-fresh" not in recipes
    assert '--qbc-lane "$r5_lane"' in source
    assert "-- --list" not in source and "-- --exact --nocapture" not in source
    compare = source.index('binary_digest="$("$paired_python"')
    assert source.index('require_frozen_source "$quanta_root"') < build
    assert build < compare < source.index(target, compare)
    assert '--custody-binary "$custody_binary"' in source
    runner = source.rindex(target)
    assert source.rindex("require_binary_custody\n", compare, runner) > compare
    after_runner = (
        source[runner:].split('--kernel-resolution "$kernel_resolution"\n', 1)[1].splitlines()
    )
    assert len(after_runner) == 3
    assert all(line.startswith("printf 'paired-") for line in after_runner)


@dataclass(frozen=True)
class ManifestTemplates:
    root: Path
    paired_checkout: Path


def _run(root: Path, *args: str) -> None:
    subprocess.run([*args], cwd=root, check=True, capture_output=True, text=True)


def _digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _build_fixture_root(tmp_path: Path) -> tuple[Path, dict]:
    root = tmp_path / "repo"
    root.mkdir(parents=True)
    registry = CHECKER._read_toml(REGISTRY_PATH)
    registry_path = root / "tools/ci/proof-authority.toml"
    registry_path.parent.mkdir(parents=True)
    shutil.copyfile(REGISTRY_PATH, registry_path)
    shutil.copyfile(SCHEMA_PATH, root / "tools/ci/proof-manifest.schema.json")
    shutil.copyfile(REPO_ROOT / "tools/ci/proof_json.py", root / "tools/ci/proof_json.py")
    shutil.copyfile(AGGREGATE_SCHEMA_PATH, root / "tools/ci/proof-aggregate.schema.json")
    shutil.copyfile(
        ERROR_INVENTORY_WRITER_PATH, root / "tools/ci/write-error-authority-inventory.py"
    )
    shutil.copyfile(
        REPO_ROOT / "tools/ci/error-authority-inventory.schema.json",
        root / "tools/ci/error-authority-inventory.schema.json",
    )
    (root / ".gitignore").write_text("/artifacts/\n__pycache__/\n*.py[cod]\n", encoding="utf-8")

    test_authority = root / "tools/ci/test-authority.toml"
    shutil.copyfile(REPO_ROOT / "tools/ci/test-authority.toml", test_authority)
    shutil.copyfile(REPO_ROOT / "Justfile", root / "Justfile")
    for proof in registry["proofs"]:
        owner = root / proof["owner"]
        owner.parent.mkdir(parents=True, exist_ok=True)
        owner.touch(exist_ok=True)

    _run(root, "git", "init", "-q")
    _run(root, "git", "config", "user.name", "Proof Fixture")
    _run(root, "git", "config", "user.email", "proof@example.invalid")
    _run(root, "git", "add", ".")
    _run(root, "git", "commit", "-qm", "fixture")
    return root, registry


def _terminal(root: Path) -> tuple[Path, dict]:
    proof_dir = root / "artifacts/proof-authority/raw"
    proof_dir.mkdir(parents=True, exist_ok=True)
    evidence = proof_dir / "p00.log"
    evidence.write_text("proof authority: passed\n", encoding="utf-8")
    inventory = root / "artifacts/sep-21/p00/error-authority-inventory.json"
    inventory.parent.mkdir(parents=True, exist_ok=True)
    inventory.write_text(
        json.dumps(ERROR_INVENTORY_WRITER.build_inventory(root), sort_keys=True, indent=2) + "\n",
        encoding="utf-8",
    )
    terminal = {
        "status": "passed",
        "counts": {"selected": 1, "executed": 1, "passed": 1, "failed": 0, "ignored": 0},
        "environment": {
            "toolchain": "python fixture",
            "features": [],
            "os": platform.system().lower(),
            "arch": platform.machine(),
            "host": {
                "profile": "fixture",
                "cpu_count": 1,
                "memory_bytes": 1024,
                "identity": "fixture-host",
            },
        },
        "daemon_binary": None,
        "state_root_format": "not-applicable",
        "inputs": {
            "fixture": None,
            "corpus": None,
            "config": None,
            "model": None,
            "provider": None,
        },
        "started_at": "2026-09-21T00:00:00Z",
        "ended_at": "2026-09-21T00:00:01Z",
        "artifacts": [
            "artifacts/proof-authority/raw/p00.log",
            "artifacts/sep-21/p00/error-authority-inventory.json",
        ],
    }
    terminal_path = proof_dir / "terminal.json"
    terminal_path.write_text(json.dumps(terminal), encoding="utf-8")
    return terminal_path, terminal


def _add_nextest_run(root: Path, terminal: dict, *, package: str, binary: str) -> None:
    raw = root / "artifacts/proof-authority/raw"
    events = raw / "nextest.jsonl"
    inventory = raw / "nextest-inventory.json"
    name = f"{package}::{binary}$passes"
    metadata = {"crate": package, "test_binary": binary, "kind": "test"}
    rows = [
        {"type": "suite", "event": "started", "test_count": 1, "nextest": metadata},
        {"type": "test", "event": "started", "name": name},
        {"type": "test", "event": "ok", "name": name},
        {
            "type": "suite",
            "event": "ok",
            "passed": 1,
            "failed": 0,
            "ignored": 0,
            "nextest": metadata,
        },
    ]
    events.write_text("".join(json.dumps(row) + "\n" for row in rows), encoding="utf-8")
    inventory.write_text(
        json.dumps(
            {
                "test-count": 1,
                "rust-suites": {
                    f"{package}::{binary}": {
                        "package-name": package,
                        "binary-name": binary,
                        "kind": "test",
                        "status": "listed",
                        "testcases": {
                            "passes": {"ignored": False, "filter-match": {"status": "matches"}}
                        },
                    }
                },
            }
        ),
        encoding="utf-8",
    )
    event_relative = events.relative_to(root).as_posix()
    inventory_relative = inventory.relative_to(root).as_posix()
    terminal["artifacts"].extend([event_relative, inventory_relative])
    terminal["execution_result"] = {
        "schema_version": 1,
        "runs": [
            {
                "format": "nextest-jsonl",
                "events": event_relative,
                "inventory": inventory_relative,
            }
        ],
    }


def _publish(root: Path, terminal_path: Path) -> tuple[Path, str, str]:
    return WRITER.publish_manifest(
        root=root,
        registry_path=root / "tools/ci/proof-authority.toml",
        schema_path=root / "tools/ci/proof-manifest.schema.json",
        proof_id="p00-authority-freeze",
        terminal_input_path=terminal_path,
    )


def _build_paired_checkout(parent: Path) -> Path:
    checkout = parent / "arbitrary-local-directory"
    checkout.mkdir(parents=True)
    (checkout / "Cargo.lock").write_text("version = 4\n", encoding="utf-8")
    resolver_lock = checkout / CHECKER.PAIRED_DEPENDENCY_LOCK
    resolver_lock.parent.mkdir(parents=True)
    resolver_lock.write_text("version = 4\n# active resolver lock\n", encoding="utf-8")
    _run(checkout, "git", "init", "-q")
    _run(checkout, "git", "config", "user.name", "Pair Fixture")
    _run(checkout, "git", "config", "user.email", "pair@example.invalid")
    _run(
        checkout,
        "git",
        "remote",
        "add",
        "origin",
        "git@github-personal:josongmin/semantica-codegraph-v2.git",
    )
    _run(checkout, "git", "add", "Cargo.lock", CHECKER.PAIRED_DEPENDENCY_LOCK)
    _run(checkout, "git", "commit", "-qm", "fixture")
    return checkout


@pytest.fixture(scope="module")
def manifest_templates(tmp_path_factory: pytest.TempPathFactory) -> ManifestTemplates:
    base = tmp_path_factory.mktemp("proof-manifest-templates")
    root, _ = _build_fixture_root(base / "root")
    paired_checkout = _build_paired_checkout(base / "paired")
    return ManifestTemplates(root=root, paired_checkout=paired_checkout)


def _fixture_root(tmp_path: Path, templates: ManifestTemplates) -> tuple[Path, dict]:
    root = tmp_path / "repo"
    shutil.copytree(templates.root, root)
    registry = CHECKER._read_toml(root / "tools/ci/proof-authority.toml")
    return root, registry


def _paired_checkout(tmp_path: Path, templates: ManifestTemplates) -> Path:
    checkout = tmp_path / "arbitrary-local-directory"
    shutil.copytree(templates.paired_checkout, checkout)
    return checkout


def test_writer_resolves_registry_source_and_null_binary_then_semantically_validates(
    tmp_path: Path,
    manifest_templates: ManifestTemplates,
) -> None:
    root, registry = _fixture_root(tmp_path, manifest_templates)
    terminal_path, _ = _terminal(root)

    output, digest, status = _publish(root, terminal_path)

    payload = json.loads(output.read_text(encoding="utf-8"))
    proof = next(proof for proof in registry["proofs"] if proof["id"] == payload["proof_id"])
    schema = json.loads((root / "tools/ci/proof-manifest.schema.json").read_text())
    assert payload["invocation"] == {
        "command": proof["command"],
        "profile": proof["profile"],
        "target": proof["target"],
        "filter": proof["filter"],
    }
    assert payload["source"]["upstream"] is None
    assert payload["source"]["merge_base"] is None
    assert payload["daemon_binary"] is None
    assert digest == _digest(output)
    assert (
        output.relative_to(root)
        .as_posix()
        .startswith("artifacts/proof-authority/archive/p00-authority-freeze/")
    )
    assert (
        root / "artifacts/proof-authority/p00-authority-freeze.json"
    ).read_bytes() == output.read_bytes()
    assert status == "passed"
    assert (
        CHECKER.check_manifest(
            payload,
            manifest_path=output,
            proof=proof,
            schema=schema,
            root=root,
            bind_source=True,
        )
        == []
    )


def test_p00_manifest_refuses_missing_error_inventory_attestation(
    tmp_path: Path,
    manifest_templates: ManifestTemplates,
) -> None:
    root, _ = _fixture_root(tmp_path, manifest_templates)
    terminal_path, terminal = _terminal(root)
    terminal["artifacts"] = ["artifacts/proof-authority/raw/p00.log"]
    terminal_path.write_text(json.dumps(terminal), encoding="utf-8")

    with pytest.raises(WRITER.ManifestRefused, match="must attest.*error-authority inventory"):
        _publish(root, terminal_path)


def test_archive_publish_is_idempotent_and_refuses_byte_replacement(
    tmp_path: Path, manifest_templates: ManifestTemplates
) -> None:
    root, _ = _fixture_root(tmp_path, manifest_templates)
    terminal_path, _ = _terminal(root)

    first_path, first_digest, _ = _publish(root, terminal_path)
    second_path, second_digest, _ = _publish(root, terminal_path)

    assert second_path == first_path
    assert second_digest == first_digest
    original = first_path.read_bytes()
    first_path.write_bytes(original + b"tampered\n")
    with pytest.raises(WRITER.ManifestRefused, match="immutable proof archive leaf was modified"):
        _publish(root, terminal_path)


def test_archive_publish_refuses_a_deleted_indexed_leaf(
    tmp_path: Path, manifest_templates: ManifestTemplates
) -> None:
    root, _ = _fixture_root(tmp_path, manifest_templates)
    terminal_path, _ = _terminal(root)
    archive_path, _, _ = _publish(root, terminal_path)

    archive_path.unlink()

    with pytest.raises(WRITER.ManifestRefused, match="immutable proof archive leaf was deleted"):
        _publish(root, terminal_path)


def test_archive_publish_refuses_a_symlinked_archive_parent(
    tmp_path: Path, manifest_templates: ManifestTemplates
) -> None:
    root, _ = _fixture_root(tmp_path, manifest_templates)
    terminal_path, _ = _terminal(root)
    outside = tmp_path / "outside"
    outside.mkdir()
    archive_root = root / "artifacts/proof-authority/archive"
    archive_root.symlink_to(outside, target_is_directory=True)

    with pytest.raises(WRITER.ManifestRefused, match="proof output parent is unsafe"):
        _publish(root, terminal_path)

    assert list(outside.iterdir()) == []


def test_dependency_alias_refuses_same_byte_in_repo_symlink(tmp_path: Path) -> None:
    root = tmp_path / "repo"
    root.mkdir()
    payload = {
        "proof_id": "upstream",
        "source": {"head": "a" * 40},
        "source_pair": None,
    }
    content = (json.dumps(payload, sort_keys=True) + "\n").encode()
    target = root / "real.json"
    target.write_bytes(content)
    alias = root / "upstream.json"
    alias.symlink_to(target.name)
    digest = _digest(target)
    archive_relative = CHECKER.proof_archive_relative_path(payload, digest)
    archive = root / archive_relative
    archive.parent.mkdir(parents=True)
    archive.write_bytes(content)

    with pytest.raises(WRITER.ManifestRefused, match="non-symlink|regular"):
        WRITER._resolve_dependencies(
            root,
            {"dependencies": ["upstream"]},
            {"upstream": {"artifact": alias.name}},
            CHECKER,
        )


def test_digest_input_refuses_same_byte_in_repo_symlink(tmp_path: Path) -> None:
    root = tmp_path / "repo"
    root.mkdir()
    target = root / "real.txt"
    target.write_text("same bytes", encoding="utf-8")
    (root / "alias.txt").symlink_to(target.name)

    with pytest.raises(WRITER.ManifestRefused, match="non-symlink|regular"):
        WRITER._digest_input(root, {"path": "alias.txt"}, label="fixture")


def test_current_alias_refuses_in_repo_symlink_before_publication(
    tmp_path: Path, manifest_templates: ManifestTemplates
) -> None:
    root, _ = _fixture_root(tmp_path, manifest_templates)
    terminal_path, _ = _terminal(root)
    alias = root / "artifacts/proof-authority/p00-authority-freeze.json"
    redirect = alias.with_name("redirect.json")
    redirect.write_bytes(b"do not overwrite\n")
    alias.symlink_to(redirect.name)

    with pytest.raises(WRITER.ManifestRefused, match="non-symlink|unsafe"):
        _publish(root, terminal_path)

    assert redirect.read_bytes() == b"do not overwrite\n"


def test_evidence_source_refuses_same_byte_in_repo_symlink(
    tmp_path: Path, manifest_templates: ManifestTemplates
) -> None:
    root, _ = _fixture_root(tmp_path, manifest_templates)
    terminal_path, _ = _terminal(root)
    evidence = root / "artifacts/proof-authority/raw/p00.log"
    target = evidence.with_name("same.log")
    target.write_bytes(evidence.read_bytes())
    evidence.unlink()
    evidence.symlink_to(target.name)

    with pytest.raises(WRITER.ManifestRefused, match="regular non-symlink repo file"):
        _publish(root, terminal_path)


def test_archive_index_refuses_same_byte_in_repo_symlink(
    tmp_path: Path, manifest_templates: ManifestTemplates
) -> None:
    root, _ = _fixture_root(tmp_path, manifest_templates)
    terminal_path, _ = _terminal(root)
    archive_path, _, _ = _publish(root, terminal_path)
    index = archive_path.parent / "index.json"
    target = index.with_name("index-copy.json")
    target.write_bytes(index.read_bytes())
    index.unlink()
    index.symlink_to(target.name)

    with pytest.raises(WRITER.ManifestRefused, match="existing proof output is unsafe"):
        _publish(root, terminal_path)


def test_archive_leaf_refuses_same_byte_in_repo_symlink(
    tmp_path: Path, manifest_templates: ManifestTemplates
) -> None:
    root, _ = _fixture_root(tmp_path, manifest_templates)
    terminal_path, _ = _terminal(root)
    archive_path, _, _ = _publish(root, terminal_path)
    target = archive_path.with_name("copy.json")
    target.write_bytes(archive_path.read_bytes())
    archive_path.unlink()
    archive_path.symlink_to(target.name)

    with pytest.raises(WRITER.ManifestRefused, match="existing proof output is unsafe"):
        _publish(root, terminal_path)


def test_archive_parent_swap_cannot_redirect_publication(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    manifest_templates: ManifestTemplates,
) -> None:
    root, _ = _fixture_root(tmp_path, manifest_templates)
    terminal_path, _ = _terminal(root)
    outside = tmp_path / "outside"
    outside.mkdir()
    original = WRITER._require_parent_identity
    swapped = False

    def swap_before_archive_link(root_arg, relative, parent_fd, *, checker):
        nonlocal swapped
        if not swapped and "/proof-authority/evidence/" in relative:
            parent = root_arg / relative
            detached = tmp_path / "detached-archive-parent"
            parent.parent.rename(detached)
            parent.parent.symlink_to(outside, target_is_directory=True)
            swapped = True
        return original(root_arg, relative, parent_fd, checker=checker)

    monkeypatch.setattr(WRITER, "_require_parent_identity", swap_before_archive_link)
    with pytest.raises(WRITER.ManifestRefused, match="parent changed or is unsafe"):
        _publish(root, terminal_path)

    assert swapped
    assert list(outside.iterdir()) == []


def test_current_alias_parent_swap_cannot_redirect_publication(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    manifest_templates: ManifestTemplates,
) -> None:
    root, _ = _fixture_root(tmp_path, manifest_templates)
    terminal_path, _ = _terminal(root)
    outside = tmp_path / "outside"
    outside.mkdir()
    original = WRITER._require_parent_identity
    swapped = False

    def swap_before_alias_replace(root_arg, relative, parent_fd, *, checker):
        nonlocal swapped
        if not swapped and relative == "artifacts/proof-authority/p00-authority-freeze.json":
            parent = (root_arg / relative).parent
            parent.rename(tmp_path / "detached-alias-parent")
            parent.symlink_to(outside, target_is_directory=True)
            swapped = True
        return original(root_arg, relative, parent_fd, checker=checker)

    monkeypatch.setattr(WRITER, "_require_parent_identity", swap_before_alias_replace)
    with pytest.raises(WRITER.ManifestRefused, match="parent changed or is unsafe"):
        _publish(root, terminal_path)

    assert swapped
    assert list(outside.iterdir()) == []


def test_historical_manifest_keeps_content_addressed_evidence_after_retry(
    tmp_path: Path,
    manifest_templates: ManifestTemplates,
) -> None:
    root, registry = _fixture_root(tmp_path, manifest_templates)
    terminal_path, _ = _terminal(root)
    first_path, _, _ = _publish(root, terminal_path)
    first_payload = json.loads(first_path.read_text(encoding="utf-8"))

    (root / "artifacts/proof-authority/raw/p00.log").write_text(
        "proof authority: passed retry\n", encoding="utf-8"
    )
    second_path, _, _ = _publish(root, terminal_path)

    proof = next(item for item in registry["proofs"] if item["id"] == "p00-authority-freeze")
    schema = json.loads((root / "tools/ci/proof-manifest.schema.json").read_text())
    assert second_path != first_path
    assert (
        CHECKER.check_manifest(
            first_payload,
            manifest_path=first_path,
            proof=proof,
            schema=schema,
            root=root,
            bind_source=True,
        )
        == []
    )


def test_passed_manifest_refuses_dirty_product_source(
    tmp_path: Path, manifest_templates: ManifestTemplates
) -> None:
    root, _ = _fixture_root(tmp_path, manifest_templates)
    terminal_path, _ = _terminal(root)
    owner = root / "docs/adr/SEP-21-DECISION-REGISTRY.md"
    owner.write_text("dirty source\n", encoding="utf-8")

    with pytest.raises(
        WRITER.ManifestRefused, match="passed proof requires a clean primary source"
    ):
        _publish(root, terminal_path)


@pytest.mark.parametrize("terminal_status", ["failed", "blocked", "not_run"])
def test_non_passed_terminal_state_is_preserved_but_cli_returns_nonzero(
    tmp_path: Path,
    terminal_status: str,
    manifest_templates: ManifestTemplates,
) -> None:
    root, _ = _fixture_root(tmp_path, manifest_templates)
    terminal_path, terminal = _terminal(root)
    output = root / "artifacts/proof-authority/p00-authority-freeze.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(b"existing-authoritative-receipt\n")
    terminal["status"] = terminal_status
    terminal["counts"] = (
        {"selected": 1, "executed": 1, "passed": 0, "failed": 1, "ignored": 0}
        if terminal_status == "failed"
        else {"selected": 0, "executed": 0, "passed": 0, "failed": 0, "ignored": 0}
    )
    terminal_path.write_text(json.dumps(terminal), encoding="utf-8")

    exit_code = WRITER.main(
        [
            "--root",
            str(root),
            "--registry",
            str(root / "tools/ci/proof-authority.toml"),
            "--schema",
            str(root / "tools/ci/proof-manifest.schema.json"),
            "--proof-id",
            "p00-authority-freeze",
            "--terminal-input",
            str(terminal_path),
        ]
    )

    assert exit_code == 1
    assert json.loads(output.read_text(encoding="utf-8"))["status"] == terminal_status


def test_unknown_terminal_state_is_refused_without_replacing_receipt(
    tmp_path: Path,
    manifest_templates: ManifestTemplates,
) -> None:
    root, _ = _fixture_root(tmp_path, manifest_templates)
    terminal_path, terminal = _terminal(root)
    output = root / "artifacts/proof-authority/p00-authority-freeze.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(b"existing-authoritative-receipt\n")
    terminal["status"] = "NOT_RUN"
    terminal_path.write_text(json.dumps(terminal), encoding="utf-8")

    with pytest.raises(WRITER.ManifestRefused, match="terminal status is not registered"):
        _publish(root, terminal_path)

    assert output.read_bytes() == b"existing-authoritative-receipt\n"


def test_free_form_invocation_is_refused_instead_of_overriding_registry(
    tmp_path: Path,
    manifest_templates: ManifestTemplates,
) -> None:
    root, _ = _fixture_root(tmp_path, manifest_templates)
    terminal_path, terminal = _terminal(root)
    terminal["invocation"] = {"command": "printf fake-green"}
    terminal_path.write_text(json.dumps(terminal), encoding="utf-8")

    with pytest.raises(WRITER.ManifestRefused, match=r"extra=\['invocation'\]"):
        _publish(root, terminal_path)

    assert not (root / "artifacts/proof-authority/p00-authority-freeze.json").exists()


def test_semantic_failure_is_validated_before_atomic_replace(
    tmp_path: Path,
    manifest_templates: ManifestTemplates,
) -> None:
    root, _ = _fixture_root(tmp_path, manifest_templates)
    terminal_path, terminal = _terminal(root)
    output = root / "artifacts/proof-authority/p00-authority-freeze.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(b"existing-authoritative-receipt\n")
    terminal["counts"] = {
        "selected": 0,
        "executed": 0,
        "passed": 0,
        "failed": 0,
        "ignored": 0,
    }
    terminal_path.write_text(json.dumps(terminal), encoding="utf-8")

    with pytest.raises(WRITER.ManifestRefused, match="semantic manifest validation failed"):
        _publish(root, terminal_path)

    assert output.read_bytes() == b"existing-authoritative-receipt\n"
    assert list(output.parent.glob(f".{output.name}.*.tmp")) == []


@pytest.mark.parametrize("prior_bytes", [None, b"prior-authoritative-manifest\n"])
def test_postpublication_source_failure_restores_prior_manifest(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    prior_bytes: bytes | None,
    manifest_templates: ManifestTemplates,
) -> None:
    root, _ = _fixture_root(tmp_path, manifest_templates)
    terminal_path, _ = _terminal(root)
    output = root / "artifacts/proof-authority/p00-authority-freeze.json"
    if prior_bytes is not None:
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(prior_bytes)
    original_check_manifest = CHECKER.check_manifest
    calls = 0

    def mutate_after_prepublication_check(*args, **kwargs):
        nonlocal calls
        findings = original_check_manifest(*args, **kwargs)
        calls += 1
        if calls == 1:
            with (root / "tools/ci/test-authority.toml").open("a", encoding="utf-8") as handle:
                handle.write("\n# changed during manifest publication\n")
        return findings

    monkeypatch.setattr(WRITER, "_load_checker", lambda: CHECKER)
    monkeypatch.setattr(CHECKER, "check_manifest", mutate_after_prepublication_check)

    with pytest.raises(WRITER.ManifestRefused, match="proof inputs changed at publication"):
        _publish(root, terminal_path)

    assert calls == 2
    if prior_bytes is None:
        assert not output.exists()
    else:
        assert output.read_bytes() == prior_bytes


def test_binary_binding_is_derived_and_none_rejects_a_daemon_path(
    tmp_path: Path,
    manifest_templates: ManifestTemplates,
) -> None:
    root, registry = _fixture_root(tmp_path, manifest_templates)
    daemon = root / "bin/searchd"
    daemon.parent.mkdir()
    daemon.write_bytes(b"release-daemon")
    proof_none = next(proof for proof in registry["proofs"] if proof["binary_binding"] == "none")
    proof_release = next(
        proof for proof in registry["proofs"] if proof["binary_binding"] == "release-daemon"
    )

    with pytest.raises(WRITER.ManifestRefused, match="requires terminal.daemon_binary=null"):
        WRITER._resolve_binary(root, proof_none, "bin/searchd", checker=CHECKER)
    binary_digest = _digest(daemon)
    assert WRITER._resolve_binary(root, proof_release, "bin/searchd", checker=CHECKER) == {
        "source_path": "bin/searchd",
        "path": CHECKER.content_archive_relative_path("binary", binary_digest),
        "sha256": binary_digest,
    }


def test_exact_pair_is_derived_from_live_external_checkout_without_persisting_path(
    tmp_path: Path,
    manifest_templates: ManifestTemplates,
) -> None:
    checkout = _paired_checkout(tmp_path, manifest_templates)
    proof = {
        "source_binding": "exact-pair",
        "paired_repository": "github:josongmin/semantica-codegraph-v2",
        "paired_dependency_lock": "Cargo.lock",
    }

    source_pair = WRITER._resolve_source_pair(proof, str(checkout), checker=CHECKER)

    assert source_pair["repository"] == "github:josongmin/semantica-codegraph-v2"
    assert (
        source_pair["source"]["head"]
        == subprocess.check_output(
            ["git", "-C", str(checkout), "rev-parse", "HEAD"],
            text=True,
        ).strip()
    )
    assert source_pair["dependency_lock"] == {
        "path": "Cargo.lock",
        "sha256": _digest(checkout / "Cargo.lock"),
    }
    assert str(checkout) not in json.dumps(source_pair, sort_keys=True)
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    jsonschema.Draft202012Validator(schema["properties"]["source_pair"]).validate(source_pair)


@pytest.mark.parametrize("scenario", ["complete", "omit-r5", "missing-binding", "advance-head"])
def test_p12a_manifest_issues_without_p11_or_paired_checkout(
    tmp_path: Path,
    manifest_templates: ManifestTemplates,
    scenario: str,
) -> None:
    from tools.ci.tests.p11_test_issuer import bind_run, pytest_bytes

    root, _ = _fixture_root(tmp_path, manifest_templates)
    terminal_path, terminal = _terminal(root)
    catalog = CHECKER._read_toml(root / "tools/ci/test-authority.toml")
    paths = {item["id"]: item["path"] for item in catalog["python_targets"]}
    registry = CHECKER._read_toml(root / "tools/ci/proof-authority.toml")
    proof = next(item for item in registry["proofs"] if item["id"] == "p12a-proof-infrastructure")
    selectors = [paths[target] for target in proof["test_authority_targets"]]
    if scenario == "omit-r5":
        selectors = [path for path in selectors if path != "tools/ci/tests/test_paired_r5_result.py"]
    raw = root / "artifacts/proof-authority/raw"
    inventory = raw / "p12a-inventory.json"
    junit = raw / "p12a-junit.xml"
    listed, events = pytest_bytes(selectors)
    inventory.write_bytes(listed)
    junit.write_bytes(events)
    terminal["counts"] = {
        "selected": len(selectors),
        "executed": len(selectors),
        "passed": len(selectors),
        "failed": 0,
        "ignored": 0,
    }
    terminal["artifacts"].extend(
        [
            "artifacts/proof-authority/raw/p12a-inventory.json",
            "artifacts/proof-authority/raw/p12a-junit.xml",
        ]
    )
    terminal["execution_result"] = {
        "schema_version": 1,
        "runs": [
            {
                "format": "pytest-junit",
                "events": "artifacts/proof-authority/raw/p12a-junit.xml",
                "inventory": "artifacts/proof-authority/raw/p12a-inventory.json",
            }
        ],
    }
    binding = "artifacts/proof-authority/raw/p12a-run-binding.json"
    if scenario != "missing-binding":
        bind_run(root, proof, terminal["execution_result"]["runs"][0], binding)
        terminal["artifacts"].append(binding)
    terminal_path.write_text(json.dumps(terminal), encoding="utf-8")
    kwargs = {
        "root": root,
        "registry_path": root / "tools/ci/proof-authority.toml",
        "schema_path": root / "tools/ci/proof-manifest.schema.json",
        "proof_id": "p12a-proof-infrastructure",
        "terminal_input_path": terminal_path,
    }
    refusals = {
        "omit-r5": "no passing runner case: proof-paired-r5",
        "missing-binding": "execution run does not match its registered source binding",
        "advance-head": "captured execution source differs",
    }
    if scenario in refusals:
        if scenario == "advance-head":
            (root / proof["owner"]).write_text("# Different source after the run.\n")
            _run(root, "git", "add", "--", proof["owner"])
            _run(root, "git", "commit", "-qm", "Advance source after P12A execution")
        with pytest.raises(WRITER.ManifestRefused, match=refusals[scenario]):
            WRITER.publish_manifest(**kwargs, paired_checkout=None)
        assert not (root / proof["artifact"]).exists()
        return
    WRITER.publish_manifest(**kwargs, paired_checkout=None)
    payload = json.loads(
        (root / "artifacts/proof-authority/p12a-proof-infrastructure.json").read_text()
    )
    assert payload["status"] == "passed"
    assert payload["source_pair"] is None
    assert payload["dependency_receipts"] == []
    manifest = root / proof["artifact"]
    archived = root / CHECKER.proof_archive_relative_path(payload, _digest(manifest))
    original = json.loads(archived.read_bytes())
    (root / proof["owner"]).write_text("# Advance source after issuance.\n")
    _run(root, "git", "add", "--", proof["owner"])
    _run(root, "git", "commit", "-qm", "Advance source after P12A publication")
    replay = {
        "root": root,
        "proof": proof,
        "manifest_path": archived,
        "bind_source": False,
    }
    assert CHECKER.check_manifest(original, **replay) == []
    original["source"] = CHECKER.proof_source_snapshot(
        root, manifest_path=manifest, proof=proof,
    )
    assert any(
        "captured execution source differs" in finding.message
        for finding in CHECKER.check_manifest(original, **replay)
    )


def test_exact_pair_manifest_is_live_bound_through_atomic_writer(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from tools.ci.tests import test_write_proof_aggregate as release_fixture

    root, registry = release_fixture._build_fixture_root(tmp_path, executable=True)
    checkout = release_fixture._build_paired_checkout(tmp_path)
    release_fixture._write_dependency_manifests(root, registry, checkout)
    by_id = {entry["id"]: entry for entry in registry["proofs"]}
    proof = by_id[release_fixture.PAIR_PROOF]
    fixture = json.loads((root / proof["artifact"]).read_text())
    from tools.ci import paired_r5_release as release
    from tools.ci.tests.p11_test_issuer import sign_statement

    monkeypatch.setattr(WRITER.platform, "system", lambda: "Linux")
    monkeypatch.setattr(WRITER.platform, "machine", lambda: fixture["environment"]["arch"])
    env = {**fixture["environment"], "host": release_fixture.RELEASE_HOST_INPUT}
    monkeypatch.setattr(release, "observed_host_environment", lambda: env.copy())
    monkeypatch.setattr(release.p11_attestation, "sign", sign_statement)
    source = root / "artifacts/proof-authority/raw/pair"
    candidate = json.loads((source / release.INDEX).read_bytes())
    p10 = json.loads((root / by_id["p10-state-migration"]["artifact"]).read_bytes())
    admission = release.admit_release(root, checkout, None, preflight_signer=False)
    assert admission["binary_path"] == p10["daemon_binary"]["path"]
    assert admission["binding"]["daemon_sha256"] == p10["daemon_binary"]["sha256"]
    assert admission["binding"]["state_root_format"] == p10["state_root_format"]
    with pytest.raises(ValueError, match="different daemon or state-root format"):
        release.admit_release(root, checkout, "0" * 64, preflight_signer=False)

    dependency = root / admission["binding"]["dependency_receipts"][0]["path"]
    dependency_bytes = dependency.read_bytes()
    with monkeypatch.context() as patch:
        writer = release._writer()
        resolve = writer._resolve_dependencies

        def replace_resolved_receipt(*args):
            receipts = resolve(*args)
            dependency.write_bytes(dependency_bytes + b"\n")
            return receipts

        patch.setattr(writer, "_resolve_dependencies", replace_resolved_receipt)
        try:
            with pytest.raises(ValueError, match="artifact digest mismatch"):
                release.admit_release(root, checkout, None, preflight_signer=False)
        finally:
            dependency.write_bytes(dependency_bytes)
    from types import SimpleNamespace

    physical_reads = []

    def read_run(path):
        physical_reads.append(path)
        raw = path.read_bytes()
        receipt = json.loads(raw)
        stdout, stderr = (path.with_name(n).read_bytes() for n in ("stdout.log", "stderr.log"))

        def digest(blob):
            return hashlib.sha256(blob).hexdigest()

        custody = {
            "lane_id": receipt["lane"],
            "run_id": receipt["last_run_id_v1"],
            "receipt_path": str(path),
            "receipt_sha256": digest(raw),
            "execution_root": receipt["lane_execution_root_v1"],
            "target_root": receipt["target_dir"],
            "stdout_path": str(path.with_name("stdout.log")),
            "stdout_sha256": digest(stdout),
            "stderr_path": str(path.with_name("stderr.log")),
            "stderr_sha256": digest(stderr),
        }
        return SimpleNamespace(
            run=SimpleNamespace(
                run_id=receipt["last_run_id_v1"],
                result_directory=path.parent,
                command_cwd=Path(receipt["command_cwd_v1"]),
                execution_root=Path(receipt["lane_execution_root_v1"]),
                command=tuple(receipt["command"]),
                lane=receipt["lane"],
                exit_code=receipt["last_exit_code"],
            ),
            receipt_payload_v1=lambda: receipt,
            receipt_bytes=raw,
            receipt_digest=digest(raw),
            process_exit_code_v1=receipt["command_exit_code_v1"],
            stdout=stdout,
            stderr=stderr,
            custody_payload_v1=lambda: custody,
        )

    owner = SimpleNamespace(campaign_source_snapshot_digest_v1=lambda _: "a" * 64)
    monkeypatch.setattr(
        release,
        "_qbc_modules",
        lambda _: (
            SimpleNamespace(read_completed_run_receipt_at_path_v1=read_run),
            None,
            SimpleNamespace(_campaign_cargo_admission_owner_v1=lambda: owner),
        ),
    )

    def finalize(check):
        def check_boundary(*_, archive, cases):
            assert archive == root / p10["daemon_binary"]["path"]
            assert cases == release.r5_cases(proof)
            check()

        monkeypatch.setattr(release, "verify_pair_boundary", check_boundary)
        return release.prepare_terminal(
            root, checkout, source, candidate, admission, lane="fixture"
        )

    def refuse_final_check():
        raise ValueError("fixture final resolver changed")

    raw_root = (root / proof["artifact"]).parent / "raw"
    with pytest.raises(ValueError, match="final resolver changed"):
        finalize(refuse_final_check)
    assert not list(raw_root.glob("paired-*/*.sig"))
    assert len(set(physical_reads)) == 4

    def replace_p10_during_final_resolver():
        dependency.write_bytes(dependency_bytes + b"\n")

    try:
        with pytest.raises(ValueError, match="current alias differs from its immutable archive"):
            finalize(replace_p10_during_final_resolver)
        assert not list(raw_root.glob("paired-*/*.sig"))
        assert not list(raw_root.glob("paired-*/terminal.json"))
    finally:
        dependency.write_bytes(dependency_bytes)

    for path, refusal in [
        (dependency, "current alias differs from its immutable archive"),
        (root / admission["binary_path"], "release daemon bytes changed during proof"),
        (checkout / proof["paired_dependency_lock"], "paired release inputs changed before publication"),
    ]:
        with monkeypatch.context() as temporary:
            prior = path.read_bytes()

            def replace_during_signing(*args, target=path, old=prior, **kwargs):
                signature = sign_statement(*args, **kwargs)
                target.write_bytes(old + b"\n")
                return signature

            temporary.setattr(release.p11_attestation, "sign", replace_during_signing)
            try:
                with pytest.raises(ValueError, match=refusal):
                    finalize(lambda: None)
                assert not list(raw_root.glob("paired-*/*.sig"))
                assert not list(raw_root.glob("paired-*/terminal.json"))
            finally:
                path.write_bytes(prior)

    with monkeypatch.context() as temporary:
        prior_candidates = set(raw_root.glob("paired-*"))

        def replace_staged_output_during_signing(*args, **kwargs):
            signature = sign_statement(*args, **kwargs)
            (candidate_dir,) = set(raw_root.glob("paired-*")) - prior_candidates
            (candidate_dir / "caller-run-stdout.log").write_bytes(b"changed staged output")
            return signature

        temporary.setattr(release.p11_attestation, "sign", replace_staged_output_during_signing)
        with pytest.raises(ValueError, match="staged QBC evidence changed during signing"):
            finalize(lambda: None)
        assert not list(raw_root.glob("paired-*/*.sig"))
        assert not list(raw_root.glob("paired-*/terminal.json"))

    stream = physical_reads[-1].with_name("stdout.log")
    original = stream.read_bytes()
    stream.write_bytes(b"changed original output")
    with pytest.raises(ValueError, match="physical completed run"):
        finalize(lambda: None)
    assert not list(raw_root.glob("paired-*/*.sig"))
    stream.write_bytes(original)
    with monkeypatch.context() as temporary:

        def refuse_signer(*_, **__):
            raise OSError("fixture signing service unavailable")

        temporary.setattr(release.p11_attestation, "sign", refuse_signer)
        with pytest.raises(OSError, match="signing service unavailable"):
            finalize(lambda: None)
        assert not list(raw_root.glob("paired-*/*.sig"))
        assert not list(raw_root.glob("paired-*/terminal.json"))
    with monkeypatch.context() as temporary:

        def refuse_signature_write(*_):
            raise OSError("fixture signature storage error")

        temporary.setattr(release, "_write_output_temporary", refuse_signature_write)
        with pytest.raises(OSError, match="signature storage error"):
            finalize(lambda: None)
        assert not list(raw_root.glob("paired-*/*.sig"))
        assert not list(raw_root.glob("paired-*/terminal.json"))
    with monkeypatch.context() as temporary:
        write = release._write_output_temporary

        def refuse_terminal_write(parent, name, content):
            if name == "terminal.json":
                assert list(raw_root.glob("paired-*/*.sig"))
                raise OSError("fixture terminal storage error")
            return write(parent, name, content)

        temporary.setattr(release, "_write_output_temporary", refuse_terminal_write)
        with pytest.raises(OSError, match="terminal storage error"):
            finalize(lambda: None)
        assert not list(raw_root.glob("paired-*/*.sig"))
        assert not list(raw_root.glob("paired-*/terminal.json"))
    terminal_path = finalize(lambda: None)
    terminal = json.loads(terminal_path.read_bytes())

    output, _, status = WRITER.publish_manifest(
        root=root,
        registry_path=root / "tools/ci/proof-authority.toml",
        schema_path=root / "tools/ci/proof-manifest.schema.json",
        proof_id=proof["id"],
        terminal_input_path=terminal_path,
        paired_checkout=checkout,
    )

    payload = json.loads(output.read_text(encoding="utf-8"))
    schema = json.loads((root / "tools/ci/proof-manifest.schema.json").read_text())
    assert status == "passed"
    assert payload["source_pair"]["repository"] == proof["paired_repository"]
    assert (
        CHECKER.check_manifest(
            payload,
            manifest_path=output,
            proof=proof,
            schema=schema,
            root=root,
            bind_source=True,
            proof_by_id=by_id,
            paired_checkouts={proof["paired_repository"]: checkout},
        )
        == []
    )
    forged = dict(payload)
    forged["counts"] = {"selected": 3, "executed": 3, "passed": 3, "failed": 0, "ignored": 0}
    findings = CHECKER.check_manifest(
        forged,
        manifest_path=output,
        proof=proof,
        schema=schema,
        root=root,
        bind_source=True,
        proof_by_id=by_id,
        paired_checkouts={proof["paired_repository"]: checkout},
    )
    assert any("manifest counts differ from raw execution" in item.message for item in findings)
    event_artifact = next(
        item
        for item in payload["artifacts"]
        if item["source_path"] == terminal["execution_result"]["runs"][0]["events"]
    )
    (root / event_artifact["path"]).write_text("proof passed\n", encoding="utf-8")
    findings = CHECKER.check_manifest(
        payload,
        manifest_path=output,
        proof=proof,
        schema=schema,
        root=root,
        bind_source=True,
        proof_by_id=by_id,
        paired_checkouts={proof["paired_repository"]: checkout},
    )
    assert any("proof artifact digest mismatch" in item.message for item in findings)
    assert any("execution result is not authoritative" in item.message for item in findings)


def test_exact_binding_refuses_a_free_paired_checkout(tmp_path: Path) -> None:
    proof = {"source_binding": "exact"}
    with pytest.raises(WRITER.ManifestRefused, match="forbids --paired-checkout"):
        WRITER._resolve_source_pair(proof, str(tmp_path), checker=CHECKER)


def test_linux_production_profile_cannot_be_spoofed_on_non_linux_host(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    manifest_templates: ManifestTemplates,
) -> None:
    root, registry = _fixture_root(tmp_path, manifest_templates)
    _, terminal = _terminal(root)
    proof = dict(registry["proofs"][0])
    proof["required_host"] = "linux-production-like"
    terminal["environment"]["os"] = "darwin"
    terminal["environment"]["host"]["profile"] = "linux-production-like"
    monkeypatch.setattr(WRITER.platform, "system", lambda: "Darwin")

    with pytest.raises(WRITER.ManifestRefused, match="cannot be published from a non-Linux"):
        WRITER.build_manifest(
            root=root,
            proof=proof,
            proof_by_id={proof["id"]: proof},
            terminal=terminal,
            checker=CHECKER,
            paired_checkout=None,
        )


def test_input_digests_are_computed_from_file_or_literal_identity(tmp_path: Path) -> None:
    root = tmp_path.resolve()
    fixture = root / "fixture.json"
    fixture.write_bytes(b'{"fixture":true}\n')

    assert WRITER._digest_input(root, {"path": "fixture.json"}, label="fixture") == (
        f"sha256:{_digest(fixture)}"
    )
    assert WRITER._digest_input(root, {"value": "model-v1"}, label="model") == (
        "sha256:1a1f4502024df8a68d12e64bb2364ad6308d04ed0a7d5e8300a676ec70867140"
    )
    with pytest.raises(WRITER.ManifestRefused, match="valid path or non-empty value"):
        WRITER._digest_input(root, {"sha256": "sha256:" + "0" * 64}, label="provider")


def test_schema_allows_upstreamless_or_detached_source_but_keeps_binary_field() -> None:
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    source = {
        "head": "a" * 40,
        "dirty_digest": "sha256:" + "b" * 64,
        "branch": None,
        "upstream": None,
        "merge_base": None,
    }
    jsonschema.Draft202012Validator(schema).validate(
        {
            "schema_version": 1,
            "proof_id": "fixture-proof",
            "family": "S",
            "status": "passed",
            "source": source,
            "source_pair": None,
            "invocation": {
                "command": "check",
                "profile": None,
                "target": None,
                "filter": None,
            },
            "counts": {
                "selected": 1,
                "executed": 1,
                "passed": 1,
                "failed": 0,
                "ignored": 0,
            },
            "environment": {
                "toolchain": "fixture",
                "features": [],
                "os": "linux",
                "arch": "x86_64",
                "host": {
                    "profile": "fixture",
                    "cpu_count": 1,
                    "memory_bytes": 1,
                    "identity_digest": "sha256:" + "c" * 64,
                },
            },
            "daemon_binary": None,
            "state_root_format": "not-applicable",
            "inputs": {name: None for name in WRITER.INPUT_NAMES},
            "started_at": "2026-09-21T00:00:00Z",
            "ended_at": "2026-09-21T00:00:01Z",
            "dependency_receipts": [],
            "artifacts": [
                {
                    "source_path": "raw.log",
                    "path": "artifacts/proof-authority/evidence/" + "d" * 64,
                    "sha256": "d" * 64,
                }
            ],
        }
    )


@pytest.mark.parametrize(
    "proof_id", ["p11-cross-repo-cutover", "p11-deployment", "p11-activation", "p11-rollback"]
)
def test_unsigned_p11_observations_refused_by_writer_and_archive(tmp_path, monkeypatch, proof_id):
    from tools.ci.tests import test_write_proof_aggregate as release_fixture

    root, registry = release_fixture._build_fixture_root(tmp_path, executable=True)
    pair = release_fixture._build_paired_checkout(tmp_path)
    release_fixture._write_dependency_manifests(root, registry, pair)
    authorities = {p["id"]: p for p in registry["proofs"]}
    proof = authorities[proof_id]
    output = root / proof["artifact"]
    before = output.read_bytes()
    payload = json.loads(before)
    schema_path = root / proof["artifact_schema"]
    for group, key, value in (
        ("environment", "toolchain", "different-toolchain"),
        ("environment", "features", ["another-profile"]),
        ("inputs", "model", "sha256:" + "9" * 64),
    ):
        changed = copy.deepcopy(payload)
        changed[group][key] = value
        findings = CHECKER.check_manifest(
            changed,
            proof=proof,
            manifest_path=output,
            root=root,
            bind_source=False,
            schema=json.loads(schema_path.read_bytes()),
            proof_by_id=authorities,
        )
        assert any("attestation" in f.message or "execution binding" in f.message for f in findings)
    payload["artifacts"] = [
        a for a in payload["artifacts"] if not a["source_path"].endswith(".sig")
    ]
    schema_path = root / proof["artifact_schema"]
    findings = CHECKER.check_manifest(
        payload,
        proof=proof,
        manifest_path=output,
        root=root,
        bind_source=False,
        schema=json.loads(schema_path.read_bytes()),
        proof_by_id=authorities,
    )
    assert any("attestation" in f.message for f in findings)
    terminal = {
        k: payload[k]
        for k in (
            "counts",
            "status",
            "state_root_format",
            "started_at",
            "ended_at",
            "execution_result",
        )
    }
    terminal.update(
        daemon_binary=payload["daemon_binary"]["source_path"],
        inputs=dict.fromkeys(WRITER.INPUT_NAMES),
        artifacts=[a["source_path"] for a in payload["artifacts"]],
        environment={**payload["environment"], "host": release_fixture.RELEASE_HOST_INPUT},
    )
    if proof_id != release_fixture.PAIR_PROOF:
        terminal["inputs"]["config"] = {"path": proof["operational_contract"]}
    terminal_path = root / "artifacts/terminal.json"
    terminal_path.write_text(json.dumps(terminal))
    monkeypatch.setattr(WRITER.platform, "system", lambda: "Linux")
    monkeypatch.setattr(WRITER.platform, "machine", lambda: "x86_64")
    with pytest.raises(WRITER.ManifestRefused, match="attestation"):
        WRITER.publish_manifest(
            root=root,
            registry_path=root / "tools/ci/proof-authority.toml",
            schema_path=schema_path,
            proof_id=proof_id,
            terminal_input_path=terminal_path,
            paired_checkout=pair,
        )
    assert output.read_bytes() == before
