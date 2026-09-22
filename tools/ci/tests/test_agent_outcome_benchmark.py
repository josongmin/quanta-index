"""Behavioral checks for the recorded-trajectory A/B/C authority."""

import copy
import json
import subprocess
import sys
from pathlib import Path

import pytest

CLI = Path(__file__).resolve().parents[2] / "benchmark" / "agent_outcome" / "__main__.py"
DIGEST = "sha256:" + "a" * 64
COMMIT = "b" * 40


def record(arm, trial="trial-1"):
    return {
        "schema_version": 1,
        "task_id": "issue-17",
        "task_digest": DIGEST,
        "checkout_commit": COMMIT,
        "trial_id": trial,
        "arm": arm,
        "arm_kind": {"A": "no_index", "B": "production_router", "C": "oracle_gold_context"}[arm],
        "arm_config_digest": "sha256:" + {"A": "1", "B": "2", "C": "3"}[arm] * 64,
        "model_id": "model-x",
        "model_revision": "rev-1",
        "model_config_digest": DIGEST,
        "scaffold_digest": DIGEST,
        "budget": {
            "max_total_tokens": 1000,
            "max_tool_calls": 10,
            "max_elapsed_ms": 10000,
            "max_cost_usd": 1.0,
        },
        "baseline_tests": {"regression": "fail", "safety": "pass"},
        "post_tests": {"regression": "pass", "safety": "pass"},
        "outcome_status": "completed",
        "trajectory": [
            {"seq": 0, "elapsed_ms": 0, "kind": "start"},
            {"seq": 1, "elapsed_ms": 100, "kind": "tool_call", "call_id": "call-1"},
            {
                "seq": 2,
                "elapsed_ms": 200,
                "kind": "evidence",
                "evidence_id": "source:1",
                "source_call_id": "call-1",
                "useful": True,
            },
            {"seq": 3, "elapsed_ms": 500, "kind": "finish"},
        ],
        "usage": {
            "input_tokens": 100,
            "output_tokens": 50,
            "tool_calls": 1,
            "elapsed_ms": 500,
            "cost_usd": 0.1,
        },
    }


def run_cli(tmp_path, rows, command="summarize", raw=None):
    source = tmp_path / "trajectories.jsonl"
    source.write_text(
        raw if raw is not None else "\n".join(json.dumps(row) for row in rows) + "\n",
        encoding="utf-8",
    )
    return subprocess.run(
        [sys.executable, str(CLI), command, str(source)],
        text=True,
        capture_output=True,
        check=False,
    )


def valid_rows():
    return [record(arm) for arm in "ABC"]


def test_paired_summary_uses_recorded_tests_and_events(tmp_path):
    rows = valid_rows()
    rows[0]["post_tests"]["regression"] = "fail"
    rows[1]["post_tests"]["safety"] = "fail"
    rows[2]["trajectory"][2]["useful"] = False
    result = run_cli(tmp_path, rows)
    assert result.returncode == 0, result.stderr
    summary = json.loads(result.stdout)
    assert summary["record_count"] == 3
    assert summary["pair_count"] == 1
    assert summary["aggregate"]["A"]["fail_to_pass"] == {"passed": 0, "total": 1, "rate": "0"}
    assert summary["aggregate"]["B"]["pass_to_pass"] == {"passed": 0, "total": 1, "rate": "0"}
    assert summary["aggregate"]["C"]["resolved_pairs"] == 1
    assert summary["paired_resolved"]["C_vs_A"] == {"wins": 1, "losses": 0, "ties": 0}
    assert summary["pairs"][0]["arms"]["C"]["first_useful_evidence_ms"] is None
    assert summary["aggregate"]["C"]["useful_evidence_pairs"] == 0
    assert summary["aggregate"]["A"]["mean_cost_usd"] == "0.1"
    assert summary["aggregate"]["A"]["mean_tool_calls"] == "1"
    assert summary["input_sha256"].startswith("sha256:")


@pytest.mark.parametrize(
    "mutation,reason",
    [
        (lambda rows: rows.pop(), "needs exactly A/B/C"),
        (lambda rows: rows.append(copy.deepcopy(rows[0])), "duplicate trajectory"),
        (lambda rows: rows[1].update(model_revision="rev-2"), "model_revision differs"),
        (
            lambda rows: rows[1].update(scaffold_digest="sha256:" + "c" * 64),
            "scaffold_digest differs",
        ),
        (lambda rows: rows[1]["budget"].update(max_tool_calls=9), "budget differs"),
        (lambda rows: rows[1]["baseline_tests"].update(regression="pass"), "baseline needs both"),
        (lambda rows: rows[1]["post_tests"].pop("safety"), "must cover exactly"),
        (lambda rows: rows[1].update(outcome_status="timeout"), "must be completed"),
        (
            lambda rows: rows[1].update(task_digest="sha256:" + "c" * 64),
            "conflicting immutable identity",
        ),
        (lambda rows: rows[1].update(checkout_commit="short"), "full lowercase Git SHA"),
        (lambda rows: rows[1]["usage"].update(tool_calls=0), "disagrees with trajectory"),
        (lambda rows: rows[1]["usage"].update(cost_usd=1.1), "cost budget exceeded"),
        (lambda rows: rows[1]["trajectory"][2].update(elapsed_ms=50), "earlier than previous"),
        (
            lambda rows: rows[1]["trajectory"][2].update(source_call_id="future"),
            "missing or future tool call",
        ),
    ],
)
def test_invalid_or_unpaired_evidence_fails_closed(tmp_path, mutation, reason):
    rows = valid_rows()
    mutation(rows)
    for command in ("validate", "summarize"):
        result = run_cli(tmp_path, rows, command=command)
        assert result.returncode == 2
        assert reason in result.stderr
        assert result.stdout == ""


def test_duplicate_json_key_is_refused(tmp_path):
    result = run_cli(tmp_path, [], raw='{"arm":"A","arm":"B"}\n')
    assert result.returncode == 2
    assert "duplicate JSON key" in result.stderr


def test_nonfinite_number_and_blank_line_are_refused(tmp_path):
    rows = valid_rows()
    raw = "\n".join(json.dumps(row) for row in rows)
    for invalid in (raw.replace('"cost_usd": 0.1', '"cost_usd": NaN', 1), raw + "\n\n"):
        result = run_cli(tmp_path, [], raw=invalid)
        assert result.returncode == 2
        assert result.stdout == ""


def test_validate_reports_coverage_without_scores(tmp_path):
    result = run_cli(tmp_path, valid_rows(), command="validate")
    assert result.returncode == 0, result.stderr
    output = json.loads(result.stdout)
    assert output["status"] == "valid"
    assert output["record_count"] == 3
    assert output["pair_count"] == 1
    assert "aggregate" not in output


def test_repeated_trial_must_keep_task_baseline(tmp_path):
    rows = valid_rows() + [record(arm, trial="trial-2") for arm in "ABC"]
    rows[3]["baseline_tests"] = {"regression": "pass", "safety": "fail"}
    result = run_cli(tmp_path, rows)
    assert result.returncode == 2
    assert "conflicting baseline tests" in result.stderr


def test_repeated_trials_aggregate_paired_counts(tmp_path):
    rows = valid_rows() + [record(arm, trial="trial-2") for arm in "ABC"]
    rows[3]["post_tests"]["regression"] = "fail"
    result = run_cli(tmp_path, rows)
    assert result.returncode == 0, result.stderr
    summary = json.loads(result.stdout)
    assert summary["pair_count"] == 2
    assert summary["record_count"] == 6
    assert summary["aggregate"]["A"]["fail_to_pass"] == {"passed": 1, "total": 2, "rate": "0.5"}
    assert summary["paired_resolved"]["C_vs_A"] == {"wins": 1, "losses": 0, "ties": 1}


def test_arm_configuration_drift_and_alias_are_refused(tmp_path):
    rows = valid_rows() + [record(arm, trial="trial-2") for arm in "ABC"]
    rows[4]["arm_config_digest"] = "sha256:" + "4" * 64
    result = run_cli(tmp_path, rows)
    assert result.returncode == 2
    assert "conflicting configuration digests" in result.stderr

    rows = valid_rows()
    rows[1]["arm_config_digest"] = rows[0]["arm_config_digest"]
    result = run_cli(tmp_path, rows)
    assert result.returncode == 2
    assert "distinct configuration digests" in result.stderr

    rows = valid_rows()
    rows[2]["arm_kind"] = "production_router"
    result = run_cli(tmp_path, rows)
    assert result.returncode == 2
    assert "arm_kind does not match" in result.stderr
