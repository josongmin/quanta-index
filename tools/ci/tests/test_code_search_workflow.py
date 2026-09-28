"""Workflow refuses divergent inputs and never publishes partial captures."""

import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "benchmark"))

import benchctl  # noqa: E402
import code_search_workflow as workflow  # noqa: E402

from tools.benchmark.retrieval import query_plan  # noqa: E402


def test_workflow_spec_refuses_unbounded_timeout_and_unknown_key(tmp_path):
    path = tmp_path / "spec.json"
    value = {"schema_version": 1, "pair_spec": "/external/pair.json",
             "external_spec": "/external/external.json", "output_root": "/external/fresh",
             "native_output_root": "/private/tmp/qd",
             "timeout_secs": 7200}
    path.write_text(json.dumps(value))
    assert workflow._read_spec(path) == value
    for mutation in ({"timeout_secs": True}, {"timeout_secs": 86401}, {"skip_external": True}):
        path.write_text(json.dumps({**value, **mutation}))
        with pytest.raises(ValueError, match="closed schema"):
            workflow._read_spec(path)


def test_native_pair_root_checks_socket_budget_before_external_capture(tmp_path):
    pair = {"strategies": [{"name": "fixed_window_strict"}], "repetitions": 1}
    short_root = Path("/private/tmp/q")
    workflow._preflight_native_output(short_root, pair)
    assert not short_root.exists()

    long_root = tmp_path / ("q" * 120)
    with pytest.raises(workflow.run.RunError, match="Unix socket path.*limit 103"):
        workflow._preflight_native_output(long_root, pair)


def test_preflight_contract_requires_lexical_only_route_labels():
    pair = {
        "execution_profiles": {
            "quanta": query_plan.execution_profile("native"),
            "semble": {
                "profile_id": "semble-lexical-only-v1",
                "mode": "lexical-only",
                "alpha": None,
                "rerank": "not_applicable",
            },
        },
        "routes": ["lexical"],
        "candidate_route": "lexical",
        "baseline_route": "semble-lexical-only",
        "semble_route": "semble-lexical-only",
        "scope": "exploratory",
        "claims": {"quality": False},
        "repetitions": 1,
        "strategies": [{"name": "fixed_window_strict"}],
    }
    workflow._require_pure_lexical_pair(pair)

    for key in ("baseline_route", "semble_route"):
        legacy = dict(pair)
        legacy[key] = "semble-hybrid"
        with pytest.raises(ValueError, match="pure-lexical pair"):
            workflow._require_pure_lexical_pair(legacy)


def test_external_failure_prevents_pair_execution_and_workflow_publication(tmp_path, monkeypatch):
    root = tmp_path / "fresh-workflow"
    spec = {"schema_version": 1, "pair_spec": str(tmp_path / "pair.json"),
            "external_spec": str(tmp_path / "external.json"), "output_root": str(root),
                "native_output_root": "/private/tmp/qd",
            "timeout_secs": 60}
    path = tmp_path / "workflow.json"
    path.write_text(json.dumps(spec))
    monkeypatch.setattr(benchctl, "require_clean_worktree", lambda repo: None)
    monkeypatch.setattr(benchctl, "resolve_checkout_head", lambda repo: "a" * 40)
    monkeypatch.setattr(workflow, "source_identity", lambda repo, closure: {"revision": "a" * 40})
    monkeypatch.setattr(
        workflow,
        "preflight",
        lambda pair, external: ({"strategies": [{"name": "fixed_window_strict"}]}, {}),
    )
    stages = []

    def failed_external(repo, root, name, args, timeout):
        stages.append(name)
        raise ValueError("external HTTP failed")

    monkeypatch.setattr(workflow, "_command", failed_external)
    with pytest.raises(ValueError, match="external HTTP failed"):
        workflow.capture(tmp_path, path)
    assert stages == ["external-live"]
    assert not (root / "workflow.json").exists()
    assert json.loads((root / "failure.json").read_text())["status"] == "failed"
