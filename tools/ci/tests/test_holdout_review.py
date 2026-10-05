"""Blind review preparation must preserve source and never invent decisions."""

from __future__ import annotations

import copy
import json
import subprocess
from pathlib import Path

import pytest

from tools.benchmark.retrieval import evaluator, execution_batch, holdout_review, query_plan


def _fixture(tmp_path: Path):
    checkout = tmp_path / "source"
    checkout.mkdir()
    for path, text in (
        ("answer.py", "def alpha():\n    return '한글'\n"),
        ("alternative.py", "def beta():\n    pass\n"),
        ("control.py", "# arbitrary source control; no assumed grade\n"),
    ):
        (checkout / path).write_text(text, encoding="utf-8")
    subprocess.run(["git", "init", "-q", str(checkout)], check=True)
    subprocess.run(["git", "-C", str(checkout), "add", "."], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(checkout),
            "-c",
            "user.name=Test",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-qm",
            "fixture",
        ],
        check=True,
    )
    commit = (
        subprocess.check_output(["git", "-C", str(checkout), "rev-parse", "HEAD"]).decode().strip()
    )
    universe = [
        {"path": p.name, "file_sha256": evaluator.digest(p.read_bytes())}
        for p in sorted(checkout.glob("*.py"))
    ]
    query = "Find alpha behavior"
    pack = {
        "schema_version": 3,
        "suite_id": "fixed-review-test",
        "suite_commitment_sha256": "a" * 64,
        "repository_commit": commit,
        "tokenizer": evaluator.TOKENIZER,
        "tokenizer_budget_version": evaluator.TOKENIZER_BUDGET_VERSION,
        "routes": ["lexical", "semble-lexical-file"],
        "file_universe": universe,
        "file_universe_digest": evaluator.universe_digest(universe),
        "comparison_contract": {
            "top_k": 10,
            "tokenizer": evaluator.TOKENIZER,
            "tokenizer_budget_version": evaluator.TOKENIZER_BUDGET_VERSION,
            "output_unit_policy": "rank_prefix",
            "span_unit": evaluator.SPAN_UNIT,
        },
        "tasks": [
            {"task_id": "toy.001", "query": query, "query_sha256": evaluator.digest(query.encode())}
        ],
    }
    contexts = {
        "toy.001": {
            "intent": "file relevance",
            "provenance": "authored source task",
            "rubric": "0 irrelevant; 1 useful; 2 relevant; 3 directly answers",
        }
    }
    by_path = {row["path"]: copy.deepcopy(row) for row in universe}
    pools = [
        {
            "pool_id": "quanta-product",
            "kind": "retrieval",
            "tasks": {"toy.001": [by_path["answer.py"]]},
        },
        {
            "pool_id": "semble-product",
            "kind": "retrieval",
            "tasks": {"toy.001": [by_path["answer.py"]]},
        },
        {
            "pool_id": "source-derived",
            "kind": "source_alternative",
            "tasks": {"toy.001": [by_path["alternative.py"]]},
        },
        {
            "pool_id": "seeded-control",
            "kind": "random_control",
            "tasks": {"toy.001": [by_path["control.py"]]},
        },
    ]
    return checkout, pack, contexts, pools


def _completed_file_review_fixture(tmp_path):
    checkout, pack, contexts, pools = _fixture(tmp_path)
    contexts["toy.001"]["answerability_min_grade"] = 2
    forms, _ = holdout_review.prepare(checkout, pack, contexts, pools, seed=42)
    for index, form in enumerate(forms):
        form["reviewer_id"] = f"ai:fixture-reviewer-{index}"
        row = form["reviews"][0]
        row.update(answerable=True, rationale="The alpha definition answers this source fixture.")
        for file in row["files"]:
            file.update(
                grade=3 if file["path"] == "answer.py" else 0, rationale="Fixed source fixture."
            )
    adjudicated = copy.deepcopy(forms[0])
    adjudicated["reviewer_id"] = "ai:fixture-adjudicator"
    contracts = {
        "toy.001": {
            "request_mode": query_plan.NATURAL_LANGUAGE_FILE_SEARCH,
            "gold_unit": "distinct_file",
            "result_unit": "distinct_file",
        }
    }
    return checkout, pack, contexts, pools, forms, adjudicated, contracts


@pytest.mark.parametrize("answerable", [False, True])
def test_file_review_issuer_preserves_sufficient_answer_threshold(tmp_path, answerable):
    checkout, pack, contexts, pools, forms, adjudicated, contracts = _completed_file_review_fixture(
        tmp_path
    )
    for form in [*forms, adjudicated]:
        form["reviews"][0]["answerable"] = answerable
        for file in form["reviews"][0]["files"]:
            if file["path"] == "answer.py":
                file["grade"] = 2 if answerable else 1
    before = copy.deepcopy((pack, contexts, pools, forms, adjudicated, contracts))
    result = holdout_review.finalize_file_review_labels(
        checkout, pack, contexts, pools, forms, adjudicated, contracts, seed=42
    )
    labels = result["task_labels"]["toy.001"]
    assert labels["answerability_min_grade"] == 2
    assert labels["answerable"] is answerable
    assert labels["evaluation_contract"] == contracts["toy.001"]
    assert next(file for file in labels["file_judgments"] if file["path"] == "answer.py")[
        "grade"
    ] == (2 if answerable else 1)
    if answerable:
        raw = (checkout / "answer.py").read_bytes()
        assert labels["gold"] == [
            {
                "path": "answer.py",
                "file_sha256": evaluator.digest(raw),
                "grade": 2,
                "block_sha256": evaluator.digest(raw),
                "start_byte": 0,
                "end_byte": len(raw),
                "start_line": 1,
                "end_line": 2,
            }
        ]
    else:
        assert labels["gold"] == []
    suite = {
        key: copy.deepcopy(pack[key])
        for key in (
            "schema_version",
            "suite_id",
            "repository_commit",
            "routes",
            "comparison_contract",
            "file_universe",
            "file_universe_digest",
        )
    }
    suite["tasks"] = [
        {
            **pack["tasks"][0],
            "query_family_id": "toy.001",
            "split": "eval",
            "category": "nl_review_fixture",
            **labels,
        }
    ]
    suite["diagnostic_policy"] = evaluator.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
    _checked, issued_pack, _tokens = evaluator.validate_suite(checkout, suite)
    assert issued_pack["suite_commitment_sha256"] != pack["suite_commitment_sha256"]
    assert (
        result["qualified"]
        is result["human_provenance_attested"]
        is result["pool_execution_attested"]
        is False
    )
    assert (pack, contexts, pools, forms, adjudicated, contracts) == before


@pytest.mark.parametrize(
    "fault",
    [
        "missing_review",
        "same_adjudicator",
        "missing_decision",
        "changed_threshold",
        "changed_hash",
        "changed_source",
        "unjudged",
        "unsupported_answer",
        "wrong_unit",
        "missing_contract",
        "wrong_mode",
        "same_reviewer",
        "original_unsupported_answer",
    ],
)
def test_file_review_issuer_rejects_incomplete_or_unbound_decisions(tmp_path, fault):
    checkout, pack, contexts, pools, forms, adjudicated, contracts = _completed_file_review_fixture(
        tmp_path
    )
    if fault == "missing_review":
        forms.pop()
    elif fault == "same_adjudicator":
        adjudicated["reviewer_id"] = forms[0]["reviewer_id"]
    elif fault == "missing_decision":
        adjudicated["reviews"] = []
    elif fault == "changed_threshold":
        adjudicated["reviews"][0]["answerability_min_grade"] = 1
    elif fault == "changed_hash":
        adjudicated["reviews"][0]["files"][0]["file_sha256"] = "f" * 64
    elif fault == "changed_source":
        adjudicated["reviews"][0]["files"][0]["source_text"] += "unbound source"
    elif fault == "unjudged":
        adjudicated["reviews"][0]["files"][0]["grade"] = None
    elif fault == "unsupported_answer":
        for file in adjudicated["reviews"][0]["files"]:
            file["grade"] = 1
    elif fault == "wrong_unit":
        contracts["toy.001"].update(gold_unit="symbol", result_unit="symbol")
    elif fault == "missing_contract":
        contracts = {}
    elif fault == "wrong_mode":
        contracts["toy.001"]["request_mode"] = query_plan.DEFAULT_FILE_SEARCH
    elif fault == "same_reviewer":
        forms[1]["reviewer_id"] = forms[0]["reviewer_id"]
    elif fault == "original_unsupported_answer":
        for file in forms[0]["reviews"][0]["files"]:
            file["grade"] = 1
    expected = {
        "missing_review": "two review forms required",
        "same_adjudicator": "adjudicator must differ",
        "missing_decision": "review task coverage changed",
        "changed_threshold": "review query/context changed",
        "changed_hash": "review source/candidate changed",
        "changed_source": "review source/candidate changed",
        "unjudged": "review grade must be 0..3",
        "unsupported_answer": "adjudicated answerability requires a sufficient pooled file",
        "wrong_unit": "evaluation_contract unit mismatch",
        "missing_contract": "file review evaluation contract coverage differs",
        "wrong_mode": "file review issuer requires the NL file search contract",
        "same_reviewer": "two distinct reviewer identities required",
        "original_unsupported_answer": "review answerability requires a sufficient pooled file",
    }
    with pytest.raises(evaluator.EvidenceError, match=expected[fault]):
        holdout_review.finalize_file_review_labels(
            checkout, pack, contexts, pools, forms, adjudicated, contracts, seed=42
        )


def test_file_review_issuer_marks_an_adjudicated_override_ambiguous(tmp_path):
    checkout, pack, contexts, pools, forms, adjudicated, contracts = _completed_file_review_fixture(
        tmp_path
    )
    for file in adjudicated["reviews"][0]["files"]:
        if file["path"] == "answer.py":
            file["grade"] = 2
    result = holdout_review.finalize_file_review_labels(
        checkout, pack, contexts, pools, forms, adjudicated, contracts, seed=42
    )
    assert result["task_labels"]["toy.001"]["label_review"]["assessment"] == "reviewed_ambiguous"
    assert result["task_labels"]["toy.001"]["gold"][0]["grade"] == 2


def test_prepare_deduplicates_blinds_and_retains_unjudged(tmp_path):
    checkout, pack, contexts, pools = _fixture(tmp_path)
    forms, custody = holdout_review.prepare(checkout, pack, contexts, pools, seed=42)
    assert len(forms) == 2
    assert custody["qualified"] is custody["pool_execution_attested"] is False
    for form in forms:
        assert form["reviewer_id"] is None
        task = form["reviews"][0]
        assert task["answerable"] is None
        assert len(task["files"]) == 3
        assert all(file["grade"] is None and file["rationale"] == "" for file in task["files"])
        assert {file["path"] for file in task["files"]} == {
            row["path"] for row in pack["file_universe"]
        }
        assert (
            "한글"
            in next(row for row in task["files"] if row["path"] == "answer.py")["source_text"]
        )
        serialized = json.dumps(form)
        assert not any(
            name in serialized for name in ("quanta", "semble", "source-derived", "seeded-control")
        )
        assert not any(key in serialized for key in ('"score"', '"rank"', '"gold"', '"labels"'))
    assert custody["membership"]["toy.001"]["answer.py"] == ["quanta-product", "semble-product"]
    assert holdout_review.prepare(checkout, pack, contexts, pools, seed=42) == (forms, custody)


def test_reviewer_order_does_not_reveal_pool_input_order(tmp_path):
    checkout, pack, contexts, pools = _fixture(tmp_path)
    first, _ = holdout_review.prepare(checkout, pack, contexts, pools, seed=17)
    second, _ = holdout_review.prepare(checkout, pack, contexts, list(reversed(pools)), seed=17)
    assert first == second


@pytest.mark.parametrize(
    "fault",
    [
        "query_hash",
        "universe",
        "file_hash",
        "unknown_file",
        "labels",
        "rank",
        "review_claim",
        "missing_context",
        "missing_pool",
        "empty_control",
        "missing_task",
        "duplicate_task",
        "duplicate_pool",
        "unknown_kind",
    ],
)
def test_preparation_refuses_invalid_or_contaminated_inputs(tmp_path, fault):
    checkout, pack, contexts, pools = _fixture(tmp_path)
    if fault == "query_hash":
        pack["tasks"][0]["query_sha256"] = "0" * 64
    elif fault == "universe":
        pack["file_universe_digest"] = "0" * 64
    elif fault == "file_hash":
        pools[0]["tasks"]["toy.001"][0]["file_sha256"] = "0" * 64
    elif fault == "unknown_file":
        pools[0]["tasks"]["toy.001"][0] = {"path": "../escape.py", "file_sha256": "0" * 64}
    elif fault == "labels":
        pack["tasks"][0]["gold"] = []
    elif fault == "rank":
        pools[0]["tasks"]["toy.001"][0]["rank"] = 1
    elif fault == "review_claim":
        contexts["toy.001"]["reviewer_id"] = "automated-reviewer"
    elif fault == "missing_context":
        contexts.clear()
    elif fault == "missing_pool":
        pools.pop(0)
    elif fault == "empty_control":
        pools[-1]["tasks"]["toy.001"] = []
    elif fault == "missing_task":
        pools[0]["tasks"] = {}
    elif fault == "duplicate_task":
        pack["tasks"].append(copy.deepcopy(pack["tasks"][0]))
    elif fault == "duplicate_pool":
        pools.append(copy.deepcopy(pools[0]))
    else:
        pools[0]["kind"] = "human_approved"
    with pytest.raises(evaluator.EvidenceError):
        holdout_review.prepare(checkout, pack, contexts, pools, seed=42)


def test_changed_source_is_refused(tmp_path):
    checkout, pack, contexts, pools = _fixture(tmp_path)
    (checkout / "answer.py").write_text("changed\n")
    with pytest.raises(evaluator.EvidenceError, match="checkout has tracked or untracked changes"):
        holdout_review.prepare(checkout, pack, contexts, pools, seed=42)


def test_export_bound_is_not_silent_truncation(tmp_path, monkeypatch):
    checkout, pack, contexts, pools = _fixture(tmp_path)
    monkeypatch.setattr(holdout_review, "MAX_REVIEW_BYTES", 1)
    with pytest.raises(evaluator.EvidenceError, match="export byte limit"):
        holdout_review.prepare(checkout, pack, contexts, pools, seed=42)


def test_write_external_unjudged_forms_and_private_custody(tmp_path):
    checkout, pack, contexts, pools = _fixture(tmp_path)
    output = tmp_path / "review"
    custody = holdout_review.write(checkout, pack, contexts, pools, output, seed=42)
    assert len(list(output.glob("reviewer-*.json"))) == 2
    assert (output / "owner").stat().st_mode & 0o777 == 0o700
    assert json.loads((output / "owner" / "custody.json").read_text()) == custody
    with pytest.raises(ValueError, match="fresh"):
        holdout_review.write(checkout, pack, contexts, pools, output, seed=42)
    with pytest.raises(ValueError, match="external"):
        holdout_review.write(checkout, pack, contexts, pools, checkout / "review", seed=42)


def test_write_cleans_partial_output(tmp_path, monkeypatch):
    checkout, pack, contexts, pools = _fixture(tmp_path)
    output = tmp_path / "review"
    real_write = Path.write_bytes

    def fail_second(path, raw):
        if path.name == "reviewer-2.json":
            raise OSError("injected failure")
        return real_write(path, raw)

    monkeypatch.setattr(Path, "write_bytes", fail_second)
    with pytest.raises(OSError, match="injected"):
        holdout_review.write(checkout, pack, contexts, pools, output, seed=42)
    assert not output.exists()


def test_preparation_forms_cannot_be_qualification_receipts(tmp_path):
    from tools.benchmark.retrieval import run

    checkout, pack, contexts, pools = _fixture(tmp_path)
    forms, _ = holdout_review.prepare(checkout, pack, contexts, pools, seed=42)
    with pytest.raises(run.RunError, match="receipt must hold exactly"):
        run._validate_gold_review_receipt(
            forms[0],
            role="annotation",
            reviewer_id="human",
            suite_sha256=pack["suite_commitment_sha256"],
            suite={"tasks": []},
            repo=checkout,
        )


def _completed_forms(forms):
    completed = copy.deepcopy(forms)
    for slot, form in enumerate(completed, 1):
        form["reviewer_id"] = f"person-{slot}"
        for task in form["reviews"]:
            task["answerable"] = True
            task["rationale"] = "Reviewed the complete source checkout."
            for file_row in task["files"]:
                file_row["grade"] = 1 if file_row["path"] == "answer.py" else 0
                file_row["rationale"] = "Source-backed file assessment."
    return completed


def test_completed_forms_validate_without_claiming_adjudication(tmp_path):
    checkout, pack, contexts, pools = _fixture(tmp_path)
    forms, _ = holdout_review.prepare(checkout, pack, contexts, pools, seed=42)
    completed = _completed_forms(forms)
    result = holdout_review.validate_completed_forms(
        checkout, pack, contexts, pools, completed, seed=42
    )
    assert result["task_count"] == 1
    assert result["disagreements"] == []
    assert result["status"] == "completed_forms_validated_unqualified"
    assert result["qualified"] is result["human_provenance_attested"] is False
    assert result["pool_execution_attested"] is False
    assert result["completed_form_sha256"] == [
        evaluator.digest(evaluator.canonical(form)) for form in completed
    ]
    for form in completed:
        assert form["status"] == "unjudged_preparation"


@pytest.mark.parametrize(
    ("field", "replacement", "message"),
    [
        ("form_slot", True, "review form source binding changed: form_slot"),
        ("schema_version", 1.0, "review form source binding changed: schema_version"),
        ("answerability_min_grade", True, "review query/context changed: answerability_min_grade"),
    ],
)
def test_completed_forms_refuse_typed_aliases_in_frozen_fields(
    tmp_path, field, replacement, message
):
    checkout, pack, contexts, pools = _fixture(tmp_path)
    forms, _ = holdout_review.prepare(checkout, pack, contexts, pools, seed=42)
    completed = _completed_forms(forms)
    if field == "answerability_min_grade":
        completed[0]["reviews"][0][field] = replacement
    else:
        completed[0][field] = replacement
    with pytest.raises(evaluator.EvidenceError, match=message):
        holdout_review.validate_completed_forms(checkout, pack, contexts, pools, completed, seed=42)


def test_completed_forms_report_disagreements_without_resolving_them(tmp_path):
    checkout, pack, contexts, pools = _fixture(tmp_path)
    forms, _ = holdout_review.prepare(checkout, pack, contexts, pools, seed=42)
    completed = _completed_forms(forms)
    completed[1]["reviews"][0]["answerable"] = False
    for file_row in completed[1]["reviews"][0]["files"]:
        file_row["grade"] = 0
    result = holdout_review.validate_completed_forms(
        checkout, pack, contexts, pools, completed, seed=42
    )
    assert result["disagreements"] == [
        {
            "task_id": "toy.001",
            "query_sha256": pack["tasks"][0]["query_sha256"],
            "answerable": [True, False],
            "files": [
                {
                    "path": "answer.py",
                    "file_sha256": next(
                        row["file_sha256"]
                        for row in pack["file_universe"]
                        if row["path"] == "answer.py"
                    ),
                    "grades": [1, 0],
                }
            ],
        }
    ]


def test_answerable_may_be_outside_the_pooled_candidates(tmp_path):
    checkout, pack, contexts, pools = _fixture(tmp_path)
    forms, _ = holdout_review.prepare(checkout, pack, contexts, pools, seed=42)
    completed = _completed_forms(forms)
    for form in completed:
        for file_row in form["reviews"][0]["files"]:
            file_row["grade"] = 0
    result = holdout_review.validate_completed_forms(
        checkout, pack, contexts, pools, completed, seed=42
    )
    assert result["disagreements"] == []
    assert result["qualified"] is False


def test_partial_clue_does_not_force_answerability_at_sufficient_answer_threshold(tmp_path):
    checkout, pack, contexts, pools = _fixture(tmp_path)
    contexts["toy.001"]["answerability_min_grade"] = 2
    forms, _ = holdout_review.prepare(checkout, pack, contexts, pools, seed=42)
    completed = _completed_forms(forms)
    for form in completed:
        form["reviews"][0]["answerable"] = False
    validated = holdout_review.validate_completed_forms(
        checkout, pack, contexts, pools, completed, seed=42
    )
    assert validated["disagreements"] == []
    assert completed[0]["reviews"][0]["answerability_min_grade"] == 2
    assert any(row["grade"] == 1 for row in completed[0]["reviews"][0]["files"])
    completed[0]["reviews"][0]["files"][0]["grade"] = 2
    with pytest.raises(evaluator.EvidenceError, match="sufficient-answer grade"):
        holdout_review.validate_completed_forms(checkout, pack, contexts, pools, completed, seed=42)


@pytest.mark.parametrize("threshold", [0, 4, True, "2", 2.0])
def test_review_refuses_malformed_answerability_threshold(tmp_path, threshold):
    checkout, pack, contexts, pools = _fixture(tmp_path)
    contexts["toy.001"]["answerability_min_grade"] = threshold
    with pytest.raises(evaluator.EvidenceError, match="answerability_min_grade"):
        holdout_review.prepare(checkout, pack, contexts, pools, seed=42)


def test_review_threshold_is_frozen_with_context(tmp_path):
    checkout, pack, contexts, pools = _fixture(tmp_path)
    contexts["toy.001"]["answerability_min_grade"] = 2
    forms, _ = holdout_review.prepare(checkout, pack, contexts, pools, seed=42)
    completed = _completed_forms(forms)
    completed[0]["reviews"][0]["answerability_min_grade"] = 1
    with pytest.raises(evaluator.EvidenceError, match="query/context changed"):
        holdout_review.validate_completed_forms(checkout, pack, contexts, pools, completed, seed=42)


@pytest.mark.parametrize(
    "fault",
    [
        "same_reviewer",
        "missing_reviewer",
        "missing_task_rationale",
        "missing_file_rationale",
        "missing_grade",
        "boolean_grade",
        "invalid_grade",
        "denied_answer_with_positive_grade",
        "changed_source",
        "changed_context",
        "changed_candidate_order",
        "changed_query",
        "changed_slot",
        "partial_candidates",
    ],
)
def test_completed_forms_refuse_missing_decisions_or_source_drift(tmp_path, fault):
    checkout, pack, contexts, pools = _fixture(tmp_path)
    forms, _ = holdout_review.prepare(checkout, pack, contexts, pools, seed=42)
    completed = _completed_forms(forms)
    task = completed[0]["reviews"][0]
    if fault == "same_reviewer":
        completed[1]["reviewer_id"] = completed[0]["reviewer_id"]
    elif fault == "missing_reviewer":
        completed[0]["reviewer_id"] = None
    elif fault == "missing_task_rationale":
        task["rationale"] = ""
    elif fault == "missing_file_rationale":
        task["files"][0]["rationale"] = ""
    elif fault == "missing_grade":
        task["files"][0]["grade"] = None
    elif fault == "boolean_grade":
        task["files"][0]["grade"] = True
    elif fault == "invalid_grade":
        task["files"][0]["grade"] = 4
    elif fault == "denied_answer_with_positive_grade":
        task["answerable"] = False
    elif fault == "changed_source":
        task["files"][0]["source_text"] = "fabricated"
    elif fault == "changed_context":
        task["rubric"] = "different rubric"
    elif fault == "changed_candidate_order":
        task["files"].reverse()
    elif fault == "changed_query":
        task["query"] = "different query"
    elif fault == "changed_slot":
        completed[0]["form_slot"] = 2
    else:
        task["files"].pop()
    with pytest.raises(evaluator.EvidenceError):
        holdout_review.validate_completed_forms(checkout, pack, contexts, pools, completed, seed=42)


def _reviewed_suite_fixture(tmp_path):
    checkout, pack, _contexts, _pools = _fixture(tmp_path)
    suite = {
        key: copy.deepcopy(pack[key])
        for key in (
            "schema_version",
            "suite_id",
            "repository_commit",
            "comparison_contract",
            "routes",
            "file_universe",
            "file_universe_digest",
        )
    }
    suite["diagnostic_policy"] = evaluator.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
    tasks = []
    for task_id, query, intent, path in (
        ("toy.nl", "Find alpha behavior", "semantic_intent", "answer.py"),
        ("toy.name", "beta", "bare_symbol", "alternative.py"),
    ):
        raw = (checkout / path).read_bytes()
        sha = evaluator.digest(raw)
        tasks.append(
            {
                "task_id": task_id,
                "split": "eval",
                "query": query,
                "query_sha256": evaluator.digest(query.encode()),
                "query_family_id": task_id,
                "query_intent": intent,
                "answerable": True,
                "evaluation_contract": {
                    "request_mode": "default_file_search",
                    "gold_unit": "distinct_file",
                    "result_unit": "distinct_file",
                },
                "judgment_policy": evaluator.COMPLETE_JUDGMENT_POLICY,
                "label_review": {
                    "assessment": "reviewed_unambiguous",
                    "reviewer_id": "ai:fixture",
                    "evidence_sha256": "e" * 64,
                },
                "file_judgments": [{"path": path, "file_sha256": sha, "grade": 3}],
                "gold": [
                    {
                        "path": path,
                        "file_sha256": sha,
                        "block_sha256": sha,
                        "start_line": 1,
                        "end_line": len(raw.splitlines()),
                        "start_byte": 0,
                        "end_byte": len(raw),
                        "grade": 3,
                    }
                ],
            }
        )
    suite["tasks"] = tasks
    return checkout, suite


def test_nl_projection_preserves_labels_and_blinds_only_selected_queries(tmp_path):
    checkout, original = _reviewed_suite_fixture(tmp_path)
    raw = evaluator.canonical(original)
    projected, pack, lineage = holdout_review.project_natural_language_file_diagnostic(
        checkout, raw, suite_id="new-nl-file-diagnostic"
    )
    expected = copy.deepcopy(original["tasks"][0])
    expected["evaluation_contract"] = {
        "request_mode": query_plan.NATURAL_LANGUAGE_FILE_SEARCH,
        "gold_unit": "distinct_file",
        "result_unit": "distinct_file",
    }
    assert projected["tasks"] == [expected]
    assert evaluator.canonical(original) == raw
    assert pack["tasks"] == [{key: expected[key] for key in ("task_id", "query", "query_sha256")}]
    assert lineage["selected_task_ids"] == ["toy.nl"]
    assert lineage["excluded_task_ids"] == ["toy.name"]
    assert lineage["input_suite_bytes_sha256"] == evaluator.digest(raw)
    assert lineage["suite_sha256"] == evaluator.digest(evaluator.canonical(projected))
    assert lineage["qualified"] is False
    assert lineage["human_provenance_attested"] is False
    assert lineage["review_receipts"] == "remain_bound_to_original_suite"
    assert lineage["split_admission"] == "not_carried_forward"


def test_suite_distinguishes_partial_relevance_from_answerability(tmp_path):
    checkout, original = _reviewed_suite_fixture(tmp_path)
    task = original["tasks"][0]
    task["answerability_min_grade"] = 2
    evaluator.validate_suite(checkout, original)
    task["answerable"] = False
    task["gold"] = []
    task["file_judgments"][0]["grade"] = 1
    checked, pack, _source = evaluator.validate_suite(checkout, original)
    assert checked["tasks"][0]["file_judgments"][0]["grade"] == 1
    assert "answerability_min_grade" not in pack["tasks"][0]
    assert (
        evaluator.file_ndcg_at_k(
            [{"path": task["file_judgments"][0]["path"]}], task["file_judgments"], 10
        )
        == 1.0
    )
    task["file_judgments"][0]["grade"] = 2
    with pytest.raises(evaluator.EvidenceError, match="answerability mismatch"):
        evaluator.validate_suite(checkout, original)


def test_suite_refuses_insufficient_answer_and_gold_grades(tmp_path):
    checkout, original = _reviewed_suite_fixture(tmp_path)
    task = original["tasks"][0]
    task["answerability_min_grade"] = 2
    task["file_judgments"][0]["grade"] = 1
    with pytest.raises(evaluator.EvidenceError, match="lacks a positive judgment"):
        evaluator.validate_suite(checkout, original)
    task["file_judgments"][0]["grade"] = 3
    task["gold"][0]["grade"] = 1
    with pytest.raises(evaluator.EvidenceError, match="gold grade below"):
        evaluator.validate_suite(checkout, original)


@pytest.mark.parametrize("threshold", [0, 4, True, "2", 2.0])
def test_suite_refuses_invalid_answerability_threshold(tmp_path, threshold):
    checkout, original = _reviewed_suite_fixture(tmp_path)
    original["tasks"][0]["answerability_min_grade"] = threshold
    with pytest.raises(evaluator.EvidenceError, match="answerability_min_grade"):
        evaluator.validate_suite(checkout, original)


@pytest.mark.parametrize("fault", ["orphan", "source_oracle"])
def test_suite_refuses_orphan_or_oracle_answerability_threshold(tmp_path, fault):
    checkout, original = _reviewed_suite_fixture(tmp_path)
    task = original["tasks"][1]
    task["answerability_min_grade"] = 2
    if fault == "orphan":
        del task["file_judgments"]
    else:
        task["source_oracle"] = {"contract": "ascii_identifier_word_v1", "unit": "distinct_file"}
    with pytest.raises(evaluator.EvidenceError, match="requires independent judgments"):
        evaluator.validate_judgments(
            evaluator.SourceSnapshot(checkout, original["repository_commit"]),
            task,
            {row["path"] for row in original["file_universe"]},
            task["task_id"],
        )


@pytest.mark.parametrize(
    ("fault", "message"),
    [
        ("no_nl", "no semantic_intent tasks"),
        ("unreviewed", "require reviewed label evidence"),
        ("partial_grade", "grade must be an integer 0-3"),
        ("wrong_hash", "file hash mismatch"),
        ("bad_excluded_gold", "block hash mismatch"),
        ("over_limit", "33 tokens"),
        ("same_id", "requires a new suite ID"),
    ],
)
def test_nl_projection_refuses_invalid_input_without_silently_dropping_tasks(
    tmp_path, fault, message
):
    checkout, suite = _reviewed_suite_fixture(tmp_path)
    suite_id = "new-nl-file-diagnostic"
    task = suite["tasks"][0]
    if fault == "no_nl":
        task["query_intent"] = "bare_symbol"
    elif fault == "unreviewed":
        task["label_review"] = {"assessment": "unreviewed"}
    elif fault == "partial_grade":
        task["file_judgments"][0]["grade"] = None
    elif fault == "wrong_hash":
        task["file_judgments"][0]["file_sha256"] = "f" * 64
    elif fault == "bad_excluded_gold":
        suite["tasks"][1]["gold"][0]["block_sha256"] = "f" * 64
    elif fault == "over_limit":
        task["query"] = " ".join(f"word{i}" for i in range(33))
        task["query_sha256"] = evaluator.digest(task["query"].encode())
    else:
        suite_id = suite["suite_id"]
    with pytest.raises((evaluator.EvidenceError, query_plan.QueryPlanError), match=message):
        holdout_review.project_natural_language_file_diagnostic(
            checkout, evaluator.canonical(suite), suite_id=suite_id
        )


def test_nl_projection_writer_refuses_overwrite_and_input_race(tmp_path, monkeypatch):
    checkout, suite = _reviewed_suite_fixture(tmp_path)
    inputs = tmp_path / "inputs"
    inputs.mkdir()
    suite_path = inputs / "suite.json"
    suite_path.write_bytes(evaluator.canonical(suite))
    output = tmp_path / "output"
    receipt = holdout_review.write_natural_language_file_diagnostic(
        checkout, suite_path, output, suite_id="new-nl-file-diagnostic"
    )
    assert json.loads((output / "lineage.json").read_bytes()) == receipt
    with pytest.raises(ValueError, match="fresh and absolute"):
        holdout_review.write_natural_language_file_diagnostic(
            checkout, suite_path, output, suite_id="new-nl-file-diagnostic"
        )
    original = holdout_review.project_natural_language_file_diagnostic

    def race(*args, **kwargs):
        result = original(*args, **kwargs)
        suite_path.write_bytes(suite_path.read_bytes() + b"\n")
        return result

    monkeypatch.setattr(holdout_review, "project_natural_language_file_diagnostic", race)
    racing_output = tmp_path / "racing-output"
    with pytest.raises(ValueError, match="changed during projection"):
        holdout_review.write_natural_language_file_diagnostic(
            checkout, suite_path, racing_output, suite_id="new-nl-file-diagnostic"
        )
    assert not racing_output.exists()


def _supplemental_fixture(tmp_path, threshold=2):
    checkout, suite = _reviewed_suite_fixture(tmp_path)
    frozen = suite["tasks"][0]
    frozen["answerability_min_grade"] = threshold
    path = "alternative.py"
    raw = (checkout / path).read_bytes()
    task = {
        **{key: frozen[key] for key in ("task_id", "query", "query_sha256")},
        "intent": "File relevance",
        "provenance": "Independent fixture context",
        "rubric": "0 irrelevant; 1 partial clue; 2 sufficient; 3 directly answers",
        "files": [
            {
                "path": path,
                "file_sha256": evaluator.digest(raw),
                "source_text": raw.decode(),
                "grade": None,
                "unresolved": None,
            }
        ],
    }
    return checkout, suite, task


@pytest.mark.parametrize("threshold", [1, 2, 3])
def test_supplemental_request_binds_frozen_threshold_without_decisions(tmp_path, threshold):
    checkout, suite, task = _supplemental_fixture(tmp_path, threshold)
    before = copy.deepcopy(task)
    suite_raw = evaluator.canonical(suite)
    bound = holdout_review.bind_supplemental_review_tasks(checkout, suite_raw, [task])
    assert bound == [{**before, "answerability_min_grade": threshold}]
    assert task == before
    assert evaluator.canonical(suite) == suite_raw
    bound[0]["files"][0]["grade"] = 3
    assert task["files"][0]["grade"] is None


def test_supplemental_request_materializes_historical_default_from_suite(tmp_path):
    checkout, suite, task = _supplemental_fixture(tmp_path)
    del suite["tasks"][0]["answerability_min_grade"]
    bound = holdout_review.bind_supplemental_review_tasks(
        checkout, evaluator.canonical(suite), [task]
    )
    assert bound[0]["answerability_min_grade"] == 1


@pytest.mark.parametrize(
    "fault",
    [
        "threshold",
        "bool_threshold",
        "query",
        "unknown_task",
        "duplicate_task",
        "duplicate_pair",
        "already_judged",
        "source_hash",
        "source_text",
        "grade",
        "unresolved",
        "empty_files",
        "suite_threshold",
    ],
)
def test_supplemental_request_refuses_unbound_or_decided_input(tmp_path, fault):
    checkout, suite, task = _supplemental_fixture(tmp_path)
    tasks = [task]
    if fault == "threshold":
        task["answerability_min_grade"] = 1
    elif fault == "bool_threshold":
        task["answerability_min_grade"] = True
    elif fault == "query":
        task["query"] += " changed"
        task["query_sha256"] = evaluator.digest(task["query"].encode())
    elif fault == "unknown_task":
        task["task_id"] = "absent"
    elif fault == "duplicate_task":
        tasks.append(copy.deepcopy(task))
    elif fault == "duplicate_pair":
        task["files"].append(copy.deepcopy(task["files"][0]))
    elif fault == "already_judged":
        path = suite["tasks"][0]["file_judgments"][0]["path"]
        raw = (checkout / path).read_bytes()
        task["files"][0].update(
            path=path, file_sha256=evaluator.digest(raw), source_text=raw.decode()
        )
    elif fault == "source_hash":
        task["files"][0]["file_sha256"] = "0" * 64
    elif fault == "source_text":
        task["files"][0]["source_text"] += " changed"
    elif fault == "grade":
        task["files"][0]["grade"] = 0
    elif fault == "unresolved":
        task["files"][0]["unresolved"] = False
    elif fault == "empty_files":
        task["files"] = []
    elif fault == "suite_threshold":
        suite["tasks"][0]["answerability_min_grade"] = 0
    with pytest.raises(evaluator.EvidenceError):
        holdout_review.bind_supplemental_review_tasks(checkout, evaluator.canonical(suite), tasks)


def test_admission_queue_runs_later_ready_repository_before_waiting(tmp_path):
    (tmp_path / "ready").mkdir()
    (tmp_path / "ready/result.json").write_text('{"status":"VERIFIED","binding":"fixed"}')
    waits = []

    def wait():
        waits.append("polled")
        (tmp_path / "pending").mkdir()
        (tmp_path / "pending/failure.json").write_text('{"status":"FAILED"}')

    queue = execution_batch.iter_repository_admissions(
        tmp_path, ["pending", "ready"], upstream_alive=lambda: True, wait=wait
    )
    assert next(queue) == ("ready", {"status": "VERIFIED", "binding": "fixed"})
    assert waits == []
    assert list(queue) == [
        ("pending", {"status": "FAILED", "reason": "repository admission failure terminal"})
    ]
    assert waits == ["polled"]


@pytest.mark.parametrize(
    "fault", ["failure", "malformed", "wrong_status", "missing", "conflicting"]
)
def test_admission_queue_preserves_failed_cells_and_drains_ready_cells(tmp_path, fault):
    (tmp_path / "bad").mkdir()
    (tmp_path / "good").mkdir()
    (tmp_path / "good/result.json").write_text('{"status":"VERIFIED"}')
    if fault in {"failure", "conflicting"}:
        (tmp_path / "bad/failure.json").write_text('{"status":"FAILED"}')
    if fault == "conflicting":
        (tmp_path / "bad/result.json").write_text('{"status":"VERIFIED"}')
    elif fault == "malformed":
        (tmp_path / "bad/result.json").write_text("{")
    elif fault == "wrong_status":
        (tmp_path / "bad/result.json").write_text('{"status":"FAILED"}')
    rows = list(
        execution_batch.iter_repository_admissions(
            tmp_path,
            ["bad", "good"],
            upstream_alive=lambda: False,
            wait=lambda: pytest.fail("must not wait on terminated upstream"),
        )
    )
    outcomes = dict(rows)
    assert outcomes["bad"]["status"] == "FAILED"
    assert outcomes["good"] == {"status": "VERIFIED"}
    assert set(outcomes) == {"bad", "good"}


def test_admission_queue_rechecks_final_publish_after_upstream_exit(tmp_path):
    def ended():
        (tmp_path / "last").mkdir()
        (tmp_path / "last/result.json").write_text('{"status":"VERIFIED"}')
        return False

    assert list(
        execution_batch.iter_repository_admissions(
            tmp_path,
            ["last"],
            upstream_alive=ended,
            wait=lambda: pytest.fail("must not wait after final publish"),
        )
    ) == [("last", {"status": "VERIFIED"})]


@pytest.mark.parametrize("repos", [[], ["same", "same"], ["../escape"], ["."], "repo", [{}]])
def test_admission_queue_rejects_invalid_repository_identity(tmp_path, repos):
    with pytest.raises(execution_batch.BatchError, match="invalid admission repository"):
        list(
            execution_batch.iter_repository_admissions(
                tmp_path,
                repos,
                upstream_alive=lambda: False,
                wait=lambda: None,
            )
        )


def test_admission_queue_observes_repository_review_failure_without_global_wait(tmp_path):
    (tmp_path / "nushell").mkdir()
    (tmp_path / "nushell/result.json").write_text('{"status":"VERIFIED"}')
    assert list(
        execution_batch.iter_repository_admissions(
            tmp_path,
            ["typeorm", "nushell"],
            upstream_alive=lambda: True,
            wait=lambda: pytest.fail("failed repository must not block ready repository"),
            repository_failure=lambda repo: "original review failed" if repo == "typeorm" else None,
        )
    ) == [
        ("typeorm", {"status": "FAILED", "reason": "original review failed"}),
        ("nushell", {"status": "VERIFIED"}),
    ]


def test_admission_queue_drains_distinct_cohort_directories(tmp_path):
    first = tmp_path / "cohort-a" / "fixture"
    second = tmp_path / "cohort-b" / "fixture"
    first.mkdir(parents=True)
    second.mkdir(parents=True)
    (first / "failure.json").write_text('{"status":"FAILED"}')
    (second / "result.json").write_text('{"status":"VERIFIED"}')
    cells = {"member-a": first, "member-b": second}
    assert list(
        execution_batch.iter_repository_admissions(
            tmp_path,
            list(cells),
            upstream_alive=lambda: True,
            wait=lambda: pytest.fail("ready sibling must drain without polling"),
            repository_cells=cells,
        )
    ) == [
        ("member-a", {"status": "FAILED", "reason": "repository admission failure terminal"}),
        ("member-b", {"status": "VERIFIED"}),
    ]
    with pytest.raises(execution_batch.BatchError, match="repository cell mapping"):
        list(
            execution_batch.iter_repository_admissions(
                tmp_path,
                list(cells),
                upstream_alive=lambda: False,
                wait=lambda: None,
                repository_cells={"member-a": first, "member-b": first},
            )
        )
