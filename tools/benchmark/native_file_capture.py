"""Owned Native file SDK capture through the existing benchmark custody path.

The adapter proves controlled producer execution and raw capture custody. It
does not grant installed-runtime, relevance-quality, or product authority.
"""

from __future__ import annotations

import hashlib
import os
import platform
import re
import socket
from datetime import datetime, timezone
from pathlib import Path

from evidence import EvidenceError, RawFile, RunStore, file_digest, parse_json, read_control
from evidence_bridge import host_identity, source_identity
from producer_execution import execute
from profile_capture import (
    _directories,
    capture_entrypoint,
    current_capture,
    load_capture,
    publish_capture,
)
from registry import registry_digest

from tools.benchmark.retrieval import evaluator, native_file_decision

PROFILE = "native-file-capture"
FAMILY = "native-file-sdk-capture"
ROOT = Path(__file__).resolve().parents[2]
PLAN_KEYS = {
    "schema_version",
    "cohort_manifest",
    "decision_policy",
    "daemon_binary",
    "daemon_config",
    "runner_binary",
    "family_id",
    "view_json",
    "open_key",
    "text_instance",
    "semantic_instance",
    "file_instance",
    "lane_top_k",
    "file_top_k",
}
HEX32 = re.compile(r"[0-9a-f]{32}\Z")
HEX64 = re.compile(r"[0-9a-f]{64}\Z")


def require_registered_owner(registry: dict) -> None:
    """Pin the adapter to its unique registry producer and validator roles."""
    try:
        profile = registry["profiles"][PROFILE]
        entry = registry["families"][FAMILY]
        producer = registry["producers"][entry["producer"]]
        validator = registry["validators"][entry["validator"]]
    except (KeyError, TypeError) as error:
        raise EvidenceError("Native capture registration is missing") from error
    if (
        profile.get("families") != [FAMILY]
        or entry.get("purpose") != "retrieval"
        or entry.get("payload") != "proof"
        or entry.get("result_unit") != "count"
        or entry.get("producer") != FAMILY
        or entry.get("validator") != FAMILY
        or entry.get("scorer") != "none"
        or entry.get("closure") != "retrieval"
        or entry.get("host_policy") != "any"
        or entry.get("gate_tier") != "contract"
        or entry.get("native_schema") != "native-file-sdk-capture:v1"
        or entry.get("authority") != "registry"
        or entry.get("sample_floor") != 0
        or entry.get("baseline") != "none"
        or producer.get("kind") != "python-module"
        or producer.get("module") != "tools/benchmark/native_file_capture.py"
        or producer.get("argv") != []
        or producer.get("outputs") != []
        or validator.get("kind") != "python-module"
        or validator.get("module") != "tools/benchmark/native_file_capture.py"
        or validator.get("argv") != ["validate"]
    ):
        raise EvidenceError("Native capture registration differs from implemented owner")


def execute_owned_native(
    runner: Path,
    flags: list[str],
    *,
    cwd: Path,
    log_dir: Path,
    timeout: int,
    epoch=None,
) -> dict:
    """Execute one runner and retain its unmodified output and terminal record."""
    if flags.count("--out") != 1:
        raise EvidenceError("Native capture needs exactly one output path")
    index = flags.index("--out") + 1
    if index >= len(flags):
        raise EvidenceError("Native capture output path is missing")
    output = Path(flags[index])
    if not output.is_absolute() or output.exists() or output.is_symlink():
        raise EvidenceError("Native capture output must be a fresh absolute path")
    log_dir.mkdir(parents=True, exist_ok=False)
    argv = [str(runner), "native-file-profile", *flags]
    kwargs = {"cwd": cwd, "env": dict(os.environ), "timeout": timeout, "log_dir": log_dir}
    if epoch is None:
        stdout, stderr, command = execute(argv, **kwargs)
    else:
        stdout, stderr, command = epoch.execute(execute, argv, **kwargs)
    if command.get("exit_code") != 0:
        raise EvidenceError("Native runner lacks successful terminal status")
    return {
        "capture": RawFile.capture(output),
        "execution": RawFile.capture(log_dir / "execution.json"),
        "stdout": stdout,
        "stderr": stderr,
        "command": command,
    }


def _plan(path: Path) -> dict:
    value = parse_json(read_control(path).decode("utf-8"))
    if (
        not isinstance(value, dict)
        or set(value) != PLAN_KEYS
        or type(value["schema_version"]) is not int
        or value["schema_version"] != 1
    ):
        raise EvidenceError("Native execution plan shape differs")
    for key in (
        "cohort_manifest",
        "decision_policy",
        "daemon_binary",
        "daemon_config",
        "runner_binary",
    ):
        if not isinstance(value[key], str) or not Path(value[key]).is_absolute():
            raise EvidenceError(f"Native execution {key} must be absolute")
        value[key] = str(Path(value[key]).resolve())
    if not isinstance(value["family_id"], str) or not value["family_id"]:
        raise EvidenceError("Native execution family ID is absent")
    if not isinstance(value["view_json"], str) or not value["view_json"]:
        raise EvidenceError("Native execution view is absent")
    for key, pattern in (
        ("open_key", HEX64),
        ("text_instance", HEX32),
        ("semantic_instance", HEX32),
        ("file_instance", HEX32),
    ):
        if not isinstance(value[key], str) or pattern.fullmatch(value[key]) is None:
            raise EvidenceError(f"Native execution {key} is malformed")
    if (
        value["open_key"] == "0" * 64
        or len({value[key] for key in ("text_instance", "semantic_instance", "file_instance")}) != 3
    ):
        raise EvidenceError("Native execution open key or instances are invalid")
    if (
        type(value["lane_top_k"]) is not int
        or type(value["file_top_k"]) is not int
        or not 10 <= value["file_top_k"] <= value["lane_top_k"] <= 1024
    ):
        raise EvidenceError("Native execution depth is invalid")
    return value


def _family(plan: dict) -> tuple[Path, dict]:
    manifest_path = Path(plan["cohort_manifest"]).resolve()
    _raw, manifest = evaluator._native_cohort_manifest(manifest_path)
    matches = [row for row in manifest["families"] if row["family_id"] == plan["family_id"]]
    if len(matches) != 1:
        raise EvidenceError("Native execution family is absent or repeated")
    return manifest_path, matches[0]


def _roles(
    plan_path: Path, plan: dict, manifest_path: Path, family: dict
) -> tuple[dict[str, Path], dict[str, Path]]:
    files = {"plan": plan_path, "cohort": manifest_path}
    files.update(
        {
            key: Path(plan[key])
            for key in ("decision_policy", "daemon_binary", "daemon_config", "runner_binary")
        }
    )
    files.update(
        {
            key: (manifest_path.parent / family[key]).resolve()
            for key in ("suite", "query_pack", "source_universe", "source_binding")
        }
    )
    trees = {
        key: (manifest_path.parent / family[key]).absolute()
        for key in ("base_release", "target_releases")
    }
    return files, trees


def _snapshot(files: dict[str, Path], trees: dict[str, Path]) -> dict[str, str]:
    return {
        **{name: file_digest(path)[0] for name, path in files.items()},
        **{
            name: "sha256:" + native_file_decision._release_tree_sha(path)
            for name, path in trees.items()
        },
    }


def _flags(plan: dict, manifest_path: Path, family: dict, socket_path: str) -> list[str]:
    suite = evaluator.read_json((manifest_path.parent / family["suite"]).resolve())
    provenance = suite.get("source_provenance") if isinstance(suite, dict) else None
    if not isinstance(provenance, dict) or provenance.get("kind") != "composite_git_releases_v1":
        raise EvidenceError("Native capture requires composite source provenance")
    output = (manifest_path.parent / family["capture"]).resolve()
    return [
        "--query-pack",
        str((manifest_path.parent / family["query_pack"]).resolve()),
        "--source-universe-sha256",
        provenance["source_universe_sha256"],
        "--source-binding-sha256",
        provenance["source_binding_sha256"],
        "--native-socket",
        socket_path,
        "--view-json",
        plan["view_json"],
        "--open-key",
        plan["open_key"],
        "--text-instance",
        plan["text_instance"],
        "--semantic-instance",
        plan["semantic_instance"],
        "--file-instance",
        plan["file_instance"],
        "--lane-top-k",
        str(plan["lane_top_k"]),
        "--file-top-k",
        str(plan["file_top_k"]),
        "--io-timeout-secs",
        "60",
        "--daemon-binary",
        plan["daemon_binary"],
        "--daemon-config",
        plan["daemon_config"],
        "--decision-policy",
        plan["decision_policy"],
        "--out",
        str(output),
    ]


def _replay_family(manifest_path: Path, family: dict, capture_path: Path) -> dict:
    def path(key: str) -> Path:
        return (manifest_path.parent / family[key]).resolve()

    def release(key: str) -> Path:
        return (manifest_path.parent / family[key]).absolute()

    return evaluator.load_native_file_replay(
        path("repo"),
        path("suite"),
        path("query_pack"),
        capture_path,
        (
            path("source_universe"),
            path("source_binding"),
            release("base_release"),
            release("target_releases"),
            family["family_id"],
        ),
    )


def replay_run(store: RunStore, evidence: dict) -> str:
    """Recheck controlled execution and return the replayed capture commitment."""
    from registry import load_registry

    require_registered_owner(load_registry())
    if (
        evidence["profile"] != PROFILE
        or evidence["family"] != FAMILY
        or evidence["verdict"]["scope"] != "contract"
    ):
        raise EvidenceError("Native capture evidence belongs to another rail")
    if evidence["source"] != source_identity(ROOT, "benchmark-retrieval"):
        raise EvidenceError("Native capture code source changed")
    if evidence["build"]["lockfile_digest"] != file_digest(ROOT / "Cargo.lock")[0]:
        raise EvidenceError("Native capture lockfile changed")
    raw = store.run_dir(evidence["run_id"]) / "raw"
    plan_path = raw / "plan.json"
    plan = _plan(plan_path)
    manifest_path, family = _family(plan)
    if evidence["case_id"] != family["family_id"]:
        raise EvidenceError("Native capture family identity differs")
    files, trees = _roles(plan_path, plan, manifest_path, family)
    current = _snapshot(files, trees)
    inputs = {
        row["id"]: row["digest"] for row in evidence["inputs"] if row["availability"] == "present"
    }
    if set(inputs) != set(current) or any(inputs[key] != value for key, value in current.items()):
        raise EvidenceError("Native capture source inputs changed after execution")
    if RawFile.capture(raw / "cohort.json").sha256 != current["cohort"]:
        raise EvidenceError("Native captured cohort differs from current cohort")
    for key in (
        "decision_policy",
        "daemon_config",
        "suite",
        "query_pack",
        "source_universe",
        "source_binding",
    ):
        if RawFile.capture(raw / f"{key}.json").sha256 != current[key]:
            raise EvidenceError(f"Native captured {key} differs from frozen input")
    if evidence["build"]["binaries"] != [
        {"name": key, "sha256": current[key]} for key in ("daemon_binary", "runner_binary")
    ]:
        raise EvidenceError("Native captured binary inventory differs")
    config = evaluator.read_json(Path(plan["daemon_config"]))
    if not isinstance(config, dict) or not isinstance(config.get("socket_path"), str):
        raise EvidenceError("Native daemon socket config is malformed")
    expected_argv = [
        plan["runner_binary"],
        "native-file-profile",
        *_flags(plan, manifest_path, family, config["socket_path"]),
    ]
    execution = parse_json(read_control(raw / "execution.json").decode("utf-8"))
    if (
        not isinstance(execution, dict)
        or execution.get("status") != "completed"
        or execution.get("command") != evidence["command"]
        or execution["command"].get("argv") != expected_argv
        or execution["command"].get("exit_code") != 0
        or execution.get("request", {}).get("argv") != expected_argv
        or execution["request"].get("cwd") != evidence["command"]["cwd"]
        or execution["request"].get("timeout_seconds") != evidence["command"]["timeout_seconds"]
        or execution.get("error_type") is not None
        or execution.get("output_errors") != []
        or not isinstance(execution.get("raw"), list)
        or len(execution["raw"]) != 2
    ):
        raise EvidenceError("Native capture controlled execution differs")
    for name in ("stdout", "stderr"):
        refs = [row for row in execution["raw"] if Path(row["path"]).name == name]
        observed = RawFile.capture(raw / name)
        if (
            len(refs) != 1
            or refs[0]["sha256"] != observed.sha256
            or refs[0]["bytes"] != observed.size
        ):
            raise EvidenceError("Native capture producer stream differs")
    native = RawFile.capture(raw / "native-capture.json")
    if file_digest((manifest_path.parent / family["capture"]).resolve())[0] != native.sha256:
        raise EvidenceError("Native capture output changed after controlled execution")
    replay = _replay_family(manifest_path, family, raw / "native-capture.json")
    native_file_decision._check_runtime(
        replay["capture"],
        daemon_sha=current["daemon_binary"].removeprefix("sha256:"),
        config_sha=current["daemon_config"].removeprefix("sha256:"),
        runner_sha=current["runner_binary"].removeprefix("sha256:"),
        policy_sha=current["decision_policy"].removeprefix("sha256:"),
        config_path=Path(plan["daemon_config"]).resolve(),
        socket=config["socket_path"],
    )
    if evidence["payload"] != {
        "kind": "proof",
        "rail": "native-file-sdk-execution",
        "selected": 1,
        "executed": 1,
        "passed": 1,
        "failed": 0,
        "source_digest": evidence["source"]["closure_digest"],
        "execution_context_digest": RawFile.capture(plan_path).sha256,
    }:
        raise EvidenceError("Native execution proof payload differs")
    return evaluator.digest(evaluator.canonical(replay["capture"]))


@capture_entrypoint(PROFILE)
def capture(repo: Path, root: Path, registry: dict, timeout: int, plan_path: Path) -> dict:
    """Capture one predeclared Native family; product qualification stays separate."""
    from benchctl import require_clean_worktree, require_frozen_source, resolve_checkout_head

    if repo.resolve() != ROOT:
        raise EvidenceError("Native capture driver must come from the requested checkout")
    require_registered_owner(registry)
    _directories(root)
    require_clean_worktree(repo)
    head = resolve_checkout_head(repo)
    source = source_identity(repo, "benchmark-retrieval")
    plan_path = plan_path.resolve()
    plan = _plan(plan_path)
    manifest_path, family = _family(plan)
    files, trees = _roles(plan_path, plan, manifest_path, family)
    initial = _snapshot(files, trees)
    manifest_raw, manifest = evaluator._native_cohort_manifest(manifest_path)
    native_file_decision._policy(
        Path(plan["decision_policy"]),
        hashlib.sha256(manifest_raw).hexdigest(),
        manifest,
    )
    config = evaluator.read_json(Path(plan["daemon_config"]))
    if (
        not isinstance(config, dict)
        or not isinstance(config.get("socket_path"), str)
        or not Path(config["socket_path"]).is_absolute()
    ):
        raise EvidenceError("Native daemon socket config is malformed")
    output = (manifest_path.parent / family["capture"]).resolve()
    if output.is_relative_to(repo.resolve()):
        raise EvidenceError("Native capture output must be external to source checkout")
    flags = _flags(plan, manifest_path, family, config["socket_path"])
    epoch = current_capture()
    result = execute_owned_native(
        Path(plan["runner_binary"]),
        flags,
        cwd=repo,
        log_dir=epoch.work / "native-execution",
        timeout=timeout,
        epoch=epoch,
    )
    if _snapshot(files, trees) != initial:
        raise EvidenceError("Native frozen inputs changed during SDK execution")
    replay = _replay_family(manifest_path, family, output)
    native_file_decision._check_runtime(
        replay["capture"],
        daemon_sha=initial["daemon_binary"].removeprefix("sha256:"),
        config_sha=initial["daemon_config"].removeprefix("sha256:"),
        runner_sha=initial["runner_binary"].removeprefix("sha256:"),
        policy_sha=initial["decision_policy"].removeprefix("sha256:"),
        config_path=Path(plan["daemon_config"]).resolve(),
        socket=config["socket_path"],
    )
    require_frozen_source(repo, head)
    host = host_identity(
        policy="any",
        os_name=platform.system().lower(),
        arch=platform.machine(),
        cpu_count=os.cpu_count() or 1,
        hostname=socket.gethostname(),
        lease_mode="none",
        lease_samples=0,
    )
    raw = {
        "plan.json": RawFile.capture(plan_path),
        "cohort.json": RawFile.capture(manifest_path),
        **{
            f"{key}.json": RawFile.capture(files[key])
            for key in (
                "decision_policy",
                "daemon_config",
                "suite",
                "query_pack",
                "source_universe",
                "source_binding",
            )
        },
        "native-capture.json": result["capture"],
        "execution.json": result["execution"],
        "stdout": result["stdout"],
        "stderr": result["stderr"],
    }
    if (
        raw["plan.json"].sha256 != initial["plan"]
        or raw["cohort.json"].sha256 != initial["cohort"]
        or any(
            raw[f"{key}.json"].sha256 != initial[key]
            for key in (
                "decision_policy",
                "daemon_config",
                "suite",
                "query_pack",
                "source_universe",
                "source_binding",
            )
        )
        or _snapshot(files, trees) != initial
    ):
        raise EvidenceError("Native frozen input changed before raw promotion")
    payload = {
        "kind": "proof",
        "rail": "native-file-sdk-execution",
        "selected": 1,
        "executed": 1,
        "passed": 1,
        "failed": 0,
        "source_digest": source["closure_digest"],
        "execution_context_digest": raw["plan.json"].sha256,
    }
    record = dict(
        run_id=f"{epoch.capture_id}-{FAMILY}",
        family=FAMILY,
        profile=PROFILE,
        case_id=plan["family_id"],
        created_utc=datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        raw_files=raw,
        payload=payload,
        source=source,
        build={
            "toolchain": "unqualified",
            "target_triple": platform.machine(),
            "lockfile_digest": file_digest(repo / "Cargo.lock")[0],
            "profile": "native-file-capture",
            "flags": [],
            "binaries": [
                {"name": key, "sha256": initial[key]} for key in ("daemon_binary", "runner_binary")
            ],
        },
        inputs=[
            {"id": key, "availability": "present", "digest": value, "reason": None}
            for key, value in sorted(initial.items())
        ],
        host=host,
        command=result["command"],
        boundary={
            "clock": "monotonic",
            "instrumentation": "owned-producer",
            "start_event": "native_sdk_start",
            "end_event": "capture_replay_complete",
        },
        verdict={"scope": "contract", "status": "pass", "reason": None, "metrics": []},
    )
    return publish_capture(
        root,
        capture_id=epoch.capture_id,
        profile=PROFILE,
        registry_digest=registry_digest(registry),
        expected_cases={FAMILY: [plan["family_id"]]},
        runs=[record],
        replay=replay_run,
        verify_source=lambda: require_frozen_source(repo, head),
    )


def validate(repo: Path, root: Path, registry: dict) -> dict:
    if repo.resolve() != ROOT:
        raise EvidenceError("Native capture driver must come from the requested checkout")
    require_registered_owner(registry)
    document = load_capture(root, profile=PROFILE, registry_digest=registry_digest(registry))
    if document["source"] != source_identity(repo, "benchmark-retrieval"):
        raise EvidenceError("Native capture code source changed")
    store = RunStore(root)
    for row in document["runs"]:
        evidence = store.load(row["run_id"])
        if evidence["build"]["lockfile_digest"] != file_digest(repo / "Cargo.lock")[0]:
            raise EvidenceError("Native capture lockfile changed")
        replay_run(store, evidence)
    return document
