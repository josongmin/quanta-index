"""Admit pinned CodeSearchNet human annotations as a diagnostic review seed.

The upstream CSV contains repeated judgments for some query/code pairs. This
tool preserves each judgment and reports the upstream mean without converting
fractional grades into this repository's integer-grade benchmark contract.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import io
import json
import math
import os
import re
import sys
import tempfile
from collections import Counter, defaultdict
from pathlib import Path
from urllib.parse import unquote, urlsplit

try:
    from tools.benchmark import evidence
except ModuleNotFoundError:  # direct script invocation
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    from tools.benchmark import evidence


UPSTREAM_REPOSITORY = "https://github.com/github/CodeSearchNet"
UPSTREAM_COMMIT = "106e827405c968597da938f6b373d30183918869"
UPSTREAM_PATH = "resources/annotationStore.csv"
UPSTREAM_SHA256 = "0340af32b551ceadb74fec147f97642b7fedf3ff039e38fb86baff49ee899846"
UPSTREAM_URL = f"{UPSTREAM_REPOSITORY}/blob/{UPSTREAM_COMMIT}/{UPSTREAM_PATH}"
COLUMNS = ("Language", "Query", "GitHubUrl", "Relevance", "Notes")
LANGUAGES = frozenset(("Go", "Java", "JavaScript", "PHP", "Python", "Ruby"))
COMMIT_RE = re.compile(r"[0-9a-f]{40}\Z")
LINE_RE = re.compile(r"L([1-9][0-9]*)(?:-L([1-9][0-9]*))?\Z")
TOOL_CHECKOUT = Path(__file__).resolve().parents[3]


class AdmissionError(ValueError):
    """The downloaded annotations do not meet the frozen source contract."""


def _source_url(raw_url: str) -> tuple[str, str]:
    try:
        url = urlsplit(raw_url)
    except ValueError as error:
        raise AdmissionError("invalid GitHubUrl") from error
    parts = url.path.split("/", 5)
    if (
        url.scheme != "https"
        or url.netloc != "github.com"
        or url.query
        or len(parts) != 6
        or parts[0] != ""
        or not parts[1]
        or not parts[2]
        or parts[3] != "blob"
        or not COMMIT_RE.fullmatch(parts[4])
        or not parts[5]
        or any(part in ("", ".", "..") for part in unquote(parts[5]).split("/"))
    ):
        raise AdmissionError("GitHubUrl must pin a GitHub source file to a 40-hex commit")
    span = LINE_RE.fullmatch(url.fragment)
    if span is None or (span.group(2) and int(span.group(2)) < int(span.group(1))):
        raise AdmissionError("GitHubUrl must contain a valid line fragment")
    return f"{parts[1]}/{parts[2]}", parts[4]


def parse_annotations(raw: bytes) -> list[dict]:
    """Parse exact CSV records, retaining each annotator grade and optional note."""
    try:
        document = raw.decode("utf-8-sig", errors="strict")
        reader = csv.DictReader(io.StringIO(document, newline=""), strict=True)
        if tuple(reader.fieldnames or ()) != COLUMNS:
            raise AdmissionError("CodeSearchNet CSV columns differ from the pinned contract")
        records = []
        for record_index, row in enumerate(reader, start=1):
            if None in row or any(row.get(column) is None for column in COLUMNS):
                raise AdmissionError(f"CSV record {record_index} has missing or extra fields")
            language, query, url, grade_text, notes = (row[column] for column in COLUMNS)
            if language not in LANGUAGES or not query.strip():
                raise AdmissionError(f"CSV record {record_index} has invalid language or query")
            if not re.fullmatch(r"[0-3]", grade_text):
                raise AdmissionError(
                    f"CSV record {record_index} has noninteger or out-of-range grade"
                )
            repository, source_sha40 = _source_url(url)
            records.append(
                {
                    "record_index": record_index,
                    "language": language,
                    "query": query,
                    "github_url": url,
                    "repository": repository,
                    "source_sha40": source_sha40,
                    "grade": int(grade_text),
                    "notes": notes,
                }
            )
        if not records:
            raise AdmissionError("CodeSearchNet CSV has no annotation records")
        return records
    except (UnicodeError, csv.Error) as error:
        raise AdmissionError("CodeSearchNet CSV cannot be parsed") from error


def aggregate_annotations(raw: bytes) -> dict:
    """Aggregate testable CSV judgments without making any upstream source claim."""
    records = parse_annotations(raw)
    groups: dict[tuple[str, str, str], list[dict]] = defaultdict(list)
    for row in records:
        groups[(row["language"], row["query"], row["github_url"])].append(row)
    qrels = []
    disagreement_count = 0
    normalized_keys = set()
    for (language, query, url), judgments in sorted(groups.items()):
        normalized_key = (language.lower(), query.lower(), url)
        if normalized_key in normalized_keys:
            raise AdmissionError("query normalization collision in CodeSearchNet judgments")
        normalized_keys.add(normalized_key)
        histogram = Counter(row["grade"] for row in judgments)
        mean_grade = sum(row["grade"] for row in judgments) / len(judgments)
        if not math.isfinite(mean_grade):
            raise AdmissionError("mean relevance grade is nonfinite")
        disagreement = len(histogram) > 1
        disagreement_count += disagreement
        qrels.append(
            {
                "language": language.lower(),
                "query": query.lower(),
                "github_url": url,
                "repository": judgments[0]["repository"],
                "source_sha40": judgments[0]["source_sha40"],
                "mean_grade": mean_grade,
                "grade_histogram": {str(grade): histogram[grade] for grade in range(4)},
                "has_disagreement": disagreement,
                "annotations": [
                    {
                        "record_index": row["record_index"],
                        "language_original": row["language"],
                        "query_original": row["query"],
                        "grade": row["grade"],
                        "notes": row["notes"],
                    }
                    for row in judgments
                ],
            }
        )
    return {
        "counts": {
            "raw_annotations": len(records),
            "judged_query_language_url": len(qrels),
            "queries": len({row["query"] for row in qrels}),
            "query_language_pairs": len({(row["query"], row["language"]) for row in qrels}),
            "disagreed_qrels": disagreement_count,
        },
        "qrels": qrels,
    }


def diagnostic_seed(raw: bytes) -> dict:
    """Construct a review seed only from the pinned official upstream CSV bytes."""
    observed_sha256 = hashlib.sha256(raw).hexdigest()
    if observed_sha256 != UPSTREAM_SHA256:
        raise AdmissionError("CodeSearchNet CSV SHA-256 differs from pinned source")
    aggregated = aggregate_annotations(raw)
    return {
        "kind": "codesearchnet_human_qrels_review_seed_v1",
        "qualification": "diagnostic_unqualified",
        "source": {
            "repository": UPSTREAM_REPOSITORY,
            "commit": UPSTREAM_COMMIT,
            "path": UPSTREAM_PATH,
            "url": UPSTREAM_URL,
            "sha256": observed_sha256,
            "bytes": len(raw),
        },
        "contract": {
            "ranking_unit": "code_snippet_url",
            "query_unit": "natural_language",
            "grade_policy": "upstream_mean_of_raw_0_to_3_judgments",
            "unjudged_policy": "unknown",
            "local_human_review": False,
        },
        **aggregated,
    }


def _check_output(output: Path) -> None:
    if not output.is_absolute():
        raise AdmissionError("output path must be absolute")
    if not output.parent.is_dir():
        raise AdmissionError("output parent directory does not exist")
    if output.resolve(strict=False).is_relative_to(TOOL_CHECKOUT):
        raise AdmissionError("diagnostic output must be outside the tool checkout")


def capture(csv_path: Path, output: Path) -> dict:
    _check_output(output)
    raw = evidence.read_control(csv_path)
    seed = diagnostic_seed(raw)
    serialized = (
        json.dumps(seed, sort_keys=True, ensure_ascii=False, allow_nan=False) + "\n"
    ).encode()
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="wb", dir=output.parent, prefix=".csn-qrels-", delete=False
        ) as handle:
            temporary = Path(handle.name)
            handle.write(serialized)
            handle.flush()
            os.fsync(handle.fileno())
        os.link(temporary, output)  # Atomic create; never replace an existing receipt.
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
    return seed["counts"]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--csv", type=Path, required=True, help="Pinned upstream annotationStore.csv"
    )
    parser.add_argument(
        "--output", type=Path, required=True, help="New diagnostic review seed JSON"
    )
    args = parser.parse_args(argv)
    counts = capture(args.csv, args.output)
    print(json.dumps({"output": str(args.output), "counts": counts}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
