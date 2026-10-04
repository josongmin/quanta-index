"""Stdlib-only retrieval wire helpers shared by evaluator and capture tools."""

from __future__ import annotations

import hashlib
import json
import math
import re
import struct
import subprocess
from pathlib import Path
from typing import Any

# Gold production and replay share package identities; exact versions belong
# to the producer's source-locked runtime, including historical captures.
GOLD_RUNTIME_PACKAGES = (
    "regex",
    "tree-sitter",
    "tree-sitter-language-pack",
    "unicodedata2",
)

TOKENIZER = "qi-regex-v1"
TOKENIZER_BUDGET_VERSION = "qb-v1"
TOKEN_RE = re.compile(r"[A-Za-z0-9_]+|[^\x00-\x20]")
OUTPUT_UNIT_POLICIES = ("rank_prefix",)
SPAN_UNIT = "byte_span_with_line_projection_v1"
COMPLETED_OUTPUT_VALIDATION = "normalized_row_score_bits_sha256_v1"


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def canonical(value: Any) -> bytes:
    return json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=False, allow_nan=False
    ).encode("utf-8")


def completed_output_sha256(row: dict) -> str:
    """Bind normalized output across languages without guessing float formatting.

    Only top-level timing is excluded. Candidate scores retain their exact f64
    bits; the remaining value uses the existing integer/string JSON domain.
    """
    if not isinstance(row, dict) or not isinstance(row.get("candidates"), list):
        raise ValueError("completed output must be a normalized result row")
    value = {key: item for key, item in row.items() if key != "timings"}
    candidates = []
    for candidate in value["candidates"]:
        if not isinstance(candidate, dict):
            raise ValueError("completed output candidate must be an object")
        candidate = dict(candidate)
        if "score" in candidate:
            score = candidate["score"]
            if type(score) not in (int, float):
                raise ValueError("completed output score must be finite f64")
            try:
                score = float(score)
            except (OverflowError, ValueError) as exc:
                raise ValueError("completed output score must be finite f64") from exc
            if not math.isfinite(score):
                raise ValueError("completed output score must be finite f64")
            candidate["score"] = struct.pack(">d", score).hex()
        candidates.append(candidate)
    value["candidates"] = candidates

    def reject_floats(item):
        if isinstance(item, float):
            raise ValueError("completed output has a float outside candidate score")
        if isinstance(item, dict):
            for child in item.values():
                reject_floats(child)
        elif isinstance(item, list):
            for child in item:
                reject_floats(child)

    reject_floats(value)
    return digest(canonical(value))


def validate_comparison_contract(value: Any, where: str) -> dict[str, Any]:
    fields = {
        "top_k",
        "tokenizer",
        "tokenizer_budget_version",
        "output_unit_policy",
        "span_unit",
    }
    if not isinstance(value, dict) or set(value) != fields:
        raise ValueError(f"{where} has missing/unknown fields")
    if type(value["top_k"]) is not int or value["top_k"] <= 0:
        raise ValueError(f"{where}.top_k must be a positive integer")
    if value["tokenizer"] != TOKENIZER:
        raise ValueError(f"{where} tokenizer mismatch")
    if value["tokenizer_budget_version"] != TOKENIZER_BUDGET_VERSION:
        raise ValueError(f"{where} tokenizer/budget version mismatch")
    if value["output_unit_policy"] not in OUTPUT_UNIT_POLICIES:
        raise ValueError(f"{where} output_unit_policy is not a frozen policy")
    if value["span_unit"] != SPAN_UNIT:
        raise ValueError(f"{where} span_unit mismatch")
    return value


def _git(repo: Path, *args: str) -> str:
    try:
        result = subprocess.run(
            ["git", "-C", str(repo), *args], check=True, capture_output=True, text=True
        )
    except (OSError, subprocess.CalledProcessError) as exc:
        raise ValueError(f"repository Git evidence unavailable: {exc}") from exc
    return result.stdout.strip()


def verify_repo(repo: Path, commit: str) -> Path:
    if not repo.is_dir():
        raise ValueError(f"repository checkout missing: {repo}")
    root_text, separator, head = _git(repo, "rev-parse", "--show-toplevel", "HEAD").rpartition("\n")
    if not separator or not root_text or not head:
        raise ValueError("repository Git evidence unavailable")
    root = Path(root_text).resolve()
    if root != repo.resolve():
        raise ValueError("--repo must name the checkout root")
    if head != commit:
        raise ValueError("checkout HEAD differs from frozen commit")
    if _git(root, "status", "--porcelain", "--untracked-files=all"):
        raise ValueError("checkout has tracked or untracked changes")
    return root
