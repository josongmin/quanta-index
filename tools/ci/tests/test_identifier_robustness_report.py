"""Independent fixture for the robustness report's admission and join contract."""

from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path

import pytest

from tools.benchmark.retrieval import evaluator
from tools.benchmark.retrieval.identifier_robustness_report import (
    _write_output,
    compose,
    compose_validated,
    verify_census_against_source,
    verify_generation_manifest,
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


def test_code_search_file_reports_scored_family_separately():
    output = compose(*fixture("code_search_file"), "prefix")
    assert output["policy"] == "code_search_file"
    assert output["case"] == "folded"
    assert output["scope"] == "content_and_path"
    assert output["ordering"] == "score_desc_path_tiebreak"
    assert output["lane"] == "prefix"
    assert output["status"] == "diagnostic_unqualified"


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


def test_new_content_absence_contract_is_separate_from_legacy():
    suite, census, record, diagnostic = fixture()
    contract = "ascii_content_absent_casefold_v1"
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
    census["lanes"] = {
        "no-answer": {
            "records": [{"task_id": "NOA1", "query": "absentName", "status": "admitted"}]
        },
        "no-answer-content": {
            "contract": contract,
            "derived_from": "no-answer",
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
    output = compose(suite, census, record, diagnostic, "no-answer-content")
    assert output["content_absence_replay_verified"] is False
    assert output["evaluation_intent"] == "content_absence_negative_control"
    assert output["no_answer"]["negative_reference_scope"] == "folded_content_absent"
    suite["tasks"][0]["source_oracle"]["contract"] = "go_exact_local_name_v3"
    census["lanes"]["no-answer-content"]["contract"] = "go_exact_local_name_v3"
    diagnostic["suite_commitment_sha256"] = evaluator.digest(evaluator.canonical(suite))
    changed = compose(suite, census, record, diagnostic, "no-answer-content")
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
