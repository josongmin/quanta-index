#!/usr/bin/env python3
"""Join fresh native and external OSA1 captures on identical source-bound inputs.

Each pair consists of a native prepared root, a fresh external root, and a
source-admission authority. Only admitted, fully replayed cells enter
the score denominator. This is a diagnostic report, never a qualified speed or
product-ranking claim.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from collections import defaultdict
from contextlib import contextmanager
from contextvars import ContextVar
from pathlib import Path
from typing import Any

try:
    from tools.benchmark.retrieval import evaluator, identifier_robustness_suite, run
    from tools.benchmark.retrieval import identifier_robustness_multiproduct_report as scoring
except ModuleNotFoundError:  # direct script invocation
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    from tools.benchmark.retrieval import evaluator, identifier_robustness_suite, run
    from tools.benchmark.retrieval import identifier_robustness_multiproduct_report as scoring

PRODUCTS = ("quanta", "semble", "sourcegraph", "cs", "opengrok")
EXTERNAL_PRODUCTS = ("sourcegraph", "cs", "opengrok")
INTENT = "declaration_name_osa1_casefold"
PAIR_REPORT = "report-semble-lexical-file-vs-lexical-fixed_window_strict.json"
ALLOWED_EXTERNAL_LEDGER_STATUSES = {
    "verified",
    "verified_eligible_cells_source_blocked_4",
}
PAIR_CUSTODY_FIELDS = (
    "cohort_contract",
    "admission_authority_sha256",
    "release_digest",
    "native_source_commit",
    "native_runner_sha256",
    "native_searchd_sha256",
    "native_merge_validator_sha256",
    "external_source_commit",
)
EXTERNAL_PRODUCER_FILES = {
    "producer": "tools/benchmark/retrieval/live_lexical_external.py",
    "sourcegraph_adapter": "tools/benchmark/retrieval/sourcegraph.py",
    "lexical_scorer": "tools/benchmark/retrieval/lexical_file_comparison.py",
    "corpus_binding": "tools/benchmark/corpus_binding.py",
    "corpus_release": "tools/benchmark/corpus_release.py",
    "query_planner": "tools/benchmark/retrieval/query_plan.py",
}


class FreshJoinError(ValueError):
    """Capture authority or paired input binding differs."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise FreshJoinError(message)


_BOUND_EVIDENCE: ContextVar[dict[str, str] | None] = ContextVar(
    "fresh_join_bound_evidence", default=None
)


def _bound_bytes(path: Path) -> bytes:
    raw = path.read_bytes()
    bound = _BOUND_EVIDENCE.get()
    if bound is not None:
        key = str(path.absolute())
        digest = hashlib.sha256(raw).hexdigest()
        require(
            key not in bound or bound[key] == digest,
            "evidence bytes changed during fresh join: " + key,
        )
        bound[key] = digest
    return raw


@contextmanager
def _evidence_session():
    """Keep all scored input bytes stable through the final report computation."""
    bound: dict[str, str] = {}
    token = _BOUND_EVIDENCE.set(bound)
    try:
        yield
        for path, digest in bound.items():
            require(
                hashlib.sha256(Path(path).read_bytes()).hexdigest() == digest,
                "evidence bytes changed before fresh join completion: " + path,
            )
    finally:
        _BOUND_EVIDENCE.reset(token)


def read(path: Path) -> Any:
    return json.loads(_bound_bytes(path))


def sha(path: Path) -> str:
    return hashlib.sha256(_bound_bytes(path)).hexdigest()


def canonical_sha(value: Any) -> str:
    return hashlib.sha256(evaluator.canonical(value)).hexdigest()


def _bound_jsonl(path: Path) -> list[dict[str, Any]]:
    rows = [json.loads(line) for line in _bound_bytes(path).splitlines() if line.strip()]
    require(all(isinstance(row, dict) for row in rows), "malformed external JSONL: " + str(path))
    return rows


def _external_producer_sources(source_checkout: Path, source_head: str) -> dict[str, str]:
    """Bind live producer bytes to tracked blobs at the claimed frozen HEAD."""
    head = subprocess.run(
        ["git", "-C", str(source_checkout), "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    require(head == source_head, "external producer checkout HEAD changed")
    hashes = {}
    for name, relative in EXTERNAL_PRODUCER_FILES.items():
        blob = subprocess.run(
            ["git", "-C", str(source_checkout), "show", "HEAD:" + relative],
            check=True,
            capture_output=True,
        ).stdout
        current = _bound_bytes(source_checkout / relative)
        require(current == blob, "external producer source differs from frozen HEAD: " + relative)
        hashes[name] = hashlib.sha256(current).hexdigest()
    return hashes


def _require_default_profiles(
    native_spec: dict[str, Any], external_spec: dict[str, Any], repo: str
) -> None:
    require(
        native_spec["execution_profiles"]["quanta"]
        == {
            "config": {},
            "planning_cost_in_latency": False,
            "policy": "code_search_file",
            "profile_id": "quanta-code-search-file-v1",
        }
        and native_spec["execution_profiles"]["semble"]
        == {
            "alpha": None,
            "mode": "lexical-file",
            "profile_id": "semble-lexical-file-v1",
            "rerank": "not_applicable",
        }
        and native_spec["candidate_route"] == "lexical"
        and native_spec["baseline_route"] == "semble-lexical-file"
        and native_spec["top_k"] == 10
        and set(external_spec)
        == {
            "corpus",
            "cs",
            "opengrok",
            "output_root",
            "query_pack",
            "schema_version",
            "sourcegraph",
            "suite",
        }
        and external_spec["sourcegraph"]["repository"] == "benchmark/" + repo
        and external_spec["opengrok"]["project"] == repo
        and external_spec["opengrok"]["indexed_view_probe"] == "full"
        and set(external_spec["cs"]) == {"binary"},
        "paired request profile is not default file search: " + repo,
    )


def _reconcile_blocked(
    blocked: list[dict[str, Any]],
    bindings: list[dict[str, Any]],
    custody: list[dict[str, Any]],
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    by_repo = {row["repository"]: row for row in bindings}
    for origin in custody:
        for repo, prior in origin["source_blocked_originals"].items():
            if repo not in by_repo:
                continue
            repaired = by_repo[repo]
            require(
                repaired["blind_pack_sha256"] == prior["blind_pack_sha256"]
                and repaired["task_identity_sha256"] == prior["task_identity_sha256"]
                and repaired["file_universe_sha256"] == prior["file_universe_sha256"],
                "repaired cell changes the fixed original query cohort or file universe: " + repo,
            )
    resolved = set(by_repo)
    return (
        [row for row in blocked if row["repository"] not in resolved],
        [row for row in blocked if row["repository"] in resolved],
    )


def _require_pair_custody_consistency(custody: list[dict[str, Any]]) -> None:
    """Different producer or evaluator contracts cannot share a score denominator."""
    require(bool(custody), "fresh pair custody is empty")
    first = tuple(custody[0].get(field) for field in PAIR_CUSTODY_FIELDS)
    require(all(isinstance(value, str) and value for value in first), "pair custody incomplete")
    for row in custody[1:]:
        require(
            tuple(row.get(field) for field in PAIR_CUSTODY_FIELDS) == first,
            "fresh pairs use different source, binary, evaluator, or corpus contracts",
        )


def _source_admission(
    authority_path: Path, prepared: dict[str, Any], manifest: dict[str, Any]
) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    """Read the original audit or the pinned global matrix into one admission shape."""
    authority = read(authority_path)
    if isinstance(authority, list):
        return authority, {
            "cohort_contract": "c5_fixed_original_source_eligibility_v1",
            "admission_authority_sha256": sha(authority_path),
            "source_eligibility_kind": "independent_original_source_audit",
        }
    require(
        isinstance(authority, dict)
        and manifest.get("schema") == "c5_osa1_external_global12_fresh_v1"
        and authority.get("schema_version") == 1
        and authority.get("status") == "diagnostic_unqualified"
        and authority.get("repository_count") == 12
        and authority.get("intent_count") == 6
        and authority.get("release_digest") == manifest.get("release_digest")
        and manifest.get("matrix_path") == str(authority_path)
        and manifest.get("matrix_sha256") == sha(authority_path),
        "global source admission matrix or capture binding differs",
    )
    matrix_receipt_path = Path(manifest["matrix_receipt_path"])
    projection_receipt_path = Path(manifest["ordinary_projection_receipt_path"])
    gold_receipt_path = Path(manifest["gold_receipt_path"])
    runtime_binding_path = Path(manifest["runtime_four_pin_binding_path"])
    matrix_receipt = read(matrix_receipt_path)
    projection = read(projection_receipt_path)
    gold = read(gold_receipt_path)
    runtime_binding = read(runtime_binding_path)
    require(
        matrix_receipt.get("status") == "factory_derived_diagnostic_unqualified"
        and matrix_receipt.get("matrix_sha256") == sha(authority_path)
        and matrix_receipt.get("gold_receipt_sha256") == sha(gold_receipt_path)
        and projection.get("status") == "global_ordinary_diagnostic_unqualified"
        and projection.get("source_matrix_sha256") == sha(authority_path)
        and projection.get("repository_count") == 12
        and manifest.get("ordinary_projection_receipt_sha256") == sha(projection_receipt_path)
        and gold.get("status") == "captured"
        and gold.get("capsule_count") == 12
        and manifest.get("gold_producer_runtime", {}).get("receipt_sha256")
        == sha(gold_receipt_path)
        and runtime_binding_path.parent == gold_receipt_path.parent
        and manifest.get("runtime_four_pin_binding_sha256") == sha(runtime_binding_path)
        and prepared.get("runtime_four_pin_binding_path") == str(runtime_binding_path)
        and prepared.get("runtime_four_pin_binding_sha256") == sha(runtime_binding_path)
        and runtime_binding.get("status") == "supplementary_runtime_binding"
        and runtime_binding.get("gold_receipt_sha256") == sha(gold_receipt_path)
        and runtime_binding.get("source_head") == gold.get("source_head")
        and runtime_binding.get("capsule_identity_sha256") == gold.get("identity_sha256")
        and runtime_binding.get("active_distribution_versions")
        == runtime_binding.get("source_lock_pins")
        == prepared.get("parser_runtime_versions")
        and runtime_binding.get("pin_files_sha256")
        == {
            "pyproject.toml": gold.get("pyproject_sha256"),
            "uv.lock": gold.get("uv_lock_sha256"),
        }
        and manifest.get("gold_producer_runtime", {}).get("source_head")
        == gold.get("source_head")
        and manifest.get("gold_producer_runtime", {}).get("dependency_versions")
        == gold.get("dependency_versions")
        and manifest.get("gold_producer_runtime", {}).get("parser_runtime_versions")
        == prepared.get("parser_runtime_versions")
        == {
            "tree-sitter": "0.23.2",
            "tree-sitter-language-pack": "0.9.1",
            "regex": "2025.10.23",
            "unicodedata2": "17.0.0",
        }
        and manifest.get("external_collector_runtime", {}).get("parser_runtime_versions")
        == prepared.get("parser_runtime_versions")
        and manifest.get("external_collector_runtime", {}).get("python_executable_sha256")
        == prepared.get("driver_python_sha256")
        and manifest.get("gold_producer_runtime", {}).get("python_executable")
        == gold.get("python_executable")
        == prepared.get("driver_python")
        and manifest.get("gold_producer_runtime", {}).get("python_executable_sha256")
        == prepared.get("driver_python_sha256")
        and manifest.get("gold_producer_runtime", {}).get("pyproject_sha256")
        == gold.get("pyproject_sha256")
        and manifest.get("gold_producer_runtime", {}).get("uv_lock_sha256")
        == gold.get("uv_lock_sha256")
        and manifest.get("suite_projector_source", {}).get("base_head")
        == projection.get("source_base_head")
        and manifest.get("suite_projector_source", {}).get("commit")
        == projection.get("projector_source_commit")
        and manifest.get("suite_projector_source", {}).get("overlay_sha256")
        == projection.get("projector_overlay_sha256")
        and prepared.get("source_commit") == gold.get("source_head")
        and prepared.get("driver_source_sha") == gold.get("source_head")
        and prepared.get("parser_dependency_versions") == gold.get("dependency_versions")
        and prepared.get("pyproject_sha256") == gold.get("pyproject_sha256")
        and prepared.get("uv_lock_sha256") == gold.get("uv_lock_sha256")
        and isinstance(prepared.get("binary_build_source_sha"), str)
        and bool(prepared["binary_build_source_sha"])
        and isinstance(prepared.get("driver_python_sha256"), str)
        and bool(prepared["driver_python_sha256"])
        and prepared.get("gold_receipt_sha256") == sha(gold_receipt_path)
        and prepared.get("projection_receipt_sha256") == sha(projection_receipt_path)
        and prepared.get("projector_overlay_sha256")
        == projection.get("projector_overlay_sha256")
        and prepared.get("projector_source_commit")
        == projection.get("projector_source_commit"),
        "global gold, matrix, projection, or native preparation differs",
    )
    selected = [row for row in authority["cells"] if row["intent"] == INTENT]
    by_repo = {row["repository"]: row for row in selected}
    cohorts = projection["cohorts"]
    capsule_root = Path(manifest["gold_capsule_root"])
    require(
        capsule_root.is_absolute()
        and capsule_root.is_dir()
        and ".." not in capsule_root.parts,
        "global gold capsule root is absent or noncanonical",
    )
    require(
        len(selected) == len(by_repo) == 12
        and set(by_repo) == set(cohorts) == {row["repository"] for row in manifest["cells"]}
        and sum(row["selected"] for row in cohorts.values()) == projection["selected_total"]
        and projection["selected_total"] == manifest["total_tasks"],
        "global source admission repository or task denominator differs",
    )
    admission = []
    for cell in manifest["cells"]:
        repo = cell["repository"]
        row, cohort = by_repo[repo], cohorts[repo]
        identity_path = Path(cell["gold_identity_path"])
        require(
            identity_path == capsule_root / repo / "identity.json",
            "global gold identity path differs from bound capsule root: " + repo,
        )
        identity = read(identity_path)
        suite_path = Path(cell["spec_path"])
        spec = read(suite_path)
        suite = read(Path(spec["suite"]))
        pack = read(Path(spec["query_pack"]))
        selected_ids = [task["task_id"] for task in suite["tasks"]]
        require(
            row["status"] == "diagnostic_unqualified"
            and row["selected_task_ids"] == cohort["selected_task_ids"] == selected_ids
            and row["repository_commit"] == suite["repository_commit"]
            and row["gold_capsule_identity_sha256"]
            == manifest["gold_producer_sources"][repo]["identity_sha256"]
            == gold["identity_sha256"][repo]
            == sha(identity_path)
            and identity["parser_runtime"]
            == manifest["gold_producer_sources"][repo]["parser_runtime"]
            and manifest["gold_producer_sources"][repo]["parser_runtime"]["tree_sitter"]
            == prepared["parser_runtime_versions"]["tree-sitter"]
            and manifest["gold_producer_sources"][repo]["parser_runtime"][
                "tree_sitter_language_pack"
            ]
            == prepared["parser_runtime_versions"]["tree-sitter-language-pack"]
            and row["release_digest"] == authority["release_digest"]
            and cell["tasks"] == cohort["selected"] == len(selected_ids)
            and cell["projected_suite_commitment_sha256"] == cohort["projected_suite_sha256"]
            and cell["blind_pack_commitment_sha256"] == cohort["blind_pack_sha256"]
            and canonical_sha(suite) == cohort["projected_suite_sha256"]
            and canonical_sha(pack) == cohort["blind_pack_sha256"],
            "global source admission task, gold, or projected suite differs: " + repo,
        )
        admission.append({"repository": repo, "status": "VALID", "tasks": len(selected_ids)})
    return admission, {
        "cohort_contract": "c5_global_c4_ordinary_osa1_v1",
        "admission_authority_sha256": canonical_sha(
            {
                "gold_receipt": sha(gold_receipt_path),
                "matrix": sha(authority_path),
                "matrix_receipt": sha(matrix_receipt_path),
                "projection_receipt": sha(projection_receipt_path),
                "runtime_four_pin_binding": sha(runtime_binding_path),
            }
        ),
        "source_eligibility_kind": "pinned_global_matrix_and_ordinary_projection",
        "gold_receipt_sha256": sha(gold_receipt_path),
        "matrix_receipt_sha256": sha(matrix_receipt_path),
        "projection_receipt_sha256": sha(projection_receipt_path),
        "native_binary_build_source_commit": prepared["binary_build_source_sha"],
        "native_driver_python_sha256": prepared["driver_python_sha256"],
    }


def _native_records(
    cell: dict[str, Any],
    prepared: dict[str, Any],
    spec: dict[str, Any],
    suite: dict[str, Any],
    tasks: dict[str, Any],
    universe: set[str],
) -> tuple[dict[str, dict[str, dict[str, Any]]], dict[str, str], str, dict[str, Any]]:
    repo, output = cell["repository"], Path(cell["output_root"])
    status_binding = _select_native_status(cell, prepared)
    verdict = read(output / "verdict.json")
    require(
        verdict["counts"]["selected"] == verdict["counts"]["executed"] == 2 * len(tasks)
        and verdict["counts"]["failed"] == 0,
        "native execution failed or incomplete: " + repo,
    )
    records: dict[str, dict[str, dict[str, Any]]] = {}
    hashes: dict[str, str] = {}
    record_paths: list[Path] = []
    for product, route in (("quanta", "lexical"), ("semble", "semble-lexical-file")):
        if product == "quanta":
            matches = sorted((output / "rep-00/quanta").glob("strategy-*/record.json"))
            require(len(matches) == 1, "native strategy count differs: " + repo)
            record_path = matches[0]
            pack_path = output / "rep-00/quanta/quanta-pack.json"
        else:
            record_path = output / "rep-00/semble/record.json"
            pack_path = output / "semble-pack.json"
        record = read(record_path)
        record_paths.append(record_path)
        hashes[product] = sha(record_path)
        require(
            record["query_pack_sha256"] == canonical_sha(read(pack_path))
            and set(record["route_provenance"]) == {route}
            and isinstance(record["route_provenance"][route]["capture_id"], str)
            and bool(record["route_provenance"][route]["capture_id"]),
            "native pack or route provenance differs: " + repo + "/" + product,
        )
        results = scoring._unique(record["results"], "task_id", repo + "/" + product)
        require(set(results) == set(tasks), "native task coverage differs: " + repo + "/" + product)
        product_rows: dict[str, dict[str, Any]] = {}
        for task_id, raw in results.items():
            task = tasks[task_id]
            require(
                raw["route"] == route
                and raw["rank_unit"] == "distinct_file"
                and raw["query_identity"]["original_query_sha256"] == task["query_sha256"]
                and len(raw["candidates"]) <= 10,
                "native route/unit/query differs: " + repo + "/" + product + "/" + task_id,
            )
            paths = scoring._top10(
                [candidate["path"] for candidate in raw["candidates"][:10]],
                universe,
                repo + "/" + product + "/" + task_id,
            )
            result_status = raw["status"]
            require(result_status in evaluator.RESULT_STATUSES, "unknown native result status")
            product_rows[task_id] = scoring._result(
                task,
                paths,
                eligible=result_status != "error",
                status=result_status,
                latency_ms=raw["timings"].get("query_latency_ms"),
            )
        records[product] = product_rows
    checked_suite, checked_pack, merged = run.merge_records(
        Path(spec["repo"]), Path(spec["suite"]), record_paths
    )
    require(
        checked_suite == suite,
        "native merged suite differs from paired source suite: " + repo,
    )
    report_path = output / PAIR_REPORT
    report_sha = sha(report_path)
    _require_native_report_binding(
        read(report_path), verdict, suite, checked_pack, merged, report_sha, repo
    )
    return records, hashes, report_sha, status_binding


def _select_native_status(cell: dict[str, Any], prepared: dict[str, Any]) -> dict[str, Any]:
    """Select the one successful bound attempt; retain earlier failed attempts."""
    output = Path(cell["output_root"])
    paths = sorted(output.parent.glob(cell["cell_id"] + "*.status.json"))
    bound: list[Path] = []
    for path in paths:
        status = read(path)
        if (
            status.get("returncode") == 0
            and status.get("source_commit") == prepared["source_commit"]
            and status.get("runner_sha256") == prepared["runner_sha256"]
            and status.get("searchd_sha256") == prepared["searchd_sha256"]
            and status.get("spec_sha256") == cell["spec_sha256"]
            and status.get("output_root") == str(output)
            and (
                "driver_source_sha" not in prepared
                or (
                    status.get("driver_source_sha") == prepared["driver_source_sha"]
                    and status.get("binary_build_source_sha")
                    == prepared["binary_build_source_sha"]
                    and status.get("driver_python_sha256") == prepared["driver_python_sha256"]
                    and status.get("parser_runtime_versions")
                    == prepared["parser_runtime_versions"]
                    and status.get("postrun_verification", {}).get("pair_valid") is True
                    and status.get("postrun_verification", {}).get("selected")
                    == 2 * cell["tasks"]
                )
            )
        ):
            bound.append(path)
    require(
        len(bound) == 1,
        "native successful status/source/binary is absent or ambiguous: " + cell["repository"],
    )
    return {
        "successful_attempt_path": str(bound[0]),
        "successful_attempt_sha256": sha(bound[0]),
        "other_attempts": [
            {"path": str(path), "sha256": sha(path)} for path in paths if path != bound[0]
        ],
    }


def _require_native_report_binding(
    report: dict[str, Any],
    verdict: dict[str, Any],
    suite: dict[str, Any],
    pack: dict[str, Any],
    merged: dict[str, Any],
    report_sha: str,
    repo: str,
) -> None:
    """Bind the two source-validated raw records to the runner's scored report."""
    comparisons = verdict.get("comparisons")
    require(
        isinstance(comparisons, list) and len(comparisons) == 1,
        "native pair comparison count differs: " + repo,
    )
    comparison = comparisons[0]
    require(
        report.get("status") == "diagnostic_unqualified"
        and report.get("suite_commitment_sha256") == canonical_sha(suite)
        and report.get("query_pack_sha256") == canonical_sha(pack)
        and report.get("runner_record_sha256") == canonical_sha(merged)
        and report.get("route_provenance") == merged["route_provenance"]
        and comparison.get("report_digest") == report_sha
        and comparison.get("record_digest") == report["runner_record_sha256"]
        and comparison.get("candidate_route") == "lexical"
        and comparison.get("baseline_route") == "semble-lexical-file",
        "native raw records, scored report, or verdict do not bind: " + repo,
    )


def _external_records(
    cell: dict[str, Any],
    receipt: dict[str, Any],
    tasks: dict[str, Any],
    universe: set[str],
    expected_sources: dict[str, str],
) -> tuple[dict[str, dict[str, dict[str, Any]]], dict[str, str]]:
    repo, root = cell["repository"], Path(cell["output_root"])
    require(
        receipt["repository"] == repo
        and receipt["tasks"] == len(tasks)
        and receipt["status"] == "verified"
        and receipt["spec_sha256"] == cell["spec_sha256"]
        and receipt["capture_completed"] is True
        and receipt["offline_verify_completed"] is True
        and receipt["capture_sha256"] == sha(root / "capture.json"),
        "external capture/verify receipt differs: " + repo,
    )
    capture = read(root / "capture.json")
    require(
        capture["status"] == "diagnostic_unqualified"
        and capture["tasks"] == len(tasks)
        and capture["opengrok_indexed_view_probe"]
        == "exact_indexed_inventory_and_served_bytes_bracketing_queries"
        and capture["producer_sources_sha256"] == expected_sources,
        "external capture incomplete or missing full probe: " + repo,
    )
    records: dict[str, dict[str, dict[str, Any]]] = {}
    for product in EXTERNAL_PRODUCTS:
        rows_path = root / (product + "_rows.jsonl")
        require(
            capture["rows_sha256"][product]
            == receipt["raw_rows_sha256"][product]
            == sha(rows_path),
            "external raw row hash differs: " + repo + "/" + product,
        )
        rows = scoring._unique(_bound_jsonl(rows_path), "task_id", repo + "/" + product)
        require(set(rows) == set(tasks), "external task coverage differs: " + repo + "/" + product)
        product_rows: dict[str, dict[str, Any]] = {}
        for task_id, raw in rows.items():
            task = tasks[task_id]
            require(
                raw["submitted_query"] == task["query"]
                and raw["lane"] == "symbol_only"
                and set(raw["gold_paths"])
                == {row["path"] for row in task["gold"] if row["grade"] > 0},
                "external query/gold differs: " + repo + "/" + product + "/" + task_id,
            )
            if product == "sourcegraph":
                request = raw["request_query"]
                require(
                    request.startswith(task["query"] + " ")
                    and request.endswith(" type:file patternType:keyword count:all")
                    and raw["out_of_manifest_match_count"] == 0,
                    "Sourcegraph default keyword file request differs: " + repo + "/" + task_id,
                )
            if product == "opengrok":
                require(raw["field"] == "full", "OpenGrok default full field differs")
            paths = scoring._top10(
                raw["paths"] if product == "cs" else raw["file_paths_top_10"],
                universe,
                repo + "/" + product + "/" + task_id,
            )
            eligible = (
                raw["exit_code"] == 0
                if product == "cs"
                else raw["http_status"] == 200 and raw["error"] is None
            )
            require(
                bool(scoring._score(paths, task["gold"])["hit_at_10"]) == raw["file_hit_at_10"],
                "external recorded hit differs: " + repo + "/" + product + "/" + task_id,
            )
            product_rows[task_id] = scoring._result(
                task,
                paths,
                eligible=eligible,
                status="success" if eligible else "failed_or_unsupported",
                latency_ms=raw["elapsed_ms"],
            )
        records[product] = product_rows
    return records, capture["rows_sha256"]


def _pair_rows(
    native_root: Path,
    external_root: Path,
    eligibility_path: Path,
) -> tuple[list[dict[str, Any]], list[dict[str, Any]], list[dict[str, Any]], dict[str, Any]]:
    prepared_path = (
        native_root / "prepared.json"
        if (native_root / "prepared.json").is_file()
        else native_root / "prepared-v2.json"
    )
    external_manifest_path = external_root / "manifest.json"
    external_ledger_path = external_root / "ledger.json"
    prepared, manifest, ledger = (
        read(prepared_path),
        read(external_manifest_path),
        read(external_ledger_path),
    )
    eligibility, admission_custody = _source_admission(eligibility_path, prepared, manifest)
    require(
        ledger["manifest_sha256"] == sha(external_manifest_path)
        and ledger["status"] in ALLOWED_EXTERNAL_LEDGER_STATUSES,
        "external ledger is not fully verified",
    )
    expected_external_sources = _external_producer_sources(
        Path(manifest["source_checkout"]), manifest["source_head"]
    )
    if "continuation" in ledger:
        require(
            ledger["continuation"]["source_eligibility_sha256"] == sha(eligibility_path),
            "source eligibility audit differs",
        )
    native_cells = {cell["repository"]: cell for cell in prepared["cells"]}
    external_cells = {cell["repository"]: cell for cell in manifest["cells"]}
    receipts = {row["repository"]: row for row in ledger["rows"]}
    require(
        len(native_cells) == len(prepared["cells"])
        and len(external_cells) == len(manifest["cells"])
        and len(eligibility) == len(manifest["cells"]),
        "duplicate or missing repository in pair manifests",
    )
    audit = {row["repository"]: row for row in eligibility}
    require(
        set(native_cells) == set(external_cells) == set(audit), "pair repository coverage differs"
    )
    require(
        all(row["status"] in {"VALID", "BLOCKED"} for row in eligibility),
        "unknown source eligibility status",
    )
    admitted = {repo for repo, row in audit.items() if row["status"] == "VALID"}
    blocked = [row for row in eligibility if row["status"] == "BLOCKED"]
    require(set(receipts) == admitted, "external verified cells differ from source-admitted cells")
    require(
        all(
            native_cells[repo]["source_admission"]
            == ("admitted" if repo in admitted else "blocked")
            and native_cells[repo]["tasks"] == external_cells[repo]["tasks"] == audit[repo]["tasks"]
            for repo in audit
        ),
        "native admission differs from independent source audit",
    )
    per_query: list[dict[str, Any]] = []
    bindings: list[dict[str, Any]] = []
    blocked_origins: dict[str, dict[str, Any]] = {}
    for repo in sorted(set(audit) - admitted):
        cell = external_cells[repo]
        spec = read(Path(cell["spec_path"]))
        suite = read(Path(spec["suite"]))
        task_identity = [
            (task["task_id"], task["query"], task["query_sha256"]) for task in suite["tasks"]
        ]
        blocked_origins[repo] = {
            "suite_sha256": sha(Path(spec["suite"])),
            "blind_pack_sha256": sha(Path(spec["query_pack"])),
            "task_identity_sha256": canonical_sha(task_identity),
            "file_universe_sha256": canonical_sha(suite["file_universe"]),
        }
    for repo in sorted(admitted):
        native_cell, external_cell, receipt = (
            native_cells[repo],
            external_cells[repo],
            receipts[repo],
        )
        native_spec_path, external_spec_path = (
            Path(native_cell["spec"]),
            Path(external_cell["spec_path"]),
        )
        require(
            sha(native_spec_path) == native_cell["spec_sha256"]
            and sha(external_spec_path) == external_cell["spec_sha256"],
            "paired spec digest differs: " + repo,
        )
        native_spec, external_spec = read(native_spec_path), read(external_spec_path)
        _require_default_profiles(native_spec, external_spec, repo)
        native_suite, external_suite = Path(native_spec["suite"]), Path(external_spec["suite"])
        native_pack, external_pack = (
            Path(native_spec["query_pack"]),
            Path(external_spec["query_pack"]),
        )
        require(
            sha(native_suite) == sha(external_suite) == external_cell["source_suite_sha256"]
            and sha(native_pack) == sha(external_pack) == external_cell["source_pack_sha256"]
            and sha(native_pack) == sha(Path(native_cell["output_root"]) / "query-pack.json"),
            "paired suite or blind pack bytes differ: " + repo,
        )
        suite = read(native_suite)
        tasks = scoring._unique(suite["tasks"], "task_id", "suite " + repo)
        universe = {row["path"] for row in suite["file_universe"]}
        require(
            len(tasks) == native_cell["tasks"]
            and len(universe) == len(suite["file_universe"])
            and suite["routes"] == ["lexical", "semble-lexical-file"]
            and all(
                task["category"] == INTENT
                and task["answerable"] is True
                and bool(task["gold"])
                and bool(task["file_judgments"])
                and all(row["path"] in universe for row in task["gold"] + task["file_judgments"])
                for task in tasks.values()
            ),
            "source suite task, universe, route, or gold differs: " + repo,
        )
        native_manifest = Path(native_cell["output_root"]) / "corpus-manifest.json"
        external_manifest = Path(external_cell["output_root"]) / "manifest.json"
        require(
            sha(native_manifest) == sha(external_manifest),
            "paired corpus file universe differs: " + repo,
        )
        native, native_hashes, native_report_sha, native_status = _native_records(
            native_cell, prepared, native_spec, suite, tasks, universe
        )
        external, external_hashes = _external_records(
            external_cell, receipt, tasks, universe, expected_external_sources
        )
        bindings.append(
            {
                "repository": repo,
                "suite_sha256": sha(native_suite),
                "blind_pack_sha256": sha(native_pack),
                "task_identity_sha256": canonical_sha(
                    [
                        (task["task_id"], task["query"], task["query_sha256"])
                        for task in suite["tasks"]
                    ]
                ),
                "file_universe_sha256": canonical_sha(suite["file_universe"]),
                "corpus_manifest_sha256": sha(native_manifest),
                "task_count": len(tasks),
                "native_record_sha256": native_hashes,
                "native_pair_report_sha256": native_report_sha,
                "native_execution_status": native_status,
                "external_rows_sha256": external_hashes,
            }
        )
        for task_id, task in tasks.items():
            per_query.append(
                {
                    "repository": repo,
                    "repository_commit": suite["repository_commit"],
                    "task_id": task_id,
                    "query_family_id": task["query_family_id"],
                    "submitted_query": task["query"],
                    "intended_name": task["intended_name"],
                    "source_strata": scoring._task_source_strata(task),
                    "products": {
                        **{name: rows[task_id] for name, rows in native.items()},
                        **{name: rows[task_id] for name, rows in external.items()},
                    },
                }
            )
    custody = {
        **admission_custody,
        "native_prepared_sha256": sha(prepared_path),
        "native_source_commit": prepared["source_commit"],
        "native_runner_sha256": prepared["runner_sha256"],
        "native_searchd_sha256": prepared["searchd_sha256"],
        "native_merge_validator_sha256": sha(Path(run.__file__)),
        "external_manifest_sha256": sha(external_manifest_path),
        "external_ledger_sha256": sha(external_ledger_path),
        "external_source_commit": manifest["source_head"],
        "external_producer_sources_sha256": expected_external_sources,
        "source_eligibility_sha256": sha(eligibility_path),
        "release_digest": manifest["release_digest"],
        "source_blocked_task_count": sum(row["tasks"] for row in blocked),
        "source_blocked_originals": blocked_origins,
    }
    return per_query, bindings, blocked, custody


def _macro(rows: list[dict[str, Any]], product: str, label: str) -> dict[str, Any]:
    repositories: dict[str, list[float]] = defaultdict(list)
    for row in rows:
        repositories[row["repository"]].append(row["products"][product][label]["hit_at_10"])
    means = {repo: sum(values) / len(values) for repo, values in sorted(repositories.items())}
    return {
        "repository_count": len(means),
        "equal_repository_mean": sum(means.values()) / len(means) if means else None,
        "repository_hit_at_10": means,
    }


def build(pairs: list[tuple[Path, Path, Path]]) -> dict[str, Any]:
    with _evidence_session():
        return _build(pairs)


def _build(pairs: list[tuple[Path, Path, Path]]) -> dict[str, Any]:
    require(bool(pairs), "at least one fresh pair is required")
    per_query: list[dict[str, Any]] = []
    bindings: list[dict[str, Any]] = []
    blocked: list[dict[str, Any]] = []
    custody: list[dict[str, Any]] = []
    for native_root, external_root, eligibility_path in pairs:
        rows, bound, excluded, origin = _pair_rows(native_root, external_root, eligibility_path)
        per_query.extend(rows)
        bindings.extend(bound)
        blocked.extend(excluded)
        custody.append(origin)
    repos = [row["repository"] for row in bindings]
    ids = [(row["repository"], row["task_id"]) for row in per_query]
    require(
        len(repos) == len(set(repos)) and len(ids) == len(set(ids)),
        "fresh pair repositories or tasks overlap",
    )
    _require_pair_custody_consistency(custody)
    paired = [
        row
        for row in per_query
        if all(row["products"][product]["eligible"] for product in PRODUCTS)
    ]
    excluded_task_ids = [
        row["task_id"]
        for row in per_query
        if not all(row["products"][product]["eligible"] for product in PRODUCTS)
    ]
    remaining_blocked, original_blocked = _reconcile_blocked(blocked, bindings, custody)
    groups: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for row in per_query:
        source_strata = row["source_strata"]
        groups["overall"].append(row)
        groups["literal_relation/" + source_strata["literal_relation"]].append(row)
        groups["surviving_components/" + source_strata["surviving_components"]].append(row)
        groups[
            "joint/"
            + source_strata["literal_relation"]
            + "/"
            + source_strata["surviving_components"]
        ].append(row)
    summaries = {}
    for group, rows in sorted(groups.items()):
        common = [
            row for row in rows if all(row["products"][product]["eligible"] for product in PRODUCTS)
        ]
        summaries[group] = {
            "selected_task_count": len(rows),
            "common_eligible_task_count": len(common),
            "query_family_count": len({row["query_family_id"] for row in rows}),
            "products_operational": {
                product: scoring.summarize([row["products"][product] for row in rows])
                for product in PRODUCTS
            },
            "products_common_eligible": {
                product: scoring.summarize([row["products"][product] for row in common])
                for product in PRODUCTS
            },
        }
    macro = {
        label: {product: _macro(paired, product, label) for product in PRODUCTS}
        for label in ("near_name_file", "intended_original_file")
    }
    repository_commits = {row["repository"]: row["repository_commit"] for row in per_query}
    cluster_rows = [
        (
            row["repository"],
            row["task_id"],
            row["query_family_id"],
            row["products"]["quanta"]["intended_original_file"]["hit_at_10"]
            - row["products"]["semble"]["intended_original_file"]["hit_at_10"],
        )
        for row in paired
    ]
    cluster_ci = (
        evaluator.repository_cluster_ci(
            cluster_rows,
            custody[0]["release_digest"].removeprefix("sha256:"),
            repository_commits,
            {repo: "c5_fixed_repository_cohort" for repo in repository_commits},
        )
        if len({row["repository"] for row in paired}) == len(repository_commits)
        else {
            "status": "NOT_APPLICABLE",
            "reason": "one_or_more_repositories_lack_common_eligible_tasks",
        }
    )
    return {
        "schema": "identifier_robustness_fresh_five_product_join_v1",
        "status": "diagnostic_unqualified",
        "request_contract": "default_file_search_across_five_products; distinct_file_top10; underlying_match_policies_differ",
        "execution": "offline_join_of_fresh_native_and_external_captures",
        "source_strata_policy": identifier_robustness_suite.TYPO_SOURCE_STRATA_POLICY,
        "report_tool_sha256": sha(Path(__file__)),
        "scoring_tool_sha256": sha(Path(scoring.__file__)),
        "evaluator_sha256": sha(Path(evaluator.__file__)),
        "custody": custody,
        "cell_bindings": bindings,
        "source_blocked_cells": remaining_blocked,
        "source_blocked_task_count": sum(row["tasks"] for row in remaining_blocked),
        "superseded_original_source_blocked_cells": original_blocked,
        "selected_task_count": len(per_query),
        "common_eligible_task_count": len(paired),
        "common_eligibility_excluded_task_ids": excluded_task_ids,
        "summaries": summaries,
        "repository_macro_common_eligible_hit_at_10": macro,
        "primary_paired_intended_file_hit_at_10_quanta_minus_semble_cluster_ci": cluster_ci,
        "per_query": per_query,
        "limitations": [
            "diagnostic labels and source eligibility remain distinct from human-reviewed relevance",
            "external service timing, SDK timing, and in-process Semble timing have different boundaries",
            "high host contention prevents qualified latency comparison",
            "Sourcegraph full indexed universe remains unproven by native service attestation",
            "default search does not measure each product fuzzy UI or explicit typo mode",
        ],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--pair",
        nargs=3,
        action="append",
        metavar=("NATIVE_ROOT", "EXTERNAL_ROOT", "SOURCE_ELIGIBILITY"),
        required=True,
    )
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    report = build(
        [
            (Path(native), Path(external), Path(eligibility))
            for native, external, eligibility in args.pair
        ]
    )
    with args.output.open("xb") as stream:
        stream.write(
            (json.dumps(report, sort_keys=True, ensure_ascii=False, indent=2) + "\n").encode()
        )
    print(
        json.dumps(
            {
                "output": str(args.output),
                "sha256": sha(args.output),
                "selected_tasks": report["selected_task_count"],
                "source_blocked_tasks": report["source_blocked_task_count"],
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
