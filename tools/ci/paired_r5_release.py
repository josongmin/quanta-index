"""Issue and replay the formal P11 envelope around the paired runner's QBC captures."""

from __future__ import annotations

import hashlib
import json
import os
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
    _restore_prior_output,
)
from tools.ci.paired_r5_result import (
    INDEX,
    SCHEMA,
    _qbc_modules,
    phase_command,
    publish_paired_output,
    read_runner_capture,
    validate_locator,
    validate_resolution_records,
    verify_pair_boundary,
    verify_runner_capture,
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
    completed, _, qbc_owner = _qbc_modules(pair, admitted_binding["source_pair"]["source"]["head"])
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

        for name in _capture_names(cases):
            write(name, read_runner_capture(output, name))

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
            publish_paired_output(root, directory / SIGNATURE, parent, signature)
            try:
                # A reader may treat this file as the completion marker.
                publish_paired_output(root, terminal_path, parent, _canonical(terminal) + b"\n")
            except BaseException:
                _restore_prior_output(parent, SIGNATURE, None)
                raise
    finally:
        os.close(parent)
    return root / terminal_path


def verify_release(root: Path, proof: dict, manifest: dict, *, cases: tuple[R5Case, ...]) -> None:
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
    return verify_runner_capture(record, read, cases=cases)
