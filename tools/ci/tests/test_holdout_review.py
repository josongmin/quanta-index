"""Blind review preparation must preserve source and never invent decisions."""

from __future__ import annotations

import copy
import json
import subprocess
from pathlib import Path

import pytest

from tools.benchmark.retrieval import evaluator, holdout_review


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
