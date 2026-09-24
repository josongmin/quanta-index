#!/usr/bin/env python3
"""Produce pre-receipt execution context and context-bound retrieval receipts.

The verdict does not yet consume the execution context or its receipt input role.
Windows production remains blocked by Bash-only source_closure and cargow.
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
    from tools.ci import source_closure
except ModuleNotFoundError:  # direct script invocation
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    from tools.benchmark.retrieval import contract_proof, proof_inventory, sdk_proof
    from tools.ci import source_closure

ROOT = Path(__file__).resolve().parents[3]
PACKAGE = "quanta-index-retrieval-bench"
FLAGS = ["--all-features", "--locked"]
FORMAT = ["--message-format", "libtest-json-plus", "--message-format-version", "0.1"]
SOURCE_CLOSURE_SCRIPT = ROOT / "tools/ci/source_closure.py"
RECEIPT_WRITER = ROOT / "tools/ci/write-verification-receipt.py"
WRAPPER = ROOT / "scripts/cargow"
PYTHON_COMMAND = "python3 -m pytest tools/ci/tests/test_retrieval_benchmark.py -q"
RUST_COMMAND = (
    "./scripts/cargow nextest run -p quanta-index-retrieval-bench "
    "--lib --test chunking_contract --all-features --locked"
)
SDK_COMMAND = "just retrieval-sdk-proof"


def _sha(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _is_sha256(value: object) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(char in "0123456789abcdef" for char in value)
    )


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
    if os.name == "nt":
        raise ValueError(
            "Windows canonical proof is blocked: scripts/cargow and source_closure use Bash; "
            "the verdict also lacks execution-context binding"
        )
    paths = {"python": Path(sys.executable)}
    for name in ("cargo", "cargo-nextest", "rustc", "git", "bash", "just"):
        found = shutil.which(name)
        if found is None:
            raise ValueError(f"required executable unavailable: {name}")
        paths[name] = Path(found)
    paths["cargow"] = WRAPPER
    result = {}
    for name, path in paths.items():
        invocation = path.absolute()
        resolved = path.resolve(strict=True)
        if not resolved.is_file():
            raise ValueError(f"required executable is not a file: {resolved}")
        if name == "cargow":
            version = "source-controlled wrapper"
        else:
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


def _environment_digest(environment: dict[str, str]) -> str:
    return hashlib.sha256(
        json.dumps(environment, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


RELEVANT_ENV = frozenset(
    {
        "CARGO_HOME",
        "CARGO_TARGET_DIR",
        "RUSTUP_TOOLCHAIN",
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTC_WRAPPER",
        "CARGO_BUILD_TARGET",
        "PYTHONPATH",
        "PYTEST_ADDOPTS",
        "QUANTA_INDEX_CACHE_ROOT",
        "QUANTA_INDEX_PRESERVE_CARGO_TARGET_DIR",
        "QUANTA_INDEX_SCCACHE",
        "CI",
        "GITHUB_SHA",
        "NEXTEST_EXPERIMENTAL_LIBTEST_JSON",
        "QUANTA_BENCH_SDK_EVIDENCE_DIR",
        "QUANTA_INDEX_SEARCHD_BIN",
        "CARGO_NET_OFFLINE",
    }
)


def _relevant_environment(environment: dict[str, str]) -> dict[str, str]:
    return {key: environment[key] for key in sorted(RELEVANT_ENV & environment.keys())}


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
            "inherited_environment": _relevant_environment(dict(os.environ)),
            "environment_sha256": _environment_digest(_relevant_environment(environment)),
            "exit_code": completed.returncode,
            "stdout": stdout,
            "stdout_sha256": _sha(out / stdout),
            "stderr": stderr,
            "stderr_sha256": _sha(out / stderr),
        }
    )
    return completed.stdout


def _run_fresh_recipe(argv: list[str], out: Path, commands: list[dict[str, object]]) -> None:
    if out.exists():
        raise ValueError(f"refusing non-fresh proof root: {out}")
    overrides = {"CARGO_NET_OFFLINE": "true"}
    environment = {**os.environ, **overrides}
    completed = subprocess.run(argv, cwd=ROOT, env=environment, capture_output=True, check=False)
    if completed.returncode != 0:
        raise ValueError(
            f"sdk-recipe failed with exit {completed.returncode}: {completed.stderr[-2000:]!r}"
        )
    if not out.is_dir():
        raise ValueError("SDK recipe returned success without a proof root")
    _write(out / "sdk-recipe.stdout", completed.stdout)
    _write(out / "sdk-recipe.stderr", completed.stderr)
    commands.append(
        {
            "name": "sdk-recipe",
            "argv": argv,
            "cwd": str(ROOT),
            "environment": overrides,
            "inherited_environment": _relevant_environment(dict(os.environ)),
            "environment_sha256": _environment_digest(_relevant_environment(environment)),
            "exit_code": completed.returncode,
            "stdout": "sdk-recipe.stdout",
            "stdout_sha256": _sha(out / "sdk-recipe.stdout"),
            "stderr": "sdk-recipe.stderr",
            "stderr_sha256": _sha(out / "sdk-recipe.stderr"),
        }
    )


def _cargo(wrapper: str, *args: str) -> list[str]:
    return [wrapper, "--lane", "test-daemon-lane", *args]


def _target_dir(wrapper: str, out: Path, commands: list[dict[str, object]]) -> Path:
    raw = _run(
        "metadata",
        _cargo(wrapper, "metadata", "--format-version", "1", "--no-deps", "--locked"),
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


def _receipt_argv(side: str, out: Path, python: str) -> list[str]:
    common = [
        python,
        str(RECEIPT_WRITER),
        "--tier",
        "correctness",
        "--evidence-format",
        "summary-json",
        "--source-closure",
        str(out / "source-closure.json"),
        "--input-evidence",
        f"execution-context={out / 'execution-context.json'}",
    ]
    if side == "python":
        return [
            *common,
            "--rail",
            "retrieval-contract-python",
            "--command",
            PYTHON_COMMAND,
            "--evidence",
            str(out / "contract_python_results.json"),
            "--input-evidence",
            f"pytest-junit={out / 'python-junit.xml'}",
            "--input-evidence",
            f"pytest-inventory={out / 'python-inventory.json'}",
            "--out",
            str(out / "contract_python_receipt.json"),
        ]
    if side == "rust":
        return [
            *common,
            "--rail",
            "retrieval-contract-rust",
            "--command",
            RUST_COMMAND,
            "--evidence",
            str(out / "contract_rust_results.json"),
            "--input-evidence",
            f"nextest-jsonl={out / 'rust-nextest.jsonl'}",
            "--input-evidence",
            f"nextest-inventory={out / 'rust-inventory.json'}",
            "--out",
            str(out / "contract_rust_receipt.json"),
        ]
    if side == "sdk":
        return [
            *common,
            "--rail",
            "retrieval-sdk-proof",
            "--command",
            SDK_COMMAND,
            "--evidence",
            str(out / "sdk_results.json"),
            "--input-evidence",
            f"nextest-jsonl={out / 'nextest.jsonl'}",
            "--input-evidence",
            f"nextest-inventory={out / 'nextest-inventory.json'}",
            "--input-evidence",
            f"runner-record={out / 'actual-runner-record.json'}",
            "--out",
            str(out / "sdk_receipt.json"),
        ]
    raise ValueError(f"unknown receipt side: {side}")


def _expected_commands(
    rail: str, out: Path, tools: dict[str, dict[str, str]], binaries: dict[str, dict[str, str]]
) -> list[tuple[str, list[str], dict[str, str]]]:
    python = tools["python"]["path"]
    wrapper = tools["cargow"]["path"]
    base = {"CARGO_NET_OFFLINE": "true"}
    test_env = {**base, "NEXTEST_EXPERIMENTAL_LIBTEST_JSON": "1"}
    source = (
        "source-closure",
        [
            python,
            str(SOURCE_CLOSURE_SCRIPT),
            "capture",
            "--profile",
            "retrieval",
            "--out",
            str(out / "source-closure.json"),
        ],
        base,
    )
    if rail == "contract":
        selector = ["-p", PACKAGE, "--lib", "--test", "chunking_contract", *FLAGS]
        return [
            source,
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
                _cargo(wrapper, "nextest", "list", *selector, "--message-format", "json"),
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
            ("rust-test", _cargo(wrapper, "nextest", "run", *selector, *FORMAT), test_env),
        ]
    return [
        ("sdk-recipe", [tools["just"]["path"], "_retrieval-sdk-proof-raw", str(out)], base),
        (
            "metadata",
            _cargo(wrapper, "metadata", "--format-version", "1", "--no-deps", "--locked"),
            base,
        ),
    ]


def produce(rail: str, out: Path) -> Path:
    if rail not in {"contract", "sdk"}:
        raise ValueError(f"unknown rail: {rail}")
    out = out.absolute()
    if out == ROOT or ROOT in out.parents:
        raise ValueError("portable proof output must be outside the source worktree")
    if os.name == "nt":
        raise ValueError("Windows canonical proof is blocked by Bash-only cargow/source_closure")
    revision = _source_revision()
    tools = _tools()
    commands: list[dict[str, object]] = []
    raw_evidence: dict[str, str] = {}
    python = tools["python"]["path"]
    wrapper = tools["cargow"]["path"]
    environment = {"NEXTEST_EXPERIMENTAL_LIBTEST_JSON": "1"}
    if rail == "contract":
        out.mkdir(parents=True, exist_ok=False)
        _run(
            "source-closure",
            [
                python,
                str(SOURCE_CLOSURE_SCRIPT),
                "capture",
                "--profile",
                "retrieval",
                "--out",
                str(out / "source-closure.json"),
            ],
            out,
            commands,
        )
        _artifact(out, "source-closure.json", raw_evidence)
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
        python_inventory = _artifact(out, "python-inventory.json", raw_evidence)
        proof_inventory.verify_inventory_authority(python_inventory, "python")
        selector = ["-p", PACKAGE, "--lib", "--test", "chunking_contract", *FLAGS]
        _run(
            "rust-collection",
            _cargo(wrapper, "nextest", "list", *selector, "--message-format", "json"),
            out,
            commands,
        )
        rust_inventory = out / "rust-inventory.json"
        _write(rust_inventory, (out / "rust-collection.stdout").read_bytes())
        _artifact(out, "rust-inventory.json", raw_evidence)
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
        junit = _artifact(out, "python-junit.xml", raw_evidence)
        nextest_argv = _cargo(wrapper, "nextest", "run", *selector, *FORMAT)
        _run("rust-test", nextest_argv, out, commands, env_overrides=environment)
        rust_events = out / "rust-test.stdout"
        _write(out / "rust-nextest.jsonl", rust_events.read_bytes())
        _artifact(out, "rust-nextest.jsonl", raw_evidence)
        python_summary = contract_proof.pytest_summary(junit, python_inventory)
        rust_summary = contract_proof.nextest_summary(rust_events, rust_inventory)
        _write_json(out / "contract_python_results.json", python_summary)
        _write_json(out / "contract_rust_results.json", rust_summary)
        binaries: dict[str, dict[str, str]] = {}
    else:
        _run_fresh_recipe(
            [tools["just"]["path"], "_retrieval-sdk-proof-raw", str(out)], out, commands
        )
        _artifact(out, "source-closure.json", raw_evidence)
        target = _target_dir(wrapper, out, commands)
        suffix = ".exe" if os.name == "nt" else ""
        searchd = target / "debug" / f"quanta-index-searchd{suffix}"
        runner = target / "debug" / f"{PACKAGE}{suffix}"
        binaries = {}
        for name, path in (("searchd", searchd), ("runner", runner)):
            if not path.is_file():
                raise ValueError(f"missing built binary: {path}")
            binaries[name] = {"path": str(path), "sha256": _sha(path)}
        inventory = out / "nextest-inventory.json"
        _artifact(out, "nextest-inventory.json", raw_evidence)
        proof_inventory.verify_inventory_authority(inventory, "sdk")
        _artifact(out, "nextest.jsonl", raw_evidence)
        record = _artifact(out, "actual-runner-record.json", raw_evidence)
        sdk_proof.build_summary(
            record, out / "nextest.jsonl", runner, inventory, searchd_path=searchd
        )
        if not (out / "sdk_results.json").is_file():
            raise ValueError("SDK recipe omitted canonical summary")
    context = {
        "schema_version": 1,
        "rail": rail,
        "revision": revision,
        "os": _os_identity(),
        "tools": tools,
        "binaries": binaries,
        "commands": commands,
        "raw_evidence": raw_evidence,
    }
    path = out / "execution-context.json"
    _write_json(path, context)
    if rail == "contract":
        for side in ("python", "rust"):
            _run(f"{side}-receipt", _receipt_argv(side, out, python), out, [])
            if not (out / f"contract_{side}_receipt.json").is_file():
                raise ValueError(f"contract {side} receipt writer omitted its output")
    else:
        _run("sdk-bound-receipt", _receipt_argv("sdk", out, python), out, [])
        if not (out / "sdk_receipt.json").is_file():
            raise ValueError("SDK receipt writer omitted its bound output")
    validate(path)
    return path


def _inside(out: Path, value: str) -> Path:
    if not isinstance(value, str) or not value or Path(value).is_absolute():
        raise ValueError("invalid relative artifact path")
    path = (out / value).resolve(strict=True)
    if path.parent != out or not path.is_file():
        raise ValueError(f"artifact escaped proof root: {value}")
    return path


def _canonical_receipt(
    path: Path,
    *,
    rail: str,
    command: str,
    summary: Path,
    inputs: dict[str, Path],
    closure: dict[str, object],
) -> None:
    receipt = _json(path)
    if not isinstance(receipt, dict) or set(receipt) != {
        "schema_version",
        "revision",
        "rail",
        "tier",
        "command",
        "evidence_path",
        "evidence_sha256",
        "test_event_count",
        "source_closure",
        "input_evidence",
    }:
        raise ValueError(f"invalid canonical receipt shape: {path}")
    result = _json(summary)
    if not isinstance(result, dict) or result.get("command") != command:
        raise ValueError(f"canonical summary command mismatch: {summary}")
    wanted_inputs = sorted(
        ({"role": role, "sha256": _sha(raw)} for role, raw in inputs.items()),
        key=lambda item: item["role"],
    )
    if (
        type(receipt["schema_version"]) is not int
        or receipt["schema_version"] != 2
        or receipt["revision"] != closure["revision"]
        or receipt["rail"] != rail
        or receipt["tier"] != "correctness"
        or receipt["command"] != command
        or receipt["evidence_path"] != str(summary)
        or receipt["evidence_sha256"] != _sha(summary)
        or type(receipt["test_event_count"]) is not int
        or receipt["test_event_count"] != result.get("executed")
        or receipt["source_closure"] != closure
        or receipt["input_evidence"] != wanted_inputs
    ):
        raise ValueError(f"canonical receipt differs from source and machine evidence: {path}")


def validate(receipt_path: Path) -> dict[str, object]:
    receipt_path = receipt_path.resolve(strict=True)
    out = receipt_path.parent.resolve()
    context = _json(receipt_path)
    if (
        not isinstance(context, dict)
        or set(context)
        != {
            "schema_version",
            "rail",
            "revision",
            "os",
            "tools",
            "binaries",
            "commands",
            "raw_evidence",
        }
        or type(context["schema_version"]) is not int
        or context["schema_version"] != 1
        or context["rail"] not in {"contract", "sdk"}
    ):
        raise ValueError("invalid execution context shape")
    closure = source_closure.validate_manifest_shape(_json(out / "source-closure.json"))
    if (
        context["revision"] != closure["revision"]
        or not isinstance(context["os"], dict)
        or set(context["os"]) != {"system", "release", "machine", "python_version"}
        or not all(isinstance(value, str) and value for value in context["os"].values())
    ):
        raise ValueError("invalid portable proof source or OS identity")
    tools = context["tools"]
    if not isinstance(tools, dict) or set(tools) != {
        "python",
        "cargo",
        "cargo-nextest",
        "rustc",
        "git",
        "bash",
        "just",
        "cargow",
    }:
        raise ValueError("invalid proof tool identities")
    for tool in tools.values():
        if (
            not isinstance(tool, dict)
            or set(tool) != {"path", "realpath", "sha256", "version"}
            or not all(
                isinstance(tool[key], str) and tool[key] for key in ("path", "realpath", "version")
            )
            or not _is_sha256(tool["sha256"])
        ):
            raise ValueError("invalid proof tool identity")
    binaries = context["binaries"]
    if not isinstance(binaries, dict) or set(binaries) != (
        {"runner", "searchd"} if context["rail"] == "sdk" else set()
    ):
        raise ValueError("invalid proof binary identities")
    for binary in binaries.values():
        if (
            not isinstance(binary, dict)
            or set(binary) != {"path", "sha256"}
            or _sha(Path(binary["path"])) != binary["sha256"]
        ):
            raise ValueError("proof binary identity changed")
    expected_commands = _expected_commands(context["rail"], out, tools, binaries)
    expected_names = [name for name, _, _ in expected_commands]
    commands = context["commands"]
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
            "inherited_environment",
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
        inherited = row["inherited_environment"]
        if (
            not isinstance(inherited, dict)
            or not all(
                key in RELEVANT_ENV and isinstance(value, str) for key, value in inherited.items()
            )
            or row["environment_sha256"] != _environment_digest({**inherited, **expected_env})
        ):
            raise ValueError("proof command environment identity changed")
        for role in ("stdout", "stderr"):
            if (
                row[role] != f"{row['name']}.{role}"
                or _sha(_inside(out, row[role])) != row[f"{role}_sha256"]
            ):
                raise ValueError("proof command output changed")
    raw_evidence = context["raw_evidence"]
    expected_raw = (
        {
            "source-closure.json",
            "python-inventory.json",
            "rust-inventory.json",
            "python-junit.xml",
            "rust-nextest.jsonl",
        }
        if context["rail"] == "contract"
        else {
            "source-closure.json",
            "nextest-inventory.json",
            "nextest.jsonl",
            "actual-runner-record.json",
        }
    )
    if not isinstance(raw_evidence, dict) or set(raw_evidence) != expected_raw:
        raise ValueError("invalid execution context raw evidence set")
    for name, digest in raw_evidence.items():
        if _sha(_inside(out, name)) != digest:
            raise ValueError(f"proof evidence changed: {name}")
    if context["rail"] == "contract":
        proof_inventory.verify_inventory_authority(out / "python-inventory.json", "python")
        proof_inventory.verify_inventory_authority(out / "rust-inventory.json", "rust")
        if (out / "rust-inventory.json").read_bytes() != (
            out / "rust-collection.stdout"
        ).read_bytes():
            raise ValueError("rust collection output differs from inventory")
        if (out / "rust-nextest.jsonl").read_bytes() != (out / "rust-test.stdout").read_bytes():
            raise ValueError("rust nextest output differs from raw evidence")
        expected = {
            "contract_python_results.json": contract_proof.pytest_summary(
                out / "python-junit.xml",
                out / "python-inventory.json",
            ),
            "contract_rust_results.json": contract_proof.nextest_summary(
                out / "rust-nextest.jsonl", out / "rust-inventory.json"
            ),
        }
        _canonical_receipt(
            out / "contract_python_receipt.json",
            rail="retrieval-contract-python",
            command=PYTHON_COMMAND,
            summary=out / "contract_python_results.json",
            inputs={
                "pytest-junit": out / "python-junit.xml",
                "pytest-inventory": out / "python-inventory.json",
                "execution-context": receipt_path,
            },
            closure=closure,
        )
        _canonical_receipt(
            out / "contract_rust_receipt.json",
            rail="retrieval-contract-rust",
            command=RUST_COMMAND,
            summary=out / "contract_rust_results.json",
            inputs={
                "nextest-jsonl": out / "rust-nextest.jsonl",
                "nextest-inventory": out / "rust-inventory.json",
                "execution-context": receipt_path,
            },
            closure=closure,
        )
    else:
        proof_inventory.verify_inventory_authority(out / "nextest-inventory.json", "sdk")
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
        expected = {
            "sdk_results.json": sdk_proof.build_summary(
                out / "actual-runner-record.json",
                out / "nextest.jsonl",
                Path(binaries["runner"]["path"]),
                out / "nextest-inventory.json",
                searchd_path=Path(binaries["searchd"]["path"]),
            )
        }
        _canonical_receipt(
            out / "sdk_receipt.json",
            rail="retrieval-sdk-proof",
            command=SDK_COMMAND,
            summary=out / "sdk_results.json",
            inputs={
                "nextest-jsonl": out / "nextest.jsonl",
                "nextest-inventory": out / "nextest-inventory.json",
                "runner-record": out / "actual-runner-record.json",
                "execution-context": receipt_path,
            },
            closure=closure,
        )
    for name, summary in expected.items():
        if _json(out / name) != summary:
            raise ValueError(f"proof summary differs from machine evidence: {name}")
    return context


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
