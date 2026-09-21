#!/usr/bin/env python3
"""Validate a SEP-21 lane handoff against Git and immutable proof authority."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import subprocess
import sys
from pathlib import Path
from types import ModuleType
from typing import Any

import jsonschema

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python 3.10 compatibility
    import tomli as tomllib


ROOT = Path(__file__).resolve().parents[3]
SCHEMA_PATH = (
    ROOT / "docs/plans/sep-21-search-plane-sota-hardening/tickets/handoffs/lane-handoff.schema.json"
)
PROOF_SCHEMA_PATH = ROOT / "tools/ci/proof-manifest.schema.json"
PROOF_REGISTRY_PATH = ROOT / "tools/ci/proof-authority.toml"
PROOF_CHECKER_PATH = ROOT / "tools/ci/lint/check-proof-authority.py"

# Canonical lane closeout policy. Order is authoritative for handoff proofs.
HANDOFF_POLICIES: dict[str, dict[str, Any]] = {
    "P00": {"ticket": "S21-00+S21-13A", "required": ["p00-authority-freeze"]},
    "P01": {"ticket": "S21-01", "required": ["p01-canonical-identity"]},
    "P02A": {"ticket": "S21-03", "required": ["p02a-repomap-compiler"]},
    "P02B": {"ticket": "S21-04", "required": ["p02b-operation-journal"]},
    "P02I": {
        "ticket": "S21-03+S21-04",
        "required": ["p02a-repomap-compiler", "p02b-operation-journal"],
    },
    "P03": {
        "ticket": "S21-01+S21-02",
        "required": ["p03-candidate-activation-owner"],
        "release": ["p03-candidate-activation"],
    },
    "P04": {
        "ticket": "S21-05",
        "required": ["p04-read-view-lifetime-owner"],
        "release": ["p04-read-view-lifetime"],
    },
    "P05": {
        "ticket": "S21-06",
        "required": ["p05-query-truth-owner"],
        "release": ["p05-query-truth"],
    },
    "P06": {
        "ticket": "S21-07",
        "required": ["p06-sdk-binding-owner"],
        "release": ["p06-sdk-binding"],
    },
    "P07": {
        "ticket": "S21-08",
        "required": ["p07-provider-boundary-owner"],
        "release": ["p07-provider-boundary"],
    },
    "P08": {
        "ticket": "S21-09",
        "required": ["p08-runtime-supervisor-owner"],
        "release": ["p08-runtime-supervisor"],
    },
    "P09": {
        "ticket": "S21-10",
        "required": ["p09-control-readiness-owner"],
        "release": ["p09-control-readiness"],
    },
    "P10": {
        "ticket": "S21-11",
        "required": ["p10-state-migration-owner"],
        "release": ["p10-state-migration"],
    },
    "P11": {
        "ticket": "S21-12",
        "required": ["p11-cross-repo-cutover"],
        "release": ["p11-deployment", "p11-activation", "p11-rollback"],
    },
    "P12A": {
        "ticket": "S21-13",
        "required": ["p12a-proof-infrastructure"],
        "deferred": ["p12-final-qualification"],
    },
    "P12": {"ticket": "S21-13", "required": ["p12-final-qualification"]},
}


def _load_module(name: str, path: Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise ValueError(f"cannot load module: {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def _read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def _read_toml(path: Path) -> dict[str, Any]:
    with path.open("rb") as handle:
        value = tomllib.load(handle)
    if not isinstance(value, dict):
        raise ValueError(f"TOML root is not an object: {path}")
    return value


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _repo_path(root: Path, value: str, *, label: str) -> Path:
    candidate = Path(value)
    if candidate.is_absolute() or ".." in candidate.parts or candidate.as_posix() != value:
        raise ValueError(f"{label} must be canonical repo-relative: {value!r}")
    resolved = (root / candidate).resolve()
    try:
        resolved.relative_to(root)
    except ValueError as error:
        raise ValueError(f"{label} escapes repository root: {value!r}") from error
    return resolved


def _git(root: Path, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", "-C", str(root), *args],
        check=check,
        capture_output=True,
        text=True,
    )


def _is_ancestor(root: Path, ancestor: str, descendant: str) -> bool:
    return (
        _git(root, "merge-base", "--is-ancestor", ancestor, descendant, check=False).returncode == 0
    )


def _patch_id(root: Path, commit: str) -> str:
    shown = _git(root, "show", "--pretty=format:", "--binary", commit).stdout
    completed = subprocess.run(
        ["git", "-C", str(root), "patch-id", "--stable"],
        input=shown,
        check=True,
        capture_output=True,
        text=True,
    )
    fields = completed.stdout.split()
    if not fields:
        raise ValueError(f"commit has no stable patch identity: {commit}")
    return fields[0]


def _git_delta_paths(root: Path, base_sha: str, result_sha: str) -> list[str]:
    completed = subprocess.run(
        [
            "git",
            "-C",
            str(root),
            "diff",
            "--name-only",
            "--no-renames",
            "-z",
            base_sha,
            result_sha,
            "--",
        ],
        check=True,
        capture_output=True,
    )
    return sorted(
        part.decode("utf-8", errors="surrogateescape")
        for part in completed.stdout.split(b"\0")
        if part
    )


def _git_blob_sha256(root: Path, result_sha: str, path: str) -> str:
    candidate = Path(path)
    if candidate.is_absolute() or ".." in candidate.parts or candidate.as_posix() != path:
        raise ValueError(f"exported contract path is not canonical: {path!r}")
    completed = subprocess.run(
        ["git", "-C", str(root), "show", f"{result_sha}:{path}"],
        check=False,
        capture_output=True,
    )
    if completed.returncode != 0:
        raise ValueError(f"exported contract is absent from result commit: {path}")
    return hashlib.sha256(completed.stdout).hexdigest()


def _validate_lane_policy(payload: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    lane = payload["lane"]
    policy = HANDOFF_POLICIES.get(lane)
    if policy is None:
        return [f"lane has no canonical handoff policy: {lane}"]
    if payload["ticket"] != policy["ticket"]:
        errors.append(f"ticket differs from {lane} authority: expected {policy['ticket']}")
    required = policy.get("required", [])
    release = policy.get("release", [])
    deferred = policy.get("deferred", [])
    expected_ids = [*required, *release, *deferred]
    proof_by_id = {item["id"]: item for item in payload["proofs"]}
    if [item["id"] for item in payload["proofs"]] != expected_ids:
        errors.append(f"proof IDs differ from {lane} authority: expected {expected_ids}")
        return errors
    if payload["status"] == "BLOCKED":
        return errors
    required_recorded = all(proof_by_id[item]["status"] == "RECORDED" for item in required)
    release_not_run = [item for item in release if proof_by_id[item]["status"] == "NOT_RUN"]
    deferred_not_run = all(proof_by_id[item]["status"] == "NOT_RUN" for item in deferred)
    if payload["status"] == "IMPLEMENTATION_DONE":
        if required_recorded:
            errors.append("IMPLEMENTATION_DONE cannot contain every required owner proof")
        return errors
    if not required_recorded:
        errors.append(f"{payload['status']} requires every {lane} owner proof to be RECORDED")
    if not deferred_not_run:
        errors.append(f"{lane} deferred proof must remain NOT_RUN")
    expected_status = "RELEASE_PROOF_PENDING" if release_not_run else "OWNER_PROOF_GREEN"
    if payload["status"] != expected_status:
        errors.append(f"status is not derived from {lane} proof states: expected {expected_status}")
    return errors


def validate_handoff(
    payload: Any,
    *,
    handoff_path: Path,
    root: Path,
    require_result_head: bool = False,
) -> list[str]:
    errors: list[str] = []
    schema_path = root / SCHEMA_PATH.relative_to(ROOT)
    proof_schema_path = root / PROOF_SCHEMA_PATH.relative_to(ROOT)
    proof_registry_path = root / PROOF_REGISTRY_PATH.relative_to(ROOT)
    proof_checker_path = root / PROOF_CHECKER_PATH.relative_to(ROOT)
    schema = _read_json(schema_path)
    validator = jsonschema.Draft202012Validator(schema)
    for error in sorted(validator.iter_errors(payload), key=lambda item: list(item.path)):
        location = ".".join(str(part) for part in error.path) or "root"
        errors.append(f"schema {location}: {error.message}")
    if not isinstance(payload, dict) or errors:
        return errors

    proof_checker = _load_module("quanta_check_proof_authority_handoff", proof_checker_path)
    proof_schema = _read_json(proof_schema_path)
    registry = _read_toml(proof_registry_path)
    registry_findings = proof_checker.check_registry(registry, root=root, path=proof_registry_path)
    if registry_findings:
        return [f"proof registry is invalid: {finding.message}" for finding in registry_findings]
    proof_by_id = {
        proof["id"]: proof
        for proof in registry.get("proofs", [])
        if isinstance(proof, dict) and isinstance(proof.get("id"), str)
    }

    base_sha = payload["base_sha"]
    result_sha = payload["result_sha"]
    for label, sha in (("base_sha", base_sha), ("result_sha", result_sha)):
        if _git(root, "cat-file", "-e", f"{sha}^{{commit}}", check=False).returncode != 0:
            errors.append(f"{label} is not a local commit: {sha}")
    if not errors and not _is_ancestor(root, base_sha, result_sha):
        errors.append("base_sha is not an ancestor of result_sha")
    if require_result_head and _git(root, "rev-parse", "HEAD").stdout.strip() != result_sha:
        errors.append("result_sha is not current HEAD")

    expected_name = f"{payload['lane']}.json"
    if handoff_path.name != expected_name:
        errors.append(f"handoff filename differs from lane authority: expected {expected_name}")
    errors.extend(_validate_lane_policy(payload))
    if not errors:
        actual_write_set = _git_delta_paths(root, base_sha, result_sha)
        if sorted(payload["write_set"]) != actual_write_set:
            errors.append(
                f"write_set differs from exact base..result Git delta: expected {actual_write_set}"
            )
    for contract in payload["exported_contracts"]:
        path, separator, claimed = contract.rpartition("@sha256:")
        if not separator:
            errors.append(f"exported contract has no digest binding: {contract!r}")
            continue
        try:
            actual = _git_blob_sha256(root, result_sha, path)
        except ValueError as error:
            errors.append(str(error))
            continue
        if claimed != actual:
            errors.append(f"exported contract digest differs from result commit: {path}")

    proof_ids = [item["id"] for item in payload["proofs"]]
    if len(proof_ids) != len(set(proof_ids)):
        errors.append("proof IDs are not unique")
    not_run_ids = [item["id"] for item in payload["proofs"] if item["status"] == "NOT_RUN"]
    if len(payload["not_run"]) != len(set(payload["not_run"])):
        errors.append("not_run entries are not unique")
    if payload["not_run"] != not_run_ids:
        errors.append("not_run must exactly equal NOT_RUN proof IDs in proof order")

    repositories = payload.get("paired_repositories")
    paired_checkouts = (
        {item["identity"]: Path(item["root"]).resolve() for item in repositories}
        if isinstance(repositories, list)
        else {}
    )

    exact_pair_manifests: list[dict[str, Any]] = []
    for item in payload["proofs"]:
        if item["status"] != "RECORDED":
            continue
        if not (item["selected"] == item["executed"] == item["passed"]):
            errors.append(f"proof {item['id']} must satisfy selected == executed == passed")
        authority = proof_by_id.get(item["id"])
        if authority is None:
            errors.append(f"proof {item['id']} is not registered")
            continue
        if item["command"] != authority["command"]:
            errors.append(f"proof {item['id']} command differs from registry")
        if item["required_host"] != authority["required_host"]:
            errors.append(f"proof {item['id']} required_host differs from registry")
        try:
            manifest_path = _repo_path(root, item["manifest"], label="proof manifest")
        except ValueError as error:
            errors.append(str(error))
            continue
        if not manifest_path.is_file():
            errors.append(f"proof manifest is missing: {item['manifest']}")
            continue
        manifest_digest = _sha256(manifest_path)
        if manifest_digest != item["manifest_sha256"]:
            errors.append(f"proof {item['id']} manifest digest mismatch")
            continue
        manifest = _read_json(manifest_path)
        if not isinstance(manifest, dict):
            errors.append(f"proof {item['id']} manifest root is not an object")
            continue
        try:
            expected_path = proof_checker.proof_archive_relative_path(
                manifest,
                manifest_digest,
            )
        except (KeyError, TypeError, ValueError) as error:
            errors.append(f"proof {item['id']} archive identity is invalid: {error}")
            continue
        if item["manifest"] != expected_path:
            errors.append(f"proof {item['id']} manifest is not its immutable archive path")
        if manifest.get("proof_id") != item["id"]:
            errors.append(f"proof {item['id']} manifest proof_id mismatch")
            continue
        if isinstance(manifest.get("source_pair"), dict):
            exact_pair_manifests.append(manifest)
        if manifest.get("status") != "passed":
            errors.append(f"proof {item['id']} manifest status is not passed")
        source = manifest.get("source", {})
        if source.get("head") != result_sha:
            errors.append(f"proof {item['id']} source HEAD differs from handoff result_sha")
        if source.get("dirty_digest") != f"sha256:{payload['dirty_digest']}":
            errors.append(f"proof {item['id']} dirty digest differs from handoff")
        if manifest.get("counts") != {
            "selected": item["selected"],
            "executed": item["executed"],
            "passed": item["passed"],
            "failed": item["failed"],
            "ignored": item["ignored"],
        }:
            errors.append(f"proof {item['id']} counts differ from manifest")
        errors.extend(
            finding.message
            for finding in proof_checker.check_manifest(
                manifest,
                manifest_path=manifest_path,
                proof=authority,
                schema=proof_schema,
                root=root,
                bind_source=require_result_head,
                paired_checkouts=paired_checkouts or None,
                proof_by_id=proof_by_id,
            )
        )

    if isinstance(repositories, list):
        quanta = repositories[0]
        if quanta["base_sha"] != base_sha:
            errors.append("top-level base_sha differs from paired quanta entry")
        if quanta["result_sha"] != result_sha:
            errors.append("top-level result_sha differs from paired quanta entry")
        if quanta["dirty_digest"] != payload["dirty_digest"]:
            errors.append("top-level dirty_digest differs from paired quanta entry")
        pair = repositories[1]
        for manifest in exact_pair_manifests:
            source_pair = manifest["source_pair"]
            if pair["identity"] != source_pair["repository"]:
                errors.append("paired repository identity differs from proof source_pair")
            if pair["result_sha"] != source_pair["source"]["head"]:
                errors.append("paired repository result_sha differs from proof source_pair")
            if f"sha256:{pair['dirty_digest']}" != source_pair["source"]["dirty_digest"]:
                errors.append("paired repository dirty_digest differs from proof source_pair")
            if pair["dependency_root_digest"] != source_pair["dependency_lock"]["sha256"]:
                errors.append(
                    "paired repository dependency_root_digest differs from proof source_pair"
                )
        for repository in repositories:
            push = repository["push"]
            if push["result"] == "PUSHED" and push["remote_sha"] != repository["result_sha"]:
                errors.append(
                    f"paired repository {repository['identity']} remote_sha differs from result_sha"
                )
            if require_result_head:
                checkout = Path(repository["root"]).resolve()
                if not checkout.is_dir():
                    errors.append(f"paired repository root is missing: {checkout}")
                    continue
                for label in ("base_sha", "result_sha"):
                    sha = repository[label]
                    if (
                        _git(
                            checkout, "cat-file", "-e", f"{sha}^{{commit}}", check=False
                        ).returncode
                        != 0
                    ):
                        errors.append(
                            f"paired repository {repository['identity']} {label} is not a local commit"
                        )
                if not _is_ancestor(checkout, repository["base_sha"], repository["result_sha"]):
                    errors.append(
                        f"paired repository {repository['identity']} base is not an ancestor of result"
                    )
                if _git(checkout, "rev-parse", "HEAD").stdout.strip() != repository["result_sha"]:
                    errors.append(
                        f"paired repository {repository['identity']} result_sha is not current HEAD"
                    )
                live_source = proof_checker.source_snapshot(checkout)
                if live_source["dirty_digest"] != f"sha256:{repository['dirty_digest']}":
                    errors.append(
                        f"paired repository {repository['identity']} dirty digest is not current"
                    )
                lock_path = checkout / "Cargo.lock"
                if (
                    not lock_path.is_file()
                    or _sha256(lock_path) != repository["dependency_root_digest"]
                ):
                    errors.append(
                        f"paired repository {repository['identity']} dependency root is not current"
                    )

    integration_commits = payload.get("integration_commits")
    if isinstance(integration_commits, list):
        for item in integration_commits:
            if item["integration_mode"] == "MERGE":
                if item["original_sha"] != item["applied_sha"]:
                    errors.append(f"{item['lane']} MERGE must preserve original_sha as applied_sha")
                if not _is_ancestor(root, item["original_sha"], result_sha):
                    errors.append(
                        f"{item['lane']} original commit is not an ancestor of result_sha"
                    )
            else:
                if not _is_ancestor(root, item["applied_sha"], result_sha):
                    errors.append(f"{item['lane']} applied commit is not an ancestor of result_sha")
                try:
                    if _patch_id(root, item["original_sha"]) != _patch_id(
                        root, item["applied_sha"]
                    ):
                        errors.append(f"{item['lane']} cherry-pick patch identity differs")
                except (subprocess.CalledProcessError, ValueError) as error:
                    errors.append(f"{item['lane']} patch identity is unavailable: {error}")
    return errors


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("handoff", type=Path)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--require-result-head", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    root = args.root.resolve()
    handoff_path = args.handoff if args.handoff.is_absolute() else root / args.handoff
    try:
        payload = _read_json(handoff_path)
        errors = validate_handoff(
            payload,
            handoff_path=handoff_path,
            root=root,
            require_result_head=args.require_result_head,
        )
    except (OSError, ValueError, json.JSONDecodeError, tomllib.TOMLDecodeError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2
    if errors:
        for error in errors:
            print(f"ERROR: {handoff_path}: {error}", file=sys.stderr)
        return 1
    print(f"OK: {handoff_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
