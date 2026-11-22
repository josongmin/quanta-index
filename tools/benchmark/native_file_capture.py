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
from collections.abc import Callable
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

from evidence import (
    EvidenceError,
    RawFile,
    RunStore,
    canonical_json,
    digest_bytes,
    file_digest,
    parse_json,
    read_control,
)
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

from tools.benchmark.retrieval import evaluator, native_file_decision, native_file_paths
from tools.benchmark.retrieval.native_file_paths import resolve_path
from tools.deployment import native_release_approval

PROFILE = "native-file-capture"
FAMILY = "native-file-sdk-capture"
ROOT = Path(__file__).resolve().parents[2]
SDK_TIMEOUT_SECONDS = 60
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
SELECTION_KEYS = {"selection_path", "selection_sha256"}
SIGNED_INSTALL = "signed_install_approval"
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
    if type(command.get("exit_code")) is not int or command["exit_code"] != 0:
        raise EvidenceError("Native runner lacks successful terminal status")
    return {
        "capture": RawFile.capture(output),
        "execution": RawFile.capture(log_dir / "execution.json"),
        "stdout": stdout,
        "stderr": stderr,
        "command": command,
    }


def _plan(path: Path | RawFile, *, require_signed_install: bool = False) -> dict:
    value = parse_json(read_control(path).decode("utf-8"))
    if not isinstance(value, dict) or type(value.get("schema_version")) is not int:
        raise EvidenceError("Native execution plan shape differs")
    version = value["schema_version"]
    allowed = {
        1: (PLAN_KEYS, PLAN_KEYS | SELECTION_KEYS),
        2: (PLAN_KEYS | SELECTION_KEYS | {SIGNED_INSTALL},),
    }
    if set(value) not in allowed.get(version, ()):
        raise EvidenceError("Native execution plan shape differs")
    if require_signed_install and version != 2:
        raise EvidenceError("Native signed installation requires an approved v2 execution plan")
    for key in (
        "cohort_manifest",
        "decision_policy",
        "daemon_binary",
        "daemon_config",
        "runner_binary",
    ):
        if not isinstance(value[key], str) or not Path(value[key]).is_absolute():
            raise EvidenceError(f"Native execution {key} must be absolute")
        value[key] = str(resolve_path(Path(value[key])))
    if SELECTION_KEYS <= set(value):
        selected_path = value["selection_path"]
        selected_sha = value["selection_sha256"]
        if not isinstance(selected_path, str) or not Path(selected_path).is_absolute():
            raise EvidenceError("Native selection path must be absolute")
        if not isinstance(selected_sha, str) or HEX64.fullmatch(selected_sha) is None:
            raise EvidenceError("Native selection digest is malformed")
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


def _check_plan_family_runtime(manifest_path: Path, family: dict, plan: dict) -> None:
    """Bind a v2 archived plan to its family-owned runtime inputs."""
    if "daemon_config" not in family:
        return
    config_path, _config_sha, selected = native_file_decision._cohort_runtime(
        manifest_path, family, None
    )
    planned_config = resolve_path(Path(plan["daemon_config"]))
    if planned_config != config_path:
        raise EvidenceError("Native execution plan daemon config differs from cohort family")
    planned_selection = (
        (Path(plan["selection_path"]).absolute(), plan["selection_sha256"])
        if "selection_path" in plan else None
    )
    if planned_selection != selected:
        raise EvidenceError("Native execution plan selection differs from cohort family")


def _check_frozen_family_snapshot(family: dict, snapshot: dict[str, str]) -> None:
    """Catch input replacement between plan validation and the managed snapshot."""
    if "daemon_config" not in family:
        return
    if snapshot.get("daemon_config") != "sha256:" + family["daemon_config_sha256"]:
        raise EvidenceError("Native execution daemon config differs from frozen cohort family")
    if "selection_path" in family and snapshot.get("selection") != "sha256:" + family["selection_sha256"]:
        raise EvidenceError("Native execution selection differs from frozen cohort family")


def _family(plan: dict, *, expected_cohort: str | None = None) -> tuple[Path, dict, dict, str]:
    manifest_path = resolve_path(Path(plan["cohort_manifest"]))
    raw, manifest = evaluator._native_cohort_manifest(manifest_path)
    cohort_sha = digest_bytes(raw)
    if expected_cohort is not None and cohort_sha != expected_cohort:
        raise EvidenceError("Native cohort bytes differ from sealed raw")
    matches = [row for row in manifest["families"] if row["family_id"] == plan["family_id"]]
    if len(matches) != 1:
        raise EvidenceError("Native execution family is absent or repeated")
    _check_plan_family_runtime(manifest_path, matches[0], plan)
    return manifest_path, manifest, matches[0], cohort_sha


@dataclass(frozen=True)
class _CapturePaths:
    files: dict[str, Path]
    releases: dict[str, Path]
    source_repo: Path


def _roles(plan_path: Path, plan: dict, manifest_path: Path, family: dict) -> _CapturePaths:
    files = {"plan": plan_path, "cohort": manifest_path}
    files.update(
        {
            key: Path(plan[key])
            for key in ("decision_policy", "daemon_binary", "daemon_config", "runner_binary")
        }
    )
    if "selection_path" in plan:
        files["selection"] = Path(plan["selection_path"])
    files.update(
        {
            key: resolve_path(manifest_path.parent / family[key])
            for key in ("suite", "query_pack", "source_universe", "source_binding")
        }
    )
    releases = {
        key: (manifest_path.parent / family[key]).absolute()
        for key in native_file_paths.RELEASE_ROOTS
    }
    return _CapturePaths(files, releases, resolve_path(manifest_path.parent / family["repo"]))


def _verify_signed_install(plan, selected, policy_sha256):
    if SIGNED_INSTALL not in plan:
        return
    if selected is None:
        raise EvidenceError("Native signed installation lacks its selected BOOT")
    try:
        native_release_approval.verify_installed_approval(
            plan[SIGNED_INSTALL], selected, plan["selection_sha256"], policy_sha256,
        )
    except (OSError, KeyError, TypeError, ValueError) as error:
        raise EvidenceError(f"Native signed installation differs: {error}") from error


def _check_capture_paths(
    repo: Path,
    root: Path,
    manifest_path: Path,
    manifest: dict,
    family: dict,
    files: dict[str, Path],
    *,
    fresh: bool = True,
) -> Path:
    """Protect all cohort inputs, including those of families captured later."""
    declared = manifest_path.parent / family["capture"]
    if not declared.is_absolute():
        raise EvidenceError("Native capture output must be a fresh absolute path")
    output = resolve_path(declared.parent) / declared.name
    if fresh and (output.is_symlink() or output.exists()):
        raise EvidenceError("Native capture output must be a fresh absolute path")
    if output.is_symlink():
        raise EvidenceError("Native capture output changed after controlled execution")
    native_file_paths.require_disjoint(
        (output,), (("source checkout", repo),),
        error="Native capture output must be external to source checkout",
    )
    protected = (
        *files.items(),
        *native_file_paths.cohort_inputs(
            manifest_path, manifest, publishing_family=family["family_id"],
        ),
    )
    native_file_paths.require_disjoint(
        native_file_paths.custody_paths(root),
        (*protected, ("source checkout", repo), ("capture", output)),
        error="Native {role} overlaps benchmark custody paths",
    )
    native_file_paths.require_disjoint(
        (output,), protected, error="Native capture output overlaps {role}",
    )
    return output


def _snapshot(files: dict[str, Path], trees: dict[str, Path]) -> dict[str, str]:
    return {
        **{
            name: (
                "sha256:"
                + hashlib.sha256(native_file_decision.selected_file_bytes(path)).hexdigest()
                if name == "selection"
                else file_digest(path)[0]
            )
            for name, path in files.items()
        },
        **{
            name: "sha256:" + native_file_decision._release_tree_sha(path)
            for name, path in trees.items()
        },
    }


def _inputs_unchanged(
    plan_file: RawFile,
    family: dict,
    paths: _CapturePaths,
    commitments: dict[str, str],
) -> bool:
    """Check frozen bytes before re-resolving the declared locations."""
    if _snapshot(paths.files, paths.releases) != commitments:
        return False
    plan = _plan(plan_file)
    manifest_path = Path(plan["cohort_manifest"])
    _check_plan_family_runtime(manifest_path, family, plan)
    return _roles(plan_file.path, plan, manifest_path, family) == paths


def _flags(
    plan: dict,
    files: dict[str, Path],
    socket_path: str,
    *,
    output: Path,
    expected_suite: tuple[str, int],
) -> list[str]:
    suite = evaluator.read_json_with_bytes(
        files["suite"],
        expected_bytes=expected_suite,
    )[1]
    provenance = suite.get("source_provenance") if isinstance(suite, dict) else None
    if not isinstance(provenance, dict) or provenance.get("kind") != "composite_git_releases_v1":
        raise EvidenceError("Native capture requires composite source provenance")
    return [
        "--query-pack",
        str(files["query_pack"]),
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
        str(SDK_TIMEOUT_SECONDS),
        "--daemon-binary",
        plan["daemon_binary"],
        "--daemon-config",
        plan["daemon_config"],
        "--decision-policy",
        plan["decision_policy"],
        "--out",
        str(output),
    ]


def _replay_family(
    paths: _CapturePaths,
    family_id: str,
    captured: RawFile,
    *,
    release_validator: Callable[[Path], dict] | None = None,
    expected_inputs: dict[str, RawFile] | None = None,
) -> dict:
    frozen = (
        {
            f"expected_{name}": (expected_inputs[name].sha256, expected_inputs[name].size)
            for name in ("suite", "query_pack", "source_universe", "source_binding")
        }
        if expected_inputs is not None
        else {}
    )
    return evaluator.load_native_file_execution(
        paths.source_repo,
        paths.files["suite"],
        paths.files["query_pack"],
        captured.path,
        (
            paths.files["source_universe"],
            paths.files["source_binding"],
            paths.releases["base_release"],
            paths.releases["target_releases"],
            family_id,
        ),
        expected_capture=(captured.sha256, captured.size),
        release_validator=release_validator,
        **frozen,
    )


def _archived_raw(store: RunStore, evidence: dict, name: str) -> RawFile:
    references = [row for row in evidence["raw"] if row["path"] == f"raw/{name}"]
    if len(references) != 1:
        raise EvidenceError(f"Native raw file lacks a unique commitment: {name}")
    reference = references[0]
    return RawFile(
        (store.run_dir(evidence["run_id"]) / reference["path"]).absolute(),
        reference["sha256"],
        reference["bytes"],
    )


def _check_live_output(path: Path, captured: RawFile) -> None:
    if resolve_path(path) != captured.path or file_digest(captured.path) != (
        captured.sha256, captured.size,
    ):
        raise EvidenceError("Native capture output changed after controlled execution")


def _check_plan_capture(plan: dict, capture: dict) -> None:
    """Join the validated SDK output to the request that produced it."""
    expected = {
        "view_json": parse_json(plan["view_json"]),
        "text_instance": list(bytes.fromhex(plan["text_instance"])),
        "semantic_instance": list(bytes.fromhex(plan["semantic_instance"])),
        "file_instance": list(bytes.fromhex(plan["file_instance"])),
        "lane_top_k": plan["lane_top_k"],
        "file_top_k": plan["file_top_k"],
        "query_deadline_seconds": SDK_TIMEOUT_SECONDS,
    }
    observed = {
        "view_json": capture["view"],
        "text_instance": capture["text_instance"],
        "semantic_instance": capture["semantic_instance"],
        "file_instance": capture["indexed_universe"]["file_instance"],
        "lane_top_k": capture["lane_top_k"],
        "file_top_k": capture["file_top_k"],
        "query_deadline_seconds": capture["query_deadline_seconds"],
    }
    for key, value in expected.items():
        if evaluator.canonical(observed[key]) != evaluator.canonical(value):
            raise EvidenceError(f"Native capture differs from execution plan: {key}")


def load_published_run(store: RunStore, run_id: str, *, registered_digest: str) -> dict:
    """Load the run pinned by its original Native capture publication."""
    suffix = "-" + FAMILY
    if not run_id.endswith(suffix):
        raise EvidenceError("Native controlled run ID has the wrong family")
    document = load_capture(
        store.root, profile=PROFILE, registry_digest=registered_digest,
        capture_id=run_id[:-len(suffix)],
    )
    if [row["run_id"] for row in document["runs"]] != [run_id]:
        raise EvidenceError("Native controlled run lacks sole original capture membership")
    return store.load(run_id, expected_digest=document["runs"][0]["digest"])


def replay_run(
    store: RunStore,
    evidence: dict,
    *,
    release_validator: Callable[[Path], dict] | None = None,
    require_signed_install: bool = False,
) -> str:
    """Recheck controlled execution and return the replayed capture commitment."""
    from registry import load_registry

    require_registered_owner(load_registry())
    if (
        evidence["profile"] != PROFILE
        or evidence["family"] != FAMILY
        or evidence["verdict"]["scope"] != "contract"
        or evidence["verdict"]["status"] != "pass"
    ):
        raise EvidenceError("Native capture evidence is not a passing contract run")
    if evidence["source"] != source_identity(ROOT, "benchmark-retrieval"):
        raise EvidenceError("Native capture code source changed")
    if evidence["build"]["lockfile_digest"] != file_digest(ROOT / "Cargo.lock")[0]:
        raise EvidenceError("Native capture lockfile changed")
    plan_file = _archived_raw(store, evidence, "plan.json")
    plan = _plan(plan_file, require_signed_install=require_signed_install)
    cohort_ref = _archived_raw(store, evidence, "cohort.json")
    manifest_path, manifest, family, cohort_sha = _family(
        plan, expected_cohort=cohort_ref.sha256,
    )
    if evidence["case_id"] != family["family_id"]:
        raise EvidenceError("Native capture family identity differs")
    paths = _roles(plan_file.path, plan, manifest_path, family)
    files = paths.files
    current = _snapshot(files, paths.releases)
    _check_frozen_family_snapshot(family, current)
    selected = None
    if "selection_path" in plan:
        _selection_raw, selected = native_file_decision.load_selected_snapshot(
            files["selection"], plan["selection_sha256"]
        )
    inputs = {
        row["id"]: row["digest"] for row in evidence["inputs"] if row["availability"] == "present"
    }
    if set(inputs) != set(current) or any(inputs[key] != value for key, value in current.items()):
        raise EvidenceError("Native capture source inputs changed after execution")
    archived_inputs = {
        key: _archived_raw(store, evidence, f"{key}.json")
        for key in (
            "cohort",
            "decision_policy",
            "daemon_config",
            "suite",
            "query_pack",
            "source_universe",
            "source_binding",
        )
    }
    for key, archived in archived_inputs.items():
        if (
            file_digest(archived.path) != (archived.sha256, archived.size)
            or archived.sha256 != current[key]
        ):
            raise EvidenceError(f"Native archived {key} differs from sealed raw or frozen input")
    native_file_decision._policy(
        archived_inputs["decision_policy"].read_control(),
        cohort_sha.removeprefix("sha256:"),
        manifest,
    )
    if selected is not None:
        archived = _archived_raw(store, evidence, "selection.json")
        selected_bytes = native_file_decision.selected_file_bytes(archived.path)
        selected_commitment = (
            "sha256:" + hashlib.sha256(selected_bytes).hexdigest(),
            len(selected_bytes),
        )
        if (
            selected_commitment != (archived.sha256, archived.size)
            or archived.sha256 != current["selection"]
        ):
            raise EvidenceError("Native archived selection differs from sealed raw or frozen input")
    if evidence["build"]["binaries"] != [
        {"name": key, "sha256": current[key]} for key in ("daemon_binary", "runner_binary")
    ]:
        raise EvidenceError("Native captured binary inventory differs")
    config_bytes = read_control(Path(plan["daemon_config"]))
    if digest_bytes(config_bytes) != current["daemon_config"]:
        raise EvidenceError("Native daemon config changed during replay")
    config = parse_json(config_bytes.decode("utf-8"))
    if not isinstance(config, dict) or not isinstance(config.get("socket_path"), str):
        raise EvidenceError("Native daemon socket config is malformed")
    live_output = manifest_path.parent / family["capture"]
    # The replay plan lives in the owned store. All declared inputs must still
    # be disjoint from that store and its external capture.
    external_files = {key: path for key, path in files.items() if key != "plan"}
    output = _check_capture_paths(
        ROOT, store.root, manifest_path, manifest, family, external_files, fresh=False,
    )
    expected_argv = [
        plan["runner_binary"],
        "native-file-profile",
        *_flags(
            plan,
            files,
            config["socket_path"],
            output=output,
            expected_suite=(archived_inputs["suite"].sha256, archived_inputs["suite"].size),
        ),
    ]
    execution = parse_json(
        _archived_raw(store, evidence, "execution.json").read_control().decode("utf-8")
    )
    if not isinstance(execution, dict) or set(execution) != {
        "status", "command", "request", "error_type", "output_errors", "raw",
    }:
        raise EvidenceError("Native capture controlled execution shape differs")
    command, request, streams = execution["command"], execution["request"], execution["raw"]
    recorded_command = evidence["command"]
    expected_request = {
        "argv": expected_argv,
        "cwd": recorded_command["cwd"],
        "timeout_seconds": recorded_command["timeout_seconds"],
    }
    if (
        execution["status"] != "completed"
        or not isinstance(command, dict)
        or command.get("status") != "completed"
        or type(command.get("exit_code")) is not int
        or command["exit_code"] != 0
        or command.get("argv") != expected_argv
        or command != recorded_command
        or canonical_json(command) != canonical_json(recorded_command)
        or request != expected_request
        or type(request["timeout_seconds"]) is not int
        or execution["error_type"] is not None
        or execution["output_errors"] != []
        or not isinstance(streams, list)
        or len(streams) != 2
        or any(
            not isinstance(row, dict)
            or set(row) != {"path", "sha256", "bytes"}
            or not isinstance(row["path"], str)
            or not Path(row["path"]).is_absolute()
            or not isinstance(row["sha256"], str)
            or type(row["bytes"]) is not int
            or row["bytes"] < 0
            for row in streams
        )
    ):
        raise EvidenceError("Native capture controlled execution differs")
    for name in ("stdout", "stderr"):
        refs = [row for row in streams if Path(row["path"]).name == name]
        observed = _archived_raw(store, evidence, name)
        if (
            len(refs) != 1
            or refs[0]["sha256"] != observed.sha256
            or refs[0]["bytes"] != observed.size
            or file_digest(observed.path) != (observed.sha256, observed.size)
        ):
            raise EvidenceError("Native capture producer stream differs")
    native = _archived_raw(store, evidence, "native-capture.json")
    output_ref = RawFile(output, native.sha256, native.size)
    _check_live_output(live_output, output_ref)
    observed = _replay_family(
        paths,
        family["family_id"],
        native,
        release_validator=release_validator,
        expected_inputs=archived_inputs,
    )
    _check_plan_capture(plan, observed)
    native_file_decision._check_runtime(
        observed,
        daemon_sha=current["daemon_binary"].removeprefix("sha256:"),
        config_sha=current["daemon_config"].removeprefix("sha256:"),
        runner_sha=current["runner_binary"].removeprefix("sha256:"),
        policy_sha=current["decision_policy"].removeprefix("sha256:"),
        config_path=files["daemon_config"],
        socket=config["socket_path"],
        selected=selected,
        daemon_path=Path(plan["daemon_binary"]),
        require_native_selection="daemon_config" in family,
    )
    if not _inputs_unchanged(plan_file, family, paths, current):
        raise EvidenceError("Native capture source inputs changed during replay")
    _check_capture_paths(
        ROOT, store.root, manifest_path, manifest, family, external_files, fresh=False,
    )
    _check_live_output(live_output, output_ref)
    if evidence["payload"] != {
        "kind": "proof",
        "rail": "native-file-sdk-execution",
        "selected": 1,
        "executed": 1,
        "passed": 1,
        "failed": 0,
        "source_digest": evidence["source"]["closure_digest"],
        "execution_context_digest": plan_file.sha256,
    }:
        raise EvidenceError("Native execution proof payload differs")
    commitment = evaluator.digest(evaluator.canonical(observed))
    evaluator.verify_repo(paths.source_repo, observed["repository_commit"])
    if selected is not None:
        _, selected = native_file_decision.load_selected_snapshot(
            files["selection"], plan["selection_sha256"],
        )
    _verify_signed_install(plan, selected, current["decision_policy"].removeprefix("sha256:"))
    return commitment


def capture(
    repo: Path, root: Path, registry: dict, timeout: int, plan_path: Path,
    *, require_signed_install: bool = False,
) -> dict:
    """Capture one predeclared Native family; product qualification stays separate."""
    if resolve_path(repo) != ROOT:
        raise EvidenceError("Native capture driver must come from the requested checkout")
    require_registered_owner(registry)
    _directories(root)
    prepared_plan = RawFile.capture(resolve_path(plan_path))
    plan = _plan(prepared_plan, require_signed_install=require_signed_install)
    manifest_path, manifest, family, cohort_sha = _family(plan)
    paths = _roles(prepared_plan.path, plan, manifest_path, family)
    prepared_output = _check_capture_paths(repo, root, manifest_path, manifest, family, paths.files)
    return _capture_admitted(
        repo, root, registry, timeout, prepared_plan, cohort_sha, paths, prepared_output,
    )


@capture_entrypoint(PROFILE)
def _capture_admitted(
    repo: Path,
    root: Path,
    registry: dict,
    timeout: int,
    prepared_plan: RawFile,
    cohort_sha: str,
    prepared_paths: _CapturePaths,
    prepared_output: Path,
) -> dict:
    from benchctl import require_clean_worktree, require_frozen_source, resolve_checkout_head

    require_clean_worktree(repo)
    head = resolve_checkout_head(repo)
    source = source_identity(repo, "benchmark-retrieval")
    plan_path = prepared_plan.path
    plan = _plan(prepared_plan)
    manifest_path, manifest, family, _ = _family(plan, expected_cohort=cohort_sha)
    paths = _roles(plan_path, plan, manifest_path, family)
    if paths != prepared_paths:
        raise EvidenceError("Native input paths changed during capture preparation")
    files = paths.files
    live_output = manifest_path.parent / family["capture"]
    output = _check_capture_paths(repo, root, manifest_path, manifest, family, files)
    if output != prepared_output:
        raise EvidenceError("Native capture output path changed during capture preparation")
    initial = _snapshot(files, paths.releases)
    _check_frozen_family_snapshot(family, initial)
    if prepared_plan.sha256 != initial["plan"] or cohort_sha != initial["cohort"]:
        raise EvidenceError("Native execution plan changed during capture preparation")
    selected = None
    if "selection_path" in plan:
        _selection_raw, selected = native_file_decision.load_selected_snapshot(
            files["selection"], plan["selection_sha256"]
        )
        native_file_decision.check_selected_inputs(
            selected,
            daemon_path=files["daemon_binary"],
            daemon_sha=initial["daemon_binary"].removeprefix("sha256:"),
            config_path=str(files["daemon_config"]),
            config_sha=initial["daemon_config"].removeprefix("sha256:"),
        )
    _, policy_sha = native_file_decision._policy(
        Path(plan["decision_policy"]),
        cohort_sha.removeprefix("sha256:"),
        manifest,
    )
    _verify_signed_install(plan, selected, policy_sha)
    if "sha256:" + policy_sha != initial["decision_policy"]:
        raise EvidenceError("Native policy changed during capture preparation")
    config_bytes = read_control(Path(plan["daemon_config"]))
    if digest_bytes(config_bytes) != initial["daemon_config"]:
        raise EvidenceError("Native daemon config changed during capture preparation")
    config = parse_json(config_bytes.decode("utf-8"))
    if (
        not isinstance(config, dict)
        or not isinstance(config.get("socket_path"), str)
        or not Path(config["socket_path"]).is_absolute()
    ):
        raise EvidenceError("Native daemon socket config is malformed")
    suite_ref = RawFile.capture(files["suite"])
    if suite_ref.sha256 != initial["suite"]:
        raise EvidenceError("Native suite changed during capture preparation")
    flags = _flags(
        plan,
        files,
        config["socket_path"],
        output=output,
        expected_suite=(suite_ref.sha256, suite_ref.size),
    )
    epoch = current_capture()
    result = execute_owned_native(
        Path(plan["runner_binary"]),
        flags,
        cwd=repo,
        log_dir=epoch.work / "native-execution",
        timeout=timeout,
        epoch=epoch,
    )
    if not _inputs_unchanged(prepared_plan, family, paths, initial):
        raise EvidenceError("Native frozen inputs changed during SDK execution")
    if selected is not None:
        _selection_raw, after = native_file_decision.load_selected_snapshot(
            files["selection"], plan["selection_sha256"]
        )
        if after != selected:
            raise EvidenceError("Native selection changed during SDK execution")
    replay_inputs = {
        "suite": suite_ref,
        **{
            key: RawFile.capture(files[key])
            for key in ("query_pack", "source_universe", "source_binding")
        },
    }
    if any(replay_inputs[key].sha256 != initial[key] for key in replay_inputs):
        raise EvidenceError("Native frozen inputs changed before SDK replay")
    observed = _replay_family(
        paths, family["family_id"], result["capture"], expected_inputs=replay_inputs
    )
    _check_plan_capture(plan, observed)
    native_file_decision._check_runtime(
        observed,
        daemon_sha=initial["daemon_binary"].removeprefix("sha256:"),
        config_sha=initial["daemon_config"].removeprefix("sha256:"),
        runner_sha=initial["runner_binary"].removeprefix("sha256:"),
        policy_sha=initial["decision_policy"].removeprefix("sha256:"),
        config_path=files["daemon_config"],
        socket=config["socket_path"],
        selected=selected,
        daemon_path=Path(plan["daemon_binary"]),
        require_native_selection="daemon_config" in family,
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
    if selected is not None:
        selected_copy = epoch.work / "selected-selection.json"
        selected_copy.write_bytes(native_file_decision.selected_file_bytes(files["selection"]))
        raw["selection.json"] = RawFile.capture(selected_copy)
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
        or not _inputs_unchanged(prepared_plan, family, paths, initial)
        or (selected is not None and raw["selection.json"].sha256 != initial["selection"])
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

    def verify_source_and_inputs() -> None:
        if not _inputs_unchanged(prepared_plan, family, paths, initial):
            raise EvidenceError("Native frozen inputs changed before publication")
        _check_capture_paths(repo, root, manifest_path, manifest, family, files, fresh=False)
        _check_live_output(live_output, result["capture"])
        if SIGNED_INSTALL in plan:
            _, live_selection = native_file_decision.load_selected_snapshot(
                files["selection"], plan["selection_sha256"],
            )
            _verify_signed_install(plan, live_selection, policy_sha)
        require_frozen_source(repo, head)
        if resolve_path(manifest_path.parent / family["repo"]) != paths.source_repo:
            raise EvidenceError("Native source repository path changed before publication")
        evaluator.verify_repo(paths.source_repo, observed["repository_commit"])

    return publish_capture(
        root,
        capture_id=epoch.capture_id,
        profile=PROFILE,
        registry_digest=registry_digest(registry),
        expected_cases={FAMILY: [plan["family_id"]]},
        runs=[record],
        replay=replay_run,
        verify_source=verify_source_and_inputs,
        verify_at_commit=verify_source_and_inputs,
    )


def validate(
    repo: Path, root: Path, registry: dict, *, require_signed_install: bool = False,
) -> dict:
    if resolve_path(repo) != ROOT:
        raise EvidenceError("Native capture driver must come from the requested checkout")
    require_registered_owner(registry)
    document = load_capture(root, profile=PROFILE, registry_digest=registry_digest(registry))
    if document["source"] != source_identity(repo, "benchmark-retrieval"):
        raise EvidenceError("Native capture code source changed")
    store = RunStore(root)
    for row in document["runs"]:
        evidence = store.load(row["run_id"], expected_digest=row["digest"])
        if evidence["build"]["lockfile_digest"] != file_digest(repo / "Cargo.lock")[0]:
            raise EvidenceError("Native capture lockfile changed")
        replay_run(store, evidence, require_signed_install=require_signed_install)
    return document
