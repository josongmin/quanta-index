#!/usr/bin/env python3
"""Validate SEP-21 proof authority and source-bound proof manifests."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any

import jsonschema

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python 3.10 compatibility
    import tomli as tomllib


ROOT = Path(__file__).resolve().parents[3]
REGISTRY_PATH = ROOT / "tools/ci/proof-authority.toml"
SCHEMA_PATH = ROOT / "tools/ci/proof-manifest.schema.json"
FAMILIES = frozenset("SUADPFQX")
CHECKPOINTS = ("M0", "M1", "M2", "M3", "M4", "M5")
GATES = frozenset(("pr", "merge", "correctness", "release"))
PROOF_ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]+$")
TICKET_RE = re.compile(r"^S21-(?:0[0-9]|1[0-3])$")
DIGEST_RE = re.compile(r"^[0-9a-f]{64}$")


@dataclass(frozen=True)
class Finding:
    path: Path
    message: str

    def render(self) -> str:
        return f"{self.path}: {self.message}"


def _read_toml(path: Path) -> dict[str, Any]:
    with path.open("rb") as handle:
        value = tomllib.load(handle)
    if not isinstance(value, dict):
        raise ValueError("registry root is not a table")
    return value


def _read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def check_registry(data: dict[str, Any], *, root: Path, path: Path) -> list[Finding]:
    findings: list[Finding] = []
    if data.get("schema") != 1:
        findings.append(Finding(path, "`schema` must be 1"))

    families = data.get("families")
    if not isinstance(families, dict):
        findings.append(Finding(path, "`families` must be a table"))
        families = {}
    family_ids = set(families)
    if family_ids != FAMILIES:
        findings.append(
            Finding(path, f"families must be exactly {sorted(FAMILIES)}, got {sorted(family_ids)}")
        )
    for family_id, family in families.items():
        if not isinstance(family, dict):
            findings.append(Finding(path, f"family {family_id!r} is not a table"))
            continue
        for field in ("name", "description"):
            if not isinstance(family.get(field), str) or not family[field].strip():
                findings.append(Finding(path, f"family {family_id!r} has no non-empty {field}"))

    proofs = data.get("proofs")
    if not isinstance(proofs, list) or not proofs:
        findings.append(Finding(path, "`proofs` must be a non-empty array of tables"))
        return findings
    test_authority_path = root / "tools/ci/test-authority.toml"
    try:
        test_authority = _read_toml(test_authority_path)
        known_test_targets = {
            target.get("id")
            for section in ("integration_targets", "fuzz_targets")
            for target in test_authority.get(section, [])
            if isinstance(target, dict) and isinstance(target.get("id"), str)
        }
    except (OSError, ValueError, tomllib.TOMLDecodeError) as error:
        findings.append(Finding(test_authority_path, f"cannot load test authority: {error}"))
        known_test_targets = set()

    seen_ids: set[str] = set()
    seen_artifacts: set[str] = set()
    tickets: set[str] = set()
    covered_families: set[str] = set()
    for index, proof in enumerate(proofs):
        where = f"proofs[{index}]"
        if not isinstance(proof, dict):
            findings.append(Finding(path, f"{where} is not a table"))
            continue
        for field in (
            "id",
            "ticket",
            "family",
            "checkpoint",
            "owner",
            "command",
            "gate",
            "profile",
            "target",
            "filter",
            "source_binding",
            "binary_binding",
            "artifact_schema",
            "artifact",
            "required_host",
        ):
            if not isinstance(proof.get(field), str) or not proof[field].strip():
                findings.append(Finding(path, f"{where}.{field} must be a non-empty string"))
        proof_id = proof.get("id")
        if isinstance(proof_id, str):
            if not PROOF_ID_RE.fullmatch(proof_id):
                findings.append(
                    Finding(path, f"{where}.id {proof_id!r} is not canonical kebab-case")
                )
            if proof_id in seen_ids:
                findings.append(Finding(path, f"duplicate proof id {proof_id!r}"))
            seen_ids.add(proof_id)
        ticket = proof.get("ticket")
        if isinstance(ticket, str):
            if not TICKET_RE.fullmatch(ticket):
                findings.append(Finding(path, f"{where}.ticket {ticket!r} is not S21-00..S21-13"))
            tickets.add(ticket)
        family = proof.get("family")
        if isinstance(family, str):
            if family not in FAMILIES:
                findings.append(Finding(path, f"{where}.family {family!r} is not registered"))
            covered_families.add(family)
        checkpoint = proof.get("checkpoint")
        if isinstance(checkpoint, str) and checkpoint not in CHECKPOINTS:
            findings.append(Finding(path, f"{where}.checkpoint {checkpoint!r} is not M0..M5"))
        gate = proof.get("gate")
        if isinstance(gate, str) and gate not in GATES:
            findings.append(Finding(path, f"{where}.gate {gate!r} is not registered"))
        source_binding = proof.get("source_binding")
        if isinstance(source_binding, str) and source_binding not in {"exact", "exact-pair"}:
            findings.append(Finding(path, f"{where}.source_binding {source_binding!r} is invalid"))
        binary_binding = proof.get("binary_binding")
        if isinstance(binary_binding, str) and binary_binding not in {"none", "release-daemon"}:
            findings.append(Finding(path, f"{where}.binary_binding {binary_binding!r} is invalid"))
        if family in {"P", "Q", "X"} and proof.get("required_host") != "linux-production-like":
            findings.append(
                Finding(path, f"{where} family {family} requires linux-production-like host")
            )
        if family in {"P", "X"} and binary_binding != "release-daemon":
            findings.append(
                Finding(path, f"{where} family {family} requires release-daemon binding")
            )
        artifact_schema = proof.get("artifact_schema")
        if isinstance(artifact_schema, str) and not (root / artifact_schema).is_file():
            findings.append(
                Finding(path, f"{where}.artifact_schema does not exist: {artifact_schema}")
            )
        owner = proof.get("owner")
        if isinstance(owner, str) and owner and not (root / owner).is_file():
            findings.append(Finding(path, f"{where}.owner does not exist: {owner}"))
        artifact = proof.get("artifact")
        if isinstance(artifact, str):
            artifact_path = Path(artifact)
            if artifact_path.is_absolute() or ".." in artifact_path.parts:
                findings.append(
                    Finding(path, f"{where}.artifact must be repo-relative: {artifact}")
                )
            if artifact in seen_artifacts:
                findings.append(Finding(path, f"duplicate artifact path {artifact!r}"))
            seen_artifacts.add(artifact)
        test_targets = proof.get("test_authority_targets")
        if not isinstance(test_targets, list) or any(
            not isinstance(item, str) for item in test_targets
        ):
            findings.append(
                Finding(path, f"{where}.test_authority_targets must be an array of strings")
            )
        else:
            for target_id in test_targets:
                if target_id not in known_test_targets:
                    findings.append(
                        Finding(
                            path,
                            f"{where} names unknown test-authority target {target_id!r}",
                        )
                    )
        dependencies = proof.get("dependencies")
        if not isinstance(dependencies, list) or any(
            not isinstance(item, str) for item in dependencies
        ):
            findings.append(Finding(path, f"{where}.dependencies must be an array of proof IDs"))

    expected_tickets = {f"S21-{index:02d}" for index in range(14)}
    if tickets != expected_tickets:
        findings.append(
            Finding(
                path,
                f"proof tickets must cover S21-00..S21-13 exactly; missing={sorted(expected_tickets - tickets)} extra={sorted(tickets - expected_tickets)}",
            )
        )
    if covered_families != FAMILIES:
        findings.append(
            Finding(
                path,
                f"proofs must cover every family; missing={sorted(FAMILIES - covered_families)}",
            )
        )
    proof_by_id = {
        proof["id"]: proof
        for proof in proofs
        if isinstance(proof, dict) and isinstance(proof.get("id"), str)
    }
    for proof_id, proof in proof_by_id.items():
        for dependency in proof.get("dependencies", []):
            if dependency not in proof_by_id:
                findings.append(
                    Finding(path, f"proof {proof_id!r} depends on unknown proof {dependency!r}")
                )

    visiting: set[str] = set()
    visited: set[str] = set()

    def visit(proof_id: str) -> None:
        if proof_id in visited:
            return
        if proof_id in visiting:
            findings.append(Finding(path, f"proof dependency cycle reaches {proof_id!r}"))
            return
        visiting.add(proof_id)
        for dependency in proof_by_id[proof_id].get("dependencies", []):
            if dependency in proof_by_id:
                visit(dependency)
        visiting.remove(proof_id)
        visited.add(proof_id)

    for proof_id in proof_by_id:
        visit(proof_id)
    return findings


def _parse_time(value: str) -> datetime:
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _git(root: Path, *args: str) -> str:
    completed = subprocess.run(
        ["git", "-C", str(root), *args],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise RuntimeError(completed.stderr.strip() or f"git {' '.join(args)} failed")
    return completed.stdout.strip()


def dirty_digest(root: Path) -> str:
    digest = hashlib.sha256()
    diff = subprocess.run(
        ["git", "-C", str(root), "diff", "--binary"],
        check=True,
        capture_output=True,
    ).stdout
    digest.update(diff)
    untracked = _git(root, "ls-files", "--others", "--exclude-standard", "-z").encode()
    for raw_path in sorted(item for item in untracked.split(b"\0") if item):
        digest.update(len(raw_path).to_bytes(8, "big"))
        digest.update(raw_path)
        file_path = root / raw_path.decode()
        if file_path.is_file():
            digest.update(_sha256(file_path).encode())
    return f"sha256:{digest.hexdigest()}"


def check_manifest(
    payload: Any,
    *,
    manifest_path: Path,
    proof: dict[str, Any],
    schema: dict[str, Any],
    root: Path,
    bind_source: bool,
) -> list[Finding]:
    findings: list[Finding] = []
    validator = jsonschema.Draft202012Validator(schema, format_checker=jsonschema.FormatChecker())
    for error in sorted(validator.iter_errors(payload), key=lambda item: list(item.path)):
        location = ".".join(str(part) for part in error.path) or "root"
        findings.append(Finding(manifest_path, f"schema {location}: {error.message}"))
    if not isinstance(payload, dict) or findings:
        return findings
    if payload["proof_id"] != proof["id"]:
        findings.append(Finding(manifest_path, f"proof_id is not registry id {proof['id']!r}"))
    if payload["family"] != proof["family"]:
        findings.append(
            Finding(manifest_path, f"family is not registry family {proof['family']!r}")
        )
    if payload["invocation"]["command"] != proof["command"]:
        findings.append(Finding(manifest_path, "invocation.command differs from proof authority"))
    for field in ("profile", "target", "filter"):
        if payload["invocation"][field] != proof[field]:
            findings.append(
                Finding(manifest_path, f"invocation.{field} differs from proof authority")
            )
    if (
        proof["required_host"] != "any"
        and payload["environment"]["host"]["profile"] != proof["required_host"]
    ):
        findings.append(
            Finding(manifest_path, f"host profile is not required {proof['required_host']!r}")
        )

    counts = payload["counts"]
    if counts["selected"] != counts["executed"] + counts["ignored"]:
        findings.append(Finding(manifest_path, "selected must equal executed + ignored"))
    if counts["executed"] != counts["passed"] + counts["failed"]:
        findings.append(Finding(manifest_path, "executed must equal passed + failed"))
    if payload["status"] == "passed":
        if counts["selected"] == 0 or counts["executed"] == 0 or counts["passed"] == 0:
            findings.append(
                Finding(manifest_path, "passed proof requires selected, executed and passed > 0")
            )
        if counts["failed"] != 0:
            findings.append(Finding(manifest_path, "passed proof cannot contain failed tests"))
        if counts["ignored"] != 0:
            findings.append(
                Finding(manifest_path, "mandatory passed proof cannot contain ignored tests")
            )
    if _parse_time(payload["ended_at"]) < _parse_time(payload["started_at"]):
        findings.append(Finding(manifest_path, "ended_at precedes started_at"))

    daemon_path = root / payload["daemon_binary"]["path"]
    if not daemon_path.is_file():
        findings.append(Finding(manifest_path, f"daemon binary is missing: {daemon_path}"))
    elif _sha256(daemon_path) != payload["daemon_binary"]["sha256"]:
        findings.append(Finding(manifest_path, "daemon binary digest mismatch"))
    for artifact in payload["artifacts"]:
        artifact_path = root / artifact["path"]
        if not artifact_path.is_file():
            findings.append(Finding(manifest_path, f"proof artifact is missing: {artifact_path}"))
        elif _sha256(artifact_path) != artifact["sha256"]:
            findings.append(
                Finding(manifest_path, f"proof artifact digest mismatch: {artifact_path}")
            )

    dependencies = payload["dependency_receipts"]
    if {item["proof_id"] for item in dependencies} != set(proof["dependencies"]):
        findings.append(
            Finding(manifest_path, "dependency receipt IDs differ from proof authority")
        )
    for dependency in dependencies:
        dependency_path = root / dependency["path"]
        if not dependency_path.is_file():
            findings.append(
                Finding(manifest_path, f"dependency receipt is missing: {dependency_path}")
            )
        elif _sha256(dependency_path) != dependency["sha256"]:
            findings.append(
                Finding(manifest_path, f"dependency receipt digest mismatch: {dependency_path}")
            )

    if bind_source:
        if payload["source"]["head"] != _git(root, "rev-parse", "HEAD"):
            findings.append(Finding(manifest_path, "source.head is not current HEAD"))
        if payload["source"]["dirty_digest"] != dirty_digest(root):
            findings.append(
                Finding(manifest_path, "source.dirty_digest is not current working tree")
            )
    return findings


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--registry", type=Path, default=None)
    parser.add_argument("--schema", type=Path, default=None)
    parser.add_argument("--manifest", action="append", type=Path, default=[])
    parser.add_argument("--require-all", action="store_true")
    parser.add_argument("--bind-source", action="store_true")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    root = args.root.resolve()
    registry_path = (args.registry or root / "tools/ci/proof-authority.toml").resolve()
    schema_path = (args.schema or root / "tools/ci/proof-manifest.schema.json").resolve()
    try:
        registry = _read_toml(registry_path)
        schema = _read_json(schema_path)
    except (OSError, ValueError, json.JSONDecodeError, tomllib.TOMLDecodeError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2

    findings = check_registry(registry, root=root, path=registry_path)
    proof_by_id = {
        proof["id"]: proof
        for proof in registry.get("proofs", [])
        if isinstance(proof, dict) and isinstance(proof.get("id"), str)
    }
    manifest_paths = [path.resolve() for path in args.manifest]
    if args.require_all:
        manifest_paths.extend(
            (root / proof["artifact"]).resolve() for proof in proof_by_id.values()
        )
    seen_paths: set[Path] = set()
    seen_proofs: set[str] = set()
    for manifest_path in manifest_paths:
        if manifest_path in seen_paths:
            continue
        seen_paths.add(manifest_path)
        if not manifest_path.is_file():
            findings.append(Finding(manifest_path, "required proof manifest is missing"))
            continue
        try:
            payload = _read_json(manifest_path)
        except (OSError, json.JSONDecodeError) as error:
            findings.append(Finding(manifest_path, f"unreadable proof manifest: {error}"))
            continue
        proof_id = payload.get("proof_id") if isinstance(payload, dict) else None
        proof = proof_by_id.get(proof_id)
        if proof is None:
            findings.append(Finding(manifest_path, f"proof_id {proof_id!r} is not registered"))
            continue
        if proof_id in seen_proofs:
            findings.append(Finding(manifest_path, f"duplicate manifest for proof_id {proof_id!r}"))
            continue
        seen_proofs.add(proof_id)
        findings.extend(
            check_manifest(
                payload,
                manifest_path=manifest_path,
                proof=proof,
                schema=schema,
                root=root,
                bind_source=args.bind_source,
            )
        )

    if findings:
        for finding in findings:
            print(f"REFUSED {finding.render()}", file=sys.stderr)
        print(f"FAIL: {len(findings)} proof-authority finding(s)", file=sys.stderr)
        return 1
    print(f"OK: {len(proof_by_id)} registered proof(s); {len(seen_proofs)} manifest(s) validated")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
