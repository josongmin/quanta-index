"""Detached diagnostic capture for a reviewed OpenAI A/B runner.

This does not approve a caller-supplied source or binary. A release owner must
first review that pair; this function enforces its exact bytes and refuses any
partial or cross-source result before publishing the three original sidecars.
"""

from __future__ import annotations

import ctypes
import errno
import hashlib
import importlib.util
import json
import math
import os
import re
import stat
import subprocess
import sys
import time
from collections.abc import Iterable, Mapping
from pathlib import Path
from tempfile import TemporaryDirectory
from types import ModuleType
from typing import Any

from benchctl import require_frozen_source
from evidence import (
    DIGEST_RE,
    HEX_RE,
    EvidenceError,
    RawFile,
    canonical_json,
    is_digest,
    parse_json,
    read_control,
)
from producer_execution import ProducerExecutionError, execute

DIMENSION = "relevance-openai-ab"
OUTPUT_NAMES = ("summary.json", "cases.json", "provider-stats.json")
CASE_COUNT = 13
PARAPHRASE_COUNT = 12
RANK_K = 10
RECALL_K = 20
REQUEST_SAMPLE_CAPACITY = 256
LEGACY_DAEMON_BIN = "QUANTA_INDEX_LEGACY_SEARCHD_BIN"
LEGACY_DAEMON_SHA256 = "QUANTA_INDEX_LEGACY_SEARCHD_SHA256"
APPROVED_AB_INPUT_DIGEST = "QUANTA_INDEX_OPENAI_AB_APPROVED_INPUT_DIGEST"
OPENAI_REQUEST_COST = "QUANTA_INDEX_PROVIDER_OPENAI_MAX_REQUEST_COST_MICROS"
OPENAI_COST_CEILING = "QUANTA_INDEX_PROVIDER_COST_CEILING_MICROS"
_JUDGED_SOURCE = "crates/quanta-index-searchd-harness/src/relevance/semantic_judgments.tsv"
_ARCHIVED_JUDGED_SOURCE = "crates/quanta-index-searchd-harness/src/relevance/corpus.rs"
_JUDGED_COLUMNS = ("id", "intent_kind", "intent", "query", "on_topic_path", "hard_negative_path")
_STRING_LITERAL = r'"(?:[^"\\]|\\.)*"'
_JUDGED_CASE = re.compile(
    rf"SemanticJudgedQuery\s*\{{\s*id:\s*(?P<id>{_STRING_LITERAL}),\s*"
    r"intent_kind:\s*SemanticIntentKind::(?P<intent_kind>ExactToken|Paraphrase),\s*"
    rf"intent:\s*(?P<intent>{_STRING_LITERAL}),\s*query:\s*(?P<query>{_STRING_LITERAL}),\s*"
    rf"on_topic_path:\s*(?P<on_topic_path>{_STRING_LITERAL}),\s*"
    rf"hard_negative_path:\s*(?P<hard_negative_path>{_STRING_LITERAL}),\s*\}},"
)
_CHECKER_PATH = Path(__file__).resolve().parents[1] / "ci/lint/check-bench-artifacts.py"


def _require(condition: bool, reason: str) -> None:
    if not condition:
        raise EvidenceError(f"detached OpenAI A/B refused: {reason}")


def _unsigned(value: object, *, positive: bool = False) -> bool:
    return type(value) is int and 0 <= value <= (1 << 64) - 1 and (not positive or value > 0)


def _checker() -> ModuleType:
    """Use the existing BenchArtifactV1 envelope oracle without CLI HEAD assumptions."""
    name = "_quanta_detached_ab_artifact_checker"
    if name in sys.modules:
        return sys.modules[name]
    spec = importlib.util.spec_from_file_location(name, _CHECKER_PATH)
    if spec is None or spec.loader is None:
        raise EvidenceError("detached OpenAI A/B artifact checker is unavailable")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def _git_source(checkout: Path, *args: str) -> str:
    committed = subprocess.run(
        ["git", "-C", str(checkout), *args],
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )
    _require(committed.returncode == 0, "approved checkout has no semantic judgment fixture")
    return committed.stdout


def _archived_cases(source: str) -> list[dict[str, str]]:
    array = re.search(
        r"^pub const SEMANTIC_JUDGED_QUERIES:\s*&\[SemanticJudgedQuery\]\s*=\s*&\[(.*?)^\];",
        source,
        re.MULTILINE | re.DOTALL,
    )
    _require(array is not None, "approved checkout has no semantic judgment array")
    remaining = re.sub(r"^[ \t]*//[^\n]*", "", array[1], flags=re.MULTILINE).strip()
    cases = []
    while remaining:
        case = _JUDGED_CASE.match(remaining)
        _require(case is not None, "approved semantic judgment is outside the fixture format")
        fields = case.groupdict()
        kind = fields.pop("intent_kind")
        cases.append(
            {
                **{field: json.loads(literal) for field, literal in fields.items()},
                "intent_kind": {"ExactToken": "exact_token", "Paraphrase": "paraphrase"}[kind],
            }
        )
        remaining = remaining[case.end() :].lstrip()
    return cases


def _source_cases(checkout: Path, head: str) -> tuple[list[dict[str, str]], bool]:
    uses_tsv = bool(_git_source(checkout, "ls-tree", head, "--", _JUDGED_SOURCE))
    if uses_tsv:
        lines = (
            _git_source(checkout, "show", f"{head}:{_JUDGED_SOURCE}").removesuffix("\n").split("\n")
        )
        _require(
            lines and tuple(lines[0].split("\t")) == _JUDGED_COLUMNS,
            "approved semantic judgment columns differ",
        )
        cases = []
        for line in lines[1:]:
            columns = line.split("\t")
            _require(
                len(columns) == len(_JUDGED_COLUMNS), "approved semantic judgment columns differ"
            )
            cases.append(dict(zip(_JUDGED_COLUMNS, columns, strict=True)))
    else:
        cases = _archived_cases(_git_source(checkout, "show", f"{head}:{_ARCHIVED_JUDGED_SOURCE}"))
    _require(len(cases) == CASE_COUNT, "approved semantic judgment roster length differs")
    ids = {case["id"] for case in cases}
    _require(len(ids) == CASE_COUNT, "approved semantic judgment IDs repeat")
    _require(
        sum(case["intent_kind"] == "paraphrase" for case in cases) == PARAPHRASE_COUNT,
        "approved semantic judgment kinds differ",
    )
    for case in cases:
        suffix = {"exact_token": ".exact", "paraphrase": ".paraphrase"}.get(case["intent_kind"])
        _require(
            suffix
            and case["id"].startswith("sem.")
            and case["id"].endswith(suffix)
            and all(value.strip() for value in case.values())
            and case["on_topic_path"] != case["hard_negative_path"],
            "approved semantic judgment is invalid",
        )
    return cases, uses_tsv


def _framed_digest(domain: str, parts: Iterable[bytes]) -> str:
    digest = hashlib.sha256()
    digest.update(domain.encode())
    digest.update(b"\0")
    for raw in parts:
        digest.update(str(len(raw)).encode())
        digest.update(b"\0")
        digest.update(raw)
    return "sha256:" + digest.hexdigest()


def _framed_config_digest(dimension: str, entries: Mapping[str, str]) -> str:
    return _framed_digest(
        f"quanta-index:bench:{dimension}:config:v1",
        (f"{key}={value}".encode() for key, value in sorted(entries.items())),
    )


def _judgments_digest(cases: list[dict[str, str]]) -> str:
    return _framed_digest(
        "quanta-index:bench:relevance-openai-ab:judgments:v1",
        (case[column].encode("utf-8") for case in cases for column in _JUDGED_COLUMNS),
    )


def _config_digest(runner_config: dict[str, Any]) -> str:
    # Preserve serde_json::Value's serialized object order.
    return _framed_config_digest(
        DIMENSION,
        {
            "rank_k": str(RANK_K),
            "recall_k": str(RECALL_K),
            "cases": str(CASE_COUNT),
            "runner_config": json.dumps(runner_config, separators=(",", ":"), ensure_ascii=False),
        },
    )


def _approved_input_digest(
    config_digest: str, source_batch_digest: str, judgments_digest: str | None
) -> str:
    entries = {"config_digest": config_digest, "source_batch_digest": source_batch_digest}
    if judgments_digest is not None:
        entries["judgments_digest"] = judgments_digest
    return _framed_config_digest("relevance-openai-ab-inputs", entries)


def _configured_bool(name: str, raw: str) -> bool:
    value = raw.strip().lower()
    _require(value in {"1", "true", "on", "yes", "0", "false", "off", "no"}, f"invalid {name}")
    return value in {"1", "true", "on", "yes"}


def _configured_uint(name: str, raw: str) -> int:
    value = raw.strip()
    _require(re.fullmatch(r"\+?[0-9]+", value) is not None, f"invalid {name}")
    digits = value.lstrip("+").lstrip("0")
    _require(len(digits) <= 20, f"invalid {name}")
    parsed = int(digits or "0")
    _require(_unsigned(parsed), f"invalid {name}")
    return parsed


def _grant_digest(env: Mapping[str, str]) -> str:
    prefix = "QUANTA_INDEX_PROVIDER_"
    fields = {
        field: env.get(f"{prefix}{field.upper()}", "").strip()
        for field in ("tenant", "endpoint", "region", "retention", "profile")
    }
    consent_name = f"{prefix}SOURCE_CONTENT_CONSENT"
    consent = bool(env.get(consent_name)) and _configured_bool(consent_name, env[consent_name])
    _require(
        all(fields.values()) and consent, "provider grant is incomplete for Source publication"
    )
    return _framed_config_digest(
        "relevance-openai-ab-provider-grant",
        {
            **fields,
            "source_content_consent": "true",
        },
    )


def _validate_approved_settings(
    config: dict[str, Any],
    env: Mapping[str, str],
    grant_digest: str,
) -> None:
    openai = config["openai"]
    numbered = (
        (
            openai,
            "QUANTA_INDEX_EMBED_",
            {
                "DIM": "dimension",
                "BATCH": "max_batch",
                "MAX_EST_TOKENS": "max_estimated_tokens_per_request",
                "MAX_RETRIES": "max_retries",
                "TIMEOUT_SECS": "timeout_secs",
                "CONCURRENCY": "concurrency",
            },
        ),
        (
            openai["cache_retention"],
            "QUANTA_INDEX_EMBED_CACHE_",
            {
                "MAX_ENTRIES": "max_entries",
                "MAX_BYTES": "max_resident_bytes",
                "MAX_NAMESPACES": "max_namespaces",
                "MAX_AGE_SECS": "max_entry_age_secs",
                "MAX_TOTAL_BYTES": "max_total_bytes",
            },
        ),
        (
            config["provider_work_budget"],
            "QUANTA_INDEX_PROVIDER_",
            {
                "MAX_INFLIGHT_REQUESTS": "inflight_requests_cap",
                "MAX_INFLIGHT_BYTES": "inflight_bytes_cap",
                "COST_CEILING_MICROS": "total_cost_micros_ceiling",
                "MAX_RETRY_ATTEMPTS": "retry_attempts_cap",
                "OPENAI_MAX_REQUEST_COST_MICROS": "openai_max_request_cost_micros",
            },
        ),
    )
    for reported, prefix, keys in numbered:
        for suffix, field in keys.items():
            name = prefix + suffix
            if env.get(name) or (name == "QUANTA_INDEX_EMBED_DIM" and name in env):
                expected = _configured_uint(name, env[name])
                _require(reported.get(field) == expected, f"runner config differs from {name}")
    for name, reported, field in (
        ("QUANTA_INDEX_EMBED_TIMEOUT_SECS", openai, "timeout_subsec_nanos"),
        (
            "QUANTA_INDEX_EMBED_CACHE_MAX_AGE_SECS",
            openai["cache_retention"],
            "max_entry_age_subsec_nanos",
        ),
    ):
        if env.get(name):
            _require(reported[field] == 0, f"runner config differs from {name}")
    if "QUANTA_INDEX_EMBED_MODEL" in env:
        _require(
            openai["model"] == env["QUANTA_INDEX_EMBED_MODEL"],
            "runner config differs from OpenAI model",
        )
    if "QUANTA_INDEX_EMBED_MODEL_REVISION" in env:
        _require(
            openai["model_revision"] == env["QUANTA_INDEX_EMBED_MODEL_REVISION"].strip(),
            "runner config differs from OpenAI model revision",
        )
    if env.get("QUANTA_INDEX_EMBED_CACHE"):
        expected = _configured_bool("QUANTA_INDEX_EMBED_CACHE", env["QUANTA_INDEX_EMBED_CACHE"])
        _require(
            openai["cache_enabled"] == expected,
            "runner config differs from QUANTA_INDEX_EMBED_CACHE",
        )
    _require(
        config["provider_egress_grant_digest"] == grant_digest,
        "runner config differs from provider grant",
    )


def _validate_runner_config(value: object) -> dict[str, Any]:
    _require(isinstance(value, dict), "runner_config is not an object")
    _require(
        set(value)
        == {
            "schema_version",
            "hash",
            "openai",
            "provider_work_budget",
            "provider_egress_grant_digest",
        },
        "runner_config keys differ",
    )
    _require(
        type(value["schema_version"]) is int and value["schema_version"] == 1,
        "runner_config schema differs",
    )
    hash_profile = value["hash"]
    openai = value["openai"]
    budget = value["provider_work_budget"]
    _require(
        isinstance(hash_profile, dict) and set(hash_profile) == {"dimension", "model_revision"},
        "Hash arm config differs",
    )
    _require(_unsigned(hash_profile["dimension"], positive=True), "Hash dimension is invalid")
    _require(
        isinstance(hash_profile["model_revision"], str) and bool(hash_profile["model_revision"]),
        "Hash model revision is missing",
    )
    openai_keys = {
        "model",
        "model_revision",
        "dimension",
        "max_batch",
        "max_estimated_tokens_per_request",
        "max_retries",
        "timeout_secs",
        "timeout_subsec_nanos",
        "cache_enabled",
        "cache_retention",
        "concurrency",
    }
    _require(isinstance(openai, dict) and set(openai) == openai_keys, "OpenAI arm config differs")
    _require(
        all(isinstance(openai[key], str) and openai[key] for key in ("model", "model_revision")),
        "OpenAI model identity is missing",
    )
    for key in (
        "dimension",
        "max_batch",
        "max_estimated_tokens_per_request",
        "concurrency",
    ):
        _require(_unsigned(openai[key], positive=True), f"OpenAI {key} is invalid")
    _require(_unsigned(openai["max_retries"]), "OpenAI max_retries is invalid")
    _require(openai["max_retries"] <= (1 << 32) - 1, "OpenAI max_retries exceeds u32")
    _require(_unsigned(openai["timeout_secs"]), "OpenAI timeout seconds are invalid")
    _require(
        _unsigned(openai["timeout_subsec_nanos"])
        and openai["timeout_subsec_nanos"] < 1_000_000_000,
        "OpenAI timeout subseconds are invalid",
    )
    _require(
        openai["timeout_secs"] > 0 or openai["timeout_subsec_nanos"] > 0, "OpenAI timeout is zero"
    )
    _require(type(openai["cache_enabled"]) is bool, "OpenAI cache flag is invalid")
    retention = openai["cache_retention"]
    retention_keys = {
        "max_entries",
        "max_resident_bytes",
        "max_namespaces",
        "max_entry_age_secs",
        "max_entry_age_subsec_nanos",
        "max_total_bytes",
    }
    _require(
        isinstance(retention, dict) and set(retention) == retention_keys,
        "OpenAI cache retention differs",
    )
    for key in retention_keys - {"max_entry_age_secs", "max_entry_age_subsec_nanos"}:
        _require(_unsigned(retention[key], positive=True), f"cache {key} is invalid")
    _require(_unsigned(retention["max_entry_age_secs"]), "cache age seconds are invalid")
    _require(
        _unsigned(retention["max_entry_age_subsec_nanos"])
        and retention["max_entry_age_subsec_nanos"] < 1_000_000_000,
        "cache age subseconds are invalid",
    )
    _require(
        retention["max_entry_age_secs"] > 0 or retention["max_entry_age_subsec_nanos"] > 0,
        "cache age is zero",
    )
    _require(
        retention["max_total_bytes"] >= retention["max_resident_bytes"],
        "cache total bytes cannot hold a namespace",
    )
    budget_keys = {
        "inflight_requests_cap",
        "inflight_bytes_cap",
        "total_cost_micros_ceiling",
        "retry_attempts_cap",
    }
    _require(
        isinstance(budget, dict)
        and set(budget) in (budget_keys, budget_keys | {"openai_max_request_cost_micros"}),
        "provider budget keys differ",
    )
    for key in budget:
        _require(_unsigned(budget[key], positive=True), f"provider budget {key} is invalid")
    _require(budget["retry_attempts_cap"] <= (1 << 32) - 1, "provider retry cap exceeds u32")
    grant_digest = value["provider_egress_grant_digest"]
    _require(
        isinstance(grant_digest, str) and DIGEST_RE.fullmatch(grant_digest) is not None,
        "provider grant digest is invalid",
    )
    return openai


def _validate_stats(value: object, config: dict[str, Any]) -> None:
    keys = {
        "schema_version",
        "total_texts_observed",
        "cache_hits",
        "distinct_miss_texts",
        "http_request_count",
        "retry_count",
        "retryable_status_count",
        "transport_error_count",
        "max_request_texts",
        "max_estimated_tokens",
        "request_samples_dropped",
        "request_samples",
    }
    _require(isinstance(value, dict) and set(value) == keys, "provider stats shape differs")
    _require(
        type(value["schema_version"]) is int and value["schema_version"] == 2,
        "provider stats schema differs",
    )
    for key in keys - {"schema_version", "request_samples"}:
        _require(_unsigned(value[key]), f"provider stat {key} is invalid")
    observed_texts = value["total_texts_observed"]
    cache_observations = value["cache_hits"] + value["distinct_miss_texts"]
    if config["cache_enabled"]:
        _require(observed_texts > 0, "provider observed no texts")
        _require(
            cache_observations <= observed_texts,
            "provider cache observations exceed observed texts",
        )
    else:
        _require(
            observed_texts == cache_observations == 0, "disabled cache reported cache observations"
        )
        _require(value["http_request_count"] > 0, "disabled cache observed no provider requests")
    failures = value["retryable_status_count"] + value["transport_error_count"]
    _require(failures <= value["http_request_count"], "provider failures exceed observed requests")
    _require(value["retry_count"] <= failures, "provider retries exceed retryable failures")
    _require(
        value["max_request_texts"] <= config["max_batch"],
        "provider run maximum exceeds configured OpenAI max_batch",
    )
    samples = value["request_samples"]
    _require(
        isinstance(samples, list)
        and len(samples) == min(value["http_request_count"], REQUEST_SAMPLE_CAPACITY),
        "provider request sample window differs",
    )
    _require(
        value["http_request_count"] == len(samples) + value["request_samples_dropped"],
        "provider request accounting is incomplete",
    )
    for index, sample in enumerate(samples):
        _require(
            isinstance(sample, dict) and set(sample) == {"texts_submitted", "estimated_tokens"},
            f"request sample {index} differs",
        )
        _require(
            _unsigned(sample["texts_submitted"], positive=True)
            and _unsigned(sample["estimated_tokens"]),
            f"request sample {index} is invalid",
        )
        # The request packer permits one oversized text as a singleton.
        _require(
            sample["texts_submitted"] == 1
            or sample["estimated_tokens"] <= config["max_estimated_tokens_per_request"],
            f"request sample {index} exceeds the configured estimated-token limit",
        )
    for sample_key, maximum_key in (
        ("texts_submitted", "max_request_texts"),
        ("estimated_tokens", "max_estimated_tokens"),
    ):
        observed = max((sample[sample_key] for sample in samples), default=0)
        maximum = value[maximum_key]
        _require(
            observed <= maximum and (value["request_samples_dropped"] > 0 or observed == maximum),
            f"provider {maximum_key} disagrees with run maxima",
        )


def _validate_reported_values(
    reported: dict[str, Any], expected: dict[str, Any], where: str
) -> None:
    for key, target in expected.items():
        actual = reported.get(key)
        if type(target) is float:
            # Rust and Python can differ by a few ULPs for log2 and summation.
            matches = (
                type(actual) is float
                and abs(actual) <= 1.0
                and math.isclose(actual, target, rel_tol=1e-12)
            )
        else:
            matches = key in reported and type(actual) is type(target) and actual == target
        _require(matches, f"{where} {key} differs from ranked results")


def _case_scores(order: list[str], on_topic: str, hard_negative: str) -> dict[str, Any]:
    on_topic_rank = order.index(on_topic) + 1 if on_topic in order else None
    ranked_in_top_k = on_topic_rank is not None and on_topic_rank <= RANK_K
    return {
        "on_topic_rank": on_topic_rank,
        "hard_negative_rank": (order.index(hard_negative) + 1 if hard_negative in order else None),
        "top1_is_on_topic": on_topic_rank == 1,
        "mrr_at_10": 1 / on_topic_rank if ranked_in_top_k else 0.0,
        # The one positive judgment supplies the entire ideal gain.
        "ndcg_at_10": 1 / math.log2(on_topic_rank + 1) if ranked_in_top_k else 0.0,
        "recall_at_20": float(on_topic_rank is not None),
    }


def _validate_cases(
    value: object,
    summary: dict[str, Any],
    expected_cases: list[dict[str, str]],
) -> None:
    _require(
        isinstance(value, dict) and set(value) == {"kind", "supplement_schema_version", "cases"},
        "case sidecar shape differs",
    )
    _require(
        value["kind"] == "quanta-index-relevance-openai-ab-cases"
        and type(value["supplement_schema_version"]) is int
        and value["supplement_schema_version"] == 1,
        "case sidecar identity differs",
    )
    cases = value["cases"]
    _require(isinstance(cases, list) and len(cases) == CASE_COUNT, "A/B case roster length differs")
    ids: set[str] = set()
    paraphrase_counts = {
        "paraphrase_case_count": 0,
        "paraphrase_hash_top1": 0,
        "paraphrase_openai_top1": 0,
        "paraphrase_openai_ranks_on_topic_higher": 0,
    }
    rows = summary["rows"]
    _require(len(rows) == len(cases), "artifact rows and cases differ")
    for index, (case, row, expected) in enumerate(zip(cases, rows, expected_cases, strict=True)):
        keys = {
            "id",
            "intent_kind",
            "intent",
            "query",
            "on_topic_path",
            "hard_negative_path",
            "hash",
            "openai",
            "delta",
        }
        _require(isinstance(case, dict) and set(case) == keys, f"case {index} shape differs")
        _require(
            all(case[field] == pinned_value for field, pinned_value in expected.items()),
            f"case {index} differs from the approved semantic judgment",
        )
        for key in ("id", "intent", "query", "on_topic_path", "hard_negative_path"):
            _require(
                isinstance(case[key], str) and bool(case[key]), f"case {index} {key} is missing"
            )
        _require(
            case["on_topic_path"] != case["hard_negative_path"],
            f"case {index} contains a duplicate judgment",
        )
        case_id = case["id"]
        _require(
            case_id not in ids and case_id.startswith("sem."),
            f"case {index} id is invalid or repeated",
        )
        ids.add(case_id)
        kind = case["intent_kind"]
        _require(kind in ("exact_token", "paraphrase"), f"case {index} kind is invalid")
        _require(
            case_id.endswith(".paraphrase" if kind == "paraphrase" else ".exact"),
            f"case {index} id/kind differ",
        )
        metrics_keys = {
            "produced_order",
            "mrr_at_10",
            "ndcg_at_10",
            "recall_at_20",
            "top1_is_on_topic",
            "hard_negative_rank",
            "on_topic_rank",
        }
        scores = {}
        for side in ("hash", "openai"):
            metrics = case[side]
            _require(
                isinstance(metrics, dict) and set(metrics) == metrics_keys,
                f"case {index} {side} metrics differ",
            )
            order = metrics["produced_order"]
            _require(
                isinstance(order, list)
                and len(order) <= RECALL_K
                and all(isinstance(path, str) and path for path in order)
                and len(set(order)) == len(order),
                f"case {index} {side} ordering differs",
            )
            scores[side] = _case_scores(order, case["on_topic_path"], case["hard_negative_path"])
            _validate_reported_values(metrics, scores[side], f"case {index} {side} score")
        _require(
            isinstance(row, dict) and row.get("scenario_id") == f"relevance-openai-ab.{case_id}",
            f"row {index} id differs",
        )
        _require(
            row.get("route_family") == "semantic" and row.get("engine_touched") == ["semantic"],
            f"row {index} route differs",
        )
        _require(
            row.get("result_count") == len(case["openai"]["produced_order"]),
            f"row {index} result count differs",
        )
        _require(
            row.get("result_shape")
            == ("empty" if not case["openai"]["produced_order"] else "candidates"),
            f"row {index} shape differs",
        )
        baseline, candidate = scores["hash"], scores["openai"]
        baseline_rank, candidate_rank = baseline["on_topic_rank"], candidate["on_topic_rank"]
        ranks_higher = candidate_rank is not None and (
            baseline_rank is None or candidate_rank < baseline_rank
        )
        expected_delta = {
            "mrr_at_10": candidate["mrr_at_10"] - baseline["mrr_at_10"],
            "ndcg_at_10": candidate["ndcg_at_10"] - baseline["ndcg_at_10"],
            "recall_at_20": candidate["recall_at_20"] - baseline["recall_at_20"],
            "on_topic_rank_hash": baseline_rank,
            "on_topic_rank_openai": candidate_rank,
            "openai_ranks_on_topic_higher": ranks_higher,
            "top1_win": candidate["top1_is_on_topic"] and not baseline["top1_is_on_topic"],
        }
        _require(
            isinstance(case["delta"], dict) and case["delta"].keys() == expected_delta.keys(),
            f"case {index} delta differs",
        )
        _validate_reported_values(case["delta"], expected_delta, f"case {index} delta")
        if kind == "paraphrase":
            paraphrase_counts["paraphrase_case_count"] += 1
            paraphrase_counts["paraphrase_hash_top1"] += baseline["top1_is_on_topic"]
            paraphrase_counts["paraphrase_openai_top1"] += candidate["top1_is_on_topic"]
            paraphrase_counts["paraphrase_openai_ranks_on_topic_higher"] += ranks_higher
    _require(
        paraphrase_counts["paraphrase_case_count"] == PARAPHRASE_COUNT,
        "paraphrase case roster differs",
    )
    detail = summary["detail"]
    _require(detail.get("status") == "captured", "capture status differs from detail")
    _validate_reported_values(detail, {"case_count": CASE_COUNT}, "A/B case total")
    _validate_reported_values(detail, paraphrase_counts, "A/B paraphrase total")
    _require(detail.get("headline_metric") == "on_topic_rank", "headline metric differs")


def _source_head(value: object, arm: str) -> tuple[dict[str, Any], list[int]]:
    _require(
        isinstance(value, dict) and set(value) == {"generation", "activation_token"},
        f"{arm} Source head shape differs",
    )
    generation = value["generation"]
    _require(
        isinstance(generation, dict)
        and set(generation) == {"lexical", "semantic", "semantic_content"},
        f"{arm} Source generation shape differs",
    )
    for field, track in (("lexical", "Lexical"), ("semantic", "Semantic")):
        snapshot = generation[field]
        _require(
            isinstance(snapshot, dict)
            and set(snapshot)
            == {"repo_id", "revision_id", "manifest_digest", "manifest_generation", "track"}
            and snapshot["track"] == track
            and type(snapshot["manifest_generation"]) is int
            and snapshot["manifest_generation"] == 1
            and all(
                isinstance(snapshot[key], str) and snapshot[key].strip()
                for key in ("repo_id", "revision_id", "manifest_digest")
            ),
            f"{arm} Source {field} snapshot differs",
        )
    lexical = generation["lexical"]
    _require(
        generation["semantic"] == {**lexical, "track": "Semantic"},
        f"{arm} Source semantic generation differs from lexical",
    )
    content = generation["semantic_content"]
    _require(
        isinstance(content, dict)
        and set(content) == {"row_root_digest", "membership_root_digest"}
        and all(is_digest(digest) for digest in content.values()),
        f"{arm} Source semantic content roots differ",
    )
    token = value["activation_token"]
    _require(
        isinstance(token, dict)
        and set(token) == {"root_incarnation", "activation_sequence"}
        and _unsigned(token["activation_sequence"], positive=True),
        f"{arm} Source activation token differs",
    )
    root = token["root_incarnation"]
    _require(
        isinstance(root, list)
        and len(root) == 16
        and all(type(byte) is int and 0 <= byte <= 255 for byte in root)
        and any(root),
        f"{arm} Source root incarnation differs",
    )
    return lexical, root


def _successful_http_requests(stats: Mapping[str, int], retry_cap: int) -> int:
    failures = stats["retryable_status_count"] + stats["transport_error_count"]
    retries = stats["retry_count"]
    _require(failures == retries, "completed phase contains an HTTP failure without a retry")
    successes = stats["http_request_count"] - retries
    _require(
        0 <= successes and retries <= successes * retry_cap,
        "completed phase exceeds the approved retry limit",
    )
    return successes


def _validate_query_observations(
    published: dict[str, Any],
    final: dict[str, Any],
    *,
    cache_enabled: bool,
    retry_cap: int,
) -> None:
    delta = {
        key: final[key] - published[key]
        for key in final
        if key not in {"schema_version", "request_samples"}
    }
    _require(
        all(change >= 0 for change in delta.values()),
        "provider telemetry decreased after publication",
    )
    expected_successes = CASE_COUNT
    if cache_enabled:
        expected_successes = delta["distinct_miss_texts"]
        _require(
            delta["total_texts_observed"] == delta["cache_hits"] + expected_successes == CASE_COUNT,
            "cache observations do not account for every query",
        )
    _require(
        _successful_http_requests(delta, retry_cap) == expected_successes,
        "HTTP observations do not account for every uncached query",
    )
    added = delta["http_request_count"]
    retained = published["request_samples"][delta["request_samples_dropped"] :]
    window = final["request_samples"]
    _require(
        window[: len(retained)] == retained, "query scrape changed retained publication samples"
    )
    query_samples = window[len(retained) :]
    _require(
        all(sample["texts_submitted"] == 1 for sample in query_samples),
        "query request samples must contain one text per HTTP attempt",
    )
    if added <= len(window):
        for sample_key, maximum_key in (
            ("texts_submitted", "max_request_texts"),
            ("estimated_tokens", "max_estimated_tokens"),
        ):
            query_maximum = max((sample[sample_key] for sample in query_samples), default=0)
            _require(
                final[maximum_key] == max(query_maximum, published[maximum_key]),
                f"query scrape changes {maximum_key} without a sample",
            )


def _validate_source_evidence(
    value: object,
    daemon_sha256: str,
    final_stats: dict[str, Any],
    runner_config: dict[str, Any],
) -> None:
    _require(
        isinstance(value, dict)
        and set(value)
        == {
            "legacy_daemon_sha256",
            "source_batch_digest",
            "hash",
            "openai",
            "openai_publication_http_requests",
            "openai_publication_stats",
        },
        "Source evidence shape differs",
    )
    _require(
        value["legacy_daemon_sha256"] == daemon_sha256, "Source evidence daemon SHA-256 differs"
    )
    _require(is_digest(value["source_batch_digest"]), "Source batch digest is invalid")
    hash_manifest, hash_root = _source_head(value["hash"], "Hash")
    openai_manifest, openai_root = _source_head(value["openai"], "OpenAI")
    _require(hash_manifest == openai_manifest, "Source manifest differs between arms")
    _require(hash_root != openai_root, "Source root incarnation is shared between arms")
    publication_requests = value["openai_publication_http_requests"]
    published = value["openai_publication_stats"]
    config = runner_config["openai"]
    _validate_stats(published, config)
    _require(
        _unsigned(publication_requests, positive=True)
        and publication_requests == published["http_request_count"]
        and publication_requests <= final_stats["http_request_count"],
        "OpenAI publication HTTP request count differs from provider stats",
    )
    budget = runner_config["provider_work_budget"]
    retry_cap = min(budget["retry_attempts_cap"], config["max_retries"])
    _require(
        _successful_http_requests(published, retry_cap) > 0,
        "Source publication did not observe a successful provider request",
    )
    charge = budget.get("openai_max_request_cost_micros")
    _require(
        _unsigned(charge, positive=True), "Source evidence lacks an approved OpenAI request cost"
    )
    _require(
        final_stats["http_request_count"] * charge <= budget["total_cost_micros_ceiling"],
        "provider attempts exceed the approved cost ceiling",
    )
    _validate_query_observations(
        published,
        final_stats,
        cache_enabled=config["cache_enabled"],
        retry_cap=retry_cap,
    )


def validate_capture(
    directory: Path,
    source_head: str,
    *,
    expected_cases: list[dict[str, str]],
    approved_env: Mapping[str, str],
    expected_legacy_daemon_sha256: str | None = None,
    allow_archived_input_digest: bool = False,
) -> dict[str, RawFile]:
    """Validate one complete original A/B output set without synthesizing metrics."""
    _require(
        directory.is_dir() and not directory.is_symlink(), "capture directory is missing or linked"
    )
    _require(
        {entry.name for entry in directory.iterdir()} == set(OUTPUT_NAMES),
        "capture output inventory differs",
    )
    files = {}
    parsed = {}
    for name in OUTPUT_NAMES:
        path = (directory / name).absolute()
        payload = read_control(path)
        files[name] = RawFile(path, f"sha256:{hashlib.sha256(payload).hexdigest()}", len(payload))
        parsed[name] = parse_json(payload.decode("utf-8"))
    summary, cases, stats = (parsed[name] for name in OUTPUT_NAMES)
    checker = _checker()
    refusals = checker.check_envelope(summary, dimension=DIMENSION, head=source_head)
    _require(not refusals, "; ".join(refusals))
    _require(summary["mode"] == "warm" and summary["concurrency"] == 1, "A/B run mode differs")
    detail = summary["detail"]
    _require(
        canonical_json(detail.get("case_results")) == canonical_json(cases),
        "summary cases differ from sidecar",
    )
    _require(
        canonical_json(detail.get("provider_stats")) == canonical_json(stats),
        "summary provider stats differ from sidecar",
    )
    runner_config = detail.get("runner_config")
    openai = _validate_runner_config(runner_config)
    _require(
        summary["provenance"]["model_revision"]
        == f"{openai['model']}@{openai['model_revision']}:d{openai['dimension']}",
        "model revision differs from runner_config",
    )
    _require(
        canonical_json(detail["provider_work_budget"])
        == canonical_json(runner_config["provider_work_budget"]),
        "top-level budget differs from runner_config",
    )
    config_digest = summary["provenance"]["config_digest"]
    _require(
        config_digest == _config_digest(runner_config), "config digest differs from runner_config"
    )
    _validate_approved_settings(runner_config, approved_env, _grant_digest(approved_env))
    _validate_stats(stats, openai)
    approved_input_digest = approved_env.get(APPROVED_AB_INPUT_DIGEST)
    # Archived in-process runners predate the external daemon and input binding.
    if (
        "source_evidence" in detail
        or expected_legacy_daemon_sha256 is not None
        or approved_input_digest is not None
    ):
        _require(
            is_digest(f"sha256:{expected_legacy_daemon_sha256}"),
            "Source evidence requires a pinned legacy daemon SHA-256",
        )
        _validate_source_evidence(
            detail.get("source_evidence"),
            expected_legacy_daemon_sha256,
            stats,
            runner_config,
        )
        source_digest = detail["source_evidence"]["source_batch_digest"]
        permitted_digests = {
            _approved_input_digest(config_digest, source_digest, _judgments_digest(expected_cases))
        }
        if allow_archived_input_digest:
            # Preserved captures checked the exact cases from the pinned checkout.
            permitted_digests.add(_approved_input_digest(config_digest, source_digest, None))
        _require(
            approved_input_digest in permitted_digests,
            "Source batch, judgments or runner config differs from the approved input digest",
        )
    _validate_cases(cases, summary, expected_cases)
    return files


def _resolve_source_checkout(source_checkout: Path, source_head: str) -> Path:
    _require(
        isinstance(source_head, str) and HEX_RE.fullmatch(source_head) is not None,
        "source HEAD is missing or invalid",
    )
    _require(
        source_checkout.is_absolute() and source_checkout.is_dir(),
        "source checkout must be an absolute directory",
    )
    return source_checkout.resolve(strict=True)


def validate_frozen_ab(
    *, directory: Path, source_checkout: Path, source_head: str, env: Mapping[str, str]
) -> dict[str, RawFile]:
    """Review preserved sidecars with their original checkout and grant."""
    checkout = _resolve_source_checkout(source_checkout, source_head)
    require_frozen_source(checkout, source_head)
    expected_cases, uses_tsv = _source_cases(checkout, source_head)
    daemon_sha256 = env.get(LEGACY_DAEMON_SHA256)
    if uses_tsv:
        _require(
            is_digest(f"sha256:{daemon_sha256}") and is_digest(env.get(APPROVED_AB_INPUT_DIGEST)),
            "current OpenAI A/B validation requires its daemon SHA-256 and input approval",
        )
    captured = validate_capture(
        directory,
        source_head,
        expected_cases=expected_cases,
        approved_env=env,
        expected_legacy_daemon_sha256=daemon_sha256,
        allow_archived_input_digest=True,
    )
    require_frozen_source(checkout, source_head)
    return captured


def _verify_runner_inputs(
    runner: Path,
    checkout: Path,
    env: dict[str, str],
    timeout: int,
    log_dir: Path,
    judgments_digest: str,
) -> None:
    preview_env = dict(env)
    preview_env.pop("OPENAI_API_KEY", None)
    try:
        result = execute(
            [str(runner), "--print-inputs"],
            cwd=checkout,
            env=preview_env,
            timeout=timeout,
            log_dir=log_dir,
        )
    except ProducerExecutionError as error:
        raise EvidenceError(
            "detached OpenAI A/B refused: credential-free runner preview failed"
        ) from error
    inputs = parse_json(result.stdout.read_control().decode("utf-8"))
    _require(
        isinstance(inputs, dict)
        and set(inputs)
        == {
            "runner_config",
            "config_digest",
            "source_batch_digest",
            "judgments_digest",
            "approval_digest",
        },
        "runner preview shape differs",
    )
    config = inputs["runner_config"]
    _validate_runner_config(config)
    _validate_approved_settings(config, env, _grant_digest(env))
    config_digest = _config_digest(config)
    source_digest = inputs["source_batch_digest"]
    _require(inputs["config_digest"] == config_digest, "runner preview config digest differs")
    _require(is_digest(source_digest), "runner preview Source batch digest is invalid")
    _require(
        inputs["judgments_digest"] == judgments_digest,
        "runner preview judgments differ from the approved checkout",
    )
    _require(
        _approved_input_digest(config_digest, source_digest, judgments_digest)
        == inputs["approval_digest"]
        == env[APPROVED_AB_INPUT_DIGEST],
        "runner preview differs from the approved input digest",
    )


def _publish_create_only(source: Path, destination: Path, parent: Path) -> None:
    """Publish a directory atomically, with no replacement even for an empty destination."""
    if sys.platform == "linux":
        symbol, flag = "renameat2", 1  # RENAME_NOREPLACE
    elif sys.platform == "darwin":
        symbol, flag = "renameatx_np", 0x4  # RENAME_EXCL
    else:
        raise EvidenceError("atomic create-only capture publication is unsupported")
    try:
        rename = getattr(ctypes.CDLL(None, use_errno=True), symbol)
    except AttributeError as error:
        raise EvidenceError("atomic create-only capture publication is unavailable") from error
    rename.argtypes = (ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint)
    rename.restype = ctypes.c_int
    directory = os.open(parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        if rename(
            directory,
            os.fsencode(source.relative_to(parent)),
            directory,
            os.fsencode(destination.relative_to(parent)),
            flag,
        ):
            code = ctypes.get_errno()
            _require(code not in {errno.EEXIST, errno.ENOTEMPTY}, "output appeared during capture")
            raise OSError(code, os.strerror(code), destination)
    finally:
        os.close(directory)


def capture_detached_ab(
    *,
    runner_binary: Path,
    runner_sha256: str,
    source_checkout: Path,
    source_head: str,
    out_dir: Path,
    env: Mapping[str, str],
    timeout: int = 3600,
) -> dict[str, RawFile]:
    """Validate a pinned runner's inputs before its paid external-daemon run.

    The supplied source/binary pair must have been reviewed separately. No
    approval is inferred here; no environment values enter argv or the return.
    """
    _require(
        runner_binary.is_absolute() and runner_binary.name == "relevance_openai_ab",
        "runner must be an absolute named relevance_openai_ab executable",
    )
    _require(
        isinstance(runner_sha256, str) and DIGEST_RE.fullmatch(runner_sha256) is not None,
        "runner SHA-256 is missing or invalid",
    )
    checkout = _resolve_source_checkout(source_checkout, source_head)
    _require(
        out_dir.is_absolute() and not out_dir.exists() and not out_dir.is_symlink(),
        "output must be a fresh absolute directory",
    )
    _require(type(timeout) is int and timeout > 0, "timeout must be positive")
    _require(
        all(isinstance(key, str) and isinstance(value, str) for key, value in env.items()),
        "environment must contain string pairs",
    )
    env = dict(env)
    final = out_dir.resolve()
    _require(
        not final.is_relative_to(checkout) and not checkout.is_relative_to(final),
        "output overlaps the producer checkout",
    )
    _require(
        not runner_binary.resolve(strict=True).is_relative_to(final),
        "output contains the runner input",
    )
    _require(
        final.parent.is_dir() and not final.parent.is_symlink(),
        "output parent is missing or linked",
    )
    require_frozen_source(checkout, source_head)
    original = RawFile.capture(runner_binary)
    _require(original.sha256 == runner_sha256, "runner SHA-256 differs")
    _require(
        stat.S_ISREG(runner_binary.stat().st_mode) and os.access(runner_binary, os.X_OK),
        "runner is not executable",
    )
    expected_cases, uses_tsv = _source_cases(checkout, source_head)
    _require(uses_tsv, "historical OpenAI A/B captures are validation-only")
    legacy_path = env.get(LEGACY_DAEMON_BIN)
    legacy_sha256 = env.get(LEGACY_DAEMON_SHA256)
    _require(
        legacy_path is not None and legacy_sha256 is not None,
        "external OpenAI A/B requires a pinned legacy daemon path and SHA-256",
    )
    path = Path(legacy_path)
    _require(
        path.is_absolute() and os.access(path, os.X_OK),
        "legacy daemon must be an absolute executable",
    )
    digest = f"sha256:{legacy_sha256}"
    _require(is_digest(digest), "legacy daemon SHA-256 is invalid")
    legacy_daemon = RawFile.capture(path)
    _require(legacy_daemon.sha256 == digest, "legacy daemon SHA-256 differs")
    charge = _configured_uint(OPENAI_REQUEST_COST, env.get(OPENAI_REQUEST_COST, "0"))
    ceiling = _configured_uint(OPENAI_COST_CEILING, env.get(OPENAI_COST_CEILING, "0"))
    _require(
        0 < charge <= ceiling,
        "external OpenAI A/B requires an explicit positive cost ceiling funding the approved request cost",
    )
    _require(
        is_digest(env.get(APPROVED_AB_INPUT_DIGEST)),
        f"external OpenAI A/B requires a valid {APPROVED_AB_INPUT_DIGEST}",
    )
    _grant_digest(env)

    with TemporaryDirectory(prefix=".relevance-openai-ab-", dir=final.parent) as scratch_name:
        scratch = Path(scratch_name)
        private_runner = scratch / "relevance_openai_ab"
        copied = original.copy_to(private_runner)
        _require(
            copied.sha256 == runner_sha256 and copied.size == original.size,
            "private runner copy differs",
        )
        private_runner.chmod(stat.S_IRUSR | stat.S_IXUSR)
        _require(
            RawFile.capture(private_runner).sha256 == runner_sha256, "private executable changed"
        )
        stage = scratch / "capture"
        empty = scratch / "empty"
        empty.mkdir()
        # Check the destination filesystem's exclusive rename support before the paid run.
        _publish_create_only(empty, scratch / "publish-probe", final.parent)
        preview_logs = scratch / "preview-logs"
        logs = scratch / "logs"
        preview_logs.mkdir()
        logs.mkdir()
        require_frozen_source(checkout, source_head)
        preview_start = time.monotonic()
        _verify_runner_inputs(
            private_runner, checkout, env, timeout, preview_logs, _judgments_digest(expected_cases)
        )
        require_frozen_source(checkout, source_head)
        _require(RawFile.capture(runner_binary) == original, "runner changed before execution")
        _require(
            RawFile.capture(private_runner) == copied, "private runner changed during its preview"
        )
        _require(
            RawFile.capture(legacy_daemon.path) == legacy_daemon,
            "legacy daemon changed before execution",
        )
        _require(not final.exists() and not final.is_symlink(), "output appeared before execution")
        remaining = math.ceil(timeout - (time.monotonic() - preview_start))
        _require(remaining > 0, "OpenAI A/B timed out before the paid run")
        execute(
            [str(private_runner), "--out-dir", str(stage)],
            cwd=checkout,
            env=env,
            timeout=remaining,
            log_dir=logs,
        )
        captured = validate_capture(
            stage,
            source_head,
            expected_cases=expected_cases,
            approved_env=env,
            expected_legacy_daemon_sha256=legacy_sha256,
        )
        require_frozen_source(checkout, source_head)
        _require(RawFile.capture(runner_binary) == original, "runner changed during execution")
        _require(
            RawFile.capture(private_runner).sha256 == runner_sha256,
            "private runner changed during execution",
        )
        _require(
            RawFile.capture(legacy_daemon.path) == legacy_daemon,
            "legacy daemon changed during execution",
        )
        _publish_create_only(stage, final, final.parent)
    return {name: RawFile(final / name, raw.sha256, raw.size) for name, raw in captured.items()}
