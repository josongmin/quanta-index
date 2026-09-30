#!/usr/bin/env python3
"""Produce execution context and context-bound retrieval receipts.

The paired verdict verifies frozen context, command logs, raw evidence and
receipt input roles. OS/tool provenance is not independently attested.
Windows production remains blocked by Bash-only cargow and Just recipes.
"""

from __future__ import annotations

import argparse
import contextvars
import hashlib
import json
import os
import platform
import sys
import tempfile
import time
from contextlib import contextmanager
from pathlib import Path

try:
    from tools.benchmark.retrieval import contract_proof, proof_inventory, sdk_proof
    from tools.ci import source_closure
except ModuleNotFoundError:  # direct script invocation
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    from tools.benchmark.retrieval import contract_proof, proof_inventory, sdk_proof
    from tools.ci import source_closure

from tools.benchmark.evidence import RawFile, file_digest, read_control
from tools.benchmark.producer_execution import execute
from tools.benchmark.retrieval.tool_custody import (
    ToolCustody,
    capture_executable,
    resolve_tool_paths,
    validate_environment,
)
from tools.ci.lint.handoff_validation import (
    _sha256_repo_regular_file,
)

ROOT = Path(__file__).resolve().parents[3]
PROOF_COMMAND_TIMEOUT_SECONDS = 7200
TOOL_TIMEOUT_SECONDS = 30
PACKAGE = "quanta-index-retrieval-bench"
FLAGS = ["--all-features", "--locked"]
EXECUTION_CONTEXT_VERSION = 2
FORMAT = ["--message-format", "libtest-json-plus", "--message-format-version", "0.1"]
SOURCE_CLOSURE_SCRIPT = ROOT / "tools/ci/source_closure.py"
RECEIPT_WRITER = ROOT / "tools/ci/write-verification-receipt.py"
WRAPPER = ROOT / "scripts/cargow"
PYTHON_COMMAND = f"python3 -m pytest {proof_inventory.PYTHON_SELECTOR} -q"
RUST_COMMAND = (
    "./scripts/cargow nextest run -p quanta-index-retrieval-bench "
    "--lib --test chunking_contract --test l5_parser_regressions --all-features --locked"
)
SDK_COMMAND = "just retrieval-sdk-proof"
_ACTIVE_CUSTODY: contextvars.ContextVar[tuple[ToolCustody, dict[str, str]] | None] = (
    contextvars.ContextVar("portable_tool_custody", default=None)
)


def _sha(path: Path) -> str:
    return file_digest(path)[0].removeprefix("sha256:")


def _is_sha256(value: object) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(char in "0123456789abcdef" for char in value)
    )


def _json(path: Path) -> object:
    path = path.absolute()
    return _json_bytes(read_control(path))


def _json_bytes(raw: RawFile | bytes) -> object:
    raw = read_control(raw)

    def unique(pairs: list[tuple[str, object]]) -> dict[str, object]:
        result: dict[str, object] = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"duplicate JSON key: {key}")
            result[key] = value
        return result

    def constant(value: str) -> None:
        raise ValueError(f"invalid JSON constant: {value}")

    return json.loads(raw, object_pairs_hook=unique, parse_constant=constant)


def selected_test_binaries(raw_collection_bytes: RawFile | bytes) -> dict[str, Path]:
    """Derive mandatory compiled executable roles from raw nextest collection.

    v2 contexts require these roles; v1 receipts are not silently upgraded.
    This is file custody, not independent compiler or OS attestation.
    """
    from tools.ci.nextest_events import parse_nextest_inventory_bytes

    raw_collection_bytes = read_control(raw_collection_bytes)
    # Share the exact selected-test/filter/count parser used by the raw oracle.
    parse_nextest_inventory_bytes(raw_collection_bytes)
    payload = _json_bytes(raw_collection_bytes)
    result: dict[str, Path] = {}
    paths: set[Path] = set()
    for binary_id, suite in payload["rust-suites"].items():
        if not any(
            case["filter-match"] == {"status": "matches"} for case in suite["testcases"].values()
        ):
            continue
        if suite.get("binary-id") != binary_id:
            raise ValueError("selected nextest binary-id differs from its inventory key")
        raw_path = suite.get("binary-path")
        if (
            not isinstance(raw_path, str)
            or not raw_path
            or "\\" in raw_path
            or "\x00" in raw_path
            or not Path(raw_path).is_absolute()
            or ".." in Path(raw_path).parts
            or Path(raw_path).as_posix() != raw_path
        ):
            raise ValueError("selected nextest executable has no canonical absolute binary-path")
        path = Path(raw_path)
        role = "nextest-" + hashlib.sha256(binary_id.encode("utf-8")).hexdigest()
        if role in result or path in paths:
            raise ValueError("duplicate selected nextest executable identity or path")
        result[role] = path
        paths.add(path)
    if not result:
        raise ValueError("missing selected nextest executable inventory")
    return dict(sorted(result.items()))


def _bind_test_binaries(raw: RawFile | bytes) -> dict[str, dict[str, str]]:
    active = _ACTIVE_CUSTODY.get()
    if active is None:
        raise ValueError("compiled test binary binding requires controlled execution")
    result = {}
    for role, path in selected_test_binaries(raw).items():
        before = capture_executable(path)
        active[0].bind_executable(path, expected_sha256=before["sha256"])
        if capture_executable(path) != before:
            raise ValueError("compiled test executable changed during binding")
        result[role] = {"path": str(path), "sha256": before["sha256"]}
    return result


def verify_reused_build(
    binary_raw: RawFile | bytes,
    metadata_raw: RawFile | bytes,
    collection_raw: RawFile | bytes,
    *,
    workspace_root: Path,
    required_non_test_binary: Path | None = None,
) -> dict[str, Path]:
    """Cross-check actual native build metadata against the selected collection."""
    selected = selected_test_binaries(collection_raw)
    binary_list = _json_bytes(binary_raw)
    metadata = _json_bytes(metadata_raw)
    collection = _json_bytes(collection_raw)
    if (
        not isinstance(binary_list, dict)
        or set(binary_list) != {"rust-build-meta", "rust-binaries"}
        or not isinstance(binary_list["rust-binaries"], dict)
        or not isinstance(binary_list["rust-build-meta"], dict)
        or not isinstance(metadata, dict)
        or metadata.get("workspace_root") != str(workspace_root)
        or metadata.get("target_directory")
        != binary_list["rust-build-meta"].get("target-directory")
        or not isinstance(metadata.get("packages"), list)
    ):
        raise ValueError("native build metadata workspace/target identity differs")
    target = metadata["target_directory"]
    if not isinstance(target, str) or not Path(target).is_absolute() or ".." in Path(target).parts:
        raise ValueError("native build metadata has no canonical target directory")
    packages = {}
    for package in metadata["packages"]:
        if (
            not isinstance(package, dict)
            or not isinstance(package.get("id"), str)
            or not package["id"]
            or package["id"] in packages
        ):
            raise ValueError("native Cargo package inventory is malformed or duplicate")
        packages[package["id"]] = package
    expected_ids = {
        binary_id
        for binary_id, suite in collection["rust-suites"].items()
        if any(
            case["filter-match"] == {"status": "matches"} for case in suite["testcases"].values()
        )
    }
    if set(binary_list["rust-binaries"]) != expected_ids:
        raise ValueError("native binary build inventory differs from selected test collection")
    fields = {"binary-id", "binary-name", "package-id", "kind", "binary-path", "build-platform"}
    for binary_id, row in binary_list["rust-binaries"].items():
        suite = collection["rust-suites"][binary_id]
        if (
            not isinstance(row, dict)
            or set(row) != fields
            or any(row.get(key) != suite.get(key) for key in fields)
            or row["package-id"] not in packages
            or packages[row["package-id"]].get("name") != PACKAGE
            or packages[row["package-id"]].get("manifest_path")
            != str(workspace_root / "benchmarks/retrieval/Cargo.toml")
        ):
            raise ValueError(
                "native compiled binary differs from collection/Cargo package identity"
            )
    if required_non_test_binary is not None:
        package_ids = [
            package_id
            for package_id, package in packages.items()
            if package.get("name") == PACKAGE
            and package.get("manifest_path")
            == str(workspace_root / "benchmarks/retrieval/Cargo.toml")
        ]
        non_test = binary_list["rust-build-meta"].get("non-test-binaries")
        expected_path = Path(target) / "debug" / PACKAGE
        if (
            len(package_ids) != 1
            or not isinstance(non_test, dict)
            or non_test.get(package_ids[0])
            != [{"name": PACKAGE, "kind": "bin-exe", "path": f"debug/{PACKAGE}"}]
            or required_non_test_binary != expected_path
        ):
            raise ValueError("SDK runner is absent from the selected native build")
    return selected


def _reuse_nextest(wrapper: str, operation: str, out: Path, *args: str) -> list[str]:
    return _cargo(
        wrapper,
        "nextest",
        operation,
        "--binaries-metadata",
        str(out / "rust-build.stdout"),
        "--cargo-metadata",
        str(out / "metadata.stdout"),
        *args,
    )


def _reuse_input_epoch(path: Path, expected: RawFile) -> tuple:
    """Bind no-follow input bytes and local file/ancestor identity, not attestation."""
    try:
        return _capture_reuse_input_epoch(path, expected)
    except (OSError, ValueError) as error:
        raise ValueError(f"nextest reuse input unavailable or changed: {path}: {error}") from error


def _capture_reuse_input_epoch(path: Path, expected: RawFile) -> tuple:
    fields = ("st_dev", "st_ino", "st_mode", "st_size", "st_mtime_ns", "st_ctime_ns")
    before = tuple(getattr(path.lstat(), field) for field in fields)
    chain = tuple(
        (str(parent), parent.lstat().st_dev, parent.lstat().st_ino, parent.lstat().st_mode)
        for parent in path.parents
    )
    actual = file_digest(path)
    after = tuple(getattr(path.lstat(), field) for field in fields)
    if before != after or actual != (expected.sha256, expected.size):
        raise ValueError("nextest reuse input changed before execution")
    return before, chain, actual


def _run_reused_nextest(
    wrapper: str,
    out: Path,
    commands: list[dict[str, object]],
    binary_raw: RawFile,
    metadata_raw: RawFile,
    *,
    env_overrides: dict[str, str],
    operation: str = "run",
) -> RawFile:
    # Collection and execution consume the same one-time build. Neither may
    # reinterpret mutable metadata or independently prepare another binary set.
    if operation not in {"list", "run"}:
        raise ValueError("unsupported reused nextest operation")
    name = "rust-test" if operation == "run" else "rust-collection"
    args = FORMAT if operation == "run" else ["--message-format", "json"]
    inputs = ((out / "rust-build.stdout", binary_raw), (out / "metadata.stdout", metadata_raw))
    epochs = [_reuse_input_epoch(path, raw) for path, raw in inputs]
    try:
        return _run(
            name,
            _reuse_nextest(wrapper, operation, out, *args),
            out,
            commands,
            env_overrides=env_overrides,
        )
    finally:
        for (path, raw), epoch in zip(inputs, epochs, strict=True):
            if _reuse_input_epoch(path, raw) != epoch:
                raise ValueError("nextest reuse input epoch changed during execution")


def _write(path: Path, data: bytes) -> None:
    read_control(data)
    with path.open("xb") as stream:
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())


def _write_json(path: Path, value: object) -> None:
    _write(path, (json.dumps(value, sort_keys=True, indent=2) + "\n").encode())


def _git(*args: str) -> str:
    active = _ACTIVE_CUSTODY.get()
    environment = dict(os.environ) if active is None else active[0].environment()
    executable = "git" if active is None else active[0].tools()["git"]["path"]
    if active is not None:
        active[0].check()
    try:
        stdout, _, _ = execute(
            [executable, *args],
            cwd=ROOT,
            env=environment,
            timeout=TOOL_TIMEOUT_SECONDS,
            log_dir=Path(tempfile.mkdtemp(prefix="quanta-proof-git-")).resolve(),
        )
    finally:
        if active is not None:
            active[0].check()
    return stdout.read_control().decode("utf-8").strip()


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
    paths = resolve_tool_paths(ROOT, dict(os.environ))
    paths["cargow"] = WRAPPER
    result = {}
    for name, path in paths.items():
        invocation = path.absolute()
        resolved = path.resolve(strict=True)
        if not resolved.is_file():
            raise ValueError(f"required executable is not a file: {resolved}")
        before = capture_executable(invocation)
        if name == "cargow":
            version = "source-controlled wrapper"
        else:
            stdout, _, _ = execute(
                [str(invocation), "-Vv" if name == "rustc" else "--version"],
                cwd=ROOT,
                env=dict(os.environ),
                timeout=TOOL_TIMEOUT_SECONDS,
                log_dir=Path(tempfile.mkdtemp(prefix="quanta-proof-tool-")).resolve(),
            )
            version = stdout.read_control().decode("utf-8").strip()
            if not version:
                raise ValueError(f"required executable has no version identity: {name}")
        if capture_executable(invocation) != before:
            raise ValueError(f"required executable changed during version probe: {name}")
        result[name] = {
            "path": str(invocation),
            "realpath": str(resolved),
            "sha256": before["sha256"],
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
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTC",
        "PATH",
        "RUSTUP_HOME",
        "PYTHONHOME",
        "PYTEST_PLUGINS",
        "BASH_ENV",
        "ENV",
        "ZDOTDIR",
        "CARGO_BUILD_TARGET",
        "CARGO_BUILD_JOBS",
        "PYTHONPATH",
        "PYTEST_ADDOPTS",
        "QUANTA_INDEX_CACHE_ROOT",
        "QUANTA_INDEX_RESOURCE_ADMISSION",
        "QUANTA_INDEX_RESOURCE_WAIT_SECONDS",
        "QUANTA_INDEX_RESOURCE_TIMEOUT_SECONDS",
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


def execution_overrides(
    tools: dict[str, dict[str, str]], inherited: dict[str, str]
) -> dict[str, str]:
    return {
        "PATH": str(Path(tools["cargo"]["path"]).parent)
        + os.pathsep
        + inherited.get("PATH", os.defpath),
        "RUSTC": tools["rustc"]["realpath"],
        "RUSTC_WRAPPER": "",
        "RUSTC_WORKSPACE_WRAPPER": "",
        "QUANTA_INDEX_SCCACHE": "0",
    }


@contextmanager
def controlled_execution():
    """Yield the shared local custody owner for a complete producer invocation.

    Sibling producers bind reference and native executables to this same owner.
    This does not authenticate a remote producer or isolate a compromised UID.
    """
    inherited = dict(os.environ)
    validate_environment(inherited)
    if any(
        inherited.get(key)
        for key in ("PYTEST_ADDOPTS", "PYTEST_PLUGINS", "PYTHONHOME", "PYTHONPATH")
    ):
        raise ValueError("portable proof refuses Python startup and test selection overrides")
    tools = _tools()
    with tempfile.TemporaryDirectory(prefix="qi-proof-tools-") as directory:
        custody = ToolCustody.create(
            ROOT, Path(directory) / "bin", tools=tools, environment=inherited
        )
        token = _ACTIVE_CUSTODY.set((custody, inherited))
        try:
            yield custody
            custody.check()
        finally:
            _ACTIVE_CUSTODY.reset(token)


def _run(
    name: str,
    argv: list[str],
    out: Path,
    commands: list[dict[str, object]],
    *,
    env_overrides: dict[str, str] | None = None,
    expected_executable_sha256: str | None = None,
) -> RawFile:
    started = time.monotonic_ns()
    if not argv or not Path(argv[0]).is_absolute():
        raise ValueError(f"{name} requires an absolute executable")
    active = _ACTIVE_CUSTODY.get()
    inherited = dict(os.environ) if active is None else active[1]
    controls = {} if active is None else execution_overrides(active[0].tools(), inherited)
    overrides = {"CARGO_NET_OFFLINE": "true", **controls, **(env_overrides or {})}
    if any(overrides.get(key) != value for key, value in controls.items()):
        raise ValueError("proof command attempted to override selected tool custody")
    environment = {**inherited, **overrides}
    if active is not None:
        if _relevant_environment(dict(os.environ)) != _relevant_environment(inherited):
            raise ValueError("proof inherited environment changed during production")
        active[0].bind_executable(Path(argv[0]), expected_sha256=expected_executable_sha256)
        active[0].check()
    elif expected_executable_sha256 is not None:
        raise ValueError("expected executable identity requires controlled execution")
    prepared = time.monotonic_ns()
    try:
        output, errors, _ = execute(
            argv,
            cwd=ROOT,
            env=environment,
            timeout=PROOF_COMMAND_TIMEOUT_SECONDS,
            log_dir=out.parent / f".{out.name}-execution" / name,
        )
    finally:
        executed = time.monotonic_ns()
        if active is not None:
            active[0].check()
    verified = time.monotonic_ns()
    stdout = f"{name}.stdout"
    stderr = f"{name}.stderr"
    retained_output = output.copy_to(out / stdout)
    errors.copy_to(out / stderr)
    commands.append(
        {
            "name": name,
            "argv": argv,
            "cwd": str(ROOT),
            "environment": overrides,
            "inherited_environment": _relevant_environment(inherited),
            "environment_sha256": _environment_digest(_relevant_environment(environment)),
            "exit_code": 0,
            "stdout": stdout,
            "stdout_sha256": output.sha256.removeprefix("sha256:"),
            "stderr": stderr,
            "stderr_sha256": errors.sha256.removeprefix("sha256:"),
        }
    )
    recorded = time.monotonic_ns()
    # Diagnostic timing only: kept outside the execution-context schema and
    # never accepted as proof of correctness or qualified product performance.
    _write_json(
        out / f"{name}.timing.json",
        {
            "schema_version": 1,
            "kind": "proof_command_timing_diagnostic",
            "command": name,
            "prepare_ns": prepared - started,
            "execute_ns": executed - prepared,
            "verify_ns": verified - executed,
            "record_ns": recorded - verified,
            "total_ns": recorded - started,
            "excludes": "timing-file write and work outside this command",
        },
    )
    return retained_output


def _run_fresh_recipe(argv: list[str], out: Path, commands: list[dict[str, object]]) -> None:
    if out.exists():
        raise ValueError(f"refusing non-fresh proof root: {out}")
    overrides = {"CARGO_NET_OFFLINE": "true"}
    environment = {**os.environ, **overrides}
    output, errors, _ = execute(
        argv,
        cwd=ROOT,
        env=environment,
        timeout=PROOF_COMMAND_TIMEOUT_SECONDS,
        log_dir=out.parent / f".{out.name}-execution" / "sdk-recipe",
    )
    if not out.is_dir():
        raise ValueError("SDK recipe returned success without a proof root")
    output.copy_to(out / "sdk-recipe.stdout")
    errors.copy_to(out / "sdk-recipe.stderr")
    commands.append(
        {
            "name": "sdk-recipe",
            "argv": argv,
            "cwd": str(ROOT),
            "environment": overrides,
            "inherited_environment": _relevant_environment(dict(os.environ)),
            "environment_sha256": _environment_digest(_relevant_environment(environment)),
            "exit_code": 0,
            "stdout": "sdk-recipe.stdout",
            "stdout_sha256": output.sha256.removeprefix("sha256:"),
            "stderr": "sdk-recipe.stderr",
            "stderr_sha256": errors.sha256.removeprefix("sha256:"),
        }
    )


def _cargo(wrapper: str, *args: str) -> list[str]:
    return [wrapper, "--lane", "test-daemon-lane", *args]


def _target_dir(wrapper: str, out: Path, commands: list[dict[str, object]]) -> Path:
    raw = _run(
        "metadata",
        _cargo(wrapper, "metadata", "--format-version", "1", "--locked"),
        out,
        commands,
    )
    payload = _json_bytes(raw)
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


def _receipt_argv(
    side: str, out: Path, python: str, *, context_path: Path | None = None
) -> list[str]:
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
        f"execution-context={context_path if context_path is not None else out / 'execution-context.json'}",
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
    rail: str,
    out: Path,
    tools: dict[str, dict[str, str]],
    binaries: dict[str, dict[str, str]],
    *,
    inherited_environment: dict[str, str] | None = None,
) -> list[tuple[str, list[str], dict[str, str]]]:
    python = tools["python"]["path"]
    wrapper = tools["cargow"]["path"]
    base = {"CARGO_NET_OFFLINE": "true"}
    if "cargo" in tools and "rustc" in tools:
        base.update(
            execution_overrides(
                tools, dict(os.environ) if inherited_environment is None else inherited_environment
            )
        )
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
        selector = [
            "-p",
            PACKAGE,
            "--lib",
            "--test",
            "chunking_contract",
            "--test",
            "l5_parser_regressions",
            *FLAGS,
        ]
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
                "rust-build",
                _cargo(
                    wrapper,
                    "nextest",
                    "list",
                    *selector,
                    "--list-type",
                    "binaries-only",
                    "--message-format",
                    "json",
                ),
                base,
            ),
            ("metadata", _cargo(wrapper, "metadata", "--format-version", "1", "--locked"), base),
            (
                "rust-collection",
                _reuse_nextest(wrapper, "list", out, "--message-format", "json"),
                base,
            ),
            (
                "python-test",
                [
                    python,
                    "-m",
                    "pytest",
                    *proof_inventory.PYTHON_SELECTORS,
                    "-q",
                    f"--junitxml={out / 'python-junit.xml'}",
                ],
                base,
            ),
            ("rust-test", _reuse_nextest(wrapper, "run", out, *FORMAT), test_env),
        ]
    selector = ["-p", PACKAGE, "--test", "sdk_roundtrip", *FLAGS]
    return [
        source,
        (
            "build-searchd",
            _cargo(
                wrapper,
                "build",
                "-p",
                "quanta-index-searchd-runtime",
                "--bin",
                "quanta-index-searchd",
                "--locked",
            ),
            base,
        ),
        (
            "rust-build",
            _cargo(
                wrapper,
                "nextest",
                "list",
                *selector,
                "--list-type",
                "binaries-only",
                "--message-format",
                "json",
            ),
            base,
        ),
        (
            "metadata",
            _cargo(wrapper, "metadata", "--format-version", "1", "--locked"),
            base,
        ),
        ("rust-collection", _reuse_nextest(wrapper, "list", out, "--message-format", "json"), base),
        (
            "rust-test",
            _reuse_nextest(wrapper, "run", out, *FORMAT),
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
    if os.name == "nt":
        raise ValueError("Windows canonical proof is blocked by Bash-only cargow/source_closure")
    with controlled_execution() as custody:
        revision = _source_revision()
        result = _produce(rail, out, revision, custody.tools())
        custody.check()
        if _source_revision() != revision:
            raise ValueError("portable proof source revision changed during production")
        _run(
            "terminal-source-verify",
            [
                custody.tools()["python"]["path"],
                str(SOURCE_CLOSURE_SCRIPT),
                "verify",
                "--manifest",
                str(out / "source-closure.json"),
            ],
            out,
            [],
        )
        published = out / "execution-context.json"
        os.link(result, published)
        result.unlink()
        return published


def _produce(rail: str, out: Path, revision: str, tools: dict[str, dict[str, str]]) -> Path:
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
        selector = [
            "-p",
            PACKAGE,
            "--lib",
            "--test",
            "chunking_contract",
            "--test",
            "l5_parser_regressions",
            *FLAGS,
        ]
        build_raw = _run(
            "rust-build",
            _cargo(
                wrapper,
                "nextest",
                "list",
                *selector,
                "--list-type",
                "binaries-only",
                "--message-format",
                "json",
            ),
            out,
            commands,
        )
        _target_dir(wrapper, out, commands)
        metadata_raw = RawFile.capture(out / "metadata.stdout")
        collected = _run_reused_nextest(
            wrapper, out, commands, build_raw, metadata_raw, env_overrides={}, operation="list"
        )
        rust_inventory = out / "rust-inventory.json"
        collected.copy_to(rust_inventory)
        _artifact(out, "rust-inventory.json", raw_evidence)
        proof_inventory.verify_inventory_authority(rust_inventory, "rust")

        verify_reused_build(build_raw, metadata_raw, collected, workspace_root=ROOT)
        binaries = _bind_test_binaries(collected)
        pytest_argv = [
            python,
            "-m",
            "pytest",
            *proof_inventory.PYTHON_SELECTORS,
            "-q",
            f"--junitxml={out / 'python-junit.xml'}",
        ]
        _run("python-test", pytest_argv, out, commands)
        junit = _artifact(out, "python-junit.xml", raw_evidence)
        events = _run_reused_nextest(
            wrapper, out, commands, build_raw, metadata_raw, env_overrides=environment
        )
        rust_events = out / "rust-test.stdout"
        events.copy_to(out / "rust-nextest.jsonl")
        _artifact(out, "rust-nextest.jsonl", raw_evidence)
        python_summary = contract_proof.pytest_summary(junit, python_inventory)
        rust_summary = contract_proof.nextest_summary(rust_events, rust_inventory)
        _write_json(out / "contract_python_results.json", python_summary)
        _write_json(out / "contract_rust_results.json", rust_summary)
    else:
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
            "build-searchd",
            _cargo(
                wrapper,
                "build",
                "-p",
                "quanta-index-searchd-runtime",
                "--bin",
                "quanta-index-searchd",
                "--locked",
            ),
            out,
            commands,
        )
        # The integration test's CARGO_BIN_EXE reference makes this preparation
        # compile the runner; bind the resulting binary after collection.
        selector = ["-p", PACKAGE, "--test", "sdk_roundtrip", *FLAGS]
        build_raw = _run(
            "rust-build",
            _cargo(
                wrapper,
                "nextest",
                "list",
                *selector,
                "--list-type",
                "binaries-only",
                "--message-format",
                "json",
            ),
            out,
            commands,
        )
        target = _target_dir(wrapper, out, commands)
        metadata_raw = RawFile.capture(out / "metadata.stdout")
        collected = _run_reused_nextest(
            wrapper, out, commands, build_raw, metadata_raw, env_overrides={}, operation="list"
        )
        collected.copy_to(out / "nextest-inventory.json")

        suffix = ".exe" if os.name == "nt" else ""
        searchd = target / "debug" / f"quanta-index-searchd{suffix}"
        runner = target / "debug" / f"{PACKAGE}{suffix}"
        verify_reused_build(
            build_raw,
            metadata_raw,
            collected,
            workspace_root=ROOT,
            required_non_test_binary=runner,
        )
        binaries = {}
        for name, path in (("searchd", searchd), ("runner", runner)):
            if not path.is_file():
                raise ValueError(f"missing built binary: {path}")
            binaries[name] = {"path": str(path), "sha256": _sha(path)}
            active = _ACTIVE_CUSTODY.get()
            if active is None:
                raise ValueError("SDK binary binding requires controlled execution")
            active[0].bind_executable(path, expected_sha256=binaries[name]["sha256"])
        inventory = out / "nextest-inventory.json"
        _artifact(out, "nextest-inventory.json", raw_evidence)
        proof_inventory.verify_inventory_authority(inventory, "sdk")
        binaries.update(_bind_test_binaries(collected))
        events = _run_reused_nextest(
            wrapper,
            out,
            commands,
            build_raw,
            metadata_raw,
            env_overrides={
                **environment,
                "QUANTA_BENCH_SDK_EVIDENCE_DIR": str(out),
                "QUANTA_INDEX_SEARCHD_BIN": str(searchd),
            },
        )
        events.copy_to(out / "nextest.jsonl")
        _artifact(out, "nextest.jsonl", raw_evidence)
        record = _artifact(out, "actual-runner-record.json", raw_evidence)
        summary = sdk_proof.build_summary(
            record, out / "nextest.jsonl", runner, inventory, searchd_path=searchd
        )
        _write_json(out / "sdk_results.json", summary)
    context = {
        "schema_version": EXECUTION_CONTEXT_VERSION,
        "rail": rail,
        "revision": revision,
        "os": _os_identity(),
        "tools": tools,
        "binaries": binaries,
        "commands": commands,
        "raw_evidence": raw_evidence,
    }
    path = out / "execution-context.pending.json"
    _write_json(path, context)
    if rail == "contract":
        for side in ("python", "rust"):
            _run(f"{side}-receipt", _receipt_argv(side, out, python, context_path=path), out, [])
            if not (out / f"contract_{side}_receipt.json").is_file():
                raise ValueError(f"contract {side} receipt writer omitted its output")
    else:
        _run("sdk-bound-receipt", _receipt_argv("sdk", out, python, context_path=path), out, [])
        if not (out / "sdk_receipt.json").is_file():
            raise ValueError("SDK receipt writer omitted its bound output")
    validate(path, _allow_pending=True)
    return path


def _inside(out: Path, value: str) -> Path:
    if (
        not isinstance(value, str)
        or not value
        or Path(value).is_absolute()
        or len(Path(value).parts) != 1
        or value in {".", ".."}
    ):
        raise ValueError("invalid relative artifact path")
    return out / value


def _canonical_receipt(
    path: Path,
    *,
    rail: str,
    command: str,
    summary: Path,
    inputs: dict[str, Path],
    closure: dict[str, object],
    execution_root: Path | None = None,
    captured: dict[str, RawFile] | None = None,
) -> None:
    commitments = {} if captured is None else captured

    def raw(value: Path) -> RawFile:
        if value.name not in commitments:
            commitments[value.name] = RawFile.capture(value)
        return commitments[value.name]

    receipt = _json_bytes(raw(path))
    summary_bytes = raw(summary).read_control()
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
    result = _json_bytes(summary_bytes)
    if not isinstance(result, dict) or result.get("command") != command:
        raise ValueError(f"canonical summary command mismatch: {summary}")
    wanted_inputs = sorted(
        (
            {"role": role, "sha256": raw(value).sha256.removeprefix("sha256:")}
            for role, value in inputs.items()
        ),
        key=lambda item: item["role"],
    )
    if (
        type(receipt["schema_version"]) is not int
        or receipt["schema_version"] != 2
        or receipt["revision"] != closure["revision"]
        or receipt["rail"] != rail
        or receipt["tier"] != "correctness"
        or receipt["command"] != command
        or receipt["evidence_path"]
        != str((execution_root / summary.name) if execution_root else summary)
        or receipt["evidence_sha256"] != hashlib.sha256(summary_bytes).hexdigest()
        or type(receipt["test_event_count"]) is not int
        or receipt["test_event_count"] != result.get("executed")
        or receipt["source_closure"] != closure
        or receipt["input_evidence"] != wanted_inputs
    ):
        raise ValueError(f"canonical receipt differs from source and machine evidence: {path}")
    if captured is None:
        for value in commitments.values():
            if file_digest(value.path) != (value.sha256, value.size):
                raise ValueError("canonical evidence changed during validation")


def validate(
    receipt_path: Path,
    *,
    execution_root: Path | None = None,
    binary_files: dict[str, Path] | None = None,
    _allow_pending: bool = False,
) -> dict[str, object]:
    """Verify frozen bytes, retaining original command/path provenance.

    Relocated immutable custody may supply the original absolute execution
    root and frozen binary files. These affect lookup only; raw commands,
    receipt paths, and native binary metadata are never rewritten.
    """
    receipt_path = receipt_path.absolute()
    if receipt_path.name == "execution-context.pending.json" and not _allow_pending:
        raise ValueError("unpublished proof execution context")
    out = receipt_path.parent
    captured: dict[str, RawFile] = {}

    def capture(name: str) -> RawFile:
        _inside(out, name)
        if name not in captured:
            captured[name] = RawFile.capture(out / name)
        return captured[name]

    def captured_digest(name: str) -> str:
        return capture(name).sha256.removeprefix("sha256:")

    def same_bytes(left: str, right: str) -> bool:
        a, b = capture(left), capture(right)
        return (a.sha256, a.size) == (b.sha256, b.size)

    execution_root = out if execution_root is None else execution_root
    if not execution_root.is_absolute() or ".." in execution_root.parts:
        raise ValueError("execution root must be an absolute canonical recorded path")
    context = _json_bytes(capture(receipt_path.name))
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
        or context["schema_version"] != EXECUTION_CONTEXT_VERSION
        or context["rail"] not in {"contract", "sdk"}
    ):
        raise ValueError("invalid execution context shape")
    closure = source_closure.validate_manifest_shape(_json_bytes(capture("source-closure.json")))
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
    selected_binaries = selected_test_binaries(capture("rust-collection.stdout"))
    expected_binary_roles = set(selected_binaries) | (
        {"runner", "searchd"} if context["rail"] == "sdk" else set()
    )
    if not isinstance(binaries, dict) or set(binaries) != expected_binary_roles:
        raise ValueError("invalid proof binary identities")
    verify_reused_build(
        capture("rust-build.stdout"),
        capture("metadata.stdout"),
        capture("rust-collection.stdout"),
        workspace_root=ROOT,
        required_non_test_binary=(
            Path(binaries["runner"]["path"])
            if context["rail"] == "sdk"
            and isinstance(binaries["runner"], dict)
            and isinstance(binaries["runner"].get("path"), str)
            else None
        ),
    )
    if any(
        not isinstance(binaries[name], dict) or binaries[name].get("path") != str(path)
        for name, path in selected_binaries.items()
    ):
        raise ValueError("compiled test binary path differs from raw collection")
    if binary_files is not None and set(binary_files) != set(binaries):
        raise ValueError("frozen proof binary inventory mismatch")
    binary_digests: dict[str, str] = {}
    for name, binary in binaries.items():
        if (
            not isinstance(binary, dict)
            or set(binary) != {"path", "sha256"}
            or not isinstance(binary["path"], str)
            or not Path(binary["path"]).is_absolute()
        ):
            raise ValueError("proof binary identity changed")
        binary_path = (
            binary_files[name] if binary_files is not None else Path(binary["path"])
        ).absolute()
        binary_digests[name] = _sha256_repo_regular_file(
            binary_path.parent, binary_path.name, label="portable proof binary"
        )
        if binary_digests[name] != binary["sha256"]:
            raise ValueError("proof binary identity changed")
    commands = context["commands"]
    if (
        not isinstance(commands, list)
        or not commands
        or not isinstance(commands[0], dict)
        or not isinstance(commands[0].get("inherited_environment"), dict)
    ):
        raise ValueError("missing or malformed proof commands")
    expected_commands = _expected_commands(
        context["rail"],
        execution_root,
        tools,
        binaries,
        inherited_environment=commands[0]["inherited_environment"],
    )
    expected_names = [name for name, _, _ in expected_commands]
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
                or captured_digest(row[role]) != row[f"{role}_sha256"]
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
        if captured_digest(name) != digest:
            raise ValueError(f"proof evidence changed: {name}")
    if context["rail"] == "contract":
        proof_inventory.verify_inventory_authority(capture("python-inventory.json"), "python")
        proof_inventory.verify_inventory_authority(capture("rust-inventory.json"), "rust")
        if not same_bytes("rust-inventory.json", "rust-collection.stdout"):
            raise ValueError("rust collection output differs from inventory")
        if not same_bytes("rust-nextest.jsonl", "rust-test.stdout"):
            raise ValueError("rust nextest output differs from raw evidence")
        expected = {
            "contract_python_results.json": contract_proof.pytest_summary(
                capture("python-junit.xml"),
                capture("python-inventory.json"),
            ),
            "contract_rust_results.json": contract_proof.nextest_summary(
                capture("rust-nextest.jsonl"), capture("rust-inventory.json")
            ),
        }
        capture("contract_python_receipt.json")
        capture("contract_python_results.json")
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
            execution_root=execution_root,
            captured=captured,
        )
        capture("contract_rust_receipt.json")
        capture("contract_rust_results.json")
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
            execution_root=execution_root,
            captured=captured,
        )
    else:
        proof_inventory.verify_inventory_authority(capture("nextest-inventory.json"), "sdk")
        if not same_bytes("nextest-inventory.json", "rust-collection.stdout") or not same_bytes(
            "nextest.jsonl", "rust-test.stdout"
        ):
            raise ValueError("SDK collection/test output differs from raw evidence")
        metadata = _json_bytes(capture("metadata.stdout"))
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
            "sdk_results.json": sdk_proof.build_summary_from_evidence(
                capture("actual-runner-record.json"),
                capture("nextest.jsonl"),
                binary_digests["runner"],
                capture("nextest-inventory.json"),
                searchd_digest=binary_digests["searchd"],
            )
        }
        capture("sdk_receipt.json")
        capture("sdk_results.json")
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
            execution_root=execution_root,
            captured=captured,
        )
    for name, summary in expected.items():
        if _json_bytes(capture(name)) != summary:
            raise ValueError(f"proof summary differs from machine evidence: {name}")
    for raw in captured.values():
        if file_digest(raw.path) != (raw.sha256, raw.size):
            raise ValueError("portable proof evidence changed during validation")
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
    except (OSError, ValueError, SystemExit) as error:
        raise SystemExit(f"portable proof refused: {error}") from error
    print(path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
