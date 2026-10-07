"""Source-bound R5 paired runner archive; no operational qualification."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import secrets
import subprocess
import sys
from pathlib import Path
from typing import Any

from tools.ci.nextest_events import parse_nextest_bytes, parse_nextest_inventory_bytes
from tools.ci.paired_cargo_resolution import resolve_from_qbc

SCHEMA = "quanta-paired-r5-runner-candidate/v1"
# Cargo.toml [[test]] index_sdk_ingress_publish_contract_test required-features.
CALLER_FEATURES = "index-sdk-ingress,retrieval-authority-contract-surface"
DIGEST = re.compile(r"[0-9a-f]{64}\Z")
CASES = (
    (
        "caller",
        "quanta-runtime",
        CALLER_FEATURES,
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


def _sha(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def _canonical(payload: object) -> bytes:
    return json.dumps(payload, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()


def _head(root: Path) -> str:
    return subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()


def _frozen(root: Path, expected: str) -> None:
    if (
        _head(root) != expected
        or subprocess.check_output(
            ["git", "-C", str(root), "status", "--porcelain=v1", "--untracked-files=all"], text=True
        ).strip()
    ):
        raise ValueError(f"source changed or is dirty: {root}")


def validate_selected(
    inventory_raw: bytes, events_raw: bytes, expected_name: str
) -> dict[str, Any]:
    """Require one exact collected and executed test, with independent bytes."""
    inventory = parse_nextest_inventory_bytes(inventory_raw)
    if set(inventory) != {expected_name}:
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
    expected = {
        "schema_version": "qbc-verification-completion-locator/v1",
        "nonce": nonce,
        "lane": lane,
        "run_id": capture.run.run_id,
        "receipt_path": str(capture.run.result_directory / "receipt.json"),
        "receipt_size_bytes": len(capture.receipt_bytes),
        "receipt_sha256": capture.receipt_digest,
        "source_snapshot_schema": "source-snapshot-identity.v1",
        "source_snapshot_digest": receipt.get("source_snapshot_digest"),
        "cargo_argv": command,
        "command_cwd": str(capture.run.command_cwd),
        "request_env_sha256": request_sha,
        "artifact_env_digest": receipt.get("artifact_env_digest"),
        "receipt_exit_code": 0,
    }
    if not re.fullmatch(r"[0-9a-f]{40}", source_head):
        raise ValueError("expected QBC source HEAD is invalid")
    # The producer's canonical JSON encoding keeps bool, integer and float
    # distinct; Python dict equality would admit False == 0 and 7.0 == 7.
    if (
        _canonical(locator) != _canonical(expected)
        or capture.run.command != tuple(command)
        or capture.run.lane != lane
    ):
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


def _qbc_modules(root: Path) -> tuple[Any, Any, Any]:
    sys.path.insert(0, str(root / "tools/quanta-build-cli"))
    import qbc_completed_run_v1 as completed
    import quanta_build_cli as qbc
    import verification_completion_locator_v1 as completion

    for module, name in (
        (completed, "qbc_completed_run_v1"),
        (completion, "verification_completion_locator_v1"),
        (qbc, "quanta_build_cli"),
    ):
        if Path(module.__file__).resolve() != root / "tools/quanta-build-cli" / (name + ".py"):
            raise ValueError("paired QBC module belongs to another source")
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
) -> int:
    """Admit only this fixed recipe through QBC's canonical owner port.

    Nextest list compiles but runs no assertions, so raw feature composition
    is correctly refused by the manual front door. The paired recipe owns
    exactly these two list/run shapes; its admission binds the command model.
    Lane locking, execution, source guards and immutable receipts remain QBC's.
    """
    if case not in CASES or phase not in ("list", "run"):
        raise ValueError("unregistered paired R5 recipe phase")
    label, package, feature, binary, kind, test = case
    expected = [
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
        feature,
        "--lib" if kind == "lib" else "--test",
    ]
    if kind == "test":
        expected.append(binary)
    expected.extend(["-E", "test(/^" + re.escape(test) + "$/)"])
    expected.extend(
        ["--message-format", "json"]
        if phase == "list"
        else ["--message-format", "libtest-json-plus", "--message-format-version", "0.1"]
    )
    if cargo != expected:
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
    }
    if phase == "run":
        compile_environment["NEXTEST_EXPERIMENTAL_LIBTEST_JSON"] = "1"
    completion_environment = {
        name: environment[name]
        for name in (
            completion.LOCATOR_PATH_ENV_V1,
            completion.LOCATOR_NONCE_ENV_V1,
            completion.REQUEST_ENV_DIGEST_ENV_V1,
            completion.EXPECTED_SOURCE_HEAD_ENV_V1,
        )
    }
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
    binary_custody: list[str],
    qbc_lane: str,
) -> dict[str, Any]:
    label, package, feature, binary, kind, test = case
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
    expected_name = f"{package}::{binary}${test}"
    captures: dict[str, Any] = {}
    archives: dict[str, Any] = {}
    for phase, suffix in (
        ("list", ["--message-format", "json"]),
        ("run", ["--message-format", "libtest-json-plus", "--message-format-version", "0.1"]),
    ):
        cargo = ["cargo", "nextest", phase, *common, "-E", exact_filter, *suffix]
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
            }
        )
        if phase == "run":
            environment["NEXTEST_EXPERIMENTAL_LIBTEST_JSON"] = "1"
        # Hash the actual process environment before adding its own digest.
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
        subprocess.run(binary_custody, check=True)
        exit_code = _execute_qbc_phase(
            semantica, case, phase, cargo, qbc_owner, completion, environment, qbc_lane
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
            "locator_path": str(locator_path),
            "locator_sha256": _sha(locator_raw),
            "qbc_custody": capture.custody_payload_v1(),
            "archived": _archive_capture(evidence, f"{label}-{phase}", capture),
        }
        _frozen(root, quanta_head)
        subprocess.run(binary_custody, check=True)
    verdict = validate_selected(captures["list"].stdout, captures["run"].stdout, expected_name)
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


def _verify_resolution_files(root: Path, semantica: Path, resolutions: dict[str, Any]) -> None:
    for key, consumer in (
        ("runtime", "quanta-runtime"),
        ("kernel", "quanta-runtime-retrieval-kernel"),
    ):
        item = resolutions[key]
        if (
            not isinstance(item, dict)
            or type(item.get("version")) is not int
            or item["version"] != 1
            or item.get("consumer") != consumer
        ):
            raise ValueError("paired resolver consumer identity changed")
        expected_features = (
            set(CALLER_FEATURES.split(",")) if key == "runtime" else {"index-sdk-ingress-surface"}
        )
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


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--quanta-root", required=True, type=Path)
    parser.add_argument("--semantica-root", required=True, type=Path)
    parser.add_argument("--verify-resolution-only", action="store_true")
    parser.add_argument("--evidence-root", type=Path)
    parser.add_argument("--quanta-head")
    parser.add_argument("--semantica-head")
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
    resolutions = {
        "runtime": json.loads(args.runtime_resolution),
        "kernel": json.loads(args.kernel_resolution),
    }
    _verify_resolution_files(root, semantica, resolutions)
    if args.verify_resolution_only:
        _qbc_modules(semantica)
        return 0
    for field in (
        "evidence_root",
        "quanta_head",
        "semantica_head",
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
    _frozen(root, args.quanta_head)
    _frozen(semantica, args.semantica_head)
    # Admit the complete canonical resolver pair before any selected runner.
    # The post-run repetition below rejects source or lock drift during proof.
    _verify_resolution_files(root, semantica, resolutions)
    _require_exact_resolution(
        _resolved_after(root, semantica, "quanta-runtime", CALLER_FEATURES, args.qbc_lane),
        resolutions["runtime"],
        "caller",
    )
    _require_exact_resolution(
        _resolved_after(
            root,
            semantica,
            "quanta-runtime-retrieval-kernel",
            "index-sdk-ingress-surface",
            args.qbc_lane,
        ),
        resolutions["kernel"],
        "kernel",
    )
    _frozen(root, args.quanta_head)
    _frozen(semantica, args.semantica_head)
    output.mkdir(mode=0o700, parents=False)
    completed, completion, qbc_owner = _qbc_modules(semantica)
    binary_custody = [
        sys.executable,
        str(root / "tools/ci/binary_custody.py"),
        "verify",
        args.daemon_digest,
        str(args.built_binary),
        str(args.provided_binary),
        str(args.custody_binary),
    ]
    cases = [
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
            binary_custody,
            args.qbc_lane,
        )
        for case in CASES
    ]
    _frozen(root, args.quanta_head)
    _frozen(semantica, args.semantica_head)
    subprocess.run(binary_custody, check=True)
    _verify_resolution_files(root, semantica, resolutions)
    _require_exact_resolution(
        _resolved_after(root, semantica, "quanta-runtime", CALLER_FEATURES, args.qbc_lane),
        resolutions["runtime"],
        "caller",
    )
    _require_exact_resolution(
        _resolved_after(
            root,
            semantica,
            "quanta-runtime-retrieval-kernel",
            "index-sdk-ingress-surface",
            args.qbc_lane,
        ),
        resolutions["kernel"],
        "kernel",
    )
    _frozen(root, args.quanta_head)
    _frozen(semantica, args.semantica_head)
    subprocess.run(binary_custody, check=True)
    payload = {
        "schema_version": SCHEMA,
        "qualification": "runner-candidate-only",
        "source_heads": {"quanta": args.quanta_head, "semantica": args.semantica_head},
        "resolutions": resolutions,
        "daemon_sha256": args.daemon_digest,
        "binary_paths": {
            "built": str(args.built_binary),
            "provided": str(args.provided_binary),
            "custody": str(args.custody_binary),
        },
        "cases": cases,
    }
    target = output / "paired-r5-result.json"
    with target.open("xb") as handle:
        handle.write(_canonical(payload) + b"\n")
        handle.flush()
        os.fsync(handle.fileno())
    print(f"paired-r5-runner-candidate: {target}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
