"""Derive a distinct typo negative suite from a frozen content-absence suite.

The source population is fixed before search results are collected. Every
submitted query is then replayed against the complete frozen source with the
independent case-folded ASCII identifier OSA<=1 absence oracle.
"""

from __future__ import annotations

import argparse
import copy
import json
import re
import shutil
from pathlib import Path

from tools.benchmark.retrieval import evaluator, source_oracle, source_oracle_suite

SOURCE_CONTRACT = source_oracle.ASCII_CONTENT_ABSENT_CASEFOLD
TARGET_CONTRACT = source_oracle.ASCII_IDENTIFIER_OSA1_ABSENT_CASEFOLD
TARGET_LANE = "typo-osa1-absence"
SOURCE_LANE = "no-answer-content"
SUITE_SUFFIX = "-identifier-osa1-absence-v1"
TOOL_FILES = (
    "tools/benchmark/retrieval/identifier_osa1_absence_suite.py",
    "tools/benchmark/retrieval/source_oracle.py",
    "tools/benchmark/retrieval/source_oracle_suite.py",
    "tools/benchmark/retrieval/evaluator.py",
)


def derive(repo: Path, source_suite: dict) -> tuple[dict, dict, dict]:
    evaluator.require(isinstance(source_suite, dict), "source suite must be an object")
    suite_id = source_suite.get("suite_id")
    evaluator.require(
        isinstance(suite_id, str)
        and re.search(r"-no-answer-content-v2-seed[0-9]+\Z", suite_id) is not None,
        "source must be the frozen content-absence suite",
    )
    evaluator.require(bool(source_suite.get("tasks")), "source suite has no tasks")
    evaluator.require(isinstance(source_suite["tasks"], list), "source suite tasks must be a list")
    evaluator.require(
        all(
            task["split"] == "eval"
            and not task["answerable"]
            and not task["file_judgments"]
            and task["source_oracle"] == {"contract": SOURCE_CONTRACT, "unit": "distinct_file"}
            for task in source_suite["tasks"]
        ),
        "source suite is not a complete content-absence population",
    )
    # Validate the source population before changing its oracle identity.
    evaluator.validate_suite(repo, source_suite)
    suite = copy.deepcopy(source_suite)
    suite["suite_id"] += SUITE_SUFFIX
    for task in suite["tasks"]:
        task["source_oracle"]["contract"] = TARGET_CONTRACT
    _checked, pack, _source = evaluator.validate_suite(repo, suite)
    source_records = [
        {"task_id": task["task_id"], "query": task["query"], "status": "admitted"}
        for task in source_suite["tasks"]
    ]
    mapping = [
        {"task_id": task["task_id"], "source_task_id": task["task_id"], "query": task["query"]}
        for task in suite["tasks"]
    ]
    census = {
        "lanes": {
            SOURCE_LANE: {
                "contract": SOURCE_CONTRACT,
                "admitted": len(source_records),
                "records": source_records,
            },
            TARGET_LANE: {
                "contract": TARGET_CONTRACT,
                "derived_from": SOURCE_LANE,
                "source_admitted": len(source_records),
                "admitted": len(mapping),
                "excluded": 0,
                "excluded_probes": [],
                "records": mapping,
            },
        }
    }
    return suite, pack, census


def write(
    repo: Path,
    source_path: Path,
    output_root: Path,
    target_suite_path: Path | None = None,
    target_pack_path: Path | None = None,
) -> dict:
    evaluator.require(
        output_root.is_absolute() and not output_root.exists(),
        "output root must be absolute and new",
    )
    parent = output_root.parent.resolve(strict=True)
    tool_root = Path(__file__).resolve().parents[3]
    evaluator.require(
        not parent.is_relative_to(repo.resolve()) and not parent.is_relative_to(tool_root),
        "output root must be outside checkouts",
    )
    source_raw = source_path.read_bytes()
    source_suite = evaluator.read_json(source_path)
    suite, pack, census = derive(repo, source_suite)
    evaluator.require(
        pack["suite_commitment_sha256"] == evaluator.digest(evaluator.canonical(suite)),
        "derived pack commitment differs from suite",
    )
    suite_raw = source_oracle_suite._json_bytes(suite)
    pack_raw = source_oracle_suite._json_bytes(pack)
    if target_suite_path is not None:
        suite_raw = target_suite_path.read_bytes()
        evaluator.require(
            evaluator.read_json(target_suite_path) == suite,
            "frozen target suite differs from independently derived suite",
        )
    if target_pack_path is not None:
        pack_raw = target_pack_path.read_bytes()
        evaluator.require(
            evaluator.read_json(target_pack_path) == pack,
            "frozen target pack differs from independently derived pack",
        )
    contents = {
        "source-no-answer-content-suite.json": source_raw,
        f"{TARGET_LANE}-suite.json": suite_raw,
        f"{TARGET_LANE}-blind-pack.json": pack_raw,
        "census.json": source_oracle_suite._json_bytes(census),
    }
    tool_files = []
    for name in TOOL_FILES:
        raw = (tool_root / name).read_bytes()
        contents["tool-sources/" + name] = raw
        tool_files.append({"path": name, "sha256": evaluator.digest(raw)})
    manifest = {
        "schema_version": 1,
        "qualification": "diagnostic_unqualified_source_exposed",
        "repository_commit": suite["repository_commit"],
        "source_suite_sha256": evaluator.digest(source_raw),
        "tool_files": tool_files,
        "parameters": {
            "source_lane": SOURCE_LANE,
            "source_admitted": len(source_suite["tasks"]),
            "target_contract": TARGET_CONTRACT,
        },
        "artifacts": sorted(
            ({"path": name, "sha256": evaluator.digest(raw)} for name, raw in contents.items()),
            key=lambda item: item["path"],
        ),
    }
    output_root.mkdir()
    try:
        for name, raw in contents.items():
            path = output_root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            with path.open("xb") as stream:
                stream.write(raw)
        with (output_root / "manifest.json").open("xb") as stream:
            stream.write(source_oracle_suite._json_bytes(manifest))
    except BaseException:
        shutil.rmtree(output_root)
        raise
    return manifest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--repo", required=True, type=Path)
    parser.add_argument("--source-suite", required=True, type=Path)
    parser.add_argument("--target-suite", type=Path)
    parser.add_argument("--target-pack", type=Path)
    parser.add_argument("--output-root", required=True, type=Path)
    args = parser.parse_args()
    try:
        manifest = write(
            args.repo.resolve(),
            args.source_suite.resolve(),
            args.output_root,
            args.target_suite.resolve() if args.target_suite else None,
            args.target_pack.resolve() if args.target_pack else None,
        )
    except (ValueError, KeyError, TypeError, OSError, evaluator.EvidenceError) as exc:
        parser.error(str(exc))
    print(json.dumps(manifest, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
