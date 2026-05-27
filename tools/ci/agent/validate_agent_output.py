#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

from jsonschema import Draft202012Validator
from jsonschema.exceptions import ValidationError as JsonSchemaValidationError

ROOT = Path(__file__).resolve().parents[3]
SCHEMA_PATH = Path(__file__).with_name("agent_output.schema.json")
FORBIDDEN_RUST_PATTERN = (
    r"unwrap\(|expect\(|panic!|todo!|unimplemented!|unreachable!|#\[allow|#!\[allow|let _ ="
)


class AgentValidationError(ValueError):
    """Raised when a structured agent output violates the fail-closed contract."""


def load_schema() -> dict:
    return json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))


def validate_status_contract(obj: dict) -> None:
    status = obj["status"]

    if status == "ok":
        if obj["required_inputs_missing"]:
            raise AgentValidationError("ok with missing inputs")

        if any(assumption["can_affect_correctness"] for assumption in obj.get("assumptions", [])):
            raise AgentValidationError("ok with correctness-affecting assumption")

        if any(not check["passed"] for check in obj.get("checks", [])):
            raise AgentValidationError("ok with failed check")

        if obj.get("errors"):
            raise AgentValidationError("ok with errors present")

    if status in {"blocked", "error"} and obj.get("artifacts"):
        raise AgentValidationError("blocked/error must not produce deployable artifacts")


def artifact_paths(obj: dict) -> list[Path]:
    paths: list[Path] = []
    for artifact in obj.get("artifacts", []):
        path = artifact.get("path")
        if isinstance(path, str) and path:
            paths.append(Path(path))
    return paths


def requires_rust_verification(paths: list[Path]) -> bool:
    rust_suffixes = {".rs"}
    rust_names = {"Cargo.toml", "Cargo.lock", "clippy.toml", "deny.toml"}
    return any(path.suffix in rust_suffixes or path.name in rust_names for path in paths)


def run_command(command: list[str], cwd: Path) -> None:
    result = subprocess.run(command, cwd=cwd, capture_output=True, text=True)
    if result.returncode != 0:
        details = "\n".join(part for part in [result.stdout.strip(), result.stderr.strip()] if part)
        raise AgentValidationError(
            f"command failed ({' '.join(command)}): {details or f'exit {result.returncode}'}"
        )


def check_forbidden_rust_patterns(cwd: Path) -> None:
    result = subprocess.run(
        ["rg", "-n", FORBIDDEN_RUST_PATTERN, "crates"],
        cwd=cwd,
        capture_output=True,
        text=True,
    )
    if result.returncode == 0:
        raise AgentValidationError("forbidden Rust pattern detected:\n" + result.stdout.strip())
    if result.returncode > 1:
        raise AgentValidationError(
            f"rg failed while checking forbidden Rust patterns: {result.stderr.strip()}"
        )


def verify_rust_artifacts(repo_root: Path) -> None:
    run_command(["./scripts/cargow", "fmt", "--all", "--", "--check"], cwd=repo_root)
    run_command(
        [
            "./scripts/cargow",
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--locked",
            "--",
            "-D",
            "warnings",
        ],
        cwd=repo_root,
    )
    run_command(
        ["./scripts/cargow", "test", "--workspace", "--all-features", "--locked"],
        cwd=repo_root,
    )
    run_command(["bash", "scripts/run-cargo-deny.sh"], cwd=repo_root)
    run_command(["python3", "scripts/check_workspace_lints.py"], cwd=repo_root)
    run_command(["bash", "scripts/check-rust-allow-attributes.sh"], cwd=repo_root)
    check_forbidden_rust_patterns(repo_root)


def validate_payload(obj: dict, repo_root: Path, skip_rust_gates: bool) -> None:
    schema = load_schema()
    try:
        Draft202012Validator(schema).validate(obj)
    except JsonSchemaValidationError as error:
        raise AgentValidationError(str(error)) from error
    validate_status_contract(obj)

    if obj["status"] == "ok" and not skip_rust_gates:
        paths = artifact_paths(obj)
        if requires_rust_verification(paths):
            verify_rust_artifacts(repo_root)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("payload", type=Path)
    parser.add_argument("--repo-root", type=Path, default=ROOT)
    parser.add_argument("--skip-rust-gates", action="store_true")
    return parser.parse_args()


def main() -> int:
    try:
        args = parse_args()
        payload = json.loads(args.payload.read_text(encoding="utf-8"))
        validate_payload(payload, args.repo_root.resolve(), args.skip_rust_gates)
    except (AgentValidationError, json.JSONDecodeError, OSError) as error:
        print(str(error), file=sys.stderr)
        return 1

    print("agent output valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
