"""Workflow refuses divergent inputs and never publishes partial captures."""

import json
import string
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "benchmark"))

import code_search_workflow as workflow  # noqa: E402

from tools.benchmark.retrieval import query_plan  # noqa: E402


def test_workflow_spec_refuses_unbounded_timeout_and_unknown_key(tmp_path):
    path = tmp_path / "spec.json"
    value = {
        "schema_version": 1,
        "pair_spec": "/external/pair.json",
        "external_spec": "/external/external.json",
        "output_root": "/external/fresh",
        "timeout_secs": 7200,
    }
    path.write_text(json.dumps(value))
    assert workflow._read_spec(path) == value
    for mutation in ({"timeout_secs": True}, {"timeout_secs": 86401}, {"skip_external": True}):
        path.write_text(json.dumps({**value, **mutation}))
        with pytest.raises(ValueError, match="closed schema"):
            workflow._read_spec(path)


def test_native_pair_root_checks_socket_budget_before_external_capture(tmp_path):
    pair = {"strategies": [{"name": "fixed_window_strict"}], "repetitions": 1}
    # Darwin leaves only two bytes beyond /private/tmp/q for this socket.
    # Reserve a one-character directory atomically so parallel tests cannot
    # reuse the same fresh root.
    for suffix in string.ascii_letters + string.digits:
        short_dir = Path("/tmp").resolve() / suffix
        try:
            short_dir.mkdir()
        except FileExistsError:
            continue
        try:
            short_root = short_dir / "q"
            workflow._preflight_native_output(short_root, pair)
            assert not short_root.exists()
        finally:
            short_dir.rmdir()
        break
    else:
        pytest.fail("no short temporary path available for socket preflight")

    long_root = tmp_path / ("q" * 120)
    limit = {"darwin": 103, "linux": 107}.get(sys.platform)
    if limit is None:
        pytest.skip("native Unix socket path limit is defined for macOS and Linux")
    with pytest.raises(workflow.run.RunError, match=rf"Unix socket path.*limit {limit}"):
        workflow._preflight_native_output(long_root, pair)
    assert not long_root.exists()


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
    spec = {
        "schema_version": 1,
        "pair_spec": str(tmp_path / "pair.json"),
        "external_spec": str(tmp_path / "external.json"),
        "output_root": str(root),
        "timeout_secs": 60,
    }
    path = tmp_path / "workflow.json"
    path.write_text(json.dumps(spec))
    monkeypatch.setattr(workflow, "_source", lambda repo: {"revision": "a" * 40})
    monkeypatch.setattr(workflow, "preflight", lambda pair, external: ({}, {}))
    monkeypatch.setattr(
        workflow, "_native_pair_root", lambda pair, root: (root / "native-pair", None)
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


@pytest.mark.skipif(sys.platform != "darwin", reason="macOS sun_path boundary")
def test_long_evidence_root_reserves_a_short_disjoint_runtime_path(tmp_path):
    pair = {"strategies": [{"name": "fixed_window_strict"}]}
    root = tmp_path / ("long-evidence-root-" * 6)
    native, reservation = workflow._native_pair_root(pair, root)
    try:
        assert reservation is not None and reservation.exists()
        assert not native.exists()
        assert not native.is_relative_to(root)
        workflow.run.preflight_daemon_socket_paths(
            native.with_name(native.name + ".staging"),
            pair["strategies"],
            repetitions=1,
            paired=True,
        )
    finally:
        if reservation is not None:
            reservation.unlink()


@pytest.mark.parametrize(
    "mutation",
    [
        {"schema_version": True},
        {"products": ["quanta_lexical"]},
        {"exclusions": []},
        {"quality_qualified": True},
        {"tasks": True},
        {"components": {}},
    ],
)
def test_workflow_verify_refuses_forged_summary_claims(tmp_path, monkeypatch, mutation):
    source = {"revision": "a" * 40}
    monkeypatch.setattr(workflow, "_source", lambda repo: source)
    value = {
        "schema_version": 1,
        "status": "diagnostic_unqualified",
        "source": source,
        "binding": {},
        "tasks": 20,
        "components": {"native_external": "sha256:" + "0" * 64, "pair": {}, "lexical": {}},
        "products": ["quanta_lexical", "semble_lexical_only", "sourcegraph", "opengrok", "cs"],
        "exclusions": [
            "independent_gold",
            "qualified_speed",
            "backend_indexed_universe_attestation",
        ],
        "input_sha256": {
            name: "sha256:" + "0" * 64
            for name in [
                "workflow-spec.json",
                "pair-spec.json",
                "external-spec.json",
                "lexical-spec.json",
            ]
        },
    }
    (tmp_path / "workflow.json").write_text(json.dumps({**value, **mutation}))
    with pytest.raises(ValueError, match="unsupported workflow metadata"):
        workflow.verify(tmp_path, tmp_path)


@pytest.mark.parametrize("mutation", ["changed", "missing", "duplicate"])
def test_cross_capture_inputs_refuse_different_native_observations(mutation):
    expected = {"sourcegraph_rows": "sha256:" + "a" * 64, "pair_report": "sha256:" + "b" * 64}
    rows = [
        {"id": role, "availability": "present", "digest": digest}
        for role, digest in expected.items()
    ]
    workflow._cross_inputs({"inputs": rows}, expected)
    if mutation == "changed":
        rows[0]["digest"] = "sha256:" + "c" * 64
    elif mutation == "missing":
        rows.pop()
    else:
        rows.append(dict(rows[0]))
    with pytest.raises(ValueError, match="workflow native observations"):
        workflow._cross_inputs({"inputs": rows}, expected)


def test_workflow_source_identity_refuses_boolean_integer_alias(tmp_path, monkeypatch):
    source = {"revision": "a" * 40, "dirty": False}
    monkeypatch.setattr(workflow, "_source", lambda repo: source)
    value = {
        "schema_version": 1,
        "status": "diagnostic_unqualified",
        "source": {"revision": "a" * 40, "dirty": 0},
        "binding": {},
        "tasks": 20,
        "components": {"native_external": "sha256:" + "0" * 64, "pair": {}, "lexical": {}},
        "products": ["quanta_lexical", "semble_lexical_only", "sourcegraph", "opengrok", "cs"],
        "exclusions": [
            "independent_gold",
            "qualified_speed",
            "backend_indexed_universe_attestation",
        ],
        "input_sha256": {},
    }
    (tmp_path / "workflow.json").write_text(json.dumps(value))
    with pytest.raises(ValueError, match="exact source identity differs"):
        workflow.verify(tmp_path, tmp_path)
