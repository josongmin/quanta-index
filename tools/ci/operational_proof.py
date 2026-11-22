#!/usr/bin/env python3
"""Run a registered P11 operational contract and issue its canonical manifest."""

from __future__ import annotations

import argparse
import json
import os
import sys
from datetime import datetime, timezone
from pathlib import Path
from types import ModuleType

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools.benchmark.evidence import write_raw_file  # noqa: E402
from tools.ci import proof_operational_result as operation  # noqa: E402
from tools.ci.proof_json import parse_proof_json  # noqa: E402


def _run_registered(args: argparse.Namespace, checker: ModuleType, writer: ModuleType) -> None:
    registry_path = ROOT / "tools/ci/proof-authority.toml"
    registry = checker._read_toml(registry_path)
    proof_by_id = {proof["id"]: proof for proof in registry["proofs"]}
    proof = proof_by_id[args.proof_id]
    operation._require(
        proof["authority_state"] == "executable",
        "staged operational action: concrete contract and authorized target inputs are missing",
    )
    findings = checker.check_registry(registry, root=ROOT, path=registry_path)
    operation._require(not findings, "; ".join(item.render() for item in findings))
    output = args.output or (
        Path(os.environ["QUANTA_PROOF_RAW_DIR"]) if os.environ.get("QUANTA_PROOF_RAW_DIR") else None
    )
    pair = args.paired_checkout or (
        Path(os.environ["SEMANTICA_CHECKOUT"]) if os.environ.get("SEMANTICA_CHECKOUT") else None
    )
    operation._require(
        output is not None and pair is not None,
        "operational action requires a fresh raw output and exact paired checkout",
    )
    output = (ROOT / output).absolute()
    pair = pair.resolve()
    environment = operation.observed_host_environment()
    canonical_environment = writer._host_environment(environment)
    source = checker.proof_source_snapshot(
        ROOT, manifest_path=ROOT / proof["artifact"], proof=proof, excluded_paths=(output, pair)
    )
    source_pair = checker.paired_source_snapshot(
        pair,
        repository=proof["paired_repository"],
        dependency_lock=Path(proof["paired_dependency_lock"]),
    )
    schema_path = ROOT / proof["artifact_schema"]
    raw_contract = checker.HANDOFF_VALIDATION._read_repo_regular_bytes(
        ROOT, proof["operational_contract"], label="operational contract"
    )
    contract = operation.validate_contract(ROOT, proof, raw_contract)
    daemon = None
    dependency_receipts = writer._resolve_dependencies(ROOT, proof, proof_by_id, checker)
    # Refuse a missing/stale prerequisite before any target mutation.
    for receipt in dependency_receipts:
        dependency_id = receipt["proof_id"]
        dependency = proof_by_id[dependency_id]
        payload = checker._payload_json(ROOT, receipt["path"], label="operational prerequisite")
        findings = checker.check_manifest(
            payload,
            proof=dependency,
            root=ROOT,
            manifest_path=ROOT / receipt["path"],
            bind_source=True,
            require_current_dependencies=True,
            proof_by_id=proof_by_id,
            paired_checkouts={proof["paired_repository"]: pair},
        )
        operation._require(not findings, "; ".join(item.render() for item in findings))
        operation.validate_prerequisite_target(operation.target_identity(contract), payload)
        binary = payload["daemon_binary"]
        operation._require(
            isinstance(binary, dict)
            and (
                daemon is None
                or (daemon["path"], daemon["sha256"]) == (binary["path"], binary["sha256"])
            ),
            "operational prerequisites attest different daemon binaries",
        )
        daemon = binary
        if dependency_id in operation.ACTIONS:
            operation._require(
                payload["environment"]["host"]["identity_digest"]
                == canonical_environment["host"]["identity_digest"],
                "operational prerequisite belongs to a different host",
            )
        environment["toolchain"] = payload["environment"]["toolchain"]
        environment["features"] = payload["environment"]["features"]
    operation._require(daemon is not None, "operational action lacks an attested release daemon")
    inputs = dict.fromkeys(writer.INPUT_NAMES)
    inputs["config"] = {"path": proof["operational_contract"]}
    binding = operation.manifest_binding(
        {
            "source": source,
            "source_pair": source_pair,
            "daemon_binary": daemon,
            "environment": writer._host_environment(environment),
            "inputs": operation.release_inputs(raw_contract),
            "state_root_format": contract["expected"]["state_root_format"],
            "dependency_receipts": dependency_receipts,
        }
    )
    common = {
        "environment": environment,
        "daemon_binary": daemon["path"],
        "state_root_format": binding["state_root_format"],
        "inputs": inputs,
    }

    def publish(name: str, terminal: dict) -> None:
        path = output / name
        raw = (json.dumps({**common, **terminal}, sort_keys=True, indent=2) + "\n").encode()
        write_raw_file(path, [raw])
        writer._publish_manifest_locked(
            root=ROOT,
            proof_id=args.proof_id,
            terminal_input_path=path,
            registry_path=registry_path,
            schema_path=schema_path,
            paired_checkout=pair,
        )

    def before_action() -> None:
        # Invalidate the old success durably before the first call that can change the target.
        now = datetime.now(timezone.utc).isoformat()
        publish(
            "pending.json",
            {
                "status": "blocked",
                "counts": dict.fromkeys(operation.PASSED_ACTION_COUNTS, 0),
                "started_at": now,
                "ended_at": now,
                "artifacts": [proof["operational_contract"]],
            },
        )

    result, artifacts = operation.run_action(
        ROOT, proof, binding, output, paired_checkout=pair, before_action=before_action
    )
    record = parse_proof_json(
        checker.HANDOFF_VALIDATION._read_repo_regular_bytes(
            ROOT, result["events"], label="operational events"
        )
    )
    publish(
        "terminal.json",
        {
            "status": "passed",
            "counts": operation.PASSED_ACTION_COUNTS,
            "started_at": record["steps"][0]["started_at"],
            "ended_at": record["steps"][-1]["ended_at"],
            "artifacts": artifacts,
            "execution_result": result,
            "source_binding": {key: binding[key] for key in ("source", "source_pair")},
        },
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("proof_id", choices=tuple(operation.ACTIONS))
    parser.add_argument("--output", type=Path)
    parser.add_argument("--paired-checkout", type=Path)
    args = parser.parse_args(argv)
    try:
        checker, writer = operation._checker(), operation._writer()
        with writer.proof_write_lock(ROOT):
            _run_registered(args, checker, writer)
    except (OSError, ValueError, KeyError, TypeError, RuntimeError) as error:
        print(f"operational proof refused: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
