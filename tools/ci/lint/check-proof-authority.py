#!/usr/bin/env python3
"""Validate SEP-21 proof authority and source-bound proof manifests."""

from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
import re
import stat
import subprocess
import sys
import tempfile
from collections.abc import Iterable
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path, PurePosixPath
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
AUTHORITY_STATES = frozenset(("executable", "staged"))
EXECUTION_MODES = frozenset(("test-authority", "non-test-assertion", "aggregate"))
VERDICTS = frozenset(("CODE_QUALIFIED", "DEPLOYED", "ACTIVATED", "ROLLBACK_PROVEN"))
PROOF_ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]+$")
TICKET_RE = re.compile(r"^S21-(?:0[0-9]|1[0-3])$")
DIGEST_RE = re.compile(r"^[0-9a-f]{64}$")
PAIRED_REPOSITORY = "github:josongmin/semantica-codegraph-v2"
PAIRED_DEPENDENCY_LOCK = "Cargo.lock"
EXPECTED_PROOF_DEPENDENCIES: dict[str, list[str]] = {
    "p00-authority-freeze": [],
    "p01-canonical-identity": ["p00-authority-freeze"],
    "p02a-repomap-compiler": ["p01-canonical-identity"],
    "p02b-operation-journal": ["p01-canonical-identity"],
    "p03-candidate-activation": ["p02a-repomap-compiler", "p02b-operation-journal"],
    "p04-read-view-lifetime": ["p03-candidate-activation", "p02b-operation-journal"],
    "p05-query-truth": ["p04-read-view-lifetime"],
    "p06-sdk-binding": [
        "p03-candidate-activation",
        "p02b-operation-journal",
        "p05-query-truth",
    ],
    "p07-provider-boundary": ["p06-sdk-binding"],
    "p08-runtime-supervisor": ["p07-provider-boundary"],
    "p09-control-readiness": ["p08-runtime-supervisor", "p02b-operation-journal"],
    "p10-state-migration": [
        "p01-canonical-identity",
        "p03-candidate-activation",
        "p02b-operation-journal",
        "p08-runtime-supervisor",
        "p09-control-readiness",
    ],
    "p11-cross-repo-cutover": [
        "p03-candidate-activation",
        "p02b-operation-journal",
        "p06-sdk-binding",
        "p10-state-migration",
    ],
    "p11-deployment": ["p11-cross-repo-cutover"],
    "p11-activation": ["p11-deployment"],
    "p11-rollback": ["p10-state-migration", "p11-activation"],
    "p12-final-qualification": [
        "p01-canonical-identity",
        "p02a-repomap-compiler",
        "p02b-operation-journal",
        "p03-candidate-activation",
        "p04-read-view-lifetime",
        "p05-query-truth",
        "p06-sdk-binding",
        "p07-provider-boundary",
        "p08-runtime-supervisor",
        "p09-control-readiness",
        "p10-state-migration",
        "p11-cross-repo-cutover",
        "p11-deployment",
        "p11-activation",
        "p11-rollback",
    ],
}
EXPECTED_VERDICT_PROOFS: dict[str, list[str]] = {
    "CODE_QUALIFIED": [
        "p00-authority-freeze",
        "p01-canonical-identity",
        "p02a-repomap-compiler",
        "p02b-operation-journal",
        "p03-candidate-activation",
        "p04-read-view-lifetime",
        "p05-query-truth",
        "p06-sdk-binding",
        "p07-provider-boundary",
        "p08-runtime-supervisor",
        "p09-control-readiness",
        "p10-state-migration",
        "p11-cross-repo-cutover",
    ],
    "DEPLOYED": ["p11-deployment"],
    "ACTIVATED": ["p11-activation"],
    "ROLLBACK_PROVEN": ["p10-state-migration", "p11-rollback"],
}
STREAM_CHUNK_SIZE = 1024 * 1024


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


def _just_recipe_body(root: Path, recipe: str) -> str | None:
    lines = (root / "Justfile").read_text(encoding="utf-8").splitlines()
    header = re.compile(rf"^{re.escape(recipe)}(?:\s+[^:]*)?:\s*(?:#.*)?$")
    for index, line in enumerate(lines):
        if header.fullmatch(line):
            body: list[str] = []
            for candidate in lines[index + 1 :]:
                if candidate and not candidate[0].isspace():
                    break
                body.append(candidate.strip())
            return "\n".join(body)
    return None


def check_registry(data: dict[str, Any], *, root: Path, path: Path) -> list[Finding]:
    findings: list[Finding] = []
    if data.get("schema") != 2:
        findings.append(Finding(path, "`schema` must be 2"))

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
        target_owners = {
            target["id"]: target.get("owner")
            for target in test_authority.get("integration_targets", [])
            if isinstance(target, dict) and isinstance(target.get("id"), str)
        }
        local_scopes = test_authority.get("local_scopes", {})
        if not isinstance(local_scopes, dict):
            local_scopes = {}
    except (OSError, ValueError, tomllib.TOMLDecodeError) as error:
        findings.append(Finding(test_authority_path, f"cannot load test authority: {error}"))
        known_test_targets = set()
        target_owners = {}
        local_scopes = {}

    def expand_scope(scope_id: str, active: set[str] | None = None) -> set[str]:
        active = set() if active is None else set(active)
        if scope_id in active:
            return set()
        active.add(scope_id)
        scope = local_scopes.get(scope_id)
        if not isinstance(scope, dict):
            return set()
        expanded = {target for target in scope.get("targets", []) if isinstance(target, str)}
        owners = {owner for owner in scope.get("owners", []) if isinstance(owner, str)}
        expanded.update(target_id for target_id, owner in target_owners.items() if owner in owners)
        for included in scope.get("includes", []):
            if isinstance(included, str):
                expanded.update(expand_scope(included, active))
        return expanded

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
            "authority_state",
            "execution_mode",
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
        authority_state = proof.get("authority_state")
        execution_mode = proof.get("execution_mode")
        if authority_state not in AUTHORITY_STATES:
            findings.append(Finding(path, f"{where}.authority_state is not registered"))
        if execution_mode not in EXECUTION_MODES:
            findings.append(Finding(path, f"{where}.execution_mode is not registered"))
        staged_reason = proof.get("staged_reason")
        if authority_state == "staged":
            if not isinstance(staged_reason, str) or not staged_reason.strip():
                findings.append(
                    Finding(path, f"{where}.staged_reason must explain the missing authority")
                )
        elif "staged_reason" in proof:
            findings.append(Finding(path, f"{where}.staged_reason is forbidden when executable"))
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
        if source_binding == "exact":
            for field in ("paired_repository", "paired_dependency_lock"):
                if field in proof:
                    findings.append(
                        Finding(path, f"{where}.{field} is forbidden for exact source binding")
                    )
        elif source_binding == "exact-pair":
            if proof.get("paired_repository") != PAIRED_REPOSITORY:
                findings.append(
                    Finding(
                        path,
                        f"{where}.paired_repository must be {PAIRED_REPOSITORY!r}",
                    )
                )
            if proof.get("paired_dependency_lock") != PAIRED_DEPENDENCY_LOCK:
                findings.append(
                    Finding(
                        path,
                        f"{where}.paired_dependency_lock must be {PAIRED_DEPENDENCY_LOCK!r}",
                    )
                )
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
            if (
                authority_state == "executable"
                and execution_mode == "test-authority"
                and not test_targets
            ):
                findings.append(
                    Finding(
                        path,
                        f"{where} executable test-authority proof requires at least one target",
                    )
                )
            if authority_state == "executable" and execution_mode == "test-authority":
                scopes = proof.get("test_authority_scopes")
                if (
                    not isinstance(scopes, list)
                    or not scopes
                    or any(not isinstance(scope, str) or not scope for scope in scopes)
                ):
                    findings.append(
                        Finding(
                            path,
                            f"{where} executable test-authority proof requires non-empty test_authority_scopes",
                        )
                    )
                else:
                    unknown_scopes = set(scopes) - set(local_scopes)
                    if unknown_scopes:
                        findings.append(
                            Finding(
                                path,
                                f"{where} names unknown local scopes {sorted(unknown_scopes)}",
                            )
                        )
                    expanded_targets: set[str] = set()
                    for scope in scopes:
                        expanded_targets.update(expand_scope(scope))
                    uncovered = set(test_targets) - expanded_targets
                    if uncovered:
                        findings.append(
                            Finding(
                                path,
                                f"{where} targets are not selected by declared scopes: {sorted(uncovered)}",
                            )
                        )
                command = proof.get("command")
                profile = proof.get("profile")
                profile_match = (
                    re.fullmatch(r"just rust-profile ([a-z0-9-]+)", command)
                    if isinstance(command, str)
                    else None
                )
                if profile_match is not None:
                    command_profile = profile_match.group(1)
                    expected_scope = command_profile.removeprefix("test-")
                    if profile != command_profile:
                        findings.append(Finding(path, f"{where} command/profile binding differs"))
                    if not isinstance(scopes, list) or expected_scope not in scopes:
                        findings.append(
                            Finding(
                                path,
                                f"{where} rust profile does not name its local scope {expected_scope!r}",
                            )
                        )
                elif not isinstance(command, str) or not command.startswith("just proof-"):
                    findings.append(
                        Finding(
                            path,
                            f"{where} executable test authority requires a canonical rust profile or dedicated proof recipe",
                        )
                    )
                else:
                    recipe = command.removeprefix("just ")
                    try:
                        recipe_body = _just_recipe_body(root, recipe)
                    except OSError as error:
                        findings.append(Finding(path, f"cannot read Justfile: {error}"))
                        recipe_body = None
                    if recipe_body is None:
                        findings.append(
                            Finding(
                                path, f"{where} dedicated proof recipe does not exist: {recipe}"
                            )
                        )
                    elif isinstance(scopes, list):
                        missing_scope_calls = [
                            scope
                            for scope in scopes
                            if not any(
                                re.fullmatch(
                                    rf"@?just rust-profile test-{re.escape(scope)}",
                                    line,
                                )
                                for line in recipe_body.splitlines()
                            )
                        ]
                        if missing_scope_calls:
                            findings.append(
                                Finding(
                                    path,
                                    f"{where} dedicated proof recipe does not execute scopes {missing_scope_calls}",
                                )
                            )
            if execution_mode in {"non-test-assertion", "aggregate"} and test_targets:
                findings.append(
                    Finding(path, f"{where} {execution_mode} proof cannot name test targets")
                )
        if execution_mode == "non-test-assertion" and proof_id != "p00-authority-freeze":
            findings.append(Finding(path, f"{where} non-test-assertion is reserved for P00"))
        if (
            proof_id == "p00-authority-freeze"
            and proof.get("command") != "just proof-p00-authority-freeze"
        ):
            findings.append(Finding(path, f"{where} must use the composite P00 authority recipe"))
        if execution_mode == "aggregate" and proof_id != "p12-final-qualification":
            findings.append(Finding(path, f"{where} aggregate execution is reserved for P12"))
        if (
            proof_id == "p12-final-qualification"
            and proof.get("command") != "just proof-authority-final-qualification"
        ):
            findings.append(Finding(path, f"{where} must use the canonical P12 aggregate recipe"))
        dependencies = proof.get("dependencies")
        if not isinstance(dependencies, list) or any(
            not isinstance(item, str) for item in dependencies
        ):
            findings.append(Finding(path, f"{where}.dependencies must be an array of proof IDs"))

    expected_tickets = {f"S21-{index:02d}" for index in range(14)}
    if seen_ids != set(EXPECTED_PROOF_DEPENDENCIES):
        findings.append(
            Finding(
                path,
                "proof IDs differ from canonical SEP-21 graph: "
                f"missing={sorted(set(EXPECTED_PROOF_DEPENDENCIES) - seen_ids)} "
                f"extra={sorted(seen_ids - set(EXPECTED_PROOF_DEPENDENCIES))}",
            )
        )
    for index, proof in enumerate(proofs):
        if not isinstance(proof, dict) or not isinstance(proof.get("id"), str):
            continue
        expected_dependencies = EXPECTED_PROOF_DEPENDENCIES.get(proof["id"])
        if expected_dependencies is not None and proof.get("dependencies") != expected_dependencies:
            findings.append(
                Finding(
                    path,
                    f"proofs[{index}].dependencies differ from canonical SEP-21 graph",
                )
            )
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

    aggregate = data.get("aggregate")
    if not isinstance(aggregate, dict):
        findings.append(Finding(path, "`aggregate` must be a table"))
    else:
        expected = {"id", "target_proof", "schema", "artifact", "verdicts"}
        if set(aggregate) != expected:
            findings.append(
                Finding(path, "aggregate keys must be id,target_proof,schema,artifact,verdicts")
            )
        if aggregate.get("id") != "p12-release-aggregate":
            findings.append(Finding(path, "aggregate.id must be p12-release-aggregate"))
        if aggregate.get("target_proof") != "p12-final-qualification":
            findings.append(Finding(path, "aggregate.target_proof must be p12-final-qualification"))
        aggregate_schema = aggregate.get("schema")
        if isinstance(aggregate_schema, str):
            schema_path, schema_error = _payload_repo_file(
                root, aggregate_schema, label="aggregate schema"
            )
            if schema_error is not None:
                findings.append(Finding(path, schema_error))
            elif schema_path is None or not schema_path.is_file():
                findings.append(
                    Finding(path, f"aggregate.schema does not exist: {aggregate_schema}")
                )
        aggregate_artifact = aggregate.get("artifact")
        if isinstance(aggregate_artifact, str):
            artifact_relative = PurePosixPath(aggregate_artifact)
            if (
                not aggregate_artifact
                or "\\" in aggregate_artifact
                or artifact_relative.is_absolute()
                or ".." in artifact_relative.parts
                or artifact_relative.as_posix() != aggregate_artifact
                or aggregate_artifact in seen_artifacts
            ):
                findings.append(
                    Finding(path, "aggregate.artifact must be unique canonical repo-relative")
                )
        target_proof = proof_by_id.get(aggregate.get("target_proof"))
        if target_proof is not None and target_proof.get("execution_mode") != "aggregate":
            findings.append(
                Finding(path, "aggregate target proof must use aggregate execution mode")
            )
        verdicts = aggregate.get("verdicts")
        if not isinstance(verdicts, dict) or set(verdicts) != VERDICTS:
            findings.append(Finding(path, f"aggregate verdicts must be exactly {sorted(VERDICTS)}"))
        else:
            if verdicts != EXPECTED_VERDICT_PROOFS:
                findings.append(
                    Finding(
                        path, "aggregate verdict proof sets differ from canonical SEP-21 meanings"
                    )
                )
            closure = set()
            try:
                closure = set(dependency_closure(proof_by_id, "p12-final-qualification"))
            except ValueError as error:
                findings.append(Finding(path, str(error)))
            covered: set[str] = set()
            for verdict, requirements in verdicts.items():
                if (
                    not isinstance(requirements, list)
                    or not requirements
                    or any(not isinstance(item, str) for item in requirements)
                ):
                    findings.append(
                        Finding(path, f"aggregate verdict {verdict} requires proof IDs")
                    )
                    continue
                unknown = set(requirements) - closure
                if unknown:
                    findings.append(
                        Finding(
                            path,
                            f"aggregate verdict {verdict} has non-dependencies {sorted(unknown)}",
                        )
                    )
                covered.update(requirements)
            if closure and covered != closure:
                findings.append(
                    Finding(
                        path,
                        f"aggregate verdict requirements must cover dependency closure; missing={sorted(closure - covered)}",
                    )
                )
    return findings


def _parse_time(value: str) -> datetime:
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(STREAM_CHUNK_SIZE), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _payload_repo_file(
    root: Path,
    value: str,
    *,
    label: str,
) -> tuple[Path | None, str | None]:
    """Resolve an untrusted manifest path without permitting repository escape."""

    relative = PurePosixPath(value)
    if (
        not value
        or "\\" in value
        or relative.is_absolute()
        or ".." in relative.parts
        or relative.as_posix() != value
        or relative == PurePosixPath(".")
    ):
        return None, f"{label} path must be canonical repo-relative: {value!r}"
    try:
        resolved_root = root.resolve()
        resolved = (resolved_root / Path(*relative.parts)).resolve()
    except (OSError, RuntimeError) as error:
        return None, f"{label} path cannot be resolved safely: {value!r}: {error}"
    try:
        resolved.relative_to(resolved_root)
    except ValueError:
        return None, f"{label} path escapes repository root: {value!r}"
    return resolved, None


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


def _git_bytes(root: Path, *args: str) -> bytes:
    completed = subprocess.run(
        ["git", "-C", str(root), *args],
        check=False,
        capture_output=True,
    )
    if completed.returncode != 0:
        message = os.fsdecode(completed.stderr).strip()
        raise RuntimeError(message or f"git {' '.join(args)} failed")
    return completed.stdout


def _optional_git(root: Path, *args: str) -> str | None:
    completed = subprocess.run(
        ["git", "-C", str(root), *args],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode == 0:
        return completed.stdout.strip()
    return None


def _merge_base(root: Path, head: str, upstream: str) -> str | None:
    completed = subprocess.run(
        ["git", "-C", str(root), "merge-base", head, upstream],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode == 1:
        return None
    if completed.returncode != 0:
        message = completed.stderr.strip()
        raise RuntimeError(message or "git merge-base failed")
    value = completed.stdout.strip()
    if not re.fullmatch(r"[0-9a-f]{40}", value):
        raise RuntimeError(f"git merge-base returned an invalid object id: {value!r}")
    return value


def _digest_record(digest: Any, *fields: bytes) -> None:
    for field in fields:
        _digest_field(digest, field)


def _digest_field(digest: Any, field: bytes) -> None:
    digest.update(len(field).to_bytes(8, "big"))
    digest.update(field)


def _digest_stream_field(digest: Any, stream: Any, size: int) -> None:
    digest.update(size.to_bytes(8, "big"))
    remaining = size
    while remaining:
        chunk = stream.read(min(STREAM_CHUNK_SIZE, remaining))
        if not chunk:
            raise RuntimeError("truncated source while hashing")
        digest.update(chunk)
        remaining -= len(chunk)


def _repo_relative_bytes(root: Path, path: Path) -> bytes | None:
    candidate = path if path.is_absolute() else root / path
    try:
        relative = candidate.absolute().relative_to(root.absolute())
    except ValueError:
        return None
    raw = os.fsencode(relative)
    return None if raw in {b"", b"."} else raw


def _is_excluded(raw_path: bytes, excluded_paths: tuple[bytes, ...]) -> bool:
    return any(raw_path == item or raw_path.startswith(item + b"/") for item in excluded_paths)


def _digest_working_tree_entry(
    digest: Any,
    domain: bytes,
    root: Path,
    raw_path: bytes,
) -> None:
    native_path = os.fsencode(root) + b"/" + raw_path
    try:
        metadata = os.lstat(native_path)
    except FileNotFoundError:
        _digest_record(digest, domain, raw_path, b"missing", b"")
        return
    mode = f"{stat.S_IMODE(metadata.st_mode):04o}".encode()
    if stat.S_ISLNK(metadata.st_mode):
        _digest_record(
            digest,
            domain,
            raw_path,
            b"symlink:" + mode,
            os.fsencode(os.readlink(native_path)),
        )
        return
    if stat.S_ISREG(metadata.st_mode):
        flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
        try:
            descriptor = os.open(native_path, flags)
        except OSError as error:
            raise RuntimeError(f"source changed while hashing {os.fsdecode(raw_path)!r}") from error
        with os.fdopen(descriptor, "rb") as handle:
            opened = os.fstat(handle.fileno())
            if not stat.S_ISREG(opened.st_mode):
                raise RuntimeError(f"source changed while hashing {os.fsdecode(raw_path)!r}")
            opened_mode = f"{stat.S_IMODE(opened.st_mode):04o}".encode()
            _digest_record(digest, domain, raw_path, b"file:" + opened_mode)
            _digest_stream_field(digest, handle, opened.st_size)
            if handle.read(1):
                raise RuntimeError(f"source grew while hashing {os.fsdecode(raw_path)!r}")
            current = os.fstat(handle.fileno())
            if current.st_size != opened.st_size or current.st_mtime_ns != opened.st_mtime_ns:
                raise RuntimeError(f"source changed while hashing {os.fsdecode(raw_path)!r}")
        return
    if stat.S_ISDIR(metadata.st_mode):
        # A changed submodule is a source-state change, but its contents are a
        # separate repository. Bind its exact checked-out commit and dirty bit.
        submodule = root / os.fsdecode(raw_path)
        head = _optional_git(submodule, "rev-parse", "--verify", "HEAD") or "missing"
        dirty = _git_bytes(submodule, "status", "--porcelain=v1", "-z")
        _digest_record(
            digest,
            domain,
            raw_path,
            b"directory:" + mode,
            head.encode() + b"\0" + dirty,
        )
        return
    _digest_record(digest, domain, raw_path, b"special:" + mode, b"")


def _index_entries(
    root: Path, raw_paths: Iterable[bytes]
) -> dict[bytes, list[tuple[bytes, bytes]]]:
    requested = set(raw_paths)
    entries_by_path: dict[bytes, list[tuple[bytes, bytes]]] = {
        raw_path: [] for raw_path in requested
    }
    if not requested:
        return entries_by_path

    # Read the index once. Passing every path as an argument would still risk
    # ARG_MAX on large staged changes, while one ls-files scan stays constant
    # in subprocess count and preserves arbitrary path bytes with -z.
    output = _git_bytes(root, "ls-files", "--stage", "-z")
    for raw_entry in (entry for entry in output.split(b"\0") if entry):
        header, _, entry_path = raw_entry.partition(b"\t")
        if entry_path not in requested:
            continue
        parts = header.split()
        if len(parts) != 3:
            raise RuntimeError(f"unexpected git index entry for {os.fsdecode(entry_path)!r}")
        mode, object_id, stage = parts
        entries_by_path[entry_path].append((b"mode=" + mode + b";stage=" + stage, object_id))
    return entries_by_path


def _digest_index_entries(
    digest: Any,
    root: Path,
    raw_paths: Iterable[bytes],
    entries_by_path: dict[bytes, list[tuple[bytes, bytes]]],
) -> None:
    ordered_paths = list(raw_paths)
    if not any(entries_by_path[raw_path] for raw_path in ordered_paths):
        for raw_path in ordered_paths:
            _digest_record(digest, b"index", raw_path, b"missing", b"")
        return

    with tempfile.TemporaryFile() as error_stream:
        process = subprocess.Popen(
            ["git", "-C", str(root), "cat-file", "--batch"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=error_stream,
        )
        assert process.stdin is not None
        assert process.stdout is not None
        try:
            pending: list[tuple[bytes, bytes, bytes]] = []
            pending_bytes = 0
            try:
                pipe_buf = max(512, os.fpathconf(process.stdin.fileno(), "PC_PIPE_BUF"))
            except (OSError, ValueError):
                pipe_buf = 512

            def digest_pending() -> None:
                nonlocal pending_bytes
                if not pending:
                    return
                # The complete request chunk fits in the stdin pipe's atomic
                # write bound before Git can block while emitting responses.
                request_chunk = b"".join(object_id + b"\n" for _, _, object_id in pending)
                if process.stdin.write(request_chunk) != len(request_chunk):
                    raise RuntimeError("truncated git cat-file --batch request")
                process.stdin.flush()
                for raw_path, metadata, object_id in pending:
                    header = process.stdout.readline().removesuffix(b"\n").split()
                    if len(header) != 3 or header[0] != object_id or header[1] != b"blob":
                        raise RuntimeError(
                            f"unexpected git cat-file --batch header for {os.fsdecode(object_id)!r}"
                        )
                    try:
                        size = int(header[2])
                    except ValueError as error:
                        raise RuntimeError("invalid git cat-file --batch object size") from error
                    _digest_record(digest, b"index", raw_path, metadata)
                    _digest_stream_field(digest, process.stdout, size)
                    if process.stdout.read(1) != b"\n":
                        raise RuntimeError("truncated git cat-file --batch object delimiter")
                pending.clear()
                pending_bytes = 0

            for raw_path in ordered_paths:
                entries = entries_by_path[raw_path]
                if not entries:
                    digest_pending()
                    _digest_record(digest, b"index", raw_path, b"missing", b"")
                    continue
                for metadata, object_id in entries:
                    request_size = len(object_id) + 1
                    if request_size > pipe_buf:
                        raise RuntimeError("Git object identifier exceeds pipe write bound")
                    if pending and pending_bytes + request_size > pipe_buf:
                        digest_pending()
                    pending.append((raw_path, metadata, object_id))
                    pending_bytes += request_size
            digest_pending()

            process.stdin.close()
            returncode = process.wait()
            error_stream.seek(0)
            message = os.fsdecode(error_stream.read()).strip()
            if returncode != 0:
                raise RuntimeError(message or "git cat-file --batch failed")
        except BaseException:
            if process.poll() is None:
                process.kill()
                process.wait()
            raise
        finally:
            if not process.stdin.closed:
                process.stdin.close()
            process.stdout.close()


def _git_status_snapshot(root: Path) -> bytes:
    return _git_bytes(
        root,
        "status",
        "--porcelain=v2",
        "--branch",
        "-z",
        "--untracked-files=all",
        "--ignore-submodules=none",
        "--no-renames",
        "--no-ahead-behind",
    )


def _dirty_paths_from_status(output: bytes) -> tuple[list[bytes], list[bytes], list[bytes]]:
    staged: set[bytes] = set()
    unstaged: set[bytes] = set()
    untracked: set[bytes] = set()
    for record in output.split(b"\0"):
        if not record or record.startswith(b"# "):
            continue
        kind = record[:1]
        if kind == b"1":
            parts = record.split(b" ", 8)
            if len(parts) != 9 or len(parts[1]) != 2:
                raise RuntimeError("unexpected porcelain-v2 ordinary status record")
            xy, path = parts[1], parts[8]
        elif kind == b"u":
            parts = record.split(b" ", 10)
            if len(parts) != 11 or len(parts[1]) != 2:
                raise RuntimeError("unexpected porcelain-v2 unmerged status record")
            xy, path = parts[1], parts[10]
        elif kind == b"?":
            if not record.startswith(b"? "):
                raise RuntimeError("unexpected porcelain-v2 untracked status record")
            untracked.add(record[2:])
            continue
        else:
            raise RuntimeError(
                f"unsupported porcelain-v2 status record: {os.fsdecode(record[:32])!r}"
            )
        if xy[:1] != b".":
            staged.add(path)
        if xy[1:] != b".":
            unstaged.add(path)
    return sorted(staged), sorted(unstaged), sorted(untracked)


def dirty_digest(
    root: Path,
    *,
    excluded_paths: Iterable[Path] = (),
    _status_snapshot: bytes | None = None,
) -> str:
    """Hash staged, unstaged and scoped untracked source bytes.

    The index and worktree are distinct domains so staging a different byte
    sequence cannot collapse into the same receipt. `excluded_paths` removes
    generated proof roots from the untracked scope and prevents a manifest
    from changing its own source identity when it is atomically published.
    """

    digest = hashlib.sha256()
    digest.update(b"quanta-index-dirty-v2\0")
    exclusions = tuple(
        sorted(
            raw for path in excluded_paths if (raw := _repo_relative_bytes(root, path)) is not None
        )
    )

    staged, unstaged, untracked = _dirty_paths_from_status(
        _status_snapshot if _status_snapshot is not None else _git_status_snapshot(root)
    )
    included_staged = [raw_path for raw_path in staged if not _is_excluded(raw_path, exclusions)]
    staged_entries = _index_entries(root, included_staged)
    _digest_index_entries(digest, root, included_staged, staged_entries)

    for raw_path in unstaged:
        if _is_excluded(raw_path, exclusions):
            continue
        _digest_working_tree_entry(digest, b"worktree", root, raw_path)

    for raw_path in untracked:
        if _is_excluded(raw_path, exclusions):
            continue
        _digest_working_tree_entry(digest, b"untracked", root, raw_path)
    return f"sha256:{digest.hexdigest()}"


def _source_identity(output: bytes) -> tuple[str, str | None, str | None]:
    """Read HEAD, branch and upstream from porcelain-v2 branch headers."""
    headers: dict[bytes, bytes] = {}
    for record in output.split(b"\0"):
        if not record.startswith(b"# "):
            continue
        key, separator, value = record[2:].partition(b" ")
        if separator:
            headers[key] = value
    raw_head = headers.get(b"branch.oid")
    raw_branch = headers.get(b"branch.head")
    if raw_head in {None, b"(initial)"} or raw_branch is None:
        raise RuntimeError("Git repository has no committed HEAD")
    branch = None if raw_branch == b"(detached)" else os.fsdecode(raw_branch)
    raw_upstream = headers.get(b"branch.upstream")
    upstream = None if raw_upstream is None else os.fsdecode(raw_upstream)
    return os.fsdecode(raw_head), branch, upstream


def source_snapshot(root: Path, *, excluded_paths: Iterable[Path] = ()) -> dict[str, Any]:
    """Return the exact Git identity used by source-bound proof manifests."""

    status_snapshot = _git_status_snapshot(root)
    head, branch, upstream = _source_identity(status_snapshot)
    merge_base = _merge_base(root, head, upstream) if upstream is not None else None
    snapshot = {
        "head": head,
        "dirty_digest": dirty_digest(
            root,
            excluded_paths=excluded_paths,
            _status_snapshot=status_snapshot,
        ),
        "branch": branch,
        "upstream": upstream,
        "merge_base": merge_base,
    }
    if _git_status_snapshot(root) != status_snapshot:
        raise RuntimeError("Git source changed while capturing proof snapshot")
    if upstream is not None and _merge_base(root, head, upstream) != merge_base:
        raise RuntimeError("Git upstream changed while capturing proof snapshot")
    return snapshot


def proof_source_snapshot(
    root: Path,
    *,
    manifest_path: Path,
    proof: dict[str, Any],
    excluded_paths: Iterable[Path] = (),
) -> dict[str, Any]:
    artifact = Path(proof["artifact"])
    artifact_root = artifact.parent
    exclusions = [manifest_path, artifact]
    if artifact_root != Path("."):
        exclusions.append(artifact_root)
    exclusions.extend(excluded_paths)
    return source_snapshot(root, excluded_paths=exclusions)


def _cached_proof_source_snapshot(
    cache: dict[tuple[Path, Path | None, tuple[Path, ...]], dict[str, Any]],
    root: Path,
    *,
    manifest_path: Path,
    proof: dict[str, Any],
    excluded_paths: Iterable[Path] = (),
) -> dict[str, Any]:
    """Reuse a source binding only inside one validation pass."""

    artifact_root = Path(proof["artifact"]).parent
    resolved_exclusions = tuple(
        sorted(
            (
                path.resolve()
                for path in excluded_paths
                if _repo_relative_bytes(root, path) is not None
            ),
            key=str,
        )
    )
    key = (
        artifact_root,
        manifest_path.resolve() if artifact_root == Path(".") else None,
        resolved_exclusions,
    )
    if key not in cache:
        cache[key] = proof_source_snapshot(
            root,
            manifest_path=manifest_path,
            proof=proof,
            excluded_paths=resolved_exclusions,
        )
    return cache[key]


def _cached_paired_source_snapshot(
    cache: dict[tuple[Path, str, Path], dict[str, Any]],
    checkout: Path,
    *,
    repository: str,
    dependency_lock: Path,
) -> dict[str, Any]:
    """Reuse one paired-checkout binding only inside one validation pass."""

    key = (checkout.resolve(), repository, dependency_lock)
    if key not in cache:
        cache[key] = paired_source_snapshot(
            checkout,
            repository=repository,
            dependency_lock=dependency_lock,
        )
    return cache[key]


def _github_repository_from_origin(origin: str) -> str | None:
    patterns = (
        r"^https://github\.com/(?P<path>[^/\s]+/[^/\s]+?)(?:\.git)?/?$",
        r"^git@github[^:]*:(?P<path>[^/\s]+/[^/\s]+?)(?:\.git)?$",
        r"^ssh://git@github[^/]*/(?P<path>[^/\s]+/[^/\s]+?)(?:\.git)?/?$",
    )
    for pattern in patterns:
        match = re.fullmatch(pattern, origin)
        if match is not None:
            return f"github:{match.group('path')}"
    return None


def paired_source_snapshot(
    checkout: Path,
    *,
    repository: str,
    dependency_lock: Path,
) -> dict[str, Any]:
    """Bind an exact external checkout without recording its host-local path."""

    checkout = checkout.resolve()
    top_level = Path(_git(checkout, "rev-parse", "--show-toplevel")).resolve()
    if top_level != checkout:
        raise ValueError(f"paired checkout is not its repository root: {checkout}")
    origin = _git(checkout, "remote", "get-url", "origin")
    actual_repository = _github_repository_from_origin(origin)
    if actual_repository != repository:
        raise ValueError(
            f"paired checkout origin identifies {actual_repository!r}, expected {repository!r}"
        )
    lock_value = dependency_lock.as_posix()
    lock_path, path_error = _payload_repo_file(
        checkout,
        lock_value,
        label="paired dependency lock",
    )
    if path_error is not None:
        raise ValueError(path_error)
    assert lock_path is not None
    if not lock_path.is_file():
        raise ValueError(f"paired dependency lock is missing: {lock_value!r}")
    # Bind the canonical repository identity, not the transport spelling of
    # the remote. The same trusted GitHub repository may be checked out via an
    # HTTPS URL or an SSH host alias; that must not fork release receipts.
    remote_digest = hashlib.sha256()
    remote_digest.update(b"quanta-index-remote-identity-v1\0")
    remote_digest.update(repository.encode("utf-8"))
    return {
        "repository": repository,
        "remote_identity_digest": f"sha256:{remote_digest.hexdigest()}",
        "source": source_snapshot(checkout),
        "dependency_lock": {
            "path": lock_value,
            "sha256": _sha256(lock_path),
        },
    }


def check_manifest(
    payload: Any,
    *,
    manifest_path: Path,
    proof: dict[str, Any],
    schema: dict[str, Any],
    root: Path,
    bind_source: bool,
    allow_non_passed: bool = False,
    paired_checkouts: dict[str, Path] | None = None,
    proof_by_id: dict[str, dict[str, Any]] | None = None,
    bound_source: dict[str, Any] | None = None,
    bound_source_pair: dict[str, Any] | None = None,
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
    if proof.get("authority_state") != "executable":
        findings.append(
            Finding(manifest_path, "staged proof cannot be authoritative or issue a manifest")
        )
    if payload["family"] != proof["family"]:
        findings.append(
            Finding(manifest_path, f"family is not registry family {proof['family']!r}")
        )
    if payload["status"] != "passed" and not allow_non_passed:
        findings.append(
            Finding(
                manifest_path,
                f"authoritative proof status must be 'passed', got {payload['status']!r}",
            )
        )
    source_pair = payload["source_pair"]
    paired_checkout: Path | None = None
    if proof["source_binding"] == "exact":
        if source_pair is not None:
            findings.append(
                Finding(manifest_path, "source_binding='exact' requires source_pair=null")
            )
    else:
        if not isinstance(source_pair, dict):
            findings.append(
                Finding(
                    manifest_path,
                    "source_binding='exact-pair' requires a source_pair object",
                )
            )
        else:
            repository = proof["paired_repository"]
            dependency_lock = proof["paired_dependency_lock"]
            if source_pair["repository"] != repository:
                findings.append(
                    Finding(manifest_path, "source_pair.repository differs from proof authority")
                )
            if source_pair["dependency_lock"]["path"] != dependency_lock:
                findings.append(
                    Finding(
                        manifest_path,
                        "source_pair.dependency_lock.path differs from proof authority",
                    )
                )
            if bind_source:
                paired_checkout = (paired_checkouts or {}).get(repository)
                if paired_checkout is None:
                    findings.append(
                        Finding(
                            manifest_path,
                            f"source_pair requires --paired-checkout {repository}=PATH",
                        )
                    )
                else:
                    try:
                        live_pair = (
                            bound_source_pair
                            if bound_source_pair is not None
                            else paired_source_snapshot(
                                paired_checkout,
                                repository=repository,
                                dependency_lock=Path(dependency_lock),
                            )
                        )
                    except (OSError, RuntimeError, ValueError) as error:
                        findings.append(Finding(manifest_path, f"cannot bind source_pair: {error}"))
                    else:
                        if source_pair != live_pair:
                            findings.append(
                                Finding(manifest_path, "source_pair is not current paired source")
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
    if (
        proof["required_host"] == "linux-production-like"
        and payload["environment"]["os"] != "linux"
    ):
        findings.append(
            Finding(manifest_path, "linux-production-like proof requires environment.os='linux'")
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

    daemon_binary = payload["daemon_binary"]
    if proof["binary_binding"] == "none":
        if daemon_binary is not None:
            findings.append(
                Finding(manifest_path, "binary_binding='none' requires daemon_binary=null")
            )
    elif not isinstance(daemon_binary, dict):
        findings.append(
            Finding(
                manifest_path,
                "binary_binding='release-daemon' requires a daemon_binary object",
            )
        )
    else:
        daemon_path, path_error = _payload_repo_file(
            root,
            daemon_binary["path"],
            label="daemon binary",
        )
        if path_error is not None:
            findings.append(Finding(manifest_path, path_error))
        elif daemon_path is None or not daemon_path.is_file():
            findings.append(Finding(manifest_path, f"daemon binary is missing: {daemon_path}"))
        elif _sha256(daemon_path) != daemon_binary["sha256"]:
            findings.append(Finding(manifest_path, "daemon binary digest mismatch"))
    for artifact in payload["artifacts"]:
        artifact_path, path_error = _payload_repo_file(
            root,
            artifact["path"],
            label="proof artifact",
        )
        if path_error is not None:
            findings.append(Finding(manifest_path, path_error))
        elif artifact_path is None or not artifact_path.is_file():
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
        if proof_by_id is not None:
            dependency_authority = proof_by_id.get(dependency["proof_id"])
            if dependency_authority is None:
                findings.append(
                    Finding(
                        manifest_path,
                        f"dependency receipt names unknown proof {dependency['proof_id']!r}",
                    )
                )
                continue
            if dependency["path"] != dependency_authority["artifact"]:
                findings.append(
                    Finding(
                        manifest_path,
                        f"dependency receipt path differs from registered artifact: {dependency['proof_id']!r}",
                    )
                )
                continue
        dependency_path, path_error = _payload_repo_file(
            root,
            dependency["path"],
            label="dependency receipt",
        )
        if path_error is not None:
            findings.append(Finding(manifest_path, path_error))
        elif dependency_path is None or not dependency_path.is_file():
            findings.append(
                Finding(manifest_path, f"dependency receipt is missing: {dependency_path}")
            )
        elif _sha256(dependency_path) != dependency["sha256"]:
            findings.append(
                Finding(manifest_path, f"dependency receipt digest mismatch: {dependency_path}")
            )

    if bind_source:
        current_source = (
            bound_source
            if bound_source is not None
            else proof_source_snapshot(
                root,
                manifest_path=manifest_path,
                proof=proof,
                excluded_paths=(() if paired_checkout is None else (paired_checkout,)),
            )
        )
        labels = {
            "head": "current HEAD",
            "dirty_digest": "current working tree",
            "branch": "current branch",
            "upstream": "current upstream",
            "merge_base": "current upstream merge-base",
        }
        for field, label in labels.items():
            if payload["source"][field] != current_source[field]:
                findings.append(Finding(manifest_path, f"source.{field} is not {label}"))
    return findings


def dependency_closure(proof_by_id: dict[str, dict[str, Any]], target_id: str) -> list[str]:
    """Return a target's transitive dependencies in dependency-first order."""

    if target_id not in proof_by_id:
        raise ValueError(f"unknown proof id {target_id!r}")
    ordered: list[str] = []
    visited: set[str] = set()
    active: set[str] = set()

    def visit(proof_id: str) -> None:
        if proof_id in active:
            raise ValueError(f"proof dependency cycle reaches {proof_id!r}")
        active.add(proof_id)
        for dependency in proof_by_id[proof_id]["dependencies"]:
            if dependency not in proof_by_id:
                raise ValueError(f"proof {proof_id!r} depends on unknown proof {dependency!r}")
            if dependency not in visited:
                visit(dependency)
                visited.add(dependency)
                ordered.append(dependency)
        active.remove(proof_id)

    visit(target_id)
    return ordered


def check_aggregate(
    payload_by_id: dict[str, dict[str, Any]],
    *,
    proof_by_id: dict[str, dict[str, Any]],
    path: Path,
) -> list[Finding]:
    findings: list[Finding] = []
    exact_sources = {
        json.dumps(payload["source"], sort_keys=True, separators=(",", ":"))
        for proof_id, payload in payload_by_id.items()
        if proof_by_id[proof_id]["source_binding"] == "exact"
    }
    if len(exact_sources) > 1:
        findings.append(
            Finding(path, "aggregate exact-source manifests do not share one source identity")
        )
    daemon_identities = {
        (payload["daemon_binary"]["path"], payload["daemon_binary"]["sha256"])
        for proof_id, payload in payload_by_id.items()
        if proof_by_id[proof_id]["binary_binding"] == "release-daemon"
        and isinstance(payload["daemon_binary"], dict)
    }
    if len(daemon_identities) > 1:
        findings.append(
            Finding(
                path,
                "aggregate release-daemon proofs do not share one daemon path and digest",
            )
        )
    paired_sources = {
        json.dumps(payload["source_pair"], sort_keys=True, separators=(",", ":"))
        for proof_id, payload in payload_by_id.items()
        if proof_by_id[proof_id]["source_binding"] == "exact-pair"
        and isinstance(payload["source_pair"], dict)
    }
    if len(paired_sources) > 1:
        findings.append(
            Finding(path, "aggregate exact-pair manifests do not share one paired source identity")
        )
    release_hosts = {
        (
            payload["environment"]["host"]["profile"],
            payload["environment"]["host"]["identity_digest"],
        )
        for proof_id, payload in payload_by_id.items()
        if proof_by_id[proof_id]["binary_binding"] == "release-daemon"
    }
    if len(release_hosts) > 1:
        findings.append(
            Finding(path, "aggregate release-daemon proofs do not share one host identity")
        )
    state_root_formats = {
        payload["state_root_format"]
        for proof_id, payload in payload_by_id.items()
        if proof_by_id[proof_id]["binary_binding"] == "release-daemon"
    }
    if len(state_root_formats) > 1:
        findings.append(
            Finding(path, "aggregate release-daemon proofs do not share one state-root format")
        )
    return findings


def check_aggregate_receipt(
    payload: Any,
    *,
    receipt_path: Path,
    registry: dict[str, Any],
    registry_path: Path,
    schema: dict[str, Any],
    root: Path,
    bind_source: bool,
    paired_checkouts: dict[str, Path] | None = None,
    require_ready: bool = False,
) -> list[Finding]:
    """Validate a truthful diagnostic or release-ready P12 aggregate receipt."""

    findings: list[Finding] = []
    validator = jsonschema.Draft202012Validator(schema, format_checker=jsonschema.FormatChecker())
    for error in sorted(validator.iter_errors(payload), key=lambda item: list(item.path)):
        location = ".".join(str(part) for part in error.path) or "root"
        findings.append(Finding(receipt_path, f"schema {location}: {error.message}"))
    if not isinstance(payload, dict) or findings:
        return findings

    aggregate = registry.get("aggregate")
    if not isinstance(aggregate, dict):
        return [Finding(receipt_path, "registry has no aggregate authority")]
    proof_by_id = {
        proof["id"]: proof
        for proof in registry.get("proofs", [])
        if isinstance(proof, dict) and isinstance(proof.get("id"), str)
    }
    target_id = aggregate.get("target_proof")
    if not isinstance(target_id, str):
        return [Finding(receipt_path, "registry aggregate has no target proof")]
    try:
        dependency_ids = dependency_closure(proof_by_id, target_id)
    except ValueError as error:
        return [Finding(receipt_path, str(error))]

    if payload.get("aggregate_id") != aggregate.get("id"):
        findings.append(Finding(receipt_path, "aggregate_id differs from registry authority"))
    if payload.get("target_proof_id") != target_id:
        findings.append(Finding(receipt_path, "target_proof_id differs from registry authority"))
    if payload.get("registry_sha256") != _sha256(registry_path):
        findings.append(Finding(receipt_path, "registry_sha256 is not the current registry"))

    receipts = payload.get("dependency_receipts")
    if not isinstance(receipts, list):
        return findings
    receipt_ids = [item.get("proof_id") for item in receipts if isinstance(item, dict)]
    if receipt_ids != dependency_ids:
        findings.append(
            Finding(
                receipt_path, "aggregate dependency receipts are not the ordered target closure"
            )
        )

    target_proof = proof_by_id[target_id]
    excluded_pair_paths = tuple((paired_checkouts or {}).values())
    source_cache: dict[tuple[Path, Path | None, tuple[Path, ...]], dict[str, Any]] = {}
    pair_cache: dict[tuple[Path, str, Path], dict[str, Any]] = {}
    current_source = _cached_proof_source_snapshot(
        source_cache,
        root,
        manifest_path=receipt_path,
        proof=target_proof,
        excluded_paths=excluded_pair_paths,
    )
    if payload.get("source") != current_source:
        findings.append(Finding(receipt_path, "aggregate source is not current source"))

    expected_pair: dict[str, Any] | None = None
    repository = target_proof.get("paired_repository")
    checkout = (paired_checkouts or {}).get(repository) if isinstance(repository, str) else None
    source_requests: list[tuple[Path, dict[str, Any], tuple[Path, ...]]] = []
    pair_requests: list[tuple[Path, str, Path]] = []
    if bind_source:
        source_requests.append((receipt_path, target_proof, excluded_pair_paths))
        if checkout is None:
            findings.append(
                Finding(receipt_path, f"aggregate requires paired checkout for {repository!r}")
            )
        else:
            try:
                expected_pair = _cached_paired_source_snapshot(
                    pair_cache,
                    checkout,
                    repository=repository,
                    dependency_lock=Path(target_proof["paired_dependency_lock"]),
                )
            except (OSError, RuntimeError, ValueError) as error:
                findings.append(
                    Finding(receipt_path, f"cannot bind aggregate source_pair: {error}")
                )
            else:
                pair_requests.append(
                    (checkout, repository, Path(target_proof["paired_dependency_lock"]))
                )
                if payload.get("source_pair") != expected_pair:
                    findings.append(
                        Finding(receipt_path, "aggregate source_pair is not current paired source")
                    )

    payload_by_id: dict[str, dict[str, Any]] = {}
    dependency_statuses: dict[str, str] = {}
    for proof_id in dependency_ids:
        proof = proof_by_id[proof_id]
        matching = [
            item for item in receipts if isinstance(item, dict) and item.get("proof_id") == proof_id
        ]
        if len(matching) != 1:
            findings.append(
                Finding(receipt_path, f"aggregate requires exactly one receipt for {proof_id!r}")
            )
            continue
        receipt = matching[0]
        if receipt.get("path") != proof["artifact"]:
            findings.append(
                Finding(receipt_path, f"aggregate receipt path differs for {proof_id!r}")
            )
            continue
        manifest_path, path_error = _payload_repo_file(
            root,
            proof["artifact"],
            label=f"aggregate dependency {proof_id!r}",
        )
        if path_error is not None:
            findings.append(Finding(receipt_path, path_error))
            continue
        assert manifest_path is not None
        expected_status = "NOT_RUN"
        expected_sha256: str | None = None
        manifest_findings: list[Finding] = []
        manifest: Any = None
        if proof.get("authority_state") != "executable":
            expected_status = "BLOCKED"
        elif not manifest_path.is_file():
            expected_status = "NOT_RUN"
        else:
            expected_sha256 = _sha256(manifest_path)
            try:
                manifest = _read_json(manifest_path)
                proof_repository = proof.get("paired_repository")
                manifest_checkout = (
                    (paired_checkouts or {}).get(proof_repository)
                    if isinstance(proof_repository, str)
                    else None
                )
                bound_source = None
                bound_source_pair = None
                if bind_source:
                    manifest_exclusions = () if manifest_checkout is None else (manifest_checkout,)
                    source_requests.append((manifest_path, proof, manifest_exclusions))
                    bound_source = _cached_proof_source_snapshot(
                        source_cache,
                        root,
                        manifest_path=manifest_path,
                        proof=proof,
                        excluded_paths=manifest_exclusions,
                    )
                    if manifest_checkout is not None:
                        dependency_lock = Path(proof["paired_dependency_lock"])
                        try:
                            bound_source_pair = _cached_paired_source_snapshot(
                                pair_cache,
                                manifest_checkout,
                                repository=proof_repository,
                                dependency_lock=dependency_lock,
                            )
                        except (OSError, RuntimeError, ValueError):
                            # Preserve check_manifest's contextual finding on
                            # an invalid paired checkout.
                            bound_source_pair = None
                        else:
                            pair_requests.append(
                                (manifest_checkout, proof_repository, dependency_lock)
                            )
                manifest_findings = check_manifest(
                    manifest,
                    manifest_path=manifest_path,
                    proof=proof,
                    schema=_read_json(root / proof["artifact_schema"]),
                    root=root,
                    bind_source=bind_source,
                    allow_non_passed=True,
                    paired_checkouts=paired_checkouts,
                    proof_by_id=proof_by_id,
                    bound_source=bound_source,
                    bound_source_pair=bound_source_pair,
                )
            except (OSError, json.JSONDecodeError) as error:
                manifest_findings = [
                    Finding(manifest_path, f"aggregate dependency is unreadable: {error}")
                ]
            if manifest_findings:
                expected_status = "FAILED"
            elif isinstance(manifest, dict):
                expected_status = {
                    "passed": "PASSED",
                    "failed": "FAILED",
                    "blocked": "BLOCKED",
                    "not_run": "NOT_RUN",
                }[manifest["status"]]
                if expected_status == "PASSED":
                    payload_by_id[proof_id] = manifest

        dependency_statuses[proof_id] = expected_status
        if receipt.get("sha256") != expected_sha256:
            findings.append(
                Finding(receipt_path, f"aggregate dependency digest mismatch: {proof_id!r}")
            )
        if receipt.get("status") != expected_status:
            findings.append(
                Finding(
                    receipt_path,
                    f"aggregate dependency status differs for {proof_id!r}: expected {expected_status}",
                )
            )
        if require_ready and expected_status != "PASSED":
            findings.extend(manifest_findings)
            findings.append(
                Finding(receipt_path, f"release aggregate dependency is not PASSED: {proof_id!r}")
            )

    consistency_findings = check_aggregate(
        payload_by_id, proof_by_id=proof_by_id, path=receipt_path
    )
    if require_ready:
        findings.extend(consistency_findings)
    all_dependencies_passed = len(dependency_statuses) == len(dependency_ids) and all(
        status == "PASSED" for status in dependency_statuses.values()
    )
    release_ready_inputs = all_dependencies_passed and not consistency_findings
    expected_daemon: dict[str, str] | None = None
    expected_host: dict[str, str] | None = None
    expected_state_root: str | None = None
    if release_ready_inputs:
        daemon_values = {
            (manifest["daemon_binary"]["path"], manifest["daemon_binary"]["sha256"])
            for proof_id, manifest in payload_by_id.items()
            if proof_by_id[proof_id]["binary_binding"] == "release-daemon"
        }
        host_values = {
            (
                manifest["environment"]["host"]["profile"],
                manifest["environment"]["host"]["identity_digest"],
            )
            for proof_id, manifest in payload_by_id.items()
            if proof_by_id[proof_id]["binary_binding"] == "release-daemon"
        }
        root_values = {
            manifest["state_root_format"]
            for proof_id, manifest in payload_by_id.items()
            if proof_by_id[proof_id]["binary_binding"] == "release-daemon"
        }
        if len(daemon_values) == 1:
            daemon_path, daemon_sha = next(iter(daemon_values))
            expected_daemon = {"path": daemon_path, "sha256": daemon_sha}
        if len(host_values) == 1:
            host_profile, host_digest = next(iter(host_values))
            expected_host = {"profile": host_profile, "identity_digest": host_digest}
        if len(root_values) == 1:
            expected_state_root = next(iter(root_values))
    if payload.get("daemon_binary") != expected_daemon:
        findings.append(Finding(receipt_path, "aggregate daemon_binary is not derived authority"))
    if payload.get("release_host") != expected_host:
        findings.append(Finding(receipt_path, "aggregate release_host is not derived authority"))
    if payload.get("state_root_format") != expected_state_root:
        findings.append(
            Finding(receipt_path, "aggregate state_root_format is not derived authority")
        )

    verdicts = payload.get("verdicts")
    registered_verdicts = aggregate.get("verdicts")
    expected_verdict_statuses: dict[str, str] = {}
    if isinstance(verdicts, dict) and isinstance(registered_verdicts, dict):
        for verdict in sorted(VERDICTS):
            actual = verdicts.get(verdict)
            expected_proofs = registered_verdicts.get(verdict)
            if not isinstance(actual, dict):
                continue
            if actual.get("required_proofs") != expected_proofs:
                findings.append(
                    Finding(receipt_path, f"aggregate verdict proof set differs for {verdict}")
                )
            required_statuses = [
                dependency_statuses.get(proof_id, "FAILED") for proof_id in expected_proofs
            ]
            if consistency_findings and all(status == "PASSED" for status in required_statuses):
                expected_status = "FAILED"
            elif "FAILED" in required_statuses:
                expected_status = "FAILED"
            elif "BLOCKED" in required_statuses:
                expected_status = "BLOCKED"
            elif "NOT_RUN" in required_statuses:
                expected_status = "NOT_RUN"
            else:
                expected_status = "PASSED"
            expected_verdict_statuses[verdict] = expected_status
            if actual.get("status") != expected_status:
                findings.append(
                    Finding(
                        receipt_path,
                        f"aggregate verdict status differs for {verdict}: expected {expected_status}",
                    )
                )
    expected_ready = (
        set(expected_verdict_statuses) == VERDICTS
        and all(status == "PASSED" for status in expected_verdict_statuses.values())
        and release_ready_inputs
    )
    if payload.get("production_ready") is not expected_ready:
        findings.append(Finding(receipt_path, "aggregate production_ready is not derived verdict"))
    if require_ready and not expected_ready:
        findings.append(Finding(receipt_path, "aggregate is not production ready"))
    if bind_source:
        final_source_cache: dict[tuple[Path, Path | None, tuple[Path, ...]], dict[str, Any]] = {}
        final_pair_cache: dict[tuple[Path, str, Path], dict[str, Any]] = {}
        for manifest_path, proof, exclusions in source_requests:
            _cached_proof_source_snapshot(
                final_source_cache,
                root,
                manifest_path=manifest_path,
                proof=proof,
                excluded_paths=exclusions,
            )
        for pair_checkout, pair_repository, dependency_lock in pair_requests:
            _cached_paired_source_snapshot(
                final_pair_cache,
                pair_checkout,
                repository=pair_repository,
                dependency_lock=dependency_lock,
            )
        if final_source_cache != source_cache or final_pair_cache != pair_cache:
            findings.append(Finding(receipt_path, "source changed during aggregate validation"))
    return findings


def _parse_paired_checkouts(values: list[str]) -> tuple[dict[str, Path], list[str]]:
    checkouts: dict[str, Path] = {}
    errors: list[str] = []
    for value in values:
        repository, separator, raw_path = value.partition("=")
        if not separator or not repository or not raw_path:
            errors.append(f"invalid --paired-checkout {value!r}; expected REPOSITORY=PATH")
            continue
        if repository in checkouts:
            errors.append(f"duplicate --paired-checkout for {repository!r}")
            continue
        checkouts[repository] = Path(raw_path)
    return checkouts, errors


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--registry", type=Path, default=None)
    parser.add_argument("--schema", type=Path, default=None)
    parser.add_argument("--manifest", action="append", type=Path, default=[])
    selection = parser.add_mutually_exclusive_group()
    selection.add_argument("--require-all", action="store_true")
    selection.add_argument("--dependencies-of", metavar="PROOF_ID")
    parser.add_argument(
        "--paired-checkout",
        action="append",
        default=[],
        metavar="REPOSITORY=PATH",
    )
    parser.add_argument("--bind-source", action="store_true")
    return parser.parse_args(argv)


def _main_locked(argv: list[str] | None = None) -> int:
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
    paired_checkouts, paired_errors = _parse_paired_checkouts(args.paired_checkout)
    findings.extend(Finding(registry_path, message) for message in paired_errors)
    paired_repositories = {
        proof["paired_repository"]
        for proof in proof_by_id.values()
        if proof.get("source_binding") == "exact-pair"
        and isinstance(proof.get("paired_repository"), str)
    }
    for repository in paired_checkouts.keys() - paired_repositories:
        findings.append(
            Finding(registry_path, f"--paired-checkout names unknown repository {repository!r}")
        )
    manifest_paths = [path.resolve() for path in args.manifest]
    expected_proof_by_path: dict[Path, str] = {}

    def require_registered_manifest(proof_id: str) -> None:
        proof = proof_by_id[proof_id]
        manifest_path, path_error = _payload_repo_file(
            root,
            proof["artifact"],
            label=f"proof {proof_id!r} manifest artifact",
        )
        if path_error is not None:
            findings.append(Finding(registry_path, path_error))
            return
        assert manifest_path is not None
        manifest_paths.append(manifest_path)
        expected_proof_by_path[manifest_path] = proof_id

    if args.require_all:
        if not args.bind_source:
            findings.append(Finding(registry_path, "--require-all requires --bind-source"))
        for proof_id in proof_by_id:
            require_registered_manifest(proof_id)
    if args.dependencies_of is not None:
        if not args.bind_source:
            findings.append(Finding(registry_path, "--dependencies-of requires --bind-source"))
        try:
            dependency_ids = dependency_closure(proof_by_id, args.dependencies_of)
        except ValueError as error:
            findings.append(Finding(registry_path, str(error)))
        else:
            for proof_id in dependency_ids:
                require_registered_manifest(proof_id)
    seen_paths: set[Path] = set()
    seen_proofs: set[str] = set()
    payload_by_id: dict[str, dict[str, Any]] = {}
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
        expected_proof_id = expected_proof_by_path.get(manifest_path)
        if expected_proof_id is not None and proof_id != expected_proof_id:
            findings.append(
                Finding(
                    manifest_path,
                    f"registered artifact requires proof_id {expected_proof_id!r}, got {proof_id!r}",
                )
            )
            continue
        proof = proof_by_id.get(proof_id)
        if proof is None:
            findings.append(Finding(manifest_path, f"proof_id {proof_id!r} is not registered"))
            continue
        if proof_id in seen_proofs:
            findings.append(Finding(manifest_path, f"duplicate manifest for proof_id {proof_id!r}"))
            continue
        seen_proofs.add(proof_id)
        manifest_findings = check_manifest(
            payload,
            manifest_path=manifest_path,
            proof=proof,
            schema=schema,
            root=root,
            bind_source=args.bind_source,
            paired_checkouts=paired_checkouts,
            proof_by_id=proof_by_id,
        )
        findings.extend(manifest_findings)
        if not manifest_findings:
            payload_by_id[proof_id] = payload

    if args.require_all or args.dependencies_of is not None:
        findings.extend(
            check_aggregate(
                payload_by_id,
                proof_by_id=proof_by_id,
                path=registry_path,
            )
        )

    if args.require_all:
        aggregate = registry.get("aggregate")
        if isinstance(aggregate, dict):
            aggregate_path, path_error = _payload_repo_file(
                root,
                aggregate.get("artifact"),
                label="registered aggregate artifact",
            )
            if path_error is not None:
                findings.append(Finding(registry_path, path_error))
            elif aggregate_path is None or not aggregate_path.is_file():
                findings.append(Finding(registry_path, "registered aggregate artifact is missing"))
            else:
                try:
                    aggregate_payload = _read_json(aggregate_path)
                    aggregate_schema = _read_json(root / aggregate["schema"])
                except (OSError, json.JSONDecodeError) as error:
                    findings.append(
                        Finding(aggregate_path, f"unreadable aggregate artifact: {error}")
                    )
                else:
                    findings.extend(
                        check_aggregate_receipt(
                            aggregate_payload,
                            receipt_path=aggregate_path,
                            registry=registry,
                            registry_path=registry_path,
                            schema=aggregate_schema,
                            root=root,
                            bind_source=True,
                            paired_checkouts=paired_checkouts,
                            require_ready=True,
                        )
                    )

    if findings:
        for finding in findings:
            print(f"REFUSED {finding.render()}", file=sys.stderr)
        print(f"FAIL: {len(findings)} proof-authority finding(s)", file=sys.stderr)
        return 1
    print(f"OK: {len(proof_by_id)} registered proof(s); {len(seen_proofs)} manifest(s) validated")
    return 0


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    root = args.root.resolve()
    completed = subprocess.run(
        ["git", "-C", str(root), "rev-parse", "--git-path", "quanta-proof-authority.lock"],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        print(
            f"ERROR: {completed.stderr.strip() or 'cannot resolve proof lock path'}",
            file=sys.stderr,
        )
        return 2
    lock_path = Path(completed.stdout.strip())
    if not lock_path.is_absolute():
        lock_path = root / lock_path
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    with lock_path.open("a+b") as lock_handle:
        fcntl.flock(lock_handle.fileno(), fcntl.LOCK_SH)
        return _main_locked(argv)


if __name__ == "__main__":
    raise SystemExit(main())
