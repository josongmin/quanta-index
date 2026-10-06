"""Typed P11 action results; staged contracts never authorize an operation."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import platform
import re
import socket
import sys
import uuid
from datetime import datetime, timezone
from functools import lru_cache
from pathlib import Path
from typing import Any

from tools.benchmark.producer_execution import execute
from tools.ci.lint.handoff_validation import _read_repo_regular_bytes
from tools.ci.proof_execution_result import ExecutionResultError, _bound_artifact_bytes
from tools.ci.proof_json import parse_proof_json

ACTIONS = {
    "p11-deployment": "deployment",
    "p11-activation": "activation",
    "p11-rollback": "restore-forward",
}
PHASES = ("pre", "action", "post")
PASSED_ACTION_COUNTS = {"selected": 1, "executed": 1, "passed": 1, "failed": 0, "ignored": 0}
BASE_STATE = {"daemon_sha256", "config_sha256", "state_root_format"}
EXTRA_STATE = {
    "deployment": set(),
    "activation": {"generation", "query_result_sha256"},
    "restore-forward": {"root_incarnation", "data_sha256", "sequence_high_water"},
}


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise ExecutionResultError(message)


def _object(value: Any, keys: set[str], label: str) -> dict[str, Any]:
    _require(isinstance(value, dict) and set(value) == keys, f"{label} fields differ")
    return value


def _digest(value: Any) -> bool:
    return isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value) is not None


def _absolute(value: Any) -> bool:
    return (
        isinstance(value, str)
        and Path(value).is_absolute()
        and ".." not in Path(value).parts
        and not any(ord(char) < 32 for char in value)
    )


def _time(value: Any) -> datetime:
    _require(isinstance(value, str), "operational timestamp must be a string")
    try:
        result = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        raise ExecutionResultError("invalid operational timestamp") from error
    _require(
        result.tzinfo is not None and result.utcoffset().total_seconds() == 0,
        "operational timestamp must have UTC timezone",
    )
    return result


def _json(raw: bytes, label: str) -> dict[str, Any]:
    result = parse_proof_json(raw)
    _require(isinstance(result, dict), f"{label} must be an object")
    return result


def validate_contract(root: Path, proof: dict[str, Any], raw: bytes) -> dict[str, Any]:
    """A registry-owned contract fixes actors and expected outcomes before execution."""
    contract = _object(
        _json(raw, "operational contract"),
        {
            "schema_version",
            "proof_id",
            "target",
            "actors",
            "timeout_seconds",
            "expected",
        },
        "operational contract",
    )
    _require(
        type(contract["schema_version"]) is int and contract["schema_version"] == 1,
        "operational contract schema differs",
    )
    _require(
        proof["id"] in ACTIONS and contract["proof_id"] == proof["id"],
        "operational action is not a registered P11 action",
    )
    target = _object(
        contract["target"],
        {
            "host_identity_digest",
            "binary_path",
            "config_path",
            "state_root",
        },
        "operational target",
    )
    _require(
        isinstance(target["host_identity_digest"], str)
        and re.fullmatch(r"sha256:[0-9a-f]{64}", target["host_identity_digest"]) is not None,
        "operational host identity differs",
    )
    paths = [target[name] for name in ("binary_path", "config_path", "state_root")]
    _require(
        all(_absolute(path) for path in paths) and len(set(paths)) == 3,
        "operational target paths must be distinct absolute paths",
    )
    actors = _object(contract["actors"], set(PHASES), "operational actors")
    actor_bytes = {}
    for phase, path in actors.items():
        _require(
            isinstance(path, str) and path.endswith(".py"),
            "operational actor must be a registry-owned Python source",
        )
        actor_bytes[phase] = _read_repo_regular_bytes(root, path, label=f"{phase} actor")
    for phase in ("pre", "post"):
        _require(
            actors[phase] != actors["action"] and actor_bytes[phase] != actor_bytes["action"],
            "success observer must be independent from the action producer",
        )
    timeout = contract["timeout_seconds"]
    _require(type(timeout) is int and 1 <= timeout <= 3600, "invalid action timeout")
    expected_keys = set(BASE_STATE)
    if ACTIONS[proof["id"]] == "activation":
        expected_keys.update(EXTRA_STATE["activation"])
    elif ACTIONS[proof["id"]] == "restore-forward":
        expected_keys.update(
            {"backup_root_incarnation", "backup_data_sha256", "sequence_high_water"}
        )
    expected = _object(contract["expected"], expected_keys, "operational expected outcome")
    for name, value in expected.items():
        if name.endswith("sha256"):
            _require(_digest(value), f"expected {name} lacks a digest")
        elif name == "sequence_high_water":
            _require(type(value) is int and value >= 0, "expected sequence high-water differs")
        else:
            _require(isinstance(value, str) and bool(value), f"expected {name} is missing")
    return contract


def _observed(raw: bytes, contract: dict[str, Any], run_id: str, phase: str) -> dict[str, Any]:
    event = _object(
        _json(raw, f"{phase} observer"),
        {
            "schema_version",
            "proof_id",
            "run_id",
            "phase",
            "target",
            "observed",
        },
        f"{phase} observation",
    )
    _require(
        type(event["schema_version"]) is int
        and event["schema_version"] == 1
        and event["proof_id"] == contract["proof_id"]
        and event["run_id"] == run_id
        and event["phase"] == phase
        and event["target"] == contract["target"],
        "observation run, phase or target differs",
    )
    action = ACTIONS[contract["proof_id"]]
    if phase == "action":
        observed = _object(event["observed"], {"action", "completed"}, "action completion")
        _require(
            observed["action"] == action and observed["completed"] is True,
            "typed action completion differs",
        )
        return observed
    observed = _object(event["observed"], BASE_STATE | EXTRA_STATE[action], "observed state")
    for name, value in observed.items():
        if value is None and phase == "pre" and action in {"deployment", "activation"}:
            continue
        if name.endswith("sha256"):
            _require(_digest(value), f"observed {name} lacks a digest")
        elif name == "sequence_high_water":
            _require(type(value) is int and value >= 0, "observed sequence high-water differs")
        else:
            _require(isinstance(value, str) and bool(value), f"observed {name} is missing")
    return observed


def validate_transition(
    contract: dict[str, Any], pre: dict[str, Any], post: dict[str, Any]
) -> None:
    action = ACTIONS[contract["proof_id"]]
    expected = contract["expected"]
    _require(
        all(post[key] == expected[key] for key in BASE_STATE),
        "installed daemon, configuration or state format differs",
    )
    if action == "deployment":
        _require(
            any(pre[key] != post[key] for key in BASE_STATE),
            "deployment has no independently observed state transition",
        )
        return
    _require(
        all(pre[key] == post[key] for key in BASE_STATE),
        "activation or restore changed the attested deployment",
    )
    if action == "activation":
        _require(
            post["generation"] == expected["generation"]
            and post["query_result_sha256"] == expected["query_result_sha256"]
            and pre["generation"] != post["generation"],
            "activation lacks the expected generation and independent query result",
        )
    else:
        _require(
            post["root_incarnation"]
            not in {
                pre["root_incarnation"],
                expected["backup_root_incarnation"],
            }
            and post["data_sha256"] == expected["backup_data_sha256"]
            and post["sequence_high_water"] == expected["sequence_high_water"],
            "restore-forward must preserve backup data and rotate root incarnation",
        )


def derive_operational_result(
    root: Path,
    proof: dict[str, Any],
    result: Any,
    artifacts: list[dict[str, str]],
    binding: dict[str, Any],
    *,
    window: tuple[str, str] | None = None,
) -> dict[str, int]:
    """Recompute one observed action; a caller's success flag or shell exit is insufficient."""
    _require(
        proof.get("execution_mode") == "operational-action" and proof["id"] in ACTIONS,
        "operational result is outside its registered action",
    )
    result = _object(
        result, {"schema_version", "kind", "contract", "events", "target"}, "operational result"
    )
    _require(
        type(result["schema_version"]) is int
        and result["schema_version"] == 1
        and result["kind"] == "operational"
        and result["contract"] == proof.get("operational_contract"),
        "operational result contract differs from the registry",
    )
    by_source = {item["source_path"]: item for item in artifacts}
    _require(len(by_source) == len(artifacts), "duplicate operational artifact source")

    def read(path: str) -> bytes:
        _require(
            isinstance(path, str) and path in by_source,
            "operational evidence is not an archived artifact",
        )
        return _bound_artifact_bytes(root, by_source[path])

    raw_contract = read(result["contract"])
    contract = validate_contract(root, proof, raw_contract)
    expected = contract["expected"]
    _require(
        result["target"] == target_identity(contract),
        "operational target summary differs from its archived contract",
    )
    _require(
        binding["daemon_sha256"] == expected["daemon_sha256"]
        and binding["host_identity_digest"] == contract["target"]["host_identity_digest"]
        and binding["state_root_format"] == expected["state_root_format"],
        "operational contract differs from manifest binary, host or state format",
    )
    record = _object(
        _json(read(result["events"]), "operational events"),
        {
            "schema_version",
            "proof_id",
            "run_id",
            "binding",
            "contract_sha256",
            "steps",
        },
        "operational events",
    )
    _require(
        type(record["schema_version"]) is int
        and record["schema_version"] == 1
        and record["proof_id"] == proof["id"]
        and record["binding"] == binding
        and record["contract_sha256"] == hashlib.sha256(raw_contract).hexdigest()
        and isinstance(record["run_id"], str)
        and re.fullmatch(r"[0-9a-f]{32}", record["run_id"]) is not None,
        "operational execution binding differs",
    )
    steps = record["steps"]
    _require(
        isinstance(steps, list) and len(steps) == len(PHASES),
        "operational execution requires complete pre/action/post custody",
    )
    paths = {result["contract"], result["events"], *contract["actors"].values()}
    previous_end = None
    observations = {}
    for phase, step in zip(PHASES, steps, strict=True):
        step = _object(
            step,
            {
                "phase",
                "owner",
                "owner_sha256",
                "argv",
                "request",
                "execution",
                "started_at",
                "ended_at",
                "stdout",
                "stderr",
            },
            "operational step",
        )
        owner = contract["actors"][phase]
        _require(
            step["phase"] == phase
            and step["owner"] == owner
            and step["owner_sha256"] == hashlib.sha256(read(owner)).hexdigest(),
            "operational actor or phase differs",
        )
        argv = step["argv"]
        _require(
            isinstance(argv, list)
            and len(argv) == 5
            and _absolute(argv[0])
            and argv[1:4] == ["-B", str(root / owner), "--request"]
            and argv[4] == str(root / step["request"]),
            "operational invocation differs from the registered actor",
        )
        request = _json(read(step["request"]), "operational request")
        expected_request = {
            "schema_version": 1,
            "proof_id": proof["id"],
            "run_id": record["run_id"],
            "phase": phase,
            "target": contract["target"],
        }
        if phase == "action":
            expected_request["expected"] = contract["expected"]
        _require(
            type(request.get("schema_version")) is int and request == expected_request,
            "operational request differs or discloses observer gold",
        )
        execution = _object(
            _json(read(step["execution"]), "owned execution"),
            {
                "command",
                "request",
                "status",
                "error_type",
                "output_errors",
                "raw",
            },
            "owned execution",
        )
        command = execution.get("command")
        _require(
            isinstance(command, dict)
            and command.get("argv") == argv
            and command.get("cwd") == str(root)
            and command.get("status") == "completed"
            and type(command.get("exit_code")) is int
            and command["exit_code"] == 0
            and type(command.get("timeout_seconds")) is int
            and command.get("timeout_seconds") == contract["timeout_seconds"]
            and execution["request"]
            == {"argv": argv, "cwd": str(root), "timeout_seconds": contract["timeout_seconds"]}
            and execution["status"] == "completed"
            and execution["error_type"] is None
            and execution["output_errors"] == [],
            "owned operational process did not complete with integer exit zero",
        )
        raw = execution.get("raw")
        _require(
            isinstance(raw, list) and len(raw) == 2,
            "owned operational execution lacks both raw outputs",
        )
        for key, output_ref in zip(("stdout", "stderr"), raw, strict=True):
            _require(
                isinstance(output_ref, dict)
                and output_ref.get("path") == str(root / step[key])
                and output_ref.get("sha256")
                == "sha256:" + hashlib.sha256(read(step[key])).hexdigest()
                and type(output_ref.get("bytes")) is int
                and output_ref["bytes"] == len(read(step[key])),
                "owned output binding differs",
            )
        start, end = _time(step["started_at"]), _time(step["ended_at"])
        _require(
            start <= end and (previous_end is None or previous_end <= start),
            "operational observations overlap or precede their action",
        )
        if window is not None:
            _require(
                _time(window[0]) <= start <= end <= _time(window[1]),
                "operational step is outside the manifest execution window",
            )
        previous_end = end
        for key in ("stdout", "stderr", "request", "execution"):
            _require(
                isinstance(step[key], str) and step[key] not in paths,
                "operational step reuses evidence",
            )
            paths.add(step[key])
        observations[phase] = _observed(read(step["stdout"]), contract, record["run_id"], phase)
        read(step["stderr"])
    validate_transition(contract, observations["pre"], observations["post"])
    return dict(PASSED_ACTION_COUNTS)


def target_identity(contract: dict[str, Any]) -> dict[str, str]:
    """Derived summary for cross-action continuity; the archived contract remains authoritative."""
    return dict(contract["target"], config_sha256=contract["expected"]["config_sha256"])


@lru_cache(maxsize=1)
def _checker():
    path = Path(__file__).with_name("lint") / "check-proof-authority.py"
    spec = importlib.util.spec_from_file_location("quanta_operational_checker", path)
    _require(spec is not None and spec.loader is not None, "proof checker is unavailable")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _check_source_binding(
    root: Path, proof: dict, binding: dict, output: Path, paired_checkout: Path
) -> None:
    checker = _checker()
    source = checker.proof_source_snapshot(
        root,
        manifest_path=root / proof["artifact"],
        proof=proof,
        excluded_paths=(output, paired_checkout),
    )
    pair = checker.paired_source_snapshot(
        paired_checkout,
        repository=proof["paired_repository"],
        dependency_lock=Path(proof["paired_dependency_lock"]),
    )
    _require(
        source == binding["source"]
        and pair == binding["source_pair"]
        and source["dirty_digest"] == checker.CLEAN_DIRTY_DIGEST
        and pair["source"]["dirty_digest"] == checker.CLEAN_DIRTY_DIGEST,
        "operational primary or paired source differs or is dirty",
    )


def manifest_binding(payload: dict[str, Any]) -> dict[str, Any]:
    _require(
        isinstance(payload["source_pair"], dict)
        and isinstance(payload["daemon_binary"], dict)
        and payload["environment"]["os"] == "linux",
        "operational result requires exact source pair and a Linux release daemon",
    )
    return {
        "source": payload["source"],
        "source_pair": payload["source_pair"],
        "daemon_sha256": payload["daemon_binary"]["sha256"],
        "host_identity_digest": payload["environment"]["host"]["identity_digest"],
        "state_root_format": payload["state_root_format"],
        "dependency_receipts": payload["dependency_receipts"],
    }


def observed_host_identity() -> str:
    return _writer()._host_environment(observed_host_environment())["host"]["identity_digest"]


def observed_host_environment() -> dict[str, Any]:
    _require(platform.system() == "Linux", "operational action requires the actual Linux host")
    machine = _read_repo_regular_bytes(
        Path("/"), "etc/machine-id", label="Linux machine identity"
    ).strip()
    _require(
        re.fullmatch(rb"[0-9a-f]{32}", machine) is not None, "Linux machine identity is invalid"
    )
    host = socket.gethostname()
    _require(bool(host), "Linux hostname is unavailable")
    cpu = os.cpu_count()
    page_size, pages = os.sysconf("SC_PAGE_SIZE"), os.sysconf("SC_PHYS_PAGES")
    _require(
        type(cpu) is int
        and cpu > 0
        and type(page_size) is int
        and page_size > 0
        and type(pages) is int
        and pages > 0,
        "actual Linux host capacity is unavailable",
    )
    return {
        "toolchain": sys.version,
        "features": [],
        "os": "linux",
        "arch": platform.machine(),
        "host": {
            "profile": "linux-production-like",
            "cpu_count": cpu,
            "memory_bytes": page_size * pages,
            "identity": "quanta-operational-host-v1:" + machine.decode() + ":" + host,
        },
    }


@lru_cache(maxsize=1)
def _writer():
    path = Path(__file__).with_name("write-proof-manifest.py")
    spec = importlib.util.spec_from_file_location("quanta_operational_manifest_writer", path)
    _require(spec is not None and spec.loader is not None, "proof manifest writer is unavailable")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def run_action(
    root: Path,
    proof: dict[str, Any],
    binding: dict[str, Any],
    output: Path,
    *,
    paired_checkout: Path,
) -> tuple[dict[str, Any], list[str]]:
    """Execute only registry-owned actors. This raw producer cannot promote a staged proof."""
    _require(
        proof.get("authority_state") == "executable",
        "staged operational action cannot execute or issue evidence",
    )
    _require(
        proof.get("execution_mode") == "operational-action" and proof["id"] in ACTIONS,
        "operational action is not registered",
    )
    path = proof.get("operational_contract")
    _require(isinstance(path, str), "operational contract is missing")
    contract_raw = _read_repo_regular_bytes(root, path, label="operational contract")
    contract = validate_contract(root, proof, contract_raw)
    _require(
        observed_host_identity()
        == binding["host_identity_digest"]
        == contract["target"]["host_identity_digest"],
        "actual operational host differs",
    )
    _require(
        binding["daemon_sha256"] == contract["expected"]["daemon_sha256"]
        and binding["state_root_format"] == contract["expected"]["state_root_format"],
        "action binding differs from its expected deployment",
    )
    output = output.absolute()
    _require(
        output.is_relative_to(root) and not output.exists() and not output.is_symlink(),
        "raw operational output must be a fresh repository-relative directory",
    )
    frozen = {
        name: _read_repo_regular_bytes(root, name, label="operational input")
        for name in {path, *contract["actors"].values()}
    }

    def check() -> None:
        _require(
            observed_host_identity() == binding["host_identity_digest"],
            "actual operational host changed during execution",
        )
        _check_source_binding(root, proof, binding, output, paired_checkout)
        for receipt in binding["dependency_receipts"]:
            _bound_artifact_bytes(root, receipt)
        if binding["dependency_receipts"]:
            checker = _checker()
            registry = checker._read_toml(root / "tools/ci/proof-authority.toml")
            authorities = {item["id"]: item for item in registry["proofs"]}
            schema = checker._payload_json(root, proof["artifact_schema"], label="manifest schema")
            for receipt in binding["dependency_receipts"]:
                payload = _json(_bound_artifact_bytes(root, receipt), "operational prerequisite")
                findings = checker.check_manifest(
                    payload,
                    proof=authorities[receipt["proof_id"]],
                    root=root,
                    manifest_path=root / receipt["path"],
                    schema=schema,
                    bind_source=True,
                    paired_checkouts={proof["paired_repository"]: paired_checkout},
                    proof_by_id=authorities,
                    bound_source=binding["source"],
                    bound_source_pair=binding["source_pair"],
                )
                _require(
                    not findings,
                    "operational prerequisite custody changed: "
                    + "; ".join(item.render() for item in findings),
                )
        for name, raw in frozen.items():
            _require(
                _read_repo_regular_bytes(root, name, label="operational input") == raw,
                "operational contract or actor changed during execution",
            )

    check()
    output.mkdir(parents=True)
    run_id = uuid.uuid4().hex
    record = {
        "schema_version": 1,
        "proof_id": proof["id"],
        "run_id": run_id,
        "binding": binding,
        "contract_sha256": hashlib.sha256(contract_raw).hexdigest(),
        "steps": [],
    }
    artifacts = list(frozen)
    observations = {}
    for phase in PHASES:
        check()
        owner = contract["actors"][phase]
        request = {
            "schema_version": 1,
            "proof_id": proof["id"],
            "run_id": run_id,
            "phase": phase,
            "target": contract["target"],
        }
        if phase == "action":
            request["expected"] = contract["expected"]
        request_path = output / f"{phase}.request.json"
        with request_path.open("x") as stream:
            stream.write(json.dumps(request, allow_nan=False) + "\n")
        request_relative = str(request_path.relative_to(root))
        frozen[request_relative] = request_path.read_bytes()
        argv = [sys.executable, "-B", str(root / owner), "--request", str(request_path)]
        log_dir = output / phase
        log_dir.mkdir()
        start = datetime.now(timezone.utc).isoformat()
        completed = execute(
            argv,
            cwd=root,
            env=dict(os.environ, PYTHONDONTWRITEBYTECODE="1"),
            timeout=contract["timeout_seconds"],
            log_dir=log_dir,
        )
        end = datetime.now(timezone.utc).isoformat()
        check()
        code = completed.command["exit_code"]
        _require(type(code) is int and code == 0, f"{phase} actor failed: {code}")
        stdout, stderr = root / completed.stdout.path, root / completed.stderr.path
        observations[phase] = _observed(stdout.read_bytes(), contract, run_id, phase)
        relative_stdout, relative_stderr = (
            str(item.relative_to(root)) for item in (stdout, stderr)
        )
        execution = str((log_dir / "execution.json").relative_to(root))
        artifacts.extend([relative_stdout, relative_stderr, request_relative, execution])
        record["steps"].append(
            {
                "phase": phase,
                "owner": owner,
                "owner_sha256": hashlib.sha256(frozen[owner]).hexdigest(),
                "argv": argv,
                "request": request_relative,
                "execution": execution,
                "started_at": start,
                "ended_at": end,
                "stdout": relative_stdout,
                "stderr": relative_stderr,
            }
        )
    validate_transition(contract, observations["pre"], observations["post"])
    check()
    events = output / "events.json"
    with events.open("x") as stream:
        stream.write(json.dumps(record, sort_keys=True, indent=2) + "\n")
        stream.flush()
        os.fsync(stream.fileno())
    relative_events = str(events.relative_to(root))
    artifacts.append(relative_events)
    return {
        "schema_version": 1,
        "kind": "operational",
        "contract": path,
        "events": relative_events,
        "target": target_identity(contract),
    }, artifacts
