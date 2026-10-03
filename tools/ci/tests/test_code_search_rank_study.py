"""Independent complete-pool ranking goldens and artifact mutation refusals."""

from __future__ import annotations

import copy
import hashlib

import pytest

from tools.benchmark.retrieval import code_search_rank_study as study
from tools.benchmark.retrieval import query_plan as qp
from tools.benchmark.retrieval import run as driver


def fixture():
    pin = {"repo_id": "repo", "revision_id": "rev", "manifest_generation": 1}

    def candidate(path, score, digest):
        repo = b"repo"
        encoded = path.encode()
        framed = (
            b"quanta-index:code-search-file:v1\0"
            + len(repo).to_bytes(8, "little")
            + repo
            + len(encoded).to_bytes(8, "little")
            + encoded
        )
        return {
            "candidate_id": "file:" + hashlib.sha256(framed).hexdigest(),
            "source_repo_id": "repo",
            "repo_id": "repo",
            "revision_id": "rev",
            "manifest_generation": 1,
            "repo_relative_path": path,
            "start_line": 1,
            "end_line": 1,
            "score": score,
            "source": {
                "file": {"source_repo_id": "repo", "repo_relative_path": path},
                "revision_id": "rev",
                "source_sha256": [digest] * 32,
            },
            "preview": {"kind": "source_file"},
        }

    usage = candidate("z_usage.rs", 109, 1)
    declaration = candidate("a_definition.rs", 105, 2)

    def details(items):
        return {"planner_trace": [{"stage": "merge", "detail": item} for item in items]}

    def page(item, eligible, exhausted):
        return {
            "generation": pin,
            "rank_unit": "file",
            "results": [item],
            "window": {
                "returned": 1,
                "outcome": {"kind": "exact_exhausted" if exhausted else "lower_bound"},
            },
            "next_cursor": None if exhausted else "opaque-cursor",
            "explanation": details(
                [
                    study.ORDINARY_SCOPE,
                    "code_search.execution.verified_matching_files=2",
                    f"code_search.execution.cursor_eligible_files={eligible}",
                    "code_search.execution.returned_files=1",
                ]
            ),
        }

    def explanation(item, declaration_bonus, occurrence, scores):
        trace = [
            "explain.score_reconciled=true",
            "explain.code_search_score.boundary_and_path=100",
            "explain.code_search_score.exact_case=5",
            "explain.code_search_score.proximity=0",
            f"explain.code_search_score.occurrence={occurrence}",
            f"explain.code_search_rank_study_v1.declaration_bonus={declaration_bonus};coverage_complete=true;original_boundary_bonus=0",
        ]
        trace += [
            f"explain.code_search_rank_study_v1.{name}={score};selected={'true' if name == 'baseline' else 'false'}"
            for name, score in zip(study.POLICIES, scores, strict=True)
        ]
        response = {
            "generation": pin,
            "presence": "indexed",
            "explanation": {
                **details(trace),
                "contributions": [
                    {
                        "signal_name": "lexical.code_search_file",
                        "signal_value": item["score"],
                        "weight": 1.0,
                        "contribution": item["score"],
                    }
                ],
            },
        }
        return {
            "candidate": item,
            "status": "returned",
            "response": response,
            "explanation_ms": 0.2,
        }

    # Fixed scores independently specified from the source fixture: a repeated
    # usage wins baseline; the declaration should lead the experimental order.
    explanations = [
        explanation(usage, 0, 4, (109, 109, 109, 107, 105, 105)),
        explanation(declaration, 64, 0, (105, 169, 105, 105, 105, 169)),
    ]
    identity = qp.derive_query_identity("code_search_file", "needle")
    captured = {
        "path": usage["repo_relative_path"],
        "score": usage["score"],
        "file_sha256": "01" * 32,
        "span_accounting": {
            "unit_id": usage["candidate_id"],
            "source_repo_id": "repo",
            "source_revision_id": "rev",
        },
    }
    record = {
        "comparison_contract": {"top_k": 1},
        "results": [
            {
                "task_id": "T1",
                "route": "lexical",
                "status": "capped",
                "query_identity": identity,
                "candidates": [captured],
            }
        ],
        "route_provenance": {"lexical": {"capture_id": "cap"}},
        "captures": {
            "cap": {
                "generation": 1,
                "source_repo_id": "repo",
                "source_revision_id": "rev",
                "execution_profile": qp.execution_profile("code_search_file"),
                "execution_profile_sha256": qp.execution_profile_sha256("code_search_file"),
            }
        },
    }
    pack = {
        "tasks": [{"task_id": "T1", "query": "needle"}],
        "file_universe": [
            {"path": "z_usage.rs", "file_sha256": "01" * 32},
            {"path": "a_definition.rs", "file_sha256": "02" * 32},
        ],
    }
    artifact = {
        "schema_version": 1,
        "kind": "quanta_code_search_rank_study",
        "qualification": "diagnostic_unqualified",
        "record_sha256": "a" * 64,
        "policy": "code_search_file",
        "execution_profile_sha256": qp.execution_profile_sha256("code_search_file"),
        "timing_boundary": "post_measurement_sdk_paging_and_explanations",
        "diagnostic_ms": 1.0,
        "limits": {"max_files": 10, "max_pages": 10, "timeout_ms": 1000},
        "results": [
            {
                "task_id": "T1",
                "route": "lexical",
                "generation": pin,
                "query_identity": {
                    **identity,
                    "policy_config_sha256": hashlib.sha256(
                        qp.policy_config_canonical("code_search_file").encode()
                    ).hexdigest(),
                },
                "effective_request": {
                    "syntax": "code_search",
                    "query_text": "needle",
                    "generation": pin,
                    "constraints": {"language_any_of": []},
                    "top_k": 1,
                },
                "collection": {
                    "status": "returned",
                    "reason": None,
                    "pool_complete": True,
                    "diagnostic_ms": 1.0,
                    "pages": [page(usage, 2, False), page(declaration, 1, True)],
                    "explanations": explanations,
                },
            }
        ],
    }
    suite = {
        "comparison_contract": {"top_k": 1},
        "tasks": [
            {
                "task_id": "T1",
                "file_judgments": [
                    {"path": "a_definition.rs", "grade": 3},
                    {"path": "z_usage.rs", "grade": 0},
                ],
            }
        ],
    }
    return artifact, record, pack, suite


def validate(artifact, record, pack):
    return study.validate_artifact(artifact, record, "a" * 64, pack)


def test_complete_pool_promotes_outside_original_top_k_with_fixed_file_goldens():
    artifact, record, pack, suite = fixture()
    report = study.compose(suite, validate(artifact, record, pack))
    declaration = report["comparisons"]["declaration_only"]
    assert declaration["paired_means"] == {
        "baseline": {"file_ndcg": 0.0, "file_hit": 0.0, "file_mrr": 0.0},
        "candidate": {"file_ndcg": 1.0, "file_hit": 1.0, "file_mrr": 1.0},
    }
    assert declaration["samples"][0]["candidate_top_k"] == ["a_definition.rs"]
    assert report["comparisons"]["occurrence_none"]["samples"][0]["candidate_top_k"] == [
        "a_definition.rs"
    ]
    assert report["comparisons"]["boundary_only"]["paired_means"]["candidate"]["file_hit"] == 0.0
    assert report["qualification"] == "diagnostic_unqualified"
    assert report["selected_policy"] == "baseline"
    assert report["declaration_span_metrics"] == "not_applicable_file_level_features"


def test_partial_top_k_pool_is_excluded_and_preserves_original_quality_row():
    artifact, record, pack, suite = fixture()
    original = copy.deepcopy(record)
    collection = artifact["results"][0]["collection"]
    collection.update(status="partial", reason="diagnostic_page_limit", pool_complete=False)
    collection["pages"] = collection["pages"][:1]
    collection["explanations"] = collection["explanations"][:1]
    report = study.compose(suite, validate(artifact, record, pack))
    for comparison in report["comparisons"].values():
        assert comparison["eligible_task_ids"] == []
        assert comparison["paired_means"]["candidate"]["file_hit"] is None
        assert comparison["excluded"] == [{"task_id": "T1", "reason": "diagnostic_page_limit"}]
    assert record == original


@pytest.mark.parametrize(
    "mutation,match",
    [
        ("digest", "record bytes"),
        ("request", "effective request"),
        ("original_score", "record file authority"),
        ("generation", "generation"),
        ("counts", "full pool"),
        ("exhaustion", "exhaustion"),
        ("duplicate", "duplicate file"),
        ("source_hash", "source-bound universe"),
        ("study_score", "algebra"),
        ("selected", "selected experimental"),
        ("explain_generation", "generation/presence"),
        ("task_omitted", "omitted"),
        ("nonfinite", "finite number"),
        ("first_window", "original result status"),
        ("source_pin", "original capture"),
        ("limit", "producer bounds"),
    ],
)
def test_bound_complete_study_rejects_mutations(mutation, match):
    artifact, record, pack, _ = fixture()
    row = artifact["results"][0]
    collection = row["collection"]
    trace = collection["explanations"][1]["response"]["explanation"]["planner_trace"]
    if mutation == "digest":
        artifact["record_sha256"] = "b" * 64
    elif mutation == "request":
        row["effective_request"]["constraints"] = {"language_any_of": ["rust"]}
    elif mutation == "original_score":
        record["results"][0]["candidates"][0]["score"] = 108
    elif mutation == "generation":
        collection["pages"][1]["generation"] = {**row["generation"], "manifest_generation": 2}
    elif mutation == "counts":
        collection["pages"][1]["explanation"]["planner_trace"][2]["detail"] = (
            "code_search.execution.cursor_eligible_files=2"
        )
    elif mutation == "exhaustion":
        collection["pages"] = collection["pages"][:1]
    elif mutation == "duplicate":
        collection["pages"][1]["results"] = collection["pages"][0]["results"]
    elif mutation == "source_hash":
        pack["file_universe"][1]["file_sha256"] = "03" * 32
    elif mutation == "study_score":
        trace[-1]["detail"] = "explain.code_search_rank_study_v1.combined=170;selected=false"
    elif mutation == "selected":
        trace[-1]["detail"] = "explain.code_search_rank_study_v1.combined=169;selected=true"
    elif mutation == "explain_generation":
        collection["explanations"][1]["response"]["generation"] = {
            **row["generation"],
            "manifest_generation": 2,
        }
    elif mutation == "task_omitted":
        artifact["results"] = []
    elif mutation == "nonfinite":
        artifact["diagnostic_ms"] = float("nan")
    elif mutation == "first_window":
        collection["pages"][0]["window"]["outcome"]["kind"] = "exact_exhausted"
    elif mutation == "source_pin":
        row["generation"]["revision_id"] = "other-revision"
    elif mutation == "limit":
        artifact["limits"]["timeout_ms"] = 300_001
    with pytest.raises(ValueError, match=match):
        validate(artifact, record, pack)


def test_zero_hit_pool_still_requires_original_source_identity():
    artifact, record, pack, suite = fixture()
    record["results"][0].update(status="abstained", candidates=[])
    collection = artifact["results"][0]["collection"]
    page = collection["pages"][1]
    page["results"] = []
    page["window"]["returned"] = 0
    page["explanation"]["planner_trace"][1:4] = [
        {"stage": "merge", "detail": f"code_search.execution.{name}=0"}
        for name in ("verified_matching_files", "cursor_eligible_files", "returned_files")
    ]
    collection.update(pages=[page], explanations=[])
    report = study.compose(suite, validate(artifact, record, pack))
    assert report["comparisons"]["boundary_only"]["paired_means"]["baseline"]["file_hit"] == 0
    artifact["results"][0]["generation"]["repo_id"] = "other-repo"
    with pytest.raises(ValueError, match="original capture"):
        validate(artifact, record, pack)


def test_refused_explain_remains_visible_and_does_not_score_as_zero():
    artifact, record, pack, suite = fixture()
    artifact["results"][0]["collection"]["explanations"][1] = {
        "candidate": artifact["results"][0]["collection"]["pages"][1]["results"][0],
        "status": "refused",
        "error": "work_budget",
        "explanation_ms": 0.2,
    }
    report = study.compose(suite, validate(artifact, record, pack))
    assert (
        report["comparisons"]["declaration_only"]["excluded"][0]["reason"]
        == "explanation_incomplete_or_refused"
    )


def test_unknown_declaration_excludes_only_declaration_policies_with_paired_denominators():
    artifact, record, pack, suite = fixture()
    item = artifact["results"][0]["collection"]["explanations"][1]
    trace = item["response"]["explanation"]["planner_trace"]
    trace[5]["detail"] = (
        "explain.code_search_rank_study_v1.declaration_bonus=unknown;coverage_complete=false;original_boundary_bonus=0"
    )
    trace[7]["detail"] = "explain.code_search_rank_study_v1.declaration_only=105;selected=false"
    trace[-1]["detail"] = "explain.code_search_rank_study_v1.combined=105;selected=false"
    report = study.compose(suite, validate(artifact, record, pack))
    assert report["comparisons"]["declaration_only"]["eligible_task_ids"] == []
    assert report["comparisons"]["occurrence_none"]["eligible_task_ids"] == ["T1"]


def test_rank_study_limits_require_ordinary_file_policy_and_lexical_only():
    limits = {"max_files": 100, "max_pages": 10, "timeout_ms": 1000}
    assert driver.rank_study_configuration(limits, "code_search_file", ["lexical"]) == limits
    with pytest.raises(driver.RunError, match="whole-process resource"):
        driver.rank_study_configuration(limits, "code_search_file", ["lexical"], speed_claim=True)
    for bad in [
        {**limits, "max_files": True},
        {**limits, "max_pages": 0},
        {**limits, "unexpected": 1},
    ]:
        with pytest.raises(driver.RunError):
            driver.rank_study_configuration(bad, "code_search_file", ["lexical"])
    for policy, routes in [
        ("code_search_typo_file", ["lexical"]),
        ("code_search_file", ["lexical", "hybrid"]),
    ]:
        with pytest.raises(driver.RunError):
            driver.rank_study_configuration(limits, policy, routes)
