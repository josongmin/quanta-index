"""Admit CLARC Group 1 as paired, synthetic-file retrieval diagnostics.

Each upstream code snippet becomes exactly one synthetic source file. The
upstream query/code IDs supply one positive judgment per query; every other
snippet is unjudged. These are not repository files or declaration-oracle gold.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any

from tools.benchmark.retrieval import query_plan

DATASET_REPOSITORY = "https://huggingface.co/datasets/ClarcTeam/CLARC"
DATASET_COMMIT = "6c87a91da92bc0d06890efb19509db104da22ecb"
SOURCES = {
    "original": {
        "path": "reconstructed_group1_original_cleaned.json",
        "sha256": "cc52009b5c6a5087814fa3072a375761bd8690c928de58078dae7ae0ab38d6a6",
    },
    "neutral_renamed": {
        "path": "reconstructed_group1_neutral_renamed_cleaned.json",
        "sha256": "702931060b29fc024cb5bee27b2cbb0567d4bfce8c5a15471292b484c5624d7a",
    },
    "dataset_card": {"path": "README.md", "sha256": "42c32d7f1130e63bbb7f9a27b8d2cb29c2e91eeafefe7695abfba65d9fb54aca"},
    "project_license_info": {
        "path": "project_license_info.csv",
        "sha256": "9d465c3fa6a07122bc6315b6f7fd666bf943897a75f9e2c48975c67ad3984258",
    },
}
EXPECTED_PAIRS = 526
ROW_KEYS = frozenset(("query_id", "query_text", "code_id", "code_text", "relevance"))
QUERY_ID = re.compile(r"q_group_1_id_(0|[1-9][0-9]*)\Z")
CODE_ID = re.compile(r"c_group_1_id_(0|[1-9][0-9]*)\Z")
KIND = "clarc_group1_paired_synthetic_files_v1"


class ClarcAdmissionError(ValueError):
    """A source or row breaks the pinned CLARC Group 1 contract."""


def _sha256(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def _pinned(raw: bytes, source: str) -> dict[str, Any]:
    expected = SOURCES[source]
    digest = _sha256(raw)
    if digest != expected["sha256"]:
        raise ClarcAdmissionError(f"{source} SHA-256 differs from pinned CLARC bytes")
    return {
        "url": f"{DATASET_REPOSITORY}/blob/{DATASET_COMMIT}/{expected['path']}",
        "sha256": digest,
        "bytes": len(raw),
    }


def _parse_rows(raw: bytes, variant: str, expected_pairs: int) -> dict[int, dict[str, Any]]:
    try:
        rows = json.loads(raw.decode("utf-8"))
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise ClarcAdmissionError(f"{variant} is not UTF-8 JSON") from exc
    if not isinstance(rows, list) or len(rows) != expected_pairs:
        raise ClarcAdmissionError(f"{variant} must have {expected_pairs} rows")
    indexed: dict[int, dict[str, Any]] = {}
    for row in rows:
        if not isinstance(row, dict) or row.keys() != ROW_KEYS:
            raise ClarcAdmissionError(f"{variant} row fields differ from the frozen contract")
        query_id, code_id = row["query_id"], row["code_id"]
        query = QUERY_ID.fullmatch(query_id) if isinstance(query_id, str) else None
        code = CODE_ID.fullmatch(code_id) if isinstance(code_id, str) else None
        if query is None or code is None or query.group(1) != code.group(1):
            raise ClarcAdmissionError(f"{variant} query/code ID mismatch")
        index = int(query.group(1))
        if index in indexed:
            raise ClarcAdmissionError(f"{variant} duplicate query/code ID")
        if (
            not isinstance(row["query_text"], str)
            or not row["query_text"].strip()
            or not isinstance(row["code_text"], str)
            or not row["code_text"].strip()
            or type(row["relevance"]) is not int
            or row["relevance"] != 2
        ):
            raise ClarcAdmissionError(f"{variant} empty text or unexpected relevance")
        indexed[index] = row
    if set(indexed) != set(range(expected_pairs)):
        raise ClarcAdmissionError(f"{variant} query/code IDs are not contiguous")
    return indexed


def _validate_pair(original_raw: bytes, neutral_raw: bytes, expected_pairs: int) -> list[dict[str, Any]]:
    original = _parse_rows(original_raw, "original", expected_pairs)
    neutral = _parse_rows(neutral_raw, "neutral_renamed", expected_pairs)
    paired = []
    for index in range(expected_pairs):
        source, changed = original[index], neutral[index]
        if (
            source["query_id"] != changed["query_id"]
            or source["code_id"] != changed["code_id"]
            or source["query_text"] != changed["query_text"]
            or source["relevance"] != changed["relevance"]
        ):
            raise ClarcAdmissionError(f"paired source identity differs at ID {index}")
        paired.append({"original": source, "neutral_renamed": changed})
    return paired


def _duplicate_content(rows: list[dict[str, Any]], variant: str) -> dict[str, Any]:
    by_digest: dict[str, list[str]] = defaultdict(list)
    for pair in rows:
        row = pair[variant]
        by_digest[_sha256(row["code_text"].encode("utf-8"))].append(row["code_id"])
    groups = [ids for ids in by_digest.values() if len(ids) > 1]
    return {"groups": len(groups), "affected_code_ids": sum(map(len, groups)), "max_group_size": max(map(len, groups), default=1)}


def plan_admission(rows: list[dict[str, Any]]) -> dict[str, Any]:
    """Preflight the existing file-search route with unchanged query text."""
    admitted: list[str] = []
    refused: list[dict[str, str]] = []
    for pair in rows:
        row = pair["original"]
        try:
            query_plan.plan_lexical_request("natural_language_file", row["query_text"])
        except query_plan.QueryPlanError as exc:
            refused.append({"query_id": row["query_id"], "reason": str(exc)})
        else:
            admitted.append(row["query_id"])
    return {"requested": len(rows), "admitted_query_ids": admitted, "refused": refused}


def admit_pinned_pair(
    original_raw: bytes, neutral_raw: bytes, dataset_card_raw: bytes, license_info_raw: bytes
) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    """Bind all external inputs before producing local-only corpus records."""
    sources = {
        key: _pinned(raw, key)
        for key, raw in (
            ("original", original_raw),
            ("neutral_renamed", neutral_raw),
            ("dataset_card", dataset_card_raw),
            ("project_license_info", license_info_raw),
        )
    }
    if not dataset_card_raw.startswith(b"---\nlicense: cc-by-sa-4.0\n"):
        raise ClarcAdmissionError("pinned dataset card does not declare CC BY-SA 4.0")
    if not license_info_raw.startswith(b"project_name,license_info,url\n"):
        raise ClarcAdmissionError("pinned project license information has no expected header")
    rows = _validate_pair(original_raw, neutral_raw, EXPECTED_PAIRS)
    admission = plan_admission(rows)
    metadata = {
        "kind": KIND,
        "qualification": "diagnostic_unqualified",
        "source": {"dataset": DATASET_REPOSITORY, "commit": DATASET_COMMIT, "artifacts": sources},
        "contract": {
            "query_unit": "unchanged_natural_language",
            "ranking_unit": "one_synthetic_file_per_code_snippet",
            "file_suffix": ".cpp",
            "qrel_policy": "one_upstream_positive_per_query_other_candidates_unjudged",
            "positive_relevance": 2,
            "source_oracle": "not_applicable",
            "source_repository_file_identity": "not_provided_by_group1_rows",
            "license_status": "dataset_card_claims_cc_by_sa_4_0_project_attribution_and_redistribution_unverified",
        },
        "pairs": EXPECTED_PAIRS,
        "admission": admission,
        "duplicate_content": {
            variant: _duplicate_content(rows, variant) for variant in ("original", "neutral_renamed")
        },
    }
    return rows, metadata


def materialize(
    original_raw: bytes,
    neutral_raw: bytes,
    dataset_card_raw: bytes,
    license_info_raw: bytes,
    output_root: Path,
) -> dict[str, Any]:
    """Create two isolated file universes and positive-only qrels in a new root."""
    rows, metadata = admit_pinned_pair(
        original_raw, neutral_raw, dataset_card_raw, license_info_raw
    )
    if not output_root.is_absolute() or not output_root.parent.is_dir():
        raise ClarcAdmissionError("output root must be absolute with an existing parent")
    checkout = Path(__file__).resolve().parents[3]
    if output_root.resolve(strict=False).is_relative_to(checkout):
        raise ClarcAdmissionError("output root must be outside the source checkout")
    output_root.mkdir(exist_ok=False)
    files: dict[str, list[dict[str, Any]]] = {"original": [], "neutral_renamed": []}
    qrels = []
    for pair in rows:
        source = pair["original"]
        filename = source["code_id"] + ".cpp"
        relative = "snippets/" + filename
        qrels.append(
            {
                "query_id": source["query_id"],
                "query_text": source["query_text"],
                "query_sha256": _sha256(source["query_text"].encode("utf-8")),
                "positive_code_id": source["code_id"],
                "positive_file": relative,
                "relevance": 2,
            }
        )
        for variant in files:
            row = pair[variant]
            destination = output_root / variant / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            raw = row["code_text"].encode("utf-8")
            with destination.open("xb") as handle:
                handle.write(raw)
            files[variant].append(
                {"code_id": row["code_id"], "path": relative, "sha256": _sha256(raw), "bytes": len(raw)}
            )
    manifest = {**metadata, "corpora": files, "qrels": qrels}
    with (output_root / "manifest.json").open("x", encoding="utf-8") as handle:
        json.dump(manifest, handle, sort_keys=True, ensure_ascii=False, indent=2)
        handle.write("\n")
    return manifest


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--original", type=Path, required=True)
    parser.add_argument("--neutral-renamed", type=Path, required=True)
    parser.add_argument("--dataset-card", type=Path, required=True)
    parser.add_argument("--project-license-info", type=Path, required=True)
    parser.add_argument("--output-root", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        manifest = materialize(
            args.original.read_bytes(),
            args.neutral_renamed.read_bytes(),
            args.dataset_card.read_bytes(),
            args.project_license_info.read_bytes(),
            args.output_root,
        )
    except (ClarcAdmissionError, OSError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2
    print(json.dumps({"output_root": str(args.output_root), "pairs": manifest["pairs"], "admission": manifest["admission"]}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
