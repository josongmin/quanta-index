"""Independent fixture for the robustness report's admission and join contract."""

from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path

import pytest

from tools.benchmark.retrieval import evaluator, identifier_osa1_absence_suite, source_oracle
from tools.benchmark.retrieval.identifier_robustness_report import (
    _verify_paired_capture_identity,
    _write_output,
    compare_paired_clean_typo,
    compose,
    compose_validated,
    verify_census_against_source,
    verify_generation_manifest,
)


def test_paired_capture_requires_same_binary_and_valid_separate_activations():
    def record(digest, *, binary="b"):
        return {
            "route_provenance": {"lexical": {"capture_id": "q"}},
            "captures": {
                "q": {
                    "system": "quanta",
                    "runner_binary": {"digest": "a"},
                    "searchd_binary": {"binary_digest": binary},
                    "activation_digest": digest,
                    "generation": 1,
                }
            },
        }

    _verify_paired_capture_identity(record("a" * 64), record("b" * 64), "lexical", "lexical")
    with pytest.raises(ValueError, match="paired capture differs in searchd_binary"):
        _verify_paired_capture_identity(
            record("a" * 64), record("b" * 64, binary="changed"), "lexical", "lexical"
        )
    with pytest.raises(ValueError, match="invalid activation digest"):
        _verify_paired_capture_identity(record("a" * 64), record("not-a-digest"), "lexical", "lexical")


def test_paired_clean_to_typo_uses_intended_gold_and_common_eligible_families():
    source = {
        "repository_commit": "a" * 40,
        "file_universe": [{"path": "x.go"}],
        "comparison_contract": {"top_k": 10},
    }
    clean = {
        **source,
        "tasks": [
            {
                "task_id": "CLN-L1",
                "query_family_id": "f1",
                "query": "Write",
                "file_judgments": [{"path": "x.go", "grade": 3}],
            },
            {
                "task_id": "CLN-L2",
                "query_family_id": "f2",
                "query": "Read",
                "file_judgments": [{"path": "y.go", "grade": 3}],
            },
            {
                "task_id": "CLN-L3",
                "query_family_id": "f3",
                "query": "Load",
                "file_judgments": [{"path": "z.go", "grade": 3}],
            },
        ],
    }
    noisy = {
        **source,
        "tasks": [
            {
                "task_id": "T1",
                "query_family_id": "f1",
                "query": "Wrtie",
                "intended_name": "Write",
                "file_judgments": [{"path": "x.go", "grade": 3}],
            },
            {
                "task_id": "T2",
                "query_family_id": "f2",
                "query": "Raed",
                "intended_name": "Read",
                "file_judgments": [{"path": "y.go", "grade": 3}],
            },
            {
                "task_id": "T3",
                "query_family_id": "f3",
                "query": "Laod",
                "intended_name": "Load",
                "file_judgments": [{"path": "z.go", "grade": 3}],
            },
        ],
    }
    census = {
        "lanes": {
            "typo-transposition": {
                "gold_kind": "intended_original_name",
                "records": [
                    {
                        "task_id": "T1",
                        "status": "admitted",
                        "base_task_id": "L1",
                        "base_query": "Write",
                        "query": "Wrtie",
                        "query_family_id": "f1",
                    },
                    {
                        "task_id": "T2",
                        "status": "admitted",
                        "base_task_id": "L2",
                        "base_query": "Read",
                        "query": "Raed",
                        "query_family_id": "f2",
                    },
                    {
                        "task_id": "T3",
                        "status": "admitted",
                        "base_task_id": "L3",
                        "base_query": "Load",
                        "query": "Laod",
                        "query_family_id": "f3",
                    },
                ],
            }
        }
    }

    def diagnostic(rows):
        return {
            "status": "diagnostic_unqualified",
            "repository_commit": source["repository_commit"],
            "comparison_contract": source["comparison_contract"],
            "judgment_metrics": {"file_judgments": {"routes": {"lexical": {}}, "per_query": rows}},
        }

    def scored(task_id, hit, mrr, ndcg):
        return {
            "task_id": task_id,
            "route": "lexical",
            "eligible": True,
            "scores": {"hit_at_10": hit, "mrr_at_10": mrr, "ndcg_at_10": ndcg},
        }

    clean_report = diagnostic(
        [
            scored("CLN-L1", 1, 1, 1),
            scored("CLN-L2", 1, 0.5, 0.63),
            {
                "task_id": "CLN-L3",
                "route": "lexical",
                "eligible": False,
                "reason": "execution_status_unavailable",
            },
        ]
    )
    noisy_report = diagnostic([scored("T1", 0, 0, 0), scored("T2", 1, 1, 1), scored("T3", 1, 1, 1)])
    result = compare_paired_clean_typo(
        clean,
        clean_report,
        noisy,
        noisy_report,
        census,
        "typo-transposition",
        clean_route="lexical",
        typo_route="lexical",
    )
    assert result["paired_eligible_families"] == 2
    assert result["excluded"] == [
        {"query_family_id": "f3", "reason": "clean_execution_status_unavailable"}
    ]
    assert result["metrics"]["hit_at_10"] == {
        "clean_mean": 1.0,
        "typo_mean": 0.5,
        "delta_typo_minus_clean": -0.5,
    }
    assert result["metrics"]["mrr_at_10"] == {
        "clean_mean": 0.75,
        "typo_mean": 0.5,
        "delta_typo_minus_clean": -0.25,
    }
    noisy["tasks"][0]["file_judgments"] = [{"path": "wrong.go", "grade": 3}]
    with pytest.raises(ValueError, match="paired family or intended gold mismatch"):
        compare_paired_clean_typo(
            clean,
            clean_report,
            noisy,
            noisy_report,
            census,
            "typo-transposition",
            clean_route="lexical",
            typo_route="lexical",
        )


def fixture(policy="keyword_file"):
    contract = "go_declaration_name_prefix_v1"
    specs = (
        ("U1", "unique", "success", True, 1.0),
        ("U2", "unique", "unavailable", False, None),
        ("A1", "ambiguous", "success", True, 1.0),
        ("A2", "ambiguous", "capped", True, 0.0),
    )
    tasks = [
        {
            "task_id": task_id,
            "query": "name" + task_id,
            "query_family_id": "family" + task_id,
            "split": "eval",
            "answerable": True,
            "file_judgments": [{"path": "x.go"}],
            "source_oracle": {"contract": contract, "unit": "distinct_file"},
        }
        for task_id, *_ in specs
    ]
    suite = {
        "suite_id": "fixed-prefix-suite",
        "repository_commit": "a" * 40,
        "comparison_contract": {"top_k": 10},
        "routes": ["lexical"],
        "tasks": tasks,
    }
    census = {
        "lanes": {
            "prefix": {
                "contract": contract,
                "admitted": 4,
                "records": [
                    {
                        "task_id": task_id,
                        "status": "admitted",
                        "query": "name" + task_id,
                        "query_family_id": "family" + task_id,
                        "answer_class": group,
                        "matched_names": 1 if group == "unique" else 2,
                        "gold_files": 1,
                    }
                    for task_id, group, *_ in specs
                ]
                + [{"task_id": "X1", "status": "ineligible", "query": None}],
            }
        }
    }
    capture = {
        "system": "quanta",
        "execution_profile": {"policy": policy},
    }
    record = {
        "comparison_contract": suite["comparison_contract"],
        "route_provenance": {"lexical": {"capture_id": "q"}},
        "captures": {"q": capture},
        "results": [
            {
                "task_id": task_id,
                "route": "lexical",
                "rank_unit": "distinct_file",
                "ordering": "score_desc_path_tiebreak",
                "status": status,
                "candidates": [],
            }
            for task_id, _group, status, *_ in specs
        ],
    }
    scored = [
        {
            "task_id": task_id,
            "route": "lexical",
            "eligible": eligible,
            **(
                {"scores": {"hit_at_10": hit}}
                if eligible
                else {"reason": "execution_status_unavailable"}
            ),
        }
        for task_id, _group, _status, eligible, hit in specs
    ]
    diagnostic = {
        "report_scope": "single_route_independent_judgment_diagnostic_v1",
        "status": "diagnostic_unqualified",
        "suite_commitment_sha256": evaluator.digest(evaluator.canonical(suite)),
        "runner_record_sha256": evaluator.digest(evaluator.canonical(record)),
        "suite_id": suite["suite_id"],
        "repository_commit": suite["repository_commit"],
        "comparison_contract": suite["comparison_contract"],
        "route": "lexical",
        "route_provenance": record["route_provenance"],
        "captures": record["captures"],
        "selected_eval_tasks": 4,
        "judgment_metrics": {
            "file_judgments": {
                "routes": {
                    "lexical": {
                        "rank_unit": "distinct_file",
                        "ordering": "score_desc_path_tiebreak",
                        "eligible_task_ids": ["U1", "A1", "A2"],
                        "eligible_count": 3,
                        "selected_answerable_tasks": 4,
                        "excluded": [{"task_id": "U2", "reason": "execution_status_unavailable"}],
                        "status_counts": {"success": 2, "capped": 1, "unavailable": 1},
                    }
                },
                "per_query": scored,
            }
        },
        "no_answer": {
            "task_ids": [],
            "sample_count": 0,
            "abstained": 0,
            "abstention_rate": evaluator.NOT_APPLICABLE,
            "nonempty_results": 0,
            "nonempty_result_rate": evaluator.NOT_APPLICABLE,
            "status_counts": {},
        },
    }
    return suite, census, record, diagnostic


def test_fixed_golden_preserves_admission_eligibility_and_capped_hit():
    output = compose(*fixture(), "prefix")
    assert output["content_absence_replay_verified"] is False
    assert (
        output["requested"],
        output["not_admitted"],
        output["submitted"],
        output["eligible"],
    ) == (5, 1, 4, 3)
    assert output["strata"]["unique"] == {"admitted": 2, "eligible": 1, "hit_at_10_count": 1}
    assert output["strata"]["ambiguous"] == {"admitted": 2, "eligible": 2, "hit_at_10_count": 1}
    assert output["status_counts"] == {"success": 2, "capped": 1, "unavailable": 1}
    assert output["detailed_strata"] == {
        "operation": {},
        "length": {},
        "short_common": {},
        "overcorrection_candidate": {},
        "near_name_collision": {},
    }
    assert output["sampling"] == {}
    assert (
        output["policy"],
        output["case"],
        output["scope"],
        output["rank_unit"],
        output["ordering"],
    ) == (
        "keyword_file",
        "sensitive",
        "content_and_path",
        "distinct_file",
        "score_desc_path_tiebreak",
    )


def test_operation_and_length_breakdown_count_capped_as_eligible():
    suite, census, record, diagnostic = fixture()
    for row, operation, length in zip(
        census["lanes"]["prefix"]["records"][:4],
        ("insertion", "insertion", "transposition", "transposition"),
        ("long", "short", "long", "short"),
        strict=True,
    ):
        row["generation"] = {"operation": operation}
        row["strata"] = {
            "length": length,
            "short_common": "yes" if length == "short" else "no",
            "overcorrection_candidate": "no",
            "near_name_collision": "no",
        }
    output = compose(suite, census, record, diagnostic, "prefix")
    assert output["detailed_strata"]["operation"] == {
        "insertion": {"admitted": 2, "eligible": 1, "hit_at_10_count": 1, "capped": 0},
        "transposition": {"admitted": 2, "eligible": 2, "hit_at_10_count": 1, "capped": 1},
    }
    assert output["detailed_strata"]["length"] == {
        "long": {"admitted": 2, "eligible": 2, "hit_at_10_count": 2, "capped": 0},
        "short": {"admitted": 2, "eligible": 1, "hit_at_10_count": 0, "capped": 1},
    }
    assert output["detailed_strata"]["short_common"]["yes"] == {
        "admitted": 2,
        "eligible": 1,
        "hit_at_10_count": 0,
        "capped": 1,
    }
    del census["lanes"]["prefix"]["records"][0]["generation"]
    with pytest.raises(ValueError, match="incomplete operation census metadata"):
        compose(suite, census, record, diagnostic, "prefix")


def test_seeded_sample_and_extra_multifile_are_separate():
    suite, census, record, diagnostic = fixture()
    census["lanes"]["prefix"]["records"][-1]["query_family_id"] = "familyX1"
    census.update(
        random_sample_family_ids=["familyU1", "familyU2"],
        multi_file_stratum_family_ids=["familyA1", "familyA2", "familyX1"],
        random_sample_families=2,
        multi_file_stratum_families=3,
        overlap_random_and_multi_file=0,
    )
    output = compose(suite, census, record, diagnostic, "prefix")
    assert output["sampling"] == {
        "additional_multi_file": {
            "admitted": 2,
            "eligible": 2,
            "hit_at_10_count": 1,
            "capped": 1,
        },
        "seeded_random": {
            "admitted": 2,
            "eligible": 1,
            "hit_at_10_count": 1,
            "capped": 0,
        },
    }
    census["random_sample_family_ids"].append("familyX1")
    with pytest.raises(ValueError, match="sampling family census mismatch"):
        compose(suite, census, record, diagnostic, "prefix")


@pytest.mark.parametrize(
    "generation", ["paired_full_osa1_casefold_v2", "paired_full_osa1_casefold_v3"]
)
def test_full_population_keeps_unselected_families_visible(generation):
    suite, census, record, diagnostic = fixture()
    census["lanes"]["prefix"]["records"][-1]["query_family_id"] = "familyX1"
    census.update(
        generation=generation,
        population_families=5,
        random_sample_family_ids=["familyU1"],
        multi_file_family_ids=["familyA1"],
    )
    output = compose(suite, census, record, diagnostic, "prefix")
    assert output["sampling"]["seeded_random"]["hit_at_10_count"] == 1
    assert output["sampling"]["additional_multi_file"]["hit_at_10_count"] == 1
    assert output["sampling"]["remaining_population"] == {
        "admitted": 2,
        "eligible": 1,
        "hit_at_10_count": 0,
        "capped": 1,
    }


def test_code_search_file_reports_scored_family_separately():
    output = compose(*fixture("code_search_file"), "prefix")
    assert output["policy"] == "code_search_file"
    assert output["case"] == "folded"
    assert output["scope"] == "content_and_path"
    assert output["ordering"] == "score_desc_path_tiebreak"
    assert output["lane"] == "prefix"
    assert output["status"] == "diagnostic_unqualified"


def test_report_refuses_partially_declared_request_contract():
    suite, census, record, diagnostic = fixture("code_search_file")
    suite["tasks"][0]["evaluation_contract"] = {
        "request_mode": "default_file_search",
        "gold_unit": "distinct_file",
        "result_unit": "distinct_file",
    }
    diagnostic["suite_commitment_sha256"] = evaluator.digest(evaluator.canonical(suite))
    with pytest.raises(ValueError, match="mixed or missing evaluation contract"):
        compose(suite, census, record, diagnostic, "prefix")


def test_code_search_typo_file_keeps_answerable_scores():
    evidence = fixture("code_search_typo_file")
    output = compose(*evidence, "prefix")
    assert (output["policy"], output["scope"], output["strata"]["unique"]["hit_at_10_count"]) == (
        "code_search_typo_file",
        "identifier",
        1,
    )


def test_code_search_file_pair_report_selects_one_bound_route(monkeypatch):
    suite, census, record, diagnostic = fixture("code_search_file")
    baseline = "semble-lexical-file"
    suite["routes"].append(baseline)
    record["captures"]["s"] = {"system": "semble", "execution_profile": {"mode": "lexical-file"}}
    record["route_provenance"][baseline] = {"capture_id": "s"}
    for row in list(record["results"]):
        other = copy.deepcopy(row)
        other["route"] = baseline
        other["ordering"] = "score_desc_native_tiebreak"
        record["results"].append(other)
    file_judgments = diagnostic["judgment_metrics"]["file_judgments"]
    file_judgments["routes"][baseline] = copy.deepcopy(file_judgments["routes"]["lexical"])
    for row in list(file_judgments["per_query"]):
        other = copy.deepcopy(row)
        other["route"] = baseline
        file_judgments["per_query"].append(other)
    diagnostic["report_scope"] = "paired_independent_file_judgment_diagnostic_v1"
    diagnostic["baseline_route"] = baseline
    diagnostic["candidate_route"] = "lexical"
    diagnostic.pop("route")
    no_answer = diagnostic.pop("no_answer")
    diagnostic["no_answer"] = {"routes": {"lexical": no_answer, baseline: copy.deepcopy(no_answer)}}
    diagnostic.pop("selected_eval_tasks")
    diagnostic["suite_commitment_sha256"] = evaluator.digest(evaluator.canonical(suite))
    diagnostic["runner_record_sha256"] = evaluator.digest(evaluator.canonical(record))
    diagnostic["route_provenance"] = copy.deepcopy(record["route_provenance"])
    diagnostic["captures"] = copy.deepcopy(record["captures"])
    monkeypatch.setattr(
        evaluator, "evaluate_paired_file_diagnostic", lambda *_: copy.deepcopy(diagnostic)
    )
    output = compose_validated(suite, {}, record, diagnostic, census, "prefix", route="lexical")
    assert output["route"] == "lexical"
    assert output["eligible"] == 3
    with pytest.raises(ValueError, match="explicit declared route"):
        compose(suite, census, record, diagnostic, "prefix")
    with pytest.raises(ValueError, match="explicit declared route"):
        compose(suite, census, record, diagnostic, "prefix", route="missing")


@pytest.mark.parametrize(
    ("target", "mutation", "reason"),
    [
        (0, lambda x: x["tasks"].append(copy.deepcopy(x["tasks"][0])), "duplicate task ID"),
        (
            1,
            lambda x: x["lanes"]["prefix"]["records"].append(
                copy.deepcopy(x["lanes"]["prefix"]["records"][0])
            ),
            "duplicate task ID",
        ),
        (
            1,
            lambda x: x["lanes"]["prefix"]["records"][0].update(query="changed"),
            "census query/family mismatch",
        ),
        (
            1,
            lambda x: x["lanes"]["prefix"]["records"][0].update(answer_class="ambiguous"),
            "answer class mismatch",
        ),
        (2, lambda x: x["results"].pop(), "record/report mismatch"),
        (2, lambda x: x["results"][0].update(rank_unit="chunk"), "record/report mismatch"),
        (
            2,
            lambda x: x["results"][0].update(ordering="path_order_constant_score"),
            "record/report mismatch",
        ),
        (
            3,
            lambda x: x["judgment_metrics"]["file_judgments"]["per_query"].pop(),
            "missing or extra diagnostic task",
        ),
        (
            3,
            lambda x: x["judgment_metrics"]["file_judgments"]["per_query"].append(
                copy.deepcopy(x["judgment_metrics"]["file_judgments"]["per_query"][0])
            ),
            "duplicate task ID",
        ),
        (
            3,
            lambda x: x["judgment_metrics"]["file_judgments"]["routes"]["lexical"].update(
                ordering="path_order_constant_score"
            ),
            "diagnostic ordering mismatch",
        ),
    ],
)
def test_refuses_mismatched_or_partial_evidence(target, mutation, reason):
    values = list(fixture())
    mutation(values[target])
    # Suite and record mutations must remain tied to an otherwise valid report
    # to exercise the later invariant, except when testing digest rejection.
    if target == 0 and reason == "duplicate task ID":
        values[3]["suite_commitment_sha256"] = evaluator.digest(evaluator.canonical(values[0]))
        values[3]["selected_eval_tasks"] = 5
    with pytest.raises(ValueError, match=reason):
        compose(*values, "prefix")


def test_refuses_policy_and_source_drift():
    values = list(fixture())
    values[2]["captures"]["q"]["execution_profile"]["policy"] = "substring_file"
    values[3]["captures"] = copy.deepcopy(values[2]["captures"])
    values[3]["runner_record_sha256"] = evaluator.digest(evaluator.canonical(values[2]))
    with pytest.raises(ValueError, match="record ordering mismatch"):
        compose(*values, "prefix")
    values = list(fixture())
    values[3]["repository_commit"] = "b" * 40
    with pytest.raises(ValueError, match="source mismatch"):
        compose(*values, "prefix")


def test_policy_refusal_is_separate_from_generator_ineligibility():
    values = list(fixture())
    records = values[1]["lanes"]["prefix"]["records"]
    records.append({"task_id": "X2", "status": "admitted", "query": "64Sl"})
    values[1]["lanes"]["prefix"]["admitted"] = 5
    output = compose(*values, "prefix")
    assert (
        output["requested"],
        output["not_admitted"],
        output["unsupported_query_form"],
        output["submitted"],
    ) == (6, 1, 1, 4)
    records[-1]["query"] = "supportedName"
    with pytest.raises(ValueError, match="admitted task missing despite supported query form"):
        compose(*values, "prefix")


def test_replay_rejects_score_tampering_without_rescoring_here(monkeypatch):
    suite, census, record, diagnostic = fixture()
    expected = copy.deepcopy(diagnostic)
    monkeypatch.setattr(evaluator, "evaluate_diagnostic", lambda *_: expected)
    assert compose_validated(suite, {}, record, diagnostic, census, "prefix")["eligible"] == 3
    diagnostic["judgment_metrics"]["file_judgments"]["per_query"][0]["scores"]["hit_at_10"] = 0.0
    with pytest.raises(ValueError, match="diagnostic differs from evaluator replay"):
        compose_validated(suite, {}, record, diagnostic, census, "prefix")


def test_new_contract_replay_rejects_mrr_tampering(monkeypatch):
    suite, census, record, diagnostic = fixture("code_search_file")
    for task in suite["tasks"]:
        task["evaluation_contract"] = {
            "request_mode": "default_file_search",
            "gold_unit": "distinct_file",
            "result_unit": "distinct_file",
        }
    diagnostic["suite_commitment_sha256"] = evaluator.digest(evaluator.canonical(suite))
    diagnostic["judgment_metrics"]["file_judgments"]["per_query"][0]["scores"]["mrr_at_10"] = 1.0
    expected = copy.deepcopy(diagnostic)
    monkeypatch.setattr(evaluator, "evaluate_diagnostic", lambda *_: copy.deepcopy(expected))
    assert compose_validated(suite, {}, record, diagnostic, census, "prefix")["eligible"] == 3
    diagnostic["judgment_metrics"]["file_judgments"]["per_query"][0]["scores"]["mrr_at_10"] = 0.5
    with pytest.raises(ValueError, match="diagnostic differs from evaluator replay"):
        compose_validated(suite, {}, record, diagnostic, census, "prefix")


def test_output_is_exclusive_and_outside_checkout(tmp_path):
    target = tmp_path / "report.json"
    _write_output(target, '{"ok":true}\n')
    assert target.read_text() == '{"ok":true}\n'
    with pytest.raises(FileExistsError):
        _write_output(target, "clobber")
    with pytest.raises(ValueError, match="absolute"):
        _write_output(Path("relative.json"), "bad")
    checkout_file = Path(__file__).resolve().parents[3] / "AGENTS.md"
    with pytest.raises(ValueError, match="outside source checkout"):
        _write_output(checkout_file, "bad")
    corpus = tmp_path / "corpus"
    corpus.mkdir()
    with pytest.raises(ValueError, match="outside corpus checkout"):
        _write_output(corpus / "report.json", "bad", repo=corpus)


def test_no_answer_success_with_candidates_is_visible():
    suite, census, record, diagnostic = fixture()
    suite["tasks"].append(
        {
            "task_id": "N1",
            "query": "absentName",
            "query_family_id": "familyN1",
            "split": "eval",
            "answerable": False,
            "file_judgments": [],
            "source_oracle": {
                "contract": "go_declaration_name_prefix_v1",
                "unit": "distinct_file",
            },
        }
    )
    census["lanes"]["prefix"]["records"].append(
        {
            "task_id": "N1",
            "status": "admitted",
            "query": "absentName",
            "query_family_id": "familyN1",
            "answer_class": "no_answer",
            "matched_names": 0,
            "gold_files": 0,
        }
    )
    census["lanes"]["prefix"]["admitted"] = 5
    record["results"].append(
        {
            "task_id": "N1",
            "route": "lexical",
            "rank_unit": "distinct_file",
            "ordering": "score_desc_path_tiebreak",
            "status": "success",
            "candidates": [{"path": "unrelated.go"}],
        }
    )
    diagnostic["suite_commitment_sha256"] = evaluator.digest(evaluator.canonical(suite))
    diagnostic["runner_record_sha256"] = evaluator.digest(evaluator.canonical(record))
    diagnostic["selected_eval_tasks"] = 5
    diagnostic["no_answer"] = {
        "task_ids": ["N1"],
        "sample_count": 1,
        "abstained": 0,
        "abstention_rate": 0.0,
        "nonempty_results": 1,
        "nonempty_result_rate": 1.0,
        "status_counts": {"success": 1},
    }
    output = compose(suite, census, record, diagnostic, "prefix")
    assert output["strata"]["no_answer"]["admitted"] == 1
    assert output["evaluation_intent"] == "declaration_name_file_retrieval_diagnostic"
    assert output["no_answer"] == {
        "negative_reference_scope": "declaration_local_name_absent",
        "sample_count": 1,
        "abstained": 0,
        "abstention_rate": 0.0,
        "nonempty_results": 1,
        "nonempty_result_rate": 1.0,
        "status_counts": {"success": 1},
    }
    diagnostic["no_answer"]["task_ids"] = []
    with pytest.raises(ValueError, match="no-answer task ID mismatch"):
        compose(suite, census, record, diagnostic, "prefix")


@pytest.mark.parametrize(
    ("policy", "contract", "intent", "scope", "lane"),
    [
        (
            "keyword_file",
            "ascii_content_absent_casefold_v1",
            "content_absence_negative_control",
            "folded_content_absent",
            "no-answer-content",
        ),
        (
            "code_search_typo_file",
            "ascii_identifier_osa1_absent_casefold_v1",
            "identifier_osa1_absence_negative_control",
            "folded_ascii_identifier_osa1_absent",
            "typo-osa1-absence",
        ),
    ],
)
def test_new_content_absence_contract_is_separate_from_legacy(
    policy, contract, intent, scope, lane
):
    suite, census, record, diagnostic = fixture(policy)
    suite["tasks"] = [
        {
            "task_id": "NOC1",
            "query": "absentName",
            "query_family_id": "familyNOC1",
            "split": "eval",
            "answerable": False,
            "file_judgments": [],
            "source_oracle": {"contract": contract, "unit": "distinct_file"},
        }
    ]
    source_lane = "no-answer-content" if policy == "code_search_typo_file" else "no-answer"
    census["lanes"] = {
        source_lane: {
            "records": [{"task_id": "NOA1", "query": "absentName", "status": "admitted"}]
        },
        lane: {
            "contract": contract,
            "derived_from": source_lane,
            "source_admitted": 1,
            "admitted": 1,
            "excluded": 0,
            "excluded_probes": [],
            "records": [{"task_id": "NOC1", "source_task_id": "NOA1"}],
        },
    }
    record["results"] = [
        {
            "task_id": "NOC1",
            "route": "lexical",
            "rank_unit": "distinct_file",
            "ordering": "score_desc_path_tiebreak",
            "status": "abstained",
            "candidates": [],
        }
    ]
    route = diagnostic["judgment_metrics"]["file_judgments"]["routes"]["lexical"]
    route.update(
        eligible_task_ids=[],
        eligible_count=0,
        selected_answerable_tasks=0,
        excluded=[],
        status_counts={},
    )
    diagnostic["judgment_metrics"]["file_judgments"]["per_query"] = []
    diagnostic["suite_commitment_sha256"] = evaluator.digest(evaluator.canonical(suite))
    diagnostic["runner_record_sha256"] = evaluator.digest(evaluator.canonical(record))
    diagnostic["selected_eval_tasks"] = 1
    diagnostic["no_answer"] = {
        "task_ids": ["NOC1"],
        "sample_count": 1,
        "abstained": 1,
        "abstention_rate": 1.0,
        "nonempty_results": 0,
        "nonempty_result_rate": 0.0,
        "status_counts": {"abstained": 1},
    }
    output = compose(suite, census, record, diagnostic, lane)
    assert output["content_absence_replay_verified"] is False
    assert output["identifier_osa1_absence_replay_verified"] is False
    assert output["evaluation_intent"] == intent
    assert output["no_answer"]["negative_reference_scope"] == scope
    old_contract = (
        "ascii_content_absent_casefold_v1"
        if policy == "code_search_typo_file"
        else "go_exact_local_name_v3"
    )
    suite["tasks"][0]["source_oracle"]["contract"] = old_contract
    census["lanes"][lane]["contract"] = old_contract
    diagnostic["suite_commitment_sha256"] = evaluator.digest(evaluator.canonical(suite))
    if policy == "code_search_typo_file":
        census["lanes"]["no-answer-content"] = census["lanes"][lane]
        with pytest.raises(ValueError, match="independent ascii_identifier_osa1_absent"):
            compose(suite, census, record, diagnostic, "no-answer-content")
        census["lanes"]["typo-content-absence"] = census["lanes"][lane]
        with pytest.raises(ValueError, match="independent ascii_identifier_osa1_absent"):
            compose(suite, census, record, diagnostic, "typo-content-absence")
        return
    changed = compose(suite, census, record, diagnostic, lane)
    assert changed["content_absence_replay_verified"] is False
    assert changed["no_answer"]["negative_reference_scope"] == "declaration_local_name_absent"


def test_ambiguity_census_must_match_independent_source_oracle(monkeypatch, tmp_path):
    suite = {
        "repository_commit": "a" * 40,
        "file_universe": [{"path": "a.go", "file_sha256": "0" * 64}],
    }
    row = {
        "task_id": "PFX-1",
        "status": "admitted",
        "query": "Alp",
        "matched_names": 1,
        "answer_class": "unique",
        "gold_files": 1,
    }
    census = {"lanes": {"prefix": {"contract": "go_declaration_name_prefix_v1", "records": [row]}}}

    class Snapshot:
        def file(self, _path):
            return b"package p\nfunc Alpha() {}\n", [], "0" * 64

    class Oracle:
        def matched_names(self, _contract, _query):
            return ["Alpha"]

        def expected_rows(self, _contract, _query, _unit):
            return [{"path": "a.go"}]

    monkeypatch.setattr(evaluator, "SourceSnapshot", lambda *_: Snapshot())
    monkeypatch.setattr(
        "tools.benchmark.retrieval.identifier_robustness_report.source_oracle.SourceOracleIndex",
        lambda *_: Oracle(),
    )
    verify_census_against_source(tmp_path, suite, census, "prefix")
    row.update(matched_names=2, answer_class="ambiguous")
    with pytest.raises(ValueError, match="census/source declaration mismatch"):
        verify_census_against_source(tmp_path, suite, census, "prefix")


def test_intended_typo_partition_is_rederived_from_source(monkeypatch, tmp_path):
    suite = {
        "repository_commit": "a" * 40,
        "file_universe": [{"path": "a.go", "file_sha256": "0" * 64}],
    }
    partition = {
        "intended_base_name": "Write",
        "intended_base_files": ["a.go"],
        "near_declaration_names": ["Write"],
        "near_declaration_files": ["a.go"],
        "other_near_declaration_names": [],
        "exact_content_collision_paths": [],
        "query_is_declaration_name": False,
        "user_intent_state": "unjudged",
    }
    row = {
        "task_id": "TYT-1",
        "status": "admitted",
        "query": "Wrtie",
        "intended_name": "Write",
        "matched_names": 1,
        "gold_files": 1,
        "source_partition": partition,
    }
    census = {
        "lanes": {
            "typo-transposition": {
                "gold_kind": "intended_original_name",
                "scoring_contract": source_oracle.GO_EXACT_LOCAL_NAME,
                "near_declaration_metadata_contract": source_oracle.GO_NAME_OSA1_CASEFOLD,
                "records": [row],
            }
        }
    }

    class Snapshot:
        def file(self, _path):
            return b"package p\nfunc Write() {}\n", [], "0" * 64

    class Oracle:
        def typo_gold_partition(self, language, query, intended_name):
            assert (language, query, intended_name) == ("go", "Wrtie", "Write")
            return partition

    monkeypatch.setattr(evaluator, "SourceSnapshot", lambda *_: Snapshot())
    monkeypatch.setattr(
        "tools.benchmark.retrieval.identifier_robustness_report.source_oracle.SourceOracleIndex",
        lambda *_: Oracle(),
    )
    verify_census_against_source(tmp_path, suite, census, "typo-transposition")
    row["source_partition"] = {**partition, "intended_base_files": ["wrong.go"]}
    with pytest.raises(ValueError, match="census/source intended-name partition mismatch"):
        verify_census_against_source(tmp_path, suite, census, "typo-transposition")


def test_generation_manifest_binds_admission_census_and_submitted_suite(tmp_path):
    suite, census, _, _ = fixture()
    suite_path = tmp_path / "prefix-suite.json"
    census_path = tmp_path / "census.json"
    manifest_path = tmp_path / "manifest.json"
    suite_path.write_text(json.dumps(suite))
    census_path.write_text(json.dumps(census))
    manifest = {
        "schema_version": 1,
        "qualification": "diagnostic_unqualified_source_exposed",
        "repository_commit": suite["repository_commit"],
        "artifacts": [
            {"path": name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
            for name, path in (
                ("census.json", census_path),
                ("prefix-suite.json", suite_path),
            )
        ],
    }
    manifest_path.write_text(json.dumps(manifest))
    assert (
        verify_generation_manifest(manifest_path, census_path, suite_path, suite, census, "prefix")
        == hashlib.sha256(manifest_path.read_bytes()).hexdigest()
    )

    census["lanes"]["prefix"]["records"].append(
        {"task_id": "X2", "status": "ineligible", "query": None}
    )
    census_path.write_text(json.dumps(census))
    with pytest.raises(ValueError, match="generation artifact mismatch: census.json"):
        verify_generation_manifest(manifest_path, census_path, suite_path, suite, census, "prefix")
    census_path.write_text(json.dumps(fixture()[1]))
    suite_path.write_text(json.dumps({**suite, "suite_id": "swapped"}))
    with pytest.raises(ValueError, match="generation artifact mismatch: prefix-suite.json"):
        verify_generation_manifest(manifest_path, census_path, suite_path, suite, census, "prefix")


def test_generation_manifest_accepts_only_exact_pair_route_projection(tmp_path):
    from tools.benchmark.retrieval.source_oracle_suite import _json_bytes

    suite, census, _, _ = fixture()
    suite_path = tmp_path / "prefix-suite.json"
    census_path = tmp_path / "census.json"
    manifest_path = tmp_path / "manifest.json"
    census_path.write_bytes(_json_bytes(census))
    source_suite = copy.deepcopy(suite)
    suite_path.write_bytes(_json_bytes(source_suite))
    manifest_path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "qualification": "diagnostic_unqualified_source_exposed",
                "repository_commit": suite["repository_commit"],
                "artifacts": [
                    {
                        "path": "census.json",
                        "sha256": hashlib.sha256(census_path.read_bytes()).hexdigest(),
                    },
                    {
                        "path": "prefix-suite.json",
                        "sha256": hashlib.sha256(suite_path.read_bytes()).hexdigest(),
                    },
                ],
            }
        )
    )
    suite["routes"] = ["lexical", "semble-lexical-file"]
    suite_path.write_bytes(_json_bytes(suite))
    verify_generation_manifest(manifest_path, census_path, suite_path, suite, census, "prefix")
    suite["routes"] = ["lexical", "external-unverified"]
    suite_path.write_bytes(_json_bytes(suite))
    with pytest.raises(ValueError, match="unsupported robustness pair route projection"):
        verify_generation_manifest(manifest_path, census_path, suite_path, suite, census, "prefix")
    suite["routes"] = ["lexical", "semble-lexical-file"]
    suite["suite_id"] = "tampered"
    suite_path.write_bytes(_json_bytes(suite))
    with pytest.raises(ValueError, match="generation artifact mismatch: prefix-suite.json"):
        verify_generation_manifest(manifest_path, census_path, suite_path, suite, census, "prefix")


def test_generation_manifest_accepts_quanta_projection_of_generated_pair(tmp_path):
    from tools.benchmark.retrieval.source_oracle_suite import _json_bytes

    suite, census, _, _ = fixture()
    census.update(generation="paired_full_osa1_casefold_v3", seed=7)
    source_suite = {**suite, "routes": ["lexical", "semble-lexical-file"]}
    source_path = tmp_path / "prefix-suite.json"
    projected_path = tmp_path / "quanta-suite.json"
    census_path = tmp_path / "census.json"
    manifest_path = tmp_path / "manifest.json"
    source_path.write_bytes(_json_bytes(source_suite))
    projected_path.write_bytes(_json_bytes(suite))
    census_path.write_bytes(_json_bytes(census))
    manifest_path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "qualification": "diagnostic_unqualified_source_exposed",
                "repository_commit": suite["repository_commit"],
                "parameters": {"paired_full": True, "seed": 7},
                "artifacts": [
                    {"path": name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
                    for name, path in (
                        ("census.json", census_path),
                        ("prefix-suite.json", source_path),
                    )
                ],
            }
        )
    )
    verify_generation_manifest(
        manifest_path, census_path, projected_path, suite, census, "prefix"
    )
    suite["suite_id"] = "different"
    projected_path.write_bytes(_json_bytes(suite))
    with pytest.raises(ValueError, match="generation artifact mismatch: prefix-suite.json"):
        verify_generation_manifest(
            manifest_path, census_path, projected_path, suite, census, "prefix"
        )


def test_generation_manifest_accepts_only_identical_suite_reserialization(tmp_path):
    from tools.benchmark.retrieval.source_oracle_suite import _json_bytes

    suite, census, _, _ = fixture()
    source_path = tmp_path / "prefix-suite.json"
    submitted_path = tmp_path / "submitted-suite.json"
    census_path = tmp_path / "census.json"
    manifest_path = tmp_path / "manifest.json"
    source_path.write_bytes(_json_bytes(suite))
    submitted_path.write_bytes(evaluator.canonical(suite))
    assert source_path.read_bytes() != submitted_path.read_bytes()
    census_path.write_bytes(_json_bytes(census))
    manifest_path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "qualification": "diagnostic_unqualified_source_exposed",
                "repository_commit": suite["repository_commit"],
                "artifacts": [
                    {"path": name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
                    for name, path in (
                        ("census.json", census_path),
                        ("prefix-suite.json", source_path),
                    )
                ],
            }
        )
    )
    verify_generation_manifest(
        manifest_path, census_path, submitted_path, suite, census, "prefix"
    )
    suite["tasks"][0]["query"] = "tampered"
    submitted_path.write_bytes(evaluator.canonical(suite))
    with pytest.raises(ValueError, match="generation artifact mismatch: prefix-suite.json"):
        verify_generation_manifest(
            manifest_path, census_path, submitted_path, suite, census, "prefix"
        )


def test_content_no_answer_population_requires_frozen_generation_count(tmp_path):
    suite, census, _, _ = fixture()
    census["lanes"]["no-answer"] = {"records": [{"task_id": "NOA1"}]}
    suite_path = tmp_path / "no-answer-content-suite.json"
    census_path = tmp_path / "census.json"
    manifest_path = tmp_path / "manifest.json"
    suite_path.write_text(json.dumps(suite))
    census_path.write_text(json.dumps(census))
    manifest_path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "qualification": "diagnostic_unqualified_source_exposed",
                "repository_commit": suite["repository_commit"],
                "parameters": {"no_answer": 1},
                "artifacts": [
                    {"path": name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
                    for name, path in (
                        ("census.json", census_path),
                        ("no-answer-content-suite.json", suite_path),
                    )
                ],
            }
        )
    )
    verify_generation_manifest(
        manifest_path, census_path, suite_path, suite, census, "no-answer-content"
    )
    census["lanes"]["no-answer"]["records"].append({"task_id": "NOA2"})
    with pytest.raises(ValueError, match="content no-answer source population mismatch"):
        verify_generation_manifest(
            manifest_path, census_path, suite_path, suite, census, "no-answer-content"
        )


def test_identifier_osa1_generation_binds_frozen_source_and_target_bytes(monkeypatch, tmp_path):
    source = {
        "suite_id": "frozen-no-answer-content-v2-seed7",
        "repository_commit": "a" * 40,
        "routes": ["lexical", "semble-lexical-file"],
        "tasks": [
            {
                "task_id": "NOC-001",
                "query": "missingName",
                "split": "eval",
                "answerable": False,
                "file_judgments": [],
                "source_oracle": {
                    "contract": "ascii_content_absent_casefold_v1",
                    "unit": "distinct_file",
                },
            }
        ],
    }

    def validate(_repo, suite):
        return (
            suite,
            {"suite_commitment_sha256": evaluator.digest(evaluator.canonical(suite))},
            None,
        )

    monkeypatch.setattr(evaluator, "validate_suite", validate)
    source_path = tmp_path / "source.json"
    source_path.write_text(json.dumps(source))
    target = copy.deepcopy(source)
    target["suite_id"] += "-identifier-osa1-absence-v1"
    target["routes"] = ["lexical"]
    target["tasks"][0]["source_oracle"]["contract"] = "ascii_identifier_osa1_absent_casefold_v1"
    target_pack = {"suite_commitment_sha256": evaluator.digest(evaluator.canonical(target))}
    target_path = tmp_path / "target.json"
    pack_path = tmp_path / "target-pack.json"
    target_path.write_text(json.dumps(target, indent=2))
    pack_path.write_text(json.dumps(target_pack, indent=2))
    corpus = tmp_path / "corpus"
    corpus.mkdir()
    root = tmp_path / "generated"
    identifier_osa1_absence_suite.write(corpus, source_path, root, target_path, pack_path)
    suite_path = root / "typo-osa1-absence-suite.json"
    census_path = root / "census.json"
    manifest_path = root / "manifest.json"
    assert suite_path.read_bytes() == target_path.read_bytes()
    assert (root / "typo-osa1-absence-blind-pack.json").read_bytes() == pack_path.read_bytes()
    census = evaluator.read_json(census_path)
    verify_generation_manifest(
        manifest_path, census_path, suite_path, target, census, "typo-osa1-absence"
    )
    wrong_route = copy.deepcopy(target)
    wrong_route["routes"].append("semble-lexical-file")
    with pytest.raises(ValueError, match="target suite is not the frozen source population"):
        verify_generation_manifest(
            manifest_path, census_path, suite_path, wrong_route, census, "typo-osa1-absence"
        )
    (root / "source-no-answer-content-suite.json").write_text("{}")
    with pytest.raises(ValueError, match="generation source suite mismatch"):
        verify_generation_manifest(
            manifest_path, census_path, suite_path, target, census, "typo-osa1-absence"
        )
    bad_target = copy.deepcopy(target)
    bad_target["tasks"][0]["query"] = "drifted"
    bad_path = tmp_path / "bad-target.json"
    bad_path.write_text(json.dumps(bad_target))
    with pytest.raises(ValueError, match="frozen target suite differs"):
        identifier_osa1_absence_suite.write(
            corpus, source_path, tmp_path / "rejected", bad_path, pack_path
        )
    assert not (tmp_path / "rejected").exists()
