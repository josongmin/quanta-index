"""Fixed runner and locator oracles for the paired R5 archive."""

from __future__ import annotations

import errno
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
    CASES,
    _require_exact_resolution,
    _verify_resolution_files,
    read_locator,
    validate_locator,
    validate_selected,
)
from tools.ci.tests.p11_test_issuer import cache_old_python_source, nextest_bytes


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


@pytest.mark.parametrize("isolated", [True, False])
def test_signed_paired_cli_isolates_signer_across_site_lifecycle(tmp_path: Path, isolated: bool):
    root = Path(__file__).resolve().parents[3]
    runner = root / "tools/ci/paired_r5_result.py"
    marker = tmp_path / "site-lifecycle"
    signer = str(tmp_path / "fixture-signer")
    environment = {
        **os.environ,
        RUNNER.SOCKET_ENV: signer,
        "SSH_AUTH_SOCK": signer,
    }
    # Exercise interpreter lifecycle hooks without touching installed site files.
    probe = (
        "import atexit, os, runpy, site, sys\n"
        "from pathlib import Path\n"
        "def observe():\n"
        "    exposed = bool(os.environ.get('QUANTA_P11_SIGNER_SOCKET') "
        "or os.environ.get('SSH_AUTH_SOCK'))\n"
        f"    with Path({str(marker)!r}).open('a') as stream:\n"
        "        stream.write(('exposed' if exposed else 'clear') + '\\n')\n"
        "def activate_site():\n"
        "    observe()\n"
        "    atexit.register(observe)\n"
        "site.main = activate_site\n"
        f"sys.path.insert(0, {str(root)!r})\n"
        f"runpy.run_path({str(runner)!r}, run_name='__main__')\n"
    )
    flags = ["-I", "-S"] if isolated else ["-S"]
    completed = subprocess.run(
        [sys.executable, *flags, "-B", "-c", probe, "--help"],
        cwd=tmp_path,
        env=environment,
        text=True,
        capture_output=True,
        check=False,
    )
    if isolated:
        assert completed.returncode == 0, completed.stderr
        assert marker.is_file() and marker.read_text() == "clear\nclear\n"
    else:
        assert completed.returncode != 0
        assert "P11 signing requires the isolated paired R5 launcher" in completed.stderr
        assert not marker.exists()


SELECTED_CASE = ("caller", "pkg", "feature", "binary", "test", "selected")


def _rows(
    outcome: str = "ok", name: str = "pkg::binary$selected", *, kind: str = "test"
) -> tuple[bytes, bytes]:
    from tools.ci.tests.p11_test_issuer import nextest_suite_id

    binary_id = nextest_suite_id("pkg", "binary", kind)
    inventory = {
        "test-count": 1,
        "rust-suites": {
            binary_id: {
                "package-name": "pkg",
                "binary-id": binary_id,
                "binary-name": "binary",
                "kind": kind,
                "status": "listed",
                "testcases": {
                    "selected": {"ignored": False, "filter-match": {"status": "matches"}}
                },
            }
        },
    }
    metadata = {"crate": "pkg", "test_binary": "binary", "kind": kind}
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
    result = validate_selected(listed, events, SELECTED_CASE)
    assert result["selected"] == result["executed"] == result["passed"] == 1
    assert result["failed"] == 0 and result["passed_names"] == ["pkg::binary$selected"]


@pytest.mark.parametrize(
    "mutation", ["zero", "failed", "wrong-name", "missing-run", "extra-list", "wrong-kind"]
)
def test_runner_refuses_incomplete_or_mismatched_proof(mutation: str) -> None:
    listed, events = _rows(
        "failed" if mutation == "failed" else "ok",
        "pkg::binary$wrong" if mutation == "wrong-name" else "pkg::binary$selected",
        kind="lib" if mutation == "wrong-kind" else "test",
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
        validate_selected(listed, events, SELECTED_CASE)


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
    ("change", "replacement_path", "expected_runs"),
    [
        (boundary, path, runs)
        for boundary, runs in (
            ("before-list", 0),
            ("after-list", 1),
            ("before-run", 1),
            ("after-run", 2),
        )
        for path in ("built", "provided", "custody")
    ]
    + [
        (None, "custody", 2),
        ("no-witness", "custody", 2),
        ("wrong-name", "custody", 2),
        ("wrong-nonce", "custody", 2),
        ("wrong-executable", "custody", 2),
        ("missing-restart", "custody", 2),
        ("different-restart", "custody", 2),
    ],
)
def test_runner_checks_daemon_custody_and_live_witness(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    change: str | None,
    replacement_path: str,
    expected_runs: int,
) -> None:
    root, paired = tmp_path / "quanta", tmp_path / "paired"
    paths = {name: tmp_path / name for name in ("built", "provided", "custody")}
    for path in paths.values():
        path.write_bytes(b"original daemon bytes")
        path.chmod(0o700)
    digest = hashlib.sha256(paths["built"].read_bytes()).hexdigest()
    real_verify = binary_custody.verify
    listed, events = _rows()
    phases: list[str] = []
    guards: list[str] = []
    last_source: Path | None = None

    def frozen(source: Path, _head: str) -> None:
        nonlocal last_source
        last_source = source

    def verify(expected_digest, watched_paths):
        assert expected_digest == digest and watched_paths == list(paths.values())
        boundary = (
            "before-" + ("list" if not phases else "run")
            if last_source == paired
            else "after-" + phases[-1]
        )
        guards.append(boundary)
        if change == boundary:
            paths[replacement_path].write_bytes(b"replaced daemon bytes")
        real_verify(expected_digest, watched_paths)

    witness = tmp_path / RUNNER.R5_WITNESS_FILE

    def execute_phase(semantica, _case, phase, cargo, _owner, _completion, env, lane, *, cases):
        assert semantica == paired and lane == "fixture"
        assert cargo[:3] == ["cargo", "nextest", phase]
        assert env.get("QUANTA_INDEX_SEARCHD_BIN") == str(paths["custody"])
        phases.append(phase)
        if phase == "list":
            assert RUNNER.WITNESS_DIR_ENV not in env and RUNNER.WITNESS_NONCE_ENV not in env
            return 0
        assert Path(env[RUNNER.WITNESS_DIR_ENV]) == tmp_path
        nonce = env[RUNNER.WITNESS_NONCE_ENV]
        assert nonce == env["FIXTURE_NONCE"]
        if change != "no-witness":
            unrelated = hashlib.sha256(b"another executable").hexdigest()
            mutations = {
                "wrong-name": {"test": "pkg::binary$different"},
                "wrong-nonce": {"nonce": ("0" if nonce[0] != "0" else "1") + nonce[1:]},
                "wrong-executable": {"executables": [unrelated, unrelated]},
                "missing-restart": {"executables": [digest]},
                "different-restart": {"executables": [digest, unrelated]},
            }
            observation = {
                "schema_version": 1,
                "test": "pkg::binary$selected",
                "nonce": nonce,
                "executables": [digest, digest],
                **mutations.get(change, {}),
            }
            witness.write_text(json.dumps(observation))
        return 0

    capture = SimpleNamespace(
        run=SimpleNamespace(lane="fixture", execution_root=paired, command_cwd=paired),
        custody_payload_v1=lambda: {},
    )

    def completed(_path: Path):
        capture.stdout = listed if phases[-1] == "list" else events
        return SimpleNamespace(**vars(capture))

    monkeypatch.setenv("QUANTA_INDEX_SEARCHD_BIN", str(paths["provided"]))
    monkeypatch.setattr(RUNNER, "_frozen", frozen)
    monkeypatch.setattr(binary_custody, "verify", verify)
    monkeypatch.setattr(RUNNER, "_execute_qbc_phase", execute_phase)
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
        digest,
        paths,
        "fixture",
    )
    witness_errors = {
        "no-witness": (FileNotFoundError, RUNNER.R5_WITNESS_FILE),
        "wrong-name": (ValueError, "differs from its selected execution"),
        "wrong-nonce": (ValueError, "differs from its selected execution"),
        "wrong-executable": (ValueError, "executed daemon differs"),
        "missing-restart": (ValueError, "must bind 2"),
        "different-restart": (ValueError, "must bind 2"),
    }
    if change is None:
        result = RUNNER._one(*arguments)
        assert (
            result["nextest"]["selected"]
            == result["nextest"]["executed"]
            == result["nextest"]["passed"]
            == 1
        )
        assert guards == ["before-list", "after-list", "before-run", "after-run"]
        assert (
            result["executions"]["run"]["daemon_witness_sha256"]
            == hashlib.sha256(witness.read_bytes()).hexdigest()
        )
        assert "daemon_witness_sha256" not in result["executions"]["list"]
    elif change in witness_errors:
        error, message = witness_errors[change]
        with pytest.raises(error, match=message):
            RUNNER._one(*arguments)
    else:
        with pytest.raises(ValueError, match="release daemon bytes changed"):
            RUNNER._one(*arguments)
        assert guards[-1] == change
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
    from tools.ci.daemon_execution_evidence import R5_PROOF, live_proof, r5_cases

    # Independent fixed literal from quanta-runtime/Cargo.toml's
    # index_sdk_ingress_publish_contract_test required-features, not the helper.
    required = "index-sdk-ingress,retrieval-authority-contract-surface"
    root = Path(__file__).resolve().parents[3]
    script = (root / "scripts/verify-repomap-cross-repo.sh").read_text(encoding="utf-8")
    caller, kernel = r5_cases(live_proof(root, R5_PROOF))
    assert set(required.split(",")).issubset(caller[2].split(","))
    # Semantica's product restart and terminal-binding witnesses for the
    # Source-only candidate. The archived CASES above still resolve old proofs.
    assert caller[-1] == "index_sdk_ingress_native_source_text_path_cas_survives_runtime_restart_v1"
    assert kernel[-1] == (
        "index_sdk_ingress::terminal_receipt_v1::tests::"
        "p11_source_only_source_history_original_request_rejects_foreign_published_tuple_v7"
    )
    assert 'resolved_pair "$runtime_package" "$runtime_features"' in script
    assert '--qbc-lane "$r5_lane"' in script
    assert '"$quanta_root/tools/ci/paired_r5_result.py" "$@"' in script
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
        "kernel": {},
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


@pytest.mark.parametrize(
    "state",
    ["clean", "dirty", "wrong-head", "foreign-module", "import-error", "wrong-lock", "stale-cache"],
)
def test_resolution_preflight_cli_checks_nested_lock_without_runner(
    tmp_path: Path, state: str
) -> None:
    root, paired, resolutions = _complete_resolution_fixture(tmp_path)
    registry = root / "tools/ci/proof-authority.toml"
    registry.parent.mkdir(parents=True)
    fields = ("role", "package", "features", "binary", "kind", "test")
    selections = "\n".join(
        "[[proofs.paired_execution_cases]]\n"
        + "\n".join(f"{key} = {json.dumps(value)}" for key, value in zip(fields, case, strict=True))
        for case in CASES
    )
    registry.write_text(
        '[[proofs]]\nid = "p11-cross-repo-cutover"\nauthority_state = "staged"\n'
        + selections
        + "\n"
    )
    # A different root lock cannot stand in for the nested Cargo resolver's lock.
    (paired / "Cargo.lock").write_bytes(b"unrelated root lock\n")
    modules = paired / "tools/quanta-build-cli"
    modules.mkdir(parents=True)
    for name in ("qbc_completed_run_v1", "verification_completion_locator_v1"):
        (modules / (name + ".py")).write_text("# import-only runtime fixture\n")
    marker = tmp_path / "qbc-module-imported"
    observe_import = f"from pathlib import Path\nPath({str(marker)!r}).touch()\n"
    qbc_module = modules / "quanta_build_cli.py"
    foreign = tmp_path / "foreign_modules"
    foreign.mkdir()
    if state == "foreign-module":
        (foreign / qbc_module.name).write_text(observe_import)
    elif state == "import-error":
        qbc_module.write_text("raise RuntimeError('QBC runtime unavailable')\n")
    else:
        qbc_module.write_text(observe_import)
    if state == "stale-cache":
        (paired / ".gitignore").write_text("__pycache__/\n")

    heads = {}
    for checkout in (root, paired):
        git = ["git", "-C", str(checkout)]
        for args in (
            ["init", "-q"],
            ["add", "."],
            [
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-qm",
                "fixture",
            ],
        ):
            subprocess.run([*git, *args], check=True, capture_output=True)
        heads[checkout] = subprocess.check_output([*git, "rev-parse", "HEAD"], text=True).strip()
    if state == "dirty":
        qbc_module.write_text(observe_import + "# changed after freeze\n")
    elif state == "wrong-head":
        heads[paired] = "f" * 40
    elif state == "wrong-lock":
        resolutions["runtime"]["dependency_lock"]["sha256"] = "sha256:" + "f" * 64
    elif state == "stale-cache":
        cache_old_python_source(qbc_module, b"raise RuntimeError('stale QBC module')\n")

    command = [
        sys.executable,
        str(Path(RUNNER.__file__)),
        "--verify-resolution-only",
        "--quanta-root",
        str(root),
        "--semantica-root",
        str(paired),
        "--quanta-head",
        heads[root],
        "--semantica-head",
        heads[paired],
        "--runtime-resolution",
        json.dumps(resolutions["runtime"]),
        "--kernel-resolution",
        json.dumps(resolutions["kernel"]),
    ]
    environment = os.environ.copy()
    environment["PYTHONPATH"] = os.pathsep.join(
        (str(Path(RUNNER.__file__).resolve().parents[2]), str(foreign))
    )
    environment["PYTHONDONTWRITEBYTECODE"] = "1"
    environment.pop("PYTHONPYCACHEPREFIX", None)
    result = subprocess.run(
        command, cwd=tmp_path, env=environment, capture_output=True, text=True, check=False
    )
    if state in {"clean", "stale-cache"}:
        assert result.returncode == 0, result.stderr
        assert marker.exists()
    else:
        expected = {
            "dirty": "QBC resolver preflight requires a clean source",
            "wrong-head": "source changed or is dirty",
            "foreign-module": "paired QBC module belongs to another source",
            "import-error": "QBC runtime unavailable",
            "wrong-lock": "paired resolver file identity changed",
        }[state]
        assert not marker.exists()
        assert result.returncode != 0
        assert expected in result.stderr


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


def _recipe_command(case, phase):
    label, package, features, binary, kind, test = case
    command = [
        "cargo",
        "nextest",
        phase,
        "--locked",
        "--manifest-path",
        "packages/analysis/quanta-v2/Cargo.toml",
        "-p",
        package,
        "--no-default-features",
        "--features",
        features,
    ]
    command.extend(["--lib"] if kind == "lib" else ["--test", binary])
    command.extend(["-E", "test(/^" + RUNNER.re.escape(test) + "$/)"])
    command.extend(
        ["--message-format", "json"]
        if phase == "list"
        else [
            "--message-format",
            "libtest-json-plus",
            "--message-format-version",
            "0.1",
        ]
    )
    return command


@pytest.mark.parametrize("case", (*CASES, (
    "caller", "quanta-sdk-runtime-executor", "ifx150-restart-recovery-test-support",
    "ifx150_restart_recovery_product_owner", "test", "source_only_fixture",
)))
@pytest.mark.parametrize("phase", ["list", "run"])
def test_exact_recipe_uses_qbc_owner_admission_and_completion_port(tmp_path, case, phase):
    command = _recipe_command(case, phase)
    calls = []
    completion = SimpleNamespace(
        LOCATOR_PATH_ENV_V1="LOCATOR",
        LOCATOR_NONCE_ENV_V1="NONCE",
        REQUEST_ENV_DIGEST_ENV_V1="REQUEST",
        EXPECTED_SOURCE_HEAD_ENV_V1="SOURCE",
        EXPECTED_SOURCE_ENV_V1="COMPLETION_SNAPSHOT",
    )
    control = {name: name.lower() for name in ("LOCATOR", "NONCE", "REQUEST", "SOURCE")}
    pinned_daemon = str(tmp_path / "pinned-searchd")
    witness_environment = {
        RUNNER.WITNESS_DIR_ENV: str(tmp_path),
        RUNNER.WITNESS_NONCE_ENV: "b" * 32,
    }
    if case[1] == "quanta-sdk-runtime-executor":
        witness_environment[RUNNER.INSTALLED_Q_SHA_ENV] = "d" * 64
    environment = {
        **control,
        **witness_environment,
        "CODEGRAPH_PERSONA": "agent",
        "QUANTA_INDEX_SEARCHD_BIN": pinned_daemon,
    }
    reference = f"quanta-index:paired-r5:{case[0]}:{phase}"

    def admit(model, *, authority):
        assert model == {"kind": "fixture", "command": command}
        assert authority == ("owner_recipe", reference)
        return "typed-command-bound-admission"

    def execute(**kwargs):
        calls.append(kwargs)
        assert kwargs["invocation_admission_v1"] == "typed-command-bound-admission"
        assert kwargs["owner_recipe_ref"] == reference
        assert kwargs["command"] == command and kwargs["runner"] == "nextest"
        assert kwargs["lane"] == "registered" and kwargs["lane_token"] is None
        assert kwargs["execution_root_v1"] == kwargs["command_cwd_v1"] == tmp_path
        assert kwargs["verification_completion_environment_v1"] == {
            **control,
            "COMPLETION_SNAPSHOT": "a" * 64,
        }
        assert kwargs["compile_env_overrides_v1"] == {
            "EXPECTED_SNAPSHOT": "a" * 64,
            "CODEGRAPH_PERSONA": "agent",
            "QUANTA_INDEX_SEARCHD_BIN": pinned_daemon,
            **({"NEXTEST_EXPERIMENTAL_LIBTEST_JSON": "1"} if phase == "run" else {}),
            **(witness_environment if case[0] == "caller" and phase == "run" else {}),
        }
        return 17

    owner = SimpleNamespace(
        _CARGO_INVOCATION_ADMISSION_OWNER_V1=SimpleNamespace(
            __file__=str(tmp_path / "tools/quanta-build-cli/cargo_invocation_admission.py"),
            validate_cargo_invocation_v1=admit,
            InvocationAuthorityV1=lambda kind, ref: (kind, ref),
            InvocationAuthorityKindV1=SimpleNamespace(OWNER_RECIPE="owner_recipe"),
        ),
        EXPECTED_SOURCE_SNAPSHOT_ENV_V1="EXPECTED_SNAPSHOT",
        _build_command_model=lambda argv, runner: {"kind": "fixture", "command": argv},
        _campaign_cargo_admission_owner_v1=lambda: SimpleNamespace(
            campaign_source_snapshot_digest_v1=lambda root: "a" * 64
        ),
        _run_lane_command=execute,
    )
    assert (
        RUNNER._execute_qbc_phase(
            tmp_path, case, phase, command, owner, completion, environment, "registered",
            cases=(case,),
        )
        == 17
    )
    assert len(calls) == 1
    mutations = [command + ["--all-features"], [token for token in command if token != "--locked"]]
    mutations.append(
        ["--tests" if token == "--lib" else token for token in command]
        if case[4] == "lib"
        else ["other-target" if token == case[3] else token for token in command]
    )
    mutations.append(["test(/.*/)" if token.startswith("test(/") else token for token in command])
    for changed in mutations:
        with pytest.raises(ValueError, match="command drift"):
            RUNNER._execute_qbc_phase(
                tmp_path, case, phase, changed, owner, completion, environment, "registered",
                cases=(case,),
            )
    with pytest.raises(ValueError, match="unregistered"):
        RUNNER._execute_qbc_phase(
            tmp_path,
            ("foreign", *case[1:]),
            phase,
            command,
            owner,
            completion,
            environment,
            "registered",
        )
    with pytest.raises(ValueError, match="unregistered"):
        RUNNER._execute_qbc_phase(
            tmp_path, case, "build", command, owner, completion, environment, "registered"
        )
    owner._CARGO_INVOCATION_ADMISSION_OWNER_V1.__file__ = str(tmp_path / "foreign.py")
    with pytest.raises(ValueError, match="another source"):
        RUNNER._execute_qbc_phase(
            tmp_path, case, phase, command, owner, completion, environment, "registered",
            cases=(case,),
        )
    assert len(calls) == 1


@pytest.mark.parametrize(
    "change",
    [
        "quanta",
        "semantica",
        "lock",
        "daemon",
        "receipt",
        "unsigned-index",
        "foreign-run",
        "mixed-snapshot",
        "wrong-caller-kind",
        "request",
        "feature-set",
        "packages",
        "extra-locator-field",
        "missing-daemon-observation",
        "wrong-daemon-observation",
        "changed-restart-observation",
        "stale-daemon-observation",
    ],
)
def test_release_replays_signed_qbc_bytes_and_source_binding(tmp_path, change):
    from tools.ci import paired_r5_release as release
    from tools.ci.proof_operational_result import manifest_binding
    from tools.ci.tests.p11_test_issuer import keypair, paired_evidence, sign_statement
    from tools.ci.tests.test_proof_operational_result import archive

    proof = {"id": release.PROOF_ID, "attestation_public_key": keypair()[0]}
    manifest = {
        "source": {"head": "a" * 40},
        "source_pair": {
            "source": {"head": "b" * 40},
            "dependency_lock": {
                "path": "packages/analysis/quanta-v2/Cargo.lock",
                "sha256": "1" * 64,
            },
        },
        "daemon_binary": {"sha256": "2" * 64},
        "environment": {"os": "linux", "host": {"identity_digest": "sha256:" + "3" * 64}},
        "inputs": dict.fromkeys(("fixture", "corpus", "config", "model", "provider")),
        "state_root_format": "v2",
        "dependency_receipts": [],
        "started_at": "2026-01-01T00:00:00Z",
        "ended_at": "2026-01-01T00:01:00Z",
    }
    manifest["execution_result"], manifest["artifacts"] = paired_evidence(
        tmp_path,
        proof,
        manifest,
        False,
        split_snapshot=(change == "mixed-snapshot"),
        wrong_caller_kind=(change == "wrong-caller-kind"),
    )
    if change == "mixed-snapshot":
        with pytest.raises(ValueError, match="one lane and source snapshot"):
            release.verify_release(tmp_path, proof, manifest, cases=CASES)
        return
    if change == "wrong-caller-kind":
        with pytest.raises(ValueError, match="inventory differs from exact selected test"):
            release.verify_release(tmp_path, proof, manifest, cases=CASES)
        return
    release.verify_release(tmp_path, proof, manifest, cases=CASES)
    if change == "missing-daemon-observation":
        manifest["artifacts"] = [
            item
            for item in manifest["artifacts"]
            if Path(item["source_path"]).name != RUNNER.R5_WITNESS_FILE
        ]
        with pytest.raises(ValueError, match="original QBC captures"):
            release.verify_release(tmp_path, proof, manifest, cases=CASES)
        return
    directory = Path(manifest["execution_result"]["runs"][0]["events"]).parent
    source = tmp_path / directory
    index = json.loads((source / release.INDEX).read_bytes())
    if change in ("quanta", "semantica"):
        index["source_heads"][change] = "c" * 40
    elif change == "lock":
        index["resolutions"]["runtime"]["dependency_lock"]["sha256"] = "sha256:" + "0" * 64
    elif change == "daemon":
        index["daemon_sha256"] = "0" * 64
    elif change in {
        "wrong-daemon-observation",
        "changed-restart-observation",
        "stale-daemon-observation",
    }:
        witness = source / RUNNER.R5_WITNESS_FILE
        observation = json.loads(witness.read_bytes())
        if change == "wrong-daemon-observation":
            observation["executables"] = ["0" * 64] * 2
        elif change == "changed-restart-observation":
            observation["executables"][1] = "0" * 64
        else:
            observation["nonce"] = "0" * 32
        raw = json.dumps(observation).encode()
        witness.write_bytes(raw)
        index["cases"][0]["executions"]["run"]["daemon_witness_sha256"] = hashlib.sha256(
            raw
        ).hexdigest()
    elif change == "request":
        index["cases"][0]["executions"]["list"]["request_env_sha256"] = "9" * 64
    elif change == "feature-set":
        index["resolutions"]["runtime"]["consumer_features"] = ["index-sdk-ingress"]
    elif change == "packages":
        index["resolutions"]["kernel"]["packages"] = []
    elif change == "extra-locator-field":
        path = source / "caller-list-completion.json"
        raw = json.dumps({**json.loads(path.read_bytes()), "extra": True}).encode()
        path.write_bytes(raw)
        index["cases"][0]["executions"]["list"]["locator_sha256"] = hashlib.sha256(raw).hexdigest()
    elif change == "receipt":
        (source / "caller-run-receipt.json").write_bytes(
            (source / "kernel-run-receipt.json").read_bytes()
        )
    elif change == "foreign-run":
        manifest["execution_result"]["runs"][0]["events"] = manifest["execution_result"]["runs"][1][
            "events"
        ]
    data = json.dumps(index).encode() + b"\n"
    (source / release.INDEX).write_bytes(data)
    if change != "unsigned-index":
        (source / release.SIGNATURE).write_bytes(
            sign_statement(
                proof["attestation_public_key"],
                proof["id"],
                release._statement(
                    data, manifest_binding(manifest), manifest["started_at"], manifest["ended_at"]
                ),
            )
        )
    manifest["artifacts"] = [archive(tmp_path, a["source_path"]) for a in manifest["artifacts"]]
    with pytest.raises(
        ValueError,
        match="release sources|lock differs|completion bytes|attestation|original QBC|locator differs|feature authority|dependency packages|daemon witness|executed daemon differs",
    ):
        release.verify_release(tmp_path, proof, manifest, cases=CASES)


@pytest.mark.parametrize("variant", ["same", "alias", "distinct", "unavailable"])
def test_qbc_children_do_not_inherit_the_producer_signing_agent(tmp_path, monkeypatch, variant):
    import socket

    signer = str(tmp_path / "s")
    ordinary = str(tmp_path / "a")
    monkeypatch.chdir(tmp_path)
    with socket.socket(socket.AF_UNIX) as signing_agent:
        signing_agent.bind("s")
    if variant == "same":
        ordinary = signer
    elif variant == "alias":
        Path(ordinary).symlink_to(signer)
    elif variant == "distinct":
        with socket.socket(socket.AF_UNIX) as ordinary_agent:
            ordinary_agent.bind("a")
    monkeypatch.setenv(RUNNER.SOCKET_ENV, signer)
    monkeypatch.setenv("SSH_AUTH_SOCK", ordinary)
    monkeypatch.setenv(RUNNER.WITNESS_DIR_ENV, str(tmp_path / "old-witness"))
    monkeypatch.setenv(RUNNER.WITNESS_NONCE_ENV, "b" * 32)
    startup_overrides = {"BASH_ENV": str(tmp_path / "bash-env"), "PYTHONPATH": str(tmp_path)}
    for name, value in startup_overrides.items():
        monkeypatch.setenv(name, value)
    monkeypatch.setenv("PYTHONNOUSERSITE", "0")
    old_cache = str(tmp_path / "existing-cache")
    monkeypatch.setenv("PYTHONPYCACHEPREFIX", old_cache)
    monkeypatch.delenv("PYTHONDONTWRITEBYTECODE", raising=False)
    monkeypatch.setattr(sys, "pycache_prefix", old_cache)
    old_write_setting = sys.dont_write_bytecode
    cache_paths = []

    def run(socket):
        assert socket == signer
        assert RUNNER.SOCKET_ENV not in os.environ
        assert RUNNER.WITNESS_DIR_ENV not in os.environ
        assert RUNNER.WITNESS_NONCE_ENV not in os.environ
        assert os.environ.get("SSH_AUTH_SOCK") == (ordinary if variant == "distinct" else None)
        assert all(name not in os.environ for name in startup_overrides)
        assert os.environ["PYTHONNOUSERSITE"] == "1"
        prefix = Path(os.environ["PYTHONPYCACHEPREFIX"])
        assert str(prefix) != old_cache and prefix.is_dir() and not list(prefix.iterdir())
        assert sys.pycache_prefix == str(prefix) and sys.dont_write_bytecode is True
        cache_paths.append(prefix)
        return 0

    monkeypatch.setattr(RUNNER, "_main", run)
    assert RUNNER.main() == 0
    assert os.environ[RUNNER.SOCKET_ENV] == signer
    assert os.environ["SSH_AUTH_SOCK"] == ordinary
    assert os.environ[RUNNER.WITNESS_DIR_ENV] == str(tmp_path / "old-witness")
    assert os.environ[RUNNER.WITNESS_NONCE_ENV] == "b" * 32
    assert {name: os.environ[name] for name in startup_overrides} == startup_overrides
    assert os.environ["PYTHONNOUSERSITE"] == "0"
    assert os.environ["PYTHONPYCACHEPREFIX"] == sys.pycache_prefix == old_cache
    assert "PYTHONDONTWRITEBYTECODE" not in os.environ
    assert sys.dont_write_bytecode is old_write_setting
    assert len(cache_paths) == 1 and not cache_paths[0].exists()


@pytest.mark.parametrize(
    "change",
    [
        None,
        "quanta",
        "semantica",
        "built",
        "provided",
        "custody",
        "kernel",
        "witness",
        "missing-witness",
        "archived-stdout",
        "archived-locator",
        "split-toolchain",
        "publication-sync",
        "existing-index",
    ],
)
def test_staged_runner_rechecks_witness_and_custody_before_publication(
    tmp_path, monkeypatch, capsys, change
):
    root, pair, resolutions = _complete_resolution_fixture(tmp_path)
    registry = root / "tools/ci/proof-authority.toml"
    registry.parent.mkdir(parents=True)
    fields = ("role", "package", "features", "binary", "kind", "test")
    rows = [
        "  { "
        + ", ".join(f"{key} = {json.dumps(value)}" for key, value in zip(fields, case, strict=True))
        + " }"
        for case in CASES
    ]
    registry.write_text(
        '[[proofs]]\nid = "p11-cross-repo-cutover"\nauthority_state = "staged"\n'
        "paired_execution_cases = [\n" + ",\n".join(rows) + "\n]\n"
    )
    for checkout in (root, pair):
        subprocess.run(["git", "init", "-q", str(checkout)], check=True)
        subprocess.run(["git", "-C", str(checkout), "add", "."], check=True)
        subprocess.run(
            [
                "git",
                "-C",
                str(checkout),
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-qm",
                "Fixture",
            ],
            check=True,
        )
    binaries = {key: tmp_path / key for key in ("built", "provided", "custody")}
    for path in binaries.values():
        path.write_bytes(b"fixed fixture daemon")
        path.chmod(0o700)
    digest = hashlib.sha256(b"fixed fixture daemon").hexdigest()
    evidence = tmp_path / "result"
    witness = evidence / RUNNER.R5_WITNESS_FILE
    index = evidence / "paired-r5-result.json"
    queries = []
    completed_runs = {}
    source_digest = "a" * 64
    completion = SimpleNamespace(
        LOCATOR_PATH_ENV_V1="LOCATOR",
        LOCATOR_NONCE_ENV_V1="NONCE",
        REQUEST_ENV_DIGEST_ENV_V1="REQUEST",
        EXPECTED_SOURCE_HEAD_ENV_V1="SOURCE",
        read_completion_locator_bytes_v1=Path.read_bytes,
    )
    owner = SimpleNamespace(
        _campaign_cargo_admission_owner_v1=lambda: SimpleNamespace(
            campaign_source_snapshot_digest_v1=lambda _root: source_digest
        )
    )

    def resolver(q, s, consumer, feature, lane):
        assert (q, s, lane) == (root, pair, "fixture")
        queries.append((consumer, feature))
        result = resolutions["runtime" if consumer == "quanta-runtime" else "kernel"]
        if len(queries) != 4:
            return result
        if change in binaries:
            binaries[change].write_bytes(b"different bytes after the final resolver")
        elif change in ("quanta", "semantica"):
            ({"quanta": root, "semantica": pair}[change] / "late-change").touch()
        elif change == "kernel":
            return {**result, "consumer": "other"}
        elif change == "witness":
            witness.write_bytes(witness.read_bytes() + b"\n")
        elif change == "missing-witness":
            witness.unlink()
        elif change == "archived-stdout":
            path = evidence / "caller-run-stdout.log"
            path.write_bytes(path.read_bytes() + b"\n")
        elif change == "archived-locator":
            (evidence / "caller-run-completion.json").write_bytes(
                (evidence / "caller-list-completion.json").read_bytes()
            )
        elif change == "existing-index":
            index.write_bytes(b"existing index")
        return result

    def execute_phase(semantica, case, phase, command, _owner, _completion, env, lane, *, cases):
        assert (semantica, lane, cases) == (pair, "fixture", CASES)
        assert env[RUNNER.DAEMON_PATH_ENV] == str(binaries["custody"])
        role, package, _, binary, kind, test = case
        run_id = f"{role}-{phase}"
        physical = tmp_path / "qbc" / run_id
        physical.mkdir(parents=True)
        receipt_path = physical / "receipt.json"
        target_root = tmp_path / "target"
        listed, events = nextest_bytes(package, binary, kind, test)
        stdout = listed if phase == "list" else events
        artifact_env = {"CODEGRAPH_PERSONA": "agent"}
        env_digest = hashlib.sha256(
            json.dumps(artifact_env, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
        toolchain = "9" if change == "split-toolchain" and run_id == "kernel-run" else "8"
        receipt = {
            "lane": lane,
            "last_run_id_v1": run_id,
            "run_state": "finished",
            "last_exit_code": 0,
            "command_exit_code_v1": 0,
            "command": command,
            "source_snapshot_digest": source_digest,
            "toolchain_digest": toolchain * 64,
            "lane_execution_root_v1": str(pair),
            "command_cwd_v1": str(pair),
            "target_dir": str(target_root),
            "artifact_env_payload": artifact_env,
            "artifact_env_digest": env_digest,
        }
        receipt_raw = json.dumps(receipt).encode()
        custody = {
            "lane_id": lane,
            "run_id": run_id,
            "execution_root": str(pair),
            "target_root": str(target_root),
        }
        for stream, name, raw in (
            ("receipt", "receipt.json", receipt_raw),
            ("stdout", "stdout.log", stdout),
            ("stderr", "stderr.log", b""),
        ):
            path = physical / name
            path.write_bytes(raw)
            custody[f"{stream}_path"] = str(path)
            custody[f"{stream}_sha256"] = hashlib.sha256(raw).hexdigest()
        locator = {
            "schema_version": "qbc-verification-completion-locator/v1",
            "lane": lane,
            "run_id": run_id,
            "nonce": env["NONCE"],
            "receipt_path": str(receipt_path),
            "receipt_size_bytes": len(receipt_raw),
            "receipt_sha256": custody["receipt_sha256"],
            "cargo_argv": command,
            "source_snapshot_schema": "source-snapshot-identity.v1",
            "source_snapshot_digest": source_digest,
            "artifact_env_digest": env_digest,
            "request_env_sha256": env["REQUEST"],
            "command_cwd": str(pair),
            "receipt_exit_code": 0,
        }
        Path(env["LOCATOR"]).write_text(json.dumps(locator))
        completed_runs[receipt_path] = SimpleNamespace(
            run=SimpleNamespace(
                lane=lane,
                run_id=run_id,
                exit_code=0,
                command=tuple(command),
                result_directory=physical,
                execution_root=pair,
                command_cwd=pair,
            ),
            receipt_bytes=receipt_raw,
            stdout=stdout,
            stderr=b"",
            receipt_digest=custody["receipt_sha256"],
            process_exit_code_v1=0,
            receipt_payload_v1=lambda: receipt,
            custody_payload_v1=lambda: custody,
        )
        if run_id == "caller-run":
            assert env[RUNNER.WITNESS_DIR_ENV] == str(evidence)
            witness.write_text(
                json.dumps(
                    {
                        "schema_version": 1,
                        "test": f"{package}::{binary}${test}",
                        "nonce": env[RUNNER.WITNESS_NONCE_ENV],
                        "executables": [digest, digest],
                    }
                )
            )
        return 0

    if change == "publication-sync":
        sync = os.fsync

        def fail_index_sync(descriptor):
            if index.exists():
                raise OSError(errno.EIO, "candidate index sync failed")
            sync(descriptor)

        monkeypatch.setattr(RUNNER.os, "fsync", fail_index_sync)
    monkeypatch.delenv(RUNNER.SOCKET_ENV, raising=False)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "paired_r5_result.py",
            "--quanta-root",
            str(root),
            "--semantica-root",
            str(pair),
            "--quanta-head",
            RUNNER.frozen_source_head(root),
            "--semantica-head",
            RUNNER.frozen_source_head(pair),
            "--evidence-root",
            str(evidence),
            "--qbc-lane",
            "fixture",
            "--daemon-digest",
            digest,
            "--built-binary",
            str(binaries["built"]),
            "--provided-binary",
            str(binaries["provided"]),
            "--custody-binary",
            str(binaries["custody"]),
            "--runtime-resolution",
            json.dumps(resolutions["runtime"]),
            "--kernel-resolution",
            json.dumps(resolutions["kernel"]),
        ],
    )
    completed = SimpleNamespace(read_completed_run_receipt_at_path_v1=completed_runs.__getitem__)
    monkeypatch.setattr(RUNNER, "_qbc_modules", lambda _root, _head: (completed, completion, owner))
    monkeypatch.setattr(RUNNER, "_execute_qbc_phase", execute_phase)
    monkeypatch.setattr(RUNNER, "_resolved_after", resolver)
    errors = {
        "quanta": "source changed",
        "semantica": "source changed",
        "built": "daemon bytes changed",
        "provided": "daemon bytes changed",
        "custody": "daemon bytes changed",
        "kernel": "resolver identity changed",
        "witness": "archived capture",
        "archived-stdout": "completion bytes differ",
        "archived-locator": "completion locator differs",
        "split-toolchain": "share one lane",
    }
    if change == "missing-witness":
        with pytest.raises(FileNotFoundError, match=RUNNER.R5_WITNESS_FILE):
            RUNNER.main()
    elif change == "publication-sync":
        with pytest.raises(OSError, match="candidate index sync failed"):
            RUNNER.main()
    elif change == "existing-index":
        with pytest.raises(FileExistsError):
            RUNNER.main()
        assert index.read_bytes() == b"existing index"
    elif change in errors:
        with pytest.raises(ValueError, match=errors[change]):
            RUNNER.main()
    else:
        assert RUNNER.main() == 0
        result = json.loads(index.read_bytes())
        assert result["qualification"] == "runner-candidate-only"
        assert result["cases"][0]["executions"]["run"]["daemon_witness_sha256"] == (
            hashlib.sha256(witness.read_bytes()).hexdigest()
        )
        assert capsys.readouterr().out == f"paired-r5-runner-candidate: {index}\n"
    assert index.exists() == (change in (None, "existing-index"))
    assert not list(evidence.glob(".paired-r5-result.json.*.tmp"))
    assert len(completed_runs) == 4
    assert (
        queries
        == [
            ("quanta-runtime", "index-sdk-ingress,retrieval-authority-contract-surface"),
            ("quanta-runtime-retrieval-kernel", "index-sdk-ingress-surface"),
        ]
        * 2
    )
