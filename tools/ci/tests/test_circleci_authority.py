"""CircleCI rails must remain reachable and propagate command failures."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import runpy
import shlex
import subprocess
import sys
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]
CHECKER = ROOT / "tools/ci/lint/check-test-authority.py"
CONFIG = ROOT / ".circleci/config.yml"


def _checker():
    spec = importlib.util.spec_from_file_location("check_test_authority_circleci", CHECKER)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    import sys

    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _rail_violations(root: Path, command: str = "just owner-test"):
    module = _checker()
    violations = []
    module._validate_rail_binding(
        root=root,
        catalog=root / "authority.toml",
        rail_id="pr-owner",
        raw_rail={
            "workflow": ".circleci/config.yml",
            "job": "verify",
            "step": "owner test",
            "tier": "pr",
        },
        command=command,
        violations=violations,
    )
    return [violation.message for violation in violations]


def _config(root: Path):
    path = root / ".circleci/config.yml"
    path.parent.mkdir(parents=True)
    data = {
        "version": 2.1,
        "parameters": {"run_heavy": {"type": "boolean", "default": False}},
        "jobs": {
            "verify": {
                "steps": [
                    {
                        "run": {
                            "name": "owner test",
                            "command": "set -euo pipefail\njust owner-test\n",
                        }
                    }
                ]
            }
        },
        "workflows": {
            "regular": {
                "when": yaml.safe_load(CONFIG.read_text(encoding="utf-8"))["workflows"]["regular"][
                    "when"
                ],
                "jobs": ["verify"],
            }
        },
    }
    path.write_text(yaml.safe_dump(data), encoding="utf-8")
    return data, path


def test_real_catalog_binds_to_circleci():
    assert _checker().audit_catalog() == []


def _required_run_steps(job, *required_names):
    steps = job["steps"]
    runs = [step["run"] for step in steps if isinstance(step, dict) and "run" in step]
    names = [run.get("name") if isinstance(run, dict) else None for run in runs]
    assert all(names.count(name) == 1 for name in required_names), names
    positions = [names.index(name) for name in required_names]
    assert positions == sorted(positions), names

    checker = _checker()
    required = [runs[index] for index in positions]
    for run in required:
        assert checker._circleci_run_error(job, run) is None, run["name"]
    for run in runs[: positions[-1] + 1]:
        assert not checker._circleci_run_halts_job(run), run
    return required


def test_legacy_reference_readiness_runs_without_default_features_after_linux_pin():
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    job = config["jobs"]["heavy-correctness"]
    steps = job["steps"]
    install, versions, _, readiness = _required_run_steps(
        job,
        "Install verification tools",
        "Verify tool versions",
        "Verify pinned Legacy searchd input",
        "Pinned Legacy reference readiness without default features",
    )
    static_steps = config["jobs"]["verify-rust-static"]["steps"]
    assert {"run": install} in static_steps
    assert {"run": versions} in static_steps

    recipe = "rust-test-process-readiness-no-default"
    log = "artifacts/test-authority/no-default-nextest.log"
    assert f"just {recipe} 2>&1 | tee {log}" in readiness["command"].splitlines()
    assert {"store_artifacts": {"path": log, "destination": log.removeprefix("artifacts/")}} in (
        steps[steps.index({"run": readiness}) + 1 :]
    )

    command = subprocess.run(
        ["just", "--dry-run", recipe], cwd=ROOT, check=True, capture_output=True, text=True
    ).stderr
    assert "nextest run -p quanta-index-searchd-runtime --test process_readiness_owner_v1" in (
        command
    )
    assert "--no-default-features" in command
    assert "--no-tests fail" in command
    assert "--all-features" not in command


def test_f15_runs_three_maintained_cases_without_default_features_after_linux_pin():
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    job = config["jobs"]["heavy-correctness"]
    _, f15 = _required_run_steps(
        job, "Verify pinned Legacy searchd input", "Pinned Legacy F15 without default features"
    )
    recipe = "rust-test-f15-no-default"
    log = "artifacts/test-authority/f15-no-default-nextest.log"
    assert f"just {recipe} 2>&1 | tee {log}" in f15["command"].splitlines()
    assert {"store_artifacts": {"path": log, "destination": log.removeprefix("artifacts/")}} in (
        job["steps"][job["steps"].index({"run": f15}) + 1 :]
    )

    command = subprocess.run(
        ["just", "--dry-run", recipe], cwd=ROOT, check=True, capture_output=True, text=True
    ).stderr
    assert "package=quanta-index-lexical" in command
    assert "binary=f15_file_authority" in command
    assert "delta=delta_delete_noop_and_cold_open_match_independent_full_build" in command
    assert "nontext=nontext_source_stays_path_only_across_cold_delta_noop_and_delete" in command
    assert "damage=damaged_file_bodies_stay_quarantined_after_restart" in command
    assert '-E "test(=$delta) | test(=$nontext) | test(=$damage)"' in command
    assert "--no-default-features --locked --offline --no-tests fail" in command
    assert '"$package" "$binary" "$delta" "$nontext" "$damage"' in command
    assert "-p quanta-index-searchd-runtime --test native_index_uds" in command
    assert "-E 'test(file_body_quarantine)'" in command


@pytest.mark.parametrize(
    ("passing", "producer_exit", "native_exit", "expected_exit"),
    [(3, 0, 0, 0), (2, 0, 0, 1), (0, 124, 0, 124), (3, 0, 100, 100)],
)
def test_f15_recipe_requires_three_passes_and_preserves_producer_failure(
    tmp_path, passing, producer_exit, native_exit, expected_exit
):
    package, binary = "quanta-index-lexical", "f15_file_authority"
    cases = (
        "delta_delete_noop_and_cold_open_match_independent_full_build",
        "nontext_source_stays_path_only_across_cold_delta_noop_and_delete",
        "damaged_file_bodies_stay_quarantined_after_restart",
    )
    suite = {"crate": package, "test_binary": binary, "kind": "test"}
    events = [{"type": "suite", "event": "started", "test_count": passing, "nextest": suite}]
    for case in cases[:passing]:
        name = f"{package}::{binary}${case}"
        events.extend({"type": "test", "event": event, "name": name} for event in ("started", "ok"))
    events.append(
        {
            "type": "suite",
            "event": "ok",
            "passed": passing,
            "failed": 0,
            "ignored": 0,
            "measured": 0,
            "filtered_out": len(cases) - passing,
            "nextest": suite,
        }
    )
    payload = "" if producer_exit else "".join(f"{json.dumps(event)}\n" for event in events)
    stub = tmp_path / "cargo wrappers" / "cargow"
    stub.parent.mkdir()
    stub.write_text(
        "#!/bin/sh\n"
        f'case " $* " in *" --test native_index_uds "*) exit {native_exit};; esac\n'
        f"printf '%s' {shlex.quote(payload)}\nexit {producer_exit}\n"
    )
    stub.chmod(0o700)
    result = subprocess.run(
        ["just", "--set", "cargo", str(stub), "rust-test-f15-no-default"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    assert result.returncode == expected_exit, (result.stdout, result.stderr)


@pytest.mark.parametrize(
    "omit_binary",
    [None, "native_index_uds", "native_product_process"]
    if sys.platform.startswith("linux")
    else [None, "native_index_uds"],
)
def test_native_history_recipe_requires_each_platform_body(tmp_path, omit_binary):
    package = "quanta-index-searchd-runtime"
    owner = "read_sessions::query::history"
    roster = {
        "native_index_uds": [
            f"{owner}::native_history_sdk_full_delta_retained_pages_and_reopen",
            f"{owner}::native_history_append_original_publishes_without_legacy_ack_and_reopens",
            f"{owner}::source_and_next_history_generation_publish_in_one_view_and_reopen",
            f"{owner}::native_restore::native_history_restore_full_append_and_reopen_cross_root",
            f"{owner}::dsl_fixture::native_history_dsl_fixed_ledger_warm_cold",
        ],
    }
    if sys.platform.startswith("linux"):
        roster["native_index_uds"].append(
            f"{owner}::native_protected::native_protected_two_source_heads_import_seal_and_reopen",
        )
        roster["native_product_process"] = [
            "native_boot::offline_legacy_history_export_streams_real_rows_and_terminal_ack",
            "native_boot::legacy_to_fresh_native_preserves_old_root_and_refuses_unverified_rollback",
        ]
    events = []
    for binary, names in roster.items():
        witnesses = names[:-1] if binary == omit_binary else names
        suite = {"crate": package, "test_binary": binary, "kind": "test"}
        events.append(
            {"type": "suite", "event": "started", "test_count": len(witnesses), "nextest": suite}
        )
        for witness in witnesses:
            name = f"{package}::{binary}${witness}"
            events.extend(
                {"type": "test", "event": event, "name": name} for event in ("started", "ok")
            )
        events.append(
            {
                "type": "suite",
                "event": "ok",
                "passed": len(witnesses),
                "failed": 0,
                "ignored": 0,
                "nextest": suite,
            }
        )

    args = tmp_path / "nextest-args"
    payload = "".join(f"{json.dumps(event)}\n" for event in events)
    stub = tmp_path / "cargow"
    stub.write_text(
        f"#!/bin/sh\nprintf '%s\\n' \"$@\" > {shlex.quote(str(args))}\n"
        f"printf '%s' {shlex.quote(payload)}\n"
    )
    stub.chmod(0o700)
    env = {
        **os.environ,
        "QUANTA_INDEX_LEGACY_SEARCHD_BIN": str(stub),
        "QUANTA_INDEX_LEGACY_SEARCHD_SHA256": hashlib.sha256(stub.read_bytes()).hexdigest(),
    }
    result = subprocess.run(
        ["just", "--set", "cargo", str(stub), "rust-test-native-history"],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
    )
    assert result.returncode == int(omit_binary is not None), (result.stdout, result.stderr)
    argv = args.read_text().splitlines()
    targets = {argv[i + 1] for i, arg in enumerate(argv) if arg == "--test"}
    assert targets == roster.keys()
    selections = argv[argv.index("-E") + 1].split(" | ")
    assert selections == [
        f"(binary(={binary}) & test(={name}))" for binary, names in roster.items() for name in names
    ]
    if omit_binary is not None:
        assert "nextest execution differs from collected tests" in result.stderr
    else:
        expected_count = sum(map(len, roster.values()))
        assert result.stdout == f"Native History: {expected_count}/{expected_count} passed\n"


@pytest.mark.parametrize("omit_kernel_body", [False, True])
def test_native_history_owner_recipe_requires_catalog_and_kernel_bodies(tmp_path, omit_kernel_body):
    roster = {
        "quanta-index-catalog": (
            "quanta_index_catalog",
            [
                "native::authority::tests::offline_protected_routes::protected_route_refuses_corrupt_registered_edge_before_visiting_publications",
                "native::authority::tests::offline_protected_routes::orphan_view_root_with_registered_workspace_refuses_current_and_retained_routes",
                "native::authority::tests::offline_source_quiescence::single_workspace_boot_and_rollback_refuse_orphan_view_root",
                "native::authority::tests::offline_source_quiescence::offline_head_checks_charge_current_and_history_payloads_before_decode",
                "native::authority::tests::read_ticket_holds_scratch_through_pin_seal_and_refunds_it",
                "native::authority::tests::read_ticket_refuses_root_scratch_without_pin_or_leaked_lease",
                "native::authority::tests::materialization_pin_is_atomic_and_does_not_extend_execution_claim",
            ],
        ),
        "quanta-index-kernel": (
            "quanta_index_kernel",
            [
                "request_decode::tests::source_history_resync_refunds_retained_backing_when_history_input_is_missing",
                "request_decode::tests::source_history_resync_requires_exact_decode_budget_and_refunds_it",
                "request_decode::tests::initial_and_post_reservation_original_cancel_never_become_corruption",
            ],
        ),
    }
    payloads = {}
    for package, (binary, names) in roster.items():
        selected = names[:-1] if omit_kernel_body and package == "quanta-index-kernel" else names
        suite = {"crate": package, "test_binary": binary, "kind": "lib"}
        events = [
            {"type": "suite", "event": "started", "test_count": len(selected), "nextest": suite}
        ]
        for name in selected:
            identity = f"{package}::{binary}${name}"
            events.extend(
                {"type": "test", "event": event, "name": identity} for event in ("started", "ok")
            )
        events.append(
            {
                "type": "suite",
                "event": "ok",
                "passed": len(selected),
                "failed": 0,
                "ignored": 0,
                "nextest": suite,
            }
        )
        payloads[package] = "".join(f"{json.dumps(event)}\n" for event in events)

    calls = tmp_path / "nextest-calls.jsonl"
    stub = tmp_path / "cargow"
    stub.write_text(
        "#!/usr/bin/env python3\nimport json\nimport sys\nfrom pathlib import Path\n"
        "argv = sys.argv[1:]\n"
        f"Path({str(calls)!r}).open('a').write(json.dumps(argv) + '\\n')\n"
        "package = argv[argv.index('-p') + 1]\n"
        f"sys.stdout.write({payloads!r}[package])\n"
    )
    stub.chmod(0o700)
    environment = os.environ.copy()
    environment.pop("QUANTA_INDEX_LEGACY_SEARCHD_BIN", None)
    environment.pop("QUANTA_INDEX_LEGACY_SEARCHD_SHA256", None)
    result = subprocess.run(
        ["just", "--set", "cargo", str(stub), "rust-test-native-history-owners"],
        cwd=ROOT,
        env=environment,
        capture_output=True,
        text=True,
    )
    assert result.returncode == int(omit_kernel_body), (result.stdout, result.stderr)
    invocations = [json.loads(line) for line in calls.read_text().splitlines()]
    assert [argv[argv.index("-p") + 1] for argv in invocations] == list(roster)
    for argv, (_package, (binary, names)) in zip(invocations, roster.items()):
        assert "--lib" in argv
        assert "--offline" in argv
        assert argv[argv.index("-E") + 1].split(" | ") == [
            f"(binary(={binary}) & test(={name}))" for name in names
        ]
    if omit_kernel_body:
        assert "nextest execution differs from collected tests" in result.stderr


@pytest.mark.parametrize(
    "recipe",
    [
        ["rust-test-process-readiness-no-default"],
        ["rust-prepare-native-dsl-history", "export", "daemon", "digest"],
    ],
)
@pytest.mark.parametrize("invalid_pin", ["missing", "digest"])
def test_history_recipes_require_legacy_pin_before_cargo(tmp_path, recipe, invalid_pin):
    env = os.environ.copy()
    env.pop("QUANTA_INDEX_LEGACY_SEARCHD_BIN", None)
    env.pop("QUANTA_INDEX_LEGACY_SEARCHD_SHA256", None)
    expected = "QUANTA_INDEX_LEGACY_SEARCHD_BIN must be an absolute regular executable"
    if invalid_pin == "digest":
        stub = tmp_path / "old-q"
        stub.write_text("#!/bin/sh\nexit 0\n")
        stub.chmod(0o700)
        env["QUANTA_INDEX_LEGACY_SEARCHD_BIN"] = str(stub)
        env["QUANTA_INDEX_LEGACY_SEARCHD_SHA256"] = "0" * 64
        expected = "Legacy searchd SHA-256 differs from the pinned input"
    result = subprocess.run(
        ["just", "--set", "cargo", "false", *recipe],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 1
    assert expected in result.stderr


def test_circleci_custody_unset_allows_only_shell_startup_hooks(tmp_path: Path):
    data, path = _config(tmp_path)
    command = data["jobs"]["verify"]["steps"][0]["run"]
    command["command"] = "set -euo pipefail\nunset BASH_ENV ENV\njust owner-test\n"
    path.write_text(yaml.safe_dump(data), encoding="utf-8")
    assert _rail_violations(tmp_path) == []

    command["command"] = "set -euo pipefail\nunset PATH\njust owner-test\n"
    path.write_text(yaml.safe_dump(data), encoding="utf-8")
    assert any("does not execute declared command" in item for item in _rail_violations(tmp_path))


def test_circleci_rail_rejects_detached_and_failure_swallowing_steps(tmp_path: Path):
    data, path = _config(tmp_path)
    assert _rail_violations(tmp_path) == []

    data["workflows"]["regular"]["jobs"] = []
    path.write_text(yaml.safe_dump(data), encoding="utf-8")
    assert any("not in CircleCI regular" in message for message in _rail_violations(tmp_path))

    data["workflows"]["regular"]["jobs"] = ["verify"]
    data["jobs"]["verify"]["steps"][0]["run"]["command"] = "set +e\njust owner-test\ntrue\n"
    path.write_text(yaml.safe_dump(data), encoding="utf-8")
    assert any(
        "does not execute declared command" in message for message in _rail_violations(tmp_path)
    )

    data["jobs"]["verify"]["steps"][0]["run"]["command"] = "set -euo pipefail\njust owner-test\n"
    data["jobs"]["verify"]["steps"][0]["run"]["when"] = "on_fail"
    path.write_text(yaml.safe_dump(data), encoding="utf-8")
    assert any("conditional" in message for message in _rail_violations(tmp_path))


def test_heavy_work_is_off_by_default():
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    assert config["parameters"]["run_heavy"]["default"] is False
    assert not _checker()._circleci_gate_enabled(
        config["workflows"]["regular"], run_heavy=True, event="pull_request", branch="main"
    )
    assert config["workflows"]["manual-heavy"]["when"] == "<< pipeline.parameters.run_heavy >>"


@pytest.mark.parametrize(
    ("event", "branch", "expected_tier"),
    [
        ("pull_request", "feature", "pr"),
        ("pull_request", "main", "pr"),
        ("push", "main", "main"),
        ("push", "feature", None),
        ("push", "", None),
        ("api", "main", None),
        ("tag", "main", None),
        ("", "main", None),
    ],
)
def test_regular_admission_matches_receipt_context_before_cargo(event, branch, expected_tier):
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    regular = config["workflows"]["regular"]
    module = _checker()
    assert module._circleci_gate_enabled(regular, run_heavy=False, event=event, branch=branch) is (
        expected_tier is not None
    )
    assert not module._circleci_gate_enabled(regular, run_heavy=True, event=event, branch=branch)
    steps = config["jobs"]["verify-rust-tests"]["steps"]
    script = next(
        step["run"]["command"]
        for step in steps
        if "run" in step and step["run"]["name"] == "rust nextest"
    )
    preflight = steps[1]["run"]
    assert preflight["name"] == "Verify verification receipt context"
    selection = 'if [[ "$CI_EVENT_NAME"' + script.split('if [[ "$CI_EVENT_NAME"', 1)[1]
    selection = selection.split("fi\n", 1)[0] + "fi\n"
    for context_script in (preflight["command"], "set -euo pipefail\n" + selection):
        result = subprocess.run(
            ["bash", "-c", context_script + 'printf "%s\\n%s\\n" "$tier" "$rail"\n'],
            env={**os.environ, "CI_EVENT_NAME": event, "CIRCLE_BRANCH": branch},
            capture_output=True,
            text=True,
            check=False,
            timeout=10,
        )
        if expected_tier is None:
            assert result.returncode == 2
            assert "No PR/main verification receipt" in result.stderr
            assert result.stdout == ""
        else:
            assert result.returncode == 0, result.stderr
            assert result.stdout.splitlines() == [
                expected_tier,
                f"{expected_tier}-workspace-nextest",
            ]


def test_workspace_jobs_preflight_the_same_pinned_legacy_reference(tmp_path):
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    rust_job = config["jobs"]["verify-rust-tests"]
    coverage_job = config["jobs"]["verify-pr-coverage"]
    heavy_job = config["jobs"]["heavy-correctness"]
    key = "QUANTA_INDEX_LEGACY_SEARCHD_SHA256"
    size_key = "QUANTA_INDEX_LEGACY_SEARCHD_BYTES"
    pin = rust_job["environment"][key]
    assert pin == coverage_job["environment"][key] == heavy_job["environment"][key]
    assert len(pin) == 64 and set(pin) <= set("0123456789abcdef")
    size = rust_job["environment"][size_key]
    assert size == coverage_job["environment"][size_key] == heavy_job["environment"][size_key]
    assert size.isdecimal() and int(size) > 0

    def preflight(job, nextest_name):
        steps = job["steps"]
        _, _, verify, nextest = _required_run_steps(
            job,
            "Install verification tools",
            "Verify tool versions",
            "Verify pinned Legacy searchd input",
            nextest_name,
        )
        assert {
            "store_artifacts": {"path": "artifacts/test-authority", "destination": "test-authority"}
        } in (steps[steps.index({"run": nextest}) + 1 :])
        return verify["command"]

    command = preflight(rust_job, "rust nextest")
    assert command == preflight(coverage_job, "Changed production Rust line coverage")
    assert command == preflight(heavy_job, "rust nextest authority receipt")
    binary = tmp_path / "legacy searchd"
    original = b"fixed executable bytes"
    binary.write_bytes(original)
    binary.chmod(0o700)
    link = tmp_path / "link"
    link.symlink_to(binary)
    fixture_pin = hashlib.sha256(original).hexdigest()

    path_key = "QUANTA_INDEX_LEGACY_SEARCHD_BIN"
    url_key = "QUANTA_INDEX_LEGACY_SEARCHD_ARTIFACT_URL"

    input_evidence = tmp_path / "artifacts/test-authority/legacy-searchd-input.txt"

    def run(path, extra_env=None):
        env = {
            name: value
            for name, value in os.environ.items()
            if name not in {path_key, url_key, "BASH_ENV", "ENV"}
        }
        env[key] = fixture_pin
        env[size_key] = str(len(original))
        if extra_env is not None:
            env.update(extra_env)
        if path is not None:
            env[path_key] = str(path)
        return subprocess.run(
            ["bash", "-c", command],
            cwd=tmp_path,
            env=env,
            check=False,
            capture_output=True,
            text=True,
            timeout=10,
        )

    observed = f"bytes={len(original)} sha256={fixture_pin}\n"
    supplied_path = run(binary)
    assert supplied_path.returncode == 0, supplied_path.stderr
    assert supplied_path.stdout == f"transport=runner-path {observed}"
    assert input_evidence.read_text(encoding="utf-8") == supplied_path.stdout
    input_evidence.unlink()
    missing = run(None)
    assert missing.returncode != 0
    assert "approved Linux Legacy searchd path" in missing.stderr
    symlink = run(link)
    assert symlink.returncode == 2
    assert "absolute regular executable" in symlink.stderr
    for wrong_size in (len(original) - 1, len(original) + 1):
        mismatch = run(binary, {size_key: str(wrong_size)})
        assert mismatch.returncode == 2
        assert "size differs from the approved Linux input" in mismatch.stderr
    tampered = b"X" + original[1:]
    binary.write_bytes(tampered)
    rejected = run(binary)
    assert rejected.returncode == 2
    assert "SHA-256 differs from the approved Linux input" in rejected.stderr
    assert not input_evidence.exists()

    curl = tmp_path / "curl"
    curl.write_text(
        "#!/bin/bash\n"
        "set -eu\n"
        'while [[ "$1" != --output ]]; do shift; done\n'
        'cat "$LEGACY_ARTIFACT_FIXTURE" > "$2"\n'
        'touch "$LEGACY_ARTIFACT_TRANSFER_COMPLETE"\n',
        encoding="utf-8",
    )
    curl.chmod(0o700)
    bash_env = tmp_path / "job-env"
    bash_env.touch()
    downloads = tmp_path / "downloads with spaces"
    downloads.mkdir()
    transfer_complete = tmp_path / "transfer-complete"
    artifact_env = {
        "PATH": f"{tmp_path}{os.pathsep}{os.environ['PATH']}",
        "BASH_ENV": str(bash_env),
        "TMPDIR": str(downloads),
        url_key: "https://artifact.example/quanta-index-searchd",
        "LEGACY_ARTIFACT_FIXTURE": str(binary),
        "LEGACY_ARTIFACT_TRANSFER_COMPLETE": str(transfer_complete),
    }
    binary.write_bytes(b"x" * 4096)
    oversize = run(None, artifact_env)
    assert oversize.returncode != 0
    assert not transfer_complete.exists()
    assert bash_env.read_text(encoding="utf-8") == ""
    assert not list(downloads.iterdir())

    binary.write_bytes(tampered)
    invalid_download = run(None, artifact_env)
    assert invalid_download.returncode == 2
    assert "SHA-256 differs from the approved Linux input" in invalid_download.stderr
    assert bash_env.read_text(encoding="utf-8") == ""
    assert not list(downloads.iterdir())

    binary.write_bytes(original)
    short_download = run(None, {**artifact_env, size_key: str(len(original) + 1)})
    assert short_download.returncode == 2
    assert "size differs from the approved Linux input" in short_download.stderr
    assert bash_env.read_text(encoding="utf-8") == ""
    assert not list(downloads.iterdir())
    assert not input_evidence.exists()

    transfer_complete.unlink()
    failed_handoff = run(None, {**artifact_env, "BASH_ENV": str(tmp_path)})
    assert failed_handoff.returncode != 0
    assert transfer_complete.exists()
    assert failed_handoff.stdout == ""
    assert not input_evidence.exists()
    assert not list(downloads.iterdir())

    discarded_handoff = run(None, {**artifact_env, "BASH_ENV": "/dev/null"})
    assert discarded_handoff.returncode == 2
    assert "BASH_ENV must be a regular file" in discarded_handoff.stderr
    assert discarded_handoff.stdout == ""
    assert not input_evidence.exists()
    assert not list(downloads.iterdir())

    supplied = run(None, artifact_env)
    assert supplied.returncode == 0, supplied.stderr
    assert supplied.stdout == f"transport=https {observed}"
    assert input_evidence.read_text(encoding="utf-8") == supplied.stdout
    next_step = subprocess.run(
        ["bash", "-c", 'printf "%s" "${QUANTA_INDEX_LEGACY_SEARCHD_BIN:?}"'],
        env={"BASH_ENV": str(bash_env)},
        check=True,
        capture_output=True,
        text=True,
        timeout=10,
    )
    downloaded = Path(next_step.stdout)
    assert downloaded.read_bytes() == original
    assert os.access(downloaded, os.X_OK)


def test_regular_gate_rejects_unresolved_or_unsupported_logic():
    module = _checker()
    for gate in (
        {"when": {"matches": []}},
        {"when": "<< pipeline.unknown >>"},
        {"when": True, "unless": False},
        {"when": {"and": []}},
    ):
        with pytest.raises(ValueError):
            module._circleci_gate_enabled(gate, run_heavy=False, event="push", branch="main")


def test_authority_rejects_old_feature_push_admission(tmp_path: Path):
    data, path = _config(tmp_path)
    data["workflows"]["regular"].pop("when")
    data["workflows"]["regular"]["unless"] = "<< pipeline.parameters.run_heavy >>"
    path.write_text(yaml.safe_dump(data), encoding="utf-8")
    assert any("receipt context mismatch" in item for item in _rail_violations(tmp_path))


def test_all_job_commands_propagate_failures():
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    for job in config["jobs"].values():
        for step in job["steps"]:
            run = step.get("run") if isinstance(step, dict) else None
            if run is not None:
                assert run["command"].lstrip().startswith("set -euo pipefail\n")


def test_regular_aggregate_waits_for_source_bound_workers_and_pr_coverage():
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    workers = ["verify-rust-static", "verify-rust-tests", "verify-rust-docs", "verify-rust-bench"]
    core_jobs = ["verify-python", *workers]
    entries = config["workflows"]["regular"]["jobs"]
    assert entries[0] == {"verify": {"requires": [*core_jobs, "verify-pr-coverage"]}}
    assert entries[1:-1] == core_jobs
    assert entries[-1] == {
        "verify-pr-coverage": {
            "requires": core_jobs,
            "filters": 'pipeline.event.name == "pull_request"',
        }
    }
    gate_steps = config["jobs"]["verify"]["steps"]
    assert gate_steps[0] == "checkout"
    assert 'test "$(git rev-parse HEAD)" = "$CIRCLE_SHA1"' in gate_steps[1]["run"]["command"]
    python_steps = config["jobs"]["verify-python"]["steps"]
    for name in workers:
        steps = config["jobs"][name]["steps"]
        common_steps = steps[:1] + steps[2:] if name == "verify-rust-tests" else steps
        assert common_steps[:3] == python_steps[:3]
        assert steps[-1] == python_steps[-1]
    source_guard = python_steps[-1]["run"]["command"]
    assert 'test "$(git rev-parse HEAD)" = "$CIRCLE_SHA1"' in source_guard
    assert "git diff --exit-code" in source_guard
    assert "git status --porcelain=v1 --untracked-files=all" in source_guard
    assert "exit 1" in source_guard
    coverage_steps = config["jobs"]["verify-pr-coverage"]["steps"]
    assert coverage_steps[0] == "checkout"
    assert coverage_steps[-2] == python_steps[-1]
    assert "store_artifacts" in coverage_steps[-1]
    rust_steps = config["jobs"]["verify-rust-tests"]["steps"]
    assert rust_steps[0] == python_steps[0] == "checkout"
    assert rust_steps[2:4] == python_steps[1:3]
    assert '"$HOME/.zprofile"' in rust_steps[2]["run"]["command"]
    assert 'test "$(git rev-parse HEAD)" = "$CIRCLE_SHA1"' in rust_steps[3]["run"]["command"]
    rust_names = {step["run"]["name"] for step in rust_steps if "run" in step}
    python_names = {step["run"]["name"] for step in python_steps if "run" in step}
    assert "rust nextest" in rust_names and "rust nextest" not in python_names
    module_step = next(
        step["run"]
        for step in python_steps
        if isinstance(step, dict)
        and step.get("run", {}).get("name") == "Verify guarded module snapshots"
    )
    assert "cargo install cargo-modules --version 0.26.0 --locked" in module_step["command"]
    assert (
        "uv run --frozen --extra dev python tools/ci/lint/check-cargo-modules-snapshot.py"
        in module_step["command"]
    )
    python_install = next(
        step["run"]
        for step in python_steps
        if isinstance(step, dict)
        and step.get("run", {}).get("name") == "Install locked Python dependencies"
    )
    assert '"$HOME/.zprofile"' in python_install["command"]
    assert "zsh -lc 'python3 --version'" in python_install["command"]
    assert "Python policy and tooling tests" in python_names
    assert "Produce and validate P00 authority manifest" in python_names
    assert "Python policy and tooling tests" not in rust_names


def test_regular_ci_runs_the_pinned_public_api_ratchet():
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    regular_jobs = config["workflows"]["regular"]["jobs"]
    assert "verify-python" in regular_jobs
    steps = config["jobs"]["verify-python"]["steps"]
    gate_index = next(
        index
        for index, step in enumerate(steps)
        if isinstance(step, dict)
        and step.get("run", {}).get("name") == "Verify guarded public API snapshots"
    )
    gate = steps[gate_index]["run"]
    python_install_index = next(
        index
        for index, step in enumerate(steps)
        if isinstance(step, dict)
        and step.get("run", {}).get("name") == "Install locked Python dependencies"
    )
    tool_install_index = next(
        index
        for index, step in enumerate(steps)
        if isinstance(step, dict)
        and step.get("run", {}).get("name") == "Install pinned public API tooling"
    )
    assert python_install_index < tool_install_index < gate_index
    assert gate_index < next(
        index
        for index, step in enumerate(steps)
        if isinstance(step, dict)
        and step.get("run", {}).get("name") == "Python policy and tooling tests"
    )
    assert gate.get("when", "always") == "always"
    command = gate["command"]
    assert command.startswith("set -euo pipefail\nunset BASH_ENV ENV\n")
    assert "--update-baseline" not in command
    public_api = runpy.run_path(str(ROOT / "tools/ci/lint/check-public-api.py"))
    assert public_api["GUARDED_CRATES"] == [
        "quanta-index-contract-base",
        "quanta-index-contract",
        "quanta-index-sdk",
    ]
    toolchain = public_api["PUBLIC_API_TOOLCHAIN"]
    install = steps[tool_install_index]["run"]["command"]
    assert install.startswith("set -euo pipefail\nunset BASH_ENV ENV\n")
    assert f"rustup toolchain install {toolchain} --profile minimal" in install
    assert "cargo install cargo-public-api --version 0.51.0 --locked" in install
    assert '[[ "$(cargo public-api --version)" == "cargo-public-api 0.51.0" ]]' in install
    assert (
        "uv run --frozen --extra dev python tools/ci/lint/check-public-api.py"
        in command.splitlines()
    )
    assert steps[tool_install_index - 1].get("restore_cache")
    cache = steps[tool_install_index - 1]["restore_cache"]["keys"][0]
    assert "cargo-public-api0.51.0" in cache
    assert steps[gate_index - 1]["save_cache"]["paths"] == ["~/.cargo/bin/cargo-public-api"]


def test_pr_coverage_uses_exact_base_and_fails_closed(tmp_path):
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    steps = config["jobs"]["verify-pr-coverage"]["steps"]
    coverage = next(
        step["run"]
        for step in steps
        if isinstance(step, dict)
        and step.get("run", {}).get("name") == "Changed production Rust line coverage"
    )
    assert coverage["environment"]["CI_PR_BASE_SHA"] == (
        "<< pipeline.event.github.pull_request.base.sha >>"
    )
    assert "cargo install cargo-llvm-cov --version 0.8.5 --locked" in next(
        step["run"]["command"]
        for step in steps
        if isinstance(step, dict)
        and step.get("run", {}).get("name") == "Install pinned coverage tooling"
    )

    tools = tmp_path / "bin"
    tools.mkdir()
    scripts = tmp_path / "scripts"
    scripts.mkdir()
    for path in (tools / "git", tools / "uv", scripts / "cargow"):
        path.write_text(
            "#!/bin/sh\n"
            'printf "%s %s\\n" "${0##*/}" "$*"\n'
            'if [ "${0##*/}" = "${FAIL_COMMAND:-}" ]; then exit 19; fi\n',
            encoding="utf-8",
        )
        path.chmod(0o700)

    base = "1a" * 20
    lcov = "/tmp/quanta-index-coverage.lcov"
    expected = [
        f"git fetch --no-tags origin {base}",
        f"git cat-file -e {base}^{{commit}}",
        "uv run --frozen --extra dev python -m pytest "
        "tools/ci/tests/test_check_changed_line_coverage.py -q",
        "cargow --lane coverage-lane llvm-cov nextest --workspace --all-features --locked "
        f"--lcov --output-path {lcov}",
        "uv run --frozen --extra dev python tools/ci/lint/check-changed-line-coverage.py "
        f"--lcov {lcov} --base {base} --minimum-percent 90",
    ]

    def run(base_sha, fail_command=""):
        return subprocess.run(
            ["bash", "-c", coverage["command"]],
            cwd=tmp_path,
            env={
                "PATH": f"{tools}{os.pathsep}{os.environ['PATH']}",
                "CI_PR_BASE_SHA": base_sha,
                "FAIL_COMMAND": fail_command,
            },
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )

    malformed = run("main")
    assert malformed.returncode == 2
    assert malformed.stdout == ""
    assert "PR coverage requires the exact pull request base SHA" in malformed.stderr
    accepted = run(base)
    assert accepted.returncode == 0, accepted.stderr
    assert accepted.stdout.splitlines() == expected
    failed = run(base, fail_command="cargow")
    assert failed.returncode == 19
    assert failed.stdout.splitlines() == expected[:-1]


def test_bench_capacity_matches_worker_without_changing_compile_scope():
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    bench = config["jobs"]["verify-rust-bench"]
    assert bench["resource_class"] == "large"
    assert bench["environment"]["CARGO_BUILD_JOBS"] == "4"
    command = next(
        step["run"]["command"]
        for step in bench["steps"]
        if isinstance(step, dict)
        and step.get("run", {}).get("name") == "Rust benchmark compilation"
    )
    assert command.splitlines()[-1] == "just rust-bench-build"
    justfile = (ROOT / "Justfile").read_text(encoding="utf-8")
    recipe = justfile.split("rust-bench-build:\n", 1)[1].split("\n\n", 1)[0]
    assert recipe.strip() == (
        "{{cargo}} --lane bench-lane bench --workspace --all-features --locked --no-run"
    )
    for name in ("verify-rust-static", "verify-rust-tests", "verify-rust-docs"):
        assert config["jobs"][name]["resource_class"] == "medium"
        assert config["jobs"][name]["environment"]["CARGO_BUILD_JOBS"] == "2"


def test_regular_workflow_keeps_standard_rust_and_precommit_gates():
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    rust_workers = (
        "verify-rust-static",
        "verify-rust-tests",
        "verify-rust-docs",
        "verify-rust-bench",
    )
    python_steps = config["jobs"]["verify-python"]["steps"]
    rust_runs = {
        step["run"]["name"]: step["run"]["command"]
        for name in rust_workers
        for step in config["jobs"][name]["steps"]
        if isinstance(step, dict) and "run" in step
    }
    python_runs = {
        step["run"]["name"]: step["run"]["command"] for step in python_steps if "run" in step
    }
    for name, commands in {
        "Rust dependency and MSRV checks": (
            "just rust-machete",
            "just rust-msrv",
        ),
        "Rust documentation": ("just rust-doc",),
        "Rust benchmark compilation": ("just rust-bench-build",),
    }.items():
        script = rust_runs[name]
        assert script.startswith("set -euo pipefail\n")
        assert "unset BASH_ENV ENV\n" in script
        assert all(f"\n{command}\n" in script for command in commands)
    precommit = python_runs["Pre-commit hooks"]
    policy = python_runs["Python policy and tooling tests"]
    assert "just rust-policy" in policy
    justfile = (ROOT / "Justfile").read_text(encoding="utf-8")
    rust_policy = justfile.split("\nrust-policy:\n", 1)[1].split("\n\n", 1)[0]
    assert "    @just rust-read-view-ambient" in rust_policy.splitlines()
    assert (
        "\nrust-read-view-ambient:\n    python3 tools/ci/lint/check-read-view-ambient-lookup.py\n"
        in justfile
    )
    assert precommit.startswith("set -euo pipefail\n")
    assert (
        "uv run --frozen --extra dev pre-commit run --all-files --show-diff-on-failure" in precommit
    )


def test_circleci_explicit_test_selector_must_be_registered(tmp_path: Path):
    data, path = _config(tmp_path)
    data["jobs"]["verify"]["steps"][0]["run"]["command"] = (
        "./scripts/cargow test -p demo --test missing\n"
    )
    path.write_text(yaml.safe_dump(data), encoding="utf-8")
    module = _checker()
    violations = []
    module._validate_workflow_test_selectors(
        root=tmp_path,
        catalog=tmp_path / "authority.toml",
        targets={"demo-covered": {"owner": "demo", "target": "covered"}},
        violations=violations,
    )
    assert any(
        "selects unknown Cargo test target demo:missing" in item.message for item in violations
    )
