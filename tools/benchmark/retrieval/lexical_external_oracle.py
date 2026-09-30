#!/usr/bin/env python3
"""Rescore preserved external file rows against source-derived lexical labels.

This is an offline diagnostic. It does not attest backend index inventories or
make native chunk rankings equivalent to distinct-file rankings.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

from tools.benchmark.retrieval import lexical_file_comparison as comparison
from tools.benchmark.retrieval import source_oracle_suite
from tools.benchmark.retrieval.evaluator import canonical

ROLES = (
    "suite",
    "query_pack",
    "capture_manifest",
    "sourcegraph_rows",
    "opengrok_rows",
    "cs_rows",
)
MODES = ("identifier-word-file", "go-declaration-file")


def verify_capture_manifest(paths: dict[str, Path], suite: dict, pack: dict) -> str:
    manifest_path = paths["capture_manifest"]
    manifest = comparison._read(manifest_path)
    binding = manifest.get("binding")
    if (
        type(manifest.get("schema_version")) is not int
        or manifest["schema_version"] != 1
        or manifest.get("status") != "diagnostic_unqualified"
        or manifest.get("tasks") != len(pack["tasks"])
        or not isinstance(binding, dict)
        or binding.get("repository_commit") != suite["repository_commit"]
        or binding.get("file_universe_digest") != "sha256:" + suite["file_universe_digest"]
        or binding.get("suite_digest") != "sha256:" + comparison._sha(paths["suite"])
        or binding.get("query_pack_digest") != "sha256:" + comparison._sha(paths["query_pack"])
        or manifest.get("rows_sha256")
        != {product: comparison._sha(paths[f"{product}_rows"]) for product in comparison.PRODUCTS}
    ):
        raise ValueError("external capture manifest does not bind suite, pack, and rows")
    expected_raw = {
        f"{product}/{task['task_id']}.{suffix}"
        for task in pack["tasks"]
        for product, suffixes in (
            ("sourcegraph", ("stream", "transport.json")),
            ("opengrok", ("json", "transport.json")),
            ("cs", ("json", "process.json", "stderr")),
        )
        for suffix in suffixes
    }
    declared_raw = manifest.get("raw_capture_sha256")
    if not isinstance(declared_raw, dict) or set(declared_raw) != expected_raw:
        raise ValueError("external capture raw response inventory differs")
    root = manifest_path.parent.resolve(strict=True)
    for name, claimed in declared_raw.items():
        path = manifest_path.parent / name
        if (
            not isinstance(claimed, str)
            or not comparison._canonical_result_path(name)
            or path.is_symlink()
            or path.resolve(strict=True).parent != root / name.split("/", 1)[0]
            or comparison._sha(path) != claimed
        ):
            raise ValueError(f"external raw response digest differs: {name}")
    return comparison._sha(manifest_path)


def evaluate(paths: dict[str, Path], repo: Path) -> dict:
    if set(paths) != set(ROLES) or any(not isinstance(path, Path) for path in paths.values()):
        raise ValueError("external oracle requires the exact input roles")
    suite_raw = comparison._bytes(paths["suite"])
    pack_raw = comparison._bytes(paths["query_pack"])
    suite, pack = comparison._json(suite_raw), comparison._json(pack_raw)
    universe = comparison._file_universe(suite, pack)
    expected = comparison._tasks(suite, pack)
    if any(path not in universe for _, gold in expected.values() for path in gold):
        raise ValueError("capture gold is outside the frozen file universe")
    manifest_sha256 = verify_capture_manifest(paths, suite, pack)

    derived = source_oracle_suite.derive_suites(repo, suite)
    modes = {}
    for mode in MODES:
        oracle_suite, oracle_pack = derived[mode]
        if [(task["task_id"], task["query"]) for task in oracle_pack["tasks"]] != [
            (task["task_id"], task["query"]) for task in pack["tasks"]
        ]:
            raise ValueError("source oracle query inventory differs from capture")
        gold = {
            task["task_id"]: sorted({item["path"] for item in task["file_judgments"]})
            for task in oracle_suite["tasks"]
        }
        if any(
            not {label["path"] for label in task["gold"]}.issubset(gold[task["task_id"]])
            or bool(task["gold"]) is not bool(gold[task["task_id"]])
            for task in oracle_suite["tasks"]
        ):
            raise ValueError("source oracle representative gold differs from file judgments")
        modes[mode] = {
            "oracle_suite_sha256": hashlib.sha256(canonical(oracle_suite)).hexdigest(),
            "query_pack_sha256": hashlib.sha256(canonical(oracle_pack)).hexdigest(),
            "rank_unit": "distinct_file",
            "products": {
                product: comparison.product_result(
                    product,
                    paths[f"{product}_rows"],
                    expected,
                    universe,
                    scoring_gold=gold,
                )
                for product in comparison.PRODUCTS
            },
        }
    return {
        "status": "diagnostic_unqualified",
        "repository_commit": suite["repository_commit"],
        "file_universe_digest": suite["file_universe_digest"],
        "capture_suite_sha256": hashlib.sha256(suite_raw).hexdigest(),
        "capture_query_pack_sha256": hashlib.sha256(pack_raw).hexdigest(),
        "capture_manifest_sha256": manifest_sha256,
        "validator_sources_sha256": {
            name: comparison._sha(Path(__file__).with_name(name))
            for name in (
                "lexical_external_oracle.py",
                "lexical_file_comparison.py",
                "source_oracle_suite.py",
                "source_oracle.py",
                "evaluator.py",
            )
        },
        "source_oracle_policy": "source_oracle_complete_v1",
        "modes": modes,
        "exclusions": [
            "human_relevance_review",
            "external_backend_indexed_universe_attestation",
            "native_response_row_replay_at_capture_source",
            "cross_product_latency_equivalence",
            "native_chunk_rank_equivalence",
        ],
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    for role in ROLES:
        parser.add_argument("--" + role.replace("_", "-"), required=True, type=Path)
    args = parser.parse_args()
    paths = {role: getattr(args, role) for role in ROLES}
    try:
        if not args.out.is_absolute() or args.out.exists() or args.out.is_symlink():
            raise ValueError("output must be a new absolute path")
        result = evaluate(paths, args.repo)
        args.out.parent.mkdir(parents=True, exist_ok=True)
        with args.out.open("x", encoding="utf-8") as stream:
            json.dump(result, stream, indent=2, sort_keys=True)
            stream.write("\n")
    except (OSError, ValueError) as exc:
        parser.error(str(exc))


if __name__ == "__main__":
    main()
