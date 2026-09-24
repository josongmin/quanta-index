#!/usr/bin/env python3
"""Run retrieval proof prerequisites without a shell; emit a separate portable receipt.

This is not a canonical verification receipt or a retrieval qualification rail.
The existing Just/receipt consumer remains authoritative until it accepts this format.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

try:
    from tools.benchmark.retrieval import contract_proof, proof_inventory, sdk_proof
except ModuleNotFoundError:  # direct script invocation
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    from tools.benchmark.retrieval import contract_proof, proof_inventory, sdk_proof

ROOT = Path(__file__).resolve().parents[3]
PACKAGE = "quanta-index-retrieval-bench"
FLAGS = ["--all-features", "--locked"]
FORMAT = ["--message-format", "libtest-json-plus", "--message-format-version", "0.1"]


def _sha(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _json(path: Path) -> object:
    def unique(pairs: list[tuple[str, object]]) -> dict[str, object]:
        result: dict[str, object] = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"duplicate JSON key: {key}")
            result[key] = value
        return result

    def constant(value: str) -> None:
        raise ValueError(f"invalid JSON constant: {value}")

    return json.loads(path.read_bytes(), object_pairs_hook=unique, parse_constant=constant)


def _write(path: Path, data: bytes) -> None:
    with path.open("xb") as stream:
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())


def _write_json(path: Path, value: object) -> None:
    _write(path, (json.dumps(value, sort_keys=True, indent=2) + "\n").encode())


def _git(*args: str) -> str:
    completed = subprocess.run(["git", *args], cwd=ROOT, capture_output=True, check=True, text=True)
    return completed.stdout.strip()


def _source_revision() -> str:
    if _git("status", "--porcelain=v1", "--untracked-files=all"):
        raise ValueError("portable proof requires a clean source worktree")
    revision = _git("rev-parse", "HEAD")
    if len(revision) != 40 or any(c not in "0123456789abcdef" for c in revision):
        raise ValueError("invalid Git revision")
    if os.environ.get("GITHUB_SHA") and os.environ["GITHUB_SHA"] != revision:
        raise ValueError("GITHUB_SHA differs from checked-out HEAD")
    return revision


def _tools() -> dict[str, dict[str, str]]:
    paths = {"python": Path(sys.executable)}
    for name in ("cargo", "cargo-nextest", "rustc", "git"):
        found = shutil.which(name)
        if found is None:
            raise ValueError(f"required executable unavailable: {name}")
        paths[name] = Path(found)
    result = {}
    for name, path in paths.items():
        invocation = path.absolute()
        resolved = path.resolve(strict=True)
        if not resolved.is_file():
            raise ValueError(f"required executable is not a file: {resolved}")
        completed = subprocess.run(
            [str(invocation), "-Vv" if name == "rustc" else "--version"],
            cwd=ROOT,
            capture_output=True,
            check=True,
            text=True,
        )
        version = completed.stdout.strip()
        if not version:
            raise ValueError(f"required executable has no version identity: {name}")
        result[name] = {
            "path": str(invocation),
            "realpath": str(resolved),
            "sha256": _sha(invocation),
            "version": version,
        }
    return result


def _os_identity() -> dict[str, str]:
    return {
        "system": platform.system(),
        "release": platform.release(),
        "machine": platform.machine(),
        "python_version": platform.python_version(),
    }


def _command(argv: list[str]) -> str:
    return json.dumps(argv, ensure_ascii=False, separators=(",", ":"))


def _environment_digest(environment: dict[str, str]) -> str:
    return hashlib.sha256(
        json.dumps(environment, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def _run(
    name: str,
    argv: list[str],
    out: Path,
    commands: list[dict[str, object]],
    *,
    env_overrides: dict[str, str] | None = None,
) -> bytes:
    if not argv or not Path(argv[0]).is_absolute():
        raise ValueError(f"{name} requires an absolute executable")
    overrides = {"CARGO_NET_OFFLINE": "true", **(env_overrides or {})}
    environment = {**os.environ, **overrides}
    completed = subprocess.run(
        argv,
        cwd=ROOT,
        env=environment,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        raise ValueError(
            f"{name} failed with exit {completed.returncode}: {completed.stderr[-2000:]!r}"
        )
    stdout = f"{name}.stdout"
    stderr = f"{name}.stderr"
    _write(out / stdout, completed.stdout)
    _write(out / stderr, completed.stderr)
    commands.append(
        {
            "name": name,
            "argv": argv,
            "cwd": str(ROOT),
            "environment": overrides,
            "environment_sha256": _environment_digest(environment),
            "exit_code": completed.returncode,
            "stdout": stdout,
            "stdout_sha256": _sha(out / stdout),
            "stderr": stderr,
            "stderr_sha256": _sha(out / stderr),
        }
    )
    return completed.stdout


def _target_dir(cargo: str, out: Path, commands: list[dict[str, object]]) -> Path:
    raw = _run(
        "metadata",
        [cargo, "metadata", "--format-version", "1", "--no-deps", "--locked"],
        out,
        commands,
    )
    payload = json.loads(raw)
    value = payload.get("target_directory") if isinstance(payload, dict) else None
    if not isinstance(value, str) or not value or not Path(value).is_absolute():
        raise ValueError("cargo metadata lacks an absolute target directory")
    return Path(value).resolve()


def _artifact(out: Path, name: str, evidence: dict[str, str]) -> Path:
    path = out / name
    if not path.is_file():
        raise ValueError(f"missing proof artifact: {name}")
    evidence[name] = _sha(path)
    return path


def _expected_commands(
    rail: str, out: Path, tools: dict[str, dict[str, str]], binaries: dict[str, dict[str, str]]
) -> list[tuple[str, list[str], dict[str, str]]]:
    python = tools["python"]["path"]
    cargo = tools["cargo"]["path"]
    base = {"CARGO_NET_OFFLINE": "true"}
    test_env = {**base, "NEXTEST_EXPERIMENTAL_LIBTEST_JSON": "1"}
    if rail == "contract":
        selector = ["-p", PACKAGE, "--lib", "--test", "chunking_contract", *FLAGS]
        return [
            (
                "python-collection",
                [
                    python,
                    str(ROOT / "tools/benchmark/retrieval/proof_inventory.py"),
                    "--out",
                    str(out / "python-inventory.json"),
                ],
                base,
            ),
            (
                "rust-collection",
                [cargo, "nextest", "list", *selector, "--message-format", "json"],
                base,
            ),
            (
                "python-test",
                [
                    python,
                    "-m",
                    "pytest",
                    proof_inventory.PYTHON_SELECTOR,
                    "-q",
                    f"--junitxml={out / 'python-junit.xml'}",
                ],
                base,
            ),
            ("rust-test", [cargo, "nextest", "run", *selector, *FORMAT], test_env),
        ]
    selector = ["-p", PACKAGE, "--test", "sdk_roundtrip", *FLAGS]
    return [
        (
            "searchd-build",
            [
                cargo,
                "build",
                "-p",
                "quanta-index-searchd-runtime",
                "--bin",
                "quanta-index-searchd",
                "--locked",
            ],
            base,
        ),
        ("runner-build", [cargo, "build", "-p", PACKAGE, "--bin", PACKAGE, "--locked"], base),
        ("metadata", [cargo, "metadata", "--format-version", "1", "--no-deps", "--locked"], base),
        ("sdk-collection", [cargo, "nextest", "list", *selector, "--message-format", "json"], base),
        (
            "sdk-test",
            [cargo, "nextest", "run", *selector, *FORMAT],
            {
                **test_env,
                "QUANTA_BENCH_SDK_EVIDENCE_DIR": str(out),
                "QUANTA_INDEX_SEARCHD_BIN": binaries["searchd"]["path"],
            },
        ),
    ]


def produce(rail: str, out: Path) -> Path:
    if rail not in {"contract", "sdk"}:
        raise ValueError(f"unknown rail: {rail}")
    out = out.absolute()
    if out == ROOT or ROOT in out.parents:
        raise ValueError("portable proof output must be outside the source worktree")
    revision = _source_revision()
    tools = _tools()
    out.mkdir(parents=True, exist_ok=False)
    commands: list[dict[str, object]] = []
    evidence: dict[str, str] = {}
    python = tools["python"]["path"]
    cargo = tools["cargo"]["path"]
    environment = {"NEXTEST_EXPERIMENTAL_LIBTEST_JSON": "1"}
    if rail == "contract":
        _run(
            "python-collection",
            [
                python,
                str(ROOT / "tools/benchmark/retrieval/proof_inventory.py"),
                "--out",
                str(out / "python-inventory.json"),
            ],
            out,
            commands,
        )
        python_inventory = _artifact(out, "python-inventory.json", evidence)
        proof_inventory.verify_inventory_authority(python_inventory, "python")
        selector = ["-p", PACKAGE, "--lib", "--test", "chunking_contract", *FLAGS]
        _run(
            "rust-collection",
            [cargo, "nextest", "list", *selector, "--message-format", "json"],
            out,
            commands,
        )
        rust_inventory = out / "rust-inventory.json"
        _write(rust_inventory, (out / "rust-collection.stdout").read_bytes())
        _artifact(out, "rust-inventory.json", evidence)
        proof_inventory.verify_inventory_authority(rust_inventory, "rust")
        pytest_argv = [
            python,
            "-m",
            "pytest",
            proof_inventory.PYTHON_SELECTOR,
            "-q",
            f"--junitxml={out / 'python-junit.xml'}",
        ]
        _run("python-test", pytest_argv, out, commands)
        junit = _artifact(out, "python-junit.xml", evidence)
        nextest_argv = [cargo, "nextest", "run", *selector, *FORMAT]
        _run("rust-test", nextest_argv, out, commands, env_overrides=environment)
        rust_events = out / "rust-test.stdout"
        python_summary = contract_proof.pytest_summary(
            junit, python_inventory, command=_command(pytest_argv)
        )
        rust_summary = contract_proof.nextest_summary(
            rust_events, rust_inventory, command=_command(nextest_argv)
        )
        _write_json(out / "contract_python_results.json", python_summary)
        _write_json(out / "contract_rust_results.json", rust_summary)
        _artifact(out, "contract_python_results.json", evidence)
        _artifact(out, "contract_rust_results.json", evidence)
        binaries: dict[str, dict[str, str]] = {}
    else:
        _run(
            "searchd-build",
            [
                cargo,
                "build",
                "-p",
                "quanta-index-searchd-runtime",
                "--bin",
                "quanta-index-searchd",
                "--locked",
            ],
            out,
            commands,
        )
        _run(
            "runner-build",
            [cargo, "build", "-p", PACKAGE, "--bin", PACKAGE, "--locked"],
            out,
            commands,
        )
        target = _target_dir(cargo, out, commands)
        suffix = ".exe" if os.name == "nt" else ""
        searchd = target / "debug" / f"quanta-index-searchd{suffix}"
        runner = target / "debug" / f"{PACKAGE}{suffix}"
        binaries = {}
        for name, path in (("searchd", searchd), ("runner", runner)):
            if not path.is_file():
                raise ValueError(f"missing built binary: {path}")
            binaries[name] = {"path": str(path), "sha256": _sha(path)}
        selector = ["-p", PACKAGE, "--test", "sdk_roundtrip", *FLAGS]
        _run(
            "sdk-collection",
            [cargo, "nextest", "list", *selector, "--message-format", "json"],
            out,
            commands,
        )
        inventory = out / "nextest-inventory.json"
        _write(inventory, (out / "sdk-collection.stdout").read_bytes())
        _artifact(out, "nextest-inventory.json", evidence)
        proof_inventory.verify_inventory_authority(inventory, "sdk")
        nextest_argv = [cargo, "nextest", "run", *selector, *FORMAT]
        _run(
            "sdk-test",
            nextest_argv,
            out,
            commands,
            env_overrides={
                **environment,
                "QUANTA_BENCH_SDK_EVIDENCE_DIR": str(out),
                "QUANTA_INDEX_SEARCHD_BIN": str(searchd),
            },
        )
        record = _artifact(out, "actual-runner-record.json", evidence)
        summary = sdk_proof.build_summary(
            record,
            out / "sdk-test.stdout",
            runner,
            inventory,
            command=_command(nextest_argv),
            searchd_path=searchd,
        )
        _write_json(out / "sdk_results.json", summary)
        _artifact(out, "sdk_results.json", evidence)
    receipt = {
        "schema_version": 1,
        "rail": rail,
        "revision": revision,
        "os": _os_identity(),
        "tools": tools,
        "binaries": binaries,
        "commands": commands,
        "evidence": evidence,
    }
    path = out / "portable-proof-receipt.json"
    _write_json(path, receipt)
    validate(path)
    return path


def _inside(out: Path, value: str) -> Path:
    if not isinstance(value, str) or not value or Path(value).is_absolute():
        raise ValueError("invalid relative artifact path")
    path = (out / value).resolve(strict=True)
    if path.parent != out or not path.is_file():
        raise ValueError(f"artifact escaped proof root: {value}")
    return path


def validate(receipt_path: Path) -> dict[str, object]:
    out = receipt_path.parent.resolve()
    receipt = _json(receipt_path)
    if (
        not isinstance(receipt, dict)
        or set(receipt)
        != {"schema_version", "rail", "revision", "os", "tools", "binaries", "commands", "evidence"}
        or type(receipt["schema_version"]) is not int
        or receipt["schema_version"] != 1
        or receipt["rail"] not in {"contract", "sdk"}
    ):
        raise ValueError("invalid portable proof receipt shape")
    if receipt["revision"] != _source_revision() or receipt["os"] != _os_identity():
        raise ValueError("portable proof source or OS identity changed")
    if receipt["tools"] != _tools():
        raise ValueError("portable proof tool binary identity changed")
    binaries = receipt["binaries"]
    if not isinstance(binaries, dict) or set(binaries) != (
        {"runner", "searchd"} if receipt["rail"] == "sdk" else set()
    ):
        raise ValueError("invalid proof binary identities")
    for binary in binaries.values():
        if (
            not isinstance(binary, dict)
            or set(binary) != {"path", "sha256"}
            or _sha(Path(binary["path"])) != binary["sha256"]
        ):
            raise ValueError("proof binary identity changed")
    expected_commands = _expected_commands(receipt["rail"], out, receipt["tools"], binaries)
    expected_names = [name for name, _, _ in expected_commands]
    commands = receipt["commands"]
    if (
        not isinstance(commands, list)
        or [row.get("name") for row in commands if isinstance(row, dict)] != expected_names
        or len(commands) != len(expected_names)
    ):
        raise ValueError("missing or reordered proof commands")
    for row, (_, expected_argv, expected_env) in zip(commands, expected_commands):
        if set(row) != {
            "name",
            "argv",
            "cwd",
            "environment",
            "environment_sha256",
            "exit_code",
            "stdout",
            "stdout_sha256",
            "stderr",
            "stderr_sha256",
        }:
            raise ValueError("invalid proof command shape")
        argv = row["argv"]
        if (
            not isinstance(argv, list)
            or not argv
            or not all(isinstance(arg, str) and arg for arg in argv)
            or row["cwd"] != str(ROOT)
            or type(row["exit_code"]) is not int
            or row["exit_code"] != 0
            or not isinstance(row["environment"], dict)
            or row["environment"].get("CARGO_NET_OFFLINE") != "true"
        ):
            raise ValueError("invalid proof command identity")
        if argv != expected_argv or row["environment"] != expected_env:
            raise ValueError("proof command or environment differs from prescribed rail")
        if row["environment_sha256"] != _environment_digest({**os.environ, **expected_env}):
            raise ValueError("proof command environment identity changed")
        for role in ("stdout", "stderr"):
            if (
                row[role] != f"{row['name']}.{role}"
                or _sha(_inside(out, row[role])) != row[f"{role}_sha256"]
            ):
                raise ValueError("proof command output changed")
    evidence = receipt["evidence"]
    expected_evidence = (
        {
            "python-inventory.json",
            "rust-inventory.json",
            "python-junit.xml",
            "contract_python_results.json",
            "contract_rust_results.json",
        }
        if receipt["rail"] == "contract"
        else {"nextest-inventory.json", "actual-runner-record.json", "sdk_results.json"}
    )
    if not isinstance(evidence, dict) or set(evidence) != expected_evidence:
        raise ValueError("invalid portable proof evidence set")
    for name, digest in evidence.items():
        if _sha(_inside(out, name)) != digest:
            raise ValueError(f"proof evidence changed: {name}")
    if receipt["rail"] == "contract":
        proof_inventory.verify_inventory_authority(out / "python-inventory.json", "python")
        proof_inventory.verify_inventory_authority(out / "rust-inventory.json", "rust")
        if (out / "rust-inventory.json").read_bytes() != (
            out / "rust-collection.stdout"
        ).read_bytes():
            raise ValueError("rust collection output differs from inventory")
        python_argv = commands[2]["argv"]
        rust_argv = commands[3]["argv"]
        expected = {
            "contract_python_results.json": contract_proof.pytest_summary(
                out / "python-junit.xml",
                out / "python-inventory.json",
                command=_command(python_argv),
            ),
            "contract_rust_results.json": contract_proof.nextest_summary(
                out / "rust-test.stdout", out / "rust-inventory.json", command=_command(rust_argv)
            ),
        }
    else:
        proof_inventory.verify_inventory_authority(out / "nextest-inventory.json", "sdk")
        if (out / "nextest-inventory.json").read_bytes() != (
            out / "sdk-collection.stdout"
        ).read_bytes():
            raise ValueError("SDK collection output differs from inventory")
        metadata = _json(out / "metadata.stdout")
        target = metadata.get("target_directory") if isinstance(metadata, dict) else None
        suffix = ".exe" if os.name == "nt" else ""
        if (
            not isinstance(target, str)
            or not Path(target).is_absolute()
            or (
                binaries["searchd"]["path"]
                != str(Path(target).resolve() / "debug" / f"quanta-index-searchd{suffix}")
                or binaries["runner"]["path"]
                != str(Path(target).resolve() / "debug" / f"{PACKAGE}{suffix}")
            )
        ):
            raise ValueError("SDK binary paths differ from cargo metadata")
        if (
            commands[4]["environment"].get("QUANTA_INDEX_SEARCHD_BIN")
            != binaries["searchd"]["path"]
        ):
            raise ValueError("SDK daemon pin differs from binary identity")
        expected = {
            "sdk_results.json": sdk_proof.build_summary(
                out / "actual-runner-record.json",
                out / "sdk-test.stdout",
                Path(binaries["runner"]["path"]),
                out / "nextest-inventory.json",
                command=_command(commands[4]["argv"]),
                searchd_path=Path(binaries["searchd"]["path"]),
            )
        }
    for name, summary in expected.items():
        if _json(out / name) != summary:
            raise ValueError(f"proof summary differs from machine evidence: {name}")
    return receipt


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    run = sub.add_parser("run")
    run.add_argument("--rail", choices=("contract", "sdk"), required=True)
    run.add_argument("--out", required=True, type=Path)
    verify = sub.add_parser("verify")
    verify.add_argument("--receipt", required=True, type=Path)
    args = parser.parse_args()
    try:
        path = produce(args.rail, args.out.resolve()) if args.action == "run" else args.receipt
        validate(path)
    except (OSError, ValueError, subprocess.CalledProcessError, SystemExit) as error:
        raise SystemExit(f"portable proof refused: {error}") from error
    print(path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
