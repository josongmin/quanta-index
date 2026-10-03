"""Freeze and score external snippet diagnostics without promoting partial qrels.

The native runner receives only a schema-3 query pack. Upstream judgments stay
in a separate commitment-bound sidecar, and source bytes must match a clean
synthetic Git checkout. This is not an evaluator suite or a qualified corpus.
"""

from __future__ import annotations

import hashlib
import json
import math
import os
import re
import shutil
import subprocess
from collections import defaultdict
from pathlib import Path
from typing import Any

from tools.benchmark.retrieval import (
    clarc_adapter,
    codesearchnet_materialize,
    codesearchnet_qrels,
    evaluator,
    query_plan,
    retrieval_contract,
)

COMMIT_RE = re.compile(r"[0-9a-f]{40}\Z")
SCORED_STATUSES = frozenset(("success", "capped", "abstained"))
FAILED_STATUSES = frozenset(("error", "timeout", "unavailable"))


class ExternalSnippetError(ValueError):
    """External input, source identity or native capture differs from its freeze."""


def _sha(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def _json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ExternalSnippetError(f"invalid JSON: {path}") from exc


def _canonical(value: Any) -> bytes:
    return retrieval_contract.canonical(value)


def _source_checkout(repo: Path, commit: str, expected: dict[str, bytes]) -> list[dict[str, str]]:
    """Require the entire tracked synthetic corpus to equal independently derived bytes."""
    if not isinstance(commit, str) or not COMMIT_RE.fullmatch(commit):
        raise ExternalSnippetError("synthetic repository commit must be full lowercase Git SHA")
    try:
        source = evaluator.SourceSnapshot(repo, commit)
    except (ValueError, evaluator.EvidenceError) as exc:
        raise ExternalSnippetError(f"synthetic repository identity invalid: {exc}") from exc
    if source.tracked != set(expected):
        raise ExternalSnippetError("synthetic repository tracked paths differ from frozen corpus")
    universe = []
    for path, raw in sorted(expected.items()):
        try:
            observed, _, digest = source.file(path)
        except (OSError, ValueError, evaluator.EvidenceError) as exc:
            raise ExternalSnippetError(f"synthetic source cannot be read: {path}") from exc
        if observed != raw or digest != _sha(raw):
            raise ExternalSnippetError(f"synthetic source bytes differ: {path}")
        universe.append({"path": path, "file_sha256": digest})
    return universe


def _admission(tasks: list[dict[str, str]], config: dict[str, int] | None) -> dict[str, Any]:
    config = query_plan.validated_execution_config("natural_language_file", config)
    ledger = []
    for task in tasks:
        try:
            query_plan.plan_lexical_request("natural_language_file", task["query"], config)
        except query_plan.QueryPlanError as exc:
            ledger.append({"task_id": task["task_id"], "status": "refused", "reason": str(exc)})
        else:
            ledger.append({"task_id": task["task_id"], "status": "admitted"})
    return {
        "request_policy": "natural_language_file",
        "config": config,
        "requested": len(tasks),
        "admitted": sum(row["status"] == "admitted" for row in ledger),
        "refused": sum(row["status"] == "refused" for row in ledger),
        "ledger": ledger,
    }


def _freeze(
    *,
    kind: str,
    suite_id: str,
    repo: Path,
    commit: str,
    expected_files: dict[str, bytes],
    tasks: list[dict[str, str]],
    judgments: dict[str, list[dict[str, Any]]],
    source: dict[str, Any],
    config: dict[str, int] | None,
    top_k: int,
    extra: dict[str, Any],
    population_task_count: int | None = None,
) -> tuple[dict[str, Any], dict[str, Any]]:
    if not isinstance(suite_id, str) or not suite_id.strip():
        raise ExternalSnippetError("suite_id must be nonempty")
    if type(top_k) is not int or top_k != 10:
        raise ExternalSnippetError("external snippet diagnostic requires top_k=10")
    if not expected_files or not tasks or set(judgments) != {t["task_id"] for t in tasks}:
        raise ExternalSnippetError("external file or task population is incomplete")
    if population_task_count is None:
        population_task_count = len(tasks)
    if type(population_task_count) is not int or population_task_count < len(tasks):
        raise ExternalSnippetError("external population count is smaller than complete tasks")
    if len({t["task_id"] for t in tasks}) != len(tasks) or len(
        {t["query_sha256"] for t in tasks}
    ) != len(tasks):
        raise ExternalSnippetError("duplicate external task ID or query")
    for task in tasks:
        if (
            set(task) != {"task_id", "query", "query_sha256"}
            or _sha(task["query"].encode("utf-8")) != task["query_sha256"]
        ):
            raise ExternalSnippetError("external task query identity differs")
    universe = _source_checkout(repo, commit, expected_files)
    paths = set(expected_files)
    for task_id, rows in judgments.items():
        if not isinstance(rows, list) or not rows:
            raise ExternalSnippetError(f"external task has no judged snippets: {task_id}")
        if len({row["path"] for row in rows}) != len(rows):
            raise ExternalSnippetError(f"external task has duplicate judged snippets: {task_id}")
        for row in rows:
            if set(row) != {"path", "grade"} or row["path"] not in paths:
                raise ExternalSnippetError(f"external judgment has unknown path: {task_id}")
            grade = row["grade"]
            if type(grade) not in (int, float) or not math.isfinite(grade) or not 0 <= grade <= 3:
                raise ExternalSnippetError(f"external judgment has invalid grade: {task_id}")
    admission = _admission(tasks, config)
    selected_ids = {row["task_id"] for row in admission["ledger"] if row["status"] == "admitted"}
    selected = [task for task in tasks if task["task_id"] in selected_ids]
    if not selected:
        raise ExternalSnippetError("no query admitted under selected request profile")
    contract = {
        "top_k": top_k,
        "tokenizer": retrieval_contract.TOKENIZER,
        "tokenizer_budget_version": retrieval_contract.TOKENIZER_BUDGET_VERSION,
        "output_unit_policy": "rank_prefix",
        "span_unit": retrieval_contract.SPAN_UNIT,
    }
    retrieval_contract.validate_comparison_contract(contract, "external.comparison_contract")
    sidecar = {
        "kind": kind,
        "qualification": "diagnostic_unqualified",
        "metric_scope": "positive_target_hit_mrr" if kind.startswith("clarc") else "pool_estimated",
        "repository_commit": commit,
        "file_universe_digest": _sha(_canonical(universe)),
        "source": source,
        "admission": admission,
        "judgments": judgments,
        "population_task_count": population_task_count,
        "materialized_complete_tasks": len(tasks),
        "selected_task_ids": [task["task_id"] for task in selected],
        **extra,
    }
    pack = {
        "schema_version": 3,
        "suite_id": suite_id,
        "suite_commitment_sha256": _sha(_canonical(sidecar)),
        "repository_commit": commit,
        "tokenizer": retrieval_contract.TOKENIZER,
        "tokenizer_budget_version": retrieval_contract.TOKENIZER_BUDGET_VERSION,
        "routes": ["lexical"],
        "file_universe": universe,
        "file_universe_digest": sidecar["file_universe_digest"],
        "comparison_contract": contract,
        "tasks": selected,
    }
    return pack, sidecar


def freeze_clarc(
    dataset_root: Path,
    variant: str,
    repo: Path,
    commit: str,
    raw_inputs: dict[str, bytes],
    *,
    suite_id: str,
    config: dict[str, int] | None = None,
) -> tuple[dict[str, Any], dict[str, Any]]:
    """Bind pinned CLARC source, existing blindpack, and synthetic Git bytes."""
    if variant not in ("original", "neutral_renamed") or set(raw_inputs) != set(
        clarc_adapter.SOURCES
    ):
        raise ExternalSnippetError("CLARC variant or source input set differs")
    rows, metadata = clarc_adapter.admit_pinned_pair(
        raw_inputs["original"],
        raw_inputs["neutral_renamed"],
        raw_inputs["dataset_card"],
        raw_inputs["project_license_info"],
    )
    expected_files = {}
    tasks = []
    judgments = {}
    for pair in rows:
        source = pair["original"]
        query = source["query_text"]
        query_hash = _sha(query.encode("utf-8"))
        prefix = "CLARC-G1-ORG-" if variant == "original" else "CLARC-G1-NEU-"
        task_id = prefix + query_hash[:24]
        path = f"snippets/{source['code_id']}.cpp"
        expected_files[path] = pair[variant]["code_text"].encode("utf-8")
        tasks.append({"task_id": task_id, "query": query, "query_sha256": query_hash})
        judgments[task_id] = [{"path": path, "grade": 2}]
    blind = _json(dataset_root / f"blindpack-{variant}.json")
    expected_blind_files = [
        {"code_id": pair[variant]["code_id"], "path": path, "sha256": _sha(raw), "bytes": len(raw)}
        for (path, raw), pair in zip(expected_files.items(), rows)
    ]
    if (
        not isinstance(blind, dict)
        or blind.get("kind") != "clarc_group1_gold_blind_input_v1"
        or blind.get("variant") != variant
        or blind.get("file_universe") != expected_blind_files
        or blind.get("requested") != len(rows)
        or blind.get("admitted") != metadata["admission"]["admitted"]
        or blind.get("request_policy") != "natural_language_file"
        or blind.get("corpus_root") != str(dataset_root / variant)
    ):
        raise ExternalSnippetError(
            "CLARC blindpack differs from pinned source or default admission"
        )
    blind_tasks = blind.get("tasks")
    if not isinstance(blind_tasks, list) or len(blind_tasks) != len(tasks):
        raise ExternalSnippetError("CLARC blindpack task population differs")
    for expected, observed, pair in zip(tasks, blind_tasks, rows):
        status = (
            "admitted"
            if pair["original"]["query_id"] in metadata["admission"]["admitted_query_ids"]
            else "refused"
        )
        if observed != {**expected, "request_status": status}:
            raise ExternalSnippetError("CLARC blindpack query or default admission differs")
    return _freeze(
        kind="clarc_group1_positive_only_v1",
        suite_id=suite_id,
        repo=repo,
        commit=commit,
        expected_files=expected_files,
        tasks=tasks,
        judgments=judgments,
        source=metadata["source"],
        config=config,
        top_k=10,
        extra={
            "variant": variant,
            "unjudged_policy": "unknown",
            "synthetic_file_unit": "one_file_per_upstream_snippet",
            "default_32_admission": metadata["admission"],
            "duplicate_content": metadata["duplicate_content"][variant],
            "license_status": metadata["contract"]["license_status"],
        },
    )


def freeze_codesearchnet(
    csv_raw: bytes,
    materialized_root: Path,
    repo: Path,
    commit: str,
    *,
    language: str,
    suite_id: str,
    config: dict[str, int] | None = None,
) -> tuple[dict[str, Any], dict[str, Any]]:
    """Preserve upstream fractional grades and exclude incomplete query pools."""
    if language not in codesearchnet_materialize.EXTENSIONS:
        raise ExternalSnippetError("CodeSearchNet requires one supported language per native pack")
    seed = codesearchnet_qrels.diagnostic_seed(csv_raw)
    materialized = _json(materialized_root / "manifest.json")
    qrels = _json(materialized_root / "qrels.json")
    spans = _json(materialized_root / "spans.json")
    sources = _json(materialized_root / "source-fetches.json")
    if (
        materialized.get("kind") != "codesearchnet_snippet_materialization_diagnostic_v1"
        or materialized.get("upstream_csv") != seed["source"]
        or materialized.get("qrel_count") != len(seed["qrels"])
        or not isinstance(qrels, list)
        or len(qrels) != len(seed["qrels"])
        or not isinstance(spans, list)
        or not isinstance(sources, dict)
    ):
        raise ExternalSnippetError("CodeSearchNet materialization does not bind pinned CSV")
    by_url = {row["github_url"]: row for row in spans}
    if len(by_url) != len(spans):
        raise ExternalSnippetError("duplicate CodeSearchNet source span")
    expected_files: dict[str, bytes] = {}
    groups: dict[tuple[str, str], list[dict[str, Any]]] = defaultdict(list)
    for original, row in zip(seed["qrels"], qrels):
        if {key: row.get(key) for key in original} != original:
            raise ExternalSnippetError("CodeSearchNet materialized qrel differs from pinned CSV")
        span = by_url.get(row["github_url"])
        if span is None or row.get("materialization_status") != span.get("status"):
            raise ExternalSnippetError("CodeSearchNet qrel/span ledger differs")
        path = row.get("snippet_path")
        if span["status"] == "admitted":
            if path != span.get("relative_paths", {}).get(row["language"]):
                raise ExternalSnippetError("CodeSearchNet snippet path differs from span ledger")
            source = sources.get(span["source_url"])
            if not isinstance(source, dict) or source.get("status") != "fetched":
                raise ExternalSnippetError("CodeSearchNet admitted span lacks fetched source")
            raw = (materialized_root / source["relative_path"]).read_bytes()
            if _sha(raw) != source["sha256"] or _sha(raw) != span["source_sha256"]:
                raise ExternalSnippetError("CodeSearchNet fetched source digest differs")
            snippet = codesearchnet_materialize._extract_span(
                raw, span["start_line"], span["end_line"]
            )
            if _sha(snippet) != span["snippet_sha256"]:
                raise ExternalSnippetError("CodeSearchNet source span digest differs")
            if (materialized_root / path).read_bytes() != snippet:
                raise ExternalSnippetError("CodeSearchNet snippet bytes differ from pinned source")
            if row["language"] == language:
                previous = expected_files.setdefault(path, snippet)
                if previous != snippet:
                    raise ExternalSnippetError("CodeSearchNet snippet path has conflicting source")
        elif path is not None:
            raise ExternalSnippetError("unavailable CodeSearchNet span has snippet path")
        groups[(row["language"], row["query"])].append(row)
    if not expected_files or materialized.get("task_count") != len(groups):
        raise ExternalSnippetError("CodeSearchNet materialized task count differs")
    tasks = []
    judgments = {}
    incomplete = []
    full_population_ledger = []
    for (task_language, query), rows in sorted(groups.items()):
        if task_language != language:
            continue
        query_hash = _sha(query.encode("utf-8"))
        task_id = (
            "CSN-"
            + task_language.upper()
            + "-"
            + _sha((task_language + "\0" + query).encode("utf-8"))[:24]
        )
        if all(row["materialization_status"] == "admitted" for row in rows):
            tasks.append({"task_id": task_id, "query": query, "query_sha256": query_hash})
            judgments[task_id] = [
                {"path": row["snippet_path"], "grade": row["mean_grade"]} for row in rows
            ]
            try:
                query_plan.plan_lexical_request("natural_language_file", query, config)
            except query_plan.QueryPlanError as exc:
                full_population_ledger.append(
                    {"task_id": task_id, "status": "refused_query_plan", "reason": str(exc)}
                )
            else:
                full_population_ledger.append({"task_id": task_id, "status": "admitted"})
        else:
            incomplete.append(task_id)
            full_population_ledger.append(
                {
                    "task_id": task_id,
                    "status": "blocked_source_unavailable",
                    "source_statuses": sorted(
                        {
                            row["materialization_status"]
                            for row in rows
                            if row["materialization_status"] != "admitted"
                        }
                    ),
                    "unavailable_qrels": sum(
                        row["materialization_status"] != "admitted" for row in rows
                    ),
                }
            )
    if not tasks:
        raise ExternalSnippetError("CodeSearchNet has no fully materialized query pools")
    pack, sidecar = _freeze(
        kind="codesearchnet_fractional_pool_v1",
        suite_id=suite_id,
        repo=repo,
        commit=commit,
        expected_files=expected_files,
        tasks=tasks,
        judgments=judgments,
        source=seed["source"],
        config=config,
        top_k=10,
        extra={
            "unjudged_policy": "unknown",
            "synthetic_file_unit": "one_file_per_commit_pinned_source_span",
            "language": language,
            "all_query_language_pairs": len(groups),
            "language_population_tasks": sum(
                task_language == language for task_language, _ in groups
            ),
            "full_population_ledger": full_population_ledger,
            "incomplete_task_ids": incomplete,
            "qrels_total": sum(row["language"] == language for row in qrels),
            "qrels_materialized": sum(
                row["language"] == language and row["materialization_status"] == "admitted"
                for row in qrels
            ),
            "source_attestation": materialized["source_attestation"],
            "license_status": materialized["license_status"],
        },
        population_task_count=sum(task_language == language for task_language, _ in groups),
    )
    return pack, sidecar


def official_csn_ndcg(predicted_paths: list[str], judgments: list[dict[str, Any]]) -> float | None:
    """Mirror CodeSearchNet's full-IDCG, judged-only-rank reference diagnostic."""
    grades = {row["path"]: row["grade"] for row in judgments}
    ideal = sorted(grades.values(), reverse=True)
    idcg = sum((2**grade - 1) / math.log2(rank + 1) for rank, grade in enumerate(ideal, 1))
    if idcg == 0:
        return None
    rank = 0
    dcg = 0.0
    seen = set()
    for path in predicted_paths:
        if path in seen:
            raise ExternalSnippetError("duplicate path in CodeSearchNet prediction")
        seen.add(path)
        if path in grades:
            rank += 1
            dcg += (2 ** grades[path] - 1) / math.log2(rank + 1)
    return dcg / idcg


def score_capture(
    pack: dict[str, Any], sidecar: dict[str, Any], record: dict[str, Any]
) -> dict[str, Any]:
    """Score a bound native record; this is not a substitute for full record replay."""
    if (
        pack.get("schema_version") != 3
        or record.get("schema_version") != 5
        or pack.get("suite_commitment_sha256") != _sha(_canonical(sidecar))
        or pack.get("repository_commit") != sidecar.get("repository_commit")
        or pack.get("file_universe_digest") != sidecar.get("file_universe_digest")
        or pack.get("file_universe_digest") != _sha(_canonical(pack.get("file_universe")))
        or record.get("query_pack_sha256") != _sha(_canonical(pack))
        or record.get("comparison_contract") != pack.get("comparison_contract")
        or pack.get("routes") != ["lexical"]
        or pack.get("comparison_contract", {}).get("top_k") != 10
    ):
        raise ExternalSnippetError("native record, pack or gold commitment differs")
    tasks = {task["task_id"]: task for task in pack["tasks"]}
    if not tasks or set(sidecar.get("selected_task_ids", [])) != set(tasks):
        raise ExternalSnippetError("selected external task population differs")
    universe = {row["path"] for row in pack["file_universe"]}
    rows = record.get("results")
    if not isinstance(rows, list) or len(rows) != len(tasks):
        raise ExternalSnippetError("native record task coverage differs")
    seen = set()
    score_rows = []
    failed = []
    unjudged = 0
    returned = 0
    for row in rows:
        if (
            not isinstance(row, dict)
            or row.get("route") != "lexical"
            or row.get("rank_unit") != "distinct_file"
        ):
            raise ExternalSnippetError("native record route or ranking unit differs")
        task_id = row.get("task_id")
        if task_id not in tasks or task_id in seen:
            raise ExternalSnippetError("native record task ID missing or duplicate")
        seen.add(task_id)
        identity = row.get("query_identity")
        if (
            not isinstance(identity, dict)
            or identity.get("original_query_sha256") != tasks[task_id]["query_sha256"]
        ):
            raise ExternalSnippetError("native record original query identity differs")
        status = row.get("status")
        if status not in SCORED_STATUSES | FAILED_STATUSES:
            raise ExternalSnippetError("native record status unknown")
        candidates = row.get("candidates")
        if not isinstance(candidates, list) or len(candidates) > 10:
            raise ExternalSnippetError("native candidates missing or exceed top_k")
        paths = []
        for rank, candidate in enumerate(candidates, 1):
            if not isinstance(candidate, dict) or candidate.get("rank") != rank:
                raise ExternalSnippetError("native candidate rank differs")
            path = candidate.get("path")
            if path not in universe or path in paths:
                raise ExternalSnippetError("native candidate path unknown or duplicate")
            paths.append(path)
        if status in FAILED_STATUSES:
            if paths:
                raise ExternalSnippetError("failed native request has candidates")
            failed.append(task_id)
            continue
        if status == "abstained" and paths:
            raise ExternalSnippetError("abstained native request has candidates")
        if status in ("success", "capped") and not paths:
            raise ExternalSnippetError("successful native request has no candidates")
        judged = sidecar["judgments"][task_id]
        candidate_rows = [{"path": path} for path in paths]
        judged_paths = {item["path"] for item in judged}
        unjudged += sum(path not in judged_paths for path in paths)
        returned += len(paths)
        result = {
            "task_id": task_id,
            "status": status,
            "hit_at_10": evaluator.file_hit_at_k_judged(candidate_rows, judged, 10),
            "mrr_at_10": evaluator.file_mrr_at_k_judged(candidate_rows, judged, 10),
        }
        if sidecar["kind"] == "codesearchnet_fractional_pool_v1":
            result["pool_estimated_ndcg_at_10"] = (
                evaluator.file_ndcg_at_k(candidate_rows, judged, 10)
                if any(item["grade"] > 0 for item in judged)
                else None
            )
        score_rows.append(result)
    if seen != set(tasks):
        raise ExternalSnippetError("native record task coverage incomplete")
    report = {
        "qualification": "diagnostic_unqualified",
        "kind": sidecar["kind"],
        "record_validation_scope": "pack_commitment_result_status_rank_path_only",
        "population_tasks": sidecar["population_task_count"],
        "materialized_complete_tasks": sidecar["materialized_complete_tasks"],
        "profile_admitted": sidecar["admission"]["admitted"],
        "executed_scored": len(score_rows),
        "execution_failed": len(failed),
        "failed_task_ids": sorted(failed),
        "unjudged_returned": unjudged,
        "returned": returned,
        "judged_returned_fraction": (returned - unjudged) / returned if returned else None,
        "hit_at_10_count": sum(row["hit_at_10"] for row in score_rows),
        "hit_at_10": (
            sum(row["hit_at_10"] for row in score_rows) / len(score_rows) if score_rows else None
        ),
        "mrr_at_10": (
            sum(row["mrr_at_10"] for row in score_rows) / len(score_rows) if score_rows else None
        ),
        "per_query": score_rows,
    }
    if sidecar["kind"] == "codesearchnet_fractional_pool_v1":
        defined = [
            row["pool_estimated_ndcg_at_10"]
            for row in score_rows
            if row["pool_estimated_ndcg_at_10"] is not None
        ]
        report["pool_estimated_ndcg_at_10"] = sum(defined) / len(defined) if defined else None
        report["pool_estimated_ndcg_defined_tasks"] = len(defined)
        report["qrels_materialized"] = sidecar["qrels_materialized"]
        report["qrels_total"] = sidecar["qrels_total"]
        report["all_query_language_pairs"] = sidecar["all_query_language_pairs"]
    return report


def write_freeze(
    pack: dict[str, Any], sidecar: dict[str, Any], output_root: Path
) -> dict[str, str]:
    """Write a fresh external pack and separately held score sidecar."""
    if (
        not output_root.is_absolute()
        or not output_root.parent.is_dir()
        or output_root.exists()
        or output_root.is_symlink()
        or output_root.resolve(strict=False).is_relative_to(Path(__file__).resolve().parents[3])
    ):
        raise ExternalSnippetError("freeze output must be a new absolute root outside checkout")
    if pack.get("suite_commitment_sha256") != _sha(_canonical(sidecar)):
        raise ExternalSnippetError("pack does not bind external score sidecar")
    output_root.mkdir(mode=0o700)
    runner_root = output_root / "runner"
    owner_root = output_root / "scorer-input"
    runner_root.mkdir(mode=0o700)
    owner_root.mkdir(mode=0o700)
    pack_path = runner_root / "query-pack.json"
    gold_path = owner_root / "gold-sidecar.json"
    pack_raw = _canonical(pack) + b"\n"
    gold_raw = _canonical(sidecar) + b"\n"
    with os.fdopen(os.open(pack_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb") as handle:
        handle.write(pack_raw)
    with os.fdopen(os.open(gold_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb") as handle:
        handle.write(gold_raw)
    return {
        "query_pack": str(pack_path),
        "query_pack_bytes_sha256": _sha(pack_raw),
        "query_pack_canonical_sha256": _sha(_canonical(pack)),
        "gold_sidecar": str(gold_path),
        "gold_sidecar_bytes_sha256": _sha(gold_raw),
    }


def _commit_synthetic_repo(repo: Path, message: str) -> str:
    """Give a verified synthetic file universe a stable Git source identity."""
    commands = (
        ["git", "init", "-q", "-b", "main", str(repo)],
        ["git", "-C", str(repo), "-c", "core.autocrlf=false", "add", "-A"],
        [
            "git",
            "-C",
            str(repo),
            "-c",
            "user.name=External Benchmark Fixture",
            "-c",
            "user.email=external-benchmark@example.invalid",
            "commit",
            "-qm",
            message,
        ],
    )
    environment = dict(os.environ)
    environment["GIT_AUTHOR_DATE"] = "2000-01-01T00:00:00+0000"
    environment["GIT_COMMITTER_DATE"] = environment["GIT_AUTHOR_DATE"]
    for command in commands:
        try:
            subprocess.run(command, check=True, capture_output=True, env=environment)
        except (OSError, subprocess.CalledProcessError) as exc:
            raise ExternalSnippetError("synthetic Git corpus commit failed") from exc
    try:
        return subprocess.check_output(
            ["git", "-C", str(repo), "rev-parse", "HEAD"], text=True
        ).strip()
    except (OSError, subprocess.CalledProcessError) as exc:
        raise ExternalSnippetError("synthetic Git corpus commit unavailable") from exc


def prepare_external_lanes(
    clarc_raw_inputs: dict[str, bytes],
    codesearchnet_csv_raw: bytes,
    codesearchnet_materialized_root: Path,
    output_root: Path,
    *,
    languages: tuple[str, ...] = tuple(sorted(codesearchnet_materialize.EXTENSIONS)),
) -> dict[str, Any]:
    """Prepare self-contained static inputs for paired CLARC and six CSN lanes.

    This only separates native pack and owner gold paths. It provides no
    runtime access-block or search-result proof; the caller must capture both.
    """
    if (
        not output_root.is_absolute()
        or not output_root.parent.is_dir()
        or output_root.exists()
        or output_root.is_symlink()
        or output_root.resolve(strict=False).is_relative_to(Path(__file__).resolve().parents[3])
        or not languages
        or len(set(languages)) != len(languages)
        or set(languages) - set(codesearchnet_materialize.EXTENSIONS)
    ):
        raise ExternalSnippetError(
            "preparation requires a new external root and distinct languages"
        )
    if set(clarc_raw_inputs) != set(clarc_adapter.SOURCES):
        raise ExternalSnippetError("CLARC pinned source input set differs")
    # Admit all external bytes before creating a potentially expensive output.
    clarc_adapter.admit_pinned_pair(
        clarc_raw_inputs["original"],
        clarc_raw_inputs["neutral_renamed"],
        clarc_raw_inputs["dataset_card"],
        clarc_raw_inputs["project_license_info"],
    )
    codesearchnet_qrels.diagnostic_seed(codesearchnet_csv_raw)
    materialized = _json(codesearchnet_materialized_root / "manifest.json")
    if materialized.get("kind") != "codesearchnet_snippet_materialization_diagnostic_v1":
        raise ExternalSnippetError("CodeSearchNet materialization kind differs")
    output_root.mkdir(mode=0o700)
    upstream_root = output_root / "upstream"
    corpora_root = output_root / "corpora"
    freezes_root = output_root / "freezes"
    for path in (upstream_root, corpora_root, freezes_root):
        path.mkdir(mode=0o700)
    clarc_inputs_root = upstream_root / "clarc-pinned"
    clarc_inputs_root.mkdir(mode=0o700)
    input_bindings = {}
    for name, raw in sorted(clarc_raw_inputs.items()):
        destination = clarc_inputs_root / name
        destination.write_bytes(raw)
        input_bindings[name] = {"path": str(destination), "sha256": _sha(raw), "bytes": len(raw)}
    csv_path = upstream_root / "codesearchnet-annotationStore.csv"
    csv_path.write_bytes(codesearchnet_csv_raw)
    input_bindings["codesearchnet_csv"] = {
        "path": str(csv_path),
        "sha256": _sha(codesearchnet_csv_raw),
        "bytes": len(codesearchnet_csv_raw),
    }
    csn_copy = upstream_root / "codesearchnet-materialized"
    shutil.copytree(codesearchnet_materialized_root, csn_copy, symlinks=False)
    for filename in ("manifest.json", "source-fetches.json", "spans.json", "qrels.json"):
        raw = (csn_copy / filename).read_bytes()
        input_bindings["codesearchnet_materialized_" + filename] = {
            "path": str(csn_copy / filename),
            "sha256": _sha(raw),
            "bytes": len(raw),
        }
    clarc_data_root = corpora_root / "clarc"
    clarc_adapter.materialize(
        clarc_raw_inputs["original"],
        clarc_raw_inputs["neutral_renamed"],
        clarc_raw_inputs["dataset_card"],
        clarc_raw_inputs["project_license_info"],
        clarc_data_root,
    )
    config = {"max_tokens": 128, "max_token_chars": 96, "min_token_chars": 1}
    profile = query_plan.execution_profile("natural_language_file", config)
    lanes = {}
    for variant in ("original", "neutral_renamed"):
        name = "clarc-" + variant
        repo = clarc_data_root / variant
        commit = _commit_synthetic_repo(repo, f"Freeze {name} synthetic snippets")
        pack, sidecar = freeze_clarc(
            clarc_data_root,
            variant,
            repo,
            commit,
            clarc_raw_inputs,
            suite_id=name + "-nl128-diagnostic",
            config=config,
        )
        written = write_freeze(pack, sidecar, freezes_root / name)
        lanes[name] = {
            "corpus_repo": str(repo),
            "repository_commit": commit,
            "population_tasks": sidecar["population_task_count"],
            "complete_tasks": sidecar["materialized_complete_tasks"],
            "submitted_tasks": len(pack["tasks"]),
            "source_blocked_tasks": 0,
            "profile_refused_tasks": sidecar["admission"]["refused"],
            "file_count": len(pack["file_universe"]),
            **written,
        }
    csn_qrels = _json(csn_copy / "qrels.json")
    for language in languages:
        name = "csn-" + language
        repo = corpora_root / name
        repo.mkdir(mode=0o700)
        paths = sorted(
            {
                row["snippet_path"]
                for row in csn_qrels
                if row["language"] == language and row.get("snippet_path") is not None
            }
        )
        if not paths:
            raise ExternalSnippetError(f"CodeSearchNet {language} has no materialized snippets")
        for path in paths:
            source = csn_copy / path
            if source.is_symlink() or not source.resolve().is_relative_to(csn_copy.resolve()):
                raise ExternalSnippetError("CodeSearchNet snippet source escapes materialization")
            destination = repo / path
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination)
        commit = _commit_synthetic_repo(repo, f"Freeze {name} synthetic snippets")
        pack, sidecar = freeze_codesearchnet(
            codesearchnet_csv_raw,
            csn_copy,
            repo,
            commit,
            language=language,
            suite_id=name + "-nl128-pool-diagnostic",
            config=config,
        )
        written = write_freeze(pack, sidecar, freezes_root / name)
        lanes[name] = {
            "corpus_repo": str(repo),
            "repository_commit": commit,
            "population_tasks": sidecar["population_task_count"],
            "complete_tasks": sidecar["materialized_complete_tasks"],
            "submitted_tasks": len(pack["tasks"]),
            "source_blocked_tasks": len(sidecar["incomplete_task_ids"]),
            "profile_refused_tasks": sidecar["admission"]["refused"],
            "file_count": len(pack["file_universe"]),
            "qrels_total": sidecar["qrels_total"],
            "qrels_materialized": sidecar["qrels_materialized"],
            **written,
        }
    summary = {
        "kind": "external_snippet_static_preparation_v1",
        "qualification": "diagnostic_unqualified",
        "runtime_gold_isolation": "not_verified_static_path_separation_only",
        "runtime_search_execution": "not_run",
        "execution_profile": profile,
        "execution_profile_sha256": query_plan.execution_profile_sha256(
            "natural_language_file", config
        ),
        "inputs": input_bindings,
        "lanes": lanes,
        "codesearchnet_total_query_language_pairs": materialized["task_count"],
        "codesearchnet_selected_languages": list(languages),
        "codesearchnet_selected_population": sum(
            lanes["csn-" + language]["population_tasks"] for language in languages
        ),
        "codesearchnet_selected_complete": sum(
            lanes["csn-" + language]["complete_tasks"] for language in languages
        ),
    }
    (output_root / "manifest.json").write_bytes(_canonical(summary) + b"\n")
    return summary
