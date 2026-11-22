"""Recompute proof test outcomes from archived runner events and collection inventories."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

try:
    from tools.ci.nextest_events import (
        NextestEvidenceError,
        parse_nextest_bytes,
        parse_nextest_inventory_bytes,
    )
except ModuleNotFoundError:  # direct tool entrypoints place only their own directory on sys.path
    sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
    from tools.ci.nextest_events import (
        NextestEvidenceError,
        parse_nextest_bytes,
        parse_nextest_inventory_bytes,
    )

from tools.benchmark.evidence import RawFile
from tools.ci.daemon_execution_evidence import (
    live_proof,
    require_daemon_test_inventory,
    validate_nonce,
    validate_witness,
)
from tools.ci.junit_events import JUnitEvidenceError, parse_pytest_junit_bytes
from tools.ci.lint.handoff_validation import _read_repo_regular_bytes
from tools.ci.proof_json import parse_proof_json
from tools.ci.proof_run_source import (
    P12A_PROOF,
    ProofRunSource,
    capture_run_source,
    frozen_run_checkout,
    require_run_source,
    verify_run_binding,
    write_run_binding,
)


class ExecutionResultError(ValueError):
    """The archived runner evidence does not establish the claimed outcome."""


ROOT = Path(__file__).resolve().parents[2]


def _validate_pytest_selectors(selectors: list[str]) -> None:
    if not selectors or any(
        re.fullmatch(r"tools/ci/tests/test_[a-z0-9_]+\.py", selector) is None
        for selector in selectors
    ):
        raise ExecutionResultError("proof collection requires complete test file selectors")
    if len(selectors) != len(set(selectors)):
        raise ExecutionResultError("proof collection contains duplicate selectors")


def _validate_pytest_selection(selectors: list[str]) -> None:
    _validate_pytest_selectors(selectors)
    for variable in ("PYTEST_ADDOPTS", "PYTEST_PLUGINS"):
        if os.environ.get(variable):
            raise ExecutionResultError(f"{variable} can alter proof test collection")


def pytest_junit_identity(nodeid: str) -> str:
    """Match pytest's JUnit address mangling without requiring pytest at replay.

    Parameter IDs are opaque: pytest separates the first ``[`` before
    splitting the file/class/function address on ``::``, then restores the
    entire parameter suffix to the case name.
    """
    address, bracket, parameters = nodeid.partition("[")
    parts = address.split("::")
    if len(parts) < 2 or not parts[0].endswith(".py") or any(not part for part in parts):
        raise ExecutionResultError(f"invalid pytest node ID: {nodeid!r}")
    module = parts[0][:-3].replace("/", ".")
    parts[-1] += bracket + parameters
    return ".".join((module, *parts[1:]))


def collect_pytest_inventory(selectors: list[str], output: Path) -> None:
    """Use pytest's actual collector rather than a hand-maintained expected-case list."""
    _validate_pytest_selection(selectors)
    if os.path.lexists(output):
        raise ExecutionResultError(f"proof collection inventory already exists: {output}")

    import pytest

    class Collector:
        nodeids: list[str] = []

        def pytest_collection_finish(self, session: Any) -> None:
            self.nodeids = [item.nodeid for item in session.items]

    collector = Collector()
    code = pytest.main(["--collect-only", "-p", "no:cacheprovider", "-q", *selectors], plugins=[collector])
    if code != pytest.ExitCode.OK or not collector.nodeids:
        raise ExecutionResultError(f"pytest collection failed or selected no tests: {code}")
    identities = sorted(pytest_junit_identity(nodeid) for nodeid in collector.nodeids)
    if len(identities) != len(set(identities)):
        raise ExecutionResultError("pytest collection contains duplicate JUnit identities")
    payload = {
        "schema_version": 1,
        "kind": "pytest",
        "selector": " ".join(selectors),
        "tests": identities,
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            prefix=f".{output.name}.",
            suffix=".tmp",
            dir=output.parent,
            delete=False,
        ) as stream:
            temporary = Path(stream.name)
            stream.write(json.dumps(payload, sort_keys=True, indent=2) + "\n")
            stream.flush()
            os.fsync(stream.fileno())
        # link creates the final path only if no prior evidence exists.
        os.link(temporary, output)
    except FileExistsError as error:
        raise ExecutionResultError(
            f"proof collection inventory already exists: {output}"
        ) from error
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def _collect_frozen_pytest(
    checkout: Path, selectors: list[str], output: Path, env: dict[str, str],
) -> None:
    command = [
        sys.executable, "-m", "tools.ci.proof_execution_result", "collect-pytest",
        "--output", str(output), *selectors,
    ]
    completed = subprocess.run(command, cwd=checkout, env=env, check=False)
    if completed.returncode != 0:
        raise ExecutionResultError(f"frozen pytest collection failed: {completed.returncode}")


def run_p12a_pytest(selectors: list[str], raw_dir: Path | None) -> int:
    """Run the P12A files and emit raw JUnit in proof mode."""
    _validate_pytest_selection(selectors)
    if Path.cwd().resolve() != ROOT:
        raise ExecutionResultError("proof pytest runner must start at the repository root")
    command = [sys.executable, "-m", "pytest", *selectors, "-q"]
    if raw_dir is None:
        return subprocess.run(command, cwd=ROOT, check=False).returncode

    raw_dir = (ROOT / raw_dir).resolve()
    inventory = raw_dir / "p12a-inventory.json"
    junit = raw_dir / "p12a-junit.xml"
    binding = raw_dir / "p12a-run-binding.json"
    outputs = (inventory, junit, binding)
    for path in outputs:
        if os.path.lexists(path):
            raise ExecutionResultError(f"proof evidence already exists: {path}")
    try:
        proof = live_proof(ROOT, P12A_PROOF)
        source = capture_run_source(ROOT, proof, outputs=outputs)
        if live_proof(ROOT, P12A_PROOF) != proof:
            raise ExecutionResultError("proof registry changed during source capture")
        with frozen_run_checkout(ROOT, proof, source, outputs=outputs) as checkout:
            if live_proof(checkout.root, P12A_PROOF) != proof:
                raise ExecutionResultError("proof registry differs from the frozen checkout")
            env = {**os.environ, "PYTHONPATH": str(checkout.root), "PYTHONDONTWRITEBYTECODE": "1"}
            _collect_frozen_pytest(checkout.root, selectors, inventory, env)
            collected = RawFile.capture(inventory)
            checkout.require_unchanged("pytest collection")
            require_run_source(ROOT, proof, source, phase="pytest collection", outputs=outputs)
            completed = subprocess.run(
                [*command, "-p", "no:cacheprovider", f"--junitxml={junit}"],
                cwd=checkout.root, env=env, check=False,
            )
            if completed.returncode != 0:
                return completed.returncode
            observed = RawFile.capture(junit)
            counts, _ = _pytest_result_bytes(
                observed.consume_seekable(lambda stream: stream.read()),
                collected.consume_seekable(lambda stream: stream.read()),
            )
            checkout.require_unchanged("pytest execution")
            require_run_source(ROOT, proof, source, phase="pytest execution", outputs=outputs)
            if RawFile.capture(inventory) != collected or RawFile.capture(junit) != observed:
                raise ExecutionResultError("pytest evidence changed during verification")
            write_run_binding(
                binding, source, format="pytest-junit",
                digests={
                    "events": observed.sha256.removeprefix("sha256:"),
                    "inventory": collected.sha256.removeprefix("sha256:"),
                },
            )
    except (OSError, ValueError) as error:
        raise ExecutionResultError(str(error)) from error
    print(
        f"proof pytest evidence: selected={counts['selected']}; "
        f"inventory={inventory}; junit={junit}; run_binding={binding}",
        flush=True,
    )
    return 0


def _pytest_result(events: Path, inventory: Path) -> tuple[dict[str, int], set[str]]:
    try:
        return _pytest_result_bytes(events.read_bytes(), inventory.read_bytes())
    except OSError as error:
        raise ExecutionResultError(f"invalid pytest result: {error}") from error


def _pytest_result_bytes(events: bytes, inventory: bytes) -> tuple[dict[str, int], set[str]]:
    try:
        expected = parse_proof_json(inventory)
    except (UnicodeError, ValueError) as error:
        raise ExecutionResultError(f"invalid pytest result: {error}") from error
    if (
        not isinstance(expected, dict)
        or set(expected) != {"schema_version", "kind", "selector", "tests"}
        or type(expected["schema_version"]) is not int
        or expected["schema_version"] != 1
        or expected["kind"] != "pytest"
        or not isinstance(expected["selector"], str)
        or not expected["selector"]
        or not isinstance(expected["tests"], list)
        or not expected["tests"]
        or any(not isinstance(name, str) or not name for name in expected["tests"])
        or expected["tests"] != sorted(set(expected["tests"]))
    ):
        raise ExecutionResultError("invalid pytest collection inventory")
    selectors = expected["selector"].split(" ")
    _validate_pytest_selectors(selectors)
    modules = {selector[:-3].replace("/", ".") for selector in selectors}
    represented: set[str] = set()
    for identity in expected["tests"]:
        matching = [module for module in modules if identity.startswith(module + ".")]
        if len(matching) != 1 or identity == matching[0] + ".":
            raise ExecutionResultError("pytest inventory testcase is outside declared selectors")
        represented.add(matching[0])
    if represented != modules:
        raise ExecutionResultError("pytest inventory omits a declared test file")
    try:
        return parse_pytest_junit_bytes(events, set(expected["tests"]))
    except JUnitEvidenceError as error:
        raise ExecutionResultError(str(error)) from error


def _bound_artifact_bytes(root: Path, artifact: dict[str, str]) -> bytes:
    digest = artifact.get("sha256")
    if not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None:
        raise ExecutionResultError("execution artifact lacks a valid digest")
    try:
        raw = _read_repo_regular_bytes(root, artifact["path"], label="execution artifact")
    except (OSError, ValueError) as error:
        raise ExecutionResultError(f"unsafe execution artifact: {error}") from error
    if hashlib.sha256(raw).hexdigest() != digest:
        raise ExecutionResultError("execution artifact digest mismatch")
    return raw


def derive_test_result(
    root: Path, result: Any, artifacts: list[dict[str, str]],
    *,
    daemon_observations: dict[str, str] | None = None,
    daemon_tests: dict[str, int] | None = None,
    required_lib_cases: dict[str, list[str]] | None = None,
    run_source: ProofRunSource | None = None,
    daemon_sha256: str | None = None,
    provider_approval_sha256: str | None = None,
) -> tuple[dict[str, int], set[tuple[str, str]]]:
    if (
        not isinstance(result, dict)
        or set(result) != {"schema_version", "runs"}
        or type(result["schema_version"]) is not int
        or result["schema_version"] != 1
        or not isinstance(result["runs"], list)
        or not result["runs"]
    ):
        raise ExecutionResultError("passed test proof requires versioned execution runs")
    by_source = {item["source_path"]: item for item in artifacts}
    if len(by_source) != len(artifacts):
        raise ExecutionResultError("duplicate archived artifact source")
    counts = {key: 0 for key in ("selected", "executed", "passed", "failed", "ignored")}
    names: set[tuple[str, str]] = set()
    used_paths: set[str] = set()
    missing_lib_cases = {
        (package, case)
        for package, cases in (required_lib_cases or {}).items()
        for case in cases
    }
    for run in result["runs"]:
        required = {"format", "events", "inventory"}
        if not isinstance(run, dict) or set(run) - {"daemon_observations", "run_binding"} != required:
            raise ExecutionResultError("execution run has invalid fields")
        if run["format"] not in {"nextest-jsonl", "pytest-junit"}:
            raise ExecutionResultError("execution run format is not allowlisted")
        if ("run_binding" in run) != (run_source is not None):
            raise ExecutionResultError("execution run does not match its registered source binding")
        references = {key: run[key] for key in ("events", "inventory")}
        if "daemon_observations" in run:
            if daemon_observations is None or run["format"] != "nextest-jsonl":
                raise ExecutionResultError("daemon observations are not registered for this proof")
            references["daemon_observations"] = run["daemon_observations"]
        if run_source is not None:
            references["run_binding"] = run["run_binding"]
        paths = list(references.values())
        if any(not isinstance(path, str) or path not in by_source for path in paths):
            raise ExecutionResultError("execution run source is not an archived proof artifact")
        if len(set(paths)) != len(paths) or used_paths.intersection(paths):
            raise ExecutionResultError("duplicate execution evidence")
        used_paths.update(paths)
        raw = {kind: _bound_artifact_bytes(root, by_source[path]) for kind, path in references.items()}
        if run_source is not None:
            verify_run_binding(
                raw["run_binding"], run_source, format=run["format"],
                digests={
                    key: by_source[path]["sha256"]
                    for key, path in references.items()
                    if key != "run_binding"
                },
                daemon_sha256=daemon_sha256,
            )
        event_bytes = raw["events"]
        inventory_bytes = raw["inventory"]
        if run["format"] == "nextest-jsonl":
            try:
                inventory = parse_nextest_inventory_bytes(inventory_bytes)
                parsed = parse_nextest_bytes(event_bytes, inventory)
            except NextestEvidenceError as error:
                raise ExecutionResultError(str(error)) from error
            for name in parsed.passed_names:
                package, _, kind = inventory[name]
                if kind == "lib":
                    missing_lib_cases.discard((package, name.partition("$")[2]))
            current = {
                "selected": parsed.selected,
                "executed": parsed.executed,
                "passed": parsed.passed,
                "failed": parsed.failed,
                "ignored": parsed.selected - parsed.executed,
            }
            current_names = set(parsed.passed_names)
            if "daemon_observations" in raw:
                batch = parse_proof_json(raw["daemon_observations"])
                observed_names = current_names.intersection(daemon_tests or {})
                if (
                    not isinstance(batch, dict)
                    or set(batch) != {"nonce", "events_sha256", "inventory_sha256", "observations"}
                    or batch["events_sha256"] != hashlib.sha256(event_bytes).hexdigest()
                    or batch["inventory_sha256"] != hashlib.sha256(inventory_bytes).hexdigest()
                    or not isinstance(batch["observations"], dict)
                    or not observed_names
                    or set(batch["observations"]) != observed_names
                ):
                    raise ExecutionResultError("daemon observations differ from their archived run")
                require_daemon_test_inventory(inventory, observed_names)
                nonce = validate_nonce(batch["nonce"])
                for test in observed_names:
                    daemon_observations[test] = validate_witness(
                        batch["observations"][test], test=test, nonce=nonce,
                        launches=(daemon_tests or {})[test],
                        provider_approval_sha256=provider_approval_sha256,
                    )
        else:
            current, current_names = _pytest_result_bytes(event_bytes, inventory_bytes)
        typed_names = {(run["format"], name) for name in current_names}
        if names.intersection(typed_names):
            raise ExecutionResultError("duplicate selected test across execution runs")
        names.update(typed_names)
        for key in counts:
            counts[key] += current[key]
    if counts["ignored"] or counts["failed"] or not counts["passed"]:
        raise ExecutionResultError("passed proof contains skipped, failed or empty execution")
    if missing_lib_cases:
        missing = [f"{package}:{case}" for package, case in sorted(missing_lib_cases)]
        raise ExecutionResultError(f"registered nextest lib cases did not pass: {missing}")
    return counts, names


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=["collect-pytest", "run-p12a"])
    parser.add_argument("--output", type=Path)
    parser.add_argument("selectors", nargs="+")
    args = parser.parse_args(argv)
    try:
        if args.operation == "collect-pytest":
            if args.output is None:
                raise ExecutionResultError("collect-pytest requires --output")
            collect_pytest_inventory(args.selectors, args.output)
        else:
            if args.output is not None:
                raise ExecutionResultError("run-p12a does not accept --output")
            raw_dir = os.environ.get("QUANTA_PROOF_RAW_DIR")
            return run_p12a_pytest(args.selectors, Path(raw_dir) if raw_dir else None)
    except (ExecutionResultError, OSError) as error:
        parser.error(str(error))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
