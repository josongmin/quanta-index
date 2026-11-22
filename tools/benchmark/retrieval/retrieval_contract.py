"""Stdlib-only retrieval wire helpers shared by evaluator and capture tools."""

from __future__ import annotations

import hashlib
import json
import math
import os
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
QUANTA_MODELED_ROUTES = frozenset({"semantic", "hybrid", "hybrid_file"})
QUANTA_NO_MODEL_ROUTES = frozenset({"lexical", "symbol"})


def validate_quanta_route_models(
    captures: dict, routes: dict, *, require_all_bound: bool = False
) -> tuple[str, str] | None:
    """Validate each Quanta route's model and return its modeled identity."""
    if not isinstance(captures, dict) or not isinstance(routes, dict) or not routes:
        raise ValueError("Quanta admission model lacks bound routes and captures")
    identities: set[tuple[str, str]] = set()
    bound_ids: set[str] = set()
    for route, binding in routes.items():
        if route not in QUANTA_NO_MODEL_ROUTES | QUANTA_MODELED_ROUTES or not isinstance(
            binding, dict
        ):
            raise ValueError("Quanta admission model route is malformed")
        capture_id = binding.get("capture_id")
        if not isinstance(capture_id, str) or not isinstance(captures.get(capture_id), dict):
            raise ValueError("Quanta admission model route has no capture")
        bound_ids.add(capture_id)
        model = captures[capture_id].get("model")
        revision = captures[capture_id].get("model_revision")
        if route in QUANTA_NO_MODEL_ROUTES:
            if (model, revision) != (f"none:{route}", "not-applicable"):
                raise ValueError("Quanta no-model route has an invalid model identity")
        else:
            if (
                not isinstance(model, str)
                or not model.strip()
                or model != model.strip()
                or model.startswith("none:")
                or not isinstance(revision, str)
                or not revision.strip()
                or revision != revision.strip()
                or revision == "not-applicable"
            ):
                raise ValueError("Quanta modeled route lacks a real model identity")
            identities.add((model, revision))
    if require_all_bound and bound_ids != set(captures):
        raise ValueError("Quanta admission model has unbound captures")
    if len(identities) > 1:
        raise ValueError("Quanta admission requires one consistent routed model identity")
    return next(iter(identities)) if identities else None


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def canonical(value: Any) -> bytes:
    return json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=False, allow_nan=False
    ).encode("utf-8")


def _score_bits(score: object, error: str) -> str:
    if type(score) not in (int, float):
        raise ValueError(error)
    try:
        number = float(score)
    except (OverflowError, ValueError) as exc:
        raise ValueError(error) from exc
    if not math.isfinite(number):
        raise ValueError(error)
    return struct.pack(">d", number).hex()


def completed_output_sha256(row: dict) -> str:
    """Bind normalized output across languages without guessing float formatting.

    Top-level timing is excluded. Explicit hybrid_file evidence also excludes
    its SDK explanation observations. Candidate and file-lane scores retain
    exact f64 bits; other values use the existing integer/string JSON domain.
    """
    if not isinstance(row, dict) or not isinstance(row.get("candidates"), list):
        raise ValueError("completed output must be a normalized result row")
    value = {key: item for key, item in row.items() if key != "timings"}
    candidates = []
    for candidate in value["candidates"]:
        if not isinstance(candidate, dict):
            raise ValueError("completed output candidate must be an object")
        if "score" in candidate:
            candidate = dict(candidate, score=_score_bits(
                candidate["score"], "completed output score must be finite f64"
            ))
        candidates.append(candidate)
    value["candidates"] = candidates
    if row.get("route") == "hybrid_file" and "hybrid_file_evidence" in value:
        evidence = value["hybrid_file_evidence"]
        if not isinstance(evidence, dict) or not isinstance(evidence.get("response"), dict):
            raise ValueError("completed hybrid file evidence must hold a response object")
        response = dict(evidence["response"])
        if "explanation" not in response or not isinstance(response.get("results"), list):
            raise ValueError("completed hybrid file response lacks results or explanation")
        del response["explanation"]

        def project_score(owner, key):
            if not isinstance(owner, dict) or key not in owner:
                raise ValueError("completed hybrid file score is missing")
            return {**owner, key: _score_bits(owner[key], "completed hybrid file score must be finite f64")}

        hits = []
        for hit in response["results"]:
            projected = project_score(hit, "file_rrf_score")
            for lane in ("lexical", "dense"):
                if lane not in hit:
                    raise ValueError("completed hybrid file lane witness is missing")
                witness = hit[lane]
                if witness is not None:
                    if not isinstance(witness, dict):
                        raise ValueError("completed hybrid file lane witness is malformed")
                    projected[lane] = dict(witness, candidate=project_score(witness.get("candidate"), "score"))
            hits.append(projected)
        response["results"] = hits
        value["hybrid_file_evidence"] = dict(evidence, response=response)

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
    initial_checkout = repo.stat()
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
    if _git(root, "rev-parse", "HEAD") != commit:
        raise ValueError("checkout HEAD differs from frozen commit")
    try:
        same_checkout = os.path.samestat(initial_checkout, repo.stat())
    except (FileNotFoundError, NotADirectoryError):
        same_checkout = False
    if not same_checkout:
        raise ValueError("checkout directory changed during verification")
    return root
