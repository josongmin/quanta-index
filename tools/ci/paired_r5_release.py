"""Issue and replay the formal P11 envelope around the paired runner's QBC captures."""

from __future__ import annotations

import hashlib
import json
import os
import re
import uuid
from collections.abc import Callable
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from tools.ci import binary_custody, p11_attestation
from tools.ci.daemon_execution_evidence import (
    R5_PROOF,
    R5_WITNESS_FILE,
    REGISTRY_PATH,
    R5Case,
    r5_cases,
    read_proof_registry,
)
from tools.ci.lint.handoff_validation import (
    _open_repo_output_parent,
    _read_repo_regular_bytes,
    _write_output_temporary,
)
from tools.ci.paired_r5_result import (
    SCHEMA,
    _qbc_modules,
    phase_command,
    validate_locator,
    validate_locator_fields,
    validate_resolution_records,
    validate_selected,
    verify_captured_daemon_witness,
    verify_pair_boundary,
)
from tools.ci.proof_execution_result import _bound_artifact_bytes
from tools.ci.proof_json import parse_proof_json
from tools.ci.proof_operational_result import (
    _checker,
    _writer,
    manifest_binding,
    observed_host_environment,
    release_inputs,
    validate_release_prerequisite,
)

PROOF_ID = R5_PROOF
INDEX = "paired-r5-result.json"
SIGNATURE = INDEX + ".sig"


def _sha(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def _canonical(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()


def _statement(index: bytes, binding: dict, start: str, end: str) -> bytes:
    return _canonical(
        {"index_sha256": _sha(index), "binding": binding, "started_at": start, "ended_at": end}
    )


def _capture_names(cases: tuple[R5Case, ...]) -> list[str]:
    return [R5_WITNESS_FILE] + [
        f"{case[0]}-{phase}-{name}"
        for case in cases
        for phase in ("list", "run")
        for name in ("completion.json", "receipt.json", "stdout.log", "stderr.log")
    ]


def _runs(directory: Path, cases: tuple[R5Case, ...]) -> list[dict[str, str]]:
    return [
        {
            "format": "nextest-jsonl",
            "inventory": str(directory / f"{case[0]}-list-stdout.log"),
            "events": str(directory / f"{case[0]}-run-stdout.log"),
        }
        for case in cases
    ]


def admit_release(
    root: Path,
    pair: Path,
    daemon: str | None,
    *,
    preflight_signer: bool = True,
    signer_socket: str | None = None,
) -> dict | None:
    """Admit the current P10 receipt and optionally check a previously pinned daemon digest."""
    registry = read_proof_registry(root)
    proofs = {p["id"]: p for p in registry["proofs"]}
    proof = proofs[PROOF_ID]
    if proof["authority_state"] == "staged":
        return None
    checker, writer = _checker(), _writer()
    findings = checker.check_registry(registry, root=root, path=root / REGISTRY_PATH)
    if findings or proof["authority_state"] != "executable":
        raise ValueError(
            "invalid paired release authority: " + "; ".join(f.message for f in findings)
        )
    environment = observed_host_environment()
    receipts = writer._resolve_dependencies(root, proof, proofs, checker)
    (receipt,) = receipts
    dep = proofs[receipt["proof_id"]]
    prev = parse_proof_json(_bound_artifact_bytes(root, receipt))
    findings = checker.check_manifest(
        prev,
        proof=dep,
        root=root,
        manifest_path=root / receipt["path"],
        bind_source=True,
        proof_by_id=proofs,
        require_current_dependencies=True,
    )
    if findings:
        raise ValueError(
            "invalid paired release prerequisite: " + "; ".join(f.message for f in findings)
        )
    binding = manifest_binding(
        {
            "source": checker.proof_source_snapshot(
                root, manifest_path=root / proof["artifact"], proof=proof, excluded_paths=(pair,)
            ),
            "source_pair": checker.paired_source_snapshot(
                pair,
                repository=proof["paired_repository"],
                dependency_lock=Path(proof["paired_dependency_lock"]),
            ),
            "daemon_binary": {
                "sha256": prev["daemon_binary"]["sha256"] if daemon is None else daemon
            },
            "state_root_format": prev["state_root_format"],
            "environment": writer._host_environment(environment),
            "inputs": release_inputs(),
            "dependency_receipts": receipts,
        }
    )
    start = datetime.now(timezone.utc).isoformat()
    validate_release_prerequisite(binding, None, prev, started_at=start)
    if preflight_signer:
        p11_attestation.sign(
            proof["attestation_public_key"],
            PROOF_ID,
            b"P11 signing preflight",
            socket=signer_socket,
        )
    return {
        "proof": proof,
        "binding": binding,
        "environment": environment,
        "binary_path": prev["daemon_binary"]["path"],
        "started_at": start,
    }


def prepare_terminal(
    root: Path,
    pair: Path,
    output: Path,
    candidate: dict,
    release: dict,
    *,
    lane: str,
    signer_socket: str | None = None,
) -> Path:
    """Make an admitted runner's original captures consumable by the normal atomic writer."""
    proof, admitted_binding = release["proof"], release["binding"]
    cases = r5_cases(proof)
    index = _read_repo_regular_bytes(output, INDEX, label="original paired runner index")
    if index != _canonical(candidate) + b"\n":
        raise ValueError("original paired runner index differs from the completed execution")
    completed, _, qbc_owner = _qbc_modules(pair)
    source_digest = (
        qbc_owner._campaign_cargo_admission_owner_v1().campaign_source_snapshot_digest_v1(pair)
    )
    directory = Path(proof["artifact"]).parent / "raw" / ("paired-" + uuid.uuid4().hex)
    terminal_path = directory / "terminal.json"
    parent = _open_repo_output_parent(root, terminal_path.as_posix(), sync_ancestors=True)
    sealed: dict[str, str] = {}
    try:

        def write(name: str, raw: bytes) -> None:
            fd = os.open(
                name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=parent
            )
            with os.fdopen(fd, "wb") as stream:
                stream.write(raw)
                stream.flush()
                os.fsync(stream.fileno())
            sealed[name] = _sha(raw)

        def publish(name: str, raw: bytes) -> None:
            temporary = _write_output_temporary(parent, name, raw)
            try:
                os.link(
                    temporary, name, src_dir_fd=parent, dst_dir_fd=parent, follow_symlinks=False
                )
                try:
                    os.fsync(parent)
                except OSError:
                    os.unlink(name, dir_fd=parent)
                    raise
            finally:
                os.unlink(temporary, dir_fd=parent)

        for name in _capture_names(cases):
            write(name, _read_repo_regular_bytes(output, name, label="original QBC capture"))

        def read(name: str) -> bytes:
            return _read_repo_regular_bytes(
                root, (directory / name).as_posix(), label="archived QBC capture"
            )

        profile = _verify_index(index, admitted_binding, read, cases=cases)
        for case, row in zip(cases, candidate["cases"], strict=True):
            for phase in ("list", "run"):
                capture = row["executions"][phase]
                custody = capture["qbc_custody"]
                original = completed.read_completed_run_receipt_at_path_v1(
                    Path(custody["receipt_path"])
                )
                prefix = f"{case[0]}-{phase}-"
                validate_locator(
                    parse_proof_json(read(prefix + "completion.json")),
                    original,
                    nonce=capture["nonce"],
                    request_sha=capture["request_env_sha256"],
                    command=phase_command(case, phase),
                    source_head=admitted_binding["source_pair"]["source"]["head"],
                    source_digest=source_digest,
                    lane=lane,
                )
                if (
                    original.run.execution_root != pair
                    or original.run.command_cwd != pair
                    or original.custody_payload_v1() != custody
                    or original.stdout != read(prefix + "stdout.log")
                    or original.stderr != read(prefix + "stderr.log")
                    or original.receipt_bytes != read(prefix + "receipt.json")
                ):
                    raise ValueError("archived QBC capture differs from the physical completed run")
        write(INDEX, index)
        verify_pair_boundary(
            root, pair, candidate, lane, archive=root / release["binary_path"], cases=cases
        )
        checker = _checker()

        def require_current_release() -> None:
            current = admit_release(root, pair, candidate["daemon_sha256"], preflight_signer=False)
            if current is None or _canonical(current["proof"]) != _canonical(proof):
                raise ValueError("paired release inputs changed before publication")
            current_binding = current["binding"]
            if (
                checker.release_source_identity(current_binding)
                != checker.release_source_identity(admitted_binding)
                or {
                    **current_binding,
                    "source": admitted_binding["source"],
                    "source_pair": admitted_binding["source_pair"],
                }
                != admitted_binding
            ):
                raise ValueError("paired release inputs changed before publication")

        require_current_release()
        binding = {
            **admitted_binding,
            "environment": {**admitted_binding["environment"], **profile},
        }
        start, end = release["started_at"], datetime.now(timezone.utc).isoformat()
        terminal = {
            "status": "passed",
            "counts": {"selected": 2, "executed": 2, "passed": 2, "failed": 0, "ignored": 0},
            "environment": {**release["environment"], **profile},
            "daemon_binary": release["binary_path"],
            "state_root_format": binding["state_root_format"],
            "inputs": release_inputs(),
            "started_at": start,
            "ended_at": end,
            "execution_result": {"schema_version": 1, "runs": _runs(directory, cases)},
            "artifacts": [
                str(directory / name) for name in [INDEX, SIGNATURE, *_capture_names(cases)]
            ],
            "source_binding": {key: binding[key] for key in ("source", "source_pair")},
        }
        os.fsync(parent)
        signature = p11_attestation.sign(
            proof["attestation_public_key"],
            PROOF_ID,
            _statement(index, binding, start, end),
            socket=signer_socket,
        )
        writer = _writer()
        # The signer can delay publication. Recheck the release and staged bytes
        # while receipt selection is serialized.
        with writer.proof_write_lock(root):
            archive = root / release["binary_path"]
            binary_custody.verify(binding["daemon_sha256"], [archive], archive=archive)
            require_current_release()
            if any(_sha(read(name)) != digest for name, digest in sealed.items()):
                raise ValueError("staged QBC evidence changed during signing")
            publish(SIGNATURE, signature)
            try:
                # A reader may treat this file as the completion marker.
                publish(terminal_path.name, _canonical(terminal) + b"\n")
            except OSError:
                os.unlink(SIGNATURE, dir_fd=parent)
                raise
    finally:
        os.close(parent)
    return root / terminal_path


def verify_release(
    root: Path, proof: dict, manifest: dict, *, cases: tuple[R5Case, ...]
) -> None:
    artifacts = {item["source_path"]: item for item in manifest["artifacts"]}
    indexes = [Path(path) for path in artifacts if Path(path).name == INDEX]
    if len(indexes) != 1:
        raise ValueError("formal P11 requires the paired runner's QBC capture index")
    directory = indexes[0].parent
    expected = {str(directory / name) for name in [INDEX, SIGNATURE, *_capture_names(cases)]}
    if set(artifacts) != expected or len(artifacts) != len(manifest["artifacts"]):
        raise ValueError("formal P11 requires the original QBC captures and producer attestation")

    def read(name: str) -> bytes:
        return _bound_artifact_bytes(root, artifacts[str(directory / name)])

    index = read(INDEX)
    binding = manifest_binding(manifest)
    p11_attestation.verify(
        proof["attestation_public_key"],
        PROOF_ID,
        _statement(index, binding, manifest["started_at"], manifest["ended_at"]),
        read(SIGNATURE),
    )
    if manifest["execution_result"]["runs"] != _runs(directory, cases):
        raise ValueError("paired manifest does not select the original QBC list/run outputs")
    profile = _verify_index(index, binding, read, cases=cases)
    if any(binding["environment"].get(k) != value for k, value in profile.items()):
        raise ValueError(
            "paired release environment differs from the QBC toolchain or selected features"
        )


def _verify_index(
    index: bytes, binding: dict, read: Callable[[str], bytes], *, cases: tuple[R5Case, ...]
) -> dict:
    record = parse_proof_json(index)
    pair = binding["source_pair"]
    if (
        not isinstance(record, dict)
        or record.get("schema_version") != SCHEMA
        or record.get("qualification") != "runner-candidate-only"
        or record.get("source_heads")
        != {"quanta": binding["source"]["head"], "semantica": pair["source"]["head"]}
        or record.get("daemon_sha256") != binding["daemon_sha256"]
    ):
        raise ValueError("paired QBC capture differs from release sources, daemon or selected runs")
    lock = pair["dependency_lock"]
    if binding["inputs"] != release_inputs():
        raise ValueError("paired release inputs differ from the signed recipe")
    validate_resolution_records(record["resolutions"], cases=cases)
    for key in ("runtime", "kernel"):
        if record["resolutions"][key]["dependency_lock"] != {
            "path": lock["path"],
            "sha256": "sha256:" + lock["sha256"],
        }:
            raise ValueError("paired QBC resolver lock differs from the release pair")
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
                daemon_sha256=binding["daemon_sha256"],
            )
    if len(source_lanes) != 1:
        raise ValueError(
            "paired QBC executions must share one lane and source snapshot and toolchain"
        )
    return {
        "toolchain": "sha256:" + toolchain,
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
    if not isinstance(source_digest, str) or not re.fullmatch(r"[0-9a-f]{64}", source_digest):
        raise ValueError("QBC receipt is missing its source snapshot")
    environment = receipt.get("artifact_env_payload")
    if not isinstance(environment, dict) or _sha(_canonical(environment)) != receipt.get(
        "artifact_env_digest"
    ):
        raise ValueError("QBC receipt environment differs from its digest")
    if not isinstance(locator, dict) or any(
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
    if not isinstance(toolchain, str) or not re.fullmatch(r"[0-9a-f]{64}", toolchain):
        raise ValueError("QBC receipt lacks its observed toolchain digest")
    return source_digest, toolchain, streams["stdout.log"]
