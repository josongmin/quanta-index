"""Build single-route, source-derived lexical diagnostic suites from a frozen suite.

The output is mechanical and diagnostic only. It never consumes search results
or changes the source suite, corpus checkout, or an existing output root.
"""

from __future__ import annotations

import argparse
import copy
import json
import sys
from pathlib import Path
from typing import Any

try:
    from tools.benchmark.evidence import read_control
    from tools.benchmark.retrieval import evaluator, source_oracle
except ModuleNotFoundError:  # direct script invocation
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    from tools.benchmark.evidence import read_control
    from tools.benchmark.retrieval import evaluator, source_oracle

MODES = {
    "identifier-word-file": (source_oracle.ASCII_IDENTIFIER_WORD, "distinct_file", "lexical"),
    "go-declaration-file": (source_oracle.GO_EXACT_LOCAL_NAME, "distinct_file", "lexical"),
    "go-declaration-symbol": (source_oracle.GO_EXACT_LOCAL_NAME, "symbol", "symbol"),
}
OWNED_TASK_FIELDS = frozenset(
    {"source_oracle", "file_judgments", "declaration_judgments", "judgment_policy", "label_review"}
)
TOOL_FILES = (
    "tools/benchmark/retrieval/source_oracle_suite.py",
    "tools/benchmark/retrieval/source_oracle.py",
    "tools/benchmark/retrieval/evaluator.py",
    "tools/benchmark/retrieval/suite.schema.json",
)


def _tool_digests() -> list[dict[str, str]]:
    tool_root = Path(__file__).resolve().parents[3]
    return [
        {"path": name, "sha256": evaluator.digest((tool_root / name).read_bytes())}
        for name in TOOL_FILES
    ]


def derive_suites(repo: Path, baseline: dict[str, Any]) -> dict[str, tuple[dict, dict]]:
    """Derive complete judgments, then revalidate each final suite and blind pack."""
    _checked, _pack, source = evaluator.validate_suite(repo, baseline)
    for task in baseline["tasks"]:
        evaluator.require(
            not (OWNED_TASK_FIELDS & task.keys()),
            f"source-oracle derivation requires an unannotated task: {task['task_id']}",
        )
        evaluator.require(
            task.get("query_intent", "bare_symbol") == "bare_symbol",
            f"source-oracle derivation requires bare_symbol intent: {task['task_id']}",
        )
    entries = baseline["file_universe"]
    oracle = source_oracle.SourceOracleIndex(
        {entry["path"]: (source.file(entry["path"])[0], entry["file_sha256"]) for entry in entries},
        {task["query"] for task in baseline["tasks"]},
    )
    outputs: dict[str, tuple[dict, dict]] = {}
    for mode, (contract, unit, route) in MODES.items():
        suite = copy.deepcopy(baseline)
        suite["suite_id"] = baseline["suite_id"] + "-" + mode + "-source-oracle-v1"
        suite["routes"] = [route]
        suite["diagnostic_policy"] = evaluator.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
        kind = "declaration_judgments" if unit == "symbol" else "file_judgments"
        for task in suite["tasks"]:
            task["query_intent"] = "bare_symbol"
            task["source_oracle"] = {"contract": contract, "unit": unit}
            task["judgment_policy"] = evaluator.SOURCE_ORACLE_JUDGMENT_POLICY
            task[kind] = oracle.expected_rows(contract, task["query"], unit)
        _checked, pack, _source = evaluator.validate_suite(repo, suite)
        outputs[mode] = suite, pack
    return outputs


def _json_bytes(payload: dict) -> bytes:
    return (json.dumps(payload, indent=2, sort_keys=True, ensure_ascii=False) + "\n").encode(
        "utf-8"
    )


def _baseline_from_bytes(raw: bytes) -> dict:
    def unique_object(pairs: list[tuple[str, Any]]) -> dict:
        result: dict[str, Any] = {}
        for key, value in pairs:
            evaluator.require(key not in result, f"duplicate baseline JSON key: {key}")
            result[key] = value
        return result

    def reject_constant(value: str) -> Any:
        raise evaluator.EvidenceError(f"non-finite baseline JSON number: {value}")

    try:
        return json.loads(
            raw.decode("utf-8"),
            object_pairs_hook=unique_object,
            parse_constant=reject_constant,
        )
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise evaluator.EvidenceError(f"invalid baseline JSON: {exc}") from exc


def write_suites(repo: Path, baseline_path: Path, output_root: Path) -> dict[str, Any]:
    """Validate everything before creating a new external output root."""
    tool_files = _tool_digests()
    baseline_bytes = read_control(baseline_path)
    baseline = _baseline_from_bytes(baseline_bytes)
    suites = derive_suites(repo, baseline)
    artifacts = []
    contents: dict[str, bytes] = {}
    for mode, (suite, pack) in suites.items():
        for kind, payload in (("suite", suite), ("blind-pack", pack)):
            name = f"{mode}-{kind}.json"
            raw = _json_bytes(payload)
            contents[name] = raw
            artifacts.append({"path": name, "sha256": evaluator.digest(raw)})
    tool_root = Path(__file__).resolve().parents[3]
    for item in tool_files:
        name = "tool-sources/" + item["path"]
        raw = (tool_root / item["path"]).read_bytes()
        evaluator.require(
            evaluator.digest(raw) == item["sha256"], "tool source changed during derivation"
        )
        contents[name] = raw
        artifacts.append({"path": name, "sha256": item["sha256"]})
    manifest = {
        "schema_version": 1,
        "qualification": "diagnostic_unqualified",
        "repository_commit": baseline["repository_commit"],
        "input_suite_sha256": evaluator.digest(baseline_bytes),
        "tool_files": tool_files,
        "artifacts": sorted(artifacts, key=lambda item: item["path"]),
    }
    evaluator.require(output_root.is_absolute(), "output root must be absolute")
    evaluator.require(not output_root.exists(), "output root already exists")
    evaluator.require(_tool_digests() == tool_files, "tool source changed during derivation")
    parent = output_root.parent.resolve(strict=True)
    evaluator.require(parent.is_dir(), "output root parent is not a directory")
    for checkout in (repo.resolve(), tool_root.resolve()):
        evaluator.require(
            parent != checkout and checkout not in parent.parents,
            "output root must be outside source and tool checkouts",
        )
    output_root.mkdir()
    for name, raw in contents.items():
        path = output_root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("xb") as stream:
            stream.write(raw)
    with (output_root / "manifest.json").open("xb") as stream:
        stream.write(_json_bytes(manifest))
    return manifest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--repo", required=True, type=Path)
    parser.add_argument("--baseline-suite", required=True, type=Path)
    parser.add_argument("--output-root", required=True, type=Path)
    args = parser.parse_args()
    try:
        manifest = write_suites(args.repo.resolve(), args.baseline_suite, args.output_root)
    except (OSError, ValueError, source_oracle.SourceOracleError) as exc:
        parser.exit(2, f"ERROR: {exc}\n")
    print(json.dumps(manifest, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
