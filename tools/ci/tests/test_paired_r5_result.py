"""Fixed runner and locator oracles for the paired R5 archive."""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
from copy import deepcopy
from pathlib import Path
from types import SimpleNamespace

import pytest

from tools.ci import binary_custody
from tools.ci import paired_r5_result as RUNNER
from tools.ci.paired_r5_result import (
    CALLER_FEATURES,
    CASES,
    _require_exact_resolution,
    _verify_resolution_files,
    read_locator,
    validate_locator,
    validate_selected,
)


def test_paired_runner_imports_its_owner_from_foreign_cwd(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[3]
    foreign_tools = tmp_path / "tools"
    foreign_tools.mkdir()
    (foreign_tools / "__init__.py").write_text("raise RuntimeError('foreign tools imported')\n")
    environment = os.environ.copy()
    environment["PYTHONDONTWRITEBYTECODE"] = "1"
    environment["PYTHONPATH"] = os.pathsep.join((str(root), str(tmp_path)))
    result = subprocess.run(
        [sys.executable, str(root / "tools/ci/paired_r5_result.py"), "--help"],
        cwd=tmp_path,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert "--qbc-lane" in result.stdout


def _rows(outcome: str = "ok", name: str = "pkg::binary$selected") -> tuple[bytes, bytes]:
    inventory = {
        "test-count": 1,
        "rust-suites": {
            "pkg::binary": {
                "package-name": "pkg",
                "binary-name": "binary",
                "kind": "test",
                "status": "listed",
                "testcases": {
                    "selected": {"ignored": False, "filter-match": {"status": "matches"}}
                },
            }
        },
    }
    metadata = {"crate": "pkg", "test_binary": "binary", "kind": "test"}
    rows = [
        {"type": "suite", "event": "started", "test_count": 1, "nextest": metadata},
        {"type": "test", "event": "started", "name": name},
        {"type": "test", "event": outcome, "name": name},
        {
            "type": "suite",
            "event": "ok" if outcome == "ok" else "failed",
            "passed": int(outcome == "ok"),
            "failed": int(outcome != "ok"),
            "ignored": 0,
            "nextest": metadata,
        },
    ]
    return (
        json.dumps(inventory).encode(),
        b"".join(json.dumps(row).encode() + b"\n" for row in rows),
    )


def test_exact_inventory_and_terminal_selected_name() -> None:
    listed, events = _rows()
    result = validate_selected(listed, events, "pkg::binary$selected")
    assert result["selected"] == result["executed"] == result["passed"] == 1
    assert result["failed"] == 0 and result["passed_names"] == ["pkg::binary$selected"]


@pytest.mark.parametrize("mutation", ["zero", "failed", "wrong-name", "missing-run", "extra-list"])
def test_runner_refuses_incomplete_or_mismatched_proof(mutation: str) -> None:
    listed, events = _rows(
        "failed" if mutation == "failed" else "ok",
        "pkg::binary$wrong" if mutation == "wrong-name" else "pkg::binary$selected",
    )
    if mutation == "zero":
        listed = listed.replace(b'"test-count": 1', b'"test-count": 0')
    if mutation == "missing-run":
        events = b""
    if mutation == "extra-list":
        listed = listed.replace(
            b'"selected": {',
            b'"extra": {"ignored": false, "filter-match": {"status": "matches"}}, "selected": {',
        ).replace(b'"test-count": 1', b'"test-count": 2')
    with pytest.raises(ValueError):
        validate_selected(listed, events, "pkg::binary$selected")


def test_locator_requires_exact_immutable_custody() -> None:
    command = ["cargo", "nextest", "run", "--lib"]
    receipt = {"source_snapshot_digest": "a" * 64, "artifact_env_digest": "b" * 64}
    run = SimpleNamespace(
        run_id="run",
        lane="local",
        result_directory=Path("/state/verification-results/run"),
        command_cwd=Path("/source"),
        command=tuple(command),
        exit_code=0,
    )

    class Capture:
        receipt_bytes = b"receipt"
        receipt_digest = "c" * 64
        process_exit_code_v1 = 0

        def __init__(self) -> None:
            self.run = run

        def receipt_payload_v1(self) -> dict:
            return receipt

    capture = Capture()
    locator = {
        "schema_version": "qbc-verification-completion-locator/v1",
        "nonce": "d" * 32,
        "lane": "local",
        "run_id": "run",
        "receipt_path": "/state/verification-results/run/receipt.json",
        "receipt_size_bytes": 7,
        "receipt_sha256": "c" * 64,
        "source_snapshot_schema": "source-snapshot-identity.v1",
        "source_snapshot_digest": "a" * 64,
        "cargo_argv": command,
        "command_cwd": "/source",
        "request_env_sha256": "e" * 64,
        "artifact_env_digest": "b" * 64,
        "receipt_exit_code": 0,
    }
    validate_locator(
        locator,
        capture,
        nonce="d" * 32,
        request_sha="e" * 64,
        command=command,
        source_head="f" * 40,
        source_digest="a" * 64,
    )
    for field, replacement in (
        ("nonce", "0" * 32),
        ("cargo_argv", command[:-1]),
        ("receipt_sha256", "0" * 64),
        ("source_snapshot_digest", "0" * 64),
        ("command_cwd", "/wrong"),
        ("lane", "wrong"),
        ("request_env_sha256", "0" * 64),
        ("receipt_exit_code", 1),
        ("receipt_exit_code", False),
        ("receipt_size_bytes", 7.0),
    ):
        changed = {**locator, field: replacement}
        with pytest.raises(ValueError):
            validate_locator(
                changed,
                capture,
                nonce="d" * 32,
                request_sha="e" * 64,
                command=command,
                source_head="f" * 40,
                source_digest="a" * 64,
            )


def test_missing_and_duplicate_locator_are_refused(tmp_path: Path) -> None:
    class BoundedReader:
        @staticmethod
        def read_completion_locator_bytes_v1(path: Path) -> bytes:
            return path.read_bytes()

    with pytest.raises(FileNotFoundError):
        read_locator(tmp_path / "missing.json", BoundedReader())
    malformed = tmp_path / "duplicate.json"
    malformed.write_bytes(b'{"nonce":"a","nonce":"b"}')
    with pytest.raises(ValueError, match="duplicate QBC completion locator key"):
        read_locator(malformed, BoundedReader())


def test_recipe_selected_caller_and_kernel_literals() -> None:
    # Fixed from Semantica quanta-runtime/Cargo.toml's target required-features.
    assert CALLER_FEATURES == "index-sdk-ingress,retrieval-authority-contract-surface"
    assert CASES == (
        (
            "caller",
            "quanta-runtime",
            "index-sdk-ingress,retrieval-authority-contract-surface",
            "index_sdk_ingress_publish_contract_test",
            "test",
            "index_sdk_ingress_live_repomap_roundtrip_survives_runtime_restart_v1",
        ),
        (
            "kernel",
            "quanta-runtime-retrieval-kernel",
            "index-sdk-ingress-surface",
            "quanta_runtime_retrieval_kernel",
            "lib",
            "index_sdk_ingress::terminal_receipt_v1::tests::repomap_v2_receipts_require_exact_full_bundle_and_transition_v2",
        ),
    )


@pytest.mark.parametrize(
    ("replacement_boundary", "expected_runs"),
    [(None, 2), ("before-list", 0), ("after-list", 1), ("before-run", 1), ("after-run", 2)],
)
@pytest.mark.parametrize("replacement_path", ["built", "provided", "custody"])
def test_runner_checks_daemon_custody_at_each_nextest_boundary(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    replacement_boundary: str | None,
    expected_runs: int,
    replacement_path: str,
) -> None:
    root, paired = tmp_path / "quanta", tmp_path / "paired"
    paths = {name: tmp_path / name for name in ("built", "provided", "custody")}
    for path in paths.values():
        path.write_bytes(b"original daemon bytes")
        path.chmod(0o700)
    digest = hashlib.sha256(paths["built"].read_bytes()).hexdigest()
    custody_command = ["verify-fixture-daemon", digest, *map(str, paths.values())]
    listed, events = _rows()
    phases: list[str] = []
    guards: list[str] = []
    last_source: Path | None = None

    def frozen(source: Path, _head: str) -> None:
        nonlocal last_source
        last_source = source

    def run(command: list[str], **_kwargs):
        if command[0] == "verify-fixture-daemon":
            assert command == custody_command
            boundary = (
                "before-" + ("list" if not phases else "run")
                if last_source == paired
                else "after-" + phases[-1]
            )
            guards.append(boundary)
            if replacement_boundary == boundary:
                paths[replacement_path].write_bytes(b"replaced daemon bytes")
            binary_custody.verify(command[1], [Path(path) for path in command[2:]])
        else:
            assert command[:4] == [
                str(paired / "scripts/quanta-build-cli"),
                "nextest",
                "--lane",
                "fixture",
            ]
            phases.append(command[5])
        return SimpleNamespace(returncode=0)

    capture = SimpleNamespace(
        run=SimpleNamespace(lane="fixture", execution_root=paired, command_cwd=paired),
        custody_payload_v1=lambda: {},
    )

    def completed(_path: Path):
        capture.stdout = listed if phases[-1] == "list" else events
        return SimpleNamespace(**vars(capture))

    monkeypatch.setattr(RUNNER, "_frozen", frozen)
    monkeypatch.setattr(RUNNER.subprocess, "run", run)
    monkeypatch.setattr(
        RUNNER, "read_locator", lambda *_: (b"locator", {"receipt_path": "/fixture/receipt.json"})
    )
    # Locator/content parsing has separate malformed and fixed-byte oracles.
    # This test replaces only QBC transport, keeping the real custody verifier.
    monkeypatch.setattr(RUNNER, "validate_locator", lambda *_args, **_kwargs: None)
    monkeypatch.setattr(RUNNER, "_archive_capture", lambda *_: {})
    arguments = (
        paired,
        root,
        tmp_path,
        "a" * 40,
        "b" * 40,
        ("caller", "pkg", "feature", "binary", "test", "selected"),
        SimpleNamespace(read_completed_run_receipt_at_path_v1=completed),
        SimpleNamespace(
            LOCATOR_PATH_ENV_V1="FIXTURE_LOCATOR",
            LOCATOR_NONCE_ENV_V1="FIXTURE_NONCE",
            REQUEST_ENV_DIGEST_ENV_V1="FIXTURE_ENV_DIGEST",
            EXPECTED_SOURCE_HEAD_ENV_V1="FIXTURE_SOURCE",
        ),
        SimpleNamespace(
            _campaign_cargo_admission_owner_v1=lambda: SimpleNamespace(
                campaign_source_snapshot_digest_v1=lambda _: "c" * 64
            )
        ),
        custody_command,
        "fixture",
    )
    if replacement_boundary is None:
        result = RUNNER._one(*arguments)
        assert (
            result["nextest"]["selected"]
            == result["nextest"]["executed"]
            == result["nextest"]["passed"]
            == 1
        )
        assert guards == ["before-list", "after-list", "before-run", "after-run"]
    else:
        with pytest.raises(ValueError, match="release daemon bytes changed"):
            RUNNER._one(*arguments)
        assert guards[-1] == replacement_boundary
    assert len(phases) == expected_runs


def test_locator_refuses_wrong_current_source_and_failed_process() -> None:
    # Reuse the complete fixed locator fixture without making a second authority.
    # The positive fixture above covers exact content; this test checks the
    # independent current-source and process-exit projections directly.
    class Capture:
        run = SimpleNamespace(
            run_id="run",
            lane="local",
            result_directory=Path("/state/verification-results/run"),
            command_cwd=Path("/source"),
            command=("cargo", "nextest", "run"),
            exit_code=0,
        )
        receipt_bytes = b"receipt"
        receipt_digest = "c" * 64
        process_exit_code_v1 = 1

        def receipt_payload_v1(self) -> dict:
            return {"source_snapshot_digest": "a" * 64, "artifact_env_digest": "b" * 64}

    locator = {
        "schema_version": "qbc-verification-completion-locator/v1",
        "nonce": "d" * 32,
        "lane": "local",
        "run_id": "run",
        "receipt_path": "/state/verification-results/run/receipt.json",
        "receipt_size_bytes": 7,
        "receipt_sha256": "c" * 64,
        "source_snapshot_schema": "source-snapshot-identity.v1",
        "source_snapshot_digest": "a" * 64,
        "cargo_argv": ["cargo", "nextest", "run"],
        "command_cwd": "/source",
        "request_env_sha256": "e" * 64,
        "artifact_env_digest": "b" * 64,
        "receipt_exit_code": 0,
    }
    kwargs = dict(
        nonce="d" * 32,
        request_sha="e" * 64,
        command=["cargo", "nextest", "run"],
        source_head="f" * 40,
    )
    successful = Capture()
    successful.process_exit_code_v1 = 0
    with pytest.raises(ValueError, match="current frozen source"):
        validate_locator(locator, successful, **kwargs, source_digest="0" * 64)
    with pytest.raises(ValueError, match="successful process exit"):
        validate_locator(locator, Capture(), **kwargs, source_digest="a" * 64)
    boolean_exit = Capture()
    boolean_exit.process_exit_code_v1 = False
    with pytest.raises(ValueError, match="successful process exit"):
        validate_locator(locator, boolean_exit, **kwargs, source_digest="a" * 64)
    boolean_run = Capture()
    boolean_run.run = SimpleNamespace(**{**vars(Capture.run), "exit_code": False})
    boolean_run.process_exit_code_v1 = 0
    with pytest.raises(ValueError, match="successful process exit"):
        validate_locator(locator, boolean_run, **kwargs, source_digest="a" * 64)


def test_default_recipe_uses_typed_selected_runner_and_same_cargo_target_floor() -> None:
    # Independent fixed literal from quanta-runtime/Cargo.toml's
    # index_sdk_ingress_publish_contract_test required-features, not the helper.
    required = "index-sdk-ingress,retrieval-authority-contract-surface"
    script = (
        Path(__file__).resolve().parents[3] / "scripts/verify-repomap-cross-repo.sh"
    ).read_text(encoding="utf-8")
    assert script.count(f"resolved_pair quanta-runtime {required})") == 2
    assert '--qbc-lane "$r5_lane"' in script
    assert '"$paired_python" "$quanta_root/tools/ci/paired_r5_result.py"' in script
    assert "--list" not in script
    assert "| rg '^index_sdk_ingress" not in script
    assert "--all-features" not in script


def test_paired_resolver_refuses_missing_caller_required_feature(tmp_path: Path) -> None:
    resolutions = {
        "runtime": {
            "version": 1,
            "consumer": "quanta-runtime",
            "consumer_features": ["index-sdk-ingress"],
        },
    }
    with pytest.raises(ValueError, match="selected caller/kernel feature authority"):
        _verify_resolution_files(tmp_path, tmp_path, resolutions)


def _complete_resolution_fixture(tmp_path: Path) -> tuple[Path, Path, dict]:
    root = tmp_path / "quanta"
    paired = tmp_path / "semantica"
    workspace = paired / "packages/analysis/quanta-v2"
    workspace.mkdir(parents=True)
    root.mkdir()
    (root / "Cargo.toml").write_bytes(b"[workspace]\n")
    (workspace / "Cargo.lock").write_bytes(b"version = 3\n")

    def digest(path: Path) -> str:
        return "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()

    packages = []
    for name in ("quanta-index-contract", "quanta-index-ipc", "quanta-index-sdk"):
        manifest = root / "crates" / name / "Cargo.toml"
        manifest.parent.mkdir(parents=True)
        manifest.write_text(f'[package]\nname = "{name}"\nversion = "0.1.0"\n', encoding="utf-8")
        packages.append(
            {
                "name": name,
                "version": "0.1.0",
                "manifest": f"crates/{name}/Cargo.toml",
                "manifest_sha256": digest(manifest),
                "features": [],
            }
        )
    base = {
        "version": 1,
        "workspace": "packages/analysis/quanta-v2",
        "dependency_lock": {
            "path": "packages/analysis/quanta-v2/Cargo.lock",
            "sha256": digest(workspace / "Cargo.lock"),
        },
        "quanta_workspace_manifest_sha256": digest(root / "Cargo.toml"),
        "packages": packages,
    }
    return (
        root,
        paired,
        {
            "runtime": {
                **base,
                "consumer": "quanta-runtime",
                "consumer_features": ["index-sdk-ingress", "retrieval-authority-contract-surface"],
            },
            "kernel": {
                **base,
                "consumer": "quanta-runtime-retrieval-kernel",
                "consumer_features": ["index-sdk-ingress-surface"],
            },
        },
    )


def test_complete_minimal_pair_file_and_feature_custody(tmp_path: Path) -> None:
    root, paired, resolutions = _complete_resolution_fixture(tmp_path)
    assert _verify_resolution_files(root, paired, resolutions) is None
    assert (
        _require_exact_resolution(resolutions["runtime"], resolutions["runtime"], "caller") is None
    )
    assert _require_exact_resolution(resolutions["kernel"], resolutions["kernel"], "kernel") is None


@pytest.mark.parametrize("lock_path", ["Cargo.lock", "/Cargo.lock", "../Cargo.lock"])
def test_paired_lock_path_refuses_another_anchor(tmp_path: Path, lock_path: str) -> None:
    root, paired, resolutions = _complete_resolution_fixture(tmp_path)
    resolutions["runtime"]["dependency_lock"]["path"] = lock_path
    with pytest.raises(ValueError, match="relative to the paired checkout"):
        _verify_resolution_files(root, paired, resolutions)


def test_resolution_preflight_cli_checks_nested_lock_without_runner(tmp_path: Path) -> None:
    root, paired, resolutions = _complete_resolution_fixture(tmp_path)
    # A different root lock cannot stand in for the nested Cargo resolver's lock.
    (paired / "Cargo.lock").write_bytes(b"unrelated root lock\n")
    modules = paired / "tools/quanta-build-cli"
    modules.mkdir(parents=True)
    for name in ("qbc_completed_run_v1", "verification_completion_locator_v1", "quanta_build_cli"):
        (modules / (name + ".py")).write_text("# import-only runtime fixture\n")
    command = [
        sys.executable,
        str(Path(RUNNER.__file__)),
        "--verify-resolution-only",
        "--quanta-root",
        str(root),
        "--semantica-root",
        str(paired),
        "--runtime-resolution",
        json.dumps(resolutions["runtime"]),
        "--kernel-resolution",
        json.dumps(resolutions["kernel"]),
    ]
    environment = os.environ.copy()
    environment["PYTHONPATH"] = str(Path(RUNNER.__file__).resolve().parents[2])
    environment["PYTHONDONTWRITEBYTECODE"] = "1"
    result = subprocess.run(
        command, cwd=tmp_path, env=environment, capture_output=True, text=True, check=False
    )
    assert result.returncode == 0, result.stderr
    (modules / "quanta_build_cli.py").write_text("raise RuntimeError('QBC runtime unavailable')\n")
    result = subprocess.run(
        command, cwd=tmp_path, env=environment, capture_output=True, text=True, check=False
    )
    assert result.returncode != 0
    assert "QBC runtime unavailable" in result.stderr
    (modules / "quanta_build_cli.py").write_text("# import-only runtime fixture\n")
    (paired / "packages/analysis/quanta-v2/Cargo.lock").write_bytes(b"changed nested lock\n")
    result = subprocess.run(
        command, cwd=tmp_path, env=environment, capture_output=True, text=True, check=False
    )
    assert result.returncode != 0
    assert "paired resolver file identity changed" in result.stderr


@pytest.mark.parametrize("version", [False, True, 1.0])
def test_paired_resolver_rejects_non_integer_version(tmp_path: Path, version: object) -> None:
    root, paired, resolutions = _complete_resolution_fixture(tmp_path)
    for selected in ("runtime", "kernel"):
        mutated = deepcopy(resolutions)
        mutated[selected]["version"] = version
        with pytest.raises(ValueError, match="consumer identity changed"):
            _verify_resolution_files(root, paired, mutated)


@pytest.mark.parametrize("alias", [True, 1.0])
def test_canonical_resolution_comparison_rejects_numeric_aliases(
    tmp_path: Path, alias: object
) -> None:
    _root, _paired, resolutions = _complete_resolution_fixture(tmp_path)
    observed = deepcopy(resolutions["runtime"])
    observed["version"] = alias
    # Native Python dict equality admits both aliases against integer 1.
    assert observed == resolutions["runtime"]
    with pytest.raises(ValueError, match="caller resolver identity changed"):
        _require_exact_resolution(observed, resolutions["runtime"], "caller")
