"""Source-bound R5 paired runner archive; no operational qualification."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import re
import secrets
import site
import sys
from collections.abc import Callable
from pathlib import Path
from typing import Any

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from tools.ci import binary_custody as daemon_custody
from tools.ci.daemon_execution_evidence import (
    DAEMON_PATH_ENV,
    R5_PROOF,
    R5_WITNESS_FILE,
    WITNESS_DIR_ENV,
    WITNESS_NONCE_ENV,
    R5Case,
    live_proof,
    r5_cases,
    read_witness,
    verify_r5_witness,
)
from tools.ci.lint.handoff_validation import (
    _open_repo_output_parent,
    _read_repo_regular_bytes,
    _require_output_parent_identity,
    _restore_prior_output,
    _write_output_temporary,
)
from tools.ci.nextest_events import parse_nextest_bytes, parse_nextest_inventory_bytes
from tools.ci.p11_attestation import SOCKET_ENV, remove_signing_agent
from tools.ci.paired_cargo_resolution import (
    REQUIRED_QUANTA_PACKAGES,
    frozen_source_head,
    resolve_from_qbc,
    source_python_environment,
)
from tools.ci.proof_json import parse_proof_json

SCHEMA = "quanta-paired-r5-runner-candidate/v1"
INDEX = "paired-r5-result.json"
DIGEST = re.compile(r"[0-9a-f]{64}\Z")
CASES = r5_cases({}, allow_legacy=True)


def _sha(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def _canonical(payload: object) -> bytes:
    return json.dumps(payload, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()


def _frozen(root: Path, expected: str) -> None:
    try:
        observed = frozen_source_head(root)
    except ValueError as error:
        raise ValueError(f"source changed or is dirty: {root}") from error
    if observed != expected:
        raise ValueError(f"source changed or is dirty: {root}")


def validate_selected(inventory_raw: bytes, events_raw: bytes, case: R5Case) -> dict[str, Any]:
    """Require one exact collected and executed test, with independent bytes."""
    _, package, _, binary, kind, test = case
    expected_name = f"{package}::{binary}${test}"
    inventory = parse_nextest_inventory_bytes(inventory_raw)
    if dict(inventory) != {expected_name: (package, binary, kind)}:
        raise ValueError("nextest inventory differs from exact selected test")
    events = parse_nextest_bytes(events_raw, inventory)
    if (events.selected, events.executed, events.passed, events.failed) != (1, 1, 1, 0):
        raise ValueError("nextest selected test did not terminate passed")
    if events.passed_names != frozenset({expected_name}):
        raise ValueError("nextest passed name differs from exact selection")
    return {
        "selected": 1,
        "executed": 1,
        "passed": 1,
        "failed": 0,
        "inventory_sha256": _sha(inventory_raw),
        "events_sha256": events.sha256,
        "passed_names": sorted(events.passed_names),
    }


def validate_release_witnesses(
    passed_cases: set[tuple[str, str]], *, cases: tuple[R5Case, ...] = CASES
) -> None:
    expected = {
        ("nextest-jsonl", f"{package}::{binary}${test}") for _, package, _, binary, _, test in cases
    }
    if passed_cases != expected:
        raise ValueError("release proof requires the exact P11 caller and kernel witnesses")


def verify_captured_daemon_witness(
    raw: bytes, execution: dict[str, Any], *, test: str, daemon_sha256: str
) -> None:
    if execution.get("daemon_witness_sha256") != _sha(raw):
        raise ValueError("paired R5 daemon witness differs from its archived capture")
    verify_r5_witness(raw, test=test, nonce=execution["nonce"], daemon_sha256=daemon_sha256)


def validate_locator_fields(
    locator: dict,
    *,
    nonce: str,
    request_sha: str,
    command: list[str],
    lane: str,
    run_id: str,
    path: str,
    size: int,
    receipt_sha: str,
    cwd: str,
    source_digest: str,
    artifact_env: str,
) -> None:
    if (
        not isinstance(nonce, str)
        or not re.fullmatch(r"[0-9a-f]{32}", nonce)
        or any(
            not isinstance(value, str) or not DIGEST.fullmatch(value)
            for value in (request_sha, source_digest, artifact_env, receipt_sha)
        )
    ):
        raise ValueError("QBC completion lacks fixed request, environment or source identity")
    expected = {
        "schema_version": "qbc-verification-completion-locator/v1",
        "nonce": nonce,
        "lane": lane,
        "run_id": run_id,
        "receipt_path": path,
        "receipt_size_bytes": size,
        "receipt_sha256": receipt_sha,
        "source_snapshot_schema": "source-snapshot-identity.v1",
        "source_snapshot_digest": source_digest,
        "cargo_argv": command,
        "command_cwd": cwd,
        "request_env_sha256": request_sha,
        "artifact_env_digest": artifact_env,
        "receipt_exit_code": 0,
    }
    if _canonical(locator) != _canonical(expected):
        raise ValueError("QBC completion locator differs from immutable selected run")


def validate_locator(
    locator: dict[str, Any],
    capture: Any,
    *,
    nonce: str,
    request_sha: str,
    command: list[str],
    source_head: str,
    source_digest: str,
    lane: str = "local",
) -> None:
    """Bind QBC's physical immutable receipt to the requested nextest invocation."""
    receipt = capture.receipt_payload_v1()
    if not re.fullmatch(r"[0-9a-f]{40}", source_head):
        raise ValueError("expected QBC source HEAD is invalid")
    validate_locator_fields(
        locator,
        nonce=nonce,
        request_sha=request_sha,
        command=command,
        lane=lane,
        run_id=capture.run.run_id,
        path=str(capture.run.result_directory / "receipt.json"),
        size=len(capture.receipt_bytes),
        receipt_sha=capture.receipt_digest,
        cwd=str(capture.run.command_cwd),
        source_digest=receipt.get("source_snapshot_digest"),
        artifact_env=receipt.get("artifact_env_digest"),
    )
    if capture.run.command != tuple(command) or capture.run.lane != lane:
        raise ValueError("QBC completion locator differs from immutable selected run")
    if (
        type(capture.run.exit_code) is not int
        or capture.run.exit_code != 0
        or type(capture.process_exit_code_v1) is not int
        or capture.process_exit_code_v1 != 0
    ):
        raise ValueError("QBC runner lacks observed successful process exit")
    if not DIGEST.fullmatch(source_digest) or locator["source_snapshot_digest"] != source_digest:
        raise ValueError("QBC completion source differs from current frozen source")


def read_locator(path: Path, completion: Any) -> tuple[bytes, dict[str, Any]]:
    raw = completion.read_completion_locator_bytes_v1(path)

    def unique(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("duplicate QBC completion locator key")
            result[key] = value
        return result

    locator = json.loads(raw, object_pairs_hook=unique)
    if not isinstance(locator, dict):
        raise ValueError("QBC completion locator is not an object")
    return raw, locator


def _archive_capture(directory: Path, phase: str, capture: Any) -> dict[str, str]:
    result: dict[str, str] = {}
    for name, raw in (
        ("receipt.json", capture.receipt_bytes),
        ("stdout.log", capture.stdout),
        ("stderr.log", capture.stderr),
    ):
        destination = directory / f"{phase}-{name}"
        with destination.open("xb") as output:
            output.write(raw)
            output.flush()
            os.fsync(output.fileno())
        result[name] = str(destination)
        result[f"{name}_sha256"] = _sha(raw)
    return result


def read_runner_capture(directory: Path, name: str) -> bytes:
    if name == R5_WITNESS_FILE:
        return read_witness(directory, name)
    return _read_repo_regular_bytes(directory, name, label="original QBC capture")


def publish_paired_output(root: Path, relative: Path, parent: int, raw: bytes) -> None:
    _require_output_parent_identity(root, relative.as_posix(), parent)
    name = relative.name
    temporary = _write_output_temporary(parent, name, raw)
    try:
        os.link(temporary, name, src_dir_fd=parent, dst_dir_fd=parent, follow_symlinks=False)
    except BaseException:
        os.unlink(temporary, dir_fd=parent)
        raise
    try:
        os.unlink(temporary, dir_fd=parent)
        os.fsync(parent)
        _require_output_parent_identity(root, relative.as_posix(), parent)
    except BaseException:
        _restore_prior_output(parent, name, None)
        raise


def _qbc_modules(root: Path, expected_head: str) -> tuple[Any, Any, Any]:
    _frozen(root, expected_head)
    directory = root / "tools/quanta-build-cli"
    module_paths = {
        name: directory / (name + ".py")
        for name in (
            "qbc_completed_run_v1",
            "quanta_build_cli",
            "verification_completion_locator_v1",
        )
    }
    sys.path.insert(0, str(directory))
    # Check every possible entry before any of them can import a sibling.
    for name, path in module_paths.items():
        spec = importlib.util.find_spec(name)
        if spec is None or spec.origin != str(path) or not path.is_file() or path.resolve() != path:
            raise ValueError("paired QBC module belongs to another source")
    modules = [importlib.import_module(name) for name in module_paths]
    for module, path in zip(modules, module_paths.values(), strict=True):
        if Path(module.__file__).resolve() != path:
            raise ValueError("paired QBC module belongs to another source")
    completed, qbc, completion = modules
    return completed, completion, qbc


def _execute_qbc_phase(
    semantica: Path,
    case: tuple[str, str, str, str, str, str],
    phase: str,
    cargo: list[str],
    owner: Any,
    completion: Any,
    environment: dict[str, str],
    lane: str,
    *,
    cases: tuple[R5Case, ...] = CASES,
) -> int:
    """Admit only this fixed recipe through QBC's canonical owner port.

    Nextest list compiles but runs no assertions, so raw feature composition
    is correctly refused by the manual front door. The paired recipe owns
    exactly these two list/run shapes; its admission binds the command model.
    Lane locking, execution, source guards and immutable receipts remain QBC's.
    """
    if case not in cases or phase not in ("list", "run"):
        raise ValueError("unregistered paired R5 recipe phase")
    label = case[0]
    if cargo != phase_command(case, phase):
        raise ValueError("paired R5 owner recipe command drift")
    admission_owner = owner._CARGO_INVOCATION_ADMISSION_OWNER_V1
    if Path(admission_owner.__file__).resolve() != (
        semantica / "tools/quanta-build-cli/cargo_invocation_admission.py"
    ):
        raise ValueError("paired QBC admission module belongs to another source")
    reference = f"quanta-index:paired-r5:{label}:{phase}"
    admission = admission_owner.validate_cargo_invocation_v1(
        owner._build_command_model(cargo, runner="nextest"),
        authority=admission_owner.InvocationAuthorityV1(
            admission_owner.InvocationAuthorityKindV1.OWNER_RECIPE,
            reference,
        ),
    )
    source_digest = owner._campaign_cargo_admission_owner_v1().campaign_source_snapshot_digest_v1(
        semantica
    )
    compile_environment = {
        owner.EXPECTED_SOURCE_SNAPSHOT_ENV_V1: source_digest,
        "CODEGRAPH_PERSONA": environment["CODEGRAPH_PERSONA"],
        DAEMON_PATH_ENV: environment[DAEMON_PATH_ENV],
    }
    if phase == "run":
        compile_environment["NEXTEST_EXPERIMENTAL_LIBTEST_JSON"] = "1"
    if label == "caller" and phase == "run":
        compile_environment[WITNESS_DIR_ENV] = environment[WITNESS_DIR_ENV]
        compile_environment[WITNESS_NONCE_ENV] = environment[WITNESS_NONCE_ENV]
    completion_environment = {
        name: environment[name]
        for name in (
            completion.LOCATOR_PATH_ENV_V1,
            completion.LOCATOR_NONCE_ENV_V1,
            completion.REQUEST_ENV_DIGEST_ENV_V1,
            completion.EXPECTED_SOURCE_HEAD_ENV_V1,
        )
    }
    completion_environment[completion.EXPECTED_SOURCE_ENV_V1] = source_digest
    return owner._run_lane_command(
        lane=lane,
        runner="nextest",
        command=cargo,
        jobs=None,
        wait_seconds=0.0,
        lane_token=None,
        seed_policy="off",
        explicit_source_lane=None,
        allow_locked_source=False,
        warmroot_preview_enabled=False,
        owner_recipe_ref=reference,
        invocation_admission_v1=admission,
        execution_root_v1=semantica,
        command_cwd_v1=semantica,
        compile_env_overrides_v1=compile_environment,
        verification_completion_environment_v1=completion_environment,
    )


def phase_command(case: tuple[str, str, str, str, str, str], phase: str) -> list[str]:
    _, package, feature, binary, kind, test = case
    common = [
        "--locked",
        "--manifest-path",
        "packages/analysis/quanta-v2/Cargo.toml",
        "-p",
        package,
        "--no-default-features",
        "--features",
        feature,
        "--lib" if kind == "lib" else "--test",
    ]
    if kind == "test":
        common.append(binary)
    exact_filter = "test(/^" + re.escape(test) + "$/)"
    suffix = {
        "list": ["--message-format", "json"],
        "run": ["--message-format", "libtest-json-plus", "--message-format-version", "0.1"],
    }[phase]
    return ["cargo", "nextest", phase, *common, "-E", exact_filter, *suffix]


def _verify_binary_custody(
    digest: str, paths: dict[str, Path], archive: Path | None = None
) -> None:
    if archive is None:
        daemon_custody.verify(digest, list(paths.values()))
        return
    if paths["built"] != archive or paths["custody"] == archive:
        raise ValueError("formal P11 requires the P10 archive and a separate executable copy")
    daemon_custody.verify(digest, list(paths.values()), archive=archive)


def _one(
    semantica: Path,
    root: Path,
    evidence: Path,
    source_head: str,
    quanta_head: str,
    case: tuple[str, str, str, str, str, str],
    completed: Any,
    completion: Any,
    qbc_owner: Any,
    daemon_digest: str,
    binary_paths: dict[str, Path],
    qbc_lane: str,
    *,
    archive: Path | None = None,
    cases: tuple[R5Case, ...] = CASES,
) -> dict[str, Any]:
    label, package, feature, binary, kind, test = case
    expected_name = f"{package}::{binary}${test}"
    captures: dict[str, Any] = {}
    archives: dict[str, Any] = {}
    for phase in ("list", "run"):
        cargo = phase_command(case, phase)
        nonce = secrets.token_hex(16)
        locator_path = evidence / f"{label}-{phase}-completion.json"
        environment = os.environ.copy()
        for name in (
            completion.LOCATOR_PATH_ENV_V1,
            completion.LOCATOR_NONCE_ENV_V1,
            completion.REQUEST_ENV_DIGEST_ENV_V1,
            completion.EXPECTED_SOURCE_HEAD_ENV_V1,
            "NEXTEST_EXPERIMENTAL_LIBTEST_JSON",
        ):
            environment.pop(name, None)
        environment.update(
            {
                completion.LOCATOR_PATH_ENV_V1: str(locator_path),
                completion.LOCATOR_NONCE_ENV_V1: nonce,
                completion.EXPECTED_SOURCE_HEAD_ENV_V1: source_head,
                "CODEGRAPH_PERSONA": "agent",
                DAEMON_PATH_ENV: str(binary_paths["custody"]),
            }
        )
        if phase == "run":
            environment["NEXTEST_EXPERIMENTAL_LIBTEST_JSON"] = "1"
        witness_required = label == "caller" and phase == "run"
        if witness_required:
            environment[WITNESS_DIR_ENV] = str(evidence)
            environment[WITNESS_NONCE_ENV] = nonce
        # Hash the caller request environment before adding its own digest.
        # QBC separately binds its prepared child artifact_env_digest.
        request_sha = _sha(
            _canonical(
                {
                    "schema": SCHEMA,
                    "case": label,
                    "phase": phase,
                    "cargo_argv": cargo,
                    "environment": environment,
                }
            )
        )
        environment[completion.REQUEST_ENV_DIGEST_ENV_V1] = request_sha
        _frozen(root, quanta_head)
        _frozen(semantica, source_head)
        _verify_binary_custody(daemon_digest, binary_paths, archive)
        exit_code = _execute_qbc_phase(
            semantica, case, phase, cargo, qbc_owner, completion, environment, qbc_lane, cases=cases
        )
        if exit_code != 0:
            raise ValueError(f"QBC nextest {label}/{phase} failed: {exit_code}")
        locator_raw, locator = read_locator(locator_path, completion)
        receipt_path = locator.get("receipt_path")
        if not isinstance(receipt_path, str) or not Path(receipt_path).is_absolute():
            raise ValueError("QBC completion locator has no absolute receipt path")
        capture = completed.read_completed_run_receipt_at_path_v1(Path(receipt_path))
        if (
            capture.run.lane != qbc_lane
            or capture.run.execution_root != semantica
            or capture.run.command_cwd != semantica
        ):
            raise ValueError("QBC nextest run belongs to another lane or source cwd")
        observed_source = (
            qbc_owner._campaign_cargo_admission_owner_v1().campaign_source_snapshot_digest_v1(
                semantica
            )
        )
        validate_locator(
            locator,
            capture,
            nonce=nonce,
            request_sha=request_sha,
            command=cargo,
            source_head=source_head,
            source_digest=observed_source,
            lane=qbc_lane,
        )
        captures[phase] = capture
        archives[phase] = {
            "nonce": nonce,
            "request_env_sha256": request_sha,
            "locator_path": str(locator_path),
            "locator_sha256": _sha(locator_raw),
            "qbc_custody": capture.custody_payload_v1(),
            "archived": _archive_capture(evidence, f"{label}-{phase}", capture),
        }
        if witness_required:
            raw = read_witness(evidence, R5_WITNESS_FILE)
            verify_r5_witness(raw, test=expected_name, nonce=nonce, daemon_sha256=daemon_digest)
            archives[phase]["daemon_witness_sha256"] = _sha(raw)
        _frozen(root, quanta_head)
        _verify_binary_custody(daemon_digest, binary_paths, archive)
    verdict = validate_selected(captures["list"].stdout, captures["run"].stdout, case)
    return {
        "package": package,
        "feature": feature,
        "binary": binary,
        "kind": kind,
        "selected_test": test,
        "event_name": expected_name,
        "nextest": verdict,
        "executions": archives,
    }


def validate_resolution_records(
    resolutions: dict[str, Any], *, cases: tuple[R5Case, ...] = CASES
) -> None:
    if not isinstance(resolutions, dict) or set(resolutions) != {"runtime", "kernel"}:
        raise ValueError("paired resolver requires both selected consumers")
    for key, (_, consumer, features, _, _, _) in zip(("runtime", "kernel"), cases, strict=True):
        item = resolutions[key]
        if (
            not isinstance(item, dict)
            or type(item.get("version")) is not int
            or item["version"] != 1
            or item.get("consumer") != consumer
        ):
            raise ValueError("paired resolver consumer identity changed")
        expected_features = set(features.split(","))
        observed_features = item.get("consumer_features")
        if not isinstance(observed_features, list) or not expected_features.issubset(
            set(observed_features)
        ):
            raise ValueError("paired resolver lacks the selected caller/kernel feature authority")
        if item.get("workspace") != "packages/analysis/quanta-v2":
            raise ValueError("paired resolver workspace identity changed")
        lock = item.get("dependency_lock")
        if not isinstance(lock, dict) or lock.get("path") != item["workspace"] + "/Cargo.lock":
            raise ValueError("paired resolver lock must be relative to the paired checkout")

        def digest(value: Any) -> bool:
            return (
                isinstance(value, str) and re.fullmatch(r"sha256:[0-9a-f]{64}", value) is not None
            )

        if not digest(lock.get("sha256")) or not digest(
            item.get("quanta_workspace_manifest_sha256")
        ):
            raise ValueError("paired resolver lacks lock or workspace manifest digest")
        names = set()
        for package in item["packages"]:
            path = Path(package["manifest"])
            if (
                path.is_absolute()
                or ".." in path.parts
                or path.name != "Cargo.toml"
                or not digest(package.get("manifest_sha256"))
                or not package.get("version")
                or not isinstance(package.get("features"), list)
                or not package.get("name")
                or package["name"] in names
            ):
                raise ValueError("paired resolver package identity is malformed")
            names.add(package["name"])
        if not REQUIRED_QUANTA_PACKAGES.issubset(names):
            raise ValueError("paired resolver lacks required Quanta dependency packages")


def _verify_resolution_files(
    root: Path, semantica: Path, resolutions: dict[str, Any], *, cases: tuple[R5Case, ...] = CASES
) -> None:
    validate_resolution_records(resolutions, cases=cases)
    for item in resolutions.values():
        lock = item["dependency_lock"]
        for parent, field, base in (
            (lock, "sha256", semantica),
            (
                {"path": "Cargo.toml", "sha256": item.get("quanta_workspace_manifest_sha256")},
                "sha256",
                root,
            ),
        ):
            if not isinstance(parent, dict) or not isinstance(parent.get("path"), str):
                raise ValueError("paired resolver file identity is missing")
            path = (base / parent["path"]).resolve(strict=True)
            if not path.is_relative_to(base) or parent.get(field) != "sha256:" + _sha(
                path.read_bytes()
            ):
                raise ValueError("paired resolver file identity changed")
        for package in item.get("packages", []):
            if not isinstance(package, dict) or not isinstance(package.get("manifest"), str):
                raise ValueError("paired resolver package identity is malformed")
            path = (root / package["manifest"]).resolve(strict=True)
            if not path.is_relative_to(root) or package.get("manifest_sha256") != "sha256:" + _sha(
                path.read_bytes()
            ):
                raise ValueError("paired resolver package source identity changed")


def _require_exact_resolution(actual: dict[str, Any], expected: dict[str, Any], label: str) -> None:
    # Python equality aliases bool/int and integer/float inside nested JSON.
    # Compare the canonical bytes already used by this result writer.
    if _canonical(actual) != _canonical(expected):
        raise ValueError(f"{label} resolver identity changed during runner proof")


def _resolved_after(
    root: Path, semantica: Path, consumer: str, feature: str, lane: str
) -> dict[str, Any]:
    return resolve_from_qbc(
        quanta_root=root, paired_root=semantica, consumer=consumer, feature=feature, lane=lane
    )


def verify_runner_capture(
    record: dict, read: Callable[[str], bytes], *, cases: tuple[R5Case, ...]
) -> dict[str, Any]:
    """Replay the paired candidate's raw observations and QBC outputs."""
    run_ids = set()
    nonces = set()
    source_lanes = set()
    for case, row in zip(cases, record["cases"], strict=True):
        label, package, feature, binary, kind, test = case
        event = f"{package}::{binary}${test}"
        if [
            row[k] for k in ("package", "feature", "binary", "kind", "selected_test", "event_name")
        ] != [package, feature, binary, kind, test, event]:
            raise ValueError("paired QBC case identity differs from the release witness")
        outputs = {}
        for phase in ("list", "run"):
            capture = row["executions"][phase]
            source_digest, toolchain, stdout = _read_phase(case, phase, capture, read)
            custody = capture["qbc_custody"]
            source_lanes.add(
                (
                    custody["lane_id"],
                    custody["execution_root"],
                    custody["target_root"],
                    source_digest,
                    toolchain,
                )
            )
            if custody["run_id"] in run_ids:
                raise ValueError("QBC completion is reused across list/run or caller/kernel")
            if capture["nonce"] in nonces:
                raise ValueError("paired phases reuse a QBC request nonce")
            nonces.add(capture["nonce"])
            run_ids.add(custody["run_id"])
            outputs[phase] = stdout
        if row["nextest"] != validate_selected(outputs["list"], outputs["run"], case):
            raise ValueError("paired QBC result differs from its collected and executed body")
        if label == "caller":
            verify_captured_daemon_witness(
                read(R5_WITNESS_FILE),
                row["executions"]["run"],
                test=event,
                daemon_sha256=record["daemon_sha256"],
            )
    if len(source_lanes) != 1:
        raise ValueError(
            "paired QBC executions must share one lane and source snapshot and toolchain"
        )
    return {
        "toolchain": "sha256:" + next(iter(source_lanes))[-1],
        "features": sorted({feature for case in cases for feature in case[2].split(",")}),
    }


def _read_phase(
    case: tuple[str, str, str, str, str, str],
    phase: str,
    capture: dict,
    read: Callable[[str], bytes],
) -> tuple[str, str, bytes]:
    prefix = f"{case[0]}-{phase}-"
    locator_raw = read(prefix + "completion.json")
    locator = parse_proof_json(locator_raw)
    custody = capture["qbc_custody"]
    streams = {}
    for name, field in (
        ("receipt.json", "receipt"),
        ("stdout.log", "stdout"),
        ("stderr.log", "stderr"),
    ):
        raw = read(prefix + name)
        digest = _sha(raw)
        if digest != capture["archived"][name + "_sha256"] or digest != custody[field + "_sha256"]:
            raise ValueError("original QBC completion bytes differ from the paired capture")
        streams[name] = raw
    receipt = parse_proof_json(streams["receipt.json"])
    if not isinstance(receipt, dict) or not isinstance(locator, dict):
        raise ValueError("QBC completion requires its original receipt and locator")
    command = phase_command(case, phase)
    if not all(
        isinstance(custody.get(k), str) and Path(custody[k]).is_absolute()
        for k in ("execution_root", "target_root", "receipt_path")
    ):
        raise ValueError("QBC completion lacks its registered lane paths")
    identity = (custody["lane_id"], custody["run_id"])
    if (
        not all(isinstance(value, str) and value for value in identity)
        or (receipt.get("lane"), receipt.get("last_run_id_v1")) != identity
        or receipt.get("run_state") != "finished"
        or receipt.get("command") != command
        or any(
            type(receipt.get(k)) is not int or receipt[k] != 0
            for k in ("last_exit_code", "command_exit_code_v1")
        )
    ):
        raise ValueError("QBC receipt does not record the successful selected process")
    source_digest = receipt.get("source_snapshot_digest")
    if not isinstance(source_digest, str) or not DIGEST.fullmatch(source_digest):
        raise ValueError("QBC receipt is missing its source snapshot")
    environment = receipt.get("artifact_env_payload")
    if not isinstance(environment, dict) or _sha(_canonical(environment)) != receipt.get(
        "artifact_env_digest"
    ):
        raise ValueError("QBC receipt environment differs from its digest")
    if any(
        Path(custody.get(field, "")) != Path(custody["receipt_path"]).with_name(filename)
        for field, filename in (("stdout_path", "stdout.log"), ("stderr_path", "stderr.log"))
    ):
        raise ValueError("QBC completion streams differ from the physical receipt")
    validate_locator_fields(
        locator,
        nonce=capture["nonce"],
        request_sha=capture["request_env_sha256"],
        command=command,
        lane=identity[0],
        run_id=identity[1],
        path=custody["receipt_path"],
        size=len(streams["receipt.json"]),
        receipt_sha=custody["receipt_sha256"],
        cwd=custody["execution_root"],
        source_digest=source_digest,
        artifact_env=receipt.get("artifact_env_digest"),
    )
    if (
        _sha(locator_raw) != capture["locator_sha256"]
        or receipt.get("lane_execution_root_v1") != custody["execution_root"]
        or receipt.get("command_cwd_v1") != custody["execution_root"]
        or receipt.get("target_dir") != custody["target_root"]
    ):
        raise ValueError("QBC locator differs from the receipt, source or physical lane")
    toolchain = receipt.get("toolchain_digest")
    if not isinstance(toolchain, str) or not DIGEST.fullmatch(toolchain):
        raise ValueError("QBC receipt lacks its observed toolchain digest")
    return source_digest, toolchain, streams["stdout.log"]


def verify_pair_boundary(
    root: Path,
    pair: Path,
    record: dict,
    lane: str,
    *,
    archive: Path | None = None,
    cases: tuple[R5Case, ...] = CASES,
) -> None:
    """The runner and issuer both close on the same live resolver, source and binary."""
    heads = record["source_heads"]
    binaries = {key: Path(record["binary_paths"][key]) for key in ("built", "provided", "custody")}

    def check_sources_and_daemon() -> None:
        _frozen(root, heads["quanta"])
        _frozen(pair, heads["semantica"])
        _verify_binary_custody(record["daemon_sha256"], binaries, archive)

    check_sources_and_daemon()
    resolutions = record["resolutions"]
    _verify_resolution_files(root, pair, resolutions, cases=cases)
    for key, case in zip(("runtime", "kernel"), cases, strict=True):
        _require_exact_resolution(
            _resolved_after(root, pair, case[1], case[2], lane), resolutions[key], case[0]
        )
    check_sources_and_daemon()


def main(*, standalone: bool = False) -> int:
    # QBC and Cargo spawn descendants using this process' environment.
    socket = os.environ.pop(SOCKET_ENV, None)
    removed_agent = remove_signing_agent(os.environ, socket)
    deferred_site = sys.flags.isolated and sys.flags.no_site
    if standalone and socket and not deferred_site:
        raise ValueError("P11 signing requires the isolated paired R5 launcher")
    git_no_replace = os.environ.get("GIT_NO_REPLACE_OBJECTS")
    os.environ["GIT_NO_REPLACE_OBJECTS"] = "1"
    removed_witness = {
        key: os.environ.pop(key)
        for key in (WITNESS_DIR_ENV, WITNESS_NONCE_ENV)
        if key in os.environ
    }
    python_cache = (sys.pycache_prefix, sys.dont_write_bytecode)
    try:
        with source_python_environment(os.environ) as cache_prefix:
            sys.pycache_prefix, sys.dont_write_bytecode = cache_prefix, True
            # The isolated launcher defers installed site packages and hooks
            # until the signing agent has been removed from the environment.
            if standalone and deferred_site:
                site.main()
            return _main(socket)
    finally:
        sys.pycache_prefix, sys.dont_write_bytecode = python_cache
        # Only in-process callers regain their environment. A CLI may still
        # run site exit hooks after returning from main.
        if not standalone:
            if socket is not None:
                os.environ[SOCKET_ENV] = socket
            if removed_agent is not None:
                os.environ["SSH_AUTH_SOCK"] = removed_agent
        os.environ.update(removed_witness)
        if git_no_replace is None:
            os.environ.pop("GIT_NO_REPLACE_OBJECTS", None)
        else:
            os.environ["GIT_NO_REPLACE_OBJECTS"] = git_no_replace


def _main(signer_socket: str | None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--quanta-root", required=True, type=Path)
    parser.add_argument("--semantica-root", required=True, type=Path)
    parser.add_argument(
        "--verify-resolution-only",
        action="store_true",
        help="verify paired resolution and print the P10 archive for executable P11",
    )
    parser.add_argument("--evidence-root", type=Path)
    parser.add_argument("--quanta-head", required=True)
    parser.add_argument("--semantica-head", required=True)
    parser.add_argument("--daemon-digest")
    parser.add_argument("--built-binary", type=Path)
    parser.add_argument("--provided-binary", type=Path)
    parser.add_argument("--custody-binary", type=Path)
    parser.add_argument("--runtime-resolution", required=True)
    parser.add_argument("--kernel-resolution", required=True)
    parser.add_argument("--qbc-lane")
    args = parser.parse_args()
    root = args.quanta_root.resolve(strict=True)
    semantica = args.semantica_root.resolve(strict=True)
    from tools.ci.paired_r5_release import admit_release, prepare_terminal

    _frozen(root, args.quanta_head)
    _frozen(semantica, args.semantica_head)
    registered_cases = r5_cases(live_proof(root, R5_PROOF))
    resolutions = {
        "runtime": parse_proof_json(args.runtime_resolution.encode()),
        "kernel": parse_proof_json(args.kernel_resolution.encode()),
    }
    _verify_resolution_files(root, semantica, resolutions, cases=registered_cases)
    if args.verify_resolution_only:
        _qbc_modules(semantica, args.semantica_head)
        release = admit_release(root, semantica, None, preflight_signer=False)
        _frozen(root, args.quanta_head)
        _frozen(semantica, args.semantica_head)
        if release is not None:
            print(root / release["binary_path"])
        return 0
    for field in (
        "evidence_root",
        "daemon_digest",
        "built_binary",
        "provided_binary",
        "custody_binary",
        "qbc_lane",
    ):
        if getattr(args, field) is None:
            parser.error("--" + field.replace("_", "-") + " is required for runner execution")
    output = args.evidence_root
    if not output.is_absolute() or output.exists() or output.is_symlink():
        raise ValueError("R5 evidence root must be a fresh absolute external directory")
    output_parent = output.parent.resolve(strict=True)
    if output_parent.is_relative_to(root) or output_parent.is_relative_to(semantica):
        raise ValueError("R5 evidence root must be outside both source checkouts")
    output = output_parent / output.name
    if not all(DIGEST.fullmatch(value) for value in (args.daemon_digest,)):
        raise ValueError("daemon digest is invalid")
    binary_paths = {
        role: getattr(args, f"{role}_binary").absolute()
        for role in ("built", "provided", "custody")
    }
    _frozen(root, args.quanta_head)
    _frozen(semantica, args.semantica_head)
    release = admit_release(root, semantica, args.daemon_digest, signer_socket=signer_socket)
    if release is not None and registered_cases != r5_cases(release["proof"]):
        raise ValueError("paired release selection changed during admission")
    archive = root / release["binary_path"] if release is not None else None
    _verify_binary_custody(args.daemon_digest, binary_paths, archive)
    # Admit the complete canonical resolver pair before any selected runner.
    # The post-run repetition below rejects source or lock drift during proof.
    _verify_resolution_files(root, semantica, resolutions, cases=registered_cases)
    for key, (role, consumer, features, _, _, _) in zip(
        ("runtime", "kernel"), registered_cases, strict=True
    ):
        _require_exact_resolution(
            _resolved_after(root, semantica, consumer, features, args.qbc_lane),
            resolutions[key],
            role,
        )
    _frozen(root, args.quanta_head)
    _frozen(semantica, args.semantica_head)
    output.mkdir(mode=0o700, parents=False)
    completed, completion, qbc_owner = _qbc_modules(semantica, args.semantica_head)
    case_results = [
        _one(
            semantica,
            root,
            output,
            args.semantica_head,
            args.quanta_head,
            case,
            completed,
            completion,
            qbc_owner,
            args.daemon_digest,
            binary_paths,
            args.qbc_lane,
            archive=archive,
            cases=registered_cases,
        )
        for case in registered_cases
    ]
    payload = {
        "schema_version": SCHEMA,
        "qualification": "runner-candidate-only",
        "source_heads": {"quanta": args.quanta_head, "semantica": args.semantica_head},
        "resolutions": resolutions,
        "daemon_sha256": args.daemon_digest,
        "binary_paths": {role: str(path) for role, path in binary_paths.items()},
        "cases": case_results,
    }
    if release is None:
        verify_pair_boundary(root, semantica, payload, args.qbc_lane, cases=registered_cases)
    parent = _open_repo_output_parent(output, INDEX, create=False)
    try:
        verify_runner_capture(
            payload,
            lambda name: read_runner_capture(output, name),
            cases=registered_cases,
        )
        publish_paired_output(output, Path(INDEX), parent, _canonical(payload) + b"\n")
    finally:
        os.close(parent)
    if release is not None:
        terminal = prepare_terminal(
            root,
            semantica,
            output,
            payload,
            release,
            lane=args.qbc_lane,
            signer_socket=signer_socket,
        )
        print(f"paired-r5-release-terminal: {terminal}")
    print(f"paired-r5-runner-candidate: {output / INDEX}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(standalone=True))
