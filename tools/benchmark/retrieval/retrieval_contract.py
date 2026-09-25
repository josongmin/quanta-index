"""Stdlib-only retrieval wire helpers shared by evaluator and capture tools."""

from __future__ import annotations

import hashlib
import json
import re
import subprocess
from pathlib import Path
from typing import Any

TOKENIZER = "qi-regex-v1"
TOKENIZER_BUDGET_VERSION = "qb-v1"
TOKEN_RE = re.compile(r"[A-Za-z0-9_]+|[^\x00-\x20]")
OUTPUT_UNIT_POLICIES = ("rank_prefix",)
SPAN_UNIT = "byte_span_with_line_projection_v1"


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def canonical(value: Any) -> bytes:
    return json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=False, allow_nan=False
    ).encode("utf-8")


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
    root_text, separator, head = _git(
        repo, "rev-parse", "--show-toplevel", "HEAD"
    ).rpartition("\n")
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
