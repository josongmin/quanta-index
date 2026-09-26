"""Recompute proof test outcomes from archived runner events and collection inventories."""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import Any

try:
    from tools.ci.nextest_events import NextestEvidenceError, parse_nextest, parse_nextest_inventory
except ModuleNotFoundError:  # direct tool entrypoints place only their own directory on sys.path
    sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
    from tools.ci.nextest_events import NextestEvidenceError, parse_nextest, parse_nextest_inventory


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
    """Map a collected pytest node ID to its JUnit classname and case name."""
    parts = nodeid.split("::")
    if len(parts) < 2 or not parts[0].endswith(".py") or any(not part for part in parts):
        raise ExecutionResultError(f"invalid pytest node ID: {nodeid!r}")
    module = parts[0][:-3].replace("/", ".")
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
    code = pytest.main(["--collect-only", "-q", *selectors], plugins=[collector])
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
    if os.path.lexists(junit):
        raise ExecutionResultError(f"proof JUnit result already exists: {junit}")
    collect_pytest_inventory(selectors, inventory)
    completed = subprocess.run([*command, f"--junitxml={junit}"], cwd=ROOT, check=False)
    if completed.returncode != 0:
        return completed.returncode
    counts, _ = _pytest_result(junit, inventory)
    print(
        f"proof pytest evidence: selected={counts['selected']}; "
        f"inventory={inventory}; junit={junit}",
        flush=True,
    )
    return 0


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ExecutionResultError(f"duplicate inventory key: {key}")
        result[key] = value
    return result


def _pytest_result(events: Path, inventory: Path) -> tuple[dict[str, int], set[str]]:
    try:
        expected = json.loads(
            inventory.read_text(encoding="utf-8"), object_pairs_hook=_unique_object
        )
        root = ET.parse(events).getroot()
    except (OSError, UnicodeError, ValueError, ET.ParseError) as error:
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
    if root.tag not in {"testsuite", "testsuites"}:
        raise ExecutionResultError("invalid pytest JUnit root")
    # Interpret runner outcomes only after checking the admitted JUnit grammar.
    # A suite-level error or an outcome hidden inside an unknown wrapper must
    # never become a pass merely because the declared counters say zero.
    for node in root.iter():
        if node.tag in {"failure", "error", "skipped"}:
            raise ExecutionResultError("pytest testcase did not pass: JUnit contains an outcome")
        allowed_children = {
            "testsuites": {"testsuite"},
            "testsuite": {"testcase", "properties", "system-out", "system-err"},
            "testcase": {"properties", "system-out", "system-err", "failure", "error", "skipped"},
            "properties": {"property"},
            "property": set(),
            "system-out": set(),
            "system-err": set(),
        }.get(node.tag)
        if allowed_children is None or any(child.tag not in allowed_children for child in node):
            raise ExecutionResultError("pytest JUnit contains an unsupported element placement")
    suites = [root] if root.tag == "testsuite" else list(root.findall("testsuite"))
    if (
        not suites
        or (root.tag == "testsuites" and root.findall("testcase"))
        or any(suite.findall("testsuite") for suite in suites)
        or len(list(root.iter("testcase")))
        != sum(len(suite.findall("testcase")) for suite in suites)
    ):
        raise ExecutionResultError("pytest JUnit has missing or nested suites")
    observed: set[str] = set()
    for suite in suites:
        cases = suite.findall("testcase")
        if not cases:
            raise ExecutionResultError("pytest JUnit has an empty suite")
        for case in cases:
            name = case.get("name")
            classname = case.get("classname")
            if not name or not classname:
                raise ExecutionResultError("pytest testcase lacks identity")
            identity = f"{classname}.{name}"
            if identity in observed:
                raise ExecutionResultError("duplicate pytest testcase")
            observed.add(identity)
            if any(case.find(outcome) is not None for outcome in ("failure", "error", "skipped")):
                raise ExecutionResultError(f"pytest testcase did not pass: {identity}")
        for field, actual in (
            ("tests", len(cases)),
            ("failures", 0),
            ("errors", 0),
            ("skipped", 0),
        ):
            if suite.get(field) != str(actual):
                raise ExecutionResultError(f"pytest suite {field} disagrees with testcases")
    if root.tag == "testsuites":
        for field, actual in (
            ("tests", len(observed)),
            ("failures", 0),
            ("errors", 0),
            ("skipped", 0),
        ):
            if root.get(field) is not None and root.get(field) != str(actual):
                raise ExecutionResultError(f"pytest root {field} disagrees with testcases")
    if observed != set(expected["tests"]):
        raise ExecutionResultError("pytest execution differs from collected required tests")
    return {
        "selected": len(observed),
        "executed": len(observed),
        "passed": len(observed),
        "failed": 0,
        "ignored": 0,
    }, observed


def derive_test_result(
    root: Path, result: Any, artifacts: list[dict[str, str]]
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
    for run in result["runs"]:
        if not isinstance(run, dict) or set(run) != {"format", "events", "inventory"}:
            raise ExecutionResultError("execution run has invalid fields")
        if run["format"] not in {"nextest-jsonl", "pytest-junit"}:
            raise ExecutionResultError("execution run format is not allowlisted")
        paths = (run["events"], run["inventory"])
        if any(not isinstance(path, str) or path not in by_source for path in paths):
            raise ExecutionResultError("execution run source is not an archived proof artifact")
        if paths[0] == paths[1] or used_paths.intersection(paths):
            raise ExecutionResultError("duplicate execution evidence")
        used_paths.update(paths)
        event_path = root / by_source[paths[0]]["path"]
        inventory_path = root / by_source[paths[1]]["path"]
        if run["format"] == "nextest-jsonl":
            try:
                parsed = parse_nextest(event_path, parse_nextest_inventory(inventory_path))
            except NextestEvidenceError as error:
                raise ExecutionResultError(str(error)) from error
            current = {
                "selected": parsed.selected,
                "executed": parsed.executed,
                "passed": parsed.passed,
                "failed": parsed.failed,
                "ignored": parsed.selected - parsed.executed,
            }
            current_names = set(parsed.passed_names)
        else:
            current, current_names = _pytest_result(event_path, inventory_path)
        typed_names = {(run["format"], name) for name in current_names}
        if names.intersection(typed_names):
            raise ExecutionResultError("duplicate selected test across execution runs")
        names.update(typed_names)
        for key in counts:
            counts[key] += current[key]
    if counts["ignored"] or counts["failed"] or not counts["passed"]:
        raise ExecutionResultError("passed proof contains skipped, failed or empty execution")
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
