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
from collections.abc import Callable
from datetime import datetime, timezone
from functools import lru_cache
from pathlib import Path
from typing import Any

from tools.benchmark.evidence import RawFile, write_raw_file
from tools.benchmark.producer_execution import execute
from tools.ci import p11_attestation
from tools.ci.lint.handoff_validation import (
    _open_repo_output_parent,
    _read_repo_regular_bytes,
    _repo_path,
)
from tools.ci.proof_execution_result import ExecutionResultError, _bound_artifact_bytes
from tools.ci.proof_json import parse_proof_json

ACTIONS = {
    "p11-deployment": "deployment",
    "p11-activation": "activation",
    "p11-rollback": "restore-forward",
}
PHASES = ("pre", "action", "post")
PASSED_ACTION_COUNTS = {"selected": 1, "executed": 1, "passed": 1, "failed": 0, "ignored": 0}
BASE_STATE = {"daemon_sha256", "config_sha256", "state_root_format", "release_manifest_sha256"}
EXTRA_STATE = {
    "deployment": set(),
    "activation": {"generation", "query_result_sha256"},
    "restore-forward": {"root_incarnation", "data_sha256", "sequence_high_water"},
}


# Pass this bootstrap by value to the isolated child. It verifies and consumes
# each input once, before executing any registered actor code.
_ACTOR_BOOTSTRAP = """\
import hashlib
import os
import sys
import tempfile
import types
from pathlib import Path

actor, actor_sha, request, request_sha = sys.argv[1:]
def checked(path, expected):
    raw = Path(path).read_bytes()
    if hashlib.sha256(raw).hexdigest() != expected:
        raise SystemExit("registered operational input changed")
    return raw

code = compile(checked(actor, actor_sha), actor, "exec")
with tempfile.TemporaryFile() as pinned:
    pinned.write(checked(request, request_sha))
    pinned.seek(0)
    sys.argv = [actor, "--request", "/dev/fd/" + str(pinned.fileno())]
    sys.path.insert(0, os.path.dirname(actor))
    main = types.ModuleType("__main__")
    main.__file__ = actor
    sys.modules["__main__"] = main
    exec(code, main.__dict__)
"""


def actor_argv(
    python: str, owner: Path, actor_sha: str, request: Path, request_sha: str
) -> list[str]:
    return [
        python,
        "-I",
        "-B",
        "-c",
        _ACTOR_BOOTSTRAP,
        str(owner),
        actor_sha,
        str(request),
        request_sha,
    ]


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
        and os.path.normpath(value) == value
        and not value.startswith("//")
        and Path(value) != Path("/")
        and ".." not in Path(value).parts
        and not any(ord(char) < 32 for char in value)
    )


def _validate_target_paths(target: dict[str, Any]) -> None:
    """Refuse ambiguous or overlapping install/state destinations before actors run."""
    names = ("binary_path", "config_path", "state_root")
    paths = [target[name] for name in names]
    _require(
        all(_absolute(path) for path in paths),
        "operational target paths must be normalized absolute paths below root",
    )
    parsed = [Path(path) for path in paths]
    _require(
        all(
            not left.is_relative_to(right) and not right.is_relative_to(left)
            for index, left in enumerate(parsed)
            for right in parsed[index + 1 :]
        ),
        "operational binary, config and state paths must not overlap",
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


def _validate_actor_independence(actors: dict[str, str], actor_bytes: dict[str, bytes]) -> None:
    for phase in ("pre", "post"):
        _require(
            actors[phase] != actors["action"] and actor_bytes[phase] != actor_bytes["action"],
            "success observer must be independent from the action producer",
        )


def _contract_with_actors(
    root: Path,
    proof: dict[str, Any],
    raw: bytes,
    *,
    read_actor: Callable[[str], bytes] | None = None,
) -> tuple[dict[str, Any], dict[str, bytes]]:
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
    p11_attestation.public_key(proof.get("attestation_public_key"))
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
    _validate_target_paths(target)
    actors = _object(contract["actors"], set(PHASES), "operational actors")
    actor_bytes = {}
    for phase, path in actors.items():
        _require(
            isinstance(path, str) and path.endswith(".py"),
            "operational actor must be a registry-owned Python source",
        )
        _repo_path(root, path, label=f"{phase} actor")
        actor_bytes[phase] = (
            read_actor(path)
            if read_actor is not None
            else _read_repo_regular_bytes(root, path, label=f"{phase} actor")
        )
    _validate_actor_independence(actors, actor_bytes)
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
    return contract, actor_bytes


def validate_contract(root: Path, proof: dict[str, Any], raw: bytes) -> dict[str, Any]:
    """A registry-owned contract fixes actors and expected outcomes before execution."""
    contract, _ = _contract_with_actors(root, proof, raw)
    return contract


def _require_committed_sources(root: Path, head: str, files: dict[str, bytes]) -> None:
    """Bind every operational instruction file to a regular blob in the claimed commit."""
    try:
        committed = _checker().committed_source_files(root, head, files)
    except ValueError as error:
        raise ExecutionResultError(f"cannot read committed operational source: {error}") from error
    for name, raw in files.items():
        _require(committed[name] == raw, f"{name} differs from committed operational source")


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


def validate_precondition(contract: dict[str, Any], pre: dict[str, Any]) -> None:
    """Refuse an inadmissible live state before invoking the mutating actor."""
    action = ACTIONS[contract["proof_id"]]
    expected = contract["expected"]
    if action == "deployment":
        _require(
            any(pre[key] != expected[key] for key in BASE_STATE),
            "deployment has no independently observed state transition",
        )
        return
    _require(
        all(pre[key] == expected[key] for key in BASE_STATE),
        "operational pre-state differs from the attested deployment",
    )
    if action == "activation":
        _require(
            pre["generation"] != expected["generation"],
            "activation pre-state already selects the expected generation",
        )


def validate_transition(
    contract: dict[str, Any], pre: dict[str, Any], post: dict[str, Any]
) -> None:
    validate_precondition(contract, pre)
    action = ACTIONS[contract["proof_id"]]
    expected = contract["expected"]
    _require(
        all(post[key] == expected[key] for key in BASE_STATE),
        "installed daemon, configuration, state format or release manifest differs",
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
        _repo_path(root, path, label="archived operational input")
        return _bound_artifact_bytes(root, by_source[path])

    raw_contract = read(result["contract"])
    contract, actor_bytes = _contract_with_actors(root, proof, raw_contract, read_actor=read)
    _require_committed_sources(
        root,
        binding["source"]["head"],
        {
            result["contract"]: raw_contract,
            **{owner: actor_bytes[phase] for phase, owner in contract["actors"].items()},
        },
    )
    expected = contract["expected"]
    _require(
        binding["inputs"] == release_inputs(raw_contract),
        "operational execution binding differs from contract inputs",
    )
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
        and json.dumps(record["binding"], sort_keys=True) == json.dumps(binding, sort_keys=True)
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
    run_root = None
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
            and step["owner_sha256"] == hashlib.sha256(actor_bytes[phase]).hexdigest(),
            "operational actor or phase differs",
        )
        argv = step["argv"]
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
            isinstance(command, dict) and _absolute(command.get("cwd")),
            "owned operational process lacks its original checkout path",
        )
        cwd = Path(command["cwd"])
        if run_root is None:
            run_root = cwd
        _require(
            cwd == run_root
            and isinstance(argv, list)
            and bool(argv)
            and _absolute(argv[0])
            and argv
            == actor_argv(
                argv[0],
                cwd / owner,
                step["owner_sha256"],
                cwd / step["request"],
                by_source[step["request"]]["sha256"],
            ),
            "operational invocation differs from the registered actor or checkout",
        )
        _require(
            command.get("argv") == argv
            and command.get("status") == "completed"
            and type(command.get("exit_code")) is int
            and command["exit_code"] == 0
            and type(command.get("timeout_seconds")) is int
            and command.get("timeout_seconds") == contract["timeout_seconds"]
            and execution["request"]
            == {"argv": argv, "cwd": str(cwd), "timeout_seconds": contract["timeout_seconds"]}
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
        outputs = {key: read(step[key]) for key in ("stdout", "stderr")}
        for key, output_ref in zip(outputs, raw, strict=True):
            _require(
                isinstance(output_ref, dict)
                and output_ref.get("path") == str(cwd / step[key])
                and output_ref.get("sha256") == "sha256:" + hashlib.sha256(outputs[key]).hexdigest()
                and type(output_ref.get("bytes")) is int
                and output_ref["bytes"] == len(outputs[key]),
                "owned output binding differs",
            )
        start, end = _time(step["started_at"]), _time(step["ended_at"])
        _require(
            start <= end and (previous_end is None or previous_end <= start),
            "operational observations overlap or precede their action",
        )
        previous_end = end
        for key in ("stdout", "stderr", "request", "execution"):
            _require(
                isinstance(step[key], str) and step[key] not in paths,
                "operational step reuses evidence",
            )
            paths.add(step[key])
        observations[phase] = _observed(outputs["stdout"], contract, record["run_id"], phase)
    if window is not None:
        _require(
            window == (steps[0]["started_at"], steps[-1]["ended_at"]),
            "manifest execution window differs from the recorded pre/post observations",
        )
    validate_transition(contract, observations["pre"], observations["post"])
    signature_path = result["events"] + ".sig"
    _require(signature_path in by_source, "operational execution lacks producer attestation")
    _require(
        paths | {signature_path} == set(by_source),
        "operational archive contains unreferenced sources",
    )
    statement = p11_attestation.artifact_statement(
        {name: by_source[name]["sha256"] for name in paths}
    )
    p11_attestation.verify(
        proof["attestation_public_key"], proof["id"], statement, read(signature_path)
    )
    return dict(PASSED_ACTION_COUNTS)


def target_identity(contract: dict[str, Any]) -> dict[str, str]:
    """Derived summary for cross-action continuity; the archived contract remains authoritative."""
    return dict(
        contract["target"],
        config_sha256=contract["expected"]["config_sha256"],
        release_manifest_sha256=contract["expected"]["release_manifest_sha256"],
    )


def installation_identity(target: dict[str, Any]) -> tuple[str, str, str]:
    root = target.get("state_root")
    config = target.get("config_sha256")
    manifest = target.get("release_manifest_sha256")
    _require(
        _absolute(root) and _digest(config) and _digest(manifest),
        "signed installation target lacks its state root, config SHA-256 or release manifest SHA-256",
    )
    return root, config, manifest


def operational_installation_identity(payload_by_id: dict[str, Any]) -> tuple[str, str, str]:
    identities = set()
    for proof_id in ACTIONS:
        proof = payload_by_id.get(proof_id, {})
        result = proof.get("execution_result")
        _require(
            isinstance(result, dict) and result.get("kind") == "operational"
            and isinstance(result.get("target"), dict),
            f"{proof_id} lacks the signed installation target binding",
        )
        identities.add(installation_identity(result["target"]))
    _require(len(identities) == 1, "operational actions differ on the signed installation target")
    return identities.pop()


def validate_prerequisite_target(target: dict[str, str] | None, payload: dict[str, Any]) -> None:
    """Operational edges must refer to the same target before any new action."""
    if payload["proof_id"] not in ACTIONS:
        return
    result = payload.get("execution_result")
    _require(
        isinstance(result, dict)
        and result.get("kind") == "operational"
        and result.get("target") == target,
        "operational prerequisite belongs to a different target or configuration",
    )


def validate_release_binding(
    binding: dict[str, Any],
    payload: dict[str, Any],
    *,
    release_daemon: bool,
) -> None:
    """Bind a selected proof to the same release inputs throughout the P11 closure."""
    proof_id = payload["proof_id"]
    checker = _checker()
    _require(
        checker.source_content_identity(payload["source"])
        == checker.source_content_identity(binding["source"]),
        f"{proof_id} prerequisite attests a different source",
    )
    pair = payload["source_pair"]
    if pair is not None:
        _require(
            checker.paired_content_identity(pair)
            == checker.paired_content_identity(binding["source_pair"]),
            f"{proof_id} prerequisite attests a different source pair or resolver lock",
        )
    if release_daemon:
        binary = payload.get("daemon_binary")
        _require(
            isinstance(binary, dict)
            and binary.get("sha256") == binding["daemon_sha256"]
            and payload["state_root_format"] == binding["state_root_format"],
            f"{proof_id} attests a different daemon or state-root format",
        )


def validate_release_prerequisite(
    binding: dict[str, Any],
    target: dict[str, str] | None,
    payload: dict[str, Any],
    *,
    started_at: str,
) -> None:
    """Apply the same P11 release edge checks before execution and during archive replay."""
    proof_id = payload["proof_id"]
    _require(payload.get("status") == "passed", f"{proof_id} prerequisite has no passing result")
    validate_prerequisite_target(target, payload)
    validate_release_binding(binding, payload, release_daemon=True)
    _require(
        _time(payload["ended_at"]) <= _time(started_at),
        f"{proof_id} prerequisite ends after the successor starts",
    )


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
        checker.release_source_identity({"source": source, "source_pair": pair})
        == checker.release_source_identity(binding)
        and source["dirty_digest"] == checker.CLEAN_DIRTY_DIGEST
        and pair["source"]["dirty_digest"] == checker.CLEAN_DIRTY_DIGEST,
        "operational primary or paired source differs or is dirty",
    )


def release_inputs(contract_raw: bytes | None = None) -> dict[str, str | None]:
    inputs = dict.fromkeys(_writer().INPUT_NAMES)
    if contract_raw is not None:
        inputs["config"] = "sha256:" + hashlib.sha256(contract_raw).hexdigest()
    return inputs


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
        "environment": payload["environment"],
        "inputs": payload["inputs"],
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
    before_action: Callable[[], None],
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
    contract, actor_bytes = _contract_with_actors(root, proof, contract_raw)
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
    _require(
        binding["inputs"] == release_inputs(contract_raw),
        "action binding differs from contract inputs",
    )
    output = output.absolute()
    _require(
        output != root
        and output.is_relative_to(root)
        and not output.is_relative_to(root / ".git")
        and not output.is_relative_to(paired_checkout)
        and output == output.resolve()
        and not output.exists(),
        "raw operational output must be a fresh canonical repository-relative directory",
    )
    output_relative = output.relative_to(root).as_posix()
    artifact_root = (root / proof["artifact"]).parent
    _require(
        output.is_relative_to(artifact_root)
        or _checker()._optional_git(root, "check-ignore", "--", f"{output_relative}/") is not None,
        "raw operational output would dirty the release source during publication",
    )
    frozen = {
        path: contract_raw,
        **{owner: actor_bytes[phase] for phase, owner in contract["actors"].items()},
    }
    _require_committed_sources(root, binding["source"]["head"], frozen)
    sealed_outputs: list[RawFile] = []
    started_at = datetime.now(timezone.utc).isoformat()
    dependency_receipts = binding["dependency_receipts"]
    _require(
        [item["proof_id"] for item in dependency_receipts] == proof["dependencies"],
        "operational prerequisites differ from the registered edges",
    )

    def check() -> None:
        _require(
            observed_host_identity() == binding["host_identity_digest"],
            "actual operational host changed during execution",
        )
        _check_source_binding(root, proof, binding, output, paired_checkout)
        if dependency_receipts:
            checker = _checker()
            registry = checker._read_toml(root / "tools/ci/proof-authority.toml")
            authorities = {item["id"]: item for item in registry["proofs"]}
            for receipt in dependency_receipts:
                payload = _json(_bound_artifact_bytes(root, receipt), "operational prerequisite")
                _require(
                    payload["proof_id"] == receipt["proof_id"],
                    "operational prerequisite proof_id differs from its edge",
                )
                validate_release_prerequisite(
                    binding, target_identity(contract), payload, started_at=started_at
                )
                _require(
                    checker._payload_sha256(
                        root,
                        authorities[receipt["proof_id"]]["artifact"],
                        label="current prerequisite",
                    )
                    == receipt["sha256"],
                    "operational prerequisite is no longer the current receipt",
                )
                checker.require_current_release(root, payload, receipt["sha256"])
                findings = checker.check_manifest(
                    payload,
                    proof=authorities[receipt["proof_id"]],
                    root=root,
                    manifest_path=root / receipt["path"],
                    bind_source=True,
                    require_current_dependencies=True,
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
        for item in sealed_outputs:
            _require(
                RawFile.capture(item.path) == item,
                f"operational output custody changed: {item.path.relative_to(root)}",
            )

    check()
    p11_attestation.sign(proof["attestation_public_key"], proof["id"], b"P11 signing preflight")
    parent_fd = _open_repo_output_parent(root, output_relative, sync_ancestors=True)
    try:
        os.mkdir(output.name, mode=0o700, dir_fd=parent_fd)
        os.fsync(parent_fd)
    finally:
        os.close(parent_fd)
    run_id = uuid.uuid4().hex
    record = {
        "schema_version": 1,
        "proof_id": proof["id"],
        "run_id": run_id,
        "binding": binding,
        "contract_sha256": hashlib.sha256(contract_raw).hexdigest(),
        "steps": [],
    }
    actor_env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
    signer_socket = actor_env.pop(p11_attestation.SOCKET_ENV, None)
    p11_attestation.remove_signing_agent(actor_env, signer_socket)
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
        request_raw = (json.dumps(request, allow_nan=False) + "\n").encode()
        write_raw_file(request_path, [request_raw])
        request_relative = str(request_path.relative_to(root))
        frozen[request_relative] = request_raw
        argv = actor_argv(
            sys.executable,
            root / owner,
            hashlib.sha256(frozen[owner]).hexdigest(),
            request_path,
            hashlib.sha256(request_raw).hexdigest(),
        )
        log_dir = output / phase
        log_dir.mkdir()
        if phase == "action":
            before_action()
            check()
        start = datetime.now(timezone.utc).isoformat()
        completed = execute(
            argv,
            cwd=root,
            env=actor_env,
            timeout=contract["timeout_seconds"],
            log_dir=log_dir,
        )
        end = datetime.now(timezone.utc).isoformat()
        check()
        code = completed.command["exit_code"]
        _require(type(code) is int and code == 0, f"{phase} actor failed: {code}")
        sealed_outputs.extend(
            (completed.stdout, completed.stderr, RawFile.capture(log_dir / "execution.json"))
        )
        observations[phase] = _observed(completed.stdout.read_control(), contract, run_id, phase)
        if phase == "pre":
            validate_precondition(contract, observations[phase])
        relative_stdout, relative_stderr = (
            str(item.path.relative_to(root)) for item in (completed.stdout, completed.stderr)
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
    event_raw = (json.dumps(record, sort_keys=True, indent=2) + "\n").encode()
    sealed_outputs.append(write_raw_file(events, [event_raw]))
    relative_events = str(events.relative_to(root))
    digests = {name: hashlib.sha256(data).hexdigest() for name, data in frozen.items()}
    digests.update(
        {
            item.path.relative_to(root).as_posix(): item.sha256.removeprefix("sha256:")
            for item in sealed_outputs
        }
    )
    signature = p11_attestation.sign(
        proof["attestation_public_key"], proof["id"], p11_attestation.artifact_statement(digests)
    )
    check()
    write_raw_file(output / "events.json.sig", [signature])
    artifacts.extend([relative_events, relative_events + ".sig"])
    return {
        "schema_version": 1,
        "kind": "operational",
        "contract": path,
        "events": relative_events,
        "target": target_identity(contract),
    }, artifacts
