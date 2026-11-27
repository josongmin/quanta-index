"""Ephemeral identities and runner evidence for isolated proof fixtures."""

from __future__ import annotations

import os
import py_compile
import subprocess
import sys
import tempfile
from functools import lru_cache
from hashlib import sha256
from pathlib import Path


def cache_old_python_source(source: Path, older: bytes) -> None:
    """Leave a timestamp cache from an equal-sized earlier module version."""
    current = source.read_bytes()
    timestamp = source.stat()
    assert len(older) <= len(current)
    cache = source.parent / "__pycache__" / f"{source.stem}.{sys.implementation.cache_tag}.pyc"
    cache.parent.mkdir(exist_ok=True)
    source.write_bytes(older.ljust(len(current), b"\n"))
    os.utime(source, ns=(timestamp.st_atime_ns, timestamp.st_mtime_ns))
    try:
        py_compile.compile(
            str(source),
            cfile=str(cache),
            doraise=True,
            invalidation_mode=py_compile.PycInvalidationMode.TIMESTAMP,
        )
    finally:
        source.write_bytes(current)
        os.utime(source, ns=(timestamp.st_atime_ns, timestamp.st_mtime_ns))


@lru_cache(maxsize=1)
def keypair() -> tuple[str, bytes]:
    with tempfile.TemporaryDirectory() as tmp:
        key = Path(tmp) / "issuer"
        subprocess.run(
            ["/usr/bin/ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key)],
            check=True,
            stdin=subprocess.DEVNULL,
        )
        public = " ".join(key.with_suffix(".pub").read_text().split()[:2])
        return public, key.read_bytes()


def sign_statement(key: str, proof_id: str, message: bytes, **_kwargs) -> bytes:
    public, secret = keypair()
    assert key == public
    with tempfile.TemporaryDirectory() as tmp:
        identity = Path(tmp) / "issuer"
        identity.write_bytes(secret)
        identity.chmod(0o600)
        return subprocess.run(
            [
                "/usr/bin/ssh-keygen",
                "-Y",
                "sign",
                "-f",
                str(identity),
                "-n",
                f"quanta/{proof_id}/v1",
            ],
            env={"PATH": os.defpath},
            input=message,
            capture_output=True,
            check=True,
        ).stdout


def nextest_suite_id(package: str, binary: str, kind: str) -> str:
    if kind in {"lib", "proc-macro"}:
        return package
    if kind == "test":
        return f"{package}::{binary}"
    return f"{package}::{kind}/{binary}"


def nextest_bytes(package: str, binary: str, kind: str, test: str) -> tuple[bytes, bytes]:
    import json

    binary_id = nextest_suite_id(package, binary, kind)
    meta = {"crate": package, "test_binary": binary, "kind": kind}
    suite = {
        "package-name": package,
        "binary-id": binary_id,
        "binary-name": binary,
        "kind": kind,
        "status": "listed",
        "testcases": {test: {"ignored": False, "filter-match": {"status": "matches"}}},
    }
    name = f"{package}::{binary}${test}"
    events = [
        {"type": "suite", "event": "started", "test_count": 1, "nextest": meta},
        {"type": "test", "event": "started", "name": name},
        {"type": "test", "event": "ok", "name": name},
        {"type": "suite", "event": "ok", "passed": 1, "failed": 0, "ignored": 0, "nextest": meta},
    ]
    return json.dumps({"test-count": 1, "rust-suites": {binary_id: suite}}).encode(), b"".join(
        json.dumps(row).encode() + b"\n" for row in events
    )


def pytest_bytes(selectors: list[str]) -> tuple[bytes, bytes]:
    import json

    cases = sorted(f"{path[:-3].replace('/', '.')}.test_case" for path in selectors)
    inventory = json.dumps(
        {
            "schema_version": 1,
            "kind": "pytest",
            "selector": " ".join(selectors),
            "tests": cases,
        }
    ).encode()
    junit = (
        f'<testsuite tests="{len(cases)}" failures="0" errors="0" skipped="0">'
        + "".join(
            f'<testcase classname="{case.rsplit(".", 1)[0]}" name="test_case"/>' for case in cases
        )
        + "</testsuite>"
    ).encode()
    return inventory, junit


def bind_run(
    root: Path,
    proof: dict,
    run: dict,
    source_path: str,
    *,
    daemon_sha256: str | None = None,
) -> None:
    from tools.ci.proof_run_source import capture_run_source, write_run_binding

    digests = {
        key: sha256((root / run[key]).read_bytes()).hexdigest()
        for key in ("events", "inventory", "daemon_observations")
        if key in run
    }
    write_run_binding(
        root / source_path,
        capture_run_source(root, proof),
        format=run["format"],
        digests=digests,
        daemon_sha256=daemon_sha256,
    )
    run["run_binding"] = source_path


def paired_evidence(
    root: Path,
    proof: dict,
    payload: dict,
    wrong_caller: bool,
    *,
    split_snapshot: bool = False,
    wrong_caller_kind: bool = False,
):
    import hashlib
    import json

    from tools.ci import paired_r5_release as release
    from tools.ci import paired_r5_result as runner
    from tools.ci.daemon_execution_evidence import R5_WITNESS_FILE, r5_cases
    from tools.ci.proof_operational_result import manifest_binding
    from tools.ci.tests.test_proof_operational_result import archive

    def digest(raw: bytes) -> str:
        return hashlib.sha256(raw).hexdigest()

    directory = Path("artifacts/proof-authority/raw/pair")
    cases = r5_cases(proof, allow_legacy=True)
    folder = root / directory
    folder.mkdir(parents=True, exist_ok=True)
    payload["environment"].update(
        toolchain="sha256:" + "8" * 64,
        features=sorted({feature for case in cases for feature in case[2].split(",")}),
    )
    pair = payload["source_pair"]
    lock = {**pair["dependency_lock"], "sha256": "sha256:" + pair["dependency_lock"]["sha256"]}
    checkout = root.parent / "semantica"
    packages = [
        {
            "name": name,
            "version": "0.1.0",
            "features": [],
            "manifest": f"crates/{name}/Cargo.toml",
            "manifest_sha256": "sha256:" + digest(name.encode()),
        }
        for name in ("quanta-index-contract", "quanta-index-ipc", "quanta-index-sdk")
    ]
    record = {
        "schema_version": runner.SCHEMA,
        "qualification": "runner-candidate-only",
        "source_heads": {"quanta": payload["source"]["head"], "semantica": pair["source"]["head"]},
        "daemon_sha256": payload["daemon_binary"]["sha256"],
        "resolutions": {
            key: {
                "dependency_lock": lock,
                "workspace": "packages/analysis/quanta-v2",
                "version": 1,
                "consumer": consumer,
                "consumer_features": features.split(","),
                "quanta_workspace_manifest_sha256": "sha256:" + "9" * 64,
                "packages": packages,
            }
            for key, (_, consumer, features, _, _, _) in zip(
                ("runtime", "kernel"), cases, strict=True
            )
        },
        "cases": [],
    }
    for case in cases:
        label, package, feature, binary, kind, test = case
        if label == "caller" and wrong_caller:
            test = "other_caller"
        observed_kind = "lib" if label == "caller" and wrong_caller_kind else kind
        listed, events = nextest_bytes(package, binary, observed_kind, test)
        name = f"{package}::{binary}${test}"
        row = {
            "package": package,
            "feature": feature,
            "binary": binary,
            "kind": kind,
            "selected_test": test,
            "event_name": name,
            "nextest": runner.validate_selected(listed, events, (*case[:4], observed_kind, test)),
            "executions": {},
        }
        for phase, stdout in (("list", listed), ("run", events)):
            run_id = label + "-" + phase
            source_digest = ("b" if label == "kernel" and split_snapshot else "a") * 64
            command = runner.phase_command(case, phase)
            artifact_env = {"fixture": run_id}
            env_sha = digest(
                json.dumps(artifact_env, sort_keys=True, separators=(",", ":")).encode()
            )
            nonce = digest(run_id.encode())[:32]
            request_sha = digest(("fixture-request-" + run_id).encode())
            physical = root.parent / (root.name + "-qbc") / run_id
            physical.mkdir(parents=True, exist_ok=True)
            receipt = json.dumps(
                {
                    "lane": "fixture",
                    "last_run_id_v1": run_id,
                    "command": command,
                    "run_state": "finished",
                    "last_exit_code": 0,
                    "command_exit_code_v1": 0,
                    "toolchain_digest": "8" * 64,
                    "artifact_env_digest": env_sha,
                    "artifact_env_payload": artifact_env,
                    "source_snapshot_digest": source_digest,
                    "lane_execution_root_v1": str(checkout),
                    "command_cwd_v1": str(checkout),
                    "target_dir": "/fixture/target",
                }
            ).encode()
            locator = json.dumps(
                {
                    "lane": "fixture",
                    "run_id": run_id,
                    "nonce": nonce,
                    "request_env_sha256": request_sha,
                    "artifact_env_digest": env_sha,
                    "receipt_sha256": digest(receipt),
                    "cargo_argv": command,
                    "schema_version": "qbc-verification-completion-locator/v1",
                    "source_snapshot_schema": "source-snapshot-identity.v1",
                    "source_snapshot_digest": source_digest,
                    "receipt_size_bytes": len(receipt),
                    "receipt_exit_code": 0,
                    "command_cwd": str(checkout),
                    "receipt_path": str(physical / "receipt.json"),
                }
            ).encode()
            custody = {
                "lane_id": "fixture",
                "run_id": run_id,
                "receipt_path": str(physical / "receipt.json"),
                "execution_root": str(checkout),
                "target_root": "/fixture/target",
                "stdout_path": str(physical / "stdout.log"),
                "stderr_path": str(physical / "stderr.log"),
            }
            archived = {}
            for filename, field, raw in (
                ("receipt.json", "receipt", receipt),
                ("stdout.log", "stdout", stdout),
                ("stderr.log", "stderr", b""),
            ):
                (folder / f"{run_id}-{filename}").write_bytes(raw)
                (physical / filename).write_bytes(raw)
                archived[filename + "_sha256"] = custody[field + "_sha256"] = digest(raw)
            (folder / f"{run_id}-completion.json").write_bytes(locator)
            row["executions"][phase] = {
                "nonce": nonce,
                "request_env_sha256": request_sha,
                "locator_sha256": digest(locator),
                "qbc_custody": custody,
                "archived": archived,
            }
            if label == "caller" and phase == "run":
                witness = json.dumps(
                    {
                        "schema_version": 1,
                        "test": name,
                        "nonce": nonce,
                        "executables": [payload["daemon_binary"]["sha256"]] * 2,
                    }
                ).encode()
                (folder / R5_WITNESS_FILE).write_bytes(witness)
                row["executions"][phase]["daemon_witness_sha256"] = digest(witness)
        record["cases"].append(row)
    index = json.dumps(record, sort_keys=True, separators=(",", ":")).encode() + b"\n"
    (folder / release.INDEX).write_bytes(index)
    (folder / release.SIGNATURE).write_bytes(
        sign_statement(
            proof["attestation_public_key"],
            proof["id"],
            release._statement(
                index, manifest_binding(payload), payload["started_at"], payload["ended_at"]
            ),
        )
    )
    artifacts = [
        archive(root, (directory / name).as_posix())
        for name in [release.INDEX, release.SIGNATURE, *release._capture_names(cases)]
    ]
    return {"schema_version": 1, "runs": release._runs(directory, cases)}, artifacts
