"""Build single-route, source-derived lexical diagnostic suites from a frozen suite.

The output is mechanical and diagnostic only. It never consumes search results
or changes the source suite, corpus checkout, or an existing output root.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import sys
from collections import defaultdict
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
    "pyproject.toml",
    "uv.lock",
    "tools/benchmark/evidence.py",
    "tools/ci/lint/handoff_validation.py",
    "tools/ci/proof_json.py",
    "tools/benchmark/retrieval/source_oracle_suite.py",
    "tools/benchmark/retrieval/source_oracle.py",
    "tools/benchmark/retrieval/evaluator.py",
    "tools/benchmark/retrieval/query_plan.py",
    "tools/benchmark/retrieval/retrieval_contract.py",
    "tools/benchmark/retrieval/finite_json.py",
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
    _checked, _pack, source = evaluator.validate_suite(repo, baseline, source_oracle_admission=True)
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
        suite["suite_id"] = baseline["suite_id"] + "-" + mode + "-source-oracle-v3"
        suite["routes"] = [route]
        suite["diagnostic_policy"] = evaluator.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
        kind = "declaration_judgments" if unit == "symbol" else "file_judgments"
        for task in suite["tasks"]:
            task["query_intent"] = "bare_symbol"
            task["source_oracle"] = {"contract": contract, "unit": unit}
            task["judgment_policy"] = evaluator.SOURCE_ORACLE_JUDGMENT_POLICY
            task[kind] = oracle.expected_rows(
                contract, task["query"], unit, include_name_spans=(unit == "symbol")
            )
            task["gold"] = evaluator.source_oracle_gold(source, oracle, contract, task["query"])
            task["answerable"] = bool(task[kind])
            evaluator.require(
                bool(task["gold"]) == task["answerable"],
                f"source oracle gold/judgment mismatch: {task['task_id']}",
            )
        _checked, pack, _source = evaluator.validate_suite(repo, suite)
        outputs[mode] = suite, pack
    return outputs


def _json_bytes(payload: dict) -> bytes:
    return (json.dumps(payload, indent=2, sort_keys=True, ensure_ascii=False) + "\n").encode(
        "utf-8"
    )


def identifier_word_suite(
    repo: Path,
    manifest: dict,
    *,
    suite_id: str,
    seed: int = 20261010,
    per_stratum: int = 15,
    negatives: int = 10,
) -> tuple[dict, dict, dict]:
    """Freeze content-token lookup tasks without consuming product outputs.

    Sample six length/document-frequency strata by seeded hash, retaining
    underfilled strata. The exposed source is not an independent holdout.
    """
    evaluator.require(
        type(seed) is int and type(per_stratum) is int and 0 < per_stratum <= 100,
        "invalid identifier sampling policy",
    )
    evaluator.require(type(negatives) is int and 0 <= negatives <= 100, "invalid negative count")
    source = evaluator.SourceSnapshot(
        repo, manifest["repository_commit"], max_total_bytes=source_oracle.MAX_SOURCE_BYTES
    )
    entries, universe = evaluator.validate_file_universe(source, manifest["files"])
    files = {path: (source.file(path)[0], entries[path]) for path in sorted(entries)}
    postings: dict[str, set[str]] = defaultdict(set)
    folded: dict[str, set[str]] = defaultdict(set)
    for path, (raw, _sha) in files.items():
        for match in source_oracle.WORDS.finditer(raw):
            name = match[0].decode("ascii")
            folded[name.casefold()].add(name)
            if 3 <= len(name) <= 64 and name.upper() not in {"AND", "OR", "NOT"}:
                postings[name].add(path)
    strata: dict[str, list[str]] = defaultdict(list)
    for name, paths in postings.items():
        if len(folded[name.casefold()]) != 1:
            continue
        length = (
            "short_3_6" if len(name) <= 6 else "medium_7_16" if len(name) <= 16 else "long_17_64"
        )
        frequency = "single_file" if len(paths) == 1 else "multiple_files"
        strata[length + ":" + frequency].append(name)
    selected = []
    counts = {}
    selected_grams = []

    def order(name):
        return hashlib.sha256(f"{seed}:{name}".encode()).digest(), name

    for length in ("short_3_6", "medium_7_16", "long_17_64"):
        for frequency in ("single_file", "multiple_files"):
            key = length + ":" + frequency
            names = []
            excluded_near_duplicates = 0
            for name in sorted(strata[key], key=order):
                grams = evaluator.query_shingles(evaluator.normalize_query(name))
                if any(
                    evaluator.shingle_jaccard(grams, previous) >= evaluator.QUERY_NEAR_DUP_JACCARD
                    for previous in selected_grams
                ):
                    excluded_near_duplicates += 1
                    continue
                names.append(name)
                selected_grams.append(grams)
                if len(names) == per_stratum:
                    break
            selected.extend((key, name, source_oracle.ASCII_IDENTIFIER_WORD) for name in names)
            counts[key] = {
                "available": len(strata[key]),
                "selected": len(names),
                "underfilled": len(names) < per_stratum,
                "excluded_near_duplicates": excluded_near_duplicates,
            }
    negative_queries = [
        "X" + hashlib.sha256(f"{seed}:negative:{i}".encode()).hexdigest()[:30]
        for i in range(negatives)
    ]
    selected.extend(
        ("no_answer", query, source_oracle.ASCII_CODE_SEARCH_DEFAULT_ABSENT_CASEFOLD)
        for query in negative_queries
    )
    oracle = source_oracle.SourceOracleIndex(files, {name for _key, name, _contract in selected})
    tasks = []
    for number, (stratum, name, contract) in enumerate(selected):
        judgments = oracle.expected_rows(contract, name, "distinct_file")
        tasks.append(
            {
                "task_id": f"I{number:04d}",
                "split": "eval",
                "query": name,
                "query_sha256": evaluator.digest(name.encode()),
                "query_family_id": "word-" + evaluator.digest(name.encode()),
                "category": stratum,
                "query_intent": "bare_symbol",
                "answerable": bool(judgments),
                "gold": evaluator.source_oracle_gold(source, oracle, contract, name),
                "source_oracle": {"contract": contract, "unit": "distinct_file"},
                "judgment_policy": evaluator.SOURCE_ORACLE_JUDGMENT_POLICY,
                "file_judgments": judgments,
                "evaluation_contract": {
                    "request_mode": "default_file_search",
                    "gold_unit": "distinct_file",
                    "result_unit": "distinct_file",
                },
            }
        )
    suite = {
        "schema_version": 3,
        "suite_id": suite_id,
        "repository_commit": manifest["repository_commit"],
        "file_universe": universe,
        "file_universe_digest": evaluator.universe_digest(universe),
        "comparison_contract": {
            "top_k": 10,
            "tokenizer": evaluator.TOKENIZER,
            "tokenizer_budget_version": evaluator.TOKENIZER_BUDGET_VERSION,
            "output_unit_policy": "rank_prefix",
            "span_unit": evaluator.SPAN_UNIT,
        },
        "routes": ["lexical", "semble-lexical-file"],
        "tasks": tasks,
        "diagnostic_policy": evaluator.OBSERVED_PREFIX_DIAGNOSTIC_POLICY,
    }
    checked, pack, _source = evaluator.validate_suite(repo, suite)
    return (
        checked,
        pack,
        {
            "seed": seed,
            "per_stratum": per_stratum,
            "strata": counts,
            "negative_tasks": negatives,
            "tasks": len(tasks),
            "selection_inputs": "frozen_source_only_no_product_outputs",
            "track": "native_content_identifier_file_relevance",
            "qualification": "exposed_corpus_diagnostic_not_independent_holdout",
        },
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
    inputs = parser.add_mutually_exclusive_group(required=True)
    inputs.add_argument("--baseline-suite", type=Path)
    inputs.add_argument("--corpus-manifest", type=Path)
    parser.add_argument("--suite-id")
    parser.add_argument("--seed", type=int, default=20261010)
    parser.add_argument("--per-stratum", type=int, default=15)
    parser.add_argument("--negatives", type=int, default=10)
    parser.add_argument("--output-root", required=True, type=Path)
    args = parser.parse_args()
    try:
        if args.corpus_manifest is None:
            manifest = write_suites(args.repo.resolve(), args.baseline_suite, args.output_root)
        else:
            evaluator.require(bool(args.suite_id), "identifier suite requires --suite-id")
            tool_files = _tool_digests()
            suite, pack, policy = identifier_word_suite(
                args.repo.resolve(),
                _baseline_from_bytes(read_control(args.corpus_manifest)),
                suite_id=args.suite_id,
                seed=args.seed,
                per_stratum=args.per_stratum,
                negatives=args.negatives,
            )
            output = args.output_root
            evaluator.require(
                output.is_absolute() and not output.exists(),
                "output root must be fresh and absolute",
            )
            parent = output.parent.resolve(strict=True)
            for checkout in (args.repo.resolve(), Path(__file__).resolve().parents[3]):
                evaluator.require(
                    not parent.is_relative_to(checkout),
                    "output root must be outside source and tool checkouts",
                )
            evaluator.require(
                _tool_digests() == tool_files, "tool source changed during derivation"
            )
            contents = {"suite.json": suite, "blind-pack.json": pack, "sampling.json": policy}
            manifest = {
                "schema_version": 1,
                "qualification": "diagnostic_unqualified",
                "repository_commit": suite["repository_commit"],
                "tool_files": tool_files,
                "artifacts": [
                    {"path": name, "sha256": evaluator.digest(_json_bytes(value))}
                    for name, value in contents.items()
                ],
            }
            output.mkdir()
            for name, value in {**contents, "manifest.json": manifest}.items():
                with (output / name).open("xb") as stream:
                    stream.write(_json_bytes(value))
    except (OSError, ValueError, source_oracle.SourceOracleError) as exc:
        parser.exit(2, f"ERROR: {exc}\n")
    print(json.dumps(manifest, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
