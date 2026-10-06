#!/usr/bin/env python3
"""Run a registered P11 operational contract and issue its canonical manifest."""

from __future__ import annotations

import argparse
import json
import os
import sys
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools.ci import proof_operational_result as operation  # noqa: E402
from tools.ci.proof_json import parse_proof_json  # noqa: E402


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("proof_id", choices=tuple(operation.ACTIONS))
    parser.add_argument("--output", type=Path)
    parser.add_argument("--paired-checkout", type=Path)
    args = parser.parse_args(argv)
    try:
        checker, writer = operation._checker(), operation._writer()
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
            Path(os.environ["QUANTA_PROOF_RAW_DIR"])
            if os.environ.get("QUANTA_PROOF_RAW_DIR")
            else None
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
        schema = parse_proof_json(schema_path.read_bytes())
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
                schema=schema,
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
        operation._require(
            daemon is not None, "operational action lacks an attested release daemon"
        )
        binding = {
            "source": source,
            "source_pair": source_pair,
            "daemon_sha256": daemon["sha256"],
            "host_identity_digest": canonical_environment["host"]["identity_digest"],
            "state_root_format": contract["expected"]["state_root_format"],
            "dependency_receipts": dependency_receipts,
        }
        started = datetime.now(timezone.utc).isoformat()
        result, artifacts = operation.run_action(ROOT, proof, binding, output, paired_checkout=pair)
        terminal = {
            "status": "passed",
            "counts": operation.PASSED_ACTION_COUNTS,
            "environment": environment,
            "daemon_binary": daemon["path"],
            "state_root_format": binding["state_root_format"],
            "inputs": {
                "fixture": None,
                "corpus": None,
                "config": {"path": proof["operational_contract"]},
                "model": None,
                "provider": None,
            },
            "started_at": started,
            "ended_at": datetime.now(timezone.utc).isoformat(),
            "artifacts": artifacts,
            "execution_result": result,
        }
        terminal_path = output / "terminal.json"
        with terminal_path.open("x") as stream:
            stream.write(json.dumps(terminal, sort_keys=True, indent=2) + "\n")
        writer.publish_manifest(
            root=ROOT,
            proof_id=args.proof_id,
            terminal_input_path=terminal_path,
            registry_path=registry_path,
            schema_path=schema_path,
            paired_checkout=pair,
        )
    except (OSError, ValueError, KeyError, TypeError, RuntimeError) as error:
        print(f"operational proof refused: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
