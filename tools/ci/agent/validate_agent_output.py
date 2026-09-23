#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

from jsonschema import Draft202012Validator
from jsonschema.exceptions import ValidationError as JsonSchemaValidationError

ROOT = Path(__file__).resolve().parents[3]
SCHEMA_PATH = Path(__file__).with_name("agent_output.schema.json")


class AgentValidationError(ValueError):
    """Raised when a structured agent output violates the fail-closed contract."""


def load_schema() -> dict:
    return json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))


def validate_status_contract(obj: dict) -> None:
    required_claims = [claim for claim in obj["claims"] if claim["required"]]
    has_failed = any(claim["status"] == "failed" for claim in required_claims)
    has_blocked = bool(obj["required_inputs_missing"]) or any(
        claim["status"] in {"blocked", "not_run"} for claim in required_claims
    )
    has_correctness_assumption = any(
        assumption["can_affect_correctness"] for assumption in obj["assumptions"]
    )

    if obj["errors"] or has_failed:
        expected = "error"
    elif has_blocked or has_correctness_assumption:
        expected = "blocked"
    else:
        expected = "ok"

    if obj["status"] != expected:
        raise AgentValidationError(
            f"status {obj['status']!r} conflicts with derived status {expected!r}"
        )

    if obj["status"] in {"blocked", "error"} and any(
        artifact["deployable"] for artifact in obj["artifacts"]
    ):
        raise AgentValidationError("blocked/error must not produce deployable artifacts")


def _bound_file(repo_root: Path, raw_path: str, *, label: str) -> Path:
    path = Path(raw_path)
    if path.is_absolute() or ".." in path.parts:
        raise AgentValidationError(f"{label} path must be repository-relative: {raw_path}")
    resolved = (repo_root / path).resolve()
    if resolved == repo_root or repo_root not in resolved.parents:
        raise AgentValidationError(f"{label} path escapes repository: {raw_path}")
    if not resolved.is_file():
        raise AgentValidationError(f"{label} file does not exist: {raw_path}")
    return resolved


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(65536), b""):
            digest.update(block)
    return digest.hexdigest()


def validate_claims(obj: dict, repo_root: Path) -> None:
    names: set[str] = set()
    for claim in obj["claims"]:
        name = claim["name"]
        if name in names:
            raise AgentValidationError(f"duplicate claim name: {name}")
        names.add(name)

        status = claim["status"]
        if status in {"verified", "failed"}:
            if claim["command"] is None:
                raise AgentValidationError(f"{status} claim lacks command: {name}")
            if not claim["evidence"]:
                raise AgentValidationError(f"{status} claim lacks evidence: {name}")
        if status == "verified" and not claim["covered_scope"]:
            raise AgentValidationError(f"verified claim lacks covered scope: {name}")

        for evidence in claim["evidence"]:
            path = _bound_file(repo_root, evidence["path"], label=f"claim {name!r} evidence")
            actual = _sha256(path)
            if actual != evidence["sha256"]:
                raise AgentValidationError(
                    f"claim {name!r} evidence digest mismatch: {evidence['path']}"
                )


def artifact_paths(obj: dict, repo_root: Path) -> list[Path]:
    paths: list[Path] = []
    for artifact in obj.get("artifacts", []):
        paths.append(_bound_file(repo_root, artifact["path"], label="artifact"))
    return paths


def validate_payload(obj: dict, repo_root: Path) -> None:
    schema = load_schema()
    try:
        Draft202012Validator(schema).validate(obj)
    except JsonSchemaValidationError as error:
        raise AgentValidationError(str(error)) from error
    validate_claims(obj, repo_root)
    validate_status_contract(obj)
    artifact_paths(obj, repo_root)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("payload", type=Path)
    parser.add_argument("--repo-root", type=Path, default=ROOT)
    return parser.parse_args()


def main() -> int:
    try:
        args = parse_args()
        payload = json.loads(args.payload.read_text(encoding="utf-8"))
        validate_payload(payload, args.repo_root.resolve())
    except (AgentValidationError, json.JSONDecodeError, OSError) as error:
        print(str(error), file=sys.stderr)
        return 1

    print("agent output valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
