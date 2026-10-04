"""Real Git source, blinded runner boundary and budgeted route scoring."""

from __future__ import annotations

import ast
import copy
import ctypes
import hashlib
import html
import io
import json
import math
import os
import shlex
import shutil
import stat
import subprocess
import sys
import tempfile
import threading
import zipfile
from pathlib import Path
from types import SimpleNamespace

import jsonschema
import pytest

from tools.benchmark.retrieval import conditional_proof as cp
from tools.benchmark.retrieval import evaluator as ev
from tools.benchmark.retrieval import execution_batch as eb
from tools.benchmark.retrieval import parity_reference, portable_proof
from tools.benchmark.retrieval import query_plan as qp
from tools.benchmark.retrieval import query_pool_guard as pool_guard
from tools.benchmark.retrieval import query_timing_overhead as overhead
from tools.benchmark.retrieval import run as pairrun
from tools.benchmark.retrieval import semble as semble_adapter
from tools.ci import source_closure
from tools.ci.tests.test_portable_proof import proof_actor_environment as proof_actor_environment


def test_execution_batch_preserves_member_packs_and_names_shared_query() -> None:
    shared = {
        "schema_version": 3,
        "repository_commit": "a" * 40,
        "tokenizer": ev.TOKENIZER,
        "tokenizer_budget_version": ev.TOKENIZER_BUDGET_VERSION,
        "routes": ["lexical"],
        "file_universe": [{"path": "source.py", "file_sha256": "b" * 64}],
        "file_universe_digest": "c" * 64,
        "comparison_contract": {"top_k": 10},
    }

    def task(task_id: str, query: str) -> dict:
        return {
            "task_id": task_id,
            "query": query,
            "query_sha256": hashlib.sha256(query.encode()).hexdigest(),
        }

    first = {
        **shared,
        "suite_id": "exact",
        "suite_commitment_sha256": "d" * 64,
        "tasks": [task("e1", "parseThing"), task("e2", "writeThing")],
    }
    second = {
        **shared,
        "suite_id": "typo",
        "suite_commitment_sha256": "e" * 64,
        "tasks": [task("t1", "parseThing"), task("t2", "parseThng")],
    }
    before = copy.deepcopy([first, second])
    union, membership = eb.build_execution_pack([second, first])
    assert [row["task_id"] for row in union["tasks"]] == ["e1", "e2", "t2"]
    assert membership["members"][1]["tasks"][0]["execution_task_id"] == "e1"
    assert (union, membership) == eb.build_execution_pack([first, second])
    assert [first, second] == before
    eb.verify_execution_membership([second, first], union, membership)
    assert eb.execution_validation_view(union)["tasks"] == [
        {"task_id": row["task_id"], "query": row["query"], "split": "eval"}
        for row in union["tasks"]
    ]
    assert (
        eb.execution_validation_view(union)["comparison_contract"] == shared["comparison_contract"]
    )

    native = {
        "query_pack_sha256": membership["execution_pack_sha256"],
        "results": [
            {"task_id": row["task_id"], "route": "lexical", "candidates": [row["query"]]}
            for row in union["tasks"]
        ],
    }
    view = eb.project_scoring_view(union, membership, second, native)
    assert [row["task_id"] for row in view["results"]] == ["t1", "t2"]
    assert view["results"][0]["candidates"] == ["parseThing"]
    assert native["results"][0]["task_id"] == "e1"
    assert view["query_pack_sha256"] == ev.digest(ev.canonical(second))
    duplicate = copy.deepcopy(native)
    duplicate["results"].append(copy.deepcopy(duplicate["results"][0]))
    with pytest.raises(eb.BatchError, match="incomplete or duplicated"):
        eb.project_scoring_view(union, membership, second, duplicate)

    altered = copy.deepcopy(membership)
    altered["members"][1]["tasks"][0]["execution_task_id"] = "e2"
    with pytest.raises(eb.BatchError, match="differs"):
        eb.verify_execution_membership([first, second], union, altered)
    with pytest.raises(eb.BatchError, match="query does not match"):
        eb.project_scoring_view(union, altered, second, native)
    changed = copy.deepcopy(second)
    changed["file_universe_digest"] = "f" * 64
    with pytest.raises(eb.BatchError, match="file_universe_digest"):
        eb.build_execution_pack([first, changed])
    changed = copy.deepcopy(second)
    changed["tasks"][0]["task_id"] = "e1"
    with pytest.raises(eb.BatchError, match="repeats task ID"):
        eb.build_execution_pack([first, changed])
    changed = copy.deepcopy(second)
    changed["tasks"][0]["query_sha256"] = "0" * 64
    with pytest.raises(eb.BatchError, match="identity is invalid"):
        eb.build_execution_pack([first, changed])


def test_quality_batch_spec_and_product_contract_refuse_drift(tmp_path, monkeypatch) -> None:
    batch_path = tmp_path / "batch.json"
    member_paths = [tmp_path / "first.json", tmp_path / "second.json"]
    batch = {
        "schema_version": 1,
        "member_specs": [str(path) for path in member_paths],
        "output_root": str(tmp_path / "output"),
    }
    batch_path.write_text(json.dumps(batch), encoding="utf-8")
    assert pairrun.load_quality_batch_spec(batch_path) == batch
    for bad in (
        {**batch, "member_specs": [str(member_paths[0])]},
        {**batch, "member_specs": [str(member_paths[0])] * 2},
        {**batch, "output_root": "relative/output"},
        {**batch, "unknown": True},
    ):
        batch_path.write_text(json.dumps(bad), encoding="utf-8")
        with pytest.raises(pairrun.RunError):
            pairrun.load_quality_batch_spec(batch_path)

    cache = tmp_path / "model-cache"
    cache.mkdir()
    suites = []
    packs = []
    specs = {}
    for index, path in enumerate(member_paths):
        suite = tmp_path / f"suite-{index}.json"
        pack = tmp_path / f"pack-{index}.json"
        suite.write_text(json.dumps({"id": index}), encoding="utf-8")
        pack.write_text(json.dumps({"id": index}), encoding="utf-8")
        suites.append(suite)
        packs.append(pack)
        specs[str(path)] = {
            "scope": "exploratory",
            "claims": {},
            "repetitions": 1,
            "strategies": [{"name": "fixed_window_strict"}],
            "repo": str(tmp_path),
            "suite": str(suite),
            "query_pack": str(pack),
            "output_root": str(tmp_path / f"old-{index}"),
            "run_id": f"old-{index}",
            "semble_cache_root": str(cache),
            "semble_model_revision": "a" * 40,
        }
    monkeypatch.setattr(pairrun, "load_spec", lambda path: specs[str(path)])
    monkeypatch.setattr(
        pairrun.semble_adapter, "resolve_model_revision", lambda *_args: ("a" * 40, "b" * 64)
    )
    monkeypatch.setattr(
        pairrun,
        "validate_suite",
        lambda _repo, payload, **_kwargs: (payload, payload, object()),
    )
    valid, model = pairrun._quality_batch_members(batch)
    assert len(valid) == 2 and model["model_asset_sha256"] == "b" * 64
    for path in member_paths:
        path.write_text(json.dumps(specs[str(path)]), encoding="utf-8")
    snapshot = pairrun._quality_batch_input_snapshot(valid)
    assert len(snapshot) == 2

    specs[str(member_paths[1])]["strategies"] = [{"name": "whole_file"}]
    with pytest.raises(pairrun.RunError, match="product/source contract differs"):
        pairrun._quality_batch_members(batch)
    specs[str(member_paths[1])]["strategies"] = [{"name": "fixed_window_strict"}]
    specs[str(member_paths[1])]["claims"] = {"speed": True}
    with pytest.raises(pairrun.RunError, match="exploratory only"):
        pairrun._quality_batch_members(batch)
    specs[str(member_paths[1])]["claims"] = {}
    original_resolver = pairrun.semble_adapter.resolve_model_revision
    calls = iter([("a" * 40, "b" * 64), ("a" * 40, "c" * 64)])
    monkeypatch.setattr(
        pairrun.semble_adapter,
        "resolve_model_revision",
        lambda *_args: next(calls),
    )
    with pytest.raises(pairrun.RunError, match="model assets differ"):
        pairrun._quality_batch_members(batch)
    monkeypatch.setattr(pairrun.semble_adapter, "resolve_model_revision", original_resolver)
    packs[1].write_text(json.dumps({"id": "wrong"}), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="blind pack differs"):
        pairrun._quality_batch_members(batch)
    with pytest.raises(pairrun.RunError, match="inputs changed while loading"):
        pairrun._quality_batch_input_snapshot(valid)


def test_quality_matrix_groups_and_replay_custody(tmp_path, monkeypatch) -> None:
    specs = [tmp_path / f"member-{index}.json" for index in range(4)]
    for index, path in enumerate(specs):
        path.write_text(
            json.dumps({"repo": str(tmp_path / f"repo-{index // 2}")}), encoding="utf-8"
        )
    monkeypatch.setattr(pairrun, "load_spec", lambda path: json.loads(path.read_text()))
    matrix = {
        "schema_version": 1,
        "member_specs": [str(path) for path in (specs[2], specs[0], specs[3], specs[1])],
        "output_root": str(tmp_path / "matrix"),
    }
    groups = pairrun._quality_matrix_groups(matrix)
    assert len(groups) == 2
    assert [row[2] for row in groups] == [
        sorted([str(specs[0]), str(specs[1])]),
        sorted([str(specs[2]), str(specs[3])]),
    ]
    root = Path(matrix["output_root"])
    root.mkdir()
    rows = []
    for group in groups:
        name, repo, paths = group
        batch_path = root / f"{name}-spec.json"
        batch_path.write_bytes(
            pairrun.canonical_bytes(pairrun._quality_matrix_batch(matrix, group))
        )
        child_manifest = root / name / "batch-manifest.json"
        child_manifest.parent.mkdir()
        child_manifest.write_text("{}", encoding="utf-8")
        rows.append(
            {
                "name": name,
                "repo": repo,
                "member_specs": paths,
                "batch_spec_sha256": pairrun.sha_file(batch_path),
                "batch_manifest_sha256": pairrun.sha_file(child_manifest),
            }
        )
    (root / "matrix-manifest.json").write_text(
        json.dumps(
            {
                "schema_version": 1,
                "kind": "retrieval_quality_matrix_v1",
                "qualification": "diagnostic_unqualified",
                "matrix_spec_sha256": ev.digest(ev.canonical(matrix)),
                "groups": rows,
            }
        ),
        encoding="utf-8",
    )
    replayed = []
    monkeypatch.setattr(pairrun, "verify_quality_batch", lambda batch: replayed.append(batch))
    assert pairrun.verify_quality_matrix(matrix) == 0
    assert len(replayed) == 2
    (root / f"{groups[0][0]}-spec.json").write_text("{}", encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="batch custody changed"):
        pairrun.verify_quality_matrix(matrix)


def test_quality_matrix_rejects_stale_runner_before_gold_or_output(tmp_path, monkeypatch) -> None:
    specs = [tmp_path / f"member-{index}.json" for index in range(4)]
    for index, path in enumerate(specs):
        path.write_text(
            json.dumps(
                {
                    "repo": str(tmp_path / f"repo-{index // 2}"),
                    "runner_binary": str(tmp_path / "stale-runner"),
                }
            ),
            encoding="utf-8",
        )
    monkeypatch.setattr(pairrun, "load_spec", lambda path: json.loads(path.read_text()))
    monkeypatch.setattr(
        pairrun,
        "probe_runner_capabilities",
        lambda _binary: (_ for _ in ()).throw(pairrun.RunError("stale runner")),
    )
    monkeypatch.setattr(
        pairrun,
        "_quality_batch_members",
        lambda _batch: (_ for _ in ()).throw(AssertionError("gold preflight ran")),
    )
    matrix = {
        "schema_version": 1,
        "member_specs": [str(path) for path in specs],
        "output_root": str(tmp_path / "matrix"),
    }
    with pytest.raises(pairrun.RunError, match="stale runner"):
        pairrun.run_quality_matrix(matrix)
    assert not Path(matrix["output_root"]).exists()
    with pytest.raises(pairrun.RunError, match="stale runner"):
        pairrun.run_quality_batch(
            {
                "schema_version": 1,
                "member_specs": matrix["member_specs"][:2],
                "output_root": str(tmp_path / "batch"),
            }
        )
    assert not (tmp_path / "batch").exists()


def test_quality_matrix_reuses_first_verified_driver_closure(tmp_path, monkeypatch, capsys) -> None:
    specs = [tmp_path / f"member-{index}.json" for index in range(4)]
    for index, path in enumerate(specs):
        path.write_text(
            json.dumps(
                {
                    "repo": str(tmp_path / f"repo-{index // 2}"),
                    "runner_binary": str(tmp_path / "runner"),
                }
            ),
            encoding="utf-8",
        )
    monkeypatch.setattr(pairrun, "load_spec", lambda path: json.loads(path.read_text()))
    monkeypatch.setattr(pairrun, "probe_runner_capabilities", lambda _binary: None)
    monkeypatch.setattr(
        pairrun,
        "_quality_batch_members",
        lambda _batch: ([(None, {"strategies": [{"name": "whole_file"}]})], {}),
    )
    monkeypatch.setattr(pairrun, "_quality_batch_input_snapshot", lambda _members: [])
    monkeypatch.setattr(pairrun, "preflight_daemon_socket_paths", lambda *_args: None)
    observed = []

    def run_batch(batch, *, prevalidated, closure_source):
        assert prevalidated[0]
        out = Path(batch["output_root"])
        out.mkdir()
        (out / "driver-source-closure.json").write_text("{}", encoding="utf-8")
        (out / "batch-manifest.json").write_text("{}", encoding="utf-8")
        observed.append((out, closure_source))

    monkeypatch.setattr(pairrun, "run_quality_batch", run_batch)
    matrix = {
        "schema_version": 1,
        "member_specs": [str(path) for path in specs],
        "output_root": str(tmp_path / "matrix"),
    }
    assert pairrun.run_quality_matrix(matrix) == 0
    assert len(observed) == 2
    assert observed[0][1] is None
    assert observed[1][1] == observed[0][0] / "driver-source-closure.json"
    result = json.loads(capsys.readouterr().out)
    phases = result["driver_phase_ms"]
    assert phases["prevalidation"] >= 0
    assert phases["batches_and_manifest"] >= 0
    assert sum(phases.values()) == pytest.approx(result["driver_total_ms"])


def test_driver_phase_durations_use_adjacent_clock_marks() -> None:
    assert pairrun._driver_phase_durations_ms(
        [("start", 0), ("setup", 1_000_000), ("capture", 4_000_000)]
    ) == {"setup": 1.0, "capture": 3.0}


def _clean_host_timeline_fixture():
    host = {
        "system": "Darwin",
        "release": "test",
        "machine": "arm64",
        "processor": "cpu",
        "cpu_count": 8,
        "python": "3.11",
        "rustc": "test",
        "concurrent_processes": {"none": []},
        "contention_override": False,
        "thermal": {"status": "clean"},
        "frequency": {"status": "bounded"},
        "power": {"status": "bounded", "digest": "a" * 64},
    }
    profile = {"fingerprint": pairrun._host_fingerprint(host)}
    payload = {
        "schema_version": 1,
        "interval_ns": 1_000_000_000,
        "started_ns": 100,
        "finished_ns": 10_000_000_100,
        "samples": [
            {"started_ns": ns, "finished_ns": ns + 10, "probe": copy.deepcopy(host)}
            for ns in (100, 5_000_000_100, 10_000_000_090)
        ],
        "errors": [],
        "reservation_id": "a" * 32,
        "monitor_sha256": "b" * 64,
    }
    return payload, profile


def test_host_timeline_rejects_middle_contention_and_identity_drift():
    payload, profile = _clean_host_timeline_fixture()
    pairrun.validate_host_timeline(payload, profile)
    for field, value in (
        ("concurrent_processes", {"rustc": [123]}),
        ("thermal", {"status": "unavailable"}),
        ("machine", "different-host"),
    ):
        changed = copy.deepcopy(payload)
        changed["samples"][1]["probe"][field] = value
        with pytest.raises(pairrun.RunError, match="host timeline.*unclean"):
            pairrun.validate_host_timeline(changed, profile)


@pytest.mark.parametrize("mutation", ["gap", "error", "empty", "unordered", "bool", "unknown"])
def test_host_timeline_rejects_missing_or_malformed_observations(mutation):
    payload, profile = _clean_host_timeline_fixture()
    if mutation == "gap":
        payload["finished_ns"] = 50_000_000_100
    elif mutation == "error":
        payload["errors"] = ["probe failed"]
    elif mutation == "empty":
        payload["samples"] = []
    elif mutation == "unordered":
        payload["samples"].reverse()
    elif mutation == "bool":
        payload["samples"][1]["started_ns"] = True
    else:
        payload["unsupported"] = True
    with pytest.raises(pairrun.RunError, match="host timeline"):
        pairrun.validate_host_timeline(payload, profile)


def test_host_timeline_capture_observes_background_contention(tmp_path, monkeypatch):
    payload, profile = _clean_host_timeline_fixture()
    host = payload["samples"][0]["probe"]
    observed = threading.Event()
    calls = 0

    def probe(identity, override):
        nonlocal calls
        calls += 1
        if calls == 2:
            observed.set()
            return {**host, "concurrent_processes": {"rustc": [123]}}
        return copy.deepcopy(host)

    monkeypatch.setattr(pairrun, "_host_dynamic_probe", probe)
    monkeypatch.setattr(pairrun, "HOST_SAMPLE_INTERVAL_NS", 1_000_000)
    path = tmp_path / "timeline.json"
    with pairrun.HostTimeline(path, host, False):
        assert observed.wait(timeout=5), "background probe did not observe the measured interval"
    with pytest.raises(pairrun.RunError, match="host timeline.*unclean"):
        pairrun.validate_host_timeline(json.loads(path.read_text()), profile)


def test_host_timeline_capture_keeps_probe_failure(tmp_path, monkeypatch):
    payload, profile = _clean_host_timeline_fixture()
    host = payload["samples"][0]["probe"]

    def broken_probe(identity, override):
        raise OSError("missing host input")

    monkeypatch.setattr(pairrun, "_host_dynamic_probe", broken_probe)
    path = tmp_path / "timeline.json"
    calls = 0

    def first_success_then_failure(identity, override):
        nonlocal calls
        calls += 1
        if calls == 1:
            return copy.deepcopy(host)
        return broken_probe(identity, override)

    monkeypatch.setattr(pairrun, "_host_dynamic_probe", first_success_then_failure)
    with pairrun.HostTimeline(path, host, False):
        pass
    captured = json.loads(path.read_text())
    assert captured["errors"] and "OSError" in captured["errors"][0]
    with pytest.raises(pairrun.RunError, match="host timeline.*errors"):
        pairrun.validate_host_timeline(captured, profile)


def test_host_timeline_bounds_collector_memory_before_appending(tmp_path, monkeypatch):
    payload, _profile = _clean_host_timeline_fixture()
    host = payload["samples"][0]["probe"]
    monkeypatch.setattr(pairrun, "_host_dynamic_probe", lambda *_args: host)
    monitor = pairrun.HostTimeline(tmp_path / "timeline.json", host, False)
    monitor.sample_bytes = pairrun.CONTROL_DOCUMENT_BYTES - 4096
    with pytest.raises(pairrun.RunError, match="byte budget"):
        monitor._sample()
    assert monitor.samples == []


def _current_symbol_metrics(metrics):
    """Handwritten complete-state fixtures, separate from producer execution."""
    metrics.update(
        {
            "symbol_coverage_policy": "require-complete",
            "empty_scopes": 0,
            "symbol_preflight_out": "symbol-preflight.json",
            "symbol_preflight_sha256": "d" * 64,
            "symbol_producer_policy_sha256": "e" * 64,
            "symbol_incomplete_files": 0,
        }
    )
    metrics["phases_ms"]["symbol_preflight"] = 0.0
    for row in metrics["symbol_coverage"]:
        if row["language"] is None:
            row.update(
                {
                    "coverage": {"state": "unsupported"},
                    "failure": "unsupported_language",
                    "definition_count": None,
                }
            )
        else:
            row.update(
                {
                    "coverage": {"state": "complete", "symbol_count": row["definition_count"]},
                    "failure": None,
                }
            )
    if any(row["language"] is None for row in metrics["symbol_coverage"]):
        metrics["symbol_coverage_policy"] = "allow-incomplete"
        metrics["symbol_incomplete_files"] = sum(
            row["language"] is None for row in metrics["symbol_coverage"]
        )
    return metrics


def _bind_preflight_fixture(phase_path, repository_commit):
    metrics = _current_symbol_metrics(json.loads(phase_path.read_text()))
    policy = {
        "max_file_bytes": 1048576,
        "max_symbols_per_file": 100000,
        "max_symbols_total": 1000000,
        "max_diagnostics_per_file": 32,
        "max_diagnostics_total": 1024,
        "timeout_per_file_ns": "10000000000",
        "timeout_total_ns": "120000000000",
    }
    files = [
        {
            **{key: row[key] for key in pairrun.symbol_coverage.ROW_KEYS},
            "failure_detail": None,
            "failure_detail_truncated": False,
            "diagnostics": [],
            "diagnostics_total": 0,
            "diagnostics_truncated": False,
            "diagnostics_complete": True,
        }
        for row in metrics["symbol_coverage"]
    ]
    for row in files:
        if row["language"] is None:
            row.update(
                {
                    "failure_detail": "unsupported symbol language: " + row["path"],
                    "diagnostics": [
                        {"kind": "unsupported_language", "byte_start": None, "byte_end": None}
                    ],
                    "diagnostics_total": 1,
                }
            )
    metrics["symbol_unsupported_details"] = [row for row in files if row["language"] is None]
    metrics["symbol_unsupported_files"] = len(metrics["symbol_unsupported_details"])
    metrics["symbol_producer_policy_sha256"] = pairrun.symbol_coverage.policy_digest(policy)
    universe = b"".join(
        row["path"].encode() + b"\0" + row["source_sha256"].encode() + b"\0" for row in files
    )
    report = {
        "symbol_coverage_policy": metrics["symbol_coverage_policy"],
        "repository_commit": repository_commit,
        "file_universe_sha256": hashlib.sha256(universe).hexdigest(),
        "preflight": {
            "schema": "symbol-preflight-v1",
            "producer_identity": pairrun.QUANTA_SYMBOL_PRODUCER_IDENTITY,
            "grammar_identity": pairrun.QUANTA_SYMBOL_GRAMMARS,
            "lockfile_sha256": pairrun.sha_file(pairrun.symbol_coverage.ROOT / "Cargo.lock"),
            "producer_policy_sha256": metrics["symbol_producer_policy_sha256"],
            "policy": policy,
            "files": files,
            "admitted_files": len(files),
            "incomplete_files": metrics["symbol_incomplete_files"],
        },
    }
    artifact = phase_path.with_name("symbol-preflight.json")
    artifact.write_text(json.dumps(report) + "\n")
    metrics["symbol_preflight_sha256"] = pairrun.sha_file(artifact)
    phase_path.write_text(json.dumps(metrics) + "\n")
    return artifact


def test_symbol_preflight_rejects_missing_tampered_stale_and_partial_evidence(tmp_path):
    phase = tmp_path / "phase.json"
    corpus = {
        "repository_commit": "a" * 40,
        "files": [
            {"path": "a.rs", "file_sha256": "b" * 64},
            {"path": "b.tsx", "file_sha256": "c" * 64},
        ],
    }
    phase.write_text(
        json.dumps(
            {
                "file_count": 2,
                "symbol_count": 1,
                "symbol_only_scopes": 0,
                "symbol_producer_identity": pairrun.QUANTA_SYMBOL_PRODUCER_IDENTITY,
                "symbol_grammars": pairrun.QUANTA_SYMBOL_GRAMMARS,
                "phases_ms": {},
                "symbol_coverage": [
                    {
                        "path": "a.rs",
                        "source_sha256": "b" * 64,
                        "language": "rust",
                        "definition_count": 1,
                    },
                    {
                        "path": "b.tsx",
                        "source_sha256": "c" * 64,
                        "language": "typescript_tsx",
                        "definition_count": 0,
                    },
                ],
            }
        )
    )
    artifact = _bind_preflight_fixture(phase, corpus["repository_commit"])
    metrics = json.loads(phase.read_text())
    raw = artifact.read_bytes()
    verify = pairrun.symbol_coverage.verify_artifact
    assert verify(metrics, phase, corpus) == artifact
    for mutation in (
        "missing",
        "symlink",
        "bytes",
        "duplicate_json",
        "reorder",
        "partial",
        "duplicate_row",
        "wrong_commit",
        "wrong_universe",
        "wrong_hash",
        "wrong_grammar",
        "wrong_lock",
        "changed_policy",
        "missing_policy",
        "unknown_state",
        "boolean_count",
        "bool_file_count",
        "false_complete",
        "truncated",
        "wrong_language",
        "parent_path",
        "absolute_path",
        "non_string_state",
        "deep_json",
    ):
        candidate = copy.deepcopy(metrics)
        report = json.loads(raw)
        if mutation == "missing":
            artifact.unlink()
        elif mutation == "symlink":
            artifact.unlink()
            target = tmp_path / "other.json"
            target.write_bytes(raw)
            artifact.symlink_to(target)
        elif mutation == "bytes":
            artifact.write_bytes(raw + b" ")
        elif mutation == "parent_path":
            candidate["symbol_preflight_out"] = "../symbol-preflight.json"
        elif mutation == "absolute_path":
            candidate["symbol_preflight_out"] = str(artifact.resolve())
        elif mutation == "bool_file_count":
            candidate["file_count"] = True
        else:
            files = report["preflight"]["files"]
            if mutation == "reorder":
                files.reverse()
            elif mutation == "partial":
                files.pop()
            elif mutation == "duplicate_row":
                files[1] = copy.deepcopy(files[0])
            elif mutation == "wrong_commit":
                report["repository_commit"] = "f" * 40
            elif mutation == "wrong_universe":
                report["file_universe_sha256"] = "f" * 64
            elif mutation == "wrong_hash":
                files[0]["source_sha256"] = "f" * 64
            elif mutation == "wrong_grammar":
                report["preflight"]["grammar_identity"] += ";forged"
            elif mutation == "wrong_lock":
                report["preflight"]["lockfile_sha256"] = "f" * 64
            elif mutation == "changed_policy":
                report["preflight"]["policy"]["max_symbols_total"] += 1
            elif mutation == "missing_policy":
                del report["preflight"]["policy"]["max_symbols_total"]
            elif mutation == "unknown_state":
                files[0]["coverage"] = {"state": "producer_failed"}
            elif mutation == "non_string_state":
                files[0]["coverage"] = {"state": []}
            elif mutation == "boolean_count":
                files[0]["coverage"]["symbol_count"] = True
            elif mutation == "false_complete":
                files[0]["failure"] = "syntax_error"
            elif mutation == "truncated":
                files[0]["diagnostics_complete"] = False
            elif mutation == "wrong_language":
                files[1]["language"] = "typescript"
            encoded = json.dumps(report).encode()
            if mutation == "duplicate_json":
                encoded = b'{"repository_commit":"' + b"a" * 40 + b'",' + encoded[1:]
            elif mutation == "deep_json":
                encoded = b"[" * 10000 + b"0" + b"]" * 10000
            artifact.write_bytes(encoded)
            candidate["symbol_preflight_sha256"] = pairrun.sha_file(artifact)
        with pytest.raises((ValueError, OSError), match="."):
            verify(candidate, phase, corpus)
        if artifact.is_symlink():
            artifact.unlink()
        artifact.write_bytes(raw)
    assert verify(metrics, phase, corpus) == artifact


def _complete_preflight_case(directory, path="a.rs"):
    phase = directory / "phase.json"
    corpus = {
        "repository_commit": "a" * 40,
        "files": [
            {"path": path, "file_sha256": "b" * 64},
        ],
    }
    phase.write_text(
        json.dumps(
            {
                "file_count": 1,
                "symbol_count": 0,
                "symbol_only_scopes": 0,
                "symbol_producer_identity": pairrun.QUANTA_SYMBOL_PRODUCER_IDENTITY,
                "symbol_grammars": pairrun.QUANTA_SYMBOL_GRAMMARS,
                "phases_ms": {},
                "symbol_coverage": [
                    {
                        "path": path,
                        "source_sha256": "b" * 64,
                        "language": "rust",
                        "definition_count": 0,
                    },
                ],
            }
        )
    )
    artifact = _bind_preflight_fixture(phase, corpus["repository_commit"])
    return json.loads(phase.read_text()), phase, corpus, artifact


@pytest.mark.parametrize(
    "path",
    [
        "C:/a.rs",
        "z:a.rs",
        "a\x7f.rs",
        "a\x80.rs",
        "a\x9f.rs",
        "a" * 4094 + ".rs",
        "가" * 1365 + ".rs",
    ],
    ids=[
        "absolute-drive",
        "relative-drive",
        "del",
        "c1-start",
        "c1-end",
        "ascii-byte-limit",
        "utf8-byte-limit",
    ],
)
def test_symbol_preflight_rejects_paths_outside_producer_contract(tmp_path, path):
    # Rebind every digest and census to the candidate: path syntax itself must
    # match ExactRepoRelativePathV1, independently of matching artifact hashes.
    metrics, phase, corpus, _ = _complete_preflight_case(tmp_path, path)
    with pytest.raises(ValueError, match="path"):
        pairrun.symbol_coverage.verify_artifact(metrics, phase, corpus)


def test_symbol_preflight_accepts_exact_path_byte_boundary_and_unicode(tmp_path):
    for path in ["src/검색.rs", "가" * 1364 + "a.rs", "a" * 4093 + ".rs", "ab:c.rs"]:
        metrics, phase, corpus, artifact = _complete_preflight_case(tmp_path, path)
        assert pairrun.symbol_coverage.verify_artifact(metrics, phase, corpus) == artifact


def test_symbol_preflight_rejects_ancestor_symlink(tmp_path):
    metrics, phase, corpus, artifact = _complete_preflight_case(tmp_path)
    alias = tmp_path / "alias"
    alias.symlink_to(tmp_path, target_is_directory=True)
    with pytest.raises(ValueError, match="symlink"):
        pairrun.symbol_coverage.verify_artifact(metrics, alias / phase.name, corpus)
    assert pairrun.symbol_coverage.verify_artifact(metrics, phase, corpus) == artifact


def test_symbol_preflight_rejects_replaced_file_during_admission(tmp_path, monkeypatch):
    metrics, phase, corpus, artifact = _complete_preflight_case(tmp_path)
    raw = artifact.read_bytes()
    lstat = Path.lstat
    replaced = False

    def replace_after_stat(path, *args, **kwargs):
        nonlocal replaced
        info = lstat(path, *args, **kwargs)
        if path == artifact and not replaced:
            replaced = True
            replacement = artifact.with_suffix(".replacement")
            replacement.write_bytes(raw)
            replacement.replace(artifact)
        return info

    monkeypatch.setattr(Path, "lstat", replace_after_stat)
    with pytest.raises(ValueError, match="changed"):
        pairrun.symbol_coverage.verify_artifact(metrics, phase, corpus)
    assert replaced


def test_symbol_preflight_budget_cannot_be_bypassed_by_file_growth(tmp_path, monkeypatch):
    metrics, phase, corpus, artifact = _complete_preflight_case(tmp_path)
    raw = artifact.read_bytes()
    budget = 64 * 1024 * 1024
    # Whitespace preserves a valid report. The limit is inclusive and applies
    # to actual consumed bytes, including bytes appended after path inspection.
    at_limit = raw + b" " * (budget - len(raw))
    artifact.write_bytes(at_limit)
    metrics["symbol_preflight_sha256"] = hashlib.sha256(at_limit).hexdigest()
    assert pairrun.symbol_coverage.verify_artifact(metrics, phase, corpus) == artifact
    oversized = at_limit + b" "
    artifact.write_bytes(raw)
    metrics["symbol_preflight_sha256"] = hashlib.sha256(oversized).hexdigest()
    lstat = Path.lstat
    grown = False

    def grow_after_stat(path, *args, **kwargs):
        nonlocal grown
        info = lstat(path, *args, **kwargs)
        if path == artifact and not grown:
            grown = True
            artifact.write_bytes(oversized)
        return info

    monkeypatch.setattr(Path, "lstat", grow_after_stat)
    with pytest.raises(ValueError, match="budget|changed"):
        pairrun.symbol_coverage.verify_artifact(metrics, phase, corpus)
    assert grown


def test_symbol_preflight_enforces_diagnostic_retention_policy(tmp_path):
    phase = tmp_path / "phase.json"
    corpus = {
        "repository_commit": "a" * 40,
        "files": [
            {"path": "a.txt", "file_sha256": "b" * 64},
            {"path": "b.txt", "file_sha256": "c" * 64},
        ],
    }
    phase.write_text(
        json.dumps(
            {
                "file_count": 2,
                "symbol_count": 0,
                "symbol_only_scopes": 0,
                "symbol_producer_identity": pairrun.QUANTA_SYMBOL_PRODUCER_IDENTITY,
                "symbol_grammars": pairrun.QUANTA_SYMBOL_GRAMMARS,
                "phases_ms": {},
                "symbol_coverage": [
                    {
                        "path": row["path"],
                        "source_sha256": row["file_sha256"],
                        "language": None,
                        "definition_count": None,
                    }
                    for row in corpus["files"]
                ],
            }
        )
    )
    artifact = _bind_preflight_fixture(phase, corpus["repository_commit"])
    original = json.loads(artifact.read_text())
    metrics = json.loads(phase.read_text())

    def check(report):
        candidate = copy.deepcopy(metrics)
        candidate["symbol_unsupported_details"] = copy.deepcopy(report["preflight"]["files"])
        policy_sha = pairrun.symbol_coverage.policy_digest(report["preflight"]["policy"])
        candidate["symbol_producer_policy_sha256"] = policy_sha
        report["preflight"]["producer_policy_sha256"] = policy_sha
        artifact.write_text(json.dumps(report))
        candidate["symbol_preflight_sha256"] = pairrun.sha_file(artifact)
        return pairrun.symbol_coverage.verify_artifact(candidate, phase, corpus)

    assert check(copy.deepcopy(original)) == artifact
    # Both files are unsupported: each has exactly one diagnostic. An explicit
    # shared limit of one retains the first and truthfully truncates the second.
    bounded = copy.deepcopy(original)
    bounded["preflight"]["policy"]["max_diagnostics_total"] = 1
    bounded["preflight"]["files"][1].update(diagnostics=[], diagnostics_truncated=True)
    assert check(bounded) == artifact
    for mutation in ("omit_with_capacity", "invent_diagnostics", "skip_first_file"):
        forged = copy.deepcopy(original)
        files = forged["preflight"]["files"]
        if mutation == "invent_diagnostics":
            files[0]["diagnostics"].append(copy.deepcopy(files[0]["diagnostics"][0]))
            files[0]["diagnostics_total"] = 2
        else:
            files[0].update(diagnostics=[], diagnostics_truncated=True)
            if mutation == "skip_first_file":
                forged["preflight"]["policy"]["max_diagnostics_total"] = 1
        with pytest.raises(ValueError, match="diagnostic"):
            check(forged)


def test_pair_binds_symbol_preflight_inventory_and_declared_capability(tmp_path):
    st = _pair_stage(tmp_path)
    assert _stage_verdict(st)["states"]["PAIR_VALID"] == "pass"
    layout = st["rep_layouts"][0]
    phase_path = Path(layout["quanta"]["whole_file"]).parent / "phase-metrics.json"
    artifact = phase_path.parent / "symbol-preflight.json"
    original = artifact.read_bytes()
    artifact.unlink()
    with pytest.raises(pairrun.RunError, match="symbol_preflights artifact is missing"):
        _stage_verdict(st)
    artifact.write_bytes(original)
    protocol = st["stage"] / "protocol-lock.json"
    value = json.loads(protocol.read_text())
    original_policy = value["symbol_coverage_policy"]
    assert original_policy == "allow-incomplete"
    value["symbol_coverage_policy"] = "require-complete"
    protocol.write_text(json.dumps(value))
    assert _stage_verdict(st)["states"]["PAIR_VALID"] == "fail"
    value["symbol_coverage_policy"] = original_policy
    protocol.write_text(json.dumps(value))
    assert _stage_verdict(st)["states"]["PAIR_VALID"] == "pass"
    # A malformed state inside the raw report must become a failed pair verdict,
    # even when the outer phase digest has been updated to those exact bytes.
    malformed = json.loads(original)
    malformed["preflight"]["files"][0]["coverage"] = {"state": []}
    for encoded in (json.dumps(malformed).encode(), b"[" * 10000 + b"0" + b"]" * 10000):
        artifact.write_bytes(encoded)
        phase = json.loads(phase_path.read_text())
        phase["symbol_preflight_sha256"] = pairrun.sha_file(artifact)
        phase_path.write_text(json.dumps(phase))
        assert _stage_verdict(st)["states"]["PAIR_VALID"] == "fail"


def test_parity_reference_refuses_unpinned_assets_and_library(tmp_path, monkeypatch):
    names = tuple(parity_reference.PINNED_ASSET_SHA256)
    for name in names:
        (tmp_path / name).write_bytes(name.encode())
    digests = {name: hashlib.sha256(name.encode()).hexdigest() for name in names}
    monkeypatch.setattr(parity_reference.importlib.metadata, "version", lambda _: "0.9.0")
    monkeypatch.setattr(parity_reference, "PINNED_ASSET_SHA256", digests)
    assert parity_reference.verify_reference_inputs(tmp_path) == digests
    for name in names:
        monkeypatch.setattr(parity_reference, "PINNED_ASSET_SHA256", digests | {name: "0" * 64})
        with pytest.raises(ValueError, match="SHA-256 mismatch"):
            parity_reference.verify_reference_inputs(tmp_path)
    monkeypatch.setattr(parity_reference.importlib.metadata, "version", lambda _: "0.9.1")
    with pytest.raises(ValueError, match="model2vec version mismatch"):
        parity_reference.verify_reference_inputs(tmp_path)


def test_parity_reference_rejects_partial_nonfinite_or_zero_vectors():
    for vector in (
        [0.0] * 256,
        [1.0] * 255,
        [1.0] * 257,
        [float("nan")] + [1.0] * 255,
        [float("inf")] + [1.0] * 255,
    ):
        with pytest.raises(ValueError, match="reference vector"):
            parity_reference.l2_normalize(vector)
    assert parity_reference.l2_normalize([1.0] * 256) == [0.0625] * 256


def test_parity_reference_v2_input_envelope():
    at_text_limit = "x" * parity_reference.FULL_V2_MAX_TEXT_BYTES
    parity_reference.validate_v2_input_envelope([at_text_limit])
    with pytest.raises(ValueError, match="text exceeds"):
        parity_reference.validate_v2_input_envelope([at_text_limit + "x"])
    at_batch_limit = [at_text_limit] * (
        parity_reference.FULL_V2_MAX_BATCH_TEXT_BYTES // parity_reference.FULL_V2_MAX_TEXT_BYTES
    )
    parity_reference.validate_v2_input_envelope(at_batch_limit)
    with pytest.raises(ValueError, match="batch exceeds"):
        parity_reference.validate_v2_input_envelope(at_batch_limit + [at_text_limit])
    parity_reference.validate_v2_input_envelope(["x" * 4096] * 1024)


def test_current_v7_pair_replays_nested_indexing_stages_and_phase_contract(tmp_path):
    stage = _pair_stage(tmp_path, diagnostic_version=7)
    diagnostic_path = next(
        stage["stage"].glob("rep-00/quanta/strategy-*/retrieval-diagnostic.json")
    )
    diagnostic = json.loads(diagnostic_path.read_text())
    phase = json.loads(diagnostic_path.with_name("phase-metrics.json").read_text())
    protocol = json.loads((stage["stage"] / "protocol-lock.json").read_text())
    assert protocol["lock_version"] == 5
    assert phase["schema_version"] == 3
    assert phase["phases_ms"]["daemon_boot_and_readiness"] == 1.0
    assert "model_provider_prepare" not in phase["phases_ms"]
    assert diagnostic["ingest"]["observation"]["lexical_stages"]["seal_ns"] == 40
    pairrun._validate_phase_metrics(phase, "current phase fixture")
    assert _stage_verdict(stage)["states"]["PAIR_VALID"] == "pass"


def test_retrieval_diagnostic_v5_binds_actual_server_observation_policy(tmp_path):
    stage = _pair_stage(tmp_path)
    path = next(stage["stage"].glob("rep-00/quanta/strategy-*/retrieval-diagnostic.json"))
    record_path = path.with_name("record.json")
    record = json.loads(record_path.read_text())
    suite = json.loads(stage["suite_path"].read_text())
    pack = json.loads((stage["stage"] / "query-pack.json").read_text())
    pack, _ = pairrun.project_pack_and_suite(pack, suite, sorted(record["route_provenance"]))
    diagnostic = json.loads(path.read_text())
    diagnostic["schema_version"] = 5
    diagnostic["server_observation"] = pairrun.server_observation_configuration("enabled")
    diagnostic["ingest"] = _diagnostic_ingest_fixture(record)
    record_sha = ev.digest(record_path.read_bytes())
    pairrun.validate_retrieval_diagnostic(diagnostic, record, record_sha, pack)
    disabled = json.loads(json.dumps(diagnostic))
    disabled["server_observation"] = pairrun.server_observation_configuration("disabled")
    for row in disabled["results"]:
        row["response"]["explanation"]["stage_timings"] = None
    pairrun.validate_retrieval_diagnostic(disabled, record, record_sha, pack)
    for mutate, match in (
        (lambda value: value.pop("server_observation"), "must hold exactly"),
        (lambda value: value["server_observation"].update(config_sha256="0" * 64), "digest/scope"),
        (
            lambda value: value["server_observation"].update(query_stages="unknown"),
            "exactly enabled",
        ),
        (
            lambda value: value["results"][0]["response"]["explanation"].update(stage_timings=[]),
            "unmeasured",
        ),
        (
            lambda value: value["results"][0]["response"]["explanation"].update(request_id=0),
            "transport request",
        ),
    ):
        forged = json.loads(json.dumps(disabled))
        mutate(forged)
        with pytest.raises(pairrun.RunError, match=match):
            pairrun.validate_retrieval_diagnostic(forged, record, record_sha, pack)
    contradictory = json.loads(json.dumps(disabled))
    contradictory["server_observation"] = diagnostic["server_observation"]
    with pytest.raises(pairrun.RunError, match="stage timings"):
        pairrun.validate_retrieval_diagnostic(contradictory, record, record_sha, pack)


def _diagnostic_ingest_fixture(record):
    roots = {
        "row_root_digest": "sha256:" + "a" * 64,
        "membership_root_digest": "sha256:" + "b" * 64,
    }
    receipt = {
        "generation": 7,
        "manifest_digest": "manifest:fixture",
        "batch_digest": "c" * 64,
        "accepted_replace_scopes": 1,
        "accepted_tombstone_scopes": 0,
        "accepted_semantic_replace_scopes": 1,
        "accepted_semantic_tombstone_scopes": 0,
        "accepted_clear_surfaces": 0,
        "sealed": True,
        "applied": True,
        "durable_sequence": 1,
        "semantic_content": roots,
    }
    scope = {
        "repo_id": "bench-repo",
        "revision_id": "bench-rev",
        "manifest_generation": 7,
        "manifest_digest": receipt["manifest_digest"],
    }
    ack = {
        "active": {
            "generation": {
                "lexical": {**scope, "track": "Lexical"},
                "semantic": {**scope, "track": "Semantic"},
                "semantic_content": roots,
            },
            "activation_token": {"root_incarnation": [1] * 16, "activation_sequence": 1},
        },
        "previous_sealed_active": None,
    }
    report = {
        key: 0
        for key in (
            "owner_scopes",
            "windows",
            "semantic_delete_calls",
            "semantic_delete_commits",
            "membership_delete_calls",
            "membership_delete_commits",
            "semantic_append_calls",
            "membership_append_calls",
        )
    }
    report["durations"] = {
        key: 0
        for key in (
            "total",
            "prepare",
            "promotion",
            "clear_surfaces",
            "stream",
            "semantic_delete",
            "membership_delete",
            "semantic_append",
            "membership_append",
            "tombstones",
            "seal",
        )
    }
    report["durations"]["embedding"] = None
    observation = {
        "request_id": 9,
        "repo_id": "bench-repo",
        "revision_id": "bench-rev",
        "generation": 7,
        "batch_digest": receipt["batch_digest"],
        "status": "executed",
        "semantic": report,
        "lexical_build_ns": 0,
        "finalize_ns": 0,
        "activation_ns": None,
    }
    for capture in record["captures"].values():
        capture.update(
            generation=7,
            receipt_digest=pairrun.digest(pairrun.canonical_bytes(receipt)),
            activation_digest=pairrun.digest(pairrun.canonical_bytes(ack["active"])),
        )
    return {"receipt": receipt, "activation_ack": ack, "observation": observation}


def test_retrieval_diagnostic_v5_ingest_rejects_unbound_partial_replayed_or_forged_measurements():
    record = {"captures": {"capture": {}}}
    raw = _diagnostic_ingest_fixture(record)
    assert pairrun._validate_ingest_diagnostic(raw, record) == pairrun.ingest_request_identity({})
    for mutate in (
        lambda x: x.pop("observation"),
        lambda x: x["observation"].pop("activation_ns"),
        lambda x: x["observation"].update(request_id=0),
        lambda x: x["observation"].update(request_id=True),
        lambda x: x["observation"].update(repo_id="other"),
        lambda x: x["observation"].update(revision_id="other"),
        lambda x: x["observation"].update(generation=8),
        lambda x: x["observation"].update(generation=True),
        lambda x: x["observation"].update(batch_digest="f" * 64),
        lambda x: x["observation"].update(status="replayed"),
        lambda x: x["observation"].update(status="partial_recovery"),
        lambda x: x["observation"].update(semantic=None),
        lambda x: x["observation"].update(lexical_build_ns=None),
        lambda x: x["observation"].update(finalize_ns=None),
        lambda x: x["observation"].update(activation_ns=0),
        lambda x: x["observation"]["semantic"]["durations"].update(seal=None),
        lambda x: x["observation"]["semantic"]["durations"].update(prepare=1),
        lambda x: x["observation"]["semantic"]["durations"].update(embedding=1),
        lambda x: x["observation"]["semantic"]["durations"].update(semantic_delete=1),
        lambda x: x["observation"]["semantic"]["durations"].update(stream=-1),
        lambda x: x["observation"]["semantic"]["durations"].update(stream=float("nan")),
        lambda x: x["observation"]["semantic"].update(windows=2**64),
        lambda x: x["receipt"].update(applied=False),
        lambda x: x["activation_ack"]["active"]["generation"]["semantic"].update(
            manifest_generation=8
        ),
    ):
        forged = json.loads(json.dumps(raw))
        mutate(forged)
        with pytest.raises(pairrun.RunError):
            pairrun._validate_ingest_diagnostic(forged, record)

    forged = json.loads(json.dumps(raw))
    forged["activation_ack"]["active"]["activation_token"]["activation_sequence"] = 2
    rebound = json.loads(json.dumps(record))
    rebound["captures"]["capture"]["activation_digest"] = pairrun.digest(
        pairrun.canonical_bytes(forged["activation_ack"]["active"])
    )
    with pytest.raises(pairrun.RunError, match="sequence must be one"):
        pairrun._validate_ingest_diagnostic(forged, rebound)
    for lane in ("lexical", "semantic"):
        forged = json.loads(json.dumps(raw))
        forged["activation_ack"]["active"]["generation"][lane]["track"] = lane
        rebound = json.loads(json.dumps(record))
        rebound["captures"]["capture"]["activation_digest"] = pairrun.digest(
            pairrun.canonical_bytes(forged["activation_ack"]["active"])
        )
        with pytest.raises(pairrun.RunError, match="identity/fresh receipt/activation"):
            pairrun._validate_ingest_diagnostic(forged, rebound)


def test_ingest_lexical_stage_intervals_reject_overcount_and_missing_measurements():
    record = {"captures": {"capture": {}}}
    raw = _diagnostic_ingest_fixture(record)
    raw["observation"]["lexical_build_ns"] = 100
    stages = {
        "preparation_ns": 10,
        "writer_mutation_ns": 20,
        "text_authority_ns": 10,
        "file_authority_ns": 10,
        "seal_ns": 40,
        "seal_writer_commit_ns": 10,
        "seal_merge_wait_ns": 10,
        "seal_commitment_ns": 15,
        "seal_file_admission_ns": 5,
    }
    raw["observation"]["lexical_stages"] = stages
    pairrun._validate_ingest_diagnostic(raw, record, lexical_stage_contract=True)
    for field, value in (
        ("preparation_ns", 101),
        ("seal_writer_commit_ns", 40),
        ("seal_file_admission_ns", 16),
        ("seal_ns", None),
        ("writer_mutation_ns", True),
        ("file_authority_ns", 2**64),
    ):
        changed = copy.deepcopy(raw)
        changed["observation"]["lexical_stages"][field] = value
        with pytest.raises(pairrun.RunError, match="ingest lexical stages"):
            pairrun._validate_ingest_diagnostic(changed, record, lexical_stage_contract=True)
    for value in (None, {}, {**stages, "unsupported": 1}):
        changed = copy.deepcopy(raw)
        changed["observation"]["lexical_stages"] = value
        with pytest.raises(pairrun.RunError):
            pairrun._validate_ingest_diagnostic(changed, record, lexical_stage_contract=True)


def test_query_plan_oracle_uses_nfc_and_rejects_unindexable_runs():
    composed = qp.plan_lexical_request("natural_language", "caf\u00e9")
    decomposed = qp.plan_lexical_request("natural_language", "cafe\u0301")
    assert composed == decomposed == 'case:no caf\u00e9'
    assert qp.plan_lexical_request("natural_language", "foo --- bar") == 'case:no foo OR bar'
    with pytest.raises(qp.QueryPlanError, match="no tokens"):
        qp.plan_lexical_request("natural_language", "--- ... ///")


def test_query_plan_oracle_enforces_utf8_term_boundary():
    assert qp.plan_lexical_request("natural_language", "\uac00" * 85)
    with pytest.raises(qp.QueryPlanError, match="258 bytes"):
        qp.plan_lexical_request("natural_language", "\uac00" * 86)


def test_exact_symbol_name_policy_keeps_bare_query_identity_and_refuses_dsl():
    request = qp.plan_lexical_request("exact_symbol_name", "writeContentType")
    assert request == "symbol.local_name.exact(writeContentType) case:yes"
    profile = qp.execution_profile("exact_symbol_name")
    jsonschema.validate(profile, _load_schema("runner.schema.json")["$defs"]["execution_profile"])
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(
            profile, _load_schema("pair-spec.schema.json")["$defs"]["quanta_profile"]
        )
    identity = qp.derive_query_identity("exact_symbol_name", "writeContentType")
    assert identity["original_query_sha256"] == hashlib.sha256(b"writeContentType").hexdigest()
    assert (
        identity["effective_lexical_request_sha256"] == hashlib.sha256(request.encode()).hexdigest()
    )
    for name in ("OR", "AND", "case", "select", "_", "a" * 4096):
        assert qp.plan_lexical_request("exact_symbol_name", name) == (
            f"symbol.local_name.exact({name}) case:yes"
        )
    for invalid in ("", "two words", "select:file Next", "WriteContentType)", "\u00e9", "a" * 4097):
        with pytest.raises(qp.QueryPlanError, match="bare ASCII symbol name"):
            qp.plan_lexical_request("exact_symbol_name", invalid)
    with pytest.raises(qp.QueryPlanError, match="unsupported v4"):
        qp.derive_query_identity_v4("exact_symbol_name", "writeContentType")


def test_retrieval_diagnostic_binds_complete_record_and_lanes():
    pack = {"tasks": [{"task_id": "T1", "query_sha256": "a" * 64}]}
    pack_sha = pairrun.digest(pairrun.canonical_bytes(pack))
    record = {
        "query_pack_sha256": pack_sha,
        "comparison_contract": {"top_k": 10},
        "route_provenance": {"hybrid": {"capture_id": "capture"}},
        "results": [
            {
                "task_id": "T1",
                "route": "hybrid",
                "status": "success",
                "error": None,
                "candidates": [
                    {
                        "rank": 1,
                        "path": "src/lib.rs",
                        "start_line": 1,
                        "end_line": 3,
                    }
                ],
            }
        ],
    }
    row = {
        "task_id": "T1",
        "query_sha256": "a" * 64,
        "route": "hybrid",
        "status": "success",
        "error_code": None,
        "candidates": [
            {
                "rank": 1,
                "candidate_id": "chunk-1",
                "path": "src/lib.rs",
                "start_line": 1,
                "end_line": 3,
                "score": 0.02,
                "contributions": [{"lane": "lexical", "rank": 2, "raw_score": 3.0}],
            }
        ],
        "response": {
            "request_id": 7,
            "early_stop_reason": "count_reached",
            "engines_executed": ["lexical", "semantic"],
            "engines_touched": ["lexical", "semantic"],
            "strategy": "hybrid-rrf",
            "window_returned": 1,
            "window_candidate_count": {"kind": "exact", "count": 1},
            "lane_traces": [
                {
                    "lane": "lexical",
                    "executed": True,
                    "contributed": True,
                    "candidates": {"kind": "exact", "count": 12},
                    "filtered_out": 3,
                    "cost": 40,
                    "profile": "bm25",
                },
                # An executed zero-hit lane stays distinct from a lane that
                # never ran (RBR-01).
                {
                    "lane": "dense",
                    "executed": True,
                    "contributed": False,
                    "candidates": {"kind": "at_least", "count": 64},
                    "filtered_out": None,
                    "cost": None,
                    "profile": None,
                },
            ],
        },
    }
    diagnostic = {
        "schema_version": 2,
        "kind": "quanta_returned_window_diagnostic",
        "record_sha256": "b" * 64,
        "query_pack_sha256": pack_sha,
        "top_k": 10,
        "scope": "returned_window_only",
        "results": [row],
        "runner_timing_detail_ms": {
            "clock": "runner_monotonic_wall_v1",
            "daemon_boot_and_readiness": 1.0,
            "sdk_publish_and_activate_opaque": 2.0,
            "runner_record_assembly": 0.1,
            "corpus_reverification": 0.2,
            "daemon_shutdown": 0.3,
        },
    }
    pairrun.validate_retrieval_diagnostic(diagnostic, record, "b" * 64, pack)

    for invalid in (10**400, -(10**400), float("nan"), float("inf"), -float("inf"), True, None):
        for field in ("score", "lane_score", "timing"):
            forged = json.loads(json.dumps(diagnostic))
            if field == "timing":
                forged["runner_timing_detail_ms"]["daemon_shutdown"] = invalid
            elif field == "lane_score":
                forged["results"][0]["candidates"][0]["contributions"][0]["raw_score"] = invalid
            else:
                forged["results"][0]["candidates"][0]["score"] = invalid
            with pytest.raises(pairrun.RunError):
                pairrun.validate_retrieval_diagnostic(forged, record, "b" * 64, pack)

    tampered = json.loads(json.dumps(diagnostic))
    tampered["results"][0]["candidates"][0]["contributions"] = []
    with pytest.raises(pairrun.RunError, match="lane provenance"):
        pairrun.validate_retrieval_diagnostic(tampered, record, "b" * 64, pack)
    tampered = json.loads(json.dumps(diagnostic))
    tampered["results"] = []
    with pytest.raises(pairrun.RunError, match="incomplete"):
        pairrun.validate_retrieval_diagnostic(tampered, record, "b" * 64, pack)
    with pytest.raises(pairrun.RunError, match="identity"):
        pairrun.validate_retrieval_diagnostic(diagnostic, record, "c" * 64, pack)
    tampered = json.loads(json.dumps(diagnostic))
    tampered["results"][0]["candidates"][0]["start_line"] = 2
    with pytest.raises(pairrun.RunError, match="candidate"):
        pairrun.validate_retrieval_diagnostic(tampered, record, "b" * 64, pack)
    tampered = json.loads(json.dumps(diagnostic))
    tampered["runner_timing_detail_ms"]["daemon_shutdown"] = float("nan")
    with pytest.raises(pairrun.RunError, match="timing"):
        pairrun.validate_retrieval_diagnostic(tampered, record, "b" * 64, pack)
    tampered = json.loads(json.dumps(diagnostic))
    tampered["results"][0]["response"]["lane_traces"][1]["contributed"] = True
    tampered["results"][0]["response"]["lane_traces"][1]["executed"] = False
    with pytest.raises(pairrun.RunError, match="lane_traces entry is invalid"):
        pairrun.validate_retrieval_diagnostic(tampered, record, "b" * 64, pack)
    tampered = json.loads(json.dumps(diagnostic))
    tampered["results"][0]["response"].update(
        {
            "request_id": None,
            "early_stop_reason": None,
            "engines_executed": None,
            "engines_touched": None,
            "strategy": None,
            "window_returned": None,
            "window_candidate_count": None,
            "lane_traces": [],
        }
    )
    pairrun.validate_retrieval_diagnostic(tampered, record, "b" * 64, pack)


def test_retrieval_diagnostic_v3_replays_typed_window_and_empty_non_exhausted():
    pack = {"tasks": [{"task_id": "T1", "query_sha256": "a" * 64}]}
    pack_sha = pairrun.digest(pairrun.canonical_bytes(pack))
    record = {
        "query_pack_sha256": pack_sha,
        "comparison_contract": {"top_k": 10},
        "route_provenance": {"hybrid": {"capture_id": "capture"}},
        "results": [
            {
                "task_id": "T1",
                "route": "hybrid",
                "status": "success",
                "error": None,
                "candidates": [{"rank": 1, "path": "src/lib.rs", "start_line": 1, "end_line": 3}],
            }
        ],
    }
    window = {
        "returned": 1,
        "candidate_count": {"kind": "exact", "value": 1},
        "outcome": {"kind": "exact_exhausted"},
        "coverage": {
            "examined": {"kind": "exact", "value": 1},
            "exhaustion_proof": {"kind": "exact_count", "total": 1},
            "lanes": [
                {
                    "lane": "lexical",
                    "executed": True,
                    "contributed": True,
                    "filtered_out": 0,
                    "candidates": {"kind": "exact", "value": 1},
                }
            ],
        },
    }
    diagnostic = {
        "schema_version": 3,
        "kind": "quanta_returned_window_diagnostic",
        "record_sha256": "b" * 64,
        "query_pack_sha256": pack_sha,
        "top_k": 10,
        "scope": "returned_window_only",
        "results": [
            {
                "task_id": "T1",
                "query_sha256": "a" * 64,
                "route": "hybrid",
                "status": "success",
                "error_code": None,
                "candidates": [
                    {
                        "rank": 1,
                        "candidate_id": "chunk-1",
                        "path": "src/lib.rs",
                        "start_line": 1,
                        "end_line": 3,
                        "score": 0.02,
                        "contributions": [{"lane": "lexical", "rank": 1, "raw_score": 3.0}],
                    }
                ],
                "response_kind": "returned_window",
                "response": {"window": window, "explanation": None},
            }
        ],
        "runner_timing_detail_ms": {
            "clock": "runner_monotonic_wall_v1",
            "daemon_boot_and_readiness": 1.0,
            "sdk_publish_and_activate_opaque": 2.0,
            "runner_record_assembly": 0.1,
            "corpus_reverification": 0.2,
            "daemon_shutdown": 0.3,
        },
    }
    pairrun.validate_retrieval_diagnostic(diagnostic, record, "b" * 64, pack)

    qualified = json.loads(json.dumps(diagnostic))
    qualified["results"][0]["response"]["window"]["coverage"]["lanes"][0]["lane"] = "hybrid.lexical"
    pairrun.validate_retrieval_diagnostic(qualified, record, "b" * 64, pack)

    measured = json.loads(json.dumps(qualified))
    measured["schema_version"] = 4
    explanation = {
        "request_id": 19,
        "early_stop_reason": None,
        "engines_executed": ["lexical", "semantic"],
        "engines_touched": ["lexical"],
        "strategy": "lexical_only",
        "stage_timings": [
            {
                "stage": f"hybrid.{name}",
                "elapsed_ns": 100,
                "calls": 1,
                "returned_candidates": (0 if name in ("dense_fetch", "dense_admission") else 1)
                if name in ("lexical_search", "dense_fetch", "dense_admission", "fusion")
                else None,
            }
            for name in (
                "prepare",
                "read_view",
                "lexical_search",
                "embedding",
                "dense_fetch",
                "dense_admission",
                "fusion",
            )
        ],
    }
    measured["results"][0]["response"]["explanation"] = explanation
    pairrun.validate_retrieval_diagnostic(measured, record, "b" * 64, pack)
    lexical_record = json.loads(json.dumps(record))
    lexical_record["results"][0]["route"] = "lexical"
    lexical_record["route_provenance"] = {"lexical": {"capture_id": "capture"}}
    lexical = json.loads(json.dumps(measured))
    lexical["results"][0]["route"] = "lexical"
    lexical["results"][0]["candidates"][0]["contributions"] = []
    lexical["results"][0]["response"]["window"]["coverage"]["lanes"][0]["lane"] = "lexical"
    lexical["results"][0]["response"]["explanation"] = {
        "request_id": 21,
        "early_stop_reason": None,
        "engines_executed": ["lexical"],
        "engines_touched": ["lexical"],
        "strategy": "lexical",
        "stage_timings": [
            {
                "stage": f"lexical.{name}",
                "elapsed_ns": 100,
                "calls": 1,
                "returned_candidates": 1 if name in ("search", "project") else None,
            }
            for name in ("prepare", "read_view", "search", "project")
        ],
    }
    pairrun.validate_retrieval_diagnostic(lexical, lexical_record, "b" * 64, pack)
    for mutate in (
        lambda value: value["results"][0]["response"]["explanation"]["stage_timings"].pop(2),
        lambda value: value["results"][0]["response"]["explanation"].update(engines_executed=[]),
        lambda value: value["results"][0]["response"]["explanation"]["stage_timings"][-1].update(
            returned_candidates=2
        ),
    ):
        tampered = json.loads(json.dumps(lexical))
        mutate(tampered)
        with pytest.raises(pairrun.RunError):
            pairrun.validate_retrieval_diagnostic(tampered, lexical_record, "b" * 64, pack)
    for mutate in (
        lambda value: value["results"][0]["response"].update(explanation=None),
        lambda value: value["results"][0]["response"]["explanation"].update(request_id=0),
        lambda value: value["results"][0]["response"]["explanation"].update(engines_executed=[]),
        lambda value: value["results"][0]["response"]["explanation"].update(engines_touched=[]),
        lambda value: value["results"][0]["response"]["explanation"].update(strategy="rrf"),
        lambda value: value["results"][0]["response"]["explanation"]["stage_timings"].pop(),
        lambda value: value["results"][0]["response"]["explanation"]["stage_timings"].reverse(),
        lambda value: value["results"][0]["response"]["explanation"]["stage_timings"][-1].update(
            returned_candidates=2
        ),
        lambda value: value["results"][0]["response"]["explanation"]["stage_timings"][0].update(
            elapsed_ns=-1
        ),
    ):
        tampered = json.loads(json.dumps(measured))
        mutate(tampered)
        with pytest.raises(pairrun.RunError):
            pairrun.validate_retrieval_diagnostic(tampered, record, "b" * 64, pack)

    unrelated = json.loads(json.dumps(qualified))
    unrelated["results"][0]["response"]["window"]["coverage"]["lanes"][0]["lane"] = "other.lexical"
    with pytest.raises(pairrun.RunError):
        pairrun.validate_retrieval_diagnostic(unrelated, record, "b" * 64, pack)

    tampered = json.loads(json.dumps(diagnostic))
    tampered["results"][0]["response"]["window"]["returned"] = 999
    with pytest.raises(
        pairrun.RunError, match="typed window|returned count|exact exhaustion|candidate count"
    ):
        pairrun.validate_retrieval_diagnostic(tampered, record, "b" * 64, pack)

    tampered = json.loads(json.dumps(diagnostic))
    lane = tampered["results"][0]["response"]["window"]["coverage"]["lanes"][0]
    lane["executed"] = False
    lane["contributed"] = False
    with pytest.raises(pairrun.RunError, match="did not execute"):
        pairrun.validate_retrieval_diagnostic(tampered, record, "b" * 64, pack)

    empty_record = json.loads(json.dumps(record))
    empty_record["results"][0].update(
        status="error",
        candidates=[],
        error={"code": "empty_non_exhausted_window", "message": "typed"},
    )
    empty = json.loads(json.dumps(diagnostic))
    empty["results"][0].update(
        status="error", error_code="empty_non_exhausted_window", candidates=[]
    )
    empty_window = empty["results"][0]["response"]["window"]
    empty_window.update(
        returned=0,
        candidate_count={"kind": "at_least", "value": 1},
        outcome={"kind": "lower_bound", "continuation": True},
        empty_provenance="zero_hit_executed",
    )
    empty_window["coverage"] = {
        "examined": {"kind": "at_least", "value": 1},
        "lanes": [
            {
                "lane": "lexical",
                "executed": True,
                "contributed": False,
                "filtered_out": 0,
                "candidates": {"kind": "exact", "value": 0},
            }
        ],
    }
    pairrun.validate_retrieval_diagnostic(empty, empty_record, "b" * 64, pack)

    forged_stale_record = json.loads(json.dumps(empty_record))
    forged_stale_record["results"][0]["error"] = {
        "code": "stale_generation",
        "message": "forged null response",
    }
    forged_stale = json.loads(json.dumps(empty))
    forged_stale["results"][0].update(
        error_code="stale_generation",
        response_kind="sdk_failure",
        response=None,
    )
    with pytest.raises(pairrun.RunError, match="SDK failure shape"):
        pairrun.validate_retrieval_diagnostic(forged_stale, forged_stale_record, "b" * 64, pack)

    rejected_record = json.loads(json.dumps(empty_record))
    rejected_record["captures"] = {"capture": {"generation": 7}}
    rejected_record["results"][0]["error"] = {
        "code": "stale_generation",
        "message": "typed",
    }
    rejected = json.loads(json.dumps(empty))
    rejected["results"][0].update(
        error_code="stale_generation",
        response_kind="rejected_response",
        response={
            "window": empty_window,
            "explanation": None,
            "observed_hit_count": 0,
            "expected_generation": {
                "repo_id": "repo",
                "revision_id": "revision",
                "manifest_generation": 7,
            },
            "observed_generation": {
                "repo_id": "repo",
                "revision_id": "revision",
                "manifest_generation": 8,
            },
        },
    )
    pairrun.validate_retrieval_diagnostic(rejected, rejected_record, "b" * 64, pack)
    rejected["results"][0]["response"]["expected_generation"]["manifest_generation"] = 6
    with pytest.raises(pairrun.RunError, match="bound to capture generation"):
        pairrun.validate_retrieval_diagnostic(rejected, rejected_record, "b" * 64, pack)


def test_retrieval_diagnostic_v4_semantic_stages_bind_execution_and_strategy():
    stages = [
        {
            "stage": f"semantic.{name}",
            "elapsed_ns": 100,
            "calls": 1,
            "returned_candidates": 1 if name in ("dense_search", "project") else None,
        }
        for name in ("prepare", "read_view", "embedding", "dense_search", "project")
    ]
    explanation = {
        "request_id": 22,
        "early_stop_reason": None,
        "engines_executed": ["semantic"],
        "engines_touched": ["semantic"],
        "strategy": "semantic",
        "stage_timings": stages,
    }
    pairrun._validate_explanation(explanation, "semantic fixture", "semantic", 4, 1)
    for field, value in (
        ("engines_executed", []),
        ("engines_touched", []),
        ("strategy", "semantic_scoped"),
    ):
        changed = json.loads(json.dumps(explanation))
        changed[field] = value
        with pytest.raises(pairrun.RunError, match="semantic stage execution"):
            pairrun._validate_explanation(changed, "semantic fixture", "semantic", 4, 1)

    scoped = json.loads(json.dumps(explanation))
    scoped["stage_timings"].insert(
        2,
        {
            "stage": "semantic.lexical_scope",
            "elapsed_ns": 100,
            "calls": 1,
            "returned_candidates": 2,
        },
    )
    scoped["engines_executed"] = ["lexical", "semantic"]
    scoped["engines_touched"] = ["lexical", "semantic"]
    scoped["strategy"] = "semantic_scoped"
    pairrun._validate_explanation(scoped, "semantic scoped fixture", "semantic", 4, 1)

    empty = json.loads(json.dumps(scoped))
    empty["stage_timings"][-2]["returned_candidates"] = 0
    empty["stage_timings"][-1]["returned_candidates"] = 0
    empty["engines_touched"] = ["lexical"]
    empty["strategy"] = "empty"
    pairrun._validate_explanation(empty, "semantic zero-hit fixture", "semantic", 4, 0)
    empty["engines_touched"] = ["lexical", "semantic"]
    with pytest.raises(pairrun.RunError, match="semantic stage execution"):
        pairrun._validate_explanation(empty, "semantic zero-hit fixture", "semantic", 4, 0)


def test_pair_record_identity_accepts_multi_route_quanta_and_rejects_mixed_captures():
    captures = {
        route: {"system": "quanta", "chunk_strategy": "fixed_window_strict"}
        for route in ("lexical", "semantic", "hybrid")
    }
    assert pairrun._record_identity({"captures": captures}, "record") == (
        "quanta",
        "fixed_window_strict",
    )

    mixed_strategy = json.loads(json.dumps(captures))
    mixed_strategy["semantic"]["chunk_strategy"] = "fixed_window"
    with pytest.raises(pairrun.RunError, match="mixes capture"):
        pairrun._record_identity({"captures": mixed_strategy}, "record")

    mixed_system = json.loads(json.dumps(captures))
    mixed_system["hybrid"]["system"] = "semble"
    with pytest.raises(pairrun.RunError, match="mixes capture"):
        pairrun._record_identity({"captures": mixed_system}, "record")

    with pytest.raises(pairrun.RunError, match="one capture"):
        pairrun._record_identity(
            {
                "captures": {
                    "a": {"system": "semble", "chunk_strategy": "fixed_window_strict"},
                    "b": {"system": "semble", "chunk_strategy": "fixed_window_strict"},
                }
            },
            "record",
        )


def test_symbol_route_records_validate_without_schema_forks(tmp_path):
    # RBR-05: the wire schemas keep routes as nonempty strings, so a
    # symbol-route record validates through the same generic path — no
    # route enum fork exists to drift.
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    suite["routes"] = ["symbol"]
    run["route_provenance"] = {"symbol": {"capture_id": "q0"}}
    kept = {}
    for row in run["results"]:
        kept[row["task_id"]] = dict(row, route="symbol")
    run["results"] = [kept[task["task_id"]] for task in suite["tasks"]]
    _, projected_pack, _ = ev.validate_suite(repo, suite)
    run["query_pack_sha256"] = ev.digest(ev.canonical(projected_pack))
    jsonschema.validate(run, _load_schema("runner.schema.json"))
    loaded_suite, _pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    assert loaded_suite["routes"] == ["symbol"]
    assert {row["route"] for row in loaded_run["results"]} == {"symbol"}
    assert loaded_run["route_provenance"] == {"symbol": {"capture_id": "q0"}}


def test_rank_report_refuses_primary_at_10_from_top_5(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    loaded = record_v3(repo, suite, run, suite_path, runner_path)
    narrow_suite = json.loads(json.dumps(loaded[0]))
    narrow_suite["comparison_contract"]["top_k"] = 5
    with pytest.raises(ev.EvidenceError, match="top_k >= 10"):
        ev.evaluate(narrow_suite, loaded[1], loaded[2], "lexical", "hybrid", strict_k=True)


def test_duplicate_json_keys_refused(tmp_path):
    path = tmp_path / "duplicate.json"
    path.write_text('{"schema_version":3,"schema_version":3}', encoding="utf-8")
    with pytest.raises(ev.EvidenceError, match="duplicate JSON key"):
        ev.read_json(path)


def test_current_cli_requires_recorded_runner(tmp_path, capsys):
    repo, suite, _run, suite_path, runner_path, _files = fixture_v3(tmp_path)
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    assert ev.main(["freeze", "--repo", str(repo), "--suite", str(suite_path)]) == 0
    pack = json.loads(capsys.readouterr().out)
    assert "gold" not in json.dumps(pack)
    assert "answerable" not in json.dumps(pack)
    assert (
        ev.main(
            [
                "evaluate",
                "--repo",
                str(repo),
                "--suite",
                str(suite_path),
                "--runner",
                str(runner_path),
                "--baseline-route",
                "lexical",
                "--candidate-route",
                "hybrid",
            ]
        )
        == 2
    )
    assert "cannot read JSON" in capsys.readouterr().err


def test_rank_prefix_does_not_skip_oversize_candidate():
    rows = [{"tokens": 3000}, {"tokens": 10}]
    assert ev.selected(rows, 2000) == ([], 0)


def test_token_unit_uses_explicit_ascii_ranges():
    assert ev.TOKEN_RE.findall("alpha_1 한글 !") == ["alpha_1", "한", "글", "!"]


# --- RB-01 current suite and scoring ---


def _write_repo(tmp_path: Path, files: dict[str, bytes]) -> tuple[Path, str]:
    repo = tmp_path / "source"
    repo.mkdir(parents=True, exist_ok=True)
    for name, data in files.items():
        (repo / name).write_bytes(data)
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    subprocess.run(["git", "-C", str(repo), "add", *files.keys()], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(repo),
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "frozen",
        ],
        check=True,
    )
    return repo, ev.git(repo, "rev-parse", "HEAD")


def _span_meta(data: bytes, start: int, end: int) -> tuple[str, str, int]:
    lines = data.splitlines(keepends=True)
    selected = b"".join(lines[start - 1 : end])
    tokens = len(ev.TOKEN_RE.findall(selected.decode("utf-8")))
    return ev.digest(data), ev.digest(selected), tokens


def _byte_span(data: bytes, start: int, end: int) -> tuple[int, int]:
    lines = data.splitlines(keepends=True)
    first = sum(len(line) for line in lines[: start - 1])
    return first, first + sum(len(line) for line in lines[start - 1 : end])


def test_current_freeze_pack_is_blind(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    _, pack, _ = ev.validate_suite(repo, suite)
    text = json.dumps(pack)
    assert "gold" not in text
    assert "grade" not in text
    assert "answerable" not in text
    assert pack["schema_version"] == ev.SCHEMA_VERSION
    assert pack["tokenizer_budget_version"] == ev.TOKENIZER_BUDGET_VERSION
    assert pack["file_universe"] == suite["file_universe"]
    assert all(set(t) == {"task_id", "query", "query_sha256"} for t in pack["tasks"])


def test_suite_intent_and_review_claims_remain_blind_and_require_review_evidence(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    _, original_pack, _ = ev.validate_suite(repo, suite)
    suite["diagnostic_policy"] = ev.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
    task = suite["tasks"][0]
    task["query_intent"] = "bare_symbol"
    task["label_review"] = {
        "assessment": "reviewed_ambiguous",
        "reviewer_id": "fixture-reviewer",
        "evidence_sha256": "a" * 64,
    }
    jsonschema.validate(
        suite, json.loads((Path(ev.__file__).with_name("suite.schema.json")).read_text())
    )
    _, pack, _ = ev.validate_suite(repo, suite)
    assert pack["suite_commitment_sha256"] != original_pack["suite_commitment_sha256"]
    assert ev.digest(ev.canonical(pack)) != run["query_pack_sha256"]
    assert "query_intent" not in json.dumps(pack)
    assert "label_review" not in json.dumps(pack)
    run["query_pack_sha256"] = ev.digest(ev.canonical(pack))
    report = ev.evaluate(*record_v3(repo, suite, run, suite_path, runner_path), "lexical", "hybrid")
    row = next(
        row for row in report["per_query"] if row["task_id"] == "T1" and row["route"] == "lexical"
    )
    assert row["query_intent_claim"] == "bare_symbol"
    assert row["label_review_claim"] == task["label_review"]
    assert row["observed_prefix"]["first_gold_file_rank"] == 1
    assert row["observed_prefix"]["first_gold_returned_context_span_rank"] == 2
    assert row["observed_prefix"]["first_gold_indexed_span_rank"] is None
    for bad in (
        {"assessment": "reviewed_ambiguous"},
        {"assessment": "unreviewed", "reviewer_id": "fixture-reviewer"},
    ):
        task["label_review"] = bad
        with pytest.raises((ev.EvidenceError, jsonschema.ValidationError)):
            jsonschema.validate(
                suite, json.loads((Path(ev.__file__).with_name("suite.schema.json")).read_text())
            )
            ev.validate_suite(repo, suite)


def test_unknown_diagnostic_policy_rejected_and_legacy_report_shape_preserved(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    schema = json.loads((Path(ev.__file__).with_name("suite.schema.json")).read_text())
    report = ev.evaluate(*record_v3(repo, suite, run, suite_path, runner_path), "lexical", "hybrid")
    assert all("observed_prefix" not in row for row in report["per_query"])
    assert all("query_intent_claim" not in row for row in report["per_query"])
    suite["diagnostic_policy"] = "unknown"
    with pytest.raises(ev.EvidenceError, match="unsupported suite diagnostic_policy"):
        ev.validate_suite(repo, suite)
    suite.pop("diagnostic_policy")
    for key, value in (
        ("query_intent", "bare_symbol"),
        ("label_review", {"assessment": "unreviewed"}),
    ):
        suite["tasks"][0][key] = value
        with pytest.raises(jsonschema.ValidationError):
            jsonschema.validate(suite, schema)
        with pytest.raises(ev.EvidenceError, match="task annotations require"):
            ev.validate_suite(repo, suite)
        suite["tasks"][0].pop(key)
    suite["diagnostic_policy"] = ev.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
    suite["comparison_contract"]["top_k"] = 5
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(suite, schema)
    with pytest.raises(ev.EvidenceError, match="require top_k >= 10"):
        ev.validate_suite(repo, suite)
    suite["comparison_contract"]["top_k"] = 10
    jsonschema.validate(suite, schema)
    ev.validate_suite(repo, suite)


def test_recorded_prefix_does_not_infer_gold_rank_past_top_k():
    gold = [{"path": "gold.go", "start_byte": 10, "end_byte": 20}]
    candidates = [
        {
            "path": "gold.go",
            "start_byte": 0,
            "end_byte": 10,
            "rank": 1,
            "span_accounting": {"indexed_start_byte": 0, "indexed_end_byte": 10},
        },
        {
            "path": "gold.go",
            "start_byte": 0,
            "end_byte": 30,
            "rank": 2,
            "span_accounting": {"indexed_start_byte": 0, "indexed_end_byte": 10},
        },
        {
            "path": "other.go",
            "start_byte": 0,
            "end_byte": 30,
            "rank": 3,
            "span_accounting": {"indexed_start_byte": 0, "indexed_end_byte": 30},
        },
    ]
    observed = ev.observed_prefix_diagnostics(
        candidates, gold, status="capped", declared_top_k=10, indexed_span_authority=True
    )
    assert observed["observed_depth"] == 3
    assert observed["result_status"] == "capped"
    assert observed["unique_files"] == 2
    assert observed["duplicate_file_candidates"] == 1
    assert observed["first_gold_file_rank"] == 1
    assert observed["first_gold_returned_context_span_rank"] == 2
    assert observed["first_gold_indexed_span_rank"] is None
    assert observed["indexed_span_authority"] == "published_unit_v1"
    unavailable = ev.observed_prefix_diagnostics(
        candidates, gold, status="success", declared_top_k=10, indexed_span_authority=False
    )
    assert unavailable["first_gold_indexed_span_rank"] is None
    assert unavailable["indexed_span_authority"] == "unavailable"


def test_current_hand_calculated_rank_metrics(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    loaded = record_v3(repo, suite, run, suite_path, runner_path)
    report = ev.evaluate(*loaded, "lexical", "hybrid", strict_k=True)
    assert report["schema_version"] == ev.SCHEMA_VERSION
    assert report["rank_metric_version"] == "rb-rank-context-density-first-coverage"
    assert report["graded"] is True
    assert report["primary_metric"] == "ndcg_at_10"
    assert "judgment_metrics" not in report  # Historical suite v3 reports stay unchanged.
    routes = report["rank_metrics"]["routes"]
    # Lexical T1: [miss a:3 (same file, wrong lines), hit b:1]; hybrid T1: [hit a:2, miss, hit b:1].
    lex_chunk = routes["lexical"]["chunk"]
    hyb_chunk = routes["hybrid"]["chunk"]
    assert lex_chunk["recall_at_1"] == pytest.approx(0.0)
    assert lex_chunk["recall_at_5"] == pytest.approx(0.5)
    assert hyb_chunk["recall_at_1"] == pytest.approx(0.5)
    assert hyb_chunk["recall_at_5"] == pytest.approx(1.0)
    assert hyb_chunk["recall_at_20"] == ev.NOT_APPLICABLE  # top_k=10 cannot measure @20
    assert report["rank_metrics"]["comparison"]["delta"]["recall_at_20"] == ev.NOT_APPLICABLE
    assert lex_chunk["mrr_at_10"] == pytest.approx(0.5)
    assert hyb_chunk["mrr_at_10"] == pytest.approx(1.0)
    import math as _math

    lex_dcg = 1 / _math.log2(3)
    hyb_dcg = 7.0 + 1 / 2
    idcg = 7.0 + 1 / _math.log2(3)
    assert lex_chunk["ndcg_at_10"] == pytest.approx(lex_dcg / idcg)
    assert hyb_chunk["ndcg_at_10"] == pytest.approx(hyb_dcg / idcg)
    # Same-file wrong lines earn file credit without span credit at rank 1 for lexical.
    assert lex_chunk["file_recall_at_10"] == pytest.approx(1.0)
    # Collapsed view keeps best chunk per file: hybrid collapsed NDCG is ideal.
    assert routes["hybrid"]["collapsed"]["ndcg_at_10"] == pytest.approx(1.0)
    assert routes["hybrid"]["collapsed"]["recall_at_5"] == pytest.approx(1.0)
    # BCY: hybrid covers every gold span, lexical covers only one.
    b4 = report["budgets"]["4000"]
    assert b4["routes"]["hybrid"]["bcy"] == 1
    assert b4["routes"]["lexical"]["bcy"] == 0
    assert b4["comparison"]["paired_wins"] == 2
    # Per-query rows and sample counts are explicit; small samples report NA intervals.
    assert report["sample_count"] == 2
    assert len(report["per_query"]) == 4
    assert report["rank_metrics"]["comparison"]["sample_count"] == 1
    assert (
        report["rank_metrics"]["comparison"]["primary_delta_ci_95"]["status"] == ev.NOT_APPLICABLE
    )


def test_ndcg_credits_each_gold_span_once_even_when_chunks_overlap():
    label = {"path": "src/lib.rs", "start_byte": 20, "end_byte": 30, "grade": 3}
    candidates = [
        {"path": "src/lib.rs", "start_byte": 0, "end_byte": 30},
        {"path": "src/lib.rs", "start_byte": 10, "end_byte": 40},
    ]
    # Ten newly useful bytes in a 30-byte candidate earn one-third gain;
    # the overlapping second chunk cannot credit that gold span again.
    assert ev.ndcg_at_k(candidates[:1], [label], 10) == pytest.approx(1 / 3)
    assert ev.ndcg_at_k(candidates, [label], 10) == pytest.approx(1 / 3)
    assert ev.ndcg_at_k(list(reversed(candidates)), [label], 10) == pytest.approx(1 / 3)


def test_zero_grade_cannot_be_gold_in_suite_schema_or_evaluator(tmp_path):
    repo, suite, run, suite_path, runner_path, _files = fixture_v3(tmp_path)
    suite["tasks"][0]["gold"][0]["grade"] = 0
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(suite, _load_schema("suite.schema.json"))
    with pytest.raises(ev.EvidenceError, match="gold grade"):
        record_v3(repo, suite, run, suite_path, runner_path)


def test_current_bcy_budget_prefix_and_out_of_budget_not_credited(tmp_path):
    repo = tmp_path / "source"
    repo.mkdir()
    source = ("token " * 3000 + "\n").encode()
    (repo / "target.txt").write_bytes(source)
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    subprocess.run(["git", "-C", str(repo), "add", "target.txt"], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(repo),
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "frozen",
        ],
        check=True,
    )
    commit = ev.git(repo, "rev-parse", "HEAD")
    label = {
        "path": "target.txt",
        "start_byte": 0,
        "end_byte": len(source),
        "start_line": 1,
        "end_line": 1,
        "file_sha256": ev.digest(source),
        "block_sha256": ev.digest(source),
        "grade": 2,
    }
    q1 = "find token handling"
    q2 = "find nonexistent"
    suite = {
        "schema_version": ev.SCHEMA_VERSION,
        "suite_id": "budget-current",
        "repository_commit": commit,
        "comparison_contract": _v3_contract(),
        "routes": ["lexical", "hybrid"],
        "file_universe": [{"path": "target.txt", "file_sha256": ev.digest(source)}],
        "file_universe_digest": ev.universe_digest(
            [{"path": "target.txt", "file_sha256": ev.digest(source)}]
        ),
        "tasks": [
            {
                "task_id": "T1",
                "split": "eval",
                "query": q1,
                "query_sha256": ev.digest(q1.encode()),
                "query_family_id": "budget-answerable",
                "answerable": True,
                "gold": [label],
            },
            {
                "task_id": "T2",
                "split": "eval",
                "query": q2,
                "query_sha256": ev.digest(q2.encode()),
                "query_family_id": "budget-no-answer",
                "answerable": False,
                "gold": [],
            },
        ],
    }
    _, pack, _ = ev.validate_suite(repo, suite)
    cand = dict({k: v for k, v in label.items() if k != "grade"}, tokens=3000, rank=1)
    run = {
        "schema_version": ev.SCHEMA_VERSION,
        "query_pack_sha256": ev.digest(ev.canonical(pack)),
        "comparison_contract": _v3_contract(),
        "runner": {
            "name": "r",
            "revision": "r@1",
            "run_id": "run-1",
            "tokenizer": ev.TOKENIZER,
            "tokenizer_budget_version": ev.TOKENIZER_BUDGET_VERSION,
            "gold_access": False,
            "blinding": "isolated",
            "isolation_method": "separate suite access",
            "access_block_log": "EACCES verified",
        },
        "captures": {"q0": _v3_capture("quanta")},
        "route_provenance": {
            "lexical": {"capture_id": "q0"},
            "hybrid": {"capture_id": "q0"},
        },
        "results": [
            {
                "task_id": "T1",
                "route": "lexical",
                "status": "abstained",
                "candidates": [],
                "timings": {"query_latency_ms": 1.0},
                "error": None,
            },
            {
                "task_id": "T1",
                "route": "hybrid",
                "status": "success",
                "candidates": [cand],
                "timings": {"query_latency_ms": 1.0},
                "error": None,
            },
            {
                "task_id": "T2",
                "route": "lexical",
                "status": "abstained",
                "candidates": [],
                "timings": {"query_latency_ms": 1.0},
                "error": None,
            },
            {
                "task_id": "T2",
                "route": "hybrid",
                "status": "abstained",
                "candidates": [],
                "timings": {"query_latency_ms": 1.0},
                "error": None,
            },
        ],
    }
    suite_path = tmp_path / "suite.json"
    runner_path = tmp_path / "run.json"
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    runner_path.write_text(json.dumps(run), encoding="utf-8")
    loaded = ev.load_evidence(repo, suite_path, runner_path)
    report = ev.evaluate(*loaded, "lexical", "hybrid")
    assert report["budgets"]["2000"]["routes"]["hybrid"]["bcy"] == 0
    assert report["budgets"]["4000"]["routes"]["hybrid"]["bcy"] == 1
    # Rank metrics are budget-independent: the covering candidate scores at rank 1.
    assert report["rank_metrics"]["routes"]["hybrid"]["chunk"]["recall_at_1"] == pytest.approx(1.0)


def test_current_all_answerable_external_suite_passes_without_invented_no_answer(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path, answerable_only=True)
    loaded = record_v3(repo, suite, run, suite_path, runner_path)
    report = ev.evaluate(*loaded, "lexical", "hybrid")
    assert report["answerable_tasks"] == 2
    assert report["no_gold_tasks"] == 0
    assert report["budgets"]["4000"]["routes"]["hybrid"]["no_gold_abstention"] == ev.NOT_APPLICABLE
    assert (
        report["budgets"]["4000"]["comparison"]["delta"]["no_gold_abstention"] == ev.NOT_APPLICABLE
    )
    assert report["rank_metrics"]["routes"]["hybrid"]["chunk"]["recall_at_5"] == pytest.approx(1.0)


def test_current_same_candidates_identical_scores_regardless_of_runner(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    first = ev.evaluate(*record_v3(repo, suite, run, suite_path, runner_path), "lexical", "hybrid")
    run["runner"]["name"] = "other-runner"
    run["runner"]["run_id"] = "run-other"
    run["captures"]["q0"]["model"] = "other-model"
    runner_path2 = tmp_path / "run2.json"
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    runner_path2.write_text(json.dumps(run), encoding="utf-8")
    second = ev.evaluate(*ev.load_evidence(repo, suite_path, runner_path2), "lexical", "hybrid")
    assert ev.canonical(first["budgets"]) == ev.canonical(second["budgets"])
    assert ev.canonical(first["rank_metrics"]) == ev.canonical(second["rank_metrics"])


def test_current_rescore_is_deterministic_under_row_order(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    first = ev.evaluate(*record_v3(repo, suite, run, suite_path, runner_path), "lexical", "hybrid")
    run["results"] = list(reversed(run["results"]))
    runner_path.write_text(json.dumps(run), encoding="utf-8")
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    second = ev.evaluate(*ev.load_evidence(repo, suite_path, runner_path), "lexical", "hybrid")
    assert ev.canonical(first["budgets"]) == ev.canonical(second["budgets"])
    assert ev.canonical(first["rank_metrics"]) == ev.canonical(second["rank_metrics"])
    assert ev.canonical(first["per_query"]) == ev.canonical(second["per_query"])


def test_current_partial_span_earns_no_full_cover_credit(tmp_path):
    files = {"a.txt": b"line one\nline two\nline three\n"}
    repo, commit = _write_repo(tmp_path, files)
    file_sha = ev.digest(files["a.txt"])
    gold_sel = b"line one\nline two\n"
    part_sel = b"line one\n"
    q = "find first two lines"
    suite = {
        "schema_version": ev.SCHEMA_VERSION,
        "suite_id": "partial-current",
        "repository_commit": commit,
        "comparison_contract": _v3_contract(),
        "routes": ["lexical", "hybrid"],
        "file_universe": [{"path": "a.txt", "file_sha256": file_sha}],
        "file_universe_digest": ev.universe_digest([{"path": "a.txt", "file_sha256": file_sha}]),
        "tasks": [
            {
                "task_id": "T1",
                "split": "eval",
                "query": q,
                "query_sha256": ev.digest(q.encode()),
                "query_family_id": "partial-answerable",
                "answerable": True,
                "gold": [
                    {
                        "path": "a.txt",
                        "start_byte": 0,
                        "end_byte": len(gold_sel),
                        "start_line": 1,
                        "end_line": 2,
                        "file_sha256": file_sha,
                        "block_sha256": ev.digest(gold_sel),
                        "grade": 3,
                    }
                ],
            },
            {
                "task_id": "T2",
                "split": "eval",
                "query": "find nothing here",
                "query_sha256": ev.digest(b"find nothing here"),
                "query_family_id": "partial-no-answer",
                "answerable": False,
                "gold": [],
            },
        ],
    }
    _, pack, _ = ev.validate_suite(repo, suite)
    part_tokens = len(ev.TOKEN_RE.findall(part_sel.decode()))
    full_tokens = len(ev.TOKEN_RE.findall(gold_sel.decode()))
    part = {
        "path": "a.txt",
        "start_byte": 0,
        "end_byte": len(part_sel),
        "start_line": 1,
        "end_line": 1,
        "file_sha256": file_sha,
        "block_sha256": ev.digest(part_sel),
        "tokens": part_tokens,
        "rank": 1,
    }
    full = {
        "path": "a.txt",
        "start_byte": 0,
        "end_byte": len(gold_sel),
        "start_line": 1,
        "end_line": 2,
        "file_sha256": file_sha,
        "block_sha256": ev.digest(gold_sel),
        "tokens": full_tokens,
        "rank": 1,
    }
    run = {
        "schema_version": ev.SCHEMA_VERSION,
        "query_pack_sha256": ev.digest(ev.canonical(pack)),
        "comparison_contract": _v3_contract(),
        "runner": {
            "name": "r",
            "revision": "r@1",
            "run_id": "x",
            "tokenizer": ev.TOKENIZER,
            "tokenizer_budget_version": ev.TOKENIZER_BUDGET_VERSION,
            "gold_access": False,
            "blinding": "attested",
            "isolation_method": "attestation only",
            "access_block_log": "no separate suite access; attested-only",
        },
        "captures": {"q0": _v3_capture("quanta")},
        "route_provenance": {
            "lexical": {"capture_id": "q0"},
            "hybrid": {"capture_id": "q0"},
        },
        "results": [
            {
                "task_id": "T1",
                "route": "lexical",
                "status": "success",
                "candidates": [part],
                "timings": {"query_latency_ms": 1.0},
                "error": None,
            },
            {
                "task_id": "T1",
                "route": "hybrid",
                "status": "success",
                "candidates": [full],
                "timings": {"query_latency_ms": 1.0},
                "error": None,
            },
            {
                "task_id": "T2",
                "route": "lexical",
                "status": "abstained",
                "candidates": [],
                "timings": {"query_latency_ms": 1.0},
                "error": None,
            },
            {
                "task_id": "T2",
                "route": "hybrid",
                "status": "abstained",
                "candidates": [],
                "timings": {"query_latency_ms": 1.0},
                "error": None,
            },
        ],
    }
    suite_path = tmp_path / "s.json"
    runner_path = tmp_path / "r.json"
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    runner_path.write_text(json.dumps(run), encoding="utf-8")
    report = ev.evaluate(*ev.load_evidence(repo, suite_path, runner_path), "lexical", "hybrid")
    lex = report["rank_metrics"]["routes"]["lexical"]["chunk"]
    hyb = report["rank_metrics"]["routes"]["hybrid"]["chunk"]
    assert lex["recall_at_1"] == pytest.approx(0.0)
    assert lex["file_recall_at_10"] == pytest.approx(1.0)
    assert hyb["recall_at_1"] == pytest.approx(1.0)
    assert report["budgets"]["4000"]["routes"]["lexical"]["bcy"] == 0
    assert report["budgets"]["4000"]["routes"]["hybrid"]["bcy"] == 1


def test_current_typed_failures_score_zero_and_are_counted(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    run["results"][1] = {
        "task_id": "T1",
        "route": "hybrid",
        "status": "timeout",
        "candidates": [],
        "query_identity": run["results"][1]["query_identity"],
        "timings": {"query_latency_ms": 5.0},
        "error": {"code": "TIMEOUT", "message": "deadline"},
    }
    report = ev.evaluate(*record_v3(repo, suite, run, suite_path, runner_path), "lexical", "hybrid")
    hyb = report["rank_metrics"]["routes"]["hybrid"]
    assert hyb["chunk"]["recall_at_10"] == pytest.approx(0.0)
    assert hyb["status_counts"].get("timeout") == 1
    assert report["budgets"]["4000"]["routes"]["hybrid"]["bcy"] == 0
    rows = [r for r in report["per_query"] if r["task_id"] == "T1" and r["route"] == "hybrid"]
    assert rows[0]["error_code"] == "TIMEOUT"
    assert rows[0]["query_latency_ms"] == pytest.approx(5.0)


def test_current_capped_result_is_scored_but_flagged(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    run["results"][1]["status"] = "capped"
    report = ev.evaluate(*record_v3(repo, suite, run, suite_path, runner_path), "lexical", "hybrid")
    assert report["rank_metrics"]["routes"]["hybrid"]["chunk"]["recall_at_5"] == pytest.approx(1.0)
    rows = [r for r in report["per_query"] if r["task_id"] == "T1" and r["route"] == "hybrid"]
    assert rows[0]["status"] == "capped"


def test_current_blinding_values_preserved(tmp_path):
    for blinding in ("isolated", "attested"):
        repo, suite, run, suite_path, runner_path, _ = fixture_v3(
            tmp_path / blinding, blinding=blinding
        )
        report = ev.evaluate(
            *record_v3(repo, suite, run, suite_path, runner_path), "lexical", "hybrid"
        )
        assert report["blinding"] == blinding
        assert report["runner"]["blinding"] == blinding


def test_current_confidence_interval_available_on_sufficient_sample(tmp_path):
    files = {"a.txt": b"alpha one\nalpha two\n"}
    repo, commit = _write_repo(tmp_path, files)
    file_sha = ev.digest(files["a.txt"])
    line1 = b"alpha one\n"
    tasks = []
    for i in range(20):
        q = "locate " + ev.digest(str(i).encode())
        tasks.append(
            {
                "task_id": f"T{i:02d}",
                "split": "eval",
                "query": q,
                "query_sha256": ev.digest(q.encode()),
                "query_family_id": f"ci-{i}",
                "answerable": True,
                "gold": [
                    {
                        "path": "a.txt",
                        "start_byte": 0,
                        "end_byte": len(line1),
                        "start_line": 1,
                        "end_line": 1,
                        "file_sha256": file_sha,
                        "block_sha256": ev.digest(line1),
                        "grade": 2,
                    }
                ],
            }
        )
    suite = {
        "schema_version": ev.SCHEMA_VERSION,
        "suite_id": "ci-current",
        "repository_commit": commit,
        "comparison_contract": _v3_contract(),
        "routes": ["lexical", "hybrid"],
        "file_universe": [{"path": "a.txt", "file_sha256": file_sha}],
        "file_universe_digest": ev.universe_digest([{"path": "a.txt", "file_sha256": file_sha}]),
        "tasks": tasks,
    }
    _, pack, _ = ev.validate_suite(repo, suite)
    tokens = len(ev.TOKEN_RE.findall(line1.decode()))
    miss_sel = b"alpha two\n"
    miss_tokens = len(ev.TOKEN_RE.findall(miss_sel.decode()))

    def hit(rank):
        return {
            "path": "a.txt",
            "start_byte": 0,
            "end_byte": len(line1),
            "start_line": 1,
            "end_line": 1,
            "file_sha256": file_sha,
            "block_sha256": ev.digest(line1),
            "tokens": tokens,
            "rank": rank,
        }

    def miss(rank):
        return {
            "path": "a.txt",
            "start_byte": len(line1),
            "end_byte": len(files["a.txt"]),
            "start_line": 2,
            "end_line": 2,
            "file_sha256": file_sha,
            "block_sha256": ev.digest(miss_sel),
            "tokens": miss_tokens,
            "rank": rank,
        }

    results = []
    for i in range(20):
        tid = f"T{i:02d}"
        # Lexical always hits; hybrid hits on even tasks only.
        results.append(
            {
                "task_id": tid,
                "route": "lexical",
                "status": "success",
                "candidates": [hit(1)],
                "timings": {"query_latency_ms": 1.0},
                "error": None,
            }
        )
        if i % 2 == 0:
            results.append(
                {
                    "task_id": tid,
                    "route": "hybrid",
                    "status": "success",
                    "candidates": [hit(1)],
                    "timings": {"query_latency_ms": 1.0},
                    "error": None,
                }
            )
        else:
            results.append(
                {
                    "task_id": tid,
                    "route": "hybrid",
                    "status": "success",
                    "candidates": [miss(1)],
                    "timings": {"query_latency_ms": 1.0},
                    "error": None,
                }
            )
    run = {
        "schema_version": ev.SCHEMA_VERSION,
        "query_pack_sha256": ev.digest(ev.canonical(pack)),
        "comparison_contract": _v3_contract(),
        "runner": {
            "name": "r",
            "revision": "r@1",
            "run_id": "ci",
            "tokenizer": ev.TOKENIZER,
            "tokenizer_budget_version": ev.TOKENIZER_BUDGET_VERSION,
            "gold_access": False,
            "blinding": "isolated",
            "isolation_method": "separate suite access",
            "access_block_log": "verified",
        },
        "captures": {"q0": _v3_capture("quanta")},
        "route_provenance": {
            "lexical": {"capture_id": "q0"},
            "hybrid": {"capture_id": "q0"},
        },
        "results": results,
    }
    suite_path = tmp_path / "s.json"
    runner_path = tmp_path / "r.json"
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    runner_path.write_text(json.dumps(run), encoding="utf-8")
    report = ev.evaluate(*ev.load_evidence(repo, suite_path, runner_path), "lexical", "hybrid")
    ci = report["rank_metrics"]["comparison"]["primary_delta_ci_95"]
    assert ci["sample_count"] == 20
    assert ci["method"] == "paired_stratified_bootstrap_percentile_v1"
    assert ci["resamples"] == 10_000
    assert ci["strata"] == {"uncategorized": 20}
    assert len(ci["seed_sha256"]) == 64
    assert ci["lower_95"] <= ci["mean"] <= ci["upper_95"]
    cluster_ci = ev.qualified_query_family_ci(suite, report, "lexical", "hybrid")
    assert cluster_ci["cluster_count"] == 20
    assert cluster_ci["method"] == "paired_query_family_cluster_bootstrap_percentile_v1"
    assert pairrun._qualified_cluster_uncertainty(
        {"sample_count": 20, "primary_delta": ci["mean"], "primary_delta_cluster_ci_95": cluster_ci}
    )
    repeated = ev.evaluate(*ev.load_evidence(repo, suite_path, runner_path), "lexical", "hybrid")[
        "rank_metrics"
    ]["comparison"]["primary_delta_ci_95"]
    assert repeated == ci
    assert report["rank_metrics"]["comparison"]["paired_losses"] == 10
    stratified = report["rank_metrics"]["comparison"]["stratified_primary_delta"]
    assert stratified["category"]["uncategorized"]["sample_count"] == 20
    assert stratified["language"]["txt"]["sample_count"] == 20
    assert stratified["repository"][commit]["sample_count"] == 20
    no_answer = report["rank_metrics"]["comparison"]["no_answer_abstention_delta"]
    assert no_answer["sample_count"] == 0
    assert no_answer["mean_delta"] == ev.NOT_APPLICABLE
    assert no_answer["strata"] == {"category": {}, "language": {}, "repository": {}}


def test_bootstrap_is_order_independent_and_rejects_invalid_pairs(monkeypatch):
    monkeypatch.setattr(ev, "MIN_CI_SAMPLE", 4)
    deltas = [0.4, -0.2, 0.1, 0.7]
    strata = [("T2", "semantic"), ("T1", "symbol"), ("T4", "semantic"), ("T3", "symbol")]
    expected = ev.mean_ci(deltas, strata)
    assert ev.mean_ci(list(reversed(deltas)), list(reversed(strata))) == expected
    with pytest.raises(ev.EvidenceError, match="identities must be unique"):
        ev.mean_ci(deltas, [("T1", "a"), ("T1", "b"), ("T3", "a"), ("T4", "b")])
    with pytest.raises(ev.EvidenceError, match="deltas must be finite"):
        ev.mean_ci([0.0, 1.0, float("nan"), 2.0], strata)


def test_qualified_cluster_interval_refuses_correlated_task_pseudoreplication():
    rows = [(f"T{i:02d}", "one-family", "symbol", 0.5 if i % 2 else -0.5) for i in range(20)]
    ci = ev.query_family_cluster_ci(rows, "a" * 40)
    assert ci["sample_count"] == 20
    assert ci["cluster_count"] == 1
    assert ci["status"] == ev.NOT_APPLICABLE
    assert ci["reason"] == "insufficient_independent_clusters"
    assert not pairrun._qualified_cluster_uncertainty(
        {"sample_count": 20, "primary_delta": 0.0, "primary_delta_cluster_ci_95": ci}
    )
    with pytest.raises(ev.EvidenceError, match="crosses categories"):
        ev.query_family_cluster_ci(
            [("T1", "same", "symbol", 0.5), ("T2", "same", "file", -0.5)], "a" * 40
        )
    sparse = [(f"T{i:02d}", f"F{i:02d}", "symbol" if i < 19 else "file", 0.5) for i in range(20)]
    assert ev.query_family_cluster_ci(sparse, "a" * 40)["reason"] == (
        "insufficient_clusters_in_stratum"
    )


def test_repository_cluster_interval_uses_equal_family_and_repository_weights():
    repositories = {f"repo-{i:02d}": f"{i + 1:040x}" for i in range(12)}
    strata = {name: f"language-{i // 3}" for i, name in enumerate(repositories)}
    rows = (
        [
            (repository, f"{repository}-a1", f"{repository}-family-a", 1.0)
            for repository in repositories
        ]
        + [
            (repository, f"{repository}-a2", f"{repository}-family-a", 1.0)
            for repository in repositories
        ]
        + [
            (repository, f"{repository}-b", f"{repository}-family-b", 0.0)
            for repository in repositories
        ]
    )
    ci = ev.repository_cluster_ci(rows, "a" * 64, repositories, strata)
    assert ci["status"] == "available"
    assert ci["strata"] == {f"language-{i}": 3 for i in range(4)}
    assert (ci["repository_count"], ci["family_count"], ci["sample_count"]) == (12, 24, 36)
    assert (ci["mean"], ci["lower_95"], ci["upper_95"]) == (0.5, 0.5, 0.5)
    assert ev.repository_cluster_ci(list(reversed(rows)), "a" * 64, repositories, strata) == ci


def test_repository_cluster_interval_refuses_incomplete_or_cross_repo_evidence():
    repositories = {f"repo-{i:02d}": f"{i + 1:040x}" for i in range(12)}
    strata = {name: f"language-{i // 3}" for i, name in enumerate(repositories)}
    rows = [
        (repository, f"task-{i:02d}", f"family-{i:02d}", 0.5)
        for i, repository in enumerate(repositories)
    ]
    with pytest.raises(ev.EvidenceError, match="lacks paired rows"):
        ev.repository_cluster_ci(rows[:-1], "a" * 64, repositories, strata)
    with pytest.raises(ev.EvidenceError, match="crosses repositories"):
        ev.repository_cluster_ci(
            [*rows, ("repo-01", "extra", "family-00", 0.5)], "a" * 64, repositories, strata
        )
    with pytest.raises(ev.EvidenceError, match="duplicated"):
        ev.repository_cluster_ci([*rows, rows[0]], "a" * 64, repositories, strata)
    with pytest.raises(ev.EvidenceError, match="not finite"):
        ev.repository_cluster_ci(
            [*rows[:-1], (*rows[-1][:3], float("nan"))], "a" * 64, repositories, strata
        )
    with pytest.raises(ev.EvidenceError, match="out of range"):
        ev.repository_cluster_ci(
            [*rows[:-1], (*rows[-1][:3], 1.01)], "a" * 64, repositories, strata
        )
    small = {name: commit for name, commit in repositories.items() if name != "repo-11"}
    result = ev.repository_cluster_ci(rows[:-1], "a" * 64, small, {k: strata[k] for k in small})
    assert result["status"] == ev.NOT_APPLICABLE
    assert result["reason"] == "insufficient_independent_repositories"
    with pytest.raises(ev.EvidenceError, match="strata inventory differs"):
        ev.repository_cluster_ci(rows, "a" * 64, repositories, {"repo-00": "language-0"})
    sparse_strata = dict(strata)
    sparse_strata["repo-11"] = "language-4"
    result = ev.repository_cluster_ci(rows, "a" * 64, repositories, sparse_strata)
    assert result["reason"] == "insufficient_repositories_in_stratum"


def test_repository_cluster_interval_does_not_count_extra_tasks_as_repositories():
    repositories = {f"repo-{i:02d}": f"{i + 1:040x}" for i in range(12)}
    strata = {name: f"language-{i // 3}" for i, name in enumerate(repositories)}
    rows = [
        (repository, f"task-{i:02d}", f"family-{i:02d}", 1.0 if i % 2 == 0 else -1.0)
        for i, repository in enumerate(repositories)
    ]
    base = ev.repository_cluster_ci(rows, "b" * 64, repositories, strata)
    extra = [("repo-00", f"replica-{i:03d}", "family-00", 1.0) for i in range(100)]
    expanded = ev.repository_cluster_ci([*rows, *extra], "b" * 64, repositories, strata)
    assert base["mean"] == expanded["mean"] == 0.0
    assert (base["lower_95"], base["upper_95"]) == (
        expanded["lower_95"],
        expanded["upper_95"],
    )
    assert base["lower_95"] < 0 < base["upper_95"]
    assert expanded["repository_count"] == 12 and expanded["sample_count"] == 112


def test_repository_cluster_report_rows_bind_every_paired_eval_task():
    repositories = {f"repo-{index:02d}": f"{index + 1:040x}" for index in range(12)}
    strata = {name: f"language-{index // 3}" for index, name in enumerate(repositories)}
    suites, reports = {}, {}
    for name, commit in repositories.items():
        family = name + ".family"
        suite = {
            "suite_id": name + ".suite",
            "repository_commit": commit,
            "tasks": [
                {"task_id": "A", "split": "eval", "gold": [1], "query_family_id": family},
                {"task_id": "B", "split": "eval", "gold": [1], "query_family_id": family},
                {"task_id": "N", "split": "eval", "gold": [], "query_family_id": name + ".no"},
            ],
        }
        rows = [
            {"task_id": task, "route": route, "ndcg_at_10": value}
            for task, left, right in (("A", 0.25, 0.75), ("B", 0.5, 0.5))
            for route, value in (("lexical", left), ("hybrid", right))
        ]
        rows.extend({"task_id": "N", "route": route} for route in ("lexical", "hybrid"))
        suites[name] = suite
        reports[name] = {
            "suite_id": suite["suite_id"],
            "suite_commitment_sha256": ev.digest(ev.canonical(suite)),
            "repository_commit": commit,
            "graded": True,
            "rank_metrics": {
                "comparison": {
                    "baseline": "lexical",
                    "candidate": "hybrid",
                    "primary_metric": "ndcg_at_10",
                    "sample_count": 2,
                    "primary_delta": 0.25,
                }
            },
            "per_query": rows,
        }

    def interval(changed_suites=suites, changed_reports=reports):
        return ev.repository_cluster_ci_from_reports(
            changed_suites, changed_reports, "a" * 64, repositories, strata, "lexical", "hybrid"
        )

    result = interval()
    assert result["status"] == "available"
    assert (result["repository_count"], result["sample_count"]) == (12, 24)
    assert (result["mean"], result["lower_95"], result["upper_95"]) == (0.25, 0.25, 0.25)
    assert interval(dict(reversed(list(suites.items()))), reports) == result

    for mutation, message in (
        (lambda r: r["per_query"].pop(), "missing a paired task row"),
        (lambda r: r["per_query"].append(r["per_query"][0]), "query rows are duplicate"),
        (lambda r: r["rank_metrics"]["comparison"].update(primary_delta=0.5), "differs"),
        (lambda r: r["rank_metrics"]["comparison"].update(sample_count=True), "differs"),
        (lambda r: r["per_query"][0].update(ndcg_at_10=10**400), "bounded finite"),
        (lambda r: r.update(rank_metrics=[]), "comparison is missing"),
        (lambda r: r.update(graded=False), "not graded"),
    ):
        changed = copy.deepcopy(reports)
        mutation(changed["repo-00"])
        with pytest.raises(ev.EvidenceError, match=message):
            interval(suites, changed)
    changed_suites = copy.deepcopy(suites)
    changed_suites["repo-00"]["tasks"][0]["query_family_id"] = "forged"
    with pytest.raises(ev.EvidenceError, match="suite binding differs"):
        interval(changed_suites, reports)
    with pytest.raises(ev.EvidenceError, match="report inventory differs"):
        interval(suites, {name: report for name, report in reports.items() if name != "repo-11"})


def test_stratified_delta_summary_binds_category_language_and_repository(monkeypatch):
    monkeypatch.setattr(ev, "MIN_CI_SAMPLE", 2)
    rows = [
        ("T1", {"category": "symbol", "gold": [{"path": "src/a.rs"}]}, 0.5),
        ("T2", {"category": "symbol", "gold": [{"path": "src/b.rs"}]}, -0.5),
        ("T3", {"category": "semantic", "gold": [{"path": "pkg/c.py"}]}, 0.25),
        (
            "T4",
            {
                "category": "semantic",
                "gold": [{"path": "pkg/d.py"}, {"path": "web/e.ts"}],
            },
            0.75,
        ),
    ]
    commit = "a" * 40
    summary = ev.stratified_delta_summary(rows, commit)
    assert set(summary) == {"category", "language", "repository"}
    assert summary["category"]["symbol"]["sample_count"] == 2
    assert summary["category"]["symbol"]["mean_delta"] == pytest.approx(0.0)
    assert summary["category"]["semantic"]["mean_delta"] == pytest.approx(0.5)
    assert summary["language"]["rs"]["sample_count"] == 2
    assert summary["language"]["py"]["sample_count"] == 1
    assert summary["language"]["mixed:py+ts"]["sample_count"] == 1
    assert summary["repository"][commit]["sample_count"] == 4
    assert summary["repository"][commit]["ci_95"].get("status") != ev.NOT_APPLICABLE
    assert ev.stratified_delta_summary(list(reversed(rows)), commit) == summary

    no_answer_rows = [
        ("N1", {"category": "negative", "gold": []}, 1.0),
        ("N2", {"category": "negative", "gold": []}, 0.0),
    ]
    no_answer = ev.no_answer_delta_summary(no_answer_rows, commit)
    assert no_answer["metric"] == "no_answer_abstention"
    assert no_answer["sample_count"] == 2
    assert no_answer["mean_delta"] == pytest.approx(0.5)
    assert no_answer["strata"]["language"]["not_applicable:no_gold"]["sample_count"] == 2


def test_qualified_uncertainty_contract_rejects_incomplete_or_forged_strata(monkeypatch):
    monkeypatch.setattr(ev, "MIN_CI_SAMPLE", 2)
    rows = [
        ("T1", {"category": "symbol", "gold": [{"path": "a.rs"}]}, 0.25),
        ("T2", {"category": "symbol", "gold": [{"path": "b.rs"}]}, -0.25),
    ]
    comparison = {
        "primary_delta": 0.0,
        "sample_count": 2,
        "paired_wins": 1,
        "paired_losses": 1,
        "paired_ties": 0,
        "primary_delta_ci_95": ev.mean_ci([0.25, -0.25], [("T1", "symbol"), ("T2", "symbol")]),
        "stratified_primary_delta": ev.stratified_delta_summary(rows, "a" * 40),
        "no_answer_abstention_delta": ev.no_answer_delta_summary([], "a" * 40),
    }
    assert pairrun._qualified_uncertainty(comparison)
    for invalid_strata in (None, [], 10**400, True):
        forged = json.loads(json.dumps(comparison))
        forged["no_answer_abstention_delta"]["strata"] = invalid_strata
        assert not pairrun._qualified_uncertainty(forged)
    for invalid_count in (10**400, -(10**400), True, None):
        forged = json.loads(json.dumps(comparison))
        forged["stratified_primary_delta"]["category"]["symbol"]["sample_count"] = invalid_count
        assert not pairrun._qualified_uncertainty(forged)
        assert not pairrun._valid_stratified_delta({}, invalid_count, 0.0)
    for invalid in (10**400, -(10**400), float("nan"), float("inf"), -float("inf"), True, None):
        for field in ("primary_delta", "ci_mean", "stratum_mean"):
            forged = json.loads(json.dumps(comparison))
            if field == "primary_delta":
                forged["primary_delta"] = invalid
            elif field == "ci_mean":
                forged["primary_delta_ci_95"]["mean"] = invalid
            else:
                forged["stratified_primary_delta"]["category"]["symbol"]["mean_delta"] = invalid
            assert not pairrun._qualified_uncertainty(forged)

    mutants = []
    for mutate in (
        lambda value: value.update(sample_count=1),
        lambda value: value.update(primary_delta=0.5),
        lambda value: value.update(paired_ties=1),
        lambda value: value["primary_delta_ci_95"].update(method="normal_approximation"),
        lambda value: value["primary_delta_ci_95"].update(resamples=100),
        lambda value: value["primary_delta_ci_95"].update(seed_sha256="bad"),
        lambda value: value["primary_delta_ci_95"].update(strata={"symbol": 1}),
        lambda value: value["primary_delta_ci_95"].update(lower_95=2.0),
        lambda value: value["primary_delta_ci_95"].update(mean=0.5),
        lambda value: value["stratified_primary_delta"].pop("language"),
        lambda value: value["stratified_primary_delta"]["category"].clear(),
        lambda value: value["stratified_primary_delta"]["category"]["symbol"].pop("mean_delta"),
        lambda value: value["stratified_primary_delta"]["category"]["symbol"].update(
            mean_delta=0.5
        ),
        lambda value: value["stratified_primary_delta"]["category"]["symbol"]["ci_95"].update(
            mean=0.5
        ),
        lambda value: value["stratified_primary_delta"]["category"]["symbol"].update(
            sample_count=1
        ),
        lambda value: value.pop("no_answer_abstention_delta"),
        lambda value: value["no_answer_abstention_delta"].update(sample_count=1),
        lambda value: value["no_answer_abstention_delta"]["ci_95"].update(method="normal"),
        lambda value: value["no_answer_abstention_delta"]["strata"]["category"].update(
            forged={"sample_count": 1, "mean_delta": 0.0, "ci_95": {}}
        ),
    ):
        mutant = json.loads(json.dumps(comparison))
        mutate(mutant)
        mutants.append(mutant)
    assert all(not pairrun._qualified_uncertainty(mutant) for mutant in mutants)

    no_answer_comparison = json.loads(json.dumps(comparison))
    no_answer_comparison["no_answer_abstention_delta"] = ev.no_answer_delta_summary(
        [
            ("N1", {"category": "negative", "gold": []}, 1.0),
            ("N2", {"category": "negative", "gold": []}, 0.0),
        ],
        "a" * 40,
    )
    assert pairrun._qualified_uncertainty(no_answer_comparison)
    for mutate in (
        lambda value: value.update(mean_delta=0.75),
        lambda value: value["strata"]["category"]["negative"].update(mean_delta=0.75),
        lambda value: value["strata"]["repository"]["a" * 40].update(sample_count=1),
    ):
        mutant = json.loads(json.dumps(no_answer_comparison))
        mutate(mutant["no_answer_abstention_delta"])
        assert not pairrun._qualified_uncertainty(mutant)


@pytest.mark.parametrize(
    "mutation,match",
    [
        (lambda s, r, f: s.update(repository_commit="0" * 40), "checkout HEAD differs"),
        (
            lambda s, r, f: s["tasks"][0]["gold"][0].update(block_sha256="0" * 64),
            "block hash mismatch",
        ),
        (
            lambda s, r, f: s["tasks"][0]["gold"][0].update(file_sha256="0" * 64),
            "file hash mismatch",
        ),
        (lambda s, r, f: s["tasks"][0]["gold"][0].update(grade=5), "grade"),
        (lambda s, r, f: s["tasks"][0]["gold"][0].update(grade=0), "gold grade"),
        (lambda s, r, f: s["tasks"][0]["gold"][0].update(grade="high"), "grade"),
        (lambda s, r, f: s["file_universe"][0].update(file_sha256="0" * 64), "file universe"),
        (lambda s, r, f: s["tasks"][1].update(answerable=True), "answerable/gold mismatch"),
        (
            lambda s, r, f: s["tasks"][1].update(
                query=s["tasks"][0]["query"], query_sha256=s["tasks"][0]["query_sha256"]
            ),
            "query leakage",
        ),
        (lambda s, r, f: r["results"].pop(), "missing task route evidence"),
        (
            lambda s, r, f: r["results"].append(dict(r["results"][0])),
            "unexpected/duplicate task route",
        ),
        (lambda s, r, f: r["results"][1]["candidates"][1].update(rank=1), "rank"),
        (lambda s, r, f: r["results"][1]["candidates"][0].update(tokens=1), "token count mismatch"),
        (
            lambda s, r, f: r["results"][1]["candidates"][0].update(file_sha256="0" * 64),
            "file hash mismatch",
        ),
        (lambda s, r, f: r["results"][1]["candidates"][0].update(gold=True), "unknown fields"),
        (lambda s, r, f: r["results"][1].update(gold=[]), "unknown fields"),
        (lambda s, r, f: r["runner"].update(tokenizer="other-tok"), "tokenizer"),
        (lambda s, r, f: r["runner"].update(tokenizer_budget_version="qb-x"), "tokenizer"),
        (lambda s, r, f: r["runner"].update(blinding="none"), "blinding"),
        (lambda s, r, f: r["route_provenance"].pop("hybrid"), "route_provenance"),
        (lambda s, r, f: r["results"][1].update(status="mystery"), "unknown result status"),
        (lambda s, r, f: r["results"][1]["timings"].update(query_latency_ms=-1), "timing"),
        (lambda s, r, f: r["results"][1]["timings"].update(query_latency_ms="fast"), "timing"),
        (lambda s, r, f: r.update(query_pack_sha256="0" * 64), "query pack hash mismatch"),
        (lambda s, r, f: s.update(suite_id="changed"), "query pack hash mismatch"),
        (lambda s, r, f: r.__setitem__("schema_version", 1), "unsupported runner schema"),
    ],
)
def test_current_fails_closed_on_invalid_evidence(tmp_path, mutation, match):
    repo, suite, run, suite_path, runner_path, files = fixture_v3(tmp_path)
    mutation(suite, run, files)
    with pytest.raises(ev.EvidenceError, match=match):
        record_v3(repo, suite, run, suite_path, runner_path)


def test_current_excluded_but_tracked_candidate_rejected(tmp_path):
    repo, suite, run, suite_path, runner_path, files = fixture_v3(tmp_path)
    file_sha, block_sha, tokens = _span_meta(files["excluded.txt"], 1, 1)
    run["results"][1]["candidates"][0] = {
        "path": "excluded.txt",
        "start_byte": 0,
        "end_byte": len(files["excluded.txt"]),
        "start_line": 1,
        "end_line": 1,
        "file_sha256": file_sha,
        "block_sha256": block_sha,
        "tokens": tokens,
        "rank": 1,
    }
    # Re-sequence remaining ranks to isolate the universe failure.
    for i, cand in enumerate(run["results"][1]["candidates"], start=1):
        cand["rank"] = i
    with pytest.raises(ev.EvidenceError, match="excluded from file universe"):
        record_v3(repo, suite, run, suite_path, runner_path)


def test_current_gold_in_excluded_file_rejected(tmp_path):
    repo, suite, run, suite_path, runner_path, files = fixture_v3(tmp_path)
    file_sha, block_sha, _ = _span_meta(files["excluded.txt"], 1, 1)
    suite["tasks"][0]["gold"][0] = {
        "path": "excluded.txt",
        "start_byte": 0,
        "end_byte": len(files["excluded.txt"]),
        "start_line": 1,
        "end_line": 1,
        "file_sha256": file_sha,
        "block_sha256": block_sha,
        "grade": 2,
    }
    with pytest.raises(ev.EvidenceError, match="excluded from file universe"):
        record_v3(repo, suite, run, suite_path, runner_path)


def test_current_unsafe_candidate_path_rejected(tmp_path):
    repo, suite, run, suite_path, runner_path, files = fixture_v3(tmp_path)
    run["results"][1]["candidates"][0]["path"] = "../a.txt"
    with pytest.raises(ev.EvidenceError, match="unsafe repository path"):
        record_v3(repo, suite, run, suite_path, runner_path)


def test_current_error_result_with_candidates_rejected(tmp_path):
    repo, suite, run, suite_path, runner_path, files = fixture_v3(tmp_path)
    run["results"][1]["status"] = "error"
    run["results"][1]["error"] = {"code": "E", "message": "m"}
    with pytest.raises(ev.EvidenceError, match="cannot contain candidates"):
        record_v3(repo, suite, run, suite_path, runner_path)


def test_current_cli_freeze_evaluate_roundtrip(tmp_path, capsys):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    pack_path = tmp_path / "pack.json"
    report_path = tmp_path / "report.json"
    assert (
        ev.main(
            ["freeze", "--repo", str(repo), "--suite", str(suite_path), "--output", str(pack_path)]
        )
        == 0
    )
    assert capsys.readouterr().out.strip() == run["query_pack_sha256"]
    text = pack_path.read_text(encoding="utf-8")
    assert "gold" not in text
    assert "grade" not in text
    runner_path.write_text(json.dumps(run), encoding="utf-8")
    assert (
        ev.main(
            [
                "evaluate",
                "--repo",
                str(repo),
                "--suite",
                str(suite_path),
                "--runner",
                str(runner_path),
                "--baseline-route",
                "lexical",
                "--candidate-route",
                "hybrid",
                "--output",
                str(report_path),
            ]
        )
        == 0
    )
    report = json.loads(report_path.read_text(encoding="utf-8"))
    assert report["schema_version"] == ev.SCHEMA_VERSION
    assert report["rank_metrics"]["comparison"]["sample_count"] == 1


# --- RB-04/RB-05 adapter and merge contracts (T11–T13) ---


def _admitted_rows(files: dict[str, bytes]) -> list[tuple[str, str]]:
    return [(name, ev.digest(data)) for name, data in sorted(files.items())]


def test_mapping_proof_clean_and_mismatch_detected(tmp_path):
    _repo, _suite, _run, _sp, _rp, files = fixture_v3(tmp_path)
    corpus = tmp_path / "corpus"
    corpus.mkdir()
    for name, data in files.items():
        if name != "excluded.txt":
            (corpus / name).write_bytes(data)
    admitted = _admitted_rows({k: v for k, v in files.items() if k != "excluded.txt"})
    proof, diff = semble_adapter.mapping_proof(admitted, ["a.txt", "b.txt"], corpus)
    assert proof["skipped"] == [] and proof["extra"] == []
    assert proof["mismatched"] == []
    assert len(diff) == 64
    assert all(row["status"] == "indexed" for row in proof["per_file"])

    with pytest.raises(semble_adapter.AdapterError, match="duplicate path"):
        semble_adapter.mapping_proof(admitted, ["a.txt", "a.txt", "b.txt"], corpus)
    with pytest.raises(semble_adapter.AdapterError, match="unsafe Semble observed path"):
        semble_adapter.mapping_proof(admitted, ["../outside.txt"], corpus)
    (corpus / "linked.txt").symlink_to(corpus / "a.txt")
    with pytest.raises(semble_adapter.AdapterError, match="uses a symlink"):
        semble_adapter.mapping_proof(admitted, ["a.txt", "b.txt", "linked.txt"], corpus)
    (corpus / "linked.txt").unlink()

    skipped, _ = semble_adapter.mapping_proof(admitted, ["a.txt"], corpus)
    assert skipped["skipped"] == ["b.txt"]
    extra, _ = semble_adapter.mapping_proof(admitted, ["a.txt", "b.txt", "zzz.txt"], corpus)
    assert extra["extra"] == ["zzz.txt"]
    assert extra["semble_side"][-1]["readable"] is False

    (corpus / "a.txt").write_bytes(b"different bytes")
    changed, _ = semble_adapter.mapping_proof(admitted, ["a.txt", "b.txt"], corpus)
    assert changed["skipped"] == changed["extra"] == []
    assert changed["mismatched"] == ["a.txt"]
    assert changed["per_file"][0]["status"] == "hash_mismatch"
    manifest = {"files": [{"path": name, "file_sha256": sha} for name, sha in admitted]}
    assert pairrun.mapping_matches_manifest(proof, manifest)
    assert not pairrun.mapping_matches_manifest(changed, manifest)
    tampered = dict(
        proof,
        semble_side=[dict(proof["semble_side"][0], file_sha256="0" * 64), proof["semble_side"][1]],
    )
    assert not pairrun.mapping_matches_manifest(tampered, manifest)


def test_semble_inputs_refuse_duplicate_keys_and_unsafe_paths(tmp_path):
    path = tmp_path / "manifest.json"
    path.write_text('{"tasks":[{"gold":[]}],"tasks":[]}', encoding="utf-8")
    with pytest.raises(semble_adapter.AdapterError, match="duplicate JSON key: tasks"):
        semble_adapter.read_json(path)
    path.write_text(
        json.dumps(
            {
                "repository_commit": "a" * 40,
                "files": [{"path": "../outside.rs", "file_sha256": "b" * 64}],
            }
        ),
        encoding="utf-8",
    )
    with pytest.raises(semble_adapter.AdapterError, match="unsafe manifest path"):
        semble_adapter.load_manifest(path)


def test_pair_driver_refuses_ambiguous_json(tmp_path):
    path = tmp_path / "run-manifest.json"
    path.write_text(
        '{"evidence":{"pair":{"same_files":false}},"evidence":{"pair":{"same_files":true}}}',
        encoding="utf-8",
    )
    with pytest.raises(pairrun.RunError, match="duplicate JSON key: evidence"):
        pairrun.read_json(path)
    path.write_text('{"query_latency_ms":NaN}', encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="non-finite JSON number: NaN"):
        pairrun.read_json(path)


def test_pair_capture_preflight_requires_external_root_and_clean_pin(tmp_path, monkeypatch):
    monkeypatch.setattr(pairrun, "probe_runner_capabilities", lambda _binary: {})
    repo, suite, _run, _sp, _rp, _files = fixture_v3(tmp_path)
    manifest = tmp_path / "manifest.json"
    manifest.write_text(
        json.dumps({"repository_commit": suite["repository_commit"]}), encoding="utf-8"
    )
    suite_file = tmp_path / "suite.json"
    suite_file.write_text(json.dumps(suite), encoding="utf-8")
    searchd = tmp_path / "searchd"
    searchd.write_bytes(b"searchd-binary")
    spec = {
        "repo": str(repo),
        "manifest": str(manifest),
        "suite": str(suite_file),
        "top_k": 10,
        "output_root": str(repo / "capture"),
        "searchd_binary": str(searchd),
        "searchd_expected_sha256": ev.digest(b"searchd-binary"),
        "runner_binary": str(tmp_path / "runner"),
    }
    with pytest.raises(pairrun.RunError, match="outside the frozen repository"):
        pairrun.preflight_capture(spec)
    spec["output_root"] = str(tmp_path / "capture")
    assert pairrun.preflight_capture(spec) == tmp_path / "capture"
    (repo / "untracked.txt").write_text("dirty", encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="checkout has tracked or untracked changes"):
        pairrun.preflight_capture(spec)
    (repo / "untracked.txt").unlink()
    spec["top_k"] = 5
    with pytest.raises(pairrun.RunError, match="spec top_k differs"):
        pairrun.preflight_capture(spec)
    spec["top_k"] = 10
    spec["searchd_expected_sha256"] = "0" * 64
    with pytest.raises(pairrun.RunError, match="searchd binary digest differs"):
        pairrun.preflight_capture(spec)


def test_runner_capability_probe_refuses_stale_and_malformed_binaries(monkeypatch, tmp_path):
    binary = tmp_path / "runner"
    observed = []

    def response(argv, **kwargs):
        observed.append((argv, kwargs["timeout"]))
        return subprocess.CompletedProcess(
            argv,
            0,
            '{"schema_version":1,"retrieval_diagnostic_schema_version":8,'
            '"completed_response_output_validation":"normalized_row_score_bits_sha256_v1"}',
            "",
        )

    monkeypatch.setattr(pairrun.subprocess, "run", response)
    assert pairrun.probe_runner_capabilities(binary)["retrieval_diagnostic_schema_version"] == 8
    assert observed == [([str(binary), "capabilities"], 10)]

    for marker in (None, True, "unchecked"):
        payload = {"schema_version": 1, "retrieval_diagnostic_schema_version": 8}
        if marker is not None:
            payload["completed_response_output_validation"] = marker
        monkeypatch.setattr(
            pairrun.subprocess, "run",
            lambda argv, payload=payload, **_kwargs: subprocess.CompletedProcess(
                argv, 0, json.dumps(payload), ""
            ),
        )
        with pytest.raises(pairrun.RunError, match="capability response invalid|contract differs"):
            pairrun.probe_runner_capabilities(binary)

    monkeypatch.setattr(
        pairrun.subprocess,
        "run",
        lambda argv, **_kwargs: subprocess.CompletedProcess(
            argv, 0, '{"schema_version":1,"retrieval_diagnostic_schema_version":7,'
            '"completed_response_output_validation":"normalized_row_score_bits_sha256_v1"}', ""
        ),
    )
    with pytest.raises(pairrun.RunError, match="diagnostic contract differs"):
        pairrun.probe_runner_capabilities(binary)
    monkeypatch.setattr(
        pairrun.subprocess,
        "run",
        lambda argv, **_kwargs: subprocess.CompletedProcess(argv, 2, "", "unknown command"),
    )
    with pytest.raises(pairrun.RunError, match="probe exited 2"):
        pairrun.probe_runner_capabilities(binary)
    monkeypatch.setattr(
        pairrun.subprocess,
        "run",
        lambda argv, **_kwargs: subprocess.CompletedProcess(
            argv,
            0,
            '{"schema_version":1,"schema_version":1,"retrieval_diagnostic_schema_version":8}',
            "",
        ),
    )
    with pytest.raises(pairrun.RunError, match="duplicate JSON key"):
        pairrun.probe_runner_capabilities(binary)


@pytest.mark.parametrize("policy", ["code_search_file", "natural_language_file"])
def test_qualified_default_file_preflight_refuses_mechanical_positive(
    tmp_path, monkeypatch, policy
):
    monkeypatch.setattr(pairrun, "probe_runner_capabilities", lambda _binary: {})
    repo, suite, _run, _sp, _rp, _files = fixture_v3(tmp_path)
    manifest = tmp_path / "manifest.json"
    manifest.write_text(json.dumps({"repository_commit": suite["repository_commit"]}))
    suite_file = tmp_path / "suite.json"
    searchd = tmp_path / "searchd"
    searchd.write_bytes(b"searchd-binary")
    spec = {
        "repo": str(repo),
        "manifest": str(manifest),
        "suite": str(suite_file),
        "top_k": 10,
        "output_root": str(tmp_path / "capture"),
        "searchd_binary": str(searchd),
        "searchd_expected_sha256": ev.digest(b"searchd-binary"),
        "runner_binary": str(tmp_path / "runner"),
        "scope": "qualified",
        "execution_profiles": {"quanta": {"policy": policy}},
    }
    positive = suite["tasks"][0]
    positive["evaluation_contract"] = {
        "request_mode": (
            "natural_language_file_search"
            if policy == "natural_language_file"
            else "default_file_search"
        ),
        "gold_unit": "distinct_file",
        "result_unit": "distinct_file",
    }
    positive["judgment_policy"] = ev.SOURCE_ORACLE_JUDGMENT_POLICY
    if policy == "natural_language_file":
        positive["query_intent"] = "semantic_intent"
    positive["source_oracle"] = {
        "contract": "go_declaration_name_components_v1",
        "unit": "distinct_file",
    }
    suite_file.write_text(json.dumps(suite))
    with pytest.raises(pairrun.RunError, match="independently reviewed complete relevance"):
        pairrun.preflight_capture(spec)

    positive.pop("source_oracle")
    positive["judgment_policy"] = ev.COMPLETE_JUDGMENT_POLICY
    negative = suite["tasks"][1]
    negative["evaluation_contract"] = dict(positive["evaluation_contract"])
    negative["judgment_policy"] = ev.SOURCE_ORACLE_JUDGMENT_POLICY
    negative["source_oracle"] = {
        "contract": "go_declaration_name_components_v1",
        "unit": "distinct_file",
    }
    if policy == "natural_language_file":
        negative["query_intent"] = "semantic_intent"
        suite_file.write_text(json.dumps(suite))
        with pytest.raises(pairrun.RunError, match="independently reviewed complete relevance"):
            pairrun.preflight_capture(spec)
        negative.pop("source_oracle")
        negative["judgment_policy"] = ev.COMPLETE_JUDGMENT_POLICY
    suite_file.write_text(json.dumps(suite))
    assert pairrun.preflight_capture(spec) == tmp_path / "capture"

    for field in ("gold_unit", "result_unit"):
        positive["evaluation_contract"][field] = "symbol"
        suite_file.write_text(json.dumps(suite))
        with pytest.raises(pairrun.RunError, match="distinct_file gold and results"):
            pairrun.preflight_capture(spec)
        positive["evaluation_contract"][field] = "distinct_file"
    if policy == "natural_language_file":
        positive["query_intent"] = "bare_symbol"
        suite_file.write_text(json.dumps(suite))
        with pytest.raises(pairrun.RunError, match="requires semantic_intent"):
            pairrun.preflight_capture(spec)
        positive["query_intent"] = "semantic_intent"

    negative.pop("evaluation_contract")
    suite_file.write_text(json.dumps(suite))
    with pytest.raises(pairrun.RunError, match="declared file request mode"):
        pairrun.preflight_capture(spec)


def _pinned_semble_cache(tmp_path):
    cache = tmp_path / "semble-cache"
    revision = "a" * 40
    slug = "models--minishlab--potion-code-16M-v2"
    ref = cache / "hf" / "hub" / slug / "refs" / "main"
    ref.parent.mkdir(parents=True)
    ref.write_text(revision + "\n", encoding="utf-8")
    snapshot = cache / "hf" / "hub" / slug / "snapshots" / revision
    snapshot.mkdir(parents=True)
    (snapshot / "config.json").write_bytes(b"{}")
    return cache, revision


def _pair_model_preflight_spec(tmp_path, cache=None, revision=None):
    spec = {
        "semble_lockfile_sha256": _fake_sha("lock"),
        "semble_python": "/pinned/python",
        "semble_lockfile": "/pinned/lock",
        "host_profile": "/pinned/host",
        "output_root": str(tmp_path / "pair"),
    }
    if cache is not None:
        spec["semble_cache_root"] = str(cache)
    if revision is not None:
        spec["semble_model_revision"] = revision
    return spec


def test_pair_model_preflight_rejects_unpinned_or_missing_cache_before_stage(tmp_path, monkeypatch):
    cache, revision = _pinned_semble_cache(tmp_path)
    monkeypatch.setattr(
        pairrun, "preflight_capture", lambda _spec: pytest.fail("runner preflight was reached")
    )
    with pytest.raises(pairrun.RunError, match="requires spec.semble_cache_root"):
        pairrun.run_pair(_pair_model_preflight_spec(tmp_path, revision=revision))
    with pytest.raises(pairrun.RunError, match="existing absolute directory"):
        pairrun.run_pair(
            _pair_model_preflight_spec(tmp_path, cache=tmp_path / "absent", revision=revision)
        )
    with pytest.raises(pairrun.RunError, match="requires spec.semble_model_revision"):
        pairrun.run_pair(_pair_model_preflight_spec(tmp_path, cache=cache))
    assert not (tmp_path / "pair.staging").exists()


def test_pair_model_preflight_rejects_stale_cache_before_stage(tmp_path, monkeypatch):
    cache, revision = _pinned_semble_cache(tmp_path)
    monkeypatch.setattr(
        pairrun, "preflight_capture", lambda _spec: pytest.fail("runner preflight was reached")
    )
    with pytest.raises(pairrun.RunError, match="model revision drift"):
        pairrun.run_pair(_pair_model_preflight_spec(tmp_path, cache=cache, revision="b" * 40))
    assert not (tmp_path / "pair.staging").exists()


def test_pair_model_preflight_accepts_pinned_cache_before_capture(tmp_path, monkeypatch):
    cache, revision = _pinned_semble_cache(tmp_path)
    output_root = tmp_path / "pair"
    calls = []
    monkeypatch.setattr(pairrun, "preflight_capture", lambda _spec: output_root)
    monkeypatch.setattr(pairrun, "_source_closure", lambda *_args: None)

    def stop_before_runner(_spec, _stage):
        calls.append("capture")
        raise pairrun.RunError("capture sentinel")

    monkeypatch.setattr(pairrun, "_run_pair_staged", stop_before_runner)
    with pytest.raises(pairrun.RunError, match="capture sentinel"):
        pairrun.run_pair(_pair_model_preflight_spec(tmp_path, cache=cache, revision=revision))
    assert calls == ["capture"]
    assert (tmp_path / "pair.staging").is_dir()
    assert not output_root.exists()


def test_pair_preflights_searchd_socket_length_before_creating_stage(tmp_path, monkeypatch):
    cache, revision = _pinned_semble_cache(tmp_path)
    monkeypatch.setattr(pairrun, "_unix_socket_path_limit", lambda: 103)
    strategies = [{"name": "fixed_window_strict"}]
    pairrun.preflight_daemon_socket_paths(
        Path("/tmp/q.staging"), strategies, repetitions=1, paired=True
    )
    long_root = tmp_path / ("long-" + "x" * 90)
    stage = long_root.with_name(long_root.name + ".staging")
    monkeypatch.setattr(pairrun, "preflight_capture", lambda _spec: long_root)
    with pytest.raises(pairrun.RunError, match="searchd Unix socket path.*shorter output_root"):
        pairrun.run_pair(
            {
                "scope": "exploratory",
                "semble_lockfile_sha256": "a" * 64,
                "semble_python": "python3",
                "semble_lockfile": "lockfile",
                "semble_cache_root": str(cache),
                "semble_model_revision": revision,
                "host_profile": "host-profile",
                "strategies": strategies,
            }
        )
    assert not stage.exists()


def test_fixed_window_strategy_path_is_short_and_matches_socket_preflight(monkeypatch):
    monkeypatch.setattr(pairrun, "_unix_socket_path_limit", lambda: 103)
    stage = Path("/private/tmp/p5/07e114f9.staging")
    assert pairrun._strategy_run_directory(0, "fixed_window_strict") == "strategy-00-fw_strict"
    assert pairrun._strategy_run_directory(0, "whole_file") == "strategy-00-whole_file"
    pairrun.preflight_daemon_socket_paths(
        stage, [{"name": "fixed_window_strict"}], repetitions=1, paired=True
    )
    with pytest.raises(pairrun.RunError, match="unknown strategy"):
        pairrun._strategy_run_directory(0, "unknown")


def test_semble_model_revision_requires_observed_pinned_cache(tmp_path):
    model = "minishlab/potion-code-16M-v2"
    with pytest.raises(semble_adapter.AdapterError, match="revision unavailable"):
        semble_adapter.resolve_model_revision(tmp_path, model, None)
    ref = tmp_path / "hub/models--minishlab--potion-code-16M-v2/refs/main"
    ref.parent.mkdir(parents=True)
    ref.write_text("a" * 40, encoding="utf-8")
    with pytest.raises(semble_adapter.AdapterError, match="snapshot unavailable"):
        semble_adapter.resolve_model_revision(tmp_path, model, "a" * 40)
    snap = tmp_path / ("hub/models--minishlab--potion-code-16M-v2/snapshots/" + "a" * 40)
    snap.mkdir(parents=True)
    (snap / "config.json").write_bytes(b"{}")
    revision, asset = semble_adapter.resolve_model_revision(tmp_path, model, "a" * 40)
    assert revision == "a" * 40
    assert len(asset) == 64
    with pytest.raises(semble_adapter.AdapterError, match="revision drift"):
        semble_adapter.resolve_model_revision(tmp_path, model, "b" * 40)


def test_semble_env_refuses_missing_lockfile(tmp_path, monkeypatch):
    interpreter = tmp_path / "python"
    interpreter.write_text("", encoding="utf-8")

    def fake_run(command, **kwargs):
        if "-c" in command:
            return subprocess.CompletedProcess(
                command,
                0,
                '{"semble_version":"0.6.0","python_version":"3.11","has_from_path":true,"has_search":true,'
                '"dist_info":"semble-0.6.0.dist-info","record_sha256":"%s","direct_url_sha256":null}'
                % ("a" * 64),
                "",
            )
        return subprocess.CompletedProcess(command, 1, "", "pip failed")

    monkeypatch.setattr(semble_adapter.subprocess, "run", fake_run)
    with pytest.raises(semble_adapter.AdapterError, match="pip freeze failed"):
        semble_adapter.check_semble_env(interpreter)

    def timed_out_probe(command, **_kwargs):
        raise subprocess.TimeoutExpired(command, 120)

    monkeypatch.setattr(semble_adapter.subprocess, "run", timed_out_probe)
    with pytest.raises(semble_adapter.AdapterError, match="env probe failed"):
        semble_adapter.check_semble_env(interpreter)

    def timed_out_freeze(command, **_kwargs):
        if "-c" in command:
            return fake_run(command)
        raise subprocess.TimeoutExpired(command, 120)

    monkeypatch.setattr(semble_adapter.subprocess, "run", timed_out_freeze)
    with pytest.raises(semble_adapter.AdapterError, match="pip freeze failed"):
        semble_adapter.check_semble_env(interpreter)


def test_semble_env_reports_lockfile_digest_and_pair_requires_pin(tmp_path, monkeypatch):
    interpreter = tmp_path / "python"
    interpreter.write_text("", encoding="utf-8")

    installed = (
        '"dist_info":"semble-0.6.0.dist-info","record_sha256":"%s","direct_url_sha256":null'
    ) % ("b" * 64)

    def fake_run(command, **kwargs):
        if "-c" in command:
            return subprocess.CompletedProcess(
                command,
                0,
                '{"semble_version":"0.6.0","python_version":"3.11","has_from_path":true,"has_search":true,'
                + installed
                + "}",
                "",
            )
        return subprocess.CompletedProcess(command, 0, "semble==0.6.0\n", "")

    monkeypatch.setattr(semble_adapter.subprocess, "run", fake_run)
    report = semble_adapter.check_semble_env(interpreter)
    assert report["observed_freeze_sha256"] == ev.digest(b"semble==0.6.0\n")
    assert report["installed_distribution"]["record_sha256"] == "b" * 64
    assert report["interpreter"]["digest"] == ev.digest(b"")
    assert report["interpreter"]["version"] == "3.11"

    def wrong_version(command, **_kwargs):
        if "-c" in command:
            return subprocess.CompletedProcess(
                command,
                0,
                '{"semble_version":"0.7.0","python_version":"3.11","has_from_path":true,"has_search":true}',
                "",
            )
        return subprocess.CompletedProcess(command, 0, "semble==0.7.0\n", "")

    monkeypatch.setattr(semble_adapter.subprocess, "run", wrong_version)
    with pytest.raises(semble_adapter.AdapterError, match="0.6.0 is pinned"):
        semble_adapter.check_semble_env(interpreter)
    with pytest.raises(pairrun.RunError, match="requires a pinned semble_lockfile_sha256"):
        pairrun.run_pair({"output_root": str(tmp_path / "out")})
    assert not (tmp_path / "out").exists()


def test_verify_lockfile_pins_external_file_not_freeze():
    pin_file = b"semble==0.6.0\nnumpy==2.0.0\n# generated lock\n"
    pin = ev.digest(pin_file)
    freeze = "numpy==2.0.0\nsemble==0.6.0\n"
    assert semble_adapter.verify_lockfile(pin_file, pin, freeze) == pin
    with pytest.raises(semble_adapter.AdapterError, match="differs from the spec pin"):
        semble_adapter.verify_lockfile(b"semble==0.6.0\n", pin, freeze)
    with pytest.raises(semble_adapter.AdapterError, match="lacks the pinned Semble line"):
        semble_adapter.verify_lockfile(pin_file, pin, "numpy==2.0.0\n")
    with pytest.raises(semble_adapter.AdapterError, match="lacks 1 locked lines"):
        semble_adapter.verify_lockfile(pin_file, pin, "semble==0.6.0\nextra==1.0\n")
    with pytest.raises(semble_adapter.AdapterError, match="has 1 unlocked lines"):
        semble_adapter.verify_lockfile(pin_file, pin, freeze + "extra==1.0\n")
    duplicate = b"semble==0.6.0\nsemble==0.6.0\n"
    with pytest.raises(semble_adapter.AdapterError, match="duplicate distribution lines"):
        semble_adapter.verify_lockfile(duplicate, ev.digest(duplicate), freeze)
    direct_reference = b"semble==0.6.0\nnumpy @ file:///tmp/numpy.whl\n"
    with pytest.raises(semble_adapter.AdapterError, match="non-version-pinned"):
        semble_adapter.verify_lockfile(direct_reference, ev.digest(direct_reference), freeze)
    duplicate_project = b"semble==0.6.0\nNumPy==2.0.0\nnumpy==2.1.0\n"
    with pytest.raises(semble_adapter.AdapterError, match="more than once"):
        semble_adapter.verify_lockfile(duplicate_project, ev.digest(duplicate_project), freeze)
    invalid_utf8 = b"semble==0.6.0\n\xff"
    with pytest.raises(semble_adapter.AdapterError, match="not UTF-8"):
        semble_adapter.verify_lockfile(invalid_utf8, ev.digest(invalid_utf8), freeze)


def test_check_semble_env_refuses_missing_installed_proof(tmp_path, monkeypatch):
    interpreter = tmp_path / "python"
    interpreter.write_text("", encoding="utf-8")

    def run_with(probe_json):
        def fake_run(command, **_kwargs):
            if "-c" in command:
                return subprocess.CompletedProcess(command, 0, probe_json, "")
            return subprocess.CompletedProcess(command, 0, "semble==0.6.0\n", "")

        monkeypatch.setattr(semble_adapter.subprocess, "run", fake_run)
        return semble_adapter.check_semble_env(interpreter)

    base = (
        '{"semble_version":"0.6.0","python_version":"3.11","has_from_path":true,"has_search":true'
    )
    with pytest.raises(semble_adapter.AdapterError, match="dist-info identity proof"):
        run_with(base + "}")
    with pytest.raises(semble_adapter.AdapterError, match="RECORD digest proof"):
        run_with(base + ',"dist_info":"semble-0.6.0.dist-info","record_sha256":null}')
    with pytest.raises(semble_adapter.AdapterError, match="malformed direct_url digest"):
        run_with(
            base + ',"dist_info":"semble-0.6.0.dist-info",'
            f'"record_sha256":"{"c" * 64}","direct_url_sha256":"zz"}}'
        )
    report = run_with(
        base + ',"dist_info":"semble-0.6.0.dist-info",'
        f'"record_sha256":"{"d" * 64}","direct_url_sha256":"{"e" * 64}"}}'
    )
    assert report["installed_distribution"]["direct_url_sha256"] == "e" * 64


def test_load_query_pack_refuses_duplicate_task_ids(tmp_path):
    repo, suite, _run, _sp, _rp, _files = fixture_v3(tmp_path, answerable_only=True)
    _s, pack, _src = ev.validate_suite(repo, suite)
    assert len(pack["tasks"]) >= 1
    pack["tasks"].append(dict(pack["tasks"][0]))
    pack_path = tmp_path / "pack.json"
    pack_path.write_text(json.dumps(pack), encoding="utf-8")
    with pytest.raises(semble_adapter.AdapterError, match="duplicate task_id"):
        semble_adapter.load_query_pack(pack_path)


def test_load_query_pack_refuses_smuggled_labels(tmp_path):
    # T01: the blind adapter refuses packs carrying gold/grade or any
    # unexpected key, at top level and per task.
    repo, suite, _run, _sp, _rp, _files = fixture_v3(tmp_path, answerable_only=True)
    _s, pack, _src = ev.validate_suite(repo, suite)
    pack_path = tmp_path / "pack.json"

    def attempt(mutator, match):
        mutated = json.loads(json.dumps(pack))
        mutator(mutated)
        pack_path.write_text(json.dumps(mutated), encoding="utf-8")
        with pytest.raises(semble_adapter.AdapterError, match=match):
            semble_adapter.load_query_pack(pack_path)

    attempt(lambda p: p["tasks"][0].update(gold=[]), "smuggling refused")
    attempt(lambda p: p["tasks"][0].update(grade=3), "smuggling refused")
    attempt(lambda p: p.update(gold=[]), "unexpected or missing top-level keys")
    pack_path.write_text(json.dumps(pack), encoding="utf-8")
    assert semble_adapter.load_query_pack(pack_path)["tasks"]


def _write_stub_semble(root: Path) -> None:
    """A dual-lane stub mirroring the pinned Semble 0.6.0 module layout."""
    resident_bytes = 64 * 1024 * 1024
    if sys.platform == "linux":
        import resource

        # ru_maxrss survives exec, so the child must exceed pytest's inherited
        # high-water mark before the worker can attribute index residency.
        resident_bytes = max(
            resident_bytes,
            resource.getrusage(resource.RUSAGE_SELF).ru_maxrss * 1024 + 16 * 1024 * 1024,
        )
    package = root / "semble"
    package.mkdir(exist_ok=True)
    (package / "__init__.py").write_text(
        "from semble.index_stub import SembleIndex\n",
        encoding="utf-8",
    )
    (package / "types.py").write_text(
        "class ContentType:\n    CODE = 'code'\n",
        encoding="utf-8",
    )
    (package / "index_stub.py").write_text(
        "from semble.search import search\n"
        "from semble.types import ContentType\n"
        "class _Chunk:\n"
        "    def __init__(self, file_path, start_line, end_line):\n"
        "        self.file_path = file_path\n"
        "        self.start_line = start_line\n"
        "        self.end_line = end_line\n"
        "class _Model:\n"
        "    def encode(self, queries):\n"
        "        return [[0.1, 0.2] for _ in queries]\n"
        "class _Stats:\n"
        "    indexed_files = 1\n"
        "    total_chunks = 1\n"
        "    languages = {'txt': 1}\n"
        "class SembleIndex:\n"
        "    @classmethod\n"
        "    def from_path(cls, corpus_dir, show_progress_bar=False):\n"
        "        self = cls()\n"
        f"        self._resident_index = bytearray({resident_bytes})\n"
        "        for offset in range(0, len(self._resident_index), 4096):\n"
        "            self._resident_index[offset] = 1\n"
        "        self.model = _Model()\n"
        "        self._content = ('code',)\n"
        "        self._semantic_index = object()\n"
        "        self._bm25_index = object()\n"
        "        self.chunks = [_Chunk('a.txt', 1, 1)]\n"
        "        self.stats = _Stats()\n"
        "        return self\n"
        "    def search(self, query, top_k=10, alpha=None, rerank=None):\n"
        "        resolved_rerank = ContentType.CODE in self._content if rerank is None else rerank\n"
        "        return search(query, self.model, self._semantic_index, self._bm25_index, self.chunks, top_k, alpha=alpha, selector=None, rerank=resolved_rerank)\n",
        encoding="utf-8",
    )
    (package / "search.py").write_text(
        "from semble.ranking import resolve_alpha\n"
        "class _Hit:\n"
        "    def __init__(self, chunk, score):\n"
        "        self.chunk = chunk\n"
        "        self.score = score\n"
        "def _search_semantic(query, model, semantic_index, chunks, top_k, selector):\n"
        "    model.encode([query])\n"
        "    return [_Hit(chunks[0], 0.9)]\n"
        "def _search_bm25(query, bm25_index, chunks, top_k, selector):\n"
        "    return [_Hit(chunks[0], 0.5)]\n"
        "def search(query, model, semantic_index, bm25_index, chunks, top_k, alpha=None, selector=None, rerank=True):\n"
        "    resolved = resolve_alpha(query, alpha)\n"
        "    candidate_count = top_k * 5\n"
        "    semantic = _search_semantic(query, model, semantic_index, chunks, candidate_count, selector)\n"
        "    lexical = _search_bm25(query, bm25_index, chunks, candidate_count, selector)\n"
        "    return semantic + lexical\n",
        encoding="utf-8",
    )
    (package / "ranking.py").write_text(
        "def resolve_alpha(query, alpha=None):\n"
        "    if alpha is not None:\n"
        "        return float(alpha)\n"
        "    return 0.5 if '?' in query else 1.0\n",
        encoding="utf-8",
    )


def _run_protocol_worker_fixture(worker, spec, *, normalization_hook=None):
    """Exercise the production parent/worker path with real span normalization."""
    tasks = {task["task_id"]: task for task in spec["tasks"]}
    source = b"def fixture(): pass\n"
    file_shas = {"a.txt": ev.digest(source)}
    file_lines = {"a.txt": source.splitlines(keepends=True)}
    profile = semble_adapter.execution_profile(
        spec.get("semble_profile", "native-default"), spec.get("alpha")
    )
    contract = _v3_contract(spec["top_k"])

    def normalize(task_id, hits, indexed_chunks):
        if normalization_hook:
            normalization_hook()
        pack = {"tasks": [tasks[task_id]], "comparison_contract": contract}
        return semble_adapter.normalize_results(
            pack,
            [{"task_id": task_id, "results": hits}],
            {},
            file_shas,
            file_lines,
            contract,
            "hybrid",
            profile,
            indexed_chunks=indexed_chunks,
        )[0]

    stderr_path = worker.parent / "protocol.stderr"
    command = [sys.executable, str(worker)]
    try:
        completed, _completed_rows = semble_adapter.run_completed_worker(
            command,
            env=dict(os.environ),
            timeout_secs=60,
            tasks={key: value["query"] for key, value in tasks.items()},
            top_k=spec["top_k"],
            route="hybrid",
            normalize_response=normalize,
            stderr_path=stderr_path,
        )
        return completed
    except semble_adapter.AdapterError:
        return subprocess.CompletedProcess(command, 1, "", stderr_path.read_text())


def test_worker_template_runs_against_stub_semble(tmp_path, monkeypatch):
    _write_stub_semble(tmp_path)
    worker = tmp_path / "worker.py"
    worker.write_text(semble_adapter.WORKER_TEMPLATE, encoding="utf-8")
    spec = {
        "corpus_dir": str(tmp_path),
        "tasks": [{"task_id": "T1", "query": "q"}],
        "top_k": 5,
        "seed": 0,
        "warmup_passes": 1,
        "repetitions": 2,
        "query_protocol": pairrun.build_query_protocol(["T1"], 0, 1, 2),
        "execution_profile_sha256": ev.digest(
            ev.canonical(semble_adapter.execution_profile("native-default", None))
        ),
    }
    spec_path = tmp_path / "spec.json"
    native_path = tmp_path / "native.json"
    spec_path.write_text(json.dumps(spec), encoding="utf-8")
    monkeypatch.setenv("SPEC_JSON", str(spec_path))
    monkeypatch.setenv("NATIVE_JSON", str(native_path))
    monkeypatch.setenv("SEMBLE_MODEL_NAME", "stub-model")
    monkeypatch.setenv("PYTHONPATH", str(tmp_path))

    def run_worker():
        return _run_protocol_worker_fixture(worker, spec)

    completed = run_worker()
    assert completed.returncode == 0, completed.stderr
    payload = json.loads(native_path.read_text(encoding="utf-8"))
    assert payload["query_timing"]["boundary"] == semble_adapter.QUERY_TIMING_BOUNDARY
    assert payload["query_timing"]["clock"] == semble_adapter.QUERY_TIMING_CLOCK
    assert len(payload["query_timing"]["observations"]) == 4
    assert all(row["status"] == "success" for row in payload["query_timing"]["observations"])
    assert payload["worker_pid"] > 0
    assert [row["task_id"] for row in payload["native"]] == ["T1"]
    assert len(payload["latencies_ms"]["T1"]) == 2
    assert payload["query_protocol"] == spec["query_protocol"]
    assert payload["cold_latency_ms"] >= 0
    assert payload["native"][0]["results"][0]["file_path"] == "a.txt"
    assert payload["observed_files"] == ["a.txt"]
    phases, total = semble_adapter.validate_worker_phase_timings(payload, protocol=True)
    assert total == payload["worker_total_ms"]
    assert phases["unattributed"] >= 0
    for key in ("discovery_ms", "worker_total_ms"):
        with pytest.raises(semble_adapter.AdapterError, match="finite|inconsistent"):
            semble_adapter.validate_worker_phase_timings(
                dict(payload, **{key: json.loads("1e309")}), protocol=True
            )
    with pytest.raises(semble_adapter.AdapterError, match="inconsistent"):
        semble_adapter.validate_worker_phase_timings(
            dict(payload, worker_total_ms=0), protocol=True
        )

    spec["tasks"].append({"task_id": "T1", "query": "q2"})
    spec_path.write_text(json.dumps(spec), encoding="utf-8")
    duplicate = run_worker()
    assert duplicate.returncode != 0
    assert "duplicate task_ids" in duplicate.stderr


def test_worker_template_preserves_all_measured_rows_after_encoding(tmp_path, monkeypatch):
    _write_stub_semble(tmp_path)
    worker = tmp_path / "worker.py"
    worker.write_text(semble_adapter.WORKER_TEMPLATE, encoding="utf-8")
    tasks = [{"task_id": "T1", "query": "first"}, {"task_id": "T2", "query": "second"}]
    protocol = pairrun.build_query_protocol([task["task_id"] for task in tasks], 0, 1, 1)
    spec = {
        "corpus_dir": str(tmp_path),
        "tasks": tasks,
        "top_k": 5,
        "seed": 0,
        "warmup_passes": 1,
        "repetitions": 1,
        "query_protocol": protocol,
        "semble_profile": "lexical-file",
        "execution_profile_sha256": ev.digest(
            ev.canonical(semble_adapter.execution_profile("lexical-file", None))
        ),
    }
    spec_path = tmp_path / "spec.json"
    native_path = tmp_path / "native.json"
    spec_path.write_text(json.dumps(spec), encoding="utf-8")
    monkeypatch.setenv("SPEC_JSON", str(spec_path))
    monkeypatch.setenv("NATIVE_JSON", str(native_path))
    monkeypatch.setenv("SEMBLE_MODEL_NAME", "stub-model")
    monkeypatch.setenv("PYTHONPATH", str(tmp_path))

    completed = _run_protocol_worker_fixture(worker, spec)
    assert completed.returncode == 0, completed.stderr
    payload = json.loads(native_path.read_bytes())
    assert [row["task_id"] for row in payload["native"]] == protocol["measurement_schedules"][0]
    assert all(
        len(row["results"]) == 1 and row["results"][0]["score"] == 0.5 for row in payload["native"]
    )
    assert len(payload["query_timing"]["observations"]) == 5


def test_worker_template_dispatches_profiles_with_lane_isolation(tmp_path, monkeypatch):
    _write_stub_semble(tmp_path)
    worker = tmp_path / "worker.py"
    worker.write_text(semble_adapter.WORKER_TEMPLATE, encoding="utf-8")
    spec_path = tmp_path / "spec.json"
    native_path = tmp_path / "native.json"
    monkeypatch.setenv("SPEC_JSON", str(spec_path))
    monkeypatch.setenv("NATIVE_JSON", str(native_path))
    monkeypatch.setenv("SEMBLE_MODEL_NAME", "stub-model")
    monkeypatch.setenv("PYTHONPATH", str(tmp_path))

    def run_profile(profile: str, alpha: float | None = None):
        spec = {
            "corpus_dir": str(tmp_path),
            "tasks": [{"task_id": "T1", "query": "where is it?"}],
            "top_k": 5,
            "seed": 0,
            "warmup_passes": 1,
            "repetitions": 1,
            "semble_profile": profile,
        }
        if alpha is not None:
            spec["alpha"] = alpha
        try:
            requested_profile = semble_adapter.execution_profile(profile, alpha)
        except semble_adapter.AdapterError:
            requested_profile = None
        spec["execution_profile_sha256"] = (
            ev.digest(ev.canonical(requested_profile))
            if requested_profile is not None
            else "0" * 64
        )
        spec_path.write_text(json.dumps(spec), encoding="utf-8")
        for stale in (native_path,):
            stale.unlink(missing_ok=True)
        try:
            completed = _run_protocol_worker_fixture(worker, spec)
        except semble_adapter.AdapterError:
            # Invalid profile arguments must still be rejected by the worker.
            completed = subprocess.run(
                [sys.executable, str(worker)], capture_output=True, text=True, timeout=60
            )
        return completed, (
            json.loads(native_path.read_text(encoding="utf-8")) if native_path.exists() else None
        )

    # native-default: both lanes run; actual alpha comes from the pinned
    # resolver (stub: no '?'-free path → 0.5).
    completed, payload = run_profile("native-default")
    assert completed.returncode == 0, completed.stderr
    assert payload["semble_profile"] == "native-default"
    assert payload["requested_alpha"] is None
    counts = payload["lane_call_counts"]
    assert counts["bm25"] > 0 and counts["semantic"] > 0 and counts["encode"] > 0
    assert payload["actual_alpha_by_task"] == {"T1": 0.5}
    assert payload["rerank_applied"] is True

    # hybrid-no-rerank: explicit alpha echoed, rerank reported disabled,
    # and both lanes still execute (alpha endpoints are ablations).
    completed, payload = run_profile("hybrid-no-rerank", alpha=0.0)
    assert completed.returncode == 0, completed.stderr
    assert payload["semble_profile"] == "hybrid-no-rerank"
    assert payload["requested_alpha"] == 0.0
    assert payload["rerank_applied"] is False
    assert payload["actual_alpha_by_task"] == {"T1": 0.0}
    assert payload["lane_call_counts"]["bm25"] > 0
    assert payload["lane_call_counts"]["semantic"] > 0

    # lexical-only: zero semantic/encode lane calls — proven at the source.
    # No query protocol: one warmup pass + one measured pass = 2 dispatches.
    completed, payload = run_profile("lexical-only")
    assert completed.returncode == 0, completed.stderr
    assert payload["lane_call_counts"] == {"bm25": 2, "semantic": 0, "encode": 0}
    assert payload["actual_alpha_by_task"] is None
    assert payload["rerank_applied"] is False

    completed, payload = run_profile("lexical-file")
    assert completed.returncode == 0, completed.stderr
    assert payload["lane_call_counts"] == {"bm25": 2, "semantic": 0, "encode": 0}
    assert all(
        event["candidate_depth"] == payload["stats"]["total_chunks"]
        for event in payload["execution_events"]
    )
    semble_adapter.validate_native_profile_report(payload, "lexical-file", None)
    forged = copy.deepcopy(payload)
    for event in forged["execution_events"]:
        event["candidate_depth"] += 1
        event["lane_candidate_depths"]["bm25"] = [event["candidate_depth"]]
    forged["execution_events_sha256"] = ev.digest(ev.canonical(forged["execution_events"]))
    with pytest.raises(semble_adapter.AdapterError, match="every indexed chunk"):
        semble_adapter.validate_native_profile_report(forged, "lexical-file", None)

    # semantic-only: zero BM25 calls.
    completed, payload = run_profile("semantic-only")
    assert completed.returncode == 0, completed.stderr
    assert payload["lane_call_counts"] == {"bm25": 0, "semantic": 2, "encode": 2}
    assert payload["rerank_applied"] is False

    # Unknown profiles are typed refusals, never a fallback.
    completed, payload = run_profile("telepathy")
    assert completed.returncode != 0
    assert "unknown semble profile" in completed.stderr
    assert payload is None

    # hybrid-no-rerank without a valid alpha refuses.
    completed, _ = run_profile("hybrid-no-rerank", alpha=None)
    assert completed.returncode != 0
    assert "alpha" in completed.stderr
    for invalid in (10**400, -(10**400), float("nan"), float("inf"), -float("inf"), True):
        completed, _ = run_profile("hybrid-no-rerank", alpha=invalid)
        assert completed.returncode != 0
        assert "alpha" in completed.stderr
        assert "OverflowError" not in completed.stderr


def test_adapter_rejects_forged_or_mismatched_profile_reports():
    for mode in ([], {}, None, True, 10**400):
        profile = {
            "profile_id": "semble-hybrid-no-rerank-v1",
            "mode": mode,
            "alpha": 0.5,
            "rerank": False,
        }
        with pytest.raises(semble_adapter.AdapterError, match="mode"):
            semble_adapter.execution_profile(mode, 0.5)
        with pytest.raises(ev.EvidenceError, match="mode"):
            ev.validate_execution_profile(profile, "semble", "profile")
        with pytest.raises(pairrun.RunError, match="mode"):
            pairrun._validate_semble_profile(profile, "profile")
    for alpha in (10**400, -(10**400), float("nan"), float("inf"), -float("inf"), True, None):
        profile = {
            "profile_id": "semble-hybrid-no-rerank-v1",
            "mode": "hybrid-no-rerank",
            "alpha": alpha,
            "rerank": False,
        }
        with pytest.raises(semble_adapter.AdapterError, match="alpha"):
            semble_adapter.execution_profile("hybrid-no-rerank", alpha)
        with pytest.raises(ev.EvidenceError, match="alpha"):
            ev.validate_execution_profile(profile, "semble", "profile")
        with pytest.raises(pairrun.RunError, match="hybrid profile"):
            pairrun._validate_semble_profile(profile, "profile")
    assert semble_adapter.execution_profile("hybrid-no-rerank", 0.5)["alpha"] == 0.5

    def event(mode: str, alpha: float | None, lanes: dict, depths: dict, rerank):
        profile = semble_adapter.execution_profile(mode, alpha)
        return {
            "rep": 0,
            "phase": "measured",
            "phase_iteration": 0,
            "task_id": "T1",
            "call_ordinal": 0,
            "submitted_query_sha256": _fake_sha("query"),
            "profile_sha256": ev.digest(ev.canonical(profile)),
            "actual_alpha": alpha,
            "actual_rerank": rerank,
            "candidate_depth": 5 if mode.endswith("only") else 25,
            "lane_entry_counts": lanes,
            "lane_candidate_depths": depths,
        }

    function_identity = {
        name: {
            "module": "semble.search",
            "qualname": name,
            "source_sha256": _fake_sha(f"profile-{name}"),
        }
        for name in ("bm25", "index_search", "module_search", "resolve_alpha", "semantic")
    }

    def complete(payload: dict) -> dict:
        value = json.loads(json.dumps(payload))
        value.setdefault("query_schedule", ["T1"])
        value.setdefault("query_protocol", None)
        value.setdefault("warmup_passes", 0)
        value.setdefault("actual_alpha_by_task", None)
        value.setdefault("function_identity", function_identity)
        value.setdefault("observed_wrapped_call_ns", 17)
        value["execution_events_sha256"] = ev.digest(ev.canonical(value["execution_events"]))
        return value

    base = complete(
        {
            "semble_profile": "lexical-only",
            "lane_call_counts": {"bm25": 1, "semantic": 0, "encode": 0},
            "rerank_applied": False,
            "native": [{"task_id": "T1", "results": []}],
            "repetitions": 1,
            "execution_events": [
                event(
                    "lexical-only",
                    None,
                    {"bm25": 1, "semantic": 0},
                    {"bm25": [5], "semantic": []},
                    None,
                )
            ],
            "actual_alpha_by_task": None,
        }
    )
    semble_adapter.validate_native_profile_report(dict(base), "lexical-only", None)
    for field in ("rep", "phase_iteration", "call_ordinal"):
        for invalid in (False, 0.0):
            forged = json.loads(json.dumps(base))
            forged["execution_events"][0][field] = invalid
            forged["execution_events_sha256"] = ev.digest(ev.canonical(forged["execution_events"]))
            with pytest.raises(semble_adapter.AdapterError, match="integer identity"):
                semble_adapter.validate_native_profile_report(forged, "lexical-only", None)
    for field in ("repetitions", "warmup_passes", "event_lane_count"):
        forged = json.loads(json.dumps(base))
        if field == "event_lane_count":
            forged["execution_events"][0]["lane_entry_counts"]["bm25"] = 10**400
            forged["execution_events_sha256"] = ev.digest(ev.canonical(forged["execution_events"]))
        else:
            forged[field] = 10**400
        with pytest.raises(semble_adapter.AdapterError, match="event count|candidate depth"):
            semble_adapter.validate_native_profile_report(forged, "lexical-only", None)

    # Worker echoing a different profile than requested.
    forged = dict(base, semble_profile="semantic-only")
    with pytest.raises(semble_adapter.AdapterError, match="different profile"):
        semble_adapter.validate_native_profile_report(forged, "lexical-only", None)

    # lexical-only claiming zero BM25 calls (no lane ran at all).
    idle = dict(base, lane_call_counts={"bm25": 0, "semantic": 0, "encode": 0})
    with pytest.raises(semble_adapter.AdapterError, match="ran no BM25 lane"):
        semble_adapter.validate_native_profile_report(idle, "lexical-only", None)

    # lexical-only secretly executing the semantic lane.
    leaked = dict(base, lane_call_counts={"bm25": 2, "semantic": 2, "encode": 2})
    with pytest.raises(semble_adapter.AdapterError, match="semantic/encode lanes"):
        semble_adapter.validate_native_profile_report(leaked, "lexical-only", None)

    # semantic-only secretly executing BM25.
    semantic = {
        "semble_profile": "semantic-only",
        "lane_call_counts": {"bm25": 1, "semantic": 2, "encode": 2},
    }
    with pytest.raises(semble_adapter.AdapterError, match="BM25 lane"):
        semble_adapter.validate_native_profile_report(semantic, "semantic-only", 0.5)

    # hybrid-no-rerank must echo alpha and report rerank disabled.
    hybrid = complete(
        {
            "semble_profile": "hybrid-no-rerank",
            "lane_call_counts": {"bm25": 1, "semantic": 1, "encode": 1},
            "requested_alpha": 0.25,
            "rerank_applied": False,
            "native": [{"task_id": "T1", "results": []}],
            "repetitions": 1,
            "execution_events": [
                event(
                    "hybrid-no-rerank",
                    0.25,
                    {"bm25": 1, "semantic": 1},
                    {"bm25": [25], "semantic": [25]},
                    False,
                )
            ],
            "actual_alpha_by_task": {"T1": 0.25},
        }
    )
    semble_adapter.validate_native_profile_report(dict(hybrid), "hybrid-no-rerank", 0.25)
    with pytest.raises(semble_adapter.AdapterError, match="alpha echo"):
        semble_adapter.validate_native_profile_report(dict(hybrid), "hybrid-no-rerank", 0.5)
    reranked = dict(hybrid, rerank_applied=True)
    with pytest.raises(semble_adapter.AdapterError, match="rerank disabled"):
        semble_adapter.validate_native_profile_report(reranked, "hybrid-no-rerank", 0.25)
    forged_encode = dict(hybrid, lane_call_counts={"bm25": 1, "semantic": 1, "encode": 9})
    with pytest.raises(semble_adapter.AdapterError, match="semantic and encode"):
        semble_adapter.validate_native_profile_report(forged_encode, "hybrid-no-rerank", 0.25)

    native = complete(
        {
            "semble_profile": "native-default",
            "lane_call_counts": {"bm25": 1, "semantic": 1, "encode": 1},
            "rerank_applied": True,
            "native": [{"task_id": "T1", "results": []}],
            "repetitions": 1,
            "execution_events": [
                event(
                    "native-default",
                    None,
                    {"bm25": 1, "semantic": 1},
                    {"bm25": [25], "semantic": [25]},
                    True,
                )
            ],
            "actual_alpha_by_task": {"T1": 0.5},
        }
    )
    native["execution_events"][0]["actual_alpha"] = 0.5
    native["execution_events_sha256"] = ev.digest(ev.canonical(native["execution_events"]))
    semble_adapter.validate_native_profile_report(native, "native-default", None)
    for malformed in (
        dict(native, rerank_applied=None),
        dict(native, rerank_applied=False),
        {key: value for key, value in native.items() if key != "rerank_applied"},
    ):
        with pytest.raises(semble_adapter.AdapterError, match="rerank_applied"):
            semble_adapter.validate_native_profile_report(malformed, "native-default", None)

    # Malformed lane reports refuse.
    for malformed in (
        {"semble_profile": "lexical-only"},
        dict(base, lane_call_counts={"bm25": 2}),
        dict(base, lane_call_counts={"bm25": -1, "semantic": 0, "encode": 0}),
    ):
        with pytest.raises(semble_adapter.AdapterError, match="malformed|invalid"):
            semble_adapter.validate_native_profile_report(malformed, "lexical-only", None)
    for malformed in (
        dict(base, lane_call_counts={"bm25": 2, "semantic": 0, "encode": 0}),
        dict(base, execution_events=[dict(base["execution_events"][0], candidate_depth=True)]),
        dict(
            base,
            execution_events=[
                dict(
                    base["execution_events"][0], lane_candidate_depths={"bm25": [], "semantic": []}
                )
            ],
        ),
    ):
        with pytest.raises(
            semble_adapter.AdapterError, match="aggregate lane calls|candidate depth"
        ):
            semble_adapter.validate_native_profile_report(malformed, "lexical-only", None)
    out_of_range_alpha = dict(
        native, execution_events=[dict(native["execution_events"][0], actual_alpha=1.5)]
    )
    with pytest.raises(semble_adapter.AdapterError, match="actual alpha is invalid"):
        semble_adapter.validate_native_profile_report(out_of_range_alpha, "native-default", None)

    wrong_ordinal = complete(
        dict(base, execution_events=[dict(base["execution_events"][0], call_ordinal=999)])
    )
    with pytest.raises(semble_adapter.AdapterError, match="order differs"):
        semble_adapter.validate_native_profile_report(wrong_ordinal, "lexical-only", None)


def test_ucd17_scalar_property_and_normalization_oracle_is_pinned():
    property_digest = hashlib.sha256()
    count = 0
    for scalar in range(0x110000):
        included = scalar < 0xD800 or scalar > 0xDFFF
        included = included and qp._is_token_char(chr(scalar))
        property_digest.update(bytes([int(included)]))
        count += int(included)
    assert count == 150_270
    assert property_digest.hexdigest() == (
        "e711b4f9486890857e77db0d581642531caa7b41d4350a5911b84ea4ea862b24"
    )
    assert qp.unicodedata2.unidata_version == "17.0.0"
    assert qp.regex.__version__ == "2025.10.23"
    for source, normalized in (
        ("cafe\u0301", "caf\u00e9"),
        ("A\u030a", "\u00c5"),
        ("\u1100\u1161", "\uac00"),
        ("\u212b", "\u00c5"),
    ):
        assert qp.unicodedata2.normalize("NFC", source) == normalized
    accepted = '"' + "a" * 16_382 + '"'
    assert len(accepted.encode()) == qp.MAX_INPUT_BYTES
    assert qp.plan_lexical_request("native", accepted) == accepted
    with pytest.raises(qp.QueryPlanError, match="16384"):
        qp.plan_lexical_request("native", accepted + "a")


from tools.benchmark.retrieval import test_sourcegraph as sourcegraph_cases  # noqa: E402


class TestSourcegraphComparator(sourcegraph_cases.SourcegraphCaptureTests):
    """Include offline code-search comparator contract cases in the RB proof rail."""


def test_run_pair_requires_lockfile_path(tmp_path):
    base = {
        "output_root": str(tmp_path / "out"),
        "semble_python": "/venv/bin/python",
        "semble_lockfile_sha256": "a" * 64,
    }
    with pytest.raises(pairrun.RunError, match="spec.semble_lockfile"):
        pairrun.run_pair(dict(base))
    with pytest.raises(pairrun.RunError, match="spec.host_profile"):
        pairrun.run_pair(dict(base, semble_lockfile=str(tmp_path / "lock.txt")))


def _mock_unbound_resource_metrics():
    return {
        "schema_version": 1,
        "sampler": "ps-process-tree-rss-cpu-v2",
        "sample_interval_ms": 50,
        "command_sha256": _fake_sha("mock-command"),
        "subject_sha256": _fake_sha("mock-subject"),
        "root_pid": 100,
        "exit_code": 0,
        "timed_out": False,
        "elapsed_ms": 1.0,
        "peak_rss_bytes": 4096,
        "peak_cpu_percent": 0.0,
        "processes": [
            {
                "pid": 100,
                "command": "mock-runner",
                "peak_rss_bytes": 4096,
                "peak_cpu_percent": 0.0,
                "samples": 1,
            }
        ],
        "samples": 1,
        "complete": True,
        "error": None,
        "cleanup_complete": True,
        "cleanup_escalated": False,
        "cleanup_error": None,
    }


def _mock_bound_resource_metrics():
    metrics = _mock_unbound_resource_metrics()
    metrics["storage"] = {
        "index_bytes": 4096,
        "model_cache_bytes": 0,
        "parser_cache_bytes": 0,
        "embedding_cache_bytes": 0,
        "discovered_files": 1,
        "indexed_chunks": 1,
        "index_storage": "disk",
        "index_measurement": "filesystem_tree_v1",
    }
    return metrics


@pytest.mark.parametrize(
    "mutate",
    [
        pytest.param(
            lambda value: value["processes"].append(dict(value["processes"][0])), id="duplicate-pid"
        ),
        pytest.param(lambda value: value.update(peak_rss_bytes=1), id="underreported-rss"),
        pytest.param(
            lambda value: value["processes"][0].update(peak_cpu_percent=1.0),
            id="underreported-cpu",
        ),
        pytest.param(
            lambda value: value["processes"][0].update(samples=2), id="row-samples-exceed-total"
        ),
    ],
)
def test_macos_resource_replay_rejects_inconsistent_process_rows(mutate):
    metrics = _mock_bound_resource_metrics()
    pairrun._validate_resource_metrics(metrics, "valid macOS resource")
    mutate(metrics)
    with pytest.raises(pairrun.RunError):
        pairrun._validate_resource_metrics(metrics, "mutated macOS resource")


def test_macos_resource_replay_allows_zero_rss_root_without_metric_row():
    metrics = _mock_bound_resource_metrics()
    # The sampler sees the live root but emits only positive-RSS metric rows.
    metrics["processes"][0]["pid"] = 101
    assert pairrun._validate_resource_metrics(metrics, "zero-RSS root") == metrics


def test_run_semble_capture_forwards_lockfile(tmp_path, monkeypatch):
    seen = {}

    def fake_run(command, **kwargs):
        seen["command"] = command
        output_root = Path(command[command.index("--output-root") + 1])
        output_root.mkdir(exist_ok=True)
        (output_root / "phase-metrics.json").write_text("{}", encoding="utf-8")
        (output_root / "native.json").write_text(
            json.dumps(
                {
                    "stats": {
                        "indexed_files": 1,
                        "total_chunks": 1,
                        "index_resident_bytes": 4096,
                        "index_measurement": "process_peak_rss_delta_v1",
                    }
                }
            ),
            encoding="utf-8",
        )
        kwargs["resource_path"].write_text(
            json.dumps(_mock_unbound_resource_metrics()), encoding="utf-8"
        )
        return {"exit_code": 0, "timed_out": False}

    monkeypatch.setattr(pairrun, "run_monitored_process", fake_run)
    spec = {
        "repo": "r",
        "manifest": "m",
        "top_k": 10,
        "semble_python": "/venv/bin/python",
        "semble_lockfile": "/frozen/semble-lockfile.txt",
        "semble_lockfile_sha256": "c" * 64,
        "semble_cache_root": str(tmp_path),
        "semble_model_revision": "a" * 40,
        "execution_profiles": {
            "quanta": qp.execution_profile("native"),
            "semble": semble_adapter.execution_profile("native-default", None),
        },
    }
    pairrun.run_semble_capture(spec, tmp_path, tmp_path / "pack.json", "hybrid")
    command = seen["command"]
    assert command[command.index("--lockfile") + 1] == "/frozen/semble-lockfile.txt"
    assert command[command.index("--lockfile-sha256") + 1] == "c" * 64
    assert command[command.index("--model-revision") + 1] == "a" * 40
    assert command[command.index("--repetitions") + 1] == "1"
    assert command[command.index("--warmup-passes") + 1] == "1"


def test_freeze_inputs_freezes_lockfile(tmp_path):
    src = tmp_path / "src"
    src.mkdir()
    (src / "lock.txt").write_bytes(b"semble==0.6.0\n")
    for name in ("suite.json", "pack.json", "manifest.json"):
        (src / name).write_text("{}", encoding="utf-8")
    (src / "host-profile.json").write_text(
        json.dumps(
            {
                "schema_version": 2,
                "profile_id": "test",
                "fingerprint": {
                    "system": "Darwin",
                    "release": "test",
                    "machine": "arm64",
                    "processor": "test",
                    "cpu_count": 8,
                    "rustc": "rustc test",
                    "power_digest": _fake_sha("power"),
                },
            }
        ),
        encoding="utf-8",
    )
    inputs = {
        "suite": str(src / "suite.json"),
        "query_pack": str(src / "pack.json"),
        "manifest": str(src / "manifest.json"),
        "semble_lockfile": str(src / "lock.txt"),
        "host_profile": str(src / "host-profile.json"),
    }
    stage = tmp_path / "stage"
    stage.mkdir()
    frozen = pairrun.freeze_inputs(inputs, stage)
    assert frozen["suite"] == str(stage / "evaluator-only" / "suite.json")
    assert frozen["semble_lockfile"] == str(stage / "semble-lockfile.txt")
    assert (stage / "semble-lockfile.txt").read_bytes() == b"semble==0.6.0\n"
    stage2 = tmp_path / "stage2"
    stage2.mkdir()
    with pytest.raises(pairrun.RunError, match="cannot freeze capture input semble_lockfile"):
        pairrun.freeze_inputs(dict(inputs, semble_lockfile=str(tmp_path / "absent.txt")), stage2)


@pytest.mark.parametrize("shared", [False, True])
def test_direct_quanta_driver_honors_requested_query_schedule(tmp_path, monkeypatch, shared):
    runner = tmp_path / "runner"
    runner.write_bytes(b"fixed runner identity")
    output = tmp_path / "output"
    pack = tmp_path / "pack.json"
    pack.write_text(json.dumps({"tasks": [{"task_id": "T1"}, {"task_id": "T2"}]}))
    spec = {
        "runner_binary": str(runner),
        "searchd_binary": "/unused/searchd",
        "searchd_expected_sha256": "b" * 64,
        "query_pack": str(pack),
        "suite": "/unused/suite",
        "strategies": [{"name": "whole_file"}],
        "query_warmup_passes": 1,
        "query_repetitions_per_root": 3,
        "seed": 7,
        "repetitions": 1,
    }
    original_protocol_bytes = None
    if shared:
        protocol_path = tmp_path / "shared-protocol.json"
        protocol_path.write_text(json.dumps(pairrun.build_query_protocol(["T1", "T2"], 8, 1, 3)))
        original_protocol_bytes = protocol_path.read_bytes()
        spec["_query_protocol"] = str(protocol_path)
        # Parent pair repetitions already own fresh roots; don't create nested ones.
        spec["repetitions"] = 3
    before = copy.deepcopy(spec)
    monkeypatch.setattr(pairrun, "preflight_capture", lambda _spec: output)
    monkeypatch.setattr(pairrun, "preflight_daemon_socket_paths", lambda *_args: None)
    monkeypatch.setattr(pairrun, "write_projected_pack", lambda *_args: pack)
    observed = []

    def fake_strategy(capture_spec, *_args):
        protocol = json.loads(Path(capture_spec["_query_protocol"]).read_bytes())
        assert protocol["task_ids"] == ["T1", "T2"]
        assert len(protocol["warmup_schedules"]) == 1
        assert len(protocol["measurement_schedules"]) == 3
        assert all(
            sorted(schedule) == ["T1", "T2"] for schedule in protocol["measurement_schedules"]
        )
        assert protocol["seed"] == (8 if shared else 7)
        observed.append(protocol)
        return {"strategy": "whole_file"}

    monkeypatch.setattr(pairrun, "run_quanta_strategy", fake_strategy)
    assert pairrun.run_quanta(spec, tmp_path) == 0
    assert len(observed) == 1
    assert spec == before
    if shared:
        assert protocol_path.read_bytes() == original_protocol_bytes
        assert not (output / "query-protocol.json").exists()
    else:
        assert (output / "query-protocol.json").is_file()


def test_direct_quanta_capture_does_not_silently_drop_fresh_root_repetitions(tmp_path):
    with pytest.raises(pairrun.RunError, match="one fresh root"):
        pairrun.run_quanta({"repetitions": 2}, tmp_path)


def test_exploratory_query_protocol_accepts_explicit_zero_warmups(tmp_path):
    spec = _g0_spec()
    spec["query_warmup_passes"] = 0
    schema = json.loads((Path(pairrun.__file__).parent / "pair-spec.schema.json").read_text())
    jsonschema.validate(spec, schema)
    path = tmp_path / "spec.json"
    path.write_text(json.dumps(spec))
    assert pairrun.load_spec(path)["query_warmup_passes"] == 0
    for invalid in (-1, True, 0.5):
        changed = {**spec, "query_warmup_passes": invalid}
        path.write_text(json.dumps(changed))
        with pytest.raises(pairrun.RunError, match="query_warmup_passes"):
            pairrun.load_spec(path)
        with pytest.raises(jsonschema.ValidationError):
            jsonschema.validate(changed, schema)


def test_quanta_capture_refuses_protocol_that_contradicts_requested_counts(tmp_path):
    protocol_path = tmp_path / "protocol.json"
    protocol_path.write_text(json.dumps(pairrun.build_query_protocol(["T1", "T2"], 8, 0, 1)))
    with pytest.raises(pairrun.RunError, match="differs from spec.query_warmup_passes"):
        pairrun._requested_quanta_query_protocol(
            {"_query_protocol": str(protocol_path), "query_warmup_passes": 1}, ["T1", "T2"]
        )
    with pytest.raises(pairrun.RunError, match="differs from spec.query_repetitions_per_root"):
        pairrun._requested_quanta_query_protocol(
            {"_query_protocol": str(protocol_path), "query_repetitions_per_root": 2}, ["T1", "T2"]
        )


def test_quanta_query_protocol_execution_refuses_missing_or_different_schedule():
    expected = pairrun.build_query_protocol(["T1", "T2"], 7, 1, 3)
    phase = {"query_protocol": expected, "warmup_passes": 1, "measurement_repetitions": 3}
    pairrun._validate_quanta_query_protocol_execution(expected, phase)
    for changed in (
        {},
        {**phase, "warmup_passes": 0},
        {**phase, "measurement_repetitions": 1},
        {**phase, "query_protocol": pairrun.build_query_protocol(["T1", "T2"], 8, 1, 3)},
    ):
        with pytest.raises(pairrun.RunError, match="differs from requested schedule"):
            pairrun._validate_quanta_query_protocol_execution(expected, changed)


def test_quanta_driver_defaults_to_potion_and_binary_digest(tmp_path, monkeypatch):
    def fake_run(command, **kwargs):
        assert command[command.index("--embedder") + 1] == "potion-code"
        assert command[command.index("--runner-revision") + 1] == "sha256:" + "a" * 64
        assert command[command.index("--searchd-bin") + 1] == "/unused/searchd"
        assert command[command.index("--searchd-expected-sha256") + 1] == "b" * 64
        assert command[command.index("--query-input-policy") + 1] == "native"
        assert command[command.index("--symbol-total-timeout-ms") + 1] == "600000"
        assert command[command.index("--diagnostics-out") + 1].endswith("retrieval-diagnostic.json")
        assert command[command.index("--refusal-out") + 1].endswith("query-plan-refusal.json")
        Path(command[command.index("--out") + 1]).write_text("{}", encoding="utf-8")
        Path(command[command.index("--metrics-out") + 1]).write_text(
            json.dumps({"file_count": 1, "chunk_count": 1}), encoding="utf-8"
        )
        state_root = Path(command[command.index("--state-root") + 1])
        state_root.mkdir(parents=True, exist_ok=True)
        (state_root / "mock-index").write_bytes(b"index")
        kwargs["resource_path"].write_text(
            json.dumps(_mock_unbound_resource_metrics()), encoding="utf-8"
        )
        return {"exit_code": 0, "timed_out": False, "elapsed_ms": 1.0}

    monkeypatch.setattr(pairrun, "run_monitored_process", fake_run)
    spec = {
        "runner_binary": "/unused/runner",
        "repo": "/unused/repo",
        "manifest": "/unused/manifest.json",
        "top_k": 10,
        "searchd_binary": "/unused/searchd",
        "searchd_expected_sha256": "b" * 64,
        "execution_profiles": {"quanta": qp.execution_profile("native")},
        "symbol_total_timeout_ms": 600_000,
    }
    with pytest.raises(pairrun.RunError, match="omitted retrieval diagnostics"):
        pairrun.run_quanta_strategy(
            spec, {"name": "whole_file"}, 0, tmp_path, ["lexical"], tmp_path / "pack.json", "a" * 64
        )
    for legacy in ("syntax", "fixed_window"):
        with pytest.raises(pairrun.RunError, match="unknown strategy"):
            pairrun.run_quanta_strategy(
                spec, {"name": legacy}, 0, tmp_path, ["lexical"], tmp_path / "pack.json", "a" * 64
            )


@pytest.mark.parametrize("budget", [0, -1, True, 1.5, 2**64])
def test_symbol_total_timeout_rejects_invalid_spec(tmp_path, budget):
    spec = {**_g0_spec(), "symbol_total_timeout_ms": budget}
    path = tmp_path / "spec.json"
    path.write_text(json.dumps(spec))
    with pytest.raises(pairrun.RunError, match="spec.symbol_total_timeout_ms"):
        pairrun.load_spec(path)
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(spec, _load_schema("pair-spec.schema.json"))


def test_symbol_query_cache_source_is_bound_to_producer_policy(tmp_path, monkeypatch):
    st = _pair_stage(tmp_path)
    phase_path = Path(st["rep_layouts"][0]["quanta_phase_metrics"]["whole_file"])
    artifact = phase_path.with_name("symbol-preflight.json")
    policy = json.loads(artifact.read_text())["preflight"]["policy"]
    baseline = pairrun.symbol_coverage.policy_digest(policy)
    read = pairrun.symbol_coverage.regular_bytes

    def changed_cache_source(path, **kwargs):
        raw = read(path, **kwargs)
        return raw + b"\n// changed cache\n" if path.name == "definition_query.rs" else raw

    monkeypatch.setattr(pairrun.symbol_coverage, "regular_bytes", changed_cache_source)
    assert pairrun.symbol_coverage.policy_digest(policy) != baseline


def test_symbol_total_timeout_binds_requested_preflight_and_replay(tmp_path):
    spec = {**_g0_spec(), "symbol_total_timeout_ms": 600_000}
    path = tmp_path / "spec.json"
    path.write_text(json.dumps(spec))
    assert pairrun.load_spec(path)["symbol_total_timeout_ms"] == 600_000
    jsonschema.validate(spec, _load_schema("pair-spec.schema.json"))
    st = _pair_stage(tmp_path / "pair")
    protocol = st["stage"] / "protocol-lock.json"
    frozen = json.loads(protocol.read_text())
    frozen["symbol_total_timeout_ms"] = 120_000
    protocol.write_text(json.dumps(frozen))
    assert _stage_verdict(st)["states"]["PAIR_VALID"] == "pass"
    # Rehashing a lock cannot make a 120s execution satisfy a requested 600s policy.
    frozen["symbol_total_timeout_ms"] = 600_000
    protocol.write_text(json.dumps(frozen))
    assert _stage_verdict(st)["states"]["PAIR_VALID"] == "fail"


@pytest.mark.parametrize("budget", [0, -1, True, 1.5, 2**64, 120_000])
def test_symbol_total_timeout_rejects_invalid_or_mismatched_actual_policy(budget):
    policy = {"timeout_total_ns": "600000000000"}
    pairrun.symbol_coverage.verify_requested_timeout(policy, 600_000)
    with pytest.raises(ValueError, match="requested symbol timeout|differs from requested"):
        pairrun.symbol_coverage.verify_requested_timeout(policy, budget)


def test_quanta_driver_freezes_typed_failure_without_record(tmp_path, monkeypatch):
    def fake_run(command, **kwargs):
        kwargs["stdout_path"].write_text("", encoding="utf-8")
        kwargs["stderr_path"].write_text("provider unavailable", encoding="utf-8")
        kwargs["resource_path"].write_text('{"exit_code":2}', encoding="utf-8")
        Path(command[command.index("--refusal-out") + 1]).write_text(
            '{"phase":"query_plan"}\n', encoding="utf-8"
        )
        return {"exit_code": 2, "timed_out": False, "elapsed_ms": 1.0}

    monkeypatch.setattr(pairrun, "run_monitored_process", fake_run)
    spec = {
        "runner_binary": "/unused/runner",
        "repo": "/unused/repo",
        "manifest": "/unused/manifest.json",
        "top_k": 10,
        "searchd_binary": "/unused/searchd",
        "searchd_expected_sha256": "b" * 64,
        "execution_profiles": {"quanta": qp.execution_profile("native")},
    }
    with pytest.raises(pairrun.RunError, match="Rust runner failed"):
        pairrun.run_quanta_strategy(
            spec,
            {"name": "whole_file"},
            0,
            tmp_path,
            ["lexical"],
            tmp_path / "pack.json",
            "a" * 64,
        )
    failure = json.loads(
        (tmp_path / "strategy-00-whole_file" / "failure.json").read_text(encoding="utf-8")
    )
    assert failure["failure_type"] == "nonzero_exit"
    assert failure["record_emitted"] is False
    assert failure["query_plan_refusal_sha256"] == pairrun.sha_file(
        tmp_path / "strategy-00-whole_file" / "query-plan-refusal.json"
    )


@pytest.mark.skipif(sys.platform == "linux", reason="legacy ps sampler is not Linux owner evidence")
def test_process_tree_resource_sampler_counts_children_and_kills_timeout(tmp_path, monkeypatch):
    # Reap the child before the root exits. An orphaned zombie can retain its
    # process group on macOS and make killpg(0) return EPERM; that is correctly
    # incomplete evidence, not the successful cleanup exercised here.
    child_code = "\n".join(
        [
            "import signal, subprocess, sys, time",
            "child = subprocess.Popen([sys.executable, '-c', "
            "'import time; x=bytearray(b\"x\"*8_000_000); time.sleep(60)'])",
            "def terminate(_signal, _frame):",
            "    child.wait(timeout=5)",
            "    sys.exit(143)",
            "signal.signal(signal.SIGTERM, terminate)",
            "print(child.pid, flush=True)",
            "time.sleep(60)",
        ]
    )
    # Start the timeout exercise after one actual positive-RSS child sample.
    # Cold interpreter scheduling is not the timeout oracle. Keep a real 30s
    # readiness bound so missing child evidence still fails instead of hanging.
    real_clock = pairrun.time
    readiness_started = real_clock.monotonic()
    deadline_started = None
    sample = pairrun._process_tree_sample

    def sample_ready_tree(root_pid):
        nonlocal deadline_started
        rows = sample(root_pid)
        if deadline_started is None and any(
            row["pid"] != root_pid and row["rss_bytes"] > 8_000_000 for row in rows
        ):
            deadline_started = real_clock.monotonic()
        return rows

    def ready_clock():
        now = real_clock.monotonic()
        if deadline_started is not None:
            return now - deadline_started
        return 0.0 if now - readiness_started < 30 else 31.0

    from types import SimpleNamespace

    monkeypatch.setattr(pairrun, "_process_tree_sample", sample_ready_tree)
    monkeypatch.setattr(
        pairrun, "time", SimpleNamespace(monotonic=ready_clock, sleep=real_clock.sleep)
    )
    metrics = pairrun.run_monitored_process(
        [sys.executable, "-c", child_code],
        stdout_path=tmp_path / "stdout.log",
        stderr_path=tmp_path / "stderr.log",
        resource_path=tmp_path / "resource.json",
        timeout_secs=1,
        sample_interval_ms=20,
    )
    assert metrics["timed_out"] is True
    assert metrics["exit_code"] != 0
    assert metrics["complete"] is True
    assert metrics["samples"] > 0
    assert deadline_started is not None, "actual child readiness was never observed"
    assert metrics["peak_rss_bytes"] > 8_000_000
    child_pid = int((tmp_path / "stdout.log").read_text().strip())
    owned = {row["pid"]: row for row in metrics["processes"]}
    assert metrics["root_pid"] in owned
    assert owned[child_pid]["peak_rss_bytes"] > 8_000_000
    assert metrics["cleanup_complete"] is True
    assert metrics["cleanup_error"] is None
    assert (
        subprocess.run(["ps", "-p", str(metrics["root_pid"])], capture_output=True).returncode != 0
    )


@pytest.mark.skipif(sys.platform == "linux", reason="legacy ps sampler is not Linux owner evidence")
def test_process_tree_sampler_excludes_zombie_processes(monkeypatch):
    snapshot = """\
100 50 1024 1.0 S quanta-runner
101 100 2048 2.0 S searchd
102 100 0 0.0 Z <defunct>
103 102 4096 3.0 S zombie-child
104 100 0 0.0 S startup-child
105 104 4096 3.0 S startup-descendant
"""
    monkeypatch.setattr(
        pairrun.subprocess,
        "check_output",
        lambda *_args, **_kwargs: snapshot,
    )

    sample = pairrun._process_tree_sample(100)

    # The zombie row (102) and its child (103) stay excluded, but the live
    # zero-RSS connector (104) must keep its positive-RSS descendant (105)
    # inside the owned process tree.
    assert [process["pid"] for process in sample] == [100, 101, 105]


def _patch_ps_snapshot(monkeypatch, snapshot: str) -> None:
    monkeypatch.setattr(
        pairrun.subprocess,
        "check_output",
        lambda *_args, **_kwargs: snapshot,
    )


def test_process_tree_sampler_keeps_positive_rss_descendant_of_live_zero_rss_parent(monkeypatch):
    # Independent invariant from the SEP-26 audit: a live positive-RSS
    # descendant remains owned through a live zero-RSS parent. The metric
    # rows cover exactly the positive-RSS owned processes.
    _patch_ps_snapshot(
        monkeypatch,
        "100 50 1024 1.0 S runner\n"
        "104 100 0 0.0 S startup-parent\n"
        "105 104 4096 3.0 S live-descendant\n",
    )

    sample = pairrun._process_tree_sample(100)

    assert [process["pid"] for process in sample] == [100, 105]
    assert sample[1]["ppid"] == 104
    assert sample[1]["rss_bytes"] == 4096 * 1024


def test_process_tree_sampler_zero_rss_ownership_is_row_order_independent(monkeypatch):
    snapshot = "100 50 1024 1.0 S runner\n104 100 0 0.0 S startup-parent\n105 104 4096 3.0 S live-descendant\n"
    _patch_ps_snapshot(monkeypatch, snapshot)
    ordered = pairrun._process_tree_sample(100)
    reversed_snapshot = "\n".join(reversed(snapshot.strip().splitlines())) + "\n"
    monkeypatch.setattr(
        pairrun.subprocess,
        "check_output",
        lambda *_args, **_kwargs: reversed_snapshot,
    )
    reversed_sample = pairrun._process_tree_sample(100)

    assert [process["pid"] for process in ordered] == [100, 105]
    assert [process["pid"] for process in reversed_sample] == [100, 105]


def test_process_tree_sampler_keeps_multi_level_zero_rss_connectors_and_excludes_unrelated(
    monkeypatch,
):
    _patch_ps_snapshot(
        monkeypatch,
        "100 50 1024 1.0 S runner\n"
        "110 100 0 0.0 S connector-a\n"
        "111 110 0 0.0 S connector-b\n"
        "112 111 8192 2.0 S worker\n"
        "900 1 65536 9.0 S unrelated-positive\n"
        "901 900 32768 8.0 S unrelated-child\n",
    )

    sample = pairrun._process_tree_sample(100)

    assert [process["pid"] for process in sample] == [100, 112]
    assert sample[-1]["rss_bytes"] == 8192 * 1024


def test_process_tree_sampler_zero_rss_root_reports_positive_children_only(monkeypatch):
    _patch_ps_snapshot(
        monkeypatch,
        "100 50 0 0.0 S runner\n101 100 2048 1.5 S searchd\n",
    )

    sample = pairrun._process_tree_sample(100)

    # The zero-RSS live root links its children but emits no metric row of
    # its own: the frozen artifact only accepts positive-RSS process rows.
    assert [process["pid"] for process in sample] == [101]
    assert sample[0]["rss_bytes"] == 2048 * 1024


def test_process_tree_sampler_rejects_malformed_and_duplicate_pid(monkeypatch):
    _patch_ps_snapshot(
        monkeypatch, "100 50 1024 1.0 S runner\n101 100 not-a-number 1.0 S bad-rss\n"
    )
    with pytest.raises(pairrun.RunError, match="malformed ps process row"):
        pairrun._process_tree_sample(100)
    _patch_ps_snapshot(
        monkeypatch,
        "100 50 1024 1.0 S runner\n103 100 4096 2.0 S worker\n103 100 512 0.5 S worker-retaken\n",
    )
    with pytest.raises(pairrun.RunError, match="duplicate ps process PID"):
        pairrun._process_tree_sample(100)


def test_process_tree_sampler_missing_root_refuses_complete_snapshot(monkeypatch):
    _patch_ps_snapshot(
        monkeypatch,
        "101 999 2048 1.0 S orphaned-worker\n102 1 4096 2.0 S unrelated\n",
    )

    # An unobserved root may have exited or been reused. The observed child
    # alone cannot prove a complete owner tree.
    with pytest.raises(pairrun.RunError, match="root process is absent"):
        pairrun._process_tree_sample(999)


@pytest.mark.skipif(sys.platform == "linux", reason="legacy ps sampler is not Linux owner evidence")
def test_process_tree_invalid_snapshot_marks_resource_artifact_incomplete(tmp_path, monkeypatch):
    _patch_ps_snapshot(monkeypatch, "not a valid ps row\n")
    metrics = pairrun.run_monitored_process(
        [sys.executable, "-c", "import time; time.sleep(0.05)"],
        stdout_path=tmp_path / "stdout.log",
        stderr_path=tmp_path / "stderr.log",
        resource_path=tmp_path / "resource.json",
        timeout_secs=2,
        sample_interval_ms=10,
    )
    assert metrics["complete"] is False
    assert "malformed ps process row" in metrics["error"]
    assert metrics["cleanup_complete"] is True
    assert json.loads((tmp_path / "resource.json").read_text()) == metrics


def _monitored_root_exit_snapshot(tmp_path, monkeypatch, *, group_survives):
    class FinishedRoot:
        pid = 43821
        returncode = 0
        polls = 0

        def poll(self):
            self.polls += 1
            return None if self.polls == 1 else 0

    root = FinishedRoot()
    calls = 0

    def sample(root_pid):
        nonlocal calls
        assert root_pid == root.pid
        calls += 1
        if calls == 1:
            return [
                {
                    "pid": root_pid,
                    "ppid": 1,
                    "rss_bytes": 1024,
                    "cpu_percent": 0.0,
                    "command": "python",
                }
            ]
        raise pairrun.ProcessRootAbsent("root process is absent from live ps snapshot")

    def group_probe(_pgid, sig):
        assert sig == 0
        if group_survives:
            return None
        raise ProcessLookupError

    monkeypatch.setattr(pairrun.subprocess, "Popen", lambda *_args, **_kwargs: root)
    monkeypatch.setattr(pairrun, "_process_tree_sample", sample)
    monkeypatch.setattr(pairrun, "_cleanup_process_group", lambda _pgid: (True, False, None))
    monkeypatch.setattr(pairrun.os, "killpg", group_probe)
    metrics = pairrun.run_monitored_process(
        [sys.executable, "-c", "pass"],
        stdout_path=tmp_path / "stdout.log",
        stderr_path=tmp_path / "stderr.log",
        resource_path=tmp_path / "resource.json",
        timeout_secs=2,
        sample_interval_ms=1,
    )
    assert calls == 2
    assert root.polls >= 3
    assert metrics["exit_code"] == 0
    assert metrics["samples"] == 1
    return metrics


@pytest.mark.skipif(sys.platform == "linux", reason="legacy ps sampler is not Linux owner evidence")
def test_process_tree_completed_root_does_not_invalidate_prior_samples(tmp_path, monkeypatch):
    metrics = _monitored_root_exit_snapshot(tmp_path, monkeypatch, group_survives=False)
    assert metrics["complete"] is True
    assert metrics["error"] is None


@pytest.mark.skipif(sys.platform == "linux", reason="legacy ps sampler is not Linux owner evidence")
def test_process_tree_exited_root_with_surviving_group_is_incomplete(tmp_path, monkeypatch):
    metrics = _monitored_root_exit_snapshot(tmp_path, monkeypatch, group_survives=True)
    assert metrics["complete"] is False
    assert "root process is absent" in metrics["error"]


def test_process_tree_sampler_propagates_ps_failure(monkeypatch):
    def _fail(*_args, **_kwargs):
        raise pairrun.subprocess.CalledProcessError(1, "ps")

    monkeypatch.setattr(pairrun.subprocess, "check_output", _fail)

    with pytest.raises(pairrun.subprocess.CalledProcessError):
        pairrun._process_tree_sample(100)


@pytest.mark.skipif(
    pairrun.platform.system() != "Darwin" or not pairrun.SANDBOX_EXEC.is_file(),
    reason="macOS Seatbelt backend is unavailable",
)
def test_isolation_boundary_denies_suite_and_allows_blind_pack(tmp_path):
    stage = tmp_path / "capture.staging"
    evaluator = stage / "evaluator-only"
    evaluator.mkdir(parents=True)
    frozen_suite = evaluator / "suite.json"
    frozen_suite.write_text('{"gold":"secret"}', encoding="utf-8")
    pack = stage / "query-pack.json"
    pack.write_text('{"query":"blind"}', encoding="utf-8")
    secret_root = tmp_path / "secret"
    secret_root.mkdir()
    original_suite = secret_root / "suite.json"
    original_suite.write_bytes(frozen_suite.read_bytes())
    source_repo = tmp_path / "repo"
    source_repo.mkdir()
    duplicate_gold = source_repo / "duplicate-gold.json"
    duplicate_gold.write_text('{"gold":"secret"}', encoding="utf-8")
    repo = stage / "runner-corpus"
    repo.mkdir()
    admitted = repo / "a.txt"
    admitted.write_text("admitted", encoding="utf-8")
    manifest = stage / "corpus-manifest.json"
    manifest.write_text(
        json.dumps(
            {
                "repository_commit": "a" * 40,
                "files": [{"path": "a.txt", "file_sha256": pairrun.sha_file(admitted)}],
            }
        ),
        encoding="utf-8",
    )
    materialized = pairrun._verify_materialized_corpus(repo, manifest)
    inputs = {}
    for name in ("runner_binary", "searchd_binary", "semble_python", "semble_lockfile"):
        path = tmp_path / name
        path.write_text(name, encoding="utf-8")
        inputs[name] = str(path)
    cache_root = tmp_path / "semble-cache"
    cache_root.mkdir()
    spec = {
        **inputs,
        "repo": str(repo),
        "manifest": str(manifest),
        "_source_repo": str(source_repo),
        "_materialized_corpus": materialized,
        "suite": str(frozen_suite),
        "query_pack": str(pack),
        "output_root": str(tmp_path / "final"),
        "blinding": "isolated",
        "suite_secret_root": str(secret_root),
        "semble_cache_root": str(cache_root),
    }
    prepared = pairrun.prepare_isolation(spec, stage, original_suite)
    assert prepared["isolation_method"] == pairrun.MACOS_ISOLATION_BACKEND
    proof_path = stage / "isolation-proof.json"
    assert prepared["access_block_log"] == f"sha256:{pairrun.sha_file(proof_path)}"
    proof = json.loads(proof_path.read_text(encoding="utf-8"))
    assert str(cache_root.resolve()) in proof["allowed_read_roots"]
    assert str(cache_root.resolve()) not in proof["allowed_write_roots"]
    denied_command, evidence = pairrun.sandbox_command(prepared, ["/bin/cat", str(original_suite)])
    denied = subprocess.run(denied_command, capture_output=True, text=True, timeout=15)
    assert denied.returncode != 0
    assert denied.stdout == ""
    denied_source, _ = pairrun.sandbox_command(prepared, ["/bin/cat", str(duplicate_gold)])
    source_attempt = subprocess.run(denied_source, capture_output=True, text=True, timeout=15)
    assert source_attempt.returncode != 0
    assert source_attempt.stdout == ""
    allowed_corpus, _ = pairrun.sandbox_command(prepared, ["/bin/cat", str(admitted)])
    admitted_attempt = subprocess.run(allowed_corpus, capture_output=True, text=True, timeout=15)
    assert admitted_attempt.returncode == 0
    assert admitted_attempt.stdout == "admitted"
    assert evidence["proof_sha256"] == pairrun.sha_file(proof_path)


def test_linux_isolation_reads_pinned_model_cache_without_write_grant(tmp_path):
    stage = tmp_path / "stage"
    stage.mkdir()
    (stage / "runner-tools.pyz").write_bytes(b"bundle")
    cache, _revision = _pinned_semble_cache(tmp_path)
    repo = tmp_path / "repo"
    repo.mkdir()
    secret = tmp_path / "secret"
    secret.mkdir()
    inputs = {}
    for name in ("manifest", "query_pack", "runner_binary", "searchd_binary", "semble_lockfile"):
        path = tmp_path / name
        path.write_bytes(name.encode())
        inputs[name] = str(path)
    interpreter = tmp_path / "venv" / "bin" / "python"
    interpreter.parent.mkdir(parents=True)
    interpreter.write_bytes(b"python")
    policy = pairrun._linux_policy(
        {
            **inputs,
            "repo": str(repo),
            "semble_python": str(interpreter),
            "semble_cache_root": str(cache),
            "repetitions": 1,
        },
        stage,
        [str(secret)],
    )
    assert str(cache.resolve()) in policy["readonly"]
    assert str(cache.resolve()) not in policy["writable"]
    assert str((stage / "rep-00").resolve()) in policy["writable"]


def test_runner_bundle_is_deterministic_closed_and_isolated(tmp_path):
    assert "finite_json.py" in pairrun.RUNNER_BUNDLE_MEMBERS
    first = tmp_path / "first.pyz"
    second = tmp_path / "second.pyz"
    first_proof = pairrun.build_runner_bundle(first)
    second_proof = pairrun.build_runner_bundle(second)
    assert first.read_bytes() == second.read_bytes()
    assert first_proof["sha256"] == second_proof["sha256"]
    pairrun.validate_runner_bundle(first, first_proof)
    for members in (None, [], [{}], [{"path": []}], [None], first_proof["manifest"]["members"] * 2):
        forged = json.loads(json.dumps(first_proof))
        forged["manifest"]["members"] = members
        with pytest.raises(pairrun.RunError, match="member manifest"):
            pairrun.validate_runner_bundle(first, forged)
    for mutate in (
        lambda value: value["manifest"].update(entrypoint="forged:main"),
        lambda value: value["manifest"].update(schema_version=True),
        lambda value: value["manifest"].update(schema_version=10**400),
        lambda value: value["manifest"].update(forged=True),
        lambda value: value["manifest"]["members"].reverse(),
        lambda value: value["manifest"]["members"][0].update(size=True),
        lambda value: value["manifest"]["members"][0].update(size=10**400),
        lambda value: value["manifest"]["members"][0].update(sha256="b" * 64),
    ):
        forged = json.loads(json.dumps(first_proof))
        mutate(forged)
        with pytest.raises(pairrun.RunError, match="manifest"):
            pairrun.validate_runner_bundle(first, forged)
    completed = subprocess.run(
        [sys.executable, "-I", "-S", str(first), "--help"],
        env={**os.environ, "PYTHONPATH": str(tmp_path / "poison")},
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert completed.returncode == 0, completed.stderr
    assert "Pinned Semble" in completed.stdout
    first.write_bytes(first.read_bytes() + b"tamper")
    with pytest.raises(pairrun.RunError, match="bundle digest"):
        pairrun.validate_runner_bundle(first, first_proof)


def test_model_cache_materialization_binds_ref_and_safe_links(tmp_path):
    source = tmp_path / "source"
    revision = "a" * 40
    model_id = "org/model"
    repo = source / "hub" / "models--org--model"
    blob = repo / "blobs" / "abc"
    blob.parent.mkdir(parents=True)
    blob.write_bytes(b"model-bytes")
    snapshot = repo / "snapshots" / revision
    snapshot.mkdir(parents=True)
    (snapshot / "model.bin").symlink_to(Path("../../blobs/abc"))
    ref = repo / "refs" / "main"
    ref.parent.mkdir(parents=True)
    ref.write_text(revision + "\n", encoding="utf-8")
    destination = tmp_path / "materialized"
    manifest = semble_adapter.materialize_model_cache(source, destination, model_id, revision)
    copied = destination / "hub" / "models--org--model" / "snapshots" / revision / "model.bin"
    assert copied.read_bytes() == b"model-bytes"
    assert not copied.is_symlink()
    assert manifest["ref"] == {"name": "main", "revision": revision}
    assert manifest["model_asset_digest"] == semble_adapter.model_asset_digest(
        destination, model_id, revision
    )

    directory_target = repo / "blobs" / "directory"
    directory_target.mkdir()
    (directory_target / "nested.bin").write_bytes(b"nested")
    (snapshot / "directory-link").symlink_to(Path("../../blobs/directory"))
    with pytest.raises(semble_adapter.AdapterError, match="directory symlink"):
        semble_adapter.materialize_model_cache(
            source, tmp_path / "directory-rejected", model_id, revision
        )
    (snapshot / "directory-link").unlink()

    escaped = tmp_path / "escaped"
    escaped.write_bytes(b"outside")
    (snapshot / "escape.bin").symlink_to(escaped)
    with pytest.raises(semble_adapter.AdapterError, match="escapes model root"):
        semble_adapter.materialize_model_cache(source, tmp_path / "rejected", model_id, revision)
    (snapshot / "escape.bin").unlink()
    ref.write_text("b" * 40 + "\n", encoding="utf-8")
    with pytest.raises(semble_adapter.AdapterError, match="ref drift"):
        semble_adapter.materialize_model_cache(source, tmp_path / "drifted", model_id, revision)


def test_semble_worker_uses_stage_owned_runtime_cache_and_readonly_model_source(
    tmp_path, monkeypatch
):
    repo, suite, _run, _sp, _rp, files = fixture_v3(tmp_path / "src")
    manifest_path = tmp_path / "manifest.json"
    manifest_path.write_text(
        json.dumps(
            {
                "repository_commit": suite["repository_commit"],
                "files": [
                    {"path": name, "file_sha256": ev.digest(data)}
                    for name, data in sorted(files.items())
                ],
            }
        ),
        encoding="utf-8",
    )
    _suite, pack, _source = ev.validate_suite(repo, suite)
    pack_path = tmp_path / "pack.json"
    pack_path.write_text(json.dumps(pack), encoding="utf-8")
    lock_path = tmp_path / "lock.txt"
    lock_path.write_text("semble==0.6.0\n", encoding="utf-8")
    cache, revision = _pinned_semble_cache(tmp_path)
    ref = cache / "hf/hub/models--minishlab--potion-code-16M-v2/refs/main"
    output = tmp_path / "adapter-output"
    original_mkdir = Path.mkdir

    def refuse_source_cache_write(path, *args, **kwargs):
        if path == cache:
            pytest.fail("adapter attempted to create the read-only source cache")
        return original_mkdir(path, *args, **kwargs)

    monkeypatch.setattr(Path, "mkdir", refuse_source_cache_write)
    monkeypatch.setattr(
        semble_adapter,
        "check_semble_env",
        lambda _python: {"observed_freeze": "semble==0.6.0\n", "semble_version": "0.6.0"},
    )

    def capture_worker(_command, **kwargs):
        env = kwargs["env"]
        assert env["SEMBLE_CACHE_LOCATION"] == str(output / "semble-runtime-cache")
        assert Path(env["SEMBLE_CACHE_LOCATION"]).is_dir()
        assert env["HF_HOME"] == str(output / "model-cache/hf")
        assert ref.read_text(encoding="utf-8").strip() == revision
        assert not (cache / "semble").exists()
        raise RuntimeError("worker environment captured")

    monkeypatch.setattr(semble_adapter, "run_completed_worker", capture_worker)
    args = semble_adapter.build_parser().parse_args(
        [
            "run",
            "--repo",
            str(repo),
            "--manifest",
            str(manifest_path),
            "--query-pack",
            str(pack_path),
            "--top-k",
            "10",
            "--python",
            sys.executable,
            "--lockfile",
            str(lock_path),
            "--lockfile-sha256",
            pairrun.sha_file(lock_path),
            "--cache-root",
            str(cache),
            "--output-root",
            str(output),
            "--model-revision",
            revision,
            "--run-id",
            "cache-bound",
            "--blinding",
            "attested",
            "--isolation-method",
            "attested",
            "--access-block-log",
            "attested",
        ]
    )
    with pytest.raises(RuntimeError, match="worker environment captured"):
        semble_adapter.run_adapter(args)


def test_spec_evidence_content_is_removed_receipts_are_frozen(tmp_path):
    spec_path = tmp_path / "spec.json"
    spec = _g0_spec()
    spec["evidence"] = {"pair": {"same_files": True}}
    spec_path.write_text(json.dumps(spec), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="spec.evidence was removed"):
        pairrun.load_spec(spec_path)
    results = tmp_path / "py-results.json"
    results.write_text('{"command": "x", "selected": 1}', encoding="utf-8")
    frozen = pairrun.freeze_receipts(
        {"receipts": {"contract_python_results": str(results)}}, tmp_path / "stage"
    )
    assert frozen == {
        "contract_python_results": str(
            tmp_path / "stage" / "receipts" / "contract_python_results.json"
        )
    }
    assert (
        tmp_path / "stage" / "receipts" / "contract_python_results.json"
    ).read_bytes() == results.read_bytes()
    with pytest.raises(pairrun.RunError, match="cannot freeze receipt"):
        pairrun.freeze_receipts(
            {"receipts": {"contract_python_results": str(tmp_path / "absent.json")}},
            tmp_path / "stage2",
        )


def test_normalize_record_proves_spans_and_order(tmp_path):
    repo, suite, _run, suite_path, _rp, files = fixture_v3(tmp_path, answerable_only=True)
    _, pack, _ = ev.validate_suite(repo, suite)
    pack_sha = ev.digest(ev.canonical(pack))
    contract = pack["comparison_contract"]
    file_shas = {name: ev.digest(data) for name, data in files.items()}
    file_lines = {name: data.splitlines(keepends=True) for name, data in files.items()}

    def build(native_rows, latencies, pack_arg=pack, contract_arg=contract):
        return semble_adapter.normalize_record(
            pack_arg,
            pack_sha,
            native_rows,
            latencies,
            repo,
            file_shas,
            file_lines,
            contract_arg,
            "run-1",
            "attested",
            "method",
            "log",
            "minishlab/potion-code-16M-v2",
            "rev",
            "semble-hybrid",
            semble_adapter.execution_profile("native-default", None),
            "cap-1",
            _fake_sha("diff"),
            _fake_sha("worker"),
        )

    native = [
        {
            "task_id": "T1",
            "results": [
                {"file_path": "a.txt", "start_line": 2, "end_line": 2, "score": 0.9},
                {"file_path": "b.txt", "start_line": 1, "end_line": 1, "score": 0.1},
            ],
        },
        {"task_id": "T2", "results": []},
    ]
    record = build(native, {"T1": [3.0], "T2": [1.0]})
    assert record["schema_version"] == 5
    assert [row["status"] for row in record["results"]] == ["success", "abstained"]
    assert [c["rank"] for c in record["results"][0]["candidates"]] == [1, 2]
    assert record["results"][0]["candidates"][0]["path"] == "a.txt"
    capture = record["captures"]["cap-1"]
    assert capture["system"] == "semble"
    assert capture["chunk_strategy"] == "semble_native"
    assert capture["searchd_binary"] is None and capture["generation"] == 0
    assert record["route_provenance"] == {"semble-hybrid": {"capture_id": "cap-1"}}
    assert capture["execution_profile"] == semble_adapter.execution_profile("native-default", None)
    assert capture["execution_profile_sha256"] == ev.digest(
        ev.canonical(capture["execution_profile"])
    )
    identity = record["results"][0]["query_identity"]
    first_query = pack["tasks"][0]["query"]
    assert identity["original_query_sha256"] == identity["submitted_query_sha256"]
    assert identity["original_query_sha256"] == hashlib.sha256(first_query.encode()).hexdigest()
    assert identity["original_query_sha256"] == pack["tasks"][0]["query_sha256"]

    duplicate_native = [
        {
            "task_id": "T1",
            "results": [
                native[0]["results"][0],
                dict(native[0]["results"][0], score=0.8),
                native[0]["results"][1],
            ],
        },
        native[1],
    ]
    collapsed = build(duplicate_native, {"T1": [3.0], "T2": [1.0]})
    first = collapsed["results"][0]["candidates"]
    assert [(row["path"], row["rank"]) for row in first] == [("a.txt", 1), ("b.txt", 2)]
    assert len(first) == 2

    malformed_duplicate = [
        {
            "task_id": "T1",
            "results": [
                native[0]["results"][0],
                dict(native[0]["results"][0], start_line=True),
            ],
        },
        native[1],
    ]
    refused = build(malformed_duplicate, {"T1": [3.0], "T2": [1.0]})
    assert refused["results"][0]["status"] == "error"
    assert refused["results"][0]["error"]["code"] == "semble_hit_bad_span"

    drifted = build(
        [
            {
                "task_id": "T1",
                "results": [
                    {"file_path": "elsewhere/a.txt", "start_line": 1, "end_line": 1, "score": 1.0}
                ],
            }
        ],
        {"T1": [1.0]},
    )
    assert drifted["results"][0]["status"] == "error"
    assert drifted["results"][0]["error"]["code"] == "semble_hit_outside_universe"

    truncated = build(
        [
            {
                "task_id": "T1",
                "results": [{"file_path": "a.txt", "start_line": 1, "end_line": 99, "score": 1.0}],
            }
        ],
        {"T1": [1.0]},
    )
    assert truncated["results"][0]["status"] == "error"
    assert truncated["results"][0]["error"]["code"] == "semble_hit_beyond_eof"

    bad_span = build(
        [
            {
                "task_id": "T1",
                "results": [{"file_path": "a.txt", "start_line": 2, "end_line": 1, "score": 1.0}],
            }
        ],
        {"T1": [1.0]},
    )
    assert bad_span["results"][0]["status"] == "error"
    assert bad_span["results"][0]["error"]["code"] == "semble_hit_bad_span"

    with pytest.raises(semble_adapter.AdapterError, match="duplicate native row"):
        build(native + [dict(native[0])], {"T1": [1.0], "T2": [1.0]})

    with pytest.raises(semble_adapter.AdapterError, match="unexpected native task"):
        build(native + [{"task_id": "EXTRA", "results": []}], {"T1": [1.0], "T2": [1.0]})

    with pytest.raises(semble_adapter.AdapterError, match="exactly task_id and results"):
        build([{"task_id": "T1"}, native[1]], {"T1": [1.0], "T2": [1.0]})

    with pytest.raises(semble_adapter.AdapterError, match="exactly task_id and results"):
        build([dict(native[0], injected=True), native[1]], {"T1": [1.0], "T2": [1.0]})

    with pytest.raises(semble_adapter.AdapterError, match="unexpected task or invalid mapping"):
        build(native, {"T1": [1.0], "T2": [1.0], "EXTRA": [1.0]})

    over = [
        {
            "task_id": "T1",
            "results": [
                {"file_path": "a.txt", "start_line": 1, "end_line": 1, "score": 1.0},
                {"file_path": "a.txt", "start_line": 2, "end_line": 2, "score": 0.5},
            ],
        }
    ]
    narrow = dict(contract, top_k=1)
    with pytest.raises(semble_adapter.AdapterError, match="exceeded top_k"):
        build(
            over,
            {"T1": [1.0]},
            pack_arg=dict(pack, comparison_contract=narrow),
            contract_arg=narrow,
        )

    with pytest.raises(semble_adapter.AdapterError, match="pack comparison contract differs"):
        build(native, {"T1": [3.0], "T2": [1.0]}, contract_arg=dict(contract, top_k=5))

    missing = build(
        [],
        {},
        pack_arg={
            "tasks": [{"task_id": "T9", "query": "q", "query_sha256": "s"}],
            "comparison_contract": contract,
        },
    )
    assert missing["results"][0]["status"] == "error"
    assert missing["results"][0]["error"]["code"] == "semble_missing_query"
    assert missing["results"][0]["timings"]["query_latency_ms"] is None

    # Missing samples are null, never 0.
    unmeasured = build(native, {})
    assert unmeasured["results"][0]["timings"]["query_latency_ms"] is None


def test_semble_file_collection_preserves_native_rank_and_exhaustion(tmp_path):
    repo, suite, _run, suite_path, runner_path, files = fixture_v3(tmp_path, answerable_only=True)
    suite["routes"] = ["semble-lexical-file"]
    suite["diagnostic_policy"] = ev.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
    for task in suite["tasks"]:
        task["label_review"] = {
            "assessment": "reviewed_unambiguous",
            "reviewer_id": "fixture-reviewer",
            "evidence_sha256": ev.digest(b"independent file fixture"),
        }
        task["judgment_policy"] = ev.UNJUDGED_POLICY
        task["file_judgments"] = [
            {"path": path, "file_sha256": ev.digest(files[path]), "grade": grade}
            for path, grade in (("a.txt", 3), ("b.txt", 1))
        ]
    _, pack, _ = ev.validate_suite(repo, suite)
    raw = [
        {
            "task_id": "T1",
            "results": [
                {"file_path": "a.txt", "start_line": 2, "end_line": 2, "score": 2.0},
                {"file_path": "a.txt", "start_line": 3, "end_line": 3, "score": 1.5},
                {"file_path": "b.txt", "start_line": 1, "end_line": 1, "score": 1.0},
            ],
        },
        {"task_id": "T2", "results": []},
    ]

    def normalized(rows):
        return semble_adapter.normalize_record(
            pack,
            ev.digest(ev.canonical(pack)),
            rows,
            {"T1": [1.0], "T2": [1.0]},
            repo,
            {path: ev.digest(data) for path, data in files.items()},
            {path: data.splitlines(keepends=True) for path, data in files.items()},
            pack["comparison_contract"],
            "file-run",
            "attested",
            "method",
            "log",
            "minishlab/potion-code-16M-v2",
            "rev",
            "semble-lexical-file",
            semble_adapter.execution_profile("lexical-file", None),
            "file-cap",
            _fake_sha("mapping"),
            _fake_sha("worker"),
            indexed_chunks=4,
        )

    run = normalized(raw)
    first, second = run["results"]
    assert [item["path"] for item in first["candidates"]] == ["a.txt", "b.txt"]
    assert [item["score"] for item in first["candidates"]] == [2.0, 1.0]
    assert first["file_collection"] == {
        "indexed_chunks": 4,
        "matched_chunks": 3,
        "matching_files": 2,
    }
    assert second["status"] == "abstained"
    assert second["file_collection"]["matched_chunks"] == 0
    jsonschema.validate(run, _load_schema("runner.schema.json"))
    loaded_suite, loaded_pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    route = ev.evaluate_diagnostic(loaded_suite, loaded_pack, loaded_run)["judgment_metrics"][
        "file_judgments"
    ]["routes"]["semble-lexical-file"]
    assert route["eligible_count"] == 2
    assert route["ordering"] == "score_desc_native_tiebreak"
    assert route["score_evidence"] == "semble_bm25_score_v1"

    for change, message in (
        (lambda row: row.pop("file_collection"), "file_collection"),
        (lambda row: row.update(rank_unit="symbol"), "scored distinct-file"),
        (lambda row: row["candidates"][1].update(score=3.0), "native score order"),
        (lambda row: row["file_collection"].update(matching_files=1), "does not explain"),
    ):
        forged = copy.deepcopy(run)
        change(forged["results"][0])
        with pytest.raises(ev.EvidenceError, match=message):
            record_v3(repo, suite, forged, suite_path, runner_path)
    inconsistent_depth = copy.deepcopy(run)
    inconsistent_depth["results"][1]["file_collection"]["indexed_chunks"] = 5
    with pytest.raises(ev.EvidenceError, match="depth changes within a capture"):
        record_v3(repo, suite, inconsistent_depth, suite_path, runner_path)
    reversed_raw = copy.deepcopy(raw)
    reversed_raw[0]["results"][2]["score"] = 3.0
    with pytest.raises(semble_adapter.AdapterError, match="native score order"):
        normalized(reversed_raw)
    tied_raw = copy.deepcopy(raw)
    tied_raw[0]["results"] = [
        {"file_path": "b.txt", "start_line": 1, "end_line": 1, "score": 2.0},
        {"file_path": "a.txt", "start_line": 2, "end_line": 2, "score": 2.0},
    ]
    tied = normalized(tied_raw)
    assert [item["path"] for item in tied["results"][0]["candidates"]] == ["b.txt", "a.txt"]
    record_v3(repo, suite, tied, suite_path, runner_path)

    profile = semble_adapter.execution_profile("lexical-file", None)
    assert pairrun._validate_semble_profile(profile, "fixture") == profile
    bound = {
        "execution_profiles": {"semble": profile},
        "semble_route": "semble-lexical-file",
        "baseline_route": "semble-lexical-file",
        "candidate_route": "lexical",
        "routes": ["lexical"],
    }
    pairrun._validate_semble_route_binding(bound)
    with pytest.raises(pairrun.RunError, match="semble_route must be"):
        pairrun._validate_semble_route_binding({**bound, "semble_route": "semble-lexical-only"})


def test_semble_file_collection_reuses_verified_source_work_across_queries(monkeypatch):
    lines = {"a.txt": [f"item {index}\n".encode() for index in range(15)]}
    lines.update({f"{letter}.txt": [b"value\n"] for letter in "bcdefghij"})
    shas = {path: ev.digest(b"".join(parts)) for path, parts in lines.items()}
    hits = [
        {
            "file_path": "a.txt",
            "start_line": index + 1,
            "end_line": index + 1,
            "score": float(100 - index),
        }
        for index in range(15)
    ]
    hits.extend(
        {"file_path": f"{letter}.txt", "start_line": 1, "end_line": 1, "score": float(80 - index)}
        for index, letter in enumerate("bcdefghij")
    )
    contract = _v3_contract(10)
    tasks = [{"task_id": task_id, "query": "item"} for task_id in ("T1", "T2")]
    pack = {"comparison_contract": contract, "tasks": tasks}
    native = [{"task_id": task["task_id"], "results": hits} for task in tasks]
    args = (
        shas,
        lines,
        contract,
        "semble-lexical-file",
        semble_adapter.execution_profile("lexical-file", None),
    )
    expected = semble_adapter.normalize_results(
        pack, native, {"T1": [1.0], "T2": [1.0]}, *args, indexed_chunks=24
    )
    assert [[candidate["path"] for candidate in row["candidates"]] for row in expected] == [
        ["a.txt", *(f"{letter}.txt" for letter in "bcdefghij")]
    ] * 2
    assert all(
        row["file_collection"] == {"indexed_chunks": 24, "matched_chunks": 24, "matching_files": 10}
        for row in expected
    )

    offsets = semble_adapter._source_line_offsets(lines)
    verified_blocks = {}
    original_count_tokens = semble_adapter.count_tokens
    token_checks = []

    def counted_tokens(text):
        token_checks.append(text)
        return original_count_tokens(text)

    monkeypatch.setattr(semble_adapter, "count_tokens", counted_tokens)
    monkeypatch.setattr(
        semble_adapter,
        "_source_line_offsets",
        lambda _lines: pytest.fail("per-query normalization rebuilt source offsets"),
    )
    actual = [
        semble_adapter.normalize_results(
            {"comparison_contract": contract, "tasks": [task]},
            [row],
            {task["task_id"]: [1.0]},
            *args,
            indexed_chunks=24,
            source_line_offsets=offsets,
            verified_blocks=verified_blocks,
        )[0]
        for task, row in zip(tasks, native, strict=True)
    ]
    assert actual == expected
    assert len(verified_blocks) == len(token_checks) == 24

    malformed = copy.deepcopy(hits)
    malformed[1]["end_line"] = 999
    rejected = semble_adapter.normalize_results(
        {"comparison_contract": contract, "tasks": [tasks[0]]},
        [{"task_id": "T1", "results": malformed}],
        {"T1": [1.0]},
        *args,
        indexed_chunks=24,
        source_line_offsets=offsets,
        verified_blocks=verified_blocks,
    )[0]
    assert rejected["status"] == "error"
    assert rejected["error"]["code"] == "semble_hit_beyond_eof"


def _single_route_record_v3(pack_sha, contract, route, system, capture_id, rows):
    return {
        "schema_version": 5,
        "query_pack_sha256": pack_sha,
        "comparison_contract": contract,
        "runner": {
            "name": f"{system}-runner",
            "revision": "r",
            "run_id": f"run-{capture_id}",
            "tokenizer": ev.TOKENIZER,
            "tokenizer_budget_version": ev.TOKENIZER_BUDGET_VERSION,
            "gold_access": False,
            "blinding": "attested",
            "isolation_method": "m",
            "access_block_log": "l",
        },
        "captures": {capture_id: _v3_capture(system, current=True)},
        "route_provenance": {route: {"capture_id": capture_id}},
        "results": rows,
    }


def _merge_fixture_v3(tmp_path):
    repo, suite, _run, suite_path, _rp, files = fixture_v3(tmp_path, answerable_only=True)
    suite["routes"] = ["lexical", "semble-hybrid"]
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    _, pack, _ = ev.validate_suite(repo, suite)

    def row(task_id, route, spans):
        query = next(task["query"] for task in pack["tasks"] if task["task_id"] == task_id)
        query_sha = ev.digest(query.encode())
        identity = (
            qp.derive_query_identity("native", query)
            if route == "lexical"
            else {
                "original_query_sha256": query_sha,
                "submitted_query_sha256": query_sha,
            }
        )
        return {
            "task_id": task_id,
            "route": route,
            "status": "success",
            "candidates": [
                _v3_block(files, *span, tokens=True, rank=i + 1) for i, span in enumerate(spans)
            ],
            "query_identity": identity,
            "timings": {"query_latency_ms": 2.0},
            "error": None,
        }

    lex_pack, _ = pairrun.project_pack_and_suite(pack, suite, ["lexical"])
    sem_pack, _ = pairrun.project_pack_and_suite(pack, suite, ["semble-hybrid"])
    lex = _single_route_record_v3(
        ev.digest(ev.canonical(lex_pack)),
        pack["comparison_contract"],
        "lexical",
        "quanta",
        "q-lex",
        [row("T1", "lexical", [("a.txt", 2, 2)]), row("T2", "lexical", [("a.txt", 3, 3)])],
    )
    sem = _single_route_record_v3(
        ev.digest(ev.canonical(sem_pack)),
        pack["comparison_contract"],
        "semble-hybrid",
        "semble",
        "s-sem",
        [
            row("T1", "semble-hybrid", [("b.txt", 1, 1)]),
            row("T2", "semble-hybrid", [("a.txt", 4, 4)]),
        ],
    )
    lex_path = tmp_path / "lex.json"
    sem_path = tmp_path / "sem.json"
    lex_path.write_text(json.dumps(lex), encoding="utf-8")
    sem_path.write_text(json.dumps(sem), encoding="utf-8")
    return repo, suite, pack, suite_path, lex_path, sem_path, files


def test_merge_combines_disjoint_records_and_scores(tmp_path):
    repo, suite, pack, suite_path, lex_path, sem_path, files = _merge_fixture_v3(tmp_path)
    merged_suite, merged_pack, combined = pairrun.merge_records(
        repo, suite_path, [lex_path, sem_path]
    )
    assert combined["schema_version"] == 5
    assert sorted(combined["route_provenance"]) == ["lexical", "semble-hybrid"]
    assert sorted(combined["captures"]) == ["q-lex", "s-sem"]
    assert combined["comparison_contract"] == pack["comparison_contract"]
    assert len(combined["results"]) == 4
    report = ev.evaluate(merged_suite, merged_pack, combined, "semble-hybrid", "lexical")
    assert report["rank_metrics"]["comparison"]["sample_count"] == 2

    # Deterministic under input order.
    _, _, swapped = pairrun.merge_records(repo, suite_path, [sem_path, lex_path])
    assert ev.digest(ev.canonical(swapped)) == ev.digest(ev.canonical(combined))

    # Duplicate route refused (distinct captures, same route).
    lex2 = json.loads(lex_path.read_text(encoding="utf-8"))
    lex2["captures"] = {"q-lex-2": _v3_capture("quanta", current=True)}
    lex2["route_provenance"] = {"lexical": {"capture_id": "q-lex-2"}}
    lex2_path = tmp_path / "lex2.json"
    lex2_path.write_text(json.dumps(lex2), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="recorded twice"):
        pairrun.merge_records(repo, suite_path, [lex_path, lex2_path])

    witnessed = json.loads(lex_path.read_text(encoding="utf-8"))
    witnessed["span_accounting_version"] = 1
    for result in witnessed["results"]:
        for candidate in result["candidates"]:
            candidate["span_accounting"] = {
                "unit_kind": "chunk",
                "unit_id": f"{result['task_id']}:{candidate['rank']}",
                "producer_identity": "whole_file",
                "indexed_start_byte": candidate["start_byte"],
                "indexed_end_byte": candidate["end_byte"],
                "sdk_start_line": candidate["start_line"],
                "sdk_end_line": candidate["end_line"],
                "extra_context_bytes": 0,
            }
    lex_path.write_text(json.dumps(witnessed), encoding="utf-8")
    _, _, witnessed_merge = pairrun.merge_records(repo, suite_path, [lex_path, sem_path])
    assert witnessed_merge["span_accounting_version"] == 1
    witnessed_report = ev.evaluate(suite, pack, witnessed_merge, "semble-hybrid", "lexical")
    assert witnessed_report["span_accounting"]["routes"]["lexical"]["status"] == "observed"
    assert (
        witnessed_report["span_accounting"]["routes"]["semble-hybrid"]["status"] == "not_applicable"
    )


def test_merge_reuses_one_source_validation_without_weakening_record_checks(tmp_path, monkeypatch):
    repo, _suite, _pack, suite_path, lex_path, sem_path, _files = _merge_fixture_v3(tmp_path)
    validate = pairrun.validate_suite
    calls = 0

    def counted(*args, **kwargs):
        nonlocal calls
        calls += 1
        return validate(*args, **kwargs)

    monkeypatch.setattr(pairrun, "validate_suite", counted)
    pairrun.merge_records(repo, suite_path, [lex_path, sem_path])
    assert calls == 1

    forged = json.loads(lex_path.read_text(encoding="utf-8"))
    forged["results"][0]["candidates"][0]["block_sha256"] = "0" * 64
    lex_path.write_text(json.dumps(forged), encoding="utf-8")
    with pytest.raises(ev.EvidenceError, match="block hash mismatch"):
        pairrun.merge_records(repo, suite_path, [lex_path, sem_path])
    assert calls == 2


def test_v3_rescore_is_deterministic_under_row_order(tmp_path):
    # T13: scores never change with row order. (The merged record id still
    # commits to the exact input bytes; only scores are compared.)
    repo, suite, pack, suite_path, lex_path, sem_path, _files = _merge_fixture_v3(tmp_path)
    merged_suite, merged_pack, combined = pairrun.merge_records(
        repo, suite_path, [lex_path, sem_path]
    )
    first = ev.evaluate(merged_suite, merged_pack, combined, "semble-hybrid", "lexical")
    for path in (lex_path, sem_path):
        payload = json.loads(path.read_text(encoding="utf-8"))
        payload["results"] = list(reversed(payload["results"]))
        path.write_text(json.dumps(payload), encoding="utf-8")
    _, _, swapped = pairrun.merge_records(repo, suite_path, [sem_path, lex_path])
    second = ev.evaluate(merged_suite, merged_pack, swapped, "semble-hybrid", "lexical")
    assert ev.canonical(first["budgets"]) == ev.canonical(second["budgets"])
    assert ev.canonical(first["rank_metrics"]) == ev.canonical(second["rank_metrics"])
    assert ev.canonical(first["per_query"]) == ev.canonical(second["per_query"])

    # Capture-id collision refused.
    sem2 = json.loads(sem_path.read_text(encoding="utf-8"))
    sem2["captures"] = {"q-lex": _v3_capture("semble", current=True)}
    sem2["route_provenance"] = {"semble-hybrid": {"capture_id": "q-lex"}}
    sem2_path = tmp_path / "sem2.json"
    sem2_path.write_text(json.dumps(sem2), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="capture_id .* recorded twice"):
        pairrun.merge_records(repo, suite_path, [lex_path, sem2_path])

    # Union must cover the suite.
    with pytest.raises(pairrun.RunError, match="!= suite routes"):
        pairrun.merge_records(repo, suite_path, [lex_path])

    # Tampered pack binding refused: routes doctored after signing.
    tampered = json.loads(lex_path.read_text(encoding="utf-8"))
    tampered["route_provenance"]["semble-hybrid"] = {"capture_id": "q-lex"}
    tampered_path = tmp_path / "tampered.json"
    tampered_path.write_text(json.dumps(tampered), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="projected pack"):
        pairrun.merge_records(repo, suite_path, [tampered_path, sem_path])

    # Old artifact stamps are refused before merge.
    old_run = json.loads(lex_path.read_text(encoding="utf-8"))
    old_run["schema_version"] = 2
    run2_path = tmp_path / "old-run.json"
    run2_path.write_text(json.dumps(old_run), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="v3/v4/v5 record required"):
        pairrun.merge_records(repo, suite_path, [run2_path, sem_path])

    # Record/pack contract drift refused at merge.
    drifted = json.loads(lex_path.read_text(encoding="utf-8"))
    drifted["comparison_contract"]["top_k"] = 5
    drifted_path = tmp_path / "drifted.json"
    drifted_path.write_text(json.dumps(drifted), encoding="utf-8")
    with pytest.raises(ev.EvidenceError, match="comparison contract differs"):
        pairrun.merge_records(repo, suite_path, [drifted_path, sem_path])


# --- Item 2: verdict state machine over frozen artifacts ---


def _counts_results(command, selected=10, executed=10, passed=10, failed=0):
    return {
        "command": command,
        "selected": selected,
        "executed": executed,
        "passed": passed,
        "failed": failed,
    }


def _receipt(
    command,
    results_bytes,
    revision,
    rail,
    raw_inputs,
    test_event_count=10,
    authority_sha256=None,
):
    authority_path = (
        Path(__file__).resolve().parents[3] / "benchmarks/retrieval/proof-required-tests.json"
    )
    closure_core = {
        "schema_version": 1,
        "profile": "retrieval",
        "revision": revision,
        "roots": ["Cargo.toml"],
        "files": [
            {"path": "Cargo.toml", "sha256": _fake_sha("source")},
            {
                "path": "benchmarks/retrieval/proof-required-tests.json",
                "sha256": authority_sha256 or pairrun.sha_file(authority_path),
            },
        ],
    }
    closure = {**closure_core, "digest": ev.digest(ev.canonical(closure_core))}
    return {
        "schema_version": 2,
        "revision": revision,
        "rail": rail,
        "tier": "correctness",
        "command": command,
        "evidence_path": "results.json",
        "evidence_sha256": ev.digest(results_bytes),
        "test_event_count": test_event_count,
        "source_closure": closure,
        "input_evidence": sorted(
            ({"role": role, "sha256": ev.digest(content)} for role, content in raw_inputs.items()),
            key=lambda entry: entry["role"],
        ),
    }


@pytest.mark.parametrize(
    "mutation",
    [
        lambda closure: closure.update(profile="other"),
        lambda closure: closure.update(revision="short"),
        lambda closure: closure.update(roots=[]),
        lambda closure: closure.update(roots=["Cargo.toml", "Cargo.toml"]),
        lambda closure: closure.update(roots=[1]),
        lambda closure: closure.update(files=[]),
        lambda closure: closure["files"][0].update(path=""),
        lambda closure: closure["files"][0].update(sha256="0"),
        lambda closure: closure["files"].append(dict(closure["files"][0])),
        lambda closure: closure.update(digest=_fake_sha("forged-closure")),
    ],
)
def test_driver_source_closure_shape_rejects_malformed_authority(mutation):
    closure = _receipt("cmd", b"{}", "a" * 40, "rail", {"raw": b"raw"})["source_closure"]
    mutation(closure)
    with pytest.raises(pairrun.RunError):
        pairrun._validate_source_closure_shape(closure, "driver source closure")


def _sdk_results(command, binary_digest, selected=6):
    return {
        "command": command,
        "separate_process": True,
        "sealed_receipt_digest": _fake_sha("sealed"),
        "activation_ack_digest": _fake_sha("ack"),
        "empty_check": True,
        "binary_digest": binary_digest,
        "sdk_route": "lexical",
        "selected": selected,
        "executed": selected,
        "passed": selected,
        "failed": 0,
    }


def _nextest_evidence(identities):
    by_binary = {}
    for identity in identities:
        binary_id, test_name = identity.split("$", 1)
        by_binary.setdefault(binary_id, []).append(test_name)
    suites = {}
    rows = []
    for binary_id, names in sorted(by_binary.items()):
        package, binary = binary_id.split("::", 1)
        kind = "lib" if binary == "quanta_index_retrieval_bench" else "test"
        metadata = {"crate": package, "test_binary": binary, "kind": kind}
        suites[binary_id] = {
            "package-name": package,
            "binary-name": binary,
            "kind": kind,
            "status": "listed",
            "testcases": {
                name: {"filter-match": {"status": "matches"}, "ignored": False} for name in names
            },
        }
        rows.append(
            {"type": "suite", "event": "started", "test_count": len(names), "nextest": metadata}
        )
        for name in names:
            event_name = f"{binary_id}${name}"
            rows.append({"type": "test", "event": "started", "name": event_name})
            rows.append({"type": "test", "event": "ok", "name": event_name})
        rows.append(
            {
                "type": "suite",
                "event": "ok",
                "passed": len(names),
                "failed": 0,
                "ignored": 0,
                "nextest": metadata,
            }
        )
    inventory = json.dumps({"test-count": len(identities), "rust-suites": suites}).encode()
    raw = b"".join(json.dumps(row).encode() + b"\n" for row in rows)
    return inventory, raw


def _sdk_raw_record(binary_digest):
    return json.dumps(
        {
            "schema_version": 5,
            "span_accounting_version": 1,
            "captures": {
                "capture": {
                    "runner_binary": {"digest": binary_digest},
                    "receipt_digest": _fake_sha("sealed"),
                    "activation_digest": _fake_sha("ack"),
                }
            },
            "route_provenance": {"lexical": {"capture_id": "capture"}},
        }
    ).encode()


def _parity_results(command, status="pass", failed=0):
    passed = 4 - failed
    return {
        "command": command,
        "status": status,
        "selected": 4,
        "executed": 4,
        "passed": passed,
        "failed": failed,
    }


def _bound_conditional_results(command, kind, *, matches=True):
    identity = {
        "source_revision": "a" * 40,
        "repository_commit": "b" * 40,
        "model_sha256": "c" * 64,
        "dependency_sha256": "d" * 64,
    }
    if kind == "model_vectors":
        raw = {
            "kind": kind,
            "rows": [
                {
                    "case_id": "case-1",
                    "reference_vector": [0.5, -0.25],
                    "observed_vector": [0.5, -0.25] if matches else [0.5, -0.2],
                }
            ],
        }
    else:
        raw = {
            "kind": kind,
            "rows": [
                {
                    "case_id": "case-1",
                    "fresh_row_ids": ["row-1"],
                    "incremental_row_ids": ["row-1"] if matches else ["row-2"],
                }
            ],
        }
    return {
        "schema_version": 1,
        "command": command,
        "status": "pass" if matches else "fail",
        "selected": 1,
        "executed": 1,
        "passed": int(matches),
        "failed": int(not matches),
        "identity": identity,
        "raw_proof": raw,
        "execution_receipt": {
            "schema_version": 1,
            "command": command,
            "exit_code": 0,
            **identity,
            "runner_binary_sha256": "e" * 64,
            "raw_sha256": ev.digest(ev.canonical(raw)),
        },
    }


class _NonSeekableLogSink:
    def __init__(self):
        self.buffer = io.BytesIO()

    def tell(self):
        return self.buffer.tell()

    def write(self, data):
        return self.buffer.write(data)

    def flush(self):
        self.buffer.flush()


def _canonical_log_zip(entries: dict[str, bytes]) -> bytes:
    """Independent stdlib fixture matching the frozen log archive contract."""
    sink = _NonSeekableLogSink()
    with zipfile.ZipFile(sink, "w", compression=zipfile.ZIP_STORED) as archive:
        for name, payload in sorted(entries.items()):
            entry = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            entry.create_system = 3
            entry.external_attr = (stat.S_IFREG | 0o600) << 16
            entry.file_size = len(payload)
            with archive.open(entry, "w") as target:
                target.write(payload)
    return sink.buffer.getvalue()


def _full_receipts(commit, binary_digest, binary_dir, *, sdk_build_profile=None):
    py_cmd = portable_proof.PYTHON_COMMAND
    rs_cmd = (
        "./scripts/cargow nextest run -p quanta-index-retrieval-bench "
        "--lib --test chunking_contract --test l5_parser_regressions --all-features --locked"
    )
    sdk_cmd = (
        "just retrieval-sdk-proof-fresh"
        if sdk_build_profile == "release-fresh"
        else "just retrieval-sdk-proof"
    )
    sdk_original_out = (binary_dir / "sdk-proof").resolve()
    sdk_binary_dir = sdk_original_out / "target" / "release" if sdk_build_profile else binary_dir
    sdk_binary_dir.mkdir(parents=True, exist_ok=True)
    # A synthetic qualified receipt models the committed source at `commit`,
    # not the concurrently dirty working tree used to execute this test.
    authority_bytes = subprocess.check_output(
        ["git", "show", f"{commit}:benchmarks/retrieval/proof-required-tests.json"],
        cwd=Path(__file__).resolve().parents[3],
    )
    authority = json.loads(authority_bytes)
    authority_sha256 = ev.digest(authority_bytes)
    py_count = len(authority["python"])
    rs_count = len(authority["rust"])
    sdk_count = len(authority["sdk"])
    py_results = _counts_results(py_cmd, py_count, py_count, py_count, 0)
    rs_results = _counts_results(rs_cmd, rs_count, rs_count, rs_count, 0)
    sdk_results = _sdk_results(sdk_cmd, binary_digest, sdk_count)
    py_bytes = json.dumps(py_results).encode()
    rs_bytes = json.dumps(rs_results).encode()
    sdk_bytes = json.dumps(sdk_results).encode()
    py_cases = "".join(
        f'<testcase classname="{html.escape(identity.rsplit(".", 1)[0], quote=True)}" '
        f'name="{html.escape(identity.rsplit(".", 1)[1], quote=True)}"/>'
        for identity in authority["python"]
    )
    py_raw = (
        f'<testsuite tests="{py_count}" failures="0" errors="0" skipped="0">{py_cases}</testsuite>\n'
    ).encode()
    rust_inventory, rust_raw = _nextest_evidence(authority["rust"])
    sdk_inventory, sdk_nextest = _nextest_evidence(authority["sdk"])
    binary_dir.mkdir(parents=True, exist_ok=True)
    inventories = []
    for rail, raw in (("contract", rust_inventory), ("sdk", sdk_inventory)):
        inventory = json.loads(raw)
        for binary_id, row in inventory["rust-suites"].items():
            role = "nextest-" + ev.digest(binary_id.encode())
            executable = (sdk_binary_dir if rail == "sdk" else binary_dir) / f"{rail}-{role}"
            executable.write_bytes(binary_id.encode())
            row.update(
                {
                    "binary-id": binary_id,
                    "binary-path": str(executable.resolve()),
                    "package-id": f"fixture:{portable_proof.PACKAGE}",
                    "build-platform": "target",
                }
            )
        inventories.append(json.dumps(inventory).encode())
    rust_inventory, sdk_inventory = inventories
    runner_path = sdk_binary_dir / (portable_proof.PACKAGE if sdk_build_profile else "runner")
    searchd_path = sdk_binary_dir / ("quanta-index-searchd" if sdk_build_profile else "searchd")
    runner_path.write_bytes(b"quanta-runner-binary")
    searchd_path.write_bytes(b"g0-seed-searchd")
    sdk_record = _sdk_raw_record(binary_digest)
    py_inventory = json.dumps(
        {
            "schema_version": 1,
            "kind": "pytest",
            "selector": portable_proof.proof_inventory.PYTHON_SELECTOR,
            "tests": authority["python"],
        }
    ).encode()
    artifacts = {
        "contract_python_results": py_bytes,
        "contract_python_raw": py_raw,
        "contract_python_inventory": py_inventory,
        "contract_python_receipt": _receipt(
            py_cmd,
            py_bytes,
            commit,
            "retrieval-contract-python",
            {"pytest-junit": py_raw, "pytest-inventory": py_inventory},
            py_count,
            authority_sha256=authority_sha256,
        ),
        "contract_rust_results": rs_bytes,
        "contract_rust_raw": rust_raw,
        "contract_rust_inventory": rust_inventory,
        "contract_rust_receipt": _receipt(
            rs_cmd,
            rs_bytes,
            commit,
            "retrieval-contract-rust",
            {"nextest-jsonl": rust_raw, "nextest-inventory": rust_inventory},
            rs_count,
            authority_sha256=authority_sha256,
        ),
        "sdk_results": sdk_bytes,
        "sdk_nextest_raw": sdk_nextest,
        "sdk_record_raw": sdk_record,
        "sdk_inventory": sdk_inventory,
        "sdk_receipt": _receipt(
            sdk_cmd,
            sdk_bytes,
            commit,
            "retrieval-sdk-proof",
            {
                "nextest-jsonl": sdk_nextest,
                "runner-record": sdk_record,
                "nextest-inventory": sdk_inventory,
            },
            sdk_count,
            authority_sha256=authority_sha256,
        ),
    }
    closure = artifacts["contract_python_receipt"]["source_closure"]
    closure_bytes = json.dumps(closure).encode()
    tools = {
        name: {
            "path": f"/fake/{name}",
            "realpath": f"/fake/{name}",
            "sha256": "a" * 64,
            "version": "fixture",
        }
        for name in ("python", "cargo", "cargo-nextest", "rustc", "git", "bash", "just", "cargow")
    }
    for rail, raw in (
        (
            "contract",
            {
                "source-closure.json": closure_bytes,
                "python-inventory.json": artifacts["contract_python_inventory"],
                "rust-inventory.json": artifacts["contract_rust_inventory"],
                "python-junit.xml": artifacts["contract_python_raw"],
                "rust-nextest.jsonl": artifacts["contract_rust_raw"],
            },
        ),
        (
            "sdk",
            {
                "source-closure.json": closure_bytes,
                "nextest-inventory.json": artifacts["sdk_inventory"],
                "nextest.jsonl": artifacts["sdk_nextest_raw"],
                "actual-runner-record.json": artifacts["sdk_record_raw"],
            },
        ),
    ):
        binaries = (
            {
                "runner": {"path": str(runner_path.resolve()), "sha256": binary_digest},
                "searchd": {
                    "path": str(searchd_path.resolve()),
                    "sha256": _fake_sha("searchd"),
                },
            }
            if rail == "sdk"
            else {}
        )
        collection_name = "nextest-inventory.json" if rail == "sdk" else "rust-inventory.json"
        binaries.update(
            {
                role: {"path": str(path), "sha256": pairrun.sha_file(path)}
                for role, path in portable_proof.selected_test_binaries(
                    raw[collection_name]
                ).items()
            }
        )
        inherited_environment = {"PATH": "/fixture/inherited/bin"}
        transcripts = (
            {
                "rust-collection.stdout": raw["nextest-inventory.json"],
                "rust-test.stdout": raw["nextest.jsonl"],
            }
            if rail == "sdk"
            else {
                "rust-collection.stdout": raw["rust-inventory.json"],
                "rust-test.stdout": raw["rust-nextest.jsonl"],
            }
        )
        inventory = json.loads(raw[collection_name])
        build_fields = {
            "binary-id",
            "binary-name",
            "package-id",
            "kind",
            "binary-path",
            "build-platform",
        }
        profile = sdk_build_profile if rail == "sdk" else None
        target_directory = str(sdk_original_out / "target" if profile else binary_dir.resolve())
        build_list = {
            "rust-build-meta": {"target-directory": target_directory},
            "rust-binaries": {
                binary_id: {key: suite[key] for key in build_fields}
                for binary_id, suite in inventory["rust-suites"].items()
            },
        }
        metadata = {
            "version": 1,
            "workspace_root": str(portable_proof.ROOT),
            "target_directory": target_directory,
            "workspace_members": [f"fixture:{portable_proof.PACKAGE}"],
            "resolve": None,
            "packages": [
                {
                    "id": f"fixture:{portable_proof.PACKAGE}",
                    "name": portable_proof.PACKAGE,
                    "version": "0.1.0",
                    "manifest_path": str(portable_proof.ROOT / "benchmarks/retrieval/Cargo.toml"),
                    "targets": [
                        {"name": suite["binary-name"], "kind": [suite["kind"]]}
                        for suite in inventory["rust-suites"].values()
                    ],
                }
            ],
        }
        transcripts.update(
            {
                "rust-build.stdout": json.dumps(build_list).encode(),
                "metadata.stdout": json.dumps(metadata).encode(),
            }
        )
        portable_proof.verify_reused_build(
            transcripts["rust-build.stdout"],
            transcripts["metadata.stdout"],
            transcripts["rust-collection.stdout"],
            workspace_root=portable_proof.ROOT,
            build_profile=profile,
        )
        commands = []
        for name, argv, overrides in portable_proof._expected_commands(
            rail,
            sdk_original_out if profile else Path("/proof"),
            tools,
            binaries,
            inherited_environment=inherited_environment,
            build_profile=profile,
        ):
            commands.append(
                {
                    "name": name,
                    "argv": argv,
                    "cwd": str(portable_proof.ROOT),
                    "environment": overrides,
                    "inherited_environment": dict(inherited_environment),
                    "environment_sha256": portable_proof._environment_digest(
                        {**inherited_environment, **overrides}
                    ),
                    "exit_code": 0,
                    "stdout": f"{name}.stdout",
                    "stdout_sha256": ev.digest(transcripts.get(f"{name}.stdout", b"")),
                    "stderr": f"{name}.stderr",
                    "stderr_sha256": ev.digest(b""),
                }
            )
        context = {
            "schema_version": (
                portable_proof.FRESH_EXECUTION_CONTEXT_VERSION
                if profile
                else portable_proof.EXECUTION_CONTEXT_VERSION
            ),
            **({"build_profile": profile} if profile else {}),
            "rail": rail,
            "revision": commit,
            "os": {
                "system": "fixture",
                "release": "fixture",
                "machine": "fixture",
                "python_version": "fixture",
            },
            "tools": tools,
            "binaries": binaries,
            "commands": commands,
            "raw_evidence": {name: ev.digest(value) for name, value in raw.items()},
        }
        context_bytes = json.dumps(context).encode()
        artifacts[f"{rail}_execution_context"] = context_bytes
        artifacts[f"{rail}_source_closure"] = closure_bytes
        artifacts[f"{rail}_execution_logs"] = _canonical_log_zip(
            {
                filename: transcripts.get(filename, b"")
                for filename in (
                    f"{name}.{stream}"
                    for name in pairrun.CONTEXT_COMMAND_NAMES[rail]
                    for stream in ("stdout", "stderr")
                )
            }
        )
        for side in ("python", "rust") if rail == "contract" else ("sdk",):
            receipt = artifacts[f"contract_{side}_receipt" if rail == "contract" else "sdk_receipt"]
            receipt["input_evidence"].append(
                {"role": "execution-context", "sha256": ev.digest(context_bytes)}
            )
            receipt["input_evidence"].sort(key=lambda row: row["role"])
    return artifacts


def _pair_stage(
    tmp_path,
    *,
    repetitions=1,
    qualified_speed_sample=False,
    blinding="attested",
    scope="exploratory",
    claims=None,
    receipts=None,
    host_clean=True,
    graded=True,
    embedder="potion-code",
    cache_regime="true_process_cold",
    alternate_system_order=True,
    diagnostic_version=4,
    hybrid_floor="100",
    query_observation="enabled",
    sdk_build_profile=None,
):
    """Build a complete valid pair stage through the real driver functions."""
    work = tmp_path / "work"
    repo, suite, run, _sp, _rp, files = fixture_v3(work / "src", answerable_only=True)
    if qualified_speed_sample:
        if repetitions != 5:
            raise ValueError("qualified speed fixture requires five fresh roots")
        original_tasks = list(suite["tasks"])
        original_results = list(run["results"])
        for index in range(3, 21):
            source_task = original_tasks[(index - 3) % len(original_tasks)]
            task = json.loads(json.dumps(source_task))
            task["task_id"] = f"T{index}"
            task["query"] = f"locate fixture {ev.digest(f'qualified-speed-task-{index}'.encode())}"
            task["query_sha256"] = ev.digest(task["query"].encode())
            task["query_family_id"] = f"fam-speed-{index}"
            suite["tasks"].append(task)
            for source_row in original_results:
                if source_row["task_id"] == source_task["task_id"]:
                    row = json.loads(json.dumps(source_row))
                    row["task_id"] = task["task_id"]
                    row["query_identity"] = qp.derive_query_identity("native", task["query"])
                    run["results"].append(row)
    if not graded:
        for task in suite["tasks"]:
            for label in task["gold"]:
                label.pop("grade", None)
    stage = work / "stage"
    stage.mkdir(parents=True)
    suite_path = stage / "evaluator-only" / "suite.json"
    suite_path.parent.mkdir()
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    _suite, pack, _source = ev.validate_suite(repo, suite)
    pack_path = stage / "query-pack.json"
    pack_path.write_text(json.dumps(pack), encoding="utf-8")
    corpus = {
        "repository_commit": suite["repository_commit"],
        "files": [
            {"path": "a.txt", "file_sha256": ev.digest(files["a.txt"])},
            {"path": "b.txt", "file_sha256": ev.digest(files["b.txt"])},
        ],
    }
    corpus_path = stage / "corpus-manifest.json"
    corpus_path.write_text(json.dumps(corpus), encoding="utf-8")
    runner_corpus = stage / "runner-corpus"
    runner_corpus.mkdir()
    for name in ("a.txt", "b.txt"):
        (runner_corpus / name).write_bytes(files[name])
    corpus_view = pairrun._verify_materialized_corpus(runner_corpus, corpus_path)
    isolation_method = "m"
    access_block_log = "l"
    resource_isolation = None
    if blinding == "isolated":
        if not pairrun.SANDBOX_EXEC.is_file():
            pytest.skip("macOS Seatbelt backend is unavailable")
        secret_root = work / "evaluator-secret"
        secret_root.mkdir()
        denied_roots = sorted(
            {str(secret_root.resolve()), str(suite_path.parent.resolve()), str(repo.resolve())}
        )
        allowed_read_roots = sorted(
            {str(pack_path.resolve()), str(runner_corpus.resolve()), str(stage.resolve())}
        )
        allowed_write_roots = [str(stage.resolve())]
        runner_bundle = stage / "runner-tools.pyz"
        bundle_proof = pairrun.build_runner_bundle(runner_bundle)
        profile = pairrun._seatbelt_profile(denied_roots, allowed_read_roots, allowed_write_roots)
        proof = {
            "schema_version": pairrun.ISOLATION_PROOF_VERSION,
            "backend": pairrun.MACOS_ISOLATION_BACKEND,
            "sandbox_exec": {
                "path": str(pairrun.SANDBOX_EXEC),
                "sha256": pairrun.sha_file(pairrun.SANDBOX_EXEC),
            },
            "policy_sha256": hashlib.sha256(profile.encode()).hexdigest(),
            "denied_roots": denied_roots,
            "allowed_read_roots": allowed_read_roots,
            "allowed_write_roots": allowed_write_roots,
            "suite": {
                "path": "evaluator-only/suite.json",
                "capture_path": str(suite_path.resolve()),
                "sha256": pairrun.sha_file(suite_path),
            },
            "query_pack": {
                "path": "query-pack.json",
                "capture_path": str(pack_path.resolve()),
                "sha256": pairrun.sha_file(pack_path),
            },
            "corpus_view": {
                "path": "runner-corpus",
                "manifest_sha256": corpus_view["manifest_sha256"],
                "proof_sha256": corpus_view["proof_sha256"],
                "file_count": len(corpus_view["files"]),
            },
            "runner_bundle": {**bundle_proof, "path": "runner-tools.pyz"},
            "platform_helpers": [],
            "probes": {
                "suite_read_denied": True,
                "query_pack_read_allowed": True,
            },
        }
        proof_path = stage / "isolation-proof.json"
        proof_path.write_text(json.dumps(proof), encoding="utf-8")
        proof_sha = pairrun.sha_file(proof_path)
        isolation_method = pairrun.MACOS_ISOLATION_BACKEND
        access_block_log = f"sha256:{proof_sha}"
        resource_isolation = {
            "backend": pairrun.MACOS_ISOLATION_BACKEND,
            "policy_sha256": proof["policy_sha256"],
            "proof_sha256": proof_sha,
        }
    corpus_dir = work / "corpus"
    corpus_dir.mkdir(parents=True)
    for name in ("a.txt", "b.txt"):
        (corpus_dir / name).write_bytes(files[name])
    admitted = [(e["path"], e["file_sha256"]) for e in corpus["files"]]
    mapping, _diff = semble_adapter.mapping_proof(admitted, ["a.txt", "b.txt"], corpus_dir)
    binary_digest = ev.digest(b"quanta-runner-binary")
    runner_binary = work / "runner-bin"
    runner_binary.write_bytes(b"quanta-runner-binary")

    lex_pack, _ = pairrun.project_pack_and_suite(pack, suite, ["lexical"])
    sem_pack, _ = pairrun.project_pack_and_suite(pack, suite, ["hybrid"])
    lex_sha = ev.digest(ev.canonical(lex_pack))
    sem_sha = ev.digest(ev.canonical(sem_pack))
    lex_rows = [r for r in run["results"] if r["route"] == "lexical"]
    sem_rows = [r for r in run["results"] if r["route"] == "hybrid"]

    def record(route_rows, pack_sha, system, capture_id, run_id):
        capture = _v3_capture(system, current=True)
        if system == "quanta":
            capture["runner_binary"]["digest"] = binary_digest
            # Match the public producer: lexical execution exercises no model.
            capture.update(model="none:lexical", model_revision="not-applicable")
        else:
            capture["receipt_digest"] = mapping["diff_digest"]
        rows = json.loads(json.dumps(route_rows))
        if system == "semble":
            task_queries = {task["task_id"]: task["query"] for task in pack["tasks"]}
            for row in rows:
                raw_sha = ev.digest(task_queries[row["task_id"]].encode())
                row["query_identity"] = {
                    "original_query_sha256": raw_sha,
                    "submitted_query_sha256": raw_sha,
                }
        return {
            "schema_version": 5,
            "query_pack_sha256": pack_sha,
            "comparison_contract": pack["comparison_contract"],
            "runner": {
                "name": (
                    "semble-adapter/native-default" if system == "semble" else f"{system}-runner"
                ),
                "revision": "r",
                "run_id": run_id,
                "tokenizer": ev.TOKENIZER,
                "tokenizer_budget_version": ev.TOKENIZER_BUDGET_VERSION,
                "gold_access": False,
                "blinding": blinding,
                "isolation_method": isolation_method,
                "access_block_log": access_block_log,
            },
            "captures": {capture_id: capture},
            "route_provenance": {route_rows[0]["route"]: {"capture_id": capture_id}},
            "results": rows,
        }

    rep_layouts = []
    measurements = 10 if qualified_speed_sample else 1
    warm_query_ms = len(pack["tasks"]) * measurements * 1.5 if qualified_speed_sample else 3.0
    for rep in range(repetitions):
        rep_dir = stage / f"rep-{rep:02d}"
        qdir = rep_dir / "quanta" / "strategy-00-whole_file"
        sdir = rep_dir / "semble"
        qdir.mkdir(parents=True)
        sdir.mkdir(parents=True)
        qrec = record(lex_rows, lex_sha, "quanta", f"q-r{rep}", f"run-q-r{rep}")
        ingest = _diagnostic_ingest_fixture(qrec) if diagnostic_version in (5, 6, 7, 8) else None
        if diagnostic_version in (7, 8):
            ingest["observation"].update(
                lexical_build_ns=100,
                lexical_stages={
                    "preparation_ns": 10,
                    "writer_mutation_ns": 20,
                    "text_authority_ns": 10,
                    "file_authority_ns": 10,
                    "seal_ns": 40,
                    "seal_writer_commit_ns": 10,
                    "seal_merge_wait_ns": 5,
                    "seal_commitment_ns": 20,
                    "seal_file_admission_ns": 15,
                },
            )
            if diagnostic_version == 8:
                ingest["observation"]["lexical_stages"].update(
                    text_authority_collect_ns=2,
                    text_authority_shard_build_ns=3,
                    text_authority_publish_ns=4,
                )
        srec = record(sem_rows, sem_sha, "semble", f"s-r{rep}", f"run-s-r{rep}")
        qpath = qdir / "record.json"
        spath = sdir / "record.json"
        qpath.write_text(json.dumps(qrec), encoding="utf-8")
        spath.write_text(json.dumps(srec), encoding="utf-8")
        task_ids = [task["task_id"] for task in pack["tasks"]]
        protocol = pairrun.build_query_protocol(task_ids, rep, 1, measurements)
        protocol_path = rep_dir / "query-protocol.json"
        protocol_path.write_text(json.dumps(protocol), encoding="utf-8")
        tasks_by_id = {task["task_id"]: task for task in pack["tasks"]}
        function_identity = {
            name: {
                "module": "semble.search",
                "qualname": name,
                "source_sha256": _fake_sha(f"semble-{name}"),
            }
            for name in ("bm25", "index_search", "module_search", "resolve_alpha", "semantic")
        }
        execution_events = []
        event_schedule = [(0, "cold", 0, protocol["cold_probe_task_id"])]
        for iteration, schedule in enumerate(protocol["warmup_schedules"]):
            event_schedule.extend((0, "warmup", iteration, task_id) for task_id in schedule)
        for repetition, schedule in enumerate(protocol["measurement_schedules"]):
            event_schedule.extend(
                (repetition, "measured", repetition, task_id) for task_id in schedule
            )
        for ordinal, (event_rep, phase, phase_iteration, task_id) in enumerate(event_schedule):
            execution_events.append(
                {
                    "rep": event_rep,
                    "phase": phase,
                    "phase_iteration": phase_iteration,
                    "task_id": task_id,
                    "call_ordinal": ordinal,
                    "submitted_query_sha256": tasks_by_id[task_id]["query_sha256"],
                    "profile_sha256": ev.digest(
                        ev.canonical(semble_adapter.execution_profile("native-default", None))
                    ),
                    "actual_alpha": 0.5,
                    "actual_rerank": True,
                    "candidate_depth": 50,
                    "lane_entry_counts": {"bm25": 1, "semantic": 1},
                    "lane_candidate_depths": {"bm25": [50], "semantic": [50]},
                }
            )
        execution_events_sha256 = ev.digest(ev.canonical(execution_events))
        lane_call_counts = {
            "bm25": len(execution_events),
            "semantic": len(execution_events),
            "encode": len(execution_events),
        }
        q_warm = {
            "lexical": {
                row["task_id"]: [row["timings"]["query_latency_ms"]] * measurements
                for row in qrec["results"]
            }
        }
        s_warm = {
            "hybrid": {
                row["task_id"]: [row["timings"]["query_latency_ms"]] * measurements
                for row in srec["results"]
            }
        }
        qphase = qdir / "phase-metrics.json"
        qphase.write_text(
            json.dumps(
                {
                    "schema_version": 3 if diagnostic_version in (7, 8) else 2,
                    "system": "quanta",
                    "timing_layer": "runner_monotonic_wall_v1",
                    "strategy": "whole_file",
                    "record_sha256": ev.digest(qpath.read_bytes()),
                    "runner_binary_sha256": binary_digest,
                    "task_count": len(pack["tasks"]),
                    "route_count": 1,
                    "file_count": 2,
                    "chunk_count": 2,
                    "symbol_count": 0,
                    "symbol_producer_identity": pairrun.QUANTA_SYMBOL_PRODUCER_IDENTITY,
                    "symbol_grammars": pairrun.QUANTA_SYMBOL_GRAMMARS,
                    "symbol_coverage": [
                        {
                            "path": "a.txt",
                            "source_sha256": ev.digest(files["a.txt"]),
                            "language": None,
                            "definition_count": 0,
                        },
                        {
                            "path": "b.txt",
                            "source_sha256": ev.digest(files["b.txt"]),
                            "language": None,
                            "definition_count": 0,
                        },
                    ],
                    "symbol_unsupported_files": 0,
                    "symbol_unsupported_details": [],
                    "symbol_only_scopes": 0,
                    "query_schedule": [task["task_id"] for task in pack["tasks"]],
                    "warmup_passes": 1,
                    "measurement_repetitions": measurements,
                    "query_protocol": protocol,
                    "warm_latencies_ms": q_warm,
                    "cold_latencies_ms": {"lexical": 1.0},
                    "phases_ms": {
                        "discovery": 1.0,
                        "chunk": 1.0,
                        "daemon_boot_and_readiness"
                        if diagnostic_version in (7, 8)
                        else "model_provider_prepare": 1.0,
                        "embed_publish_seal_activate": 1.0,
                        **(
                            {"sdk_publish": 0.6, "sdk_activate": 0.3}
                            if diagnostic_version in (7, 8)
                            else {}
                        ),
                        "cold_query": 1.0,
                        "warmup": 1.0,
                        "warm_query": warm_query_ms,
                        "unattributed": 1.0,
                    },
                    "total_ms": 7.0 + warm_query_ms,
                }
            ),
            encoding="utf-8",
        )
        preflight = _bind_preflight_fixture(qphase, pack["repository_commit"])
        sphase = sdir / "phase-metrics.json"
        sphase.write_text(
            json.dumps(
                {
                    "schema_version": 2,
                    "system": "semble",
                    "profile": "native-default",
                    "requested_alpha": None,
                    "rerank_applied": True,
                    "lane_call_counts": lane_call_counts,
                    "execution_events_sha256": execution_events_sha256,
                    "function_identity": function_identity,
                    "observed_wrapped_call_ns": 17,
                    "timing_layer": "worker_monotonic_wall_v1",
                    "strategy": "native",
                    "record_sha256": ev.digest(spath.read_bytes()),
                    "worker_sha256": _fake_sha("worker"),
                    "task_count": len(pack["tasks"]),
                    "route_count": 1,
                    "file_count": 2,
                    "chunk_count": 2,
                    "query_schedule": [task["task_id"] for task in pack["tasks"]],
                    "warmup_passes": 1,
                    "measurement_repetitions": measurements,
                    "query_protocol": protocol,
                    "warm_latencies_ms": s_warm,
                    "cold_latencies_ms": {"hybrid": 1.0},
                    "phases_ms": {
                        "discovery": 1.0,
                        "model_provider_prepare": 1.0,
                        "index": 1.0,
                        "warmup": 1.0,
                        "cold_query": 1.0,
                        "warm_query": warm_query_ms,
                        "unattributed": 1.0,
                    },
                    "phase_boundaries_ns": {
                        "worker_start": 0,
                        "discovery_end": 1_000_000,
                        "model_provider_prepare_end": 2_000_000,
                        "index_end": 3_000_000,
                        "cold_query_start": 3_000_000,
                        "cold_query_end": 4_000_000,
                        "warmup_end": 5_000_000,
                        "query_start": 5_000_000,
                        "first_query_start": 5_000_000,
                        "first_query_end": 6_500_000,
                        "query_end": 5_000_000 + int(warm_query_ms * 1_000_000),
                        "worker_end": 6_000_000 + int(warm_query_ms * 1_000_000),
                    },
                    "total_ms": 6.0 + warm_query_ms,
                }
            ),
            encoding="utf-8",
        )
        resource_payload = {
            "schema_version": 1,
            "sampler": "ps-process-tree-rss-cpu-v2",
            "sample_interval_ms": 50,
            "command_sha256": _fake_sha("command"),
            "root_pid": 100 + rep,
            "exit_code": 0,
            "timed_out": False,
            "elapsed_ms": 7.0 + warm_query_ms,
            "peak_rss_bytes": 4096,
            "peak_cpu_percent": 10.0,
            "processes": [
                {
                    "pid": 100 + rep,
                    "command": "runner",
                    "peak_rss_bytes": 4096,
                    "peak_cpu_percent": 10.0,
                    "samples": 2,
                }
            ],
            "storage": {
                "index_bytes": 4096,
                "model_cache_bytes": 1024,
                "parser_cache_bytes": 0,
                "embedding_cache_bytes": 0,
                "discovered_files": 2,
                "indexed_chunks": 2,
                "index_storage": "disk",
                "index_measurement": "filesystem_tree_v1",
            },
            "samples": 2,
            "complete": True,
            "error": None,
            "cleanup_complete": True,
            "cleanup_escalated": False,
            "cleanup_error": None,
        }
        if resource_isolation is not None:
            resource_payload["isolation"] = resource_isolation
        qresource = qdir / "resource-metrics.json"
        qresource.write_text(
            json.dumps({**resource_payload, "subject_sha256": ev.digest(qpath.read_bytes())}),
            encoding="utf-8",
        )
        sresource = rep_dir / "semble-resource-metrics.json"
        sresource.write_text(
            json.dumps(
                {
                    **resource_payload,
                    "subject_sha256": ev.digest(spath.read_bytes()),
                    "storage": {
                        **resource_payload["storage"],
                        "index_bytes": 4096,
                        "index_storage": "memory",
                        "index_measurement": "process_peak_rss_delta_v1",
                    },
                }
            ),
            encoding="utf-8",
        )
        latencies = {
            row["task_id"]: [
                row["timings"]["query_latency_ms"],
                row["timings"]["query_latency_ms"] + 0.1,
            ]
            for row in srec["results"]
        }
        (sdir / "native.json").write_text(
            json.dumps(
                {
                    "semble_profile": "native-default",
                    "requested_alpha": None,
                    "actual_alpha_by_task": {
                        task_id: 0.5 for task_id in protocol["measurement_schedules"][0]
                    },
                    "execution_events": execution_events,
                    "execution_events_sha256": execution_events_sha256,
                    "function_identity": function_identity,
                    "observed_wrapped_call_ns": 17,
                    "rerank_applied": True,
                    "lane_call_counts": lane_call_counts,
                    "native": [
                        {"task_id": task_id, "results": []}
                        for task_id in protocol["measurement_schedules"][0]
                    ],
                    "latencies_ms": latencies,
                    "stats": {
                        "indexed_files": 2,
                        "total_chunks": 2,
                        "index_resident_bytes": 4096,
                        "index_measurement": "process_peak_rss_delta_v1",
                    },
                    "query_schedule": task_ids,
                    "query_protocol": protocol,
                    "repetitions": measurements,
                    "warmup_passes": 1,
                }
            ),
            encoding="utf-8",
        )
        model_cache_manifest = {
            "schema_version": 1,
            "model_id": "minishlab/potion-code-16M-v2",
            "revision": "b" * 40,
            "ref": {"name": "main", "revision": "b" * 40},
            "members": [{"path": "model.safetensors", "sha256": _fake_sha("model"), "size": 1}],
            "model_asset_digest": _fake_sha("model"),
        }
        model_cache_manifest["snapshot_digest"] = ev.digest(ev.canonical(model_cache_manifest))
        model_cache_path = sdir / "model-cache-manifest.json"
        model_cache_path.write_text(json.dumps(model_cache_manifest), encoding="utf-8")
        (sdir / "mapping-proof.json").write_text(json.dumps(mapping), encoding="utf-8")
        lockfile = sdir / "lockfile.txt"
        lockfile.write_bytes(b"semble==0.6.0\n")
        (sdir / "adapter-manifest.json").write_text(
            json.dumps(
                {
                    "semble_version": "0.6.0",
                    "semble_python": "/venv/bin/python",
                    "interpreter": {
                        "path": "/venv/bin/python",
                        "realpath": "/venv/bin/python3.11",
                        "version": "3.11",
                        "digest": _fake_sha("interp"),
                    },
                    "worker_digest": _fake_sha("worker"),
                    "lockfile_digest": ev.digest(b"semble==0.6.0\n"),
                    "model_id": "minishlab/potion-code-16M-v2",
                    "model_revision": "b" * 40,
                    "model_asset_digest": _fake_sha("model"),
                    "model_cache_manifest_digest": pairrun.sha_file(model_cache_path),
                    "profile": semble_adapter.execution_profile("native-default", None),
                    "requested_alpha": None,
                    "actual_alpha_by_task": {
                        task_id: 0.5 for task_id in protocol["measurement_schedules"][0]
                    },
                    "rerank_applied": True,
                    "lane_call_counts": lane_call_counts,
                    "execution_events_sha256": execution_events_sha256,
                    "function_identity": function_identity,
                    "observed_wrapped_call_ns": 17,
                    "record_digest": ev.digest(spath.read_bytes()),
                }
            ),
            encoding="utf-8",
        )
        diagnostic_rows = []
        for request_index, result in enumerate(qrec["results"], 1):
            returned = len(result["candidates"])
            diagnostic_rows.append(
                {
                    "task_id": result["task_id"],
                    "query_sha256": tasks_by_id[result["task_id"]]["query_sha256"],
                    "route": result["route"],
                    "status": result["status"],
                    "error_code": result["error"]["code"] if result["error"] else None,
                    "candidates": [
                        {
                            "rank": candidate["rank"],
                            "candidate_id": f"chunk-{result['task_id']}-{index}",
                            "path": candidate["path"],
                            "start_line": candidate["start_line"],
                            "end_line": candidate["end_line"],
                            "score": 1.0,
                            "contributions": [],
                        }
                        for index, candidate in enumerate(result["candidates"])
                    ],
                    "response_kind": "returned_window",
                    "response": {
                        "window": {
                            "returned": returned,
                            "candidate_count": {"kind": "exact", "value": returned},
                            "outcome": {"kind": "exact_exhausted"},
                            "coverage": {
                                "examined": {"kind": "exact", "value": returned},
                                "exhaustion_proof": {"kind": "exact_count", "total": returned},
                                "lanes": [
                                    {
                                        "lane": result["route"],
                                        "executed": True,
                                        "contributed": returned > 0,
                                        "filtered_out": 0,
                                        "candidates": {"kind": "exact", "value": returned},
                                    }
                                ],
                            },
                        },
                        "explanation": {
                            "request_id": request_index,
                            "early_stop_reason": None,
                            "engines_executed": ["lexical"],
                            "engines_touched": ["lexical"] if returned else [],
                            "strategy": "lexical",
                            "stage_timings": [
                                {
                                    "stage": f"lexical.{name}",
                                    "elapsed_ns": 100,
                                    "calls": 1,
                                    "returned_candidates": returned
                                    if name in ("search", "project")
                                    else None,
                                }
                                for name in ("prepare", "read_view", "search", "project")
                            ],
                        },
                    },
                }
            )
        diagnostic_path = qdir / "retrieval-diagnostic.json"
        if diagnostic_version in (5, 6, 7, 8) and query_observation == "disabled":
            for row in diagnostic_rows:
                row["response"]["explanation"]["stage_timings"] = None
        if diagnostic_version in (6, 7, 8):
            for row in diagnostic_rows:
                row["response"]["explanation"]["planner_trace"] = []
        diagnostic_path.write_text(
            json.dumps(
                {
                    "schema_version": diagnostic_version,
                    **(
                        {
                            "hybrid_fetch_policy": pairrun.hybrid_fetch_policy_configuration(
                                hybrid_floor
                            )
                        }
                        if diagnostic_version in (6, 7, 8)
                        else {}
                    ),
                    **(
                        {
                            "server_observation": pairrun.server_observation_configuration(
                                query_observation
                            ),
                            "ingest": ingest,
                        }
                        if diagnostic_version in (5, 6, 7, 8)
                        else {}
                    ),
                    "kind": "quanta_returned_window_diagnostic",
                    "record_sha256": ev.digest(qpath.read_bytes()),
                    "query_pack_sha256": qrec["query_pack_sha256"],
                    "top_k": qrec["comparison_contract"]["top_k"],
                    "scope": "returned_window_only",
                    "results": diagnostic_rows,
                    "runner_timing_detail_ms": {
                        "clock": "runner_monotonic_wall_v1",
                        "daemon_boot_and_readiness": 1.0,
                        "sdk_publish_and_activate_opaque": 1.0,
                        **(
                            {"sdk_publish": 0.6, "sdk_activate": 0.3}
                            if diagnostic_version in (7, 8)
                            else {}
                        ),
                        "runner_record_assembly": 0.1,
                        "corpus_reverification": 0.1,
                        "daemon_shutdown": 0.1,
                    },
                }
            ),
            encoding="utf-8",
        )
        (rep_dir / "quanta" / "quanta-manifest.json").write_text(
            json.dumps(
                {
                    "runs": [
                        {
                            "strategy": "whole_file",
                            "record": "strategy-00-whole_file/record.json",
                            "record_digest": ev.digest(qpath.read_bytes()),
                            "retrieval_diagnostic": "strategy-00-whole_file/retrieval-diagnostic.json",
                            "retrieval_diagnostic_digest": ev.digest(diagnostic_path.read_bytes()),
                            "index_bytes": 4096,
                            "runner_binary_sha256": binary_digest,
                            "driver_ms": 1.0,
                            "symbol_preflight": "strategy-00-whole_file/symbol-preflight.json",
                            "symbol_preflight_digest": pairrun.sha_file(preflight),
                            "phase_metrics": "strategy-00-whole_file/phase-metrics.json",
                            "phase_metrics_digest": ev.digest(qphase.read_bytes()),
                            "resource_metrics": "strategy-00-whole_file/resource-metrics.json",
                            "resource_metrics_digest": ev.digest(qresource.read_bytes()),
                            "state_root": "strategy-00-whole_file/state",
                        }
                    ]
                }
            ),
            encoding="utf-8",
        )
        rep_layouts.append(
            {
                "rep": rep,
                "order": (
                    ["quanta", "semble"]
                    if rep % 2 == 0 or not alternate_system_order
                    else ["semble", "quanta"]
                ),
                "query_protocol": str(protocol_path),
                "quanta": {"whole_file": str(qpath)},
                "quanta_phase_metrics": {"whole_file": str(qphase)},
                "semble": str(spath),
                "quanta_manifest": str(rep_dir / "quanta" / "quanta-manifest.json"),
                "semble_phase_metrics": str(sphase),
                "semble_resource_metrics": str(sresource),
            }
        )

    matrix = pairrun.build_latency_matrix(rep_layouts)
    (stage / "latency-matrix.json").write_text(json.dumps(matrix), encoding="utf-8")
    _s, _p, combined = pairrun.merge_records(
        repo,
        suite_path,
        [Path(rep_layouts[0]["quanta"]["whole_file"]), Path(rep_layouts[0]["semble"])],
    )
    report = ev.evaluate(_s, _p, combined, "hybrid", "lexical", strict_k=True)
    report_name = "report-hybrid-vs-lexical-whole_file.json"
    (stage / report_name).write_text(json.dumps(report), encoding="utf-8")
    host = {
        "system": "Darwin",
        "release": "test",
        "machine": "arm64",
        "processor": "test-cpu",
        "cpu_count": 8,
        "python": "3.11",
        "rustc": "rustc test",
        "concurrent_processes": {"none": []},
        "contention_override": False,
        "thermal": {"status": "clean", "evidence": "test"},
        "frequency": {"status": "bounded", "evidence": {}},
        "power": {"status": "bounded", "digest": _fake_sha("power")},
    }
    if not host_clean:
        host = {**host, "concurrent_processes": {"cargo": [123]}}
    host_start = dict(host)
    host_end = dict(host)
    (stage / "host-start.json").write_text(json.dumps(host_start), encoding="utf-8")
    (stage / "host-end.json").write_text(json.dumps(host_end), encoding="utf-8")
    timeline, _profile = _clean_host_timeline_fixture()
    for sample in timeline["samples"]:
        sample["probe"] = copy.deepcopy(host)
    monitor_header = {
        "kind": "cooperative-host-observations",
        "schema_version": 1,
        "capture_id": "retrieval-host",
        "profile": "qualified-speed",
        "reservation_id": timeline["reservation_id"],
        "lock_identity": [1, 2, 3, stat.S_IFREG | 0o600, 1],
        "interval_ns": 1_000_000_000,
        "max_gap_ns": 10_000_000_000,
        "clock_tolerance_ns": 1_000_000_000,
        "host": {
            "os": "macos",
            "arch": "arm64",
            "cpu_count": 8,
            "hostname_hash": "sha256:" + "c" * 64,
        },
    }
    monitor_rows = [monitor_header]
    for index, sample in enumerate(timeline["samples"]):
        monitor_rows.append(
            {
                "sequence": index,
                "event": "start" if index == 0 else "end" if index == 2 else "sample",
                "phase": "preparation",
                "capture_id": "retrieval-host",
                "reservation_id": timeline["reservation_id"],
                "monotonic_ns": sample["finished_ns"],
                "wall_ns": sample["finished_ns"],
                "facts": {
                    "load_average": [0.0, 0.0, 0.0],
                    "disk_available_bytes": 100,
                    "process_count": 1,
                    "process_snapshot_sha256": "sha256:" + "d" * 64,
                    "foreign_rust": [],
                },
                "status": "completed" if index == 2 else "active",
            }
        )
    monitor_raw = stage / "host-timeline.jsonl"
    monitor_raw.write_text(
        "".join(json.dumps(row) + "\n" for row in monitor_rows), encoding="utf-8"
    )
    timeline["monitor_sha256"] = pairrun.sha_file(monitor_raw)
    (stage / "host-timeline.json").write_text(json.dumps(timeline), encoding="utf-8")
    host_profile = stage / "host-profile.json"
    host_profile.write_text(
        json.dumps(
            {
                "schema_version": 2,
                "profile_id": "test-host",
                "fingerprint": pairrun._host_fingerprint(host),
            }
        ),
        encoding="utf-8",
    )
    semble_lockfile = stage / "rep-00" / "semble" / "lockfile.txt"
    spec = {
        "spec_version": 2,
        "symbol_coverage_policy": "allow-incomplete",
        "repo": str(repo),
        "manifest": str(corpus_path),
        "suite": str(suite_path),
        "query_pack": str(pack_path),
        "execution_profiles": {
            "quanta": qp.execution_profile("native"),
            "semble": semble_adapter.execution_profile("native-default", None),
        },
        "runner_binary": str(runner_binary),
        "semble_lockfile": str(semble_lockfile),
        "host_profile": str(host_profile),
        "blinding": blinding,
        "isolation_method": isolation_method,
        "access_block_log": access_block_log,
        "scope": scope,
        "claims": claims or {},
        "embedder": embedder,
        "cache_regime": cache_regime,
    }
    if receipts == "full" or (scope == "qualified" and receipts is None):
        source_sha = pairrun.git_head_sha(Path(__file__).resolve().parents[3])
        contents = _full_receipts(
            source_sha,
            binary_digest,
            work / "receipt-binaries",
            sdk_build_profile=sdk_build_profile,
        )
    else:
        contents = receipts or {}
    frozen = {}
    if contents:
        rdir = stage / "receipts"
        rdir.mkdir(exist_ok=True)
        for key, content in contents.items():
            data = content if isinstance(content, bytes) else json.dumps(content).encode()
            target = rdir / f"{key}.json"
            target.write_bytes(data)
            frozen[key] = str(target)
        for rail in ("contract", "sdk"):
            context_key = f"{rail}_execution_context"
            if context_key not in frozen:
                continue
            context = json.loads(Path(frozen[context_key]).read_bytes())
            binary_root = rdir / f"{rail}-binaries"
            binary_root.mkdir()
            for role, row in context["binaries"].items():
                shutil.copyfile(row["path"], binary_root / role)

    frozen_admission = {}
    driver_source_closure_digest = None
    if scope == "qualified" or receipts == "full":
        source_receipt = json.loads(
            Path(frozen["contract_python_receipt"]).read_text(encoding="utf-8")
        )
        closure = source_receipt["source_closure"]
    else:
        closure = {
            "schema_version": 1,
            "profile": "retrieval",
            "revision": pairrun.git_head_sha(Path(__file__).resolve().parents[3]),
            "roots": ["tools/benchmark/retrieval"],
            "files": [{"path": "tools/benchmark/retrieval/run.py", "sha256": _fake_sha("source")}],
        }
        closure["digest"] = ev.digest(ev.canonical(closure))
    source_closure_path = stage / "driver-source-closure.json"
    source_closure_path.write_text(json.dumps(closure), encoding="utf-8")
    spec["_driver_source_closure"] = str(source_closure_path)
    driver_source_closure_digest = closure["digest"]
    if scope == "qualified":
        evidence_dir = stage / "admission"
        evidence_dir.mkdir(exist_ok=True)
        license_path = evidence_dir / "license-receipt.json"
        annotation_paths = [
            evidence_dir / "annotation-1-receipt.json",
            evidence_dir / "annotation-2-receipt.json",
        ]
        adjudication_path = evidence_dir / "adjudication-receipt.json"
        license_path.write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "reviewer_id": "license-owner",
                    "decision": "approved",
                    "repository_commit": suite["repository_commit"],
                    "corpus_manifest_sha256": pairrun.sha_file(corpus_path),
                    "rationale": "Fixture corpus approved for benchmark use.",
                }
            ),
            encoding="utf-8",
        )
        suite_sha = pairrun.sha_file(suite_path)
        reviews = [
            {
                "task_id": task["task_id"],
                "query_sha256": task["query_sha256"],
                "labels": {"answerable": task["answerable"], "gold": task["gold"]},
                "rationale": "Source span checked against the fixture file.",
            }
            for task in suite["tasks"]
        ]
        for path, reviewer_id in zip(
            annotation_paths, ("gold-owner-a", "gold-owner-b"), strict=True
        ):
            path.write_text(
                json.dumps(
                    {
                        "schema_version": 1,
                        "reviewer_id": reviewer_id,
                        "suite_sha256": suite_sha,
                        "reviews": reviews,
                    }
                ),
                encoding="utf-8",
            )
        adjudication_path.write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "reviewer_id": "gold-adjudicator",
                    "suite_sha256": suite_sha,
                    "annotation_receipt_sha256": [
                        pairrun.sha_file(path) for path in annotation_paths
                    ],
                    "reviews": reviews,
                }
            ),
            encoding="utf-8",
        )
        development_suite = json.loads(json.dumps(suite))
        development_suite["suite_id"] = "fixture-development"
        development_suite["file_universe"] = [
            {"path": "excluded.txt", "file_sha256": ev.digest(files["excluded.txt"])}
        ]
        development_suite["file_universe_digest"] = ev.universe_digest(
            development_suite["file_universe"]
        )
        development_task = json.loads(json.dumps(suite["tasks"][0]))
        development_task["task_id"] = "D1"
        development_task["query"] = "locate quarantined excluded fixture"
        development_task["query_sha256"] = ev.digest(development_task["query"].encode())
        development_task["query_family_id"] = "development-excluded"
        dev_file_sha, dev_block_sha, _ = _span_meta(files["excluded.txt"], 1, 1)
        dev_start, dev_end = _byte_span(files["excluded.txt"], 1, 1)
        development_task["gold"] = [
            {
                "path": "excluded.txt",
                "start_byte": dev_start,
                "end_byte": dev_end,
                "start_line": 1,
                "end_line": 1,
                "file_sha256": dev_file_sha,
                "block_sha256": dev_block_sha,
                "grade": 3,
            }
        ]
        development_suite["tasks"] = [development_task]
        development_suite_path = evidence_dir / "development-suite.json"
        development_suite_path.write_text(json.dumps(development_suite), encoding="utf-8")
        experiment_custody = {
            "schema_version": 1,
            "source_revision": pairrun.git_head_sha(Path(__file__).resolve().parents[3]),
            "repository_commit": suite["repository_commit"],
            "development_suite_sha256": ev.digest(ev.canonical(development_suite)),
            "holdout_suite_sha256": ev.digest(ev.canonical(suite)),
        }
        experiment_custody_path = evidence_dir / "experiment-custody.json"
        experiment_custody_path.write_text(json.dumps(experiment_custody), encoding="utf-8")
        admission_path = evidence_dir / "admission.json"
        admission = {
            "schema_version": 2,
            "admission_id": "test-qualified-admission",
            "issued_at": "2026-09-24T00:00:00Z",
            "source_revision": pairrun.git_head_sha(Path(__file__).resolve().parents[3]),
            "repository_commit": suite["repository_commit"],
            "corpus_manifest_sha256": pairrun.sha_file(corpus_path),
            "suite_sha256": pairrun.sha_file(suite_path),
            "development_suite_sha256": pairrun.sha_file(development_suite_path),
            "experiment_custody_sha256": pairrun.sha_file(experiment_custody_path),
            "query_pack_sha256": pairrun.sha_file(pack_path),
            "license": {
                "reviewer_id": "license-owner",
                "decision": "approved",
                "receipt_sha256": pairrun.sha_file(license_path),
            },
            "gold": {
                "frozen_before_results": True,
                "annotators": [
                    {
                        "annotator_id": "gold-owner-a",
                        "receipt_sha256": pairrun.sha_file(annotation_paths[0]),
                    },
                    {
                        "annotator_id": "gold-owner-b",
                        "receipt_sha256": pairrun.sha_file(annotation_paths[1]),
                    },
                ],
                "adjudicator_id": "gold-adjudicator",
                "adjudication_receipt_sha256": pairrun.sha_file(adjudication_path),
            },
            "models": {
                "quanta_model_revision": "not-applicable",
                "semble_model_revision": "b" * 40,
                "semble_model_asset_sha256": _fake_sha("model"),
            },
            "semble_lockfile_sha256": pairrun.sha_file(semble_lockfile),
            "host_profile_sha256": pairrun.sha_file(host_profile),
            "cache_regime": cache_regime,
            "verification": {
                "contract_python_receipt_sha256": pairrun.sha_file(
                    Path(frozen["contract_python_receipt"])
                ),
                "contract_rust_receipt_sha256": pairrun.sha_file(
                    Path(frozen["contract_rust_receipt"])
                ),
                "sdk_receipt_sha256": pairrun.sha_file(Path(frozen["sdk_receipt"])),
            },
        }
        admission_path.write_text(json.dumps(admission), encoding="utf-8")
        frozen_admission = {
            "manifest": str(admission_path),
            "experiment_custody": str(experiment_custody_path),
            "development_suite": str(development_suite_path),
            "license_receipt": str(license_path),
            "annotation_receipts": [str(path) for path in annotation_paths],
            "adjudication_receipt": str(adjudication_path),
        }
        spec["admission"] = dict(frozen_admission)

    (stage / "protocol-lock.json").write_text(
        json.dumps(
            {
                "lock_version": {4: 2, 5: 3, 6: 4, 7: 5, 8: 6}[diagnostic_version],
                "retrieval_diagnostic_version": diagnostic_version,
                "symbol_coverage_policy": "allow-incomplete",
                **(
                    {"hybrid_fetch_policy": pairrun.hybrid_fetch_policy_configuration(hybrid_floor)}
                    if diagnostic_version in (6, 7, 8)
                    else {}
                ),
                **(
                    {
                        "server_observation": pairrun.server_observation_configuration(
                            query_observation
                        ),
                        "ingest_request_identity": pairrun.ingest_request_identity({}),
                    }
                    if diagnostic_version in (5, 6, 7, 8)
                    else {}
                ),
                "rank_metric_k_policy": "declared_top_k_v1",
                "suite_digest": ev.digest(suite_path.read_bytes()),
                "query_pack_digest": ev.digest(pack_path.read_bytes()),
                "corpus_manifest_digest": ev.digest(corpus_path.read_bytes()),
                "top_k": 10,
                "strategies": ["whole_file"],
                "quanta_routes": ["lexical"],
                "semble_route": "hybrid",
                "searchd_expected_sha256": _fake_sha("searchd"),
                "semble_lockfile_sha256": ev.digest(b"semble==0.6.0\n"),
                "host_profile_digest": pairrun.sha_file(host_profile),
                "admission_digest": (
                    pairrun.sha_file(Path(frozen_admission["manifest"]))
                    if frozen_admission
                    else None
                ),
                "driver_source_closure_digest": driver_source_closure_digest,
                "repetitions": repetitions,
                "system_orders": [layout["order"] for layout in rep_layouts],
                "base_seed": 0,
                "query_warmup_passes": 1,
                "query_repetitions_per_root": measurements,
                "execution_profiles": spec["execution_profiles"],
                "execution_profiles_sha256": ev.digest(ev.canonical(spec["execution_profiles"])),
                "query_protocol_sha256s": [
                    json.loads(Path(layout["query_protocol"]).read_text(encoding="utf-8"))["sha256"]
                    for layout in rep_layouts
                ],
            }
        ),
        encoding="utf-8",
    )
    manifest = pairrun.build_run_manifest(
        spec, stage, rep_layouts, host_start, host_end, [report_name], frozen, frozen_admission
    )
    manifest_path = stage / "run-manifest.json"
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    result = {
        "repo": repo,
        "suite": suite,
        "stage": stage,
        # The driver-only closure path belongs to the staged capture, not to
        # the public spec that callers submit to load_spec/benchctl.
        "spec": {key: value for key, value in spec.items() if key != "_driver_source_closure"},
        "manifest": manifest,
        "manifest_path": manifest_path,
        "suite_path": suite_path,
        "rep_layouts": rep_layouts,
        "binary_digest": binary_digest,
        "commit": suite["repository_commit"],
    }
    if diagnostic_version == 8:
        from tools.ci.tests.test_completed_response_timing import _add_timing

        _add_timing(result, sdk_children=True)
        result["manifest"] = json.loads(manifest_path.read_text())
    return result


def _stage_verdict(st):
    return pairrun.build_verdict(st["repo"], st["suite_path"], st["manifest_path"])


def _allow_minimal_speed_fixture(monkeypatch, *, observations=2):
    """Exercise downstream verdict checks with the two-task synthetic fixture.

    Production thresholds are covered by the speed-spec boundary tests.
    """
    monkeypatch.setattr(pairrun, "FROZEN_TASKS_FLOOR", 2)
    monkeypatch.setattr(pairrun, "FRESH_ROOTS_FLOOR", 1)
    if observations is not None:
        monkeypatch.setattr(pairrun, "PILOT_OBSERVATIONS_FLOOR", observations)


def _rewrite_manifest(st, mutator):
    manifest = json.loads(st["manifest_path"].read_text(encoding="utf-8"))
    mutator(manifest)
    st["manifest_path"].write_text(json.dumps(manifest), encoding="utf-8")


def _rebind_phase_metrics_digests(st, *paths):
    """Keep the capture-byte binding valid when testing a later verdict gate."""
    updates = {path.relative_to(st["stage"]).as_posix(): pairrun.sha_file(path) for path in paths}
    _rewrite_manifest(
        st,
        lambda manifest: manifest["artifacts"]["phase_metrics_digests"].update(updates),
    )


def _rebind_semble_native_to_protocol(layout, protocol, pack):
    """Rebuild the synthetic Semble actual-call trace for a mutated protocol."""
    native_path = Path(layout["semble"]).parent / "native.json"
    native = json.loads(native_path.read_text(encoding="utf-8"))
    assert native["semble_profile"] == "native-default"
    tasks = {task["task_id"]: task for task in pack["tasks"]}
    schedule = [(0, "cold", 0, protocol["cold_probe_task_id"])]
    for iteration, task_ids in enumerate(protocol["warmup_schedules"]):
        schedule.extend((0, "warmup", iteration, task_id) for task_id in task_ids)
    for repetition, task_ids in enumerate(protocol["measurement_schedules"]):
        schedule.extend((repetition, "measured", repetition, task_id) for task_id in task_ids)
    profile_sha = ev.digest(ev.canonical(semble_adapter.execution_profile("native-default", None)))
    native["execution_events"] = [
        {
            "rep": repetition,
            "phase": phase,
            "phase_iteration": iteration,
            "task_id": task_id,
            "call_ordinal": ordinal,
            "submitted_query_sha256": tasks[task_id]["query_sha256"],
            "profile_sha256": profile_sha,
            "actual_alpha": 0.5,
            "actual_rerank": True,
            "candidate_depth": 50,
            "lane_entry_counts": {"bm25": 1, "semantic": 1},
            "lane_candidate_depths": {"bm25": [50], "semantic": [50]},
        }
        for ordinal, (repetition, phase, iteration, task_id) in enumerate(schedule)
    ]
    native["execution_events_sha256"] = ev.digest(ev.canonical(native["execution_events"]))
    native["lane_call_counts"] = {
        "bm25": len(schedule),
        "semantic": len(schedule),
        "encode": len(schedule),
    }
    native["query_protocol"] = protocol
    native["warmup_passes"] = len(protocol["warmup_schedules"])
    native["repetitions"] = len(protocol["measurement_schedules"])
    native["actual_alpha_by_task"] = {
        task_id: 0.5 for task_id in protocol["measurement_schedules"][0]
    }
    rows = {row["task_id"]: row for row in native["native"]}
    native["native"] = [rows[task_id] for task_id in protocol["measurement_schedules"][0]]
    phase_path = Path(layout["semble_phase_metrics"])
    phase = json.loads(phase_path.read_text(encoding="utf-8"))
    native["latencies_ms"] = phase["warm_latencies_ms"]["hybrid"]
    native_path.write_text(json.dumps(native), encoding="utf-8")
    for key in ("execution_events_sha256", "lane_call_counts"):
        phase[key] = native[key]
    phase_path.write_text(json.dumps(phase), encoding="utf-8")
    adapter_path = native_path.parent / "adapter-manifest.json"
    adapter = json.loads(adapter_path.read_text(encoding="utf-8"))
    for key in (
        "actual_alpha_by_task",
        "execution_events_sha256",
        "lane_call_counts",
        "function_identity",
        "observed_wrapped_call_ns",
        "rerank_applied",
        "requested_alpha",
    ):
        adapter[key] = native[key]
    adapter_path.write_text(json.dumps(adapter), encoding="utf-8")


def test_verdict_pair_only_green(tmp_path):
    st = _pair_stage(tmp_path)
    jsonschema.validate(st["manifest"], _load_schema("run-manifest.schema.json"))
    verdict = _stage_verdict(st)
    jsonschema.validate(verdict, _load_schema("verdict.schema.json"))
    assert verdict["verdict_version"] == 2
    assert verdict["states"] == {
        "CONTRACT_GREEN": "not_run",
        "SDK_PATH_GREEN": "not_run",
        "PAIR_VALID": "pass",
        "PERF_QUALIFIED": "not_applicable",
        "QUALITY_DELTA": "not_applicable",
    }
    assert verdict["failure_class"] == "none"
    assert "T01" in verdict["missing_t_ids"] and "T05" in verdict["missing_t_ids"]
    assert "T00" not in verdict["missing_t_ids"]
    assert verdict["not_applicable_t_ids"] == ["T15", "T16"]
    assert len(verdict["comparisons"]) == 1
    comparison = verdict["comparisons"][0]
    assert comparison["strategy"] == "whole_file"
    assert comparison["baseline_route"] == "hybrid"
    assert comparison["candidate_route"] == "lexical"
    assert len(comparison["record_digest"]) == 64
    assert verdict["counts"] == {"selected": 4, "executed": 4, "passed": 4, "failed": 0}
    for name, proof in verdict["state_evidence"].items():
        assert proof["reason"], name
    assert verdict["state_evidence"]["PAIR_VALID"]["proof_digest"] is not None
    assert st["commit"] != st["manifest"]["provenance"]["quanta"]["source_sha"]
    assert (
        verdict["provenance"]["quanta"]["source_sha"]
        == (st["manifest"]["provenance"]["quanta"]["source_sha"])
    )


def test_verdict_rederives_suite_once_and_rejects_forged_candidate(tmp_path, monkeypatch):
    st = _pair_stage(tmp_path)
    validate = pairrun.validate_suite
    calls = 0

    def counted(*args, **kwargs):
        nonlocal calls
        calls += 1
        return validate(*args, **kwargs)

    monkeypatch.setattr(pairrun, "validate_suite", counted)
    assert _stage_verdict(st)["states"]["PAIR_VALID"] == "pass"
    assert calls == 1

    record_path = Path(st["rep_layouts"][0]["quanta"]["whole_file"])
    record = json.loads(record_path.read_text(encoding="utf-8"))
    record["results"][0]["candidates"][0]["block_sha256"] = "0" * 64
    record_path.write_text(json.dumps(record), encoding="utf-8")
    assert _stage_verdict(st)["states"]["PAIR_VALID"] == "fail"
    assert calls == 2


def test_verdict_reads_large_semble_native_artifact_once(tmp_path, monkeypatch):
    st = _pair_stage(tmp_path)
    native_path = Path(st["rep_layouts"][0]["semble"]).parent / "native.json"
    read = pairrun.read_json
    native_reads = 0

    def counted(path):
        nonlocal native_reads
        if Path(path) == native_path:
            native_reads += 1
        return read(path)

    monkeypatch.setattr(pairrun, "read_json", counted)
    assert _stage_verdict(st)["states"]["PAIR_VALID"] == "pass"
    assert native_reads == 1


def test_current_pair_rejects_legacy_phase_metrics(tmp_path):
    st = _pair_stage(tmp_path)
    layout = st["rep_layouts"][0]
    phase_path = Path(layout["quanta_phase_metrics"]["whole_file"])
    phase = json.loads(phase_path.read_text(encoding="utf-8"))
    phase["schema_version"] = 1
    for key in (
        "symbol_count",
        "symbol_producer_identity",
        "symbol_grammars",
        "symbol_unsupported_files",
        "symbol_only_scopes",
    ):
        phase.pop(key)
    phase_path.write_text(json.dumps(phase), encoding="utf-8")
    manifest_path = Path(layout["quanta_manifest"])
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    manifest["runs"][0]["phase_metrics_digest"] = pairrun.sha_file(phase_path)
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert "phase_metrics_invalid" in verdict["state_evidence"]["PAIR_VALID"]["reason"]


def test_verdict_replays_semble_event_order_and_query_identity(tmp_path):
    st = _pair_stage(tmp_path)
    layout = st["rep_layouts"][0]
    native_path = Path(layout["semble"]).parent / "native.json"
    native = json.loads(native_path.read_text(encoding="utf-8"))
    native["execution_events"][0]["call_ordinal"] = 999
    native["execution_events"][0]["submitted_query_sha256"] = _fake_sha("wrong-query")
    native["execution_events_sha256"] = ev.digest(ev.canonical(native["execution_events"]))
    native_path.write_text(json.dumps(native), encoding="utf-8")
    phase_path = Path(layout["semble_phase_metrics"])
    phase = json.loads(phase_path.read_text(encoding="utf-8"))
    phase["execution_events_sha256"] = native["execution_events_sha256"]
    phase_path.write_text(json.dumps(phase), encoding="utf-8")
    _rebind_phase_metrics_digests(st, phase_path)
    verdict = _stage_verdict(st)
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert (
        "semble_native_actual_call_invalid" in (verdict["state_evidence"]["PAIR_VALID"]["reason"])
    )

    adapter_stage = _pair_stage(tmp_path / "adapter")
    adapter_path = Path(adapter_stage["stage"]) / "rep-00" / "semble" / "adapter-manifest.json"
    adapter = json.loads(adapter_path.read_text(encoding="utf-8"))
    adapter["observed_wrapped_call_ns"] += 1
    adapter_path.write_text(json.dumps(adapter), encoding="utf-8")
    adapter_verdict = _stage_verdict(adapter_stage)
    assert adapter_verdict["states"]["PAIR_VALID"] == "fail"
    assert (
        "adapter_native_actual_call_binding_broken"
        in (adapter_verdict["state_evidence"]["PAIR_VALID"]["reason"])
    )


def test_current_protocol_profile_is_bound_on_every_semble_repetition(tmp_path):
    st = _pair_stage(tmp_path, repetitions=2)
    record_path = st["stage"] / "rep-01" / "semble" / "record.json"
    record = json.loads(record_path.read_text(encoding="utf-8"))
    profile = semble_adapter.execution_profile("lexical-only", None)
    capture = next(iter(record["captures"].values()))
    capture["execution_profile"] = profile
    capture["execution_profile_sha256"] = ev.digest(ev.canonical(profile))
    record_path.write_text(json.dumps(record), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert (
        "execution_profile_record_drift:rep-01:semble"
        in verdict["state_evidence"]["PAIR_VALID"]["reason"]
    )


def test_model_cache_identity_is_bound_on_every_semble_repetition(tmp_path):
    st = _pair_stage(tmp_path, repetitions=2)
    cache_path = st["stage"] / "rep-01" / "semble" / "model-cache-manifest.json"
    manifest = json.loads(cache_path.read_text(encoding="utf-8"))
    manifest["model_asset_digest"] = _fake_sha("different-model")
    core = {key: value for key, value in manifest.items() if key != "snapshot_digest"}
    manifest["snapshot_digest"] = ev.digest(ev.canonical(core))
    cache_path.write_text(json.dumps(manifest), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert (
        "model cache asset digest differs from adapter"
        in (verdict["state_evidence"]["PAIR_VALID"]["reason"])
    )


def test_observation_protocol_v3_replays_and_refuses_policy_or_batch_scope_drift(tmp_path):
    st = _pair_stage(tmp_path, diagnostic_version=5, query_observation="disabled")
    assert _stage_verdict(st)["states"]["PAIR_VALID"] == "pass"
    lock_path = st["stage"] / "protocol-lock.json"
    lock = json.loads(lock_path.read_text())
    for key, value in (
        ("server_observation", pairrun.server_observation_configuration("enabled")),
        ("ingest_request_identity", {**lock["ingest_request_identity"], "repo_id": "another-repo"}),
    ):
        changed = json.loads(json.dumps(lock))
        changed[key] = value
        lock_path.write_text(json.dumps(changed))
        result = _stage_verdict(st)
        assert result["states"]["PAIR_VALID"] == "fail"
        assert "retrieval_diagnostic_invalid" in result["state_evidence"]["PAIR_VALID"]["reason"]
    lock_path.write_text(json.dumps(lock))
    assert _stage_verdict(st)["states"]["PAIR_VALID"] == "pass"
    for field in ("lock_version", "retrieval_diagnostic_version"):
        changed = json.loads(json.dumps(lock))
        changed[field] = float(changed[field])
        lock_path.write_text(json.dumps(changed))
        result = _stage_verdict(st)
        assert result["states"]["PAIR_VALID"] == "fail"
        assert result["state_evidence"]["PAIR_VALID"]["reason"] == "protocol_lock_malformed"


def test_hybrid_fetch_protocol_v4_binds_every_capture_and_refuses_config_aliases(tmp_path):
    st = _pair_stage(
        tmp_path, diagnostic_version=6, query_observation="disabled", hybrid_floor="25"
    )
    assert _stage_verdict(st)["states"]["PAIR_VALID"] == "pass"
    lock_path = st["stage"] / "protocol-lock.json"
    lock = json.loads(lock_path.read_text())
    assert lock["hybrid_fetch_policy"] == pairrun.hybrid_fetch_policy_configuration("25")
    for policy in (
        pairrun.hybrid_fetch_policy_configuration("50"),
        {**lock["hybrid_fetch_policy"], "floor": 25.0},
        {**lock["hybrid_fetch_policy"], "floor": True},
        {**lock["hybrid_fetch_policy"], "config_sha256": "0" * 64},
        {**lock["hybrid_fetch_policy"], "scope": "default"},
        {**lock["hybrid_fetch_policy"], "unexpected": 0},
    ):
        changed = json.loads(json.dumps(lock))
        changed["hybrid_fetch_policy"] = policy
        lock_path.write_text(json.dumps(changed))
        assert _stage_verdict(st)["states"]["PAIR_VALID"] == "fail"
    lock_path.write_text(json.dumps(lock))
    for mutate in (
        lambda value: value.pop("hybrid_fetch_policy"),
        lambda value: value.update(lock_version=4.0),
        lambda value: value.update(retrieval_diagnostic_version=6.0),
        lambda value: value.update(retrieval_diagnostic_version=5),
    ):
        changed = json.loads(json.dumps(lock))
        mutate(changed)
        lock_path.write_text(json.dumps(changed))
        assert _stage_verdict(st)["states"]["PAIR_VALID"] == "fail"
    lock_path.write_text(json.dumps(lock))
    assert _stage_verdict(st)["states"]["PAIR_VALID"] == "pass"


def test_hybrid_initial_fetch_trace_requires_exact_policy_probe_and_unique_plan():
    # Hand oracle: fixed floor, public cap 10000, one-row continuation probe.
    for floor, top_k, expected in (
        ("25", 1, 25),
        ("25", 10, 25),
        ("25", 100, 101),
        ("50", 1, 50),
        ("50", 10, 50),
        ("50", 100, 101),
        ("100", 1, 100),
        ("100", 10, 100),
        ("100", 100, 101),
        ("25", 10000, 10001),
        ("100", 10000, 10001),
    ):
        policy = pairrun.hybrid_fetch_policy_configuration(floor)
        trace = [{"stage": "plan", "detail": f"hybrid.internal_top_k={expected}"}]
        pairrun._validate_hybrid_initial_fetch(trace, top_k, policy)
        for mutant in (
            [],
            trace + trace,
            [{"stage": "plan", "detail": f"hybrid.internal_top_k={expected - 1}"}],
            [{"stage": "parse", "detail": trace[0]["detail"]}],
            [{"stage": "plan", "detail": f"hybrid.internal_top_k=0{expected}"}],
            [{"stage": "plan", "detail": f"hybrid.internal_top_k={expected}.0"}],
        ):
            with pytest.raises(pairrun.RunError, match="initial fetch"):
                pairrun._validate_hybrid_initial_fetch(mutant, top_k, policy)


def test_new_protocol_requires_bound_retrieval_diagnostic_on_replay(tmp_path):
    st = _pair_stage(tmp_path)
    stage = st["stage"]
    protocol_path = stage / "protocol-lock.json"
    protocol = json.loads(protocol_path.read_text(encoding="utf-8"))
    protocol["retrieval_diagnostic_version"] = 1
    protocol_path.write_text(json.dumps(protocol), encoding="utf-8")
    assert _stage_verdict(st)["state_evidence"]["PAIR_VALID"]["reason"] == "protocol_lock_malformed"
    protocol["rank_metric_k_policy"] = "declared_top_k_v1"
    protocol_path.write_text(json.dumps(protocol), encoding="utf-8")
    protocol["rank_metric_k_policy"] = "unknown"
    protocol_path.write_text(json.dumps(protocol), encoding="utf-8")
    assert _stage_verdict(st)["state_evidence"]["PAIR_VALID"]["reason"] == "protocol_lock_malformed"
    protocol["rank_metric_k_policy"] = "declared_top_k_v1"
    protocol["retrieval_diagnostic_version"] = 2
    protocol_path.write_text(json.dumps(protocol), encoding="utf-8")
    assert _stage_verdict(st)["state_evidence"]["PAIR_VALID"]["reason"] == "protocol_lock_malformed"
    protocol["retrieval_diagnostic_version"] = 4
    protocol_path.write_text(json.dumps(protocol), encoding="utf-8")
    qmanifest_path = stage / "rep-00" / "quanta" / "quanta-manifest.json"
    qmanifest = json.loads(qmanifest_path.read_text(encoding="utf-8"))
    run = qmanifest["runs"][0]
    diagnostic_ref = run["retrieval_diagnostic"]
    diagnostic_digest = run["retrieval_diagnostic_digest"]
    for key in ("retrieval_diagnostic", "retrieval_diagnostic_digest"):
        run.pop(key, None)
    qmanifest_path.write_text(json.dumps(qmanifest), encoding="utf-8")
    assert _stage_verdict(st)["states"]["PAIR_VALID"] == "fail"

    run["retrieval_diagnostic"] = diagnostic_ref
    run["retrieval_diagnostic_digest"] = diagnostic_digest
    qmanifest_path.write_text(json.dumps(qmanifest), encoding="utf-8")
    assert _stage_verdict(st)["states"]["PAIR_VALID"] == "pass"
    diagnostic_path = qmanifest_path.parent / diagnostic_ref
    diagnostic = json.loads(diagnostic_path.read_text(encoding="utf-8"))
    diagnostic["results"][0]["status"] = "timeout"
    diagnostic_path.write_text(json.dumps(diagnostic), encoding="utf-8")
    run["retrieval_diagnostic_digest"] = pairrun.sha_file(diagnostic_path)
    qmanifest_path.write_text(json.dumps(qmanifest), encoding="utf-8")
    assert _stage_verdict(st)["states"]["PAIR_VALID"] == "fail"


def test_verdict_incomplete_observation_fails_pair(tmp_path):
    st = _pair_stage(tmp_path)
    layout = st["rep_layouts"][0]
    spath = Path(layout["semble"])
    payload = json.loads(spath.read_text(encoding="utf-8"))
    assert payload["results"], "stage needs at least one semble row"
    payload["results"][0].update(
        status="error",
        candidates=[],
        timings={"query_latency_ms": None},
        error={"code": "semble_hit_bad_span", "message": "stub span failure"},
    )
    spath.write_text(json.dumps(payload), encoding="utf-8")
    adapter_path = spath.parent / "adapter-manifest.json"
    adapter = json.loads(adapter_path.read_text(encoding="utf-8"))
    adapter["record_digest"] = ev.digest(spath.read_bytes())
    adapter_path.write_text(json.dumps(adapter), encoding="utf-8")
    phase_path = spath.parent / "phase-metrics.json"
    phase = json.loads(phase_path.read_text(encoding="utf-8"))
    phase["record_sha256"] = ev.digest(spath.read_bytes())
    phase_path.write_text(json.dumps(phase), encoding="utf-8")
    _rebind_phase_metrics_digests(st, phase_path)
    resource_path = Path(layout["semble_resource_metrics"])
    resource = json.loads(resource_path.read_text(encoding="utf-8"))
    resource["subject_sha256"] = ev.digest(spath.read_bytes())
    resource_path.write_text(json.dumps(resource), encoding="utf-8")
    # A real run scores whatever the capture observed: rebuild the report
    # from the mutated records so only the incomplete-observation gate fires.
    _s, _p, combined = pairrun.merge_records(
        st["repo"], st["suite_path"], [Path(layout["quanta"]["whole_file"]), spath]
    )
    report = ev.evaluate(_s, _p, combined, "hybrid", "lexical", strict_k=True)
    (st["stage"] / "report-hybrid-vs-lexical-whole_file.json").write_text(
        json.dumps(report), encoding="utf-8"
    )
    verdict = _stage_verdict(st)
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert verdict["state_evidence"]["PAIR_VALID"]["reason"].startswith(
        "incomplete_observation:rep-00:semble:semble_native:"
    )
    assert verdict["counts"]["failed"] == 1


def test_required_inventory_uses_canonical_internal_temporary_path(tmp_path, monkeypatch):
    root = Path(__file__).resolve().parents[3]
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    authority_ref = "benchmarks/retrieval/proof-required-tests.json"
    committed = subprocess.check_output(["git", "show", f"{revision}:{authority_ref}"], cwd=root)
    authority = json.loads(committed)
    receipt = {
        "source_closure": {
            "revision": revision,
            "files": [
                {"path": authority_ref, "sha256": hashlib.sha256(committed).hexdigest()},
            ],
        }
    }
    inventory = tmp_path / "inventory.json"
    payload = {
        "schema_version": 1,
        "kind": "pytest",
        "selector": portable_proof.proof_inventory.PYTHON_SELECTOR,
        "tests": authority["python"],
    }
    inventory.write_text(json.dumps(payload))
    real_temp = tmp_path / "real-temp"
    real_temp.mkdir()
    alias = tmp_path / "temp-alias"
    alias.symlink_to(real_temp, target_is_directory=True)
    temporary_directory = tempfile.TemporaryDirectory

    def aliased_temp(**kwargs):
        return temporary_directory(dir=alias, **kwargs)

    monkeypatch.setattr(pairrun.tempfile, "TemporaryDirectory", aliased_temp)
    pairrun._verify_required_inventory(inventory, "python", receipt)
    payload["tests"] = payload["tests"][:-1]
    inventory.write_text(json.dumps(payload))
    with pytest.raises(pairrun.RunError, match="source authority"):
        pairrun._verify_required_inventory(inventory, "python", receipt)
    target = tmp_path / "linked-inventory.json"
    inventory.rename(target)
    inventory.symlink_to(target)
    with pytest.raises(pairrun.RunError, match="symlink"):
        pairrun._verify_required_inventory(inventory, "python", receipt)


def test_verdict_full_receipts_all_green(tmp_path):
    st = _pair_stage(tmp_path, receipts="full")
    verdict = _stage_verdict(st)
    assert verdict["states"]["CONTRACT_GREEN"] == "pass"
    assert verdict["states"]["SDK_PATH_GREEN"] == "pass"
    assert verdict["states"]["PAIR_VALID"] == "pass"
    assert verdict["failure_class"] == "none"
    assert verdict["missing_t_ids"] == []


@pytest.mark.parametrize("rail,state", [("contract", "CONTRACT_GREEN"), ("sdk", "SDK_PATH_GREEN")])
@pytest.mark.parametrize(
    "mutation", ["argv", "environment", "raw", "stdout_digest", "missing_role"]
)
def test_verdict_refuses_bound_execution_context_tampering(tmp_path, rail, state, mutation):
    st = _pair_stage(tmp_path, receipts="full")
    assert _stage_verdict(st)["states"][state] == "pass"
    receipt_dir = st["stage"] / "receipts"
    context_path = receipt_dir / f"{rail}_execution_context.json"
    context = json.loads(context_path.read_text(encoding="utf-8"))
    if mutation == "argv":
        raw_roles = (
            {
                "python-inventory.json": "contract_python_inventory",
                "rust-inventory.json": "contract_rust_inventory",
                "python-junit.xml": "contract_python_raw",
                "rust-nextest.jsonl": "contract_rust_raw",
            }
            if rail == "contract"
            else {
                "nextest-inventory.json": "sdk_inventory",
                "nextest.jsonl": "sdk_nextest_raw",
                "actual-runner-record.json": "sdk_record_raw",
            }
        )
        raw_paths = {name: receipt_dir / f"{role}.json" for name, role in raw_roles.items()}
        kwargs = {"rail": rail, "raw": raw_paths}
        if rail == "sdk":
            assert [row["name"] for row in context["commands"]] == [
                "source-closure",
                "build-searchd",
                "rust-build",
                "metadata",
                "rust-collection",
                "rust-test",
            ]
            kwargs.update(
                runner_sha=context["binaries"]["runner"]["sha256"],
                searchd_sha=context["binaries"]["searchd"]["sha256"],
            )
        # Mutants must share the frozen binary custody directory. First admit
        # their unchanged bytes so missing fixture files cannot explain RED.
        mutant_path = receipt_dir / f"{rail}-mutant-context.json"
        mutant_path.write_text(json.dumps(context), encoding="utf-8")
        pairrun._verify_execution_context(
            mutant_path,
            receipt_dir / f"{rail}_source_closure.json",
            receipt_dir / f"{rail}_execution_logs.json",
            **kwargs,
        )

        def refuse_context(forged, overrides=None, forged_logs=None, match=None):
            mutant_path.write_text(json.dumps(forged), encoding="utf-8")
            with pytest.raises(pairrun.RunError, match=match):
                pairrun._verify_execution_context(
                    mutant_path,
                    receipt_dir / f"{rail}_source_closure.json",
                    forged_logs or receipt_dir / f"{rail}_execution_logs.json",
                    **{**kwargs, **(overrides or {})},
                )

        for version in (True, 2.0, 1, 1.0):
            refuse_context(dict(context, schema_version=version))
        # Rehash every modified transcript/context link, so refusal must come
        # from native build/collection/Cargo agreement, not missing fixture logs.
        with zipfile.ZipFile(receipt_dir / f"{rail}_execution_logs.json") as source_archive:
            original_logs = {name: source_archive.read(name) for name in source_archive.namelist()}
        build_list = json.loads(original_logs["rust-build.stdout"])
        metadata = json.loads(original_logs["metadata.stdout"])
        binary_id = next(iter(build_list["rust-binaries"]))
        for case in (
            "workspace",
            "target",
            "package-name",
            "manifest",
            "missing-package",
            "missing-binary",
            "extra-binary",
            "binary-path",
            "kind",
            "package-id",
            "binary-name",
            "build-platform",
        ):
            forged = json.loads(json.dumps(context))
            changed_build, changed_metadata = copy.deepcopy(build_list), copy.deepcopy(metadata)
            if case == "workspace":
                changed_metadata["workspace_root"] = "/wrong/workspace"
            elif case == "target":
                changed_metadata["target_directory"] = "/wrong/target"
            elif case == "package-name":
                changed_metadata["packages"][0]["name"] = "wrong-package"
            elif case == "manifest":
                changed_metadata["packages"][0]["manifest_path"] = "/wrong/Cargo.toml"
            elif case == "missing-package":
                changed_metadata["packages"] = []
            elif case == "missing-binary":
                del changed_build["rust-binaries"][binary_id]
            elif case == "extra-binary":
                changed_build["rust-binaries"][binary_id + "-extra"] = dict(
                    changed_build["rust-binaries"][binary_id]
                )
            else:
                changed_build["rust-binaries"][binary_id][case] = "wrong-value"
            changed = {
                "rust-build.stdout": json.dumps(changed_build).encode(),
                "metadata.stdout": json.dumps(changed_metadata).encode(),
            }
            for command in forged["commands"]:
                if command["stdout"] in changed:
                    command["stdout_sha256"] = ev.digest(changed[command["stdout"]])
            forged_logs = tmp_path / f"native-build-{rail}-{case}.zip"
            forged_logs.write_bytes(
                _canonical_log_zip(
                    {name: changed.get(name, payload) for name, payload in original_logs.items()}
                )
            )
            refuse_context(forged, forged_logs=forged_logs, match="native reused build refused")
        native_role = next(role for role in context["binaries"] if role.startswith("nextest-"))
        for mutation_kind in ("missing", "extra", "path", "digest"):
            forged = json.loads(json.dumps(context))
            if mutation_kind == "missing":
                del forged["binaries"][native_role]
            elif mutation_kind == "extra":
                forged["binaries"]["nextest-" + "0" * 64] = dict(forged["binaries"][native_role])
            elif mutation_kind == "path":
                forged["binaries"][native_role]["path"] = "/wrong/path/native-test"
            else:
                forged["binaries"][native_role]["sha256"] = "0" * 64
            refuse_context(forged)
        frozen_binary = receipt_dir / f"{rail}-binaries" / native_role
        payload = frozen_binary.read_bytes()
        frozen_binary.write_bytes(b"substituted native binary")
        refuse_context(context)
        frozen_binary.unlink()
        refuse_context(context)
        frozen_binary.write_bytes(payload)
        unexpected = frozen_binary.parent / "unbound-native-binary"
        unexpected.write_bytes(payload)
        refuse_context(context)
        unexpected.unlink()
        # Valid collection/JSONL bytes with altered whitespace still parse.
        # Rehashing either side cannot detach it from its paired raw artifact.
        raw_links = (
            (("rust-collection", "nextest-inventory.json"), ("rust-test", "nextest.jsonl"))
            if rail == "sdk"
            else (("rust-collection", "rust-inventory.json"), ("rust-test", "rust-nextest.jsonl"))
        )
        for command_name, raw_name in raw_links:
            forged = json.loads(json.dumps(context))
            changed = b" " + raw_paths[raw_name].read_bytes()
            forged_raw = tmp_path / f"changed-{rail}-{raw_name}"
            forged_raw.write_bytes(changed)
            forged["raw_evidence"][raw_name] = pairrun.sha_file(forged_raw)
            refuse_context(forged, {"raw": {**raw_paths, raw_name: forged_raw}})
            forged = json.loads(json.dumps(context))
            command = next(row for row in forged["commands"] if row["name"] == command_name)
            command["stdout_sha256"] = ev.digest(changed)
            forged_logs = tmp_path / f"changed-{rail}-{command_name}.zip"
            with zipfile.ZipFile(receipt_dir / f"{rail}_execution_logs.json") as source_archive:
                forged_logs.write_bytes(
                    _canonical_log_zip(
                        {
                            name: changed
                            if name == command["stdout"]
                            else source_archive.read(name)
                            for name in source_archive.namelist()
                        }
                    )
                )
            refuse_context(forged, forged_logs=forged_logs)
        # Fully recompute the inventory/context/log mapping, but reuse one
        # executable path for two distinct selected binary IDs: forbidden.
        collection_name = "nextest-inventory.json" if rail == "sdk" else "rust-inventory.json"
        inventory = json.loads(raw_paths[collection_name].read_bytes())
        binary_id, suite = next(iter(inventory["rust-suites"].items()))
        duplicate_id = binary_id + "-duplicate"
        duplicate = json.loads(json.dumps(suite))
        duplicate["binary-id"] = duplicate_id
        duplicate["binary-name"] += "-duplicate"
        inventory["rust-suites"][duplicate_id] = duplicate
        inventory["test-count"] += len(duplicate["testcases"])
        changed = json.dumps(inventory).encode()
        forged_raw = tmp_path / f"duplicate-{rail}-inventory.json"
        forged_raw.write_bytes(changed)
        forged = json.loads(json.dumps(context))
        forged["raw_evidence"][collection_name] = ev.digest(changed)
        duplicate_role = "nextest-" + ev.digest(duplicate_id.encode())
        original_role = "nextest-" + ev.digest(binary_id.encode())
        forged["binaries"][duplicate_role] = dict(forged["binaries"][original_role])
        duplicate_file = frozen_binary.parent / duplicate_role
        duplicate_file.write_bytes((frozen_binary.parent / original_role).read_bytes())
        command = next(row for row in forged["commands"] if row["name"] == "rust-collection")
        command["stdout_sha256"] = ev.digest(changed)
        forged_logs = tmp_path / f"duplicate-{rail}-collection.zip"
        with zipfile.ZipFile(receipt_dir / f"{rail}_execution_logs.json") as source_archive:
            forged_logs.write_bytes(
                _canonical_log_zip(
                    {
                        name: changed if name == command["stdout"] else source_archive.read(name)
                        for name in source_archive.namelist()
                    }
                )
            )
        refuse_context(forged, {"raw": {**raw_paths, collection_name: forged_raw}}, forged_logs)
        duplicate_file.unlink()
        for key, value in (
            ("PATH", "/forged/bin"),
            ("RUSTC", "/forged/rustc"),
            ("RUSTC_WRAPPER", "/forged/wrapper"),
            ("RUSTC_WORKSPACE_WRAPPER", "/forged/workspace-wrapper"),
            ("QUANTA_INDEX_SCCACHE", "1"),
        ):
            forged = json.loads(json.dumps(context))
            command = forged["commands"][0]
            command["environment"][key] = value
            command["environment_sha256"] = portable_proof._environment_digest(
                {**command["inherited_environment"], **command["environment"]}
            )
            refuse_context(forged)
        forged = json.loads(json.dumps(context))
        forged["commands"][0]["inherited_environment"]["PATH"] = None
        refuse_context(forged)
        if rail == "sdk":
            forged = json.loads(json.dumps(context))
            # A retired raw Just recipe cannot replace the bound SDK command list.
            legacy = dict(
                forged["commands"][0],
                name="sdk-recipe",
                argv=[context["tools"]["just"]["path"], "_retrieval-sdk-proof-raw", "/proof"],
            )
            forged["commands"] = [
                legacy,
                next(row for row in forged["commands"] if row["name"] == "metadata"),
            ]
            refuse_context(forged)
            for raw_name in ("nextest-inventory.json", "nextest.jsonl"):
                forged = json.loads(json.dumps(context))
                forged_raw = tmp_path / f"forged-{raw_name}"
                forged_raw.write_bytes(b"forged transcript")
                forged["raw_evidence"][raw_name] = pairrun.sha_file(forged_raw)
                refuse_context(forged, {"raw": {**raw_paths, raw_name: forged_raw}})
    if mutation == "argv":
        context["commands"][0]["argv"][1] = "different-command"
    elif mutation == "environment":
        context["commands"][0]["environment"]["CARGO_NET_OFFLINE"] = "false"
    elif mutation == "raw":
        context["raw_evidence"]["source-closure.json"] = "0" * 64
    elif mutation == "stdout_digest":
        context["commands"][0]["stdout_sha256"] = ev.digest(b"forged transcript")
    if mutation != "missing_role":
        context_path.write_text(json.dumps(context), encoding="utf-8")
    receipt_names = (
        ("contract_python_receipt", "contract_rust_receipt")
        if rail == "contract"
        else ("sdk_receipt",)
    )
    for name in receipt_names:
        receipt_path = receipt_dir / f"{name}.json"
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
        if mutation == "missing_role":
            receipt["input_evidence"] = [
                row for row in receipt["input_evidence"] if row["role"] != "execution-context"
            ]
        else:
            next(row for row in receipt["input_evidence"] if row["role"] == "execution-context")[
                "sha256"
            ] = pairrun.sha_file(context_path)
        receipt_path.write_text(json.dumps(receipt), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"][state] == "fail"


def test_qualified_verdict_separates_unverified_os_portability(tmp_path):
    st = _pair_stage(tmp_path, scope="qualified", receipts="full")
    verdict = _stage_verdict(st)
    for state in ("CONTRACT_GREEN", "SDK_PATH_GREEN"):
        assert verdict["states"][state] == "pass"
    assert verdict["os_portability"] == {
        "qualified": False,
        "reason": "execution_os_tool_identity_unverified",
    }


def test_freeze_receipts_copies_command_transcript_bytes(tmp_path):
    source = tmp_path / "source"
    source.mkdir()
    context = source / "execution-context.json"
    contents = _full_receipts(
        pairrun.git_head_sha(Path(__file__).resolve().parents[3]),
        ev.digest(b"quanta-runner-binary"),
        source / "binaries",
    )
    context.write_bytes(contents["contract_execution_context"])
    with zipfile.ZipFile(io.BytesIO(contents["contract_execution_logs"])) as archive:
        transcripts = {name: archive.read(name) for name in archive.namelist()}
    for name in pairrun.CONTEXT_COMMAND_NAMES["contract"]:
        for stream in ("stdout", "stderr"):
            filename = f"{name}.{stream}"
            payload = (
                transcripts[filename]
                if name in ("rust-collection", "rust-test")
                else f"{name}:{stream}".encode()
            )
            (source / filename).write_bytes(payload)
    stage = tmp_path / "stage"
    stage.mkdir()
    frozen = pairrun.freeze_receipts(
        {"receipts": {"contract_execution_context": str(context)}}, stage
    )
    with zipfile.ZipFile(frozen["contract_execution_logs"]) as archive:
        assert archive.read("python-test.stdout") == b"python-test:stdout"
    declared = json.loads(context.read_bytes())["binaries"]
    binary_root = stage / "receipts" / "contract-binaries"
    assert set(path.name for path in binary_root.iterdir()) == set(declared)
    for role, row in declared.items():
        assert pairrun.sha_file(binary_root / role) == row["sha256"]
        Path(row["path"]).write_bytes(b"source changed after freeze")
        assert pairrun.sha_file(binary_root / role) == row["sha256"]
    (source / "python-test.stdout").write_bytes(b"changed")
    with zipfile.ZipFile(frozen["contract_execution_logs"]) as archive:
        assert archive.read("python-test.stdout") == b"python-test:stdout"


@pytest.mark.parametrize("mutation", ["digest", "crc", "duplicate"])
def test_verdict_refuses_frozen_command_log_tampering(tmp_path, mutation):
    st = _pair_stage(tmp_path, receipts="full")
    assert _stage_verdict(st)["states"]["CONTRACT_GREEN"] == "pass"
    archive_path = st["stage"] / "receipts" / "contract_execution_logs.json"
    with zipfile.ZipFile(archive_path) as archive:
        members = {name: archive.read(name) for name in archive.namelist()}
    if mutation == "duplicate":
        with pytest.warns(UserWarning, match="Duplicate name"):
            with zipfile.ZipFile(archive_path, "a", compression=zipfile.ZIP_STORED) as archive:
                archive.writestr("python-test.stdout", b"")
    else:
        members["python-test.stdout"] = b"forged transcript"
        archive_path.write_bytes(_canonical_log_zip(members))
        if mutation == "crc":
            archive_path.write_bytes(
                archive_path.read_bytes().replace(b"forged transcript", b"forged transcripu")
            )
    verdict = _stage_verdict(st)
    assert verdict["states"]["CONTRACT_GREEN"] == "fail"
    reason = verdict["state_evidence"]["CONTRACT_GREEN"]["reason"]
    assert (
        "frozen command output digest mismatch"
        if mutation == "digest"
        else "Bad CRC-32"
        if mutation == "crc"
        else "archive central directory exceeds limits"
    ) in reason


@pytest.mark.parametrize(
    ("receipt_name", "state"),
    [
        ("contract_python_receipt", "CONTRACT_GREEN"),
        ("contract_rust_receipt", "CONTRACT_GREEN"),
        ("sdk_receipt", "SDK_PATH_GREEN"),
    ],
)
def test_verdict_refuses_receipt_test_event_count_drift(tmp_path, receipt_name, state):
    st = _pair_stage(tmp_path, receipts="full")
    assert _stage_verdict(st)["states"][state] == "pass"
    receipt_path = st["stage"] / "receipts" / f"{receipt_name}.json"
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    receipt["test_event_count"] += 1
    receipt_path.write_text(json.dumps(receipt), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"][state] == "fail"
    assert "test_event_count" in verdict["state_evidence"][state]["reason"]


def test_verdict_lying_manifest_refused(tmp_path):
    st = _pair_stage(tmp_path, claims={"speed": True})
    # Fake contract evidence without artifacts fails instead of passing.
    _rewrite_manifest(
        st,
        lambda m: m["evidence"].update(
            {
                "contract_suites": {
                    "python": {
                        "test_result_digest": "a" * 64,
                        "raw_evidence_digest": "c" * 64,
                        "inventory_digest": "e" * 64,
                    },
                    "rust": {
                        "test_result_digest": "b" * 64,
                        "raw_evidence_digest": "d" * 64,
                        "inventory_digest": "f" * 64,
                    },
                }
            }
        ),
    )
    verdict = _stage_verdict(st)
    assert verdict["states"]["CONTRACT_GREEN"] == "fail"
    assert verdict["failure_class"] == "scoring"
    # Inflated perf numbers are re-derived, never trusted.
    st = _pair_stage(tmp_path / "perf", scope="qualified", claims={"speed": True})
    _rewrite_manifest(st, lambda m: m["evidence"]["perf"].update({"observations_floor": 9999}))
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == "perf_floor_mismatch"
    # A swapped binary pin fails the pair binding.
    st = _pair_stage(tmp_path / "binary")
    _rewrite_manifest(st, lambda m: m["provenance"]["quanta"].update({"binary_digest": "0" * 64}))
    verdict = _stage_verdict(st)
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert verdict["state_evidence"]["PAIR_VALID"]["reason"] == "binary_digest_mismatch"


def test_verdict_stale_and_swapped_receipts(tmp_path):
    st = _pair_stage(tmp_path, receipts="full")
    results_path = st["stage"] / "receipts" / "contract_python_results.json"
    tampered = json.loads(results_path.read_text(encoding="utf-8"))
    tampered["passed"] = 104
    results_path.write_text(json.dumps(tampered), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["CONTRACT_GREEN"] == "fail"
    assert "receipt digest mismatch" in verdict["state_evidence"]["CONTRACT_GREEN"]["reason"]

    st = _pair_stage(tmp_path / "closure", receipts="full")
    rust_receipt_path = st["stage"] / "receipts" / "contract_rust_receipt.json"
    rust_receipt = json.loads(rust_receipt_path.read_text(encoding="utf-8"))
    closure = rust_receipt["source_closure"]
    closure["files"][0]["sha256"] = _fake_sha("different-source")
    closure_core = {
        key: closure[key] for key in ("schema_version", "profile", "revision", "roots", "files")
    }
    closure["digest"] = ev.digest(ev.canonical(closure_core))
    rust_receipt_path.write_text(json.dumps(rust_receipt), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["CONTRACT_GREEN"] == "fail"
    assert (
        "execution context source closure mismatch"
        in verdict["state_evidence"]["CONTRACT_GREEN"]["reason"]
    )
    st = _pair_stage(tmp_path / "swap", receipts="full")
    rust_bytes = (st["stage"] / "receipts" / "contract_rust_results.json").read_bytes()
    (st["stage"] / "receipts" / "contract_python_results.json").write_bytes(rust_bytes)
    verdict = _stage_verdict(st)
    assert verdict["states"]["CONTRACT_GREEN"] == "fail"


def test_verdict_garbage_test_artifact(tmp_path):
    st = _pair_stage(tmp_path, receipts="full")
    raw_path = st["stage"] / "receipts" / "contract_python_raw.json"
    raw_path.write_text(
        '<testsuite tests="105" failures="1" errors="0" skipped="0" />\n',
        encoding="utf-8",
    )
    raw_digest = ev.digest(raw_path.read_bytes())
    receipt_path = st["stage"] / "receipts" / "contract_python_receipt.json"
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    next(entry for entry in receipt["input_evidence"] if entry["role"] == "pytest-junit")[
        "sha256"
    ] = raw_digest
    receipt_path.write_text(json.dumps(receipt), encoding="utf-8")
    _rewrite_manifest(
        st,
        lambda manifest: (
            manifest["evidence"]["contract_suites"]["python"].update(
                raw_evidence_digest=raw_digest
            ),
        ),
    )
    verdict = _stage_verdict(st)
    assert verdict["states"]["CONTRACT_GREEN"] == "fail"
    assert (
        "execution context raw evidence digest mismatch"
        in verdict["state_evidence"]["CONTRACT_GREEN"]["reason"]
    )


def test_verdict_rejects_coordinated_partial_inventory_and_receipt_rebind(tmp_path):
    st = _pair_stage(tmp_path, receipts="full")
    assert _stage_verdict(st)["states"]["CONTRACT_GREEN"] == "pass"
    manifest = json.loads(st["manifest_path"].read_text())
    paths = {
        key: st["stage"] / manifest["artifacts"][f"contract_python_{key}"]
        for key in ("raw", "inventory", "results", "receipt")
    }
    inventory = json.loads(paths["inventory"].read_text())
    inventory["tests"] = inventory["tests"][:1]
    paths["inventory"].write_text(json.dumps(inventory), encoding="utf-8")
    identity = inventory["tests"][0]
    classname = "tools.ci.tests.test_retrieval_benchmark"
    paths["raw"].write_text(
        f'<testsuite tests="1" failures="0" errors="0" skipped="0">'
        f'<testcase classname="{classname}" '
        f'name="{html.escape(identity[len(classname) + 1 :], quote=True)}"/>'
        "</testsuite>",
        encoding="utf-8",
    )
    results = _counts_results(portable_proof.PYTHON_COMMAND, 1, 1, 1, 0)
    paths["results"].write_text(json.dumps(results), encoding="utf-8")
    receipt = json.loads(paths["receipt"].read_text())
    receipt["evidence_sha256"] = pairrun.sha_file(paths["results"])
    receipt["test_event_count"] = 1
    for entry in receipt["input_evidence"]:
        if entry["role"] == "pytest-junit":
            entry["sha256"] = pairrun.sha_file(paths["raw"])
        elif entry["role"] == "pytest-inventory":
            entry["sha256"] = pairrun.sha_file(paths["inventory"])
    paths["receipt"].write_text(json.dumps(receipt), encoding="utf-8")
    manifest["evidence"]["contract_suites"]["python"].update(
        test_result_digest=pairrun.sha_file(paths["results"]),
        raw_evidence_digest=pairrun.sha_file(paths["raw"]),
        inventory_digest=pairrun.sha_file(paths["inventory"]),
    )
    st["manifest_path"].write_text(json.dumps(manifest), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["CONTRACT_GREEN"] == "fail"
    assert (
        "execution context raw evidence digest mismatch"
        in verdict["state_evidence"]["CONTRACT_GREEN"]["reason"]
    )


def test_verdict_mapping_lies(tmp_path):
    st = _pair_stage(tmp_path)
    mapping_path = st["stage"] / "rep-00" / "semble" / "mapping-proof.json"
    mapping = json.loads(mapping_path.read_text(encoding="utf-8"))
    mapping["skipped"] = [{"path": "evil.txt", "reason": "policy"}]
    mapping_path.write_text(json.dumps(mapping), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert "T00" in verdict["missing_t_ids"]
    st = _pair_stage(tmp_path / "corpus")
    corpus_path = st["stage"] / "corpus-manifest.json"
    corpus = json.loads(corpus_path.read_text(encoding="utf-8"))
    corpus["files"][0]["file_sha256"] = "f" * 64
    corpus_path.write_text(json.dumps(corpus), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert verdict["failure_class"] == "corpus_mismatch"


def test_verdict_report_tamper(tmp_path):
    st = _pair_stage(tmp_path)
    report_path = st["stage"] / "report-hybrid-vs-lexical-whole_file.json"
    report = json.loads(report_path.read_text(encoding="utf-8"))
    report["rank_metrics"]["comparison"]["primary_delta"] = 1.0
    report_path.write_text(json.dumps(report), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert verdict["state_evidence"]["PAIR_VALID"]["reason"] == "report_not_reproducible"
    assert "T13" in verdict["missing_t_ids"]


def test_verdict_matrix_tamper_and_native_disagreement(tmp_path, monkeypatch):
    _allow_minimal_speed_fixture(monkeypatch)
    st = _pair_stage(tmp_path)
    matrix_path = st["stage"] / "latency-matrix.json"
    matrix = json.loads(matrix_path.read_text(encoding="utf-8"))
    matrix["observations_floor"] = 9999
    matrix_path.write_text(json.dumps(matrix), encoding="utf-8")
    verdict = _stage_verdict(st)
    # Without a speed claim the matrix is not pair evidence.
    assert verdict["states"]["PAIR_VALID"] == "pass"
    assert verdict["states"]["PERF_QUALIFIED"] == "not_applicable"
    st = _pair_stage(tmp_path / "speed", scope="qualified", claims={"speed": True})
    matrix_path = st["stage"] / "latency-matrix.json"
    matrix = json.loads(matrix_path.read_text(encoding="utf-8"))
    matrix["observations_floor"] = 9999
    matrix_path.write_text(json.dumps(matrix), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == "matrix_not_reproducible"
    st = _pair_stage(tmp_path / "warm", scope="qualified", claims={"speed": True})
    phase_path = st["stage"] / "rep-00" / "semble" / "phase-metrics.json"
    phase = json.loads(phase_path.read_text(encoding="utf-8"))
    first_task = next(iter(phase["warm_latencies_ms"]["hybrid"]))
    phase["warm_latencies_ms"]["hybrid"][first_task][0] = 9.9
    phase_path.write_text(json.dumps(phase), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == ("phase_boundaries_incomplete")


def test_verdict_perf_frontier_and_gates(tmp_path, monkeypatch):
    st = _pair_stage(tmp_path, claims={"speed": True})
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "not_applicable"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == "exploratory_only"
    st = _pair_stage(tmp_path / "qualified", scope="qualified", claims={"speed": True})
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == "observations_floor_unmet"
    monkeypatch.setattr(pairrun, "PILOT_OBSERVATIONS_FLOOR", 2)
    monkeypatch.setattr(pairrun, "FRESH_ROOTS_FLOOR", 1)
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert "at least 20 frozen tasks" in verdict["state_evidence"]["PERF_QUALIFIED"]["reason"]
    _allow_minimal_speed_fixture(monkeypatch)
    # Null timings fail a speed claim once floors hold.
    st = _pair_stage(tmp_path / "nulls", scope="qualified", claims={"speed": True})
    record_path = st["stage"] / "rep-00" / "quanta" / "strategy-00-whole_file" / "record.json"
    record = json.loads(record_path.read_text(encoding="utf-8"))
    record["results"][0]["timings"] = {"query_latency_ms": None}
    record_path.write_text(json.dumps(record), encoding="utf-8")
    record_digest = ev.digest(record_path.read_bytes())
    phase_path = record_path.parent / "phase-metrics.json"
    phase = json.loads(phase_path.read_text(encoding="utf-8"))
    phase["record_sha256"] = record_digest
    phase_path.write_text(json.dumps(phase), encoding="utf-8")
    resource_path = record_path.parent / "resource-metrics.json"
    resource = json.loads(resource_path.read_text(encoding="utf-8"))
    resource["subject_sha256"] = record_digest
    resource_path.write_text(json.dumps(resource), encoding="utf-8")
    matrix = pairrun.build_latency_matrix(st["rep_layouts"])
    (st["stage"] / "latency-matrix.json").write_text(json.dumps(matrix), encoding="utf-8")
    quanta_manifest_path = st["stage"] / "rep-00" / "quanta" / "quanta-manifest.json"
    quanta_manifest = json.loads(quanta_manifest_path.read_text(encoding="utf-8"))
    quanta_manifest["runs"][0].update(
        record_digest=record_digest,
        phase_metrics_digest=ev.digest(phase_path.read_bytes()),
        resource_metrics_digest=ev.digest(resource_path.read_bytes()),
    )
    quanta_manifest_path.write_text(json.dumps(quanta_manifest), encoding="utf-8")

    def rebind_null_timing_fixture(manifest):
        manifest["artifacts"]["phase_metrics_digests"][
            phase_path.relative_to(st["stage"]).as_posix()
        ] = pairrun.sha_file(phase_path)
        manifest["evidence"]["perf"].update(
            {
                "observations_floor": matrix["observations_floor"],
                "fresh_roots": matrix["fresh_roots"],
            }
        )

    _rewrite_manifest(
        st,
        rebind_null_timing_fixture,
    )
    # Nulling a sample drops the floor to 1; hold floors there to isolate
    # the null-timing gate.
    monkeypatch.setattr(pairrun, "PILOT_OBSERVATIONS_FLOOR", 1)
    verdict = _stage_verdict(st)
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == "null_timings_on_speed_claim"
    monkeypatch.setattr(pairrun, "PILOT_OBSERVATIONS_FLOOR", 2)
    # Contended hosts fail with host class; the digests stay consistent.
    st = _pair_stage(tmp_path / "host", scope="qualified", claims={"speed": True})
    busy = {"concurrent_processes": {"cargo": [123]}, "contention_override": False}
    (st["stage"] / "host-start.json").write_text(json.dumps(busy), encoding="utf-8")

    def rebind(manifest):
        start = json.loads((st["stage"] / "host-start.json").read_text(encoding="utf-8"))
        end = json.loads((st["stage"] / "host-end.json").read_text(encoding="utf-8"))
        manifest["host"] = {
            **manifest["host"],
            "start_digest": ev.digest((st["stage"] / "host-start.json").read_bytes()),
            "end_digest": ev.digest((st["stage"] / "host-end.json").read_bytes()),
            "cache_regime": manifest["host"]["cache_regime"],
        }
        manifest["provenance"]["host"]["check_record_digest"] = ev.digest(
            ev.canonical({"start": start, "end": end})
        )

    _rewrite_manifest(st, rebind)
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == "host_contended"
    assert verdict["failure_class"] == "host"


@pytest.mark.parametrize("fault", ["missing", "tamper", "middle_busy", "gap"])
def test_qualified_speed_requires_bound_host_timeline(tmp_path, monkeypatch, fault):
    _allow_minimal_speed_fixture(monkeypatch)
    st = _pair_stage(tmp_path, scope="qualified", claims={"speed": True})
    path = st["stage"] / "host-timeline.json"
    if fault == "missing":

        def remove_binding(manifest):
            manifest["host"].pop("timeline_digest")
            manifest["artifacts"].pop("host_timeline")
            manifest["artifacts"].pop("host_timeline_raw")

        _rewrite_manifest(st, remove_binding)
    else:
        timeline = json.loads(path.read_text())
        if fault in ("tamper", "middle_busy"):
            timeline["samples"][1]["probe"]["concurrent_processes"] = {"rustc": [123]}
        else:
            timeline["finished_ns"] = 50_000_000_100
        path.write_text(json.dumps(timeline))
        if fault != "tamper":
            _rewrite_manifest(
                st, lambda manifest: manifest["host"].update(timeline_digest=pairrun.sha_file(path))
            )
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert "host_timeline" in verdict["state_evidence"]["PERF_QUALIFIED"]["reason"]
    if fault != "tamper":
        assert verdict["states"]["PAIR_VALID"] == "pass"


def test_host_timeline_replays_complete_bound_monitor(tmp_path):
    st = _pair_stage(tmp_path)
    timeline = json.loads((st["stage"] / "host-timeline.json").read_text())
    profile = json.loads((st["stage"] / "host-profile.json").read_text())
    pairrun.validate_host_timeline(timeline, profile)
    pairrun.validate_host_timeline_monitor(timeline, st["stage"] / "host-timeline.jsonl")
    changed = copy.deepcopy(timeline)
    changed["reservation_id"] = "e" * 32
    with pytest.raises(pairrun.host_monitor.EvidenceError, match="reservation"):
        pairrun.validate_host_timeline_monitor(changed, st["stage"] / "host-timeline.jsonl")


@pytest.mark.parametrize("fault", ["digest", "host", "count", "foreign_rust", "clock"])
def test_host_timeline_monitor_rejects_independent_binding_faults(tmp_path, fault):
    st = _pair_stage(tmp_path)
    timeline = json.loads((st["stage"] / "host-timeline.json").read_text())
    raw_path = st["stage"] / "host-timeline.jsonl"
    rows = [json.loads(line) for line in raw_path.read_text().splitlines()]
    if fault == "host":
        rows[0]["host"]["cpu_count"] = 16
    elif fault == "count":
        del timeline["samples"][1]
    elif fault == "foreign_rust":
        rows[2]["facts"]["foreign_rust"] = [
            {"pid": 123, "ppid": 1, "command_sha256": "sha256:" + "e" * 64}
        ]
    elif fault == "clock":
        timeline["samples"][1]["finished_ns"] += 1
    raw_path.write_text("".join(json.dumps(row) + "\n" for row in rows))
    if fault != "digest":
        timeline["monitor_sha256"] = pairrun.sha_file(raw_path)
    else:
        timeline["monitor_sha256"] = "0" * 64
    with pytest.raises((pairrun.RunError, pairrun.host_monitor.EvidenceError)):
        pairrun.validate_host_timeline_monitor(timeline, raw_path)


def test_qualified_speed_verdict_rejects_incomplete_response_timer_boundary(tmp_path):
    st = _pair_stage(
        tmp_path,
        repetitions=5,
        qualified_speed_sample=True,
        scope="qualified",
        claims={"speed": True},
    )
    matrix = pairrun.read_json(st["stage"] / "latency-matrix.json")
    assert matrix["fresh_roots"] == 5
    assert matrix["observations_floor"] == 1_000
    verdict = _stage_verdict(st)
    # Even a complete observation protocol cannot turn SDK/IPC latency and
    # library dispatch latency into equivalent completed-response samples.
    assert verdict["states"]["PAIR_VALID"] == "pass"
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert verdict["states"]["QUALITY_DELTA"] == "not_applicable"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == (
        "completed_response_timing_unverified: completed-response timing contract is missing or malformed"
    )
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["proof_digest"] is None


def test_qualified_speed_replay_rejects_unalternated_system_order(tmp_path):
    st = _pair_stage(
        tmp_path,
        repetitions=5,
        qualified_speed_sample=True,
        scope="qualified",
        claims={"speed": True},
        alternate_system_order=False,
    )
    assert all(layout["order"] == ["quanta", "semble"] for layout in st["rep_layouts"])
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == ("measurement_order_unverified")


def test_qualified_speed_entry_rejects_disabled_order_alternation():
    spec = {
        "scope": "qualified",
        "admission": {},
        "claims": {"speed": True},
        "alternate_order": False,
    }
    if sys.platform == "linux":
        spec.update(blinding="isolated", linux_cgroup_parent="/sys/fs/cgroup")
    with pytest.raises(pairrun.RunError, match="alternating system order"):
        pairrun.run_pair(spec)


def test_verdict_host_profile_fingerprint_is_enforced(tmp_path, monkeypatch):
    _allow_minimal_speed_fixture(monkeypatch)
    st = _pair_stage(tmp_path, scope="qualified", claims={"speed": True})
    profile_path = st["stage"] / "host-profile.json"
    profile = json.loads(profile_path.read_text(encoding="utf-8"))
    profile["fingerprint"]["cpu_count"] += 1
    profile_path.write_text(json.dumps(profile), encoding="utf-8")
    _rewrite_manifest(
        st,
        lambda manifest: manifest["provenance"]["host"].update(
            profile_digest=pairrun.sha_file(profile_path)
        ),
    )
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"].startswith("admission_unverified:")
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert verdict["state_evidence"]["PAIR_VALID"]["reason"] == ("protocol_lock_host_profile_drift")
    assert verdict["failure_class"] == "provenance"


def test_verdict_rejects_forged_phase_and_process_tree_resources(tmp_path, monkeypatch):
    _allow_minimal_speed_fixture(monkeypatch)

    quanta_stage = _pair_stage(
        tmp_path / "quanta-resource", scope="qualified", claims={"speed": True}
    )
    quanta_path = (
        quanta_stage["stage"]
        / "rep-00"
        / "quanta"
        / "strategy-00-whole_file"
        / "resource-metrics.json"
    )
    quanta_resource = json.loads(quanta_path.read_text(encoding="utf-8"))
    quanta_resource.update(complete=False, error="malformed ps process row")
    quanta_path.write_text(json.dumps(quanta_resource), encoding="utf-8")
    quanta_verdict = _stage_verdict(quanta_stage)
    assert quanta_verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == (
        "resource_accounting_incomplete"
    )

    resource_stage = _pair_stage(tmp_path / "resource", scope="qualified", claims={"speed": True})
    resource_path = resource_stage["stage"] / "rep-00" / "semble-resource-metrics.json"
    resource = json.loads(resource_path.read_text(encoding="utf-8"))
    resource.update(complete=False, error="sampler lost process tree")
    resource_path.write_text(json.dumps(resource), encoding="utf-8")
    verdict = _stage_verdict(resource_stage)
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == (
        "resource_accounting_incomplete"
    )

    phase_stage = _pair_stage(tmp_path / "phase", scope="qualified", claims={"speed": True})
    phase_path = (
        phase_stage["stage"] / "rep-00" / "quanta" / "strategy-00-whole_file" / "phase-metrics.json"
    )
    phase = json.loads(phase_path.read_text(encoding="utf-8"))
    phase["total_ms"] += 1.0
    phase_path.write_text(json.dumps(phase), encoding="utf-8")
    verdict = _stage_verdict(phase_stage)
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == ("phase_boundaries_incomplete")
    schedule_stage = _pair_stage(tmp_path / "schedule", scope="qualified", claims={"speed": True})
    schedule_path = schedule_stage["stage"] / "rep-00" / "semble" / "phase-metrics.json"
    schedule = json.loads(schedule_path.read_text(encoding="utf-8"))
    schedule["query_schedule"].reverse()
    schedule_path.write_text(json.dumps(schedule), encoding="utf-8")
    verdict = _stage_verdict(schedule_stage)
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == ("phase_boundaries_incomplete")
    memory_stage = _pair_stage(tmp_path / "memory", scope="qualified", claims={"speed": True})
    native_path = memory_stage["stage"] / "rep-00" / "semble" / "native.json"
    native = json.loads(native_path.read_text(encoding="utf-8"))
    native["stats"]["index_resident_bytes"] += 1
    native_path.write_text(json.dumps(native), encoding="utf-8")
    verdict = _stage_verdict(memory_stage)
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == (
        "resource_accounting_incomplete"
    )
    # T12: a speed claim without a declared cache regime fails once every
    # earlier gate holds; the declared stage reaches the phase frontier.
    with pytest.raises(pairrun.RunError, match="qualified admission cannot use"):
        _pair_stage(
            tmp_path / "cache", scope="qualified", claims={"speed": True}, cache_regime="undeclared"
        )


def test_qualified_verdict_rejects_handcrafted_linux_v2_on_wrong_host(tmp_path, monkeypatch):
    from tools.benchmark.retrieval.test_linux_resource_integration import _valid_qualified_v2

    _allow_minimal_speed_fixture(monkeypatch)
    st = _pair_stage(tmp_path, scope="qualified", claims={"speed": True})
    parent = tmp_path / "delegated"
    parent.mkdir()
    resource_path = st["stage"] / "rep-00" / "semble-resource-metrics.json"
    original = json.loads(resource_path.read_text(encoding="utf-8"))
    forged = _valid_qualified_v2(parent)
    forged["subject_sha256"] = original["subject_sha256"]
    forged["storage"] = original["storage"]
    resource_path.write_text(json.dumps(forged), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == (
        "resource_accounting_incomplete"
    )


def test_verdict_t15_t16_conditionals(tmp_path):
    st = _pair_stage(tmp_path, claims={"same_model": True})
    verdict = _stage_verdict(st)
    assert "T15" in verdict["missing_t_ids"]
    assert verdict["failure_class"] == "model"
    with pytest.raises(pairrun.RunError, match="must hold exactly"):
        _pair_stage(
            tmp_path / "summary-only",
            claims={"same_model": True},
            receipts={"model_parity_results": _parity_results("parity-cmd")},
        )
    parity_record = _bound_conditional_results("parity-cmd", "model_vectors")
    assert (
        pairrun._validate_parity_results_shape(parity_record, "model parity", "model_vectors")
        == parity_record
    )
    pairrun._require_conditional_identity(parity_record, **parity_record["identity"])
    for key, forged_value in (
        ("source_revision", "0" * 40),
        ("repository_commit", "0" * 40),
        ("model_sha256", "0" * 64),
        ("dependency_sha256", "0" * 64),
    ):
        expected = dict(parity_record["identity"], **{key: forged_value})
        with pytest.raises(pairrun.RunError, match="frozen source/model/dependency"):
            pairrun._require_conditional_identity(parity_record, **expected)
    for mutate, match in (
        (lambda row: row.pop("raw_proof"), "must hold exactly"),
        (lambda row: row["execution_receipt"].update(raw_sha256="0" * 64), "receipt does not bind"),
        (lambda row: row["identity"].update(model_sha256="0" * 64), "receipt does not bind"),
        (lambda row: row.update(passed=0), "summary differs from raw"),
    ):
        forged = json.loads(json.dumps(parity_record))
        mutate(forged)
        with pytest.raises(pairrun.RunError, match=match):
            pairrun._validate_parity_results_shape(forged, "model parity", "model_vectors")
    parity = {"model_parity_results": parity_record}
    st = _pair_stage(tmp_path / "parity", claims={"same_model": True}, receipts=parity)
    verdict = _stage_verdict(st)
    assert "T15" in verdict["missing_t_ids"]
    assert verdict["failure_class"] == "model"
    incremental_record = _bound_conditional_results("incr-cmd", "incremental_rows", matches=False)
    assert (
        pairrun._validate_parity_results_shape(
            incremental_record, "incremental", "incremental_rows"
        )
        == incremental_record
    )
    duplicate_rows = json.loads(json.dumps(incremental_record))
    duplicate_rows["raw_proof"]["rows"][0]["fresh_row_ids"] = ["row-1", "row-1"]
    with pytest.raises(pairrun.RunError, match="sorted unique row IDs"):
        pairrun._validate_parity_results_shape(duplicate_rows, "incremental", "incremental_rows")
    bad = {"incremental_results": incremental_record}
    st = _pair_stage(tmp_path / "incr", claims={"incremental": True}, receipts=bad)
    verdict = _stage_verdict(st)
    assert "T16" in verdict["missing_t_ids"]
    assert verdict["failure_class"] == "infra"


def test_verdict_quality_gates(tmp_path, monkeypatch):
    monkeypatch.setattr(ev, "MIN_CI_SAMPLE", 2)
    st = _pair_stage(tmp_path, claims={"quality": True})
    verdict = _stage_verdict(st)
    assert verdict["states"]["QUALITY_DELTA"] == "not_applicable"
    assert "scope:exploratory_only" in verdict["not_applicable_t_ids"]
    st = _pair_stage(
        tmp_path / "iso", blinding="isolated", scope="qualified", claims={"quality": True}
    )
    verdict = _stage_verdict(st)
    assert verdict["states"]["QUALITY_DELTA"] == "fail"
    assert verdict["state_evidence"]["QUALITY_DELTA"]["reason"] == (
        "binary_build_source_unattested"
    )
    assert verdict["failure_class"] == "provenance"
    proof_path = st["stage"] / "isolation-proof.json"
    proof = json.loads(proof_path.read_text(encoding="utf-8"))
    proof["policy_sha256"] = _fake_sha("forged-profile")
    proof_path.write_text(json.dumps(proof), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["QUALITY_DELTA"] == "fail"
    assert verdict["state_evidence"]["QUALITY_DELTA"]["reason"].startswith(
        "isolation_proof_unverified:"
    )
    assert verdict["failure_class"] == "blinding"
    st = _pair_stage(
        tmp_path / "ungraded",
        blinding="isolated",
        scope="qualified",
        claims={"quality": True},
        graded=False,
    )
    verdict = _stage_verdict(st)
    assert verdict["states"]["QUALITY_DELTA"] == "fail"
    assert verdict["state_evidence"]["QUALITY_DELTA"]["reason"] == "reports_ungraded"
    assert verdict["failure_class"] == "scoring"
    # A manifest cannot upgrade attested records to isolated quality proof.
    st = _pair_stage(tmp_path / "spoof", scope="qualified", claims={"quality": True})
    _rewrite_manifest(st, lambda m: m.update(blinding="isolated"))
    with pytest.raises(pairrun.RunError, match="current tagged backend proof binding"):
        _stage_verdict(st)
    # A lexical-only capture must not claim or require an embedding model.
    # The configured unused encoder is not evidence of model execution.
    st = _pair_stage(
        tmp_path / "hashdev",
        blinding="isolated",
        scope="qualified",
        claims={"quality": True},
        embedder="hash-dev",
    )
    assert st["manifest"]["provenance"]["quanta"]["embedder"] == "hash-dev"
    verdict = _stage_verdict(st)
    assert verdict["states"]["QUALITY_DELTA"] == "fail"
    assert verdict["state_evidence"]["QUALITY_DELTA"]["reason"] == (
        "binary_build_source_unattested"
    )
    assert verdict["failure_class"] == "provenance"
    assert verdict["provenance"]["quanta"]["embedder"] == "hash-dev"


def test_context_density_rubric_independent_oracles(tmp_path, monkeypatch):
    gold = [{"path": "a.txt", "start_byte": 100, "end_byte": 110, "grade": 3}]
    exact = [{"path": "a.txt", "start_byte": 100, "end_byte": 110}]
    whole_file = [{"path": "a.txt", "start_byte": 0, "end_byte": 1_000_000}]
    partial = [{"path": "a.txt", "start_byte": 100, "end_byte": 105}]
    assert ev.ndcg_at_k(exact, gold, 10) == 1.0
    assert ev.ndcg_at_k(whole_file, gold, 10) == pytest.approx(10 / 1_000_000)
    assert ev.ndcg_at_k(partial, gold, 10) == 0.0
    assert ev.ndcg_at_k(exact + whole_file, gold, 10) == 1.0

    two_gold = gold + [{"path": "a.txt", "start_byte": 120, "end_byte": 130, "grade": 2}]
    one_candidate = [{"path": "a.txt", "start_byte": 100, "end_byte": 130}]
    ideal = 7 + 3 / math.log2(3)
    assert ev.ndcg_at_k(one_candidate, two_gold, 10) == pytest.approx((7 * 20 / 30) / ideal)

    overlapping_gold = gold + [{"path": "a.txt", "start_byte": 105, "end_byte": 115, "grade": 2}]
    overlap_candidate = [{"path": "a.txt", "start_byte": 100, "end_byte": 115}]
    assert ev.ndcg_at_k(overlap_candidate, overlapping_gold, 10) == pytest.approx(7 / ideal)

    monkeypatch.setattr(ev, "MIN_CI_SAMPLE", 2)
    st = _pair_stage(tmp_path, blinding="isolated", scope="qualified", claims={"quality": True})
    verdict = _stage_verdict(st)
    assert verdict["states"]["QUALITY_DELTA"] == "fail"
    assert verdict["state_evidence"]["QUALITY_DELTA"]["reason"] == (
        "binary_build_source_unattested"
    )
    assert verdict["failure_class"] == "provenance"


@pytest.mark.parametrize("claim", ["quality", "speed"])
def test_qualified_claims_require_pair_contract_and_sdk_states(tmp_path, monkeypatch, claim):
    monkeypatch.setattr(ev, "MIN_CI_SAMPLE", 2)
    kwargs = (
        {"blinding": "isolated"}
        if claim == "quality"
        else {"repetitions": 5, "qualified_speed_sample": True}
    )
    st = _pair_stage(tmp_path, scope="qualified", claims={claim: True}, **kwargs)
    if claim == "speed":
        from tools.ci.tests.test_completed_response_timing import _add_timing

        _add_timing(st)
    target = "QUALITY_DELTA" if claim == "quality" else "PERF_QUALIFIED"
    baseline = _stage_verdict(st)
    assert baseline["states"]["PAIR_VALID"] == "pass"
    assert baseline["states"]["CONTRACT_GREEN"] == "pass"
    assert baseline["states"]["SDK_PATH_GREEN"] == "pass"
    assert baseline["states"][target] == "fail", baseline["state_evidence"][target]
    assert baseline["state_evidence"][target]["reason"] == "binary_build_source_unattested"

    manifest = st["manifest"]
    mutations = (
        (st["stage"] / "protocol-lock.json", "PAIR_VALID"),
        (
            st["stage"] / manifest["artifacts"]["contract_python_raw"],
            "CONTRACT_GREEN",
        ),
        (st["stage"] / manifest["artifacts"]["sdk_nextest_raw"], "SDK_PATH_GREEN"),
    )
    for path, failed_state in mutations:
        original = path.read_bytes()
        try:
            if failed_state == "PAIR_VALID":
                lock = json.loads(original)
                lock["top_k"] += 1
                path.write_text(json.dumps(lock), encoding="utf-8")
            else:
                path.write_bytes(b"invalid terminal evidence")
            verdict = _stage_verdict(st)
            assert verdict["states"][failed_state] == "fail"
            assert verdict["states"][target] == "fail"
        finally:
            path.write_bytes(original)

    _rewrite_manifest(
        st,
        lambda manifest: manifest["provenance"]["quanta"].pop("binary_build_source_revision"),
    )
    legacy = _stage_verdict(st)
    assert legacy["states"]["PAIR_VALID"] == "pass"
    assert legacy["states"][target] == "fail"
    assert legacy["state_evidence"][target]["reason"] == "binary_build_source_unattested"


def test_validated_report_penalizes_whole_file_containing_exact_gold(tmp_path):
    repo, suite, run, suite_path, runner_path, files = fixture_v3(tmp_path)
    exact_report = ev.evaluate(
        *record_v3(repo, suite, run, suite_path, runner_path), "lexical", "hybrid"
    )
    file_bytes = files["a.txt"]
    file_sha, block_sha, tokens = _span_meta(file_bytes, 1, 4)
    start_byte, end_byte = _byte_span(file_bytes, 1, 4)
    run["results"][1]["candidates"][0] = {
        "path": "a.txt",
        "start_byte": start_byte,
        "end_byte": end_byte,
        "start_line": 1,
        "end_line": 4,
        "file_sha256": file_sha,
        "block_sha256": block_sha,
        "tokens": tokens,
        "rank": 1,
    }
    whole_report = ev.evaluate(
        *record_v3(repo, suite, run, suite_path, runner_path), "lexical", "hybrid"
    )
    exact = exact_report["rank_metrics"]["routes"]["hybrid"]["chunk"]["ndcg_at_10"]
    whole = whole_report["rank_metrics"]["routes"]["hybrid"]["chunk"]["ndcg_at_10"]
    assert whole < exact
    assert whole_report["rank_metric_version"] == "rb-rank-context-density-first-coverage"


def test_qualified_admission_is_reverified_after_capture(tmp_path):
    st = _pair_stage(tmp_path, blinding="isolated", scope="qualified", claims={"quality": True})
    admission_manifest = st["stage"] / "admission" / "admission.json"
    jsonschema.validate(
        json.loads(admission_manifest.read_text(encoding="utf-8")),
        _load_schema("admission.schema.json"),
    )
    annotation = st["stage"] / "admission" / "annotation-1-receipt.json"
    annotation.write_text('{"annotator":"post-capture-tamper"}', encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["QUALITY_DELTA"] == "fail"
    assert verdict["state_evidence"]["QUALITY_DELTA"]["reason"].startswith("admission_unverified:")
    assert verdict["failure_class"] == "admission"


def test_repository_disjoint_admission_contract_uses_split_paths(tmp_path):
    st = _pair_stage(tmp_path, blinding="isolated", scope="qualified", claims={"quality": True})
    local = json.loads((st["stage"] / "admission" / "admission.json").read_text())
    disjoint = copy.deepcopy(local)
    disjoint["schema_version"] = 3
    disjoint.pop("development_suite_sha256")
    disjoint.pop("experiment_custody_sha256")
    disjoint["decision_policy_sha256"] = _fake_sha("decision-policy")
    disjoint["repository_disjoint"] = {
        "repository": "holdout",
        "release_digest": "sha256:" + _fake_sha("release"),
        "split_manifest_sha256": _fake_sha("split"),
        "split_releases_sha256": _fake_sha("releases"),
    }
    jsonschema.validate(disjoint, _load_schema("admission.schema.json"))
    assert pairrun.validate_admission_manifest(disjoint) == disjoint
    missing_policy = copy.deepcopy(disjoint)
    missing_policy.pop("decision_policy_sha256")
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(missing_policy, _load_schema("admission.schema.json"))
    with pytest.raises(pairrun.RunError, match="frozen decision policy"):
        pairrun.validate_admission_manifest(missing_policy)

    spec = copy.deepcopy(st["spec"])
    spec["admission"].pop("experiment_custody")
    spec["admission"].pop("development_suite")
    spec["admission"].update(split_manifest="split.json", split_releases="releases.json")
    jsonschema.validate(
        spec["admission"], _load_schema("pair-spec.schema.json")["properties"]["admission"]
    )
    assert pairrun._admission_keys(spec["admission"]) == pairrun.ADMISSION_DISJOINT_KEYS
    spec["suite_secret_root"] = str(st["stage"] / "evaluator-only")
    spec.pop("isolation_method")
    spec.pop("access_block_log")
    spec.update(
        top_k=10,
        output_root=str(tmp_path / "capture"),
        strategies=[{"name": "whole_file"}],
        searchd_binary=str(tmp_path / "searchd"),
        searchd_expected_sha256=_fake_sha("searchd"),
    )
    jsonschema.validate(spec, _load_schema("pair-spec.schema.json"))
    spec_path = tmp_path / "disjoint-pair-spec.json"
    spec_path.write_text(json.dumps(spec))
    assert pairrun.load_spec(spec_path)["admission"] == spec["admission"]

    manifest = copy.deepcopy(st["manifest"])
    manifest["artifacts"].pop("experiment_custody")
    manifest["artifacts"].pop("development_suite")
    manifest["artifacts"].update(split_manifest="split.json", split_releases="releases.json")
    jsonschema.validate(manifest, _load_schema("run-manifest.schema.json"))
    pairrun._validate_manifest_shape(manifest)
    manifest["artifacts"]["development_suite"] = "wrong.json"
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(manifest, _load_schema("run-manifest.schema.json"))
    with pytest.raises(pairrun.RunError, match="complete admission bundle"):
        pairrun._validate_manifest_shape(manifest)


def test_repository_disjoint_source_custody_refuses_wrong_holdout(monkeypatch, tmp_path):
    monkeypatch.syspath_prepend(str(Path(pairrun.__file__).resolve().parents[1]))
    from tools.benchmark import corpus_binding

    split_path = tmp_path / "split.json"
    split_path.write_text("{}")
    release_digest = "sha256:" + _fake_sha("release")
    releases_path = tmp_path / "releases.json"
    releases_path.write_text(json.dumps({release_digest: str(tmp_path / "release")}))
    admission = {
        "repository_commit": "a" * 40,
        "repository_disjoint": {
            "repository": "holdout",
            "release_digest": release_digest,
            "split_manifest_sha256": pairrun.sha_file(split_path),
            "split_releases_sha256": pairrun.sha_file(releases_path),
        },
    }
    suite = {
        "repository_commit": "a" * 40,
        "file_universe_digest": "b" * 64,
        "tasks": [{"split": "eval", "query_family_id": "holdout.family"}],
    }
    entry = {
        "repository": "holdout",
        "release_digest": release_digest,
        "split": "holdout",
        "repository_commit": "a" * 40,
        "code_only_universe_digest": "sha256:" + "b" * 64,
        "query_family_ids": ["holdout.family"],
    }
    monkeypatch.setattr(
        corpus_binding, "validate_split_manifest", lambda _raw, _paths: {"repositories": [entry]}
    )
    calls = []
    monkeypatch.setattr(
        pairrun, "validate_suite", lambda repo, payload: calls.append((repo, payload))
    )
    pairrun._validate_disjoint_admission_source(
        admission, suite, tmp_path, split_path, releases_path
    )
    assert calls == [(tmp_path, suite)]
    for key, value, reason in (
        ("split", "development", "holdout assignment"),
        ("code_only_universe_digest", "sha256:" + "c" * 64, "suite differs"),
        ("query_family_ids", ["missing.family"], "suite differs"),
    ):
        original = entry[key]
        entry[key] = value
        with pytest.raises(pairrun.RunError, match=reason):
            pairrun._validate_disjoint_admission_source(
                admission, suite, tmp_path, split_path, releases_path
            )
        entry[key] = original
    admission["repository_disjoint"]["split_manifest_sha256"] = "0" * 64
    with pytest.raises(pairrun.RunError, match="split bytes differ"):
        pairrun._validate_disjoint_admission_source(
            admission, suite, tmp_path, split_path, releases_path
        )


def test_repository_disjoint_admission_freeze_routes_global_custody(monkeypatch, tmp_path):
    st = _pair_stage(tmp_path, blinding="isolated", scope="qualified", claims={"quality": True})
    spec = copy.deepcopy(st["spec"])
    admission_path = Path(spec["admission"]["manifest"])
    admission = json.loads(admission_path.read_text())
    admission["schema_version"] = 3
    admission.pop("development_suite_sha256")
    admission.pop("experiment_custody_sha256")
    admission["decision_policy_sha256"] = _fake_sha("policy")
    split_path = tmp_path / "split.json"
    split_path.write_text("{}")
    releases_path = tmp_path / "releases.json"
    releases_path.write_text("{}")
    admission["repository_disjoint"] = {
        "repository": "holdout",
        "release_digest": "sha256:" + _fake_sha("release"),
        "split_manifest_sha256": pairrun.sha_file(split_path),
        "split_releases_sha256": pairrun.sha_file(releases_path),
    }
    admission_path.write_text(json.dumps(admission))
    spec["admission"].pop("experiment_custody")
    spec["admission"].pop("development_suite")
    spec["admission"].update(split_manifest=str(split_path), split_releases=str(releases_path))
    receipt_paths = {
        key: st["stage"] / st["manifest"]["artifacts"][key]
        for key in ("contract_python_receipt", "contract_rust_receipt", "sdk_receipt")
    }
    observed = []
    monkeypatch.setattr(
        pairrun,
        "_validate_disjoint_admission_source",
        lambda claim, suite, repo, split, releases: observed.append((split, releases)),
    )
    target = tmp_path / "new-stage"
    target.mkdir()
    frozen = pairrun.freeze_admission(spec, target, {k: str(v) for k, v in receipt_paths.items()})
    assert set(frozen) == set(pairrun.ADMISSION_DISJOINT_KEYS)
    assert observed == [(Path(frozen["split_manifest"]), Path(frozen["split_releases"]))]


def test_repository_disjoint_verdict_replays_frozen_split_artifacts(monkeypatch, tmp_path):
    st = _pair_stage(tmp_path, blinding="isolated", scope="qualified", claims={"quality": True})
    before = _stage_verdict(st)
    admission_path = st["stage"] / "admission" / "admission.json"
    admission = json.loads(admission_path.read_text())
    admission["schema_version"] = 3
    admission.pop("development_suite_sha256")
    admission.pop("experiment_custody_sha256")
    admission["decision_policy_sha256"] = _fake_sha("policy")
    split_path = st["stage"] / "admission" / "split-manifest.json"
    split_path.write_text("{}")
    releases_path = st["stage"] / "admission" / "split-releases.json"
    releases_path.write_text("{}")
    admission["repository_disjoint"] = {
        "repository": "holdout",
        "release_digest": "sha256:" + _fake_sha("release"),
        "split_manifest_sha256": pairrun.sha_file(split_path),
        "split_releases_sha256": pairrun.sha_file(releases_path),
    }
    admission_path.write_text(json.dumps(admission))
    manifest = json.loads(st["manifest_path"].read_text())
    manifest["artifacts"].pop("experiment_custody")
    manifest["artifacts"].pop("development_suite")
    manifest["artifacts"].update(
        split_manifest="admission/split-manifest.json",
        split_releases="admission/split-releases.json",
    )
    manifest["provenance"]["admission"]["manifest_digest"] = pairrun.sha_file(admission_path)
    st["manifest_path"].write_text(json.dumps(manifest))
    protocol_path = st["stage"] / "protocol-lock.json"
    protocol = json.loads(protocol_path.read_text())
    protocol["admission_digest"] = pairrun.sha_file(admission_path)
    protocol_path.write_text(json.dumps(protocol))
    calls = []
    monkeypatch.setattr(
        pairrun,
        "_validate_disjoint_admission_source",
        lambda _claim, _suite, _repo, split, releases: calls.append((split, releases)),
    )
    review_modes = []
    validate_review = pairrun._validate_gold_review_receipt

    def capture_review_mode(*args, **kwargs):
        review_modes.append(kwargs.get("allow_mixed_source_oracle"))
        return validate_review(*args, **kwargs)

    monkeypatch.setattr(pairrun, "_validate_gold_review_receipt", capture_review_mode)
    after = _stage_verdict(st)
    assert after["states"] == before["states"]
    assert calls == [(split_path, releases_path)]
    assert review_modes == [True, True, True]
    monkeypatch.undo()
    split_path.write_text('{"changed":true}')
    rejected = _stage_verdict(st)
    assert rejected["states"]["QUALITY_DELTA"] == "fail"
    assert "split bytes differ" in rejected["state_evidence"]["QUALITY_DELTA"]["reason"]


def test_qualified_license_receipt_requires_approved_corpus_bound_decision(tmp_path):
    st = _pair_stage(tmp_path, blinding="isolated", scope="qualified", claims={"quality": True})
    evidence = st["stage"] / "admission"
    license_path = evidence / "license-receipt.json"
    admission_path = evidence / "admission.json"
    receipt = json.loads(license_path.read_text(encoding="utf-8"))
    admission = json.loads(admission_path.read_text(encoding="utf-8"))
    pairrun._validate_license_receipt(receipt, admission)

    for changes, reason in (
        ({"schema_version": True}, "schema version mismatch"),
        ({"reviewer_id": "other"}, "reviewer mismatch"),
        ({"decision": "denied"}, "not approved"),
        ({"repository_commit": "0" * 40}, "repository_commit mismatch"),
        ({"corpus_manifest_sha256": "0" * 64}, "corpus_manifest_sha256 mismatch"),
        ({"rationale": " "}, "rationale missing"),
    ):
        bad = {**receipt, **changes}
        with pytest.raises(pairrun.RunError, match=reason):
            pairrun._validate_license_receipt(bad, admission)

    # A self-consistent digest chain cannot convert a denial into approval.
    receipt["decision"] = "denied"
    license_path.write_text(json.dumps(receipt), encoding="utf-8")
    admission["license"]["receipt_sha256"] = pairrun.sha_file(license_path)
    admission_path.write_text(json.dumps(admission), encoding="utf-8")
    _rewrite_manifest(
        st,
        lambda manifest: manifest["provenance"]["admission"].update(
            manifest_digest=pairrun.sha_file(admission_path)
        ),
    )
    verdict = _stage_verdict(st)
    assert verdict["states"]["QUALITY_DELTA"] == "fail"
    assert "license receipt is not approved" in verdict["state_evidence"]["QUALITY_DELTA"]["reason"]


def test_qualified_gold_receipts_require_source_bound_task_reviews(tmp_path):
    st = _pair_stage(tmp_path, blinding="isolated", scope="qualified", claims={"quality": True})
    evidence = st["stage"] / "admission"
    annotation_paths = [
        evidence / "annotation-1-receipt.json",
        evidence / "annotation-2-receipt.json",
    ]
    first = json.loads(annotation_paths[0].read_text(encoding="utf-8"))
    second_digest = pairrun.sha_file(annotation_paths[1])
    adjudication = json.loads((evidence / "adjudication-receipt.json").read_text())
    admission = json.loads((evidence / "admission.json").read_text())
    same_reviewer = json.loads(json.dumps(admission))
    same_reviewer["gold"]["adjudicator_id"] = "gold-owner-a"
    with pytest.raises(pairrun.RunError, match="distinct from annotators"):
        pairrun.validate_admission_manifest(same_reviewer)
    common = {
        "suite_sha256": pairrun.sha_file(st["suite_path"]),
        "suite": st["suite"],
        "repo": st["repo"],
    }

    pairrun._validate_gold_review_receipt(
        first, role="annotation 1", reviewer_id="gold-owner-a", **common
    )
    pairrun._validate_gold_review_receipt(
        adjudication,
        role="adjudication",
        reviewer_id="gold-adjudicator",
        annotation_digests=[pairrun.sha_file(annotation_paths[0]), second_digest],
        **common,
    )

    for mutate, reason in (
        (lambda row: row.update(schema_version=True), "schema version mismatch"),
        (lambda row: row.update(reviews=[]), "task coverage mismatch"),
        (
            lambda row: row["reviews"][0].update(query_sha256="0" * 64),
            "task identity mismatch",
        ),
        (
            lambda row: row["reviews"][0]["labels"]["gold"][0].update(file_sha256="0" * 64),
            "source validation failed",
        ),
    ):
        bad = json.loads(json.dumps(first))
        mutate(bad)
        with pytest.raises(pairrun.RunError, match=reason):
            pairrun._validate_gold_review_receipt(
                bad, role="annotation 1", reviewer_id="gold-owner-a", **common
            )

    bad_adjudication = json.loads(json.dumps(adjudication))
    bad_adjudication["reviews"][0]["labels"] = {"answerable": False, "gold": []}
    with pytest.raises(pairrun.RunError, match="differs from suite gold"):
        pairrun._validate_gold_review_receipt(
            bad_adjudication,
            role="adjudication",
            reviewer_id="gold-adjudicator",
            annotation_digests=[pairrun.sha_file(annotation_paths[0]), second_digest],
            **common,
        )
    bad_adjudication = json.loads(json.dumps(adjudication))
    bad_adjudication["annotation_receipt_sha256"].reverse()
    with pytest.raises(pairrun.RunError, match="annotation binding mismatch"):
        pairrun._validate_gold_review_receipt(
            bad_adjudication,
            role="adjudication",
            reviewer_id="gold-adjudicator",
            annotation_digests=[pairrun.sha_file(annotation_paths[0]), second_digest],
            **common,
        )

    # Rebind every digest a forged admission controls. The verdict must reject
    # the empty decision set on content, not merely on a stale file hash.
    first["reviews"] = []
    annotation_paths[0].write_text(json.dumps(first), encoding="utf-8")
    adjudication["annotation_receipt_sha256"][0] = pairrun.sha_file(annotation_paths[0])
    adjudication_path = evidence / "adjudication-receipt.json"
    adjudication_path.write_text(json.dumps(adjudication), encoding="utf-8")
    admission_path = evidence / "admission.json"
    admission = json.loads(admission_path.read_text(encoding="utf-8"))
    admission["gold"]["annotators"][0]["receipt_sha256"] = pairrun.sha_file(annotation_paths[0])
    admission["gold"]["adjudication_receipt_sha256"] = pairrun.sha_file(adjudication_path)
    admission_path.write_text(json.dumps(admission), encoding="utf-8")
    _rewrite_manifest(
        st,
        lambda manifest: manifest["provenance"]["admission"].update(
            manifest_digest=pairrun.sha_file(admission_path)
        ),
    )
    verdict = _stage_verdict(st)
    assert verdict["states"]["QUALITY_DELTA"] == "fail"
    assert "task coverage mismatch" in verdict["state_evidence"]["QUALITY_DELTA"]["reason"]


def test_qualified_gold_receipt_rejects_explicit_unreviewed_suite_claim(tmp_path):
    st = _pair_stage(tmp_path, blinding="isolated", scope="qualified", claims={"quality": True})
    suite = json.loads(json.dumps(st["suite"]))
    suite["diagnostic_policy"] = ev.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
    suite["tasks"][0]["label_review"] = {"assessment": "unreviewed"}
    ev.validate_suite(st["repo"], suite)
    receipt_path = st["stage"] / "admission" / "annotation-1-receipt.json"
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    suite_sha = ev.digest(ev.canonical(suite))
    receipt["suite_sha256"] = suite_sha
    with pytest.raises(pairrun.RunError, match="explicitly unreviewed"):
        pairrun._validate_gold_review_receipt(
            receipt,
            role="annotation 1",
            reviewer_id="gold-owner-a",
            suite_sha256=suite_sha,
            suite=suite,
            repo=st["repo"],
        )
    suite["tasks"][0]["label_review"] = {
        "assessment": "reviewed_ambiguous",
        "reviewer_id": "independent-reviewer",
        "evidence_sha256": ev.digest(b"reviewed ambiguous fixture"),
    }
    ev.validate_suite(st["repo"], suite)
    suite_sha = ev.digest(ev.canonical(suite))
    receipt["suite_sha256"] = suite_sha
    pairrun._validate_gold_review_receipt(
        receipt,
        role="annotation 1",
        reviewer_id="gold-owner-a",
        suite_sha256=suite_sha,
        suite=suite,
        repo=st["repo"],
    )


def test_qualified_custody_refuses_rebound_wrong_source_and_shared_gold_file(tmp_path):
    for label in ("wrong-source", "shared-gold-file"):
        st = _pair_stage(
            tmp_path / label, blinding="isolated", scope="qualified", claims={"quality": True}
        )
        admission_dir = st["stage"] / "admission"
        custody_path = admission_dir / "experiment-custody.json"
        development_path = admission_dir / "development-suite.json"
        admission_path = admission_dir / "admission.json"
        custody = json.loads(custody_path.read_text(encoding="utf-8"))
        admission = json.loads(admission_path.read_text(encoding="utf-8"))
        if label == "wrong-source":
            custody["source_revision"] = "b" * 40
        else:
            development = json.loads(development_path.read_text(encoding="utf-8"))
            holdout = json.loads(st["suite_path"].read_text(encoding="utf-8"))
            development["file_universe"] = holdout["file_universe"]
            development["file_universe_digest"] = holdout["file_universe_digest"]
            development["tasks"][0]["gold"] = [holdout["tasks"][0]["gold"][0]]
            development_path.write_text(json.dumps(development), encoding="utf-8")
            custody["development_suite_sha256"] = ev.digest(ev.canonical(development))
            admission["development_suite_sha256"] = pairrun.sha_file(development_path)
        custody_path.write_text(json.dumps(custody), encoding="utf-8")
        admission["experiment_custody_sha256"] = pairrun.sha_file(custody_path)
        admission_path.write_text(json.dumps(admission), encoding="utf-8")
        rebound_digest = pairrun.sha_file(admission_path)
        _rewrite_manifest(
            st,
            lambda manifest, digest_value=rebound_digest: manifest["provenance"][
                "admission"
            ].update(manifest_digest=digest_value),
        )
        verdict = _stage_verdict(st)
        assert verdict["states"]["QUALITY_DELTA"] == "fail"
        assert verdict["state_evidence"]["QUALITY_DELTA"]["reason"].startswith(
            "admission_unverified:"
        )


def test_shared_query_protocol_is_deterministic_digest_bound_and_permuted():
    task_ids = [f"T{index:02d}" for index in range(20)]
    first = pairrun.build_query_protocol(task_ids, 17, 1, 10)
    second = pairrun.build_query_protocol(task_ids, 17, 1, 10)
    assert first == second
    assert pairrun.validate_query_protocol(first, task_ids, "protocol") == first
    assert all(sorted(schedule) == task_ids for schedule in first["measurement_schedules"])

    mutated = json.loads(json.dumps(first))
    mutated["measurement_schedules"][0].reverse()
    with pytest.raises(pairrun.RunError, match="sha256 mismatch"):
        pairrun.validate_query_protocol(mutated, task_ids, "protocol")

    self_consistent_forgery = json.loads(json.dumps(first))
    self_consistent_forgery["measurement_schedules"][0].reverse()
    core = {key: value for key, value in self_consistent_forgery.items() if key != "sha256"}
    self_consistent_forgery["sha256"] = pairrun._protocol_digest(core)
    with pytest.raises(pairrun.RunError, match="deterministic seeded schedule"):
        pairrun.validate_query_protocol(self_consistent_forgery, task_ids, "protocol")

    malformed = json.loads(json.dumps(first))
    malformed["measurement_schedules"][0][0] = 7
    with pytest.raises(pairrun.RunError, match="exact task permutation"):
        pairrun.validate_query_protocol(malformed, task_ids, "protocol")


@pytest.mark.parametrize(
    ("spec", "task_count", "message"),
    [
        (
            {
                "repetitions": 4,
                "query_warmup_passes": 1,
                "query_repetitions_per_root": 13,
                "routes": ["hybrid"],
            },
            20,
            "fresh roots",
        ),
        (
            {
                "repetitions": 5,
                "query_warmup_passes": 1,
                "query_repetitions_per_root": 10,
                "routes": ["hybrid"],
            },
            19,
            "20 frozen tasks",
        ),
        (
            {
                "repetitions": 5,
                "query_warmup_passes": 0,
                "query_repetitions_per_root": 10,
                "routes": ["hybrid"],
            },
            20,
            "warmup pass",
        ),
        (
            {
                "repetitions": 5,
                "query_warmup_passes": 1,
                "query_repetitions_per_root": 9,
                "routes": ["hybrid"],
            },
            20,
            "warm observations",
        ),
        (
            {
                "repetitions": 5,
                "query_warmup_passes": 1,
                "query_repetitions_per_root": 10,
                "routes": ["lexical", "hybrid"],
            },
            20,
            "exactly one Quanta route",
        ),
    ],
)
def test_qualified_speed_spec_rejects_underpowered_or_biased_protocol(spec, task_count, message):
    with pytest.raises(pairrun.RunError, match=message):
        pairrun.validate_qualified_speed_spec(spec, task_count)


def test_qualified_speed_spec_accepts_1000_warm_observations_per_route():
    pairrun.validate_qualified_speed_spec(
        {
            "repetitions": 5,
            "query_warmup_passes": 1,
            "query_repetitions_per_root": 10,
            "routes": ["hybrid"],
        },
        20,
    )


def test_protocol_phase_metrics_bind_raw_warm_counts_and_cold_separately():
    task_ids = ["T1", "T2"]
    protocol = pairrun.build_query_protocol(task_ids, 3, 1, 2)
    phase = {
        "schema_version": 1,
        "system": "quanta",
        "timing_layer": "runner_monotonic_wall_v1",
        "strategy": "whole_file",
        "record_sha256": "a" * 64,
        "runner_binary_sha256": "b" * 64,
        "task_count": 2,
        "route_count": 1,
        "file_count": 1,
        "chunk_count": 1,
        "query_schedule": task_ids,
        "warmup_passes": 1,
        "measurement_repetitions": 2,
        "query_protocol": protocol,
        "warm_latencies_ms": {"hybrid": {"T1": [1.0, 1.1], "T2": [2.0, 2.1]}},
        "cold_latencies_ms": {"hybrid": 3.0},
        "phases_ms": {
            "discovery": 1.0,
            "chunk": 1.0,
            "model_provider_prepare": 1.0,
            "embed_publish_seal_activate": 1.0,
            "cold_query": 3.0,
            "warmup": 1.0,
            "warm_query": 7.0,
            "unattributed": 1.0,
        },
        "total_ms": 16.0,
    }
    assert pairrun._validate_phase_metrics(phase, "phase") == phase
    for invalid in (10**400, -(10**400), float("nan"), float("inf"), -float("inf"), True, None):
        for field in ("warm", "cold", "phase", "total"):
            forged = json.loads(json.dumps(phase))
            if field == "warm":
                forged["warm_latencies_ms"]["hybrid"]["T1"][0] = invalid
            elif field == "cold":
                forged["cold_latencies_ms"]["hybrid"] = invalid
            elif field == "phase":
                forged["phases_ms"]["discovery"] = invalid
            else:
                forged["total_ms"] = invalid
            with pytest.raises(pairrun.RunError):
                pairrun._validate_phase_metrics(forged, "phase")
    current = json.loads(json.dumps(phase))
    current.update(
        {
            "schema_version": 2,
            "symbol_count": 3,
            "symbol_producer_identity": pairrun.QUANTA_SYMBOL_PRODUCER_IDENTITY,
            "symbol_grammars": pairrun.QUANTA_SYMBOL_GRAMMARS,
            "symbol_unsupported_files": 0,
            "symbol_unsupported_details": [],
            "symbol_only_scopes": 0,
            "symbol_coverage": [
                {
                    "path": "src/a.rs",
                    "source_sha256": "c" * 64,
                    "language": "rust",
                    "definition_count": 3,
                }
            ],
        }
    )
    _current_symbol_metrics(current)
    assert pairrun._validate_phase_metrics(current, "phase") == current
    measured = copy.deepcopy(current)
    measured["schema_version"] = 3
    measured["phases_ms"]["daemon_boot_and_readiness"] = measured["phases_ms"].pop(
        "model_provider_prepare"
    )
    measured["phases_ms"].update(sdk_publish=0.6, sdk_activate=0.3)
    # Nested SDK clocks do not increase the 16 ms outer phase partition.
    assert pairrun._validate_phase_metrics(measured, "phase") == measured
    mislabeled = copy.deepcopy(measured)
    mislabeled["phases_ms"]["model_provider_prepare"] = mislabeled["phases_ms"].pop(
        "daemon_boot_and_readiness"
    )
    with pytest.raises(pairrun.RunError, match="exactly"):
        pairrun._validate_phase_metrics(mislabeled, "phase")
    for value in (True, None, float("nan"), float("inf"), -1.0, 0.5):
        forged = copy.deepcopy(measured)
        forged["phases_ms"]["sdk_activate"] = value
        with pytest.raises(pairrun.RunError):
            pairrun._validate_phase_metrics(forged, "phase")
    missing = copy.deepcopy(measured)
    del missing["phases_ms"]["sdk_publish"]
    with pytest.raises(pairrun.RunError, match="exactly"):
        pairrun._validate_phase_metrics(missing, "phase")
    stale_producer = json.loads(json.dumps(current))
    stale_producer["symbol_producer_identity"] = "source-bound-symbols-v1"
    with pytest.raises(pairrun.RunError, match="symbol producer evidence"):
        pairrun._validate_phase_metrics(stale_producer, "phase")
    for required_key in ("symbol_coverage", "symbol_unsupported_details"):
        missing = json.loads(json.dumps(current))
        del missing[required_key]
        with pytest.raises(pairrun.RunError, match="must hold exactly"):
            pairrun._validate_phase_metrics(missing, "phase")
    forged_grammars = json.loads(json.dumps(current))
    forged_grammars["symbol_grammars"] = "tree-sitter@0.25;forged@9.9"
    with pytest.raises(pairrun.RunError, match="symbol producer evidence"):
        pairrun._validate_phase_metrics(forged_grammars, "phase")
    incomplete = json.loads(json.dumps(current))
    incomplete["symbol_unsupported_files"] = 1
    incomplete["symbol_unsupported_details"] = [
        {
            "path": "docs/readme.md",
            "file_sha256": "d" * 64,
            "reason": "unsupported_language",
        }
    ]
    with pytest.raises(pairrun.RunError):
        pairrun._validate_phase_metrics(incomplete, "phase")
    incomplete["symbol_unsupported_details"][0]["file_sha256"] = "bad"
    with pytest.raises(pairrun.RunError):
        pairrun._validate_phase_metrics(incomplete, "phase")
    admitted = {"files": [{"path": "src/a.rs", "file_sha256": "c" * 64}]}
    pairrun._verify_symbol_coverage_corpus(current, admitted)
    with pytest.raises(pairrun.RunError, match="differs from frozen corpus"):
        pairrun._verify_symbol_coverage_corpus(
            current, {"files": [{"path": "src/a.rs", "file_sha256": "d" * 64}]}
        )
    with pytest.raises(pairrun.RunError, match="lack file-level symbol coverage"):
        pairrun._verify_symbol_coverage_corpus(phase, admitted)
    for change, match in (
        ({"path": "src/a.py"}, "grammar mismatch"),
        ({"path": "../a.rs"}, "path is invalid"),
        ({"source_sha256": "bad"}, "source hash"),
        ({"definition_count": 2}, "incomplete"),
    ):
        corrupted = json.loads(json.dumps(current))
        corrupted["symbol_coverage"][0].update(change)
        with pytest.raises(pairrun.RunError, match=match):
            pairrun._validate_phase_metrics(corrupted, "phase")
    incomplete = json.loads(json.dumps(current))
    incomplete["symbol_unsupported_files"] = 1
    with pytest.raises(pairrun.RunError, match="details differ from count"):
        pairrun._validate_phase_metrics(incomplete, "phase")
    del current["symbol_count"]
    with pytest.raises(pairrun.RunError, match="must hold exactly"):
        pairrun._validate_phase_metrics(current, "phase")
    phase["warm_latencies_ms"]["hybrid"]["T1"].pop()
    with pytest.raises(pairrun.RunError, match="count differs"):
        pairrun._validate_phase_metrics(phase, "phase")


def test_protocol_phase_metrics_reject_samples_longer_than_enclosing_phases(tmp_path):
    st = _pair_stage(tmp_path)
    sphase = json.loads(Path(st["rep_layouts"][0]["semble_phase_metrics"]).read_text())
    sphase.update(
        {
            "schema_version": 2,
            "profile": "native-default",
            "requested_alpha": None,
            "rerank_applied": True,
            "lane_call_counts": {"bm25": 3, "semantic": 3, "encode": 3},
            "execution_events_sha256": "c" * 64,
            "function_identity": {
                name: {"module": "semble.search", "qualname": name, "source_sha256": "d" * 64}
                for name in (
                    "bm25",
                    "index_search",
                    "module_search",
                    "resolve_alpha",
                    "semantic",
                )
            },
            "observed_wrapped_call_ns": 17,
        }
    )
    assert pairrun._validate_phase_metrics(sphase, "phase") == sphase
    sphase["lane_call_counts"]["semantic"] = 0
    with pytest.raises(pairrun.RunError, match="contradict"):
        pairrun._validate_phase_metrics(sphase, "phase")
    qphase = json.loads(
        Path(st["rep_layouts"][0]["quanta_phase_metrics"]["whole_file"]).read_text()
    )
    qphase["cold_latencies_ms"]["lexical"] = 1e12
    with pytest.raises(pairrun.RunError, match="cold samples exceed"):
        pairrun._validate_phase_metrics(qphase, "phase")
    qphase["cold_latencies_ms"]["lexical"] = 1.0
    qphase["warm_latencies_ms"]["lexical"][qphase["query_schedule"][0]][0] = 1e12
    with pytest.raises(pairrun.RunError, match="warm samples exceed"):
        pairrun._validate_phase_metrics(qphase, "phase")


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("top_k", 999),
        ("repetitions", 999),
        ("system_orders", [[1, "quanta"]]),
        ("strategies", ["not-executed"]),
        ("searchd_expected_sha256", "0" * 64),
        ("semble_lockfile_sha256", "0" * 64),
        ("host_profile_digest", "0" * 64),
        ("unknown_authority", True),
    ],
)
def test_verdict_rejects_protocol_lock_pin_mutations(tmp_path, field, value):
    st = _pair_stage(tmp_path)
    baseline = _stage_verdict(st)
    assert baseline["states"]["PAIR_VALID"] == "pass"
    lock = st["stage"] / "protocol-lock.json"
    payload = json.loads(lock.read_text())
    payload[field] = value
    lock.write_text(json.dumps(payload), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert "protocol_lock" in verdict["state_evidence"]["PAIR_VALID"]["reason"]


def test_qualified_replay_rejects_two_task_protocol_even_with_1000_samples(tmp_path):
    st = _pair_stage(tmp_path, repetitions=5, scope="qualified", claims={"speed": True})
    pack = pairrun.read_json(Path(st["spec"]["query_pack"]))
    protocol_digests = []
    for layout in st["rep_layouts"]:
        protocol_path = Path(layout["query_protocol"])
        previous = json.loads(protocol_path.read_text())
        protocol = pairrun.build_query_protocol(previous["task_ids"], previous["seed"], 1, 100)
        protocol_path.write_text(json.dumps(protocol), encoding="utf-8")
        protocol_digests.append(protocol["sha256"])
        for phase_path in (
            Path(layout["quanta_phase_metrics"]["whole_file"]),
            Path(layout["semble_phase_metrics"]),
        ):
            phase = json.loads(phase_path.read_text())
            phase["query_protocol"] = protocol
            phase["measurement_repetitions"] = 100
            for by_task in phase["warm_latencies_ms"].values():
                for task_id, values in by_task.items():
                    by_task[task_id] = values * 100
            phase["phases_ms"]["warm_query"] += 297.0
            phase["total_ms"] += 297.0
            if phase["system"] == "semble":
                phase["phase_boundaries_ns"]["query_end"] += 297_000_000
                phase["phase_boundaries_ns"]["worker_end"] += 297_000_000
            phase_path.write_text(json.dumps(phase), encoding="utf-8")
        _rebind_semble_native_to_protocol(layout, protocol, pack)
        quanta_manifest_path = Path(layout["quanta_manifest"])
        quanta_manifest = json.loads(quanta_manifest_path.read_text())
        quanta_manifest["runs"][0]["phase_metrics_digest"] = pairrun.sha_file(
            Path(layout["quanta_phase_metrics"]["whole_file"])
        )
        quanta_manifest_path.write_text(json.dumps(quanta_manifest), encoding="utf-8")
    lock_path = st["stage"] / "protocol-lock.json"
    lock = json.loads(lock_path.read_text())
    lock["query_repetitions_per_root"] = 100
    lock["query_protocol_sha256s"] = protocol_digests
    lock_path.write_text(json.dumps(lock), encoding="utf-8")
    matrix = pairrun.build_latency_matrix(st["rep_layouts"])
    assert matrix["observations_floor"] == 1000
    (st["stage"] / "latency-matrix.json").write_text(json.dumps(matrix), encoding="utf-8")
    _rewrite_manifest(
        st,
        lambda manifest: manifest["evidence"]["perf"].update(
            observations_floor=matrix["observations_floor"], fresh_roots=matrix["fresh_roots"]
        ),
    )
    _rebind_phase_metrics_digests(
        st,
        *(
            path
            for layout in st["rep_layouts"]
            for path in (
                Path(layout["quanta_phase_metrics"]["whole_file"]),
                Path(layout["semble_phase_metrics"]),
            )
        ),
    )
    verdict = _stage_verdict(st)
    assert verdict["states"]["PAIR_VALID"] == "pass"
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert "at least 20 frozen tasks" in verdict["state_evidence"]["PERF_QUALIFIED"]["reason"]


def test_verdict_cannot_qualify_cold_only_latency_as_warm_performance(tmp_path, monkeypatch):
    _allow_minimal_speed_fixture(monkeypatch)
    st = _pair_stage(tmp_path, scope="qualified", claims={"speed": True})
    layout = st["rep_layouts"][0]
    qphase_path = Path(layout["quanta_phase_metrics"]["whole_file"])
    qphase = json.loads(qphase_path.read_text(encoding="utf-8"))
    for key in ("query_protocol", "warm_latencies_ms", "cold_latencies_ms"):
        qphase.pop(key)
    qphase["phases_ms"]["first_query"] = qphase["phases_ms"].pop("cold_query")
    qphase["phases_ms"].pop("warmup")
    qphase["warmup_passes"] = 0
    qphase["total_ms"] = 7.0
    qphase_path.write_text(json.dumps(qphase), encoding="utf-8")
    quanta_manifest_path = st["stage"] / "rep-00" / "quanta" / "quanta-manifest.json"
    quanta_manifest = json.loads(quanta_manifest_path.read_text(encoding="utf-8"))
    quanta_manifest["runs"][0]["phase_metrics_digest"] = ev.digest(qphase_path.read_bytes())
    quanta_manifest_path.write_text(json.dumps(quanta_manifest), encoding="utf-8")

    sphase_path = Path(layout["semble_phase_metrics"])
    sphase = json.loads(sphase_path.read_text(encoding="utf-8"))
    for key in ("query_protocol", "warm_latencies_ms", "cold_latencies_ms"):
        sphase.pop(key)
    sphase["phases_ms"]["first_query"] = sphase["phases_ms"].pop("cold_query")
    sphase["phases_ms"]["warm_query"] = 1.0
    sphase["phase_boundaries_ns"] = {
        "worker_start": 0,
        "discovery_end": 1_000_000,
        "model_provider_prepare_end": 2_000_000,
        "index_end": 3_000_000,
        "warmup_end": 4_000_000,
        "query_start": 4_000_000,
        "first_query_start": 4_000_000,
        "first_query_end": 5_000_000,
        "query_end": 6_000_000,
        "worker_end": 7_000_000,
    }
    sphase["total_ms"] = 7.0
    sphase_path.write_text(json.dumps(sphase), encoding="utf-8")

    legacy_layout = dict(layout)
    legacy_layout.pop("query_protocol")
    legacy_layout.pop("quanta_phase_metrics")
    matrix = pairrun.build_latency_matrix([legacy_layout])
    (st["stage"] / "latency-matrix.json").write_text(json.dumps(matrix), encoding="utf-8")
    _rewrite_manifest(
        st,
        lambda manifest: manifest["evidence"]["perf"].update(
            observations_floor=matrix["observations_floor"],
            fresh_roots=matrix["fresh_roots"],
        ),
    )
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == ("phase_boundaries_incomplete")


def test_verdict_rejects_nonconsecutive_or_unbound_root_protocols(tmp_path, monkeypatch):
    _allow_minimal_speed_fixture(monkeypatch)
    st = _pair_stage(tmp_path, repetitions=2, scope="qualified", claims={"speed": True})
    protocol_path = st["stage"] / "protocol-lock.json"
    protocol = json.loads(protocol_path.read_text(encoding="utf-8"))
    protocol["base_seed"] += 1
    protocol_path.write_text(json.dumps(protocol), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == (
        "query_protocol_root_sequence_unverified"
    )


def test_source_closure_driver_uses_canonical_capture_and_verify_commands(tmp_path, monkeypatch):
    observed = []

    def fake_run(command, **kwargs):
        observed.append((command, kwargs))
        return subprocess.CompletedProcess(command, 0, stdout="ok", stderr="")

    monkeypatch.setattr(pairrun.subprocess, "run", fake_run)
    closure = tmp_path / "closure.json"
    pairrun._source_closure(tmp_path, "capture", closure)
    pairrun._source_closure(tmp_path, "verify", closure)
    assert observed[0][0][-5:] == [
        "capture",
        "--profile",
        "retrieval",
        "--out",
        str(closure),
    ]
    assert observed[1][0][-3:] == ["verify", "--manifest", str(closure)]
    assert all(call[1]["cwd"] == tmp_path and call[1]["timeout"] == 300 for call in observed)


def test_retrieval_source_closure_profile_covers_authority_surfaces():
    profile = source_closure.PROFILES["retrieval"]
    paths = set(profile["paths"])
    assert {
        "Cargo.lock",
        "Cargo.toml",
        "Justfile",
        "docs/plans/sep-27-misc/tickets",
        "tools/benchmark/retrieval",
        "tools/ci/nextest_events.py",
        "tools/ci/source_closure.py",
        "tools/ci/tests/test_retrieval_benchmark.py",
        "tools/ci/tests/test_retrieval_contract_proof.py",
        "tools/ci/tests/test_retrieval_sdk_proof.py",
        "tools/ci/write-verification-receipt.py",
    } <= paths
    assert set(profile["cargo_packages"]) == {
        "quanta-index-retrieval-bench",
        "quanta-index-searchd-runtime",
    }


def test_source_closure_driver_fails_closed_on_tool_refusal(tmp_path, monkeypatch):
    monkeypatch.setattr(
        pairrun.subprocess,
        "run",
        lambda *args, **kwargs: subprocess.CompletedProcess(args[0], 2, stdout="", stderr="dirty"),
    )
    with pytest.raises(pairrun.RunError, match="source-closure capture refused: dirty"):
        pairrun._source_closure(tmp_path, "capture", tmp_path / "closure.json")
    with pytest.raises(pairrun.RunError, match="unsupported source-closure command"):
        pairrun._source_closure(tmp_path, "check", None)


def test_source_closure_driver_reuse_invokes_bound_preflight(tmp_path, monkeypatch):
    observed = []

    def completed(args, **kwargs):
        observed.append((args, kwargs))
        return subprocess.CompletedProcess(args, 0, stdout="source closure reuse ok", stderr="")

    monkeypatch.setattr(pairrun.subprocess, "run", completed)
    reused = tmp_path / "prior.json"
    staged = tmp_path / "staged.json"
    pairrun._source_closure(tmp_path, "reuse", staged, reuse_from=reused)
    assert observed[0][0][-5:] == ["reuse", "--manifest", str(reused), "--out", str(staged)]
    assert observed[0][1]["cwd"] == tmp_path


def test_verdict_cannot_upgrade_qualified_warm_cache(tmp_path, monkeypatch):
    _allow_minimal_speed_fixture(monkeypatch)
    st = _pair_stage(tmp_path, scope="qualified", claims={"speed": True}, cache_regime="warm_cache")
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == ("unsupported_cache_protocol")


def test_qualified_quality_requires_estimable_uncertainty(tmp_path):
    st = _pair_stage(tmp_path, blinding="isolated", scope="qualified", claims={"quality": True})
    verdict = _stage_verdict(st)
    assert verdict["states"]["QUALITY_DELTA"] == "fail"
    assert verdict["state_evidence"]["QUALITY_DELTA"]["reason"] == "uncertainty_unqualified"


def test_qualified_verdict_refuses_tampered_driver_source_closure(tmp_path):
    st = _pair_stage(tmp_path, scope="qualified")
    closure_path = st["stage"] / "driver-source-closure.json"
    closure = json.loads(closure_path.read_text(encoding="utf-8"))
    closure["files"][0]["sha256"] = _fake_sha("tampered-source")
    closure_path.write_text(json.dumps(closure), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="driver source closure.digest mismatch"):
        _stage_verdict(st)


def test_exploratory_pair_binds_clean_source_closure_without_quality_claim(tmp_path):
    st = _pair_stage(tmp_path)
    manifest = st["manifest"]
    closure = json.loads((st["stage"] / "driver-source-closure.json").read_text())
    assert manifest["scope"] == "exploratory"
    assert manifest["artifacts"]["driver_source_closure"] == "driver-source-closure.json"
    assert manifest["provenance"]["quanta"]["source_closure_digest"] == closure["digest"]
    assert (
        json.loads((st["stage"] / "protocol-lock.json").read_text())["driver_source_closure_digest"]
        == closure["digest"]
    )
    jsonschema.validate(manifest, _load_schema("run-manifest.schema.json"))
    assert _stage_verdict(st)["states"]["PAIR_VALID"] == "pass"


def test_pair_provenance_keeps_driver_revision_distinct_from_unattested_binary_source(
    tmp_path,
):
    st = _pair_stage(tmp_path)
    manifest = st["manifest"]
    quanta = manifest["provenance"]["quanta"]
    closure = json.loads((st["stage"] / "driver-source-closure.json").read_text())
    assert quanta["source_sha"] == closure["revision"]
    assert quanta["binary_build_source_revision"] is None
    jsonschema.validate(manifest, _load_schema("run-manifest.schema.json"))
    verdict = _stage_verdict(st)
    assert verdict["states"]["PAIR_VALID"] == "pass"
    assert verdict["provenance"]["quanta"]["binary_build_source_revision"] is None
    jsonschema.validate(verdict, _load_schema("verdict.schema.json"))

    _rewrite_manifest(
        st,
        lambda value: value["provenance"]["quanta"].update(binary_build_source_revision="e" * 40),
    )
    with pytest.raises(pairrun.RunError, match="binary build source revision is not attested"):
        _stage_verdict(st)

    _rewrite_manifest(
        st, lambda value: value["provenance"]["quanta"].pop("binary_build_source_revision")
    )
    assert "binary_build_source_revision" not in _stage_verdict(st)["provenance"]["quanta"]


def test_pair_derives_binary_source_only_after_fresh_sdk_chain_verifies(tmp_path):
    stage = _pair_stage(tmp_path, receipts="full", sdk_build_profile="release-fresh")
    assert stage["manifest"]["provenance"]["quanta"]["binary_build_source_revision"] is None
    verdict = _stage_verdict(stage)
    assert verdict["states"]["SDK_PATH_GREEN"] == "pass", verdict["state_evidence"][
        "SDK_PATH_GREEN"
    ]
    assert (
        verdict["provenance"]["quanta"]["binary_build_source_revision"]
        == (stage["manifest"]["provenance"]["quanta"]["source_sha"])
    )
    jsonschema.validate(verdict, _load_schema("verdict.schema.json"))
    raw = stage["stage"] / stage["manifest"]["artifacts"]["sdk_nextest_raw"]
    raw.write_bytes(raw.read_bytes() + b"tampered")
    rejected = _stage_verdict(stage)
    assert rejected["states"]["SDK_PATH_GREEN"] == "fail"
    assert rejected["provenance"]["quanta"]["binary_build_source_revision"] is None


def test_exploratory_pair_refuses_unbound_or_tampered_source_closure(tmp_path):
    st = _pair_stage(tmp_path)
    closure_path = st["stage"] / "driver-source-closure.json"
    closure = json.loads(closure_path.read_text())
    closure["files"][0]["sha256"] = _fake_sha("tampered")
    closure_path.write_text(json.dumps(closure))
    with pytest.raises(pairrun.RunError, match="driver source closure.digest mismatch"):
        _stage_verdict(st)

    closure["digest"] = ev.digest(
        ev.canonical(
            {
                key: closure[key]
                for key in ("schema_version", "profile", "revision", "roots", "files")
            }
        )
    )
    closure_path.write_text(json.dumps(closure))
    verdict = _stage_verdict(st)
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert verdict["state_evidence"]["PAIR_VALID"]["reason"] == (
        "driver_source_closure_digest_drift"
    )


def test_exploratory_pair_requires_source_closure_capture_and_final_verify(tmp_path, monkeypatch):
    cache, revision = _pinned_semble_cache(tmp_path)
    spec = {
        "execution_profiles": {
            "quanta": qp.execution_profile("native"),
            "semble": semble_adapter.execution_profile("native-default", None),
        },
        "semble_lockfile_sha256": _fake_sha("lock"),
        "semble_python": "/pinned/python",
        "semble_lockfile": "/pinned/lock",
        "semble_cache_root": str(cache),
        "semble_model_revision": revision,
        "host_profile": "/pinned/host",
        "output_root": str(tmp_path / "pair"),
        "strategies": [{"name": "whole_file"}],
    }
    monkeypatch.setattr(pairrun, "preflight_capture", lambda _spec: tmp_path / "pair")
    monkeypatch.setattr(pairrun, "preflight_daemon_socket_paths", lambda *_args, **_kwargs: None)
    executed = []
    monkeypatch.setattr(pairrun, "_run_pair_staged", lambda *_args: executed.append(True))
    with pytest.raises(pairrun.RunError, match="dirty relevant source"):
        monkeypatch.setattr(
            pairrun,
            "_source_closure",
            lambda *_args: (_ for _ in ()).throw(pairrun.RunError("dirty relevant source")),
        )
        pairrun.run_pair(spec)
    assert not executed and not (tmp_path / "pair").exists()

    stage = tmp_path / "pair.staging"
    stage.rmdir()
    calls = []

    def verify_drift(_root, command, _path):
        calls.append(command)
        if command == "verify":
            raise pairrun.RunError("source closure changed")

    monkeypatch.setattr(pairrun, "_source_closure", verify_drift)
    monkeypatch.setattr(pairrun, "_run_pair_staged", lambda *_args: {"states": {}})
    with pytest.raises(pairrun.RunError, match="source closure changed"):
        pairrun.run_pair(spec)
    assert calls == ["capture", "verify"]
    assert not (tmp_path / "pair").exists()


def test_exploratory_pair_reuses_closure_but_keeps_final_verification(tmp_path, monkeypatch):
    cache, revision = _pinned_semble_cache(tmp_path)
    prior = tmp_path / "prior-closure.json"
    closure = {
        "schema_version": 1,
        "profile": "retrieval",
        "revision": "a" * 40,
        "roots": ["source.rs"],
        "files": [{"path": "source.rs", "sha256": _fake_sha("source")}],
    }
    closure["digest"] = ev.digest(ev.canonical(closure))
    prior.write_text(json.dumps(closure) + "\n", encoding="utf-8")
    spec = {
        "execution_profiles": {
            "quanta": qp.execution_profile("native"),
            "semble": semble_adapter.execution_profile("native-default", None),
        },
        "semble_lockfile_sha256": _fake_sha("lock"),
        "semble_python": "/pinned/python",
        "semble_lockfile": "/pinned/lock",
        "semble_cache_root": str(cache),
        "semble_model_revision": revision,
        "host_profile": "/pinned/host",
        "output_root": str(tmp_path / "pair"),
        "strategies": [{"name": "whole_file"}],
        "source_closure_reuse": str(prior),
    }
    monkeypatch.setattr(pairrun, "preflight_capture", lambda _spec: tmp_path / "pair")
    monkeypatch.setattr(pairrun, "preflight_daemon_socket_paths", lambda *_args, **_kwargs: None)
    monkeypatch.setattr(pairrun, "_run_pair_staged", lambda *_args: {"states": {}})
    calls = []

    def closure(_root, command, path, *, reuse_from=None):
        calls.append((command, reuse_from))
        if command == "reuse":
            path.write_bytes(prior.read_bytes())
        else:
            assert command == "verify"
            raise pairrun.RunError("source closure changed")

    monkeypatch.setattr(pairrun, "_source_closure", closure)
    with pytest.raises(pairrun.RunError, match="source closure changed"):
        pairrun.run_pair(spec)
    assert calls == [("reuse", prior), ("verify", None)]
    assert not (tmp_path / "pair").exists()

    spec["claims"] = {"quality": True}
    with pytest.raises(pairrun.RunError, match="reuse is only valid"):
        pairrun.run_pair(spec)


def test_qualified_verdict_refuses_valid_but_unbound_driver_source_closure(tmp_path):
    st = _pair_stage(tmp_path, scope="qualified")
    closure_path = st["stage"] / "driver-source-closure.json"
    closure = json.loads(closure_path.read_text(encoding="utf-8"))
    closure["files"][0]["sha256"] = _fake_sha("different-source")
    core = {
        key: closure[key] for key in ("schema_version", "profile", "revision", "roots", "files")
    }
    closure["digest"] = ev.digest(ev.canonical(core))
    closure_path.write_text(json.dumps(closure), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert verdict["state_evidence"]["PAIR_VALID"]["reason"] == (
        "driver_source_closure_digest_drift"
    )


def test_qualified_verdict_refuses_receipt_capture_closure_mismatch(tmp_path, monkeypatch):
    monkeypatch.setattr(ev, "MIN_CI_SAMPLE", 2)
    # This fixture exercises receipt binding, independently of concurrent main updates.
    driver_root = Path(__file__).resolve().parents[3]
    real_git_head_sha = pairrun.git_head_sha
    driver_revision = real_git_head_sha(driver_root)
    monkeypatch.setattr(
        pairrun,
        "git_head_sha",
        lambda path: driver_revision
        if path.resolve() == driver_root
        else real_git_head_sha(path),
    )
    st = _pair_stage(tmp_path, blinding="isolated", scope="qualified", claims={"quality": True})
    receipt_path = st["stage"] / "receipts" / "sdk_receipt.json"
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    receipt["source_closure"]["files"][0]["sha256"] = _fake_sha("other-source")
    core = {
        key: receipt["source_closure"][key]
        for key in ("schema_version", "profile", "revision", "roots", "files")
    }
    receipt["source_closure"]["digest"] = ev.digest(ev.canonical(core))
    receipt_path.write_text(json.dumps(receipt), encoding="utf-8")

    admission_path = st["stage"] / "admission" / "admission.json"
    admission = json.loads(admission_path.read_text(encoding="utf-8"))
    admission["verification"]["sdk_receipt_sha256"] = pairrun.sha_file(receipt_path)
    admission_path.write_text(json.dumps(admission), encoding="utf-8")
    admission_digest = pairrun.sha_file(admission_path)
    protocol_path = st["stage"] / "protocol-lock.json"
    protocol = json.loads(protocol_path.read_text(encoding="utf-8"))
    protocol["admission_digest"] = admission_digest
    protocol_path.write_text(json.dumps(protocol), encoding="utf-8")
    _rewrite_manifest(
        st,
        lambda manifest: manifest["provenance"]["admission"].update(
            manifest_digest=admission_digest
        ),
    )
    verdict = _stage_verdict(st)
    assert verdict["states"]["SDK_PATH_GREEN"] == "fail"
    assert verdict["states"]["QUALITY_DELTA"] == "fail"
    assert (
        "source closure differs from capture closure"
        in verdict["state_evidence"]["QUALITY_DELTA"]["reason"]
    )


def test_qualified_contract_refuses_coordinated_receipt_closure_rebind(tmp_path):
    st = _pair_stage(tmp_path, scope="qualified")
    assert _stage_verdict(st)["states"]["CONTRACT_GREEN"] == "pass"
    for side in ("python", "rust"):
        receipt_path = st["stage"] / "receipts" / f"contract_{side}_receipt.json"
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
        receipt["source_closure"]["files"][0]["sha256"] = _fake_sha("other-source")
        core = {
            key: receipt["source_closure"][key]
            for key in ("schema_version", "profile", "revision", "roots", "files")
        }
        receipt["source_closure"]["digest"] = ev.digest(ev.canonical(core))
        receipt_path.write_text(json.dumps(receipt), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["CONTRACT_GREEN"] == "fail"
    assert (
        "execution context source closure mismatch"
        in verdict["state_evidence"]["CONTRACT_GREEN"]["reason"]
    )


def test_isolation_proof_refuses_tampered_frozen_runner_tool(tmp_path, monkeypatch):
    monkeypatch.setattr(ev, "MIN_CI_SAMPLE", 2)
    st = _pair_stage(tmp_path, blinding="isolated", scope="qualified", claims={"quality": True})
    (st["stage"] / "runner-tools.pyz").write_bytes(b"tampered adapter")
    verdict = _stage_verdict(st)
    assert verdict["states"]["QUALITY_DELTA"] == "fail"
    assert "runner bundle digest mismatch" in verdict["state_evidence"]["QUALITY_DELTA"]["reason"]


def test_runtime_manifest_requires_qualified_source_closure_artifact(tmp_path):
    st = _pair_stage(tmp_path, scope="qualified")
    _rewrite_manifest(st, lambda manifest: manifest["artifacts"].pop("driver_source_closure"))
    with pytest.raises(pairrun.RunError, match="driver source closure"):
        _stage_verdict(st)


@pytest.mark.parametrize("policy", ["code_search_file", "natural_language_file"])
def test_qualified_default_file_verdict_requires_request_contract(tmp_path, policy):
    st = _pair_stage(tmp_path, scope="qualified")
    protocol = st["stage"] / "protocol-lock.json"
    payload = json.loads(protocol.read_text(encoding="utf-8"))
    payload["execution_profiles"]["quanta"]["policy"] = policy
    protocol.write_text(json.dumps(payload), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="declared file request mode"):
        _stage_verdict(st)


def test_verdict_refusals(tmp_path):
    st = _pair_stage(tmp_path)
    (st["stage"] / "rep-00" / "semble" / "native.json").unlink()
    with pytest.raises(pairrun.RunError, match="artifact is missing"):
        _stage_verdict(st)
    st = _pair_stage(tmp_path / "suite")
    foreign = st["stage"] / "foreign-suite.json"
    drifted = json.loads(json.dumps(st["suite"]))
    drifted["suite_id"] = "foreign-suite"
    foreign.write_text(json.dumps(drifted), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="CLI suite differs"):
        pairrun.build_verdict(st["repo"], foreign, st["manifest_path"])
    st = _pair_stage(tmp_path / "shape")
    st["manifest_path"].write_text('{"manifest_version": 1}', encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="must hold exactly"):
        _stage_verdict(st)


def test_verdict_cli_smoke(tmp_path):
    st = _pair_stage(tmp_path)
    out = st["stage"] / "cli-verdict.json"
    completed = subprocess.run(
        [
            sys.executable,
            str(Path(pairrun.__file__)),
            "verdict",
            "--repo",
            str(st["repo"]),
            "--suite",
            str(st["suite_path"]),
            "--run-manifest",
            str(st["manifest_path"]),
            "--out",
            str(out),
        ],
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert completed.returncode == 0, completed.stderr
    assert json.loads(out.read_text(encoding="utf-8"))["states"] == _stage_verdict(st)["states"]


def test_successful_promotion_replays_identically_in_new_process(tmp_path):
    st = _pair_stage(tmp_path)
    before = _stage_verdict(st)
    assert before["states"]["PAIR_VALID"] == "pass"
    old_stage = st["stage"]
    promoted = old_stage.with_name("promoted")
    old_stage.rename(promoted)
    output = tmp_path / "promoted-verdict.json"
    completed = subprocess.run(
        [
            sys.executable,
            str(Path(pairrun.__file__)),
            "verdict",
            "--repo",
            str(st["repo"]),
            "--suite",
            str(promoted / st["suite_path"].relative_to(old_stage)),
            "--run-manifest",
            str(promoted / "run-manifest.json"),
            "--out",
            str(output),
        ],
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert completed.returncode == 0, completed.stderr
    after = json.loads(output.read_text(encoding="utf-8"))
    assert after["states"] == before["states"]
    assert after["state_evidence"]["PAIR_VALID"] == before["state_evidence"]["PAIR_VALID"]


def test_pair_refuses_unsupported_python_before_capture(monkeypatch):
    stderr = io.StringIO()
    monkeypatch.setattr(pairrun, "sys", SimpleNamespace(version_info=(3, 9), stderr=stderr))
    assert pairrun.main(["pair", "--spec", "/missing/spec.json"]) == 2
    assert "Python 3.10 or newer" in stderr.getvalue()
    with pytest.raises(pairrun.RunError, match="Python 3.10 or newer"):
        pairrun.run_pair({})


def test_run_pair_promotes_complete_stage_and_public_verdict_replays(tmp_path, monkeypatch, capsys):
    cache, revision = _pinned_semble_cache(tmp_path)
    st = _pair_stage(tmp_path / "fixture")
    original = _stage_verdict(st)
    output_root = tmp_path / "published"
    monkeypatch.setattr(pairrun, "preflight_capture", lambda _spec: output_root)
    # Source custody is exercised separately; this test copies a complete stage.
    monkeypatch.setattr(pairrun, "_source_closure", lambda *_args: None)

    def staged(_spec, stage):
        shutil.copytree(st["stage"], stage, dirs_exist_ok=True)
        return {"status": "staged"}

    monkeypatch.setattr(pairrun, "_run_pair_staged", staged)
    assert (
        pairrun.run_pair(
            {
                "scope": "exploratory",
                "semble_lockfile_sha256": _fake_sha("lock"),
                "semble_python": "python3",
                "semble_lockfile": "lockfile",
                "semble_cache_root": str(cache),
                "semble_model_revision": revision,
                "host_profile": "host-profile",
            }
        )
        == 0
    )
    summary = json.loads(capsys.readouterr().out)
    assert summary["output_root"] == str(output_root)
    outer = summary["driver_outer_ms"]
    assert set(outer) == {
        "preflight",
        "source_closure_capture",
        "staged",
        "source_closure_verify",
        "promotion",
        "total",
    }
    assert all(value >= 0 for value in outer.values())
    assert sum(value for name, value in outer.items() if name != "total") == pytest.approx(
        outer["total"]
    )
    assert not output_root.with_name(output_root.name + ".staging").exists()
    public = subprocess.run(
        [
            sys.executable,
            str(Path(pairrun.__file__)),
            "verdict",
            "--repo",
            str(st["repo"]),
            "--suite",
            str(output_root / st["suite_path"].relative_to(st["stage"])),
            "--run-manifest",
            str(output_root / "run-manifest.json"),
            "--out",
            str(tmp_path / "replayed.json"),
        ],
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert public.returncode == 0, public.stderr
    replayed = json.loads((tmp_path / "replayed.json").read_text())
    assert replayed["states"] == original["states"]
    assert replayed["state_evidence"]["PAIR_VALID"] == original["state_evidence"]["PAIR_VALID"]


def test_pair_staging_atomicity(tmp_path, monkeypatch):
    cache, revision = _pinned_semble_cache(tmp_path)
    repo, suite, _run, _sp, _rp, _files = fixture_v3(tmp_path / "src")
    manifest = tmp_path / "manifest.json"
    manifest.write_text(
        json.dumps({"repository_commit": suite["repository_commit"]}), encoding="utf-8"
    )
    suite_file = tmp_path / "suite.json"
    suite_file.write_text(json.dumps(suite), encoding="utf-8")
    _suite, pack, _source = ev.validate_suite(repo, suite)
    pack_file = tmp_path / "pack.json"
    pack_file.write_text(json.dumps(pack), encoding="utf-8")
    searchd = tmp_path / "searchd"
    searchd.write_bytes(b"searchd")
    lockfile = tmp_path / "semble-lock.txt"
    lockfile.write_bytes(b"semble==0.6.0\n")
    host_profile = tmp_path / "host-profile.json"
    host_profile.write_text(
        json.dumps(
            {
                "schema_version": 2,
                "profile_id": "test-host",
                "fingerprint": {
                    "system": "Darwin",
                    "release": "test",
                    "machine": "arm64",
                    "processor": "test",
                    "cpu_count": 8,
                    "rustc": "rustc test",
                    "power_digest": _fake_sha("power"),
                },
            }
        ),
        encoding="utf-8",
    )
    spec = {
        "spec_version": 2,
        "repo": str(repo),
        "manifest": str(manifest),
        "suite": str(suite_file),
        "query_pack": str(pack_file),
        "execution_profiles": {
            "quanta": qp.execution_profile("native"),
            "semble": semble_adapter.execution_profile("native-default", None),
        },
        "top_k": 10,
        "output_root": str(tmp_path / "out"),
        "runner_binary": "/unused/runner",
        "strategies": [{"name": "whole_file"}],
        "searchd_binary": str(searchd),
        "searchd_expected_sha256": ev.digest(b"searchd"),
        "semble_python": "/unused/python",
        "semble_lockfile": str(lockfile),
        "semble_lockfile_sha256": _fake_sha("lock"),
        "semble_cache_root": str(cache),
        "semble_model_revision": revision,
        "host_profile": str(host_profile),
    }

    def explode(_spec, _spec_dir):
        raise pairrun.RunError("boom")

    # This test owns atomic stage promotion, not Unix socket path admission.
    # Its pytest-generated output path is deliberately long on macOS.
    monkeypatch.setattr(pairrun, "probe_runner_capabilities", lambda _binary: {})
    monkeypatch.setattr(pairrun, "preflight_daemon_socket_paths", lambda *_args, **_kwargs: None)
    monkeypatch.setattr(pairrun, "_source_closure", lambda *_args: None)
    monkeypatch.setattr(pairrun, "run_quanta", explode)
    with pytest.raises(pairrun.RunError, match="boom"):
        pairrun.run_pair(spec)
    assert not (tmp_path / "out").exists()
    stage = tmp_path / "out.staging"
    assert stage.is_dir()
    assert list(stage.rglob("verdict.json")) == []
    assert not (stage / "driver-stage-timings.json").exists()


def test_matrix_floor_no_cross_strategy_inflation():
    cells = [
        {
            "system": "quanta",
            "strategy": "whole_file",
            "rows": [("lexical", f"T{i}", "success", 1.0) for i in range(100)],
            "native_latencies": None,
            "native_route": None,
        },
        {
            "system": "quanta",
            "strategy": "brace_heuristic",
            "rows": [("lexical", f"T{i}", "success", 1.0) for i in range(3)],
            "native_latencies": None,
            "native_route": None,
        },
    ]
    matrix = pairrun.aggregate_matrix(cells, 1)
    assert matrix["floors"] == {
        "quanta:brace_heuristic:lexical": 3,
        "quanta:whole_file:lexical": 100,
    }
    assert matrix["observations_floor"] == 3
    cells = [
        {
            "system": "semble",
            "strategy": "native",
            "rows": [
                ("hybrid", "T1", "success", 2.0),
                ("hybrid", "T2", "abstained", 1.0),
                ("hybrid", "T3", "timeout", 5.0),
                ("hybrid", "T4", "success", None),
            ],
            "native_latencies": {"T1": [2.0, 2.5], "T4": [0.5]},
            "native_route": "hybrid",
        }
    ]
    matrix = pairrun.aggregate_matrix(cells, 1)
    key = "semble:native:hybrid"
    assert matrix["attempts"][key] == 4
    assert matrix["errors"][key] == 1
    assert matrix["nulls"][key] == 1
    assert matrix["abstained"][key] == 1
    assert matrix["floors"][key] == 4
    bad = json.loads(json.dumps(cells))
    bad[0]["native_latencies"]["T1"] = [9.9, 2.5]
    with pytest.raises(pairrun.RunError, match="disagree"):
        pairrun.aggregate_matrix(bad, 1)


def test_matrix_uses_only_shared_warm_samples_and_rejects_first_sample_drift():
    cell = {
        "system": "quanta",
        "strategy": "whole_file",
        "rows": [("hybrid", "T1", "success", 2.0)],
        "warm_latencies": {"hybrid": {"T1": [2.0, 2.5, 3.0]}},
        "cold_latencies": {"hybrid": 99.0},
        "native_latencies": None,
        "native_route": None,
    }
    matrix = pairrun.aggregate_matrix([cell], 1)
    assert matrix["samples"]["quanta:whole_file:hybrid:T1"] == [2.0, 2.5, 3.0]
    assert matrix["observations_floor"] == 3
    assert 99.0 not in matrix["samples"]["quanta:whole_file:hybrid:T1"]

    cell["warm_latencies"]["hybrid"]["T1"][0] = 2.1
    with pytest.raises(pairrun.RunError, match="disagree"):
        pairrun.aggregate_matrix([cell], 1)


def test_receipt_shape_mirrors_canonical_schema():
    canonical = json.loads(
        Path("tools/ci/verification-receipt.schema.json").read_text(encoding="utf-8")
    )
    results = _counts_results("cmd", 2, 2, 2, 0)
    receipt = _receipt(
        "cmd",
        json.dumps(results).encode(),
        "abcdef123456" + "0" * 28,
        "probe",
        {"raw": b"raw evidence"},
    )
    jsonschema.validate(receipt, canonical)
    assert pairrun._validate_receipt_shape(receipt, "probe") == receipt
    for key in ("tier", "test_event_count", "revision"):
        mutant = dict(receipt)
        mutant[key] = "bogus" if key != "test_event_count" else 0
        with pytest.raises(jsonschema.ValidationError):
            jsonschema.validate(mutant, canonical)
        with pytest.raises(pairrun.RunError):
            pairrun._validate_receipt_shape(mutant, "probe")


def test_host_probe_records_without_fabrication():
    probe = pairrun.host_probe()
    for key in (
        "system",
        "machine",
        "cpu_count",
        "python",
        "concurrent_processes",
        "thermal",
        "frequency",
        "power",
    ):
        assert key in probe, f"host probe lacks {key}"


def test_darwin_power_fingerprint_excludes_battery_observation(monkeypatch):
    monkeypatch.setattr(pairrun.sys, "platform", "darwin")
    settings = """Battery Power:
 lidwake              1
 lowpowermode         1
AC Power:
 lidwake              1
 lowpowermode         0
"""
    source = [
        "Now drawing from 'AC Power'\n -InternalBattery-0 80%; charging; 1:00 remaining",
        "Now drawing from 'AC Power'\n -InternalBattery-0 81%; charging; 0:55 remaining",
    ]

    def fake_run(command, **_kwargs):
        if command[-1] == "custom":
            return subprocess.CompletedProcess(command, 0, settings, "")
        return subprocess.CompletedProcess(command, 0, source.pop(0), "")

    monkeypatch.setattr(pairrun.subprocess, "run", fake_run)
    first = pairrun.read_power()
    second = pairrun.read_power()
    assert first["status"] == second["status"] == "bounded"
    assert first["digest"] == second["digest"]
    assert first["observation_digest"] != second["observation_digest"]
    assert first["active_source"] == "AC Power"


def test_darwin_thermal_limits_and_frequency_fail_closed(monkeypatch):
    monkeypatch.setattr(pairrun.sys, "platform", "darwin")
    monkeypatch.setattr(pairrun.os, "cpu_count", lambda: 8)

    def throttled(command, **_kwargs):
        return subprocess.CompletedProcess(
            command,
            0,
            "CPU_Scheduler_Limit = 100\nCPU_Available_CPUs = 8\nCPU_Speed_Limit = 50\n",
            "",
        )

    monkeypatch.setattr(pairrun.subprocess, "run", throttled)
    assert pairrun.read_thermal()["status"] == "warning"
    monkeypatch.setattr(
        pairrun,
        "read_sysctl",
        lambda _keys: {
            "hw.cpufrequency": "unavailable",
            "hw.cpufrequency_max": "unavailable",
        },
    )
    assert pairrun.read_frequency({"status": "bounded"})["status"] == "unavailable"

    monkeypatch.setattr(
        pairrun,
        "read_sysctl",
        lambda _keys: {"hw.cpufrequency": "2400000000", "hw.cpufrequency_max": "3200000000"},
    )
    assert pairrun.read_frequency()["status"] == "bounded"


def test_linux_speed_probe_requires_profile_bounds_and_complete_telemetry():
    governors = {"cpu0": "performance", "cpu1": "performance"}
    settings = {
        "governors": governors,
        "minimum_khz": {"cpu0": 2_700_000, "cpu1": 2_700_000},
        "maximum_khz": {"cpu0": 3_000_000, "cpu1": 3_000_000},
        "drivers": {"cpu0": "intel_pstate", "cpu1": "intel_pstate"},
        "boost": {"/sys/devices/system/cpu/intel_pstate/no_turbo": "1"},
    }
    power_digest = pairrun.digest(pairrun.canonical(settings))
    host = {
        "system": "Linux",
        "release": "test",
        "machine": "x86_64",
        "processor": "test",
        "cpu_count": 2,
        "rustc": "rustc test",
        "concurrent_processes": {"none": []},
        "thermal": {
            "status": "observed",
            "evidence": {
                "thermal_zone0": {"type": "x86_pkg_temp", "temp_millidegrees": 60_000},
            },
        },
        "frequency": {
            "status": "observed",
            "evidence": {
                "cpu0": {"current_khz": 2_800_000, "maximum_khz": 3_000_000},
                "cpu1": {"current_khz": 2_800_000, "maximum_khz": 3_000_000},
            },
        },
        "power": {
            "status": "bounded",
            "digest": power_digest,
            "governors": governors,
            "settings": settings,
        },
    }
    profile = pairrun.validate_host_profile(
        {
            "schema_version": 2,
            "profile_id": "linux-test",
            "fingerprint": pairrun._host_fingerprint(host),
            "linux_limits": {
                "max_thermal_millidegrees": 80_000,
                "min_frequency_percent": 90,
                "thermal_zones": {"thermal_zone0": "x86_pkg_temp"},
                "cpu_max_khz": {"cpu0": 3_000_000, "cpu1": 3_000_000},
            },
        }
    )
    assert pairrun._probe_clean(host, profile)
    for key, value in (
        (
            "thermal",
            {
                "status": "observed",
                "evidence": {
                    "thermal_zone0": {"type": "x86_pkg_temp", "temp_millidegrees": 85_000}
                },
            },
        ),
        (
            "frequency",
            {
                "status": "observed",
                "evidence": {
                    "cpu0": {"current_khz": 2_000_000, "maximum_khz": 3_000_000},
                    "cpu1": {"current_khz": 2_800_000, "maximum_khz": 3_000_000},
                },
            },
        ),
        (
            "power",
            {
                "status": "bounded",
                "digest": _fake_sha("power"),
                "governors": {"cpu0": "performance"},
            },
        ),
    ):
        assert not pairrun._probe_clean({**host, key: value}, profile)
    assert not pairrun._probe_clean(host, {**profile, "linux_limits": {}})


def test_linux_host_profile_rejects_unbounded_policy():
    base = {
        "schema_version": 2,
        "profile_id": "linux-test",
        "fingerprint": {
            "system": "Linux",
            "release": "test",
            "machine": "x86_64",
            "processor": "test",
            "cpu_count": 1,
            "rustc": "rustc test",
            "power_digest": _fake_sha("power"),
        },
        "linux_limits": {
            "max_thermal_millidegrees": 85_001,
            "min_frequency_percent": 80,
            "thermal_zones": {"thermal_zone0": "cpu_thermal"},
            "cpu_max_khz": {"cpu0": 3_000_000},
        },
    }
    with pytest.raises(pairrun.RunError, match="thermal limit"):
        pairrun.validate_host_profile(base)
    with pytest.raises(pairrun.RunError, match="schema version"):
        pairrun.validate_host_profile({**base, "schema_version": 1})


def test_linux_sysfs_presence_alone_never_qualifies_speed(tmp_path, monkeypatch):
    cpu_root = tmp_path / "cpu"
    thermal_root = tmp_path / "thermal"
    thermal_zone = thermal_root / "thermal_zone0"
    thermal_zone.mkdir(parents=True)
    (thermal_zone / "temp").write_text("60000")
    (thermal_zone / "type").write_text("x86_pkg_temp")
    for cpu in ("cpu0",):
        cpufreq = cpu_root / cpu / "cpufreq"
        cpufreq.mkdir(parents=True)
        (cpufreq / "scaling_governor").write_text("performance")
        (cpufreq / "scaling_cur_freq").write_text("2800000")
        (cpufreq / "cpuinfo_max_freq").write_text("3000000")
        (cpufreq / "scaling_min_freq").write_text("2700000")
        (cpufreq / "scaling_max_freq").write_text("3000000")
        (cpufreq / "scaling_driver").write_text("intel_pstate")
    boost_node = tmp_path / "no_turbo"
    boost_node.write_text("1")

    def fake_path(value):
        if value == "/sys/devices/system/cpu":
            return cpu_root
        if value == "/sys/class/thermal":
            return thermal_root
        if value == "/sys/devices/system/cpu/intel_pstate/no_turbo":
            return boost_node
        if value == "/sys/devices/system/cpu/cpufreq/boost":
            return tmp_path / "missing-boost"
        return Path(value)

    monkeypatch.setattr(pairrun.sys, "platform", "linux")
    monkeypatch.setattr(pairrun.os, "cpu_count", lambda: 2)
    monkeypatch.setattr(pairrun, "Path", fake_path)
    assert pairrun.read_power()["status"] == "unavailable"
    assert pairrun.read_frequency()["status"] == "unavailable"
    assert pairrun.read_thermal()["status"] == "observed"

    cpufreq = cpu_root / "cpu1" / "cpufreq"
    cpufreq.mkdir(parents=True)
    (cpufreq / "scaling_governor").write_text("performance")
    (cpufreq / "scaling_cur_freq").write_text("2800000")
    (cpufreq / "cpuinfo_max_freq").write_text("3000000")
    (cpufreq / "scaling_min_freq").write_text("2700000")
    (cpufreq / "scaling_max_freq").write_text("3000000")
    (cpufreq / "scaling_driver").write_text("intel_pstate")
    assert pairrun.read_power()["status"] == "bounded"
    assert pairrun.read_frequency()["status"] == "observed"
    boost_node.write_text("0")
    assert pairrun.read_power()["status"] == "unavailable"
    boost_node.unlink()
    assert pairrun.read_power()["status"] == "unavailable"
    (thermal_zone / "temp").write_text("not-a-temperature")
    assert pairrun.read_thermal()["status"] == "unavailable"


def test_semble_worker_resident_probe_has_native_windows_path(monkeypatch):
    namespace = {"__name__": "worker_template_test"}
    exec(compile(semble_adapter.WORKER_TEMPLATE, "worker.py", "exec"), namespace)
    assert namespace["peak_resident_bytes"]() > 0

    class FakeFunction:
        def __init__(self, callback):
            self.callback = callback

        def __call__(self, *args):
            return self.callback(*args)

    def read_memory(_handle, pointer, _size):
        ctypes.cast(pointer, ctypes.POINTER(ctypes.c_size_t))[1] = 123_456
        return 1

    class FakeDll:
        def __init__(self, name):
            if name == "kernel32":
                self.GetCurrentProcess = FakeFunction(lambda: 42)
            elif name == "psapi":
                self.GetProcessMemoryInfo = FakeFunction(read_memory)
            else:
                raise AssertionError(name)

    monkeypatch.setattr(semble_adapter.sys, "platform", "win32")
    monkeypatch.setattr(ctypes, "WinDLL", lambda name, **_kwargs: FakeDll(name), raising=False)
    assert namespace["peak_resident_bytes"]() == 123_456


@pytest.mark.skipif(
    not pairrun.SANDBOX_EXEC.is_file(),
    reason="macOS Seatbelt backend is unavailable",
)
def test_seatbelt_profile_is_default_deny_allowlist(tmp_path):
    allowed = tmp_path / "allowed.json"
    denied = tmp_path / "unrelated-secret.json"
    allowed.write_text("allowed", encoding="utf-8")
    denied.write_text("secret", encoding="utf-8")
    profile = pairrun._seatbelt_profile([], [str(allowed)], [])
    assert "(deny default)" in profile
    assert "(allow default)" not in profile
    allowed_probe = subprocess.run(
        [str(pairrun.SANDBOX_EXEC), "-p", profile, "/bin/cat", str(allowed)],
        capture_output=True,
        text=True,
    )
    denied_probe = subprocess.run(
        [str(pairrun.SANDBOX_EXEC), "-p", profile, "/bin/cat", str(denied)],
        capture_output=True,
        text=True,
    )
    assert allowed_probe.returncode == 0 and allowed_probe.stdout == "allowed"
    assert denied_probe.returncode != 0 and denied_probe.stdout == ""


@pytest.fixture
def seatbelt_socket_root():
    # Darwin's sockaddr_un path limit is independent of pytest's basetemp depth.
    with tempfile.TemporaryDirectory(prefix="qi-sb-", dir="/tmp") as directory:
        yield Path(directory).resolve()


@pytest.mark.skipif(
    pairrun.platform.system() != "Darwin" or not pairrun.SANDBOX_EXEC.is_file(),
    reason="macOS Seatbelt backend is unavailable",
)
def test_seatbelt_profile_allows_owned_unix_socket_and_child_signal(seatbelt_socket_root):
    tmp_path = seatbelt_socket_root
    socket_path = tmp_path / "allowed.sock"
    denied_socket_path = tmp_path.parent / f"{tmp_path.name}-denied.sock"
    profile = pairrun._seatbelt_profile([], [str(tmp_path)], [str(tmp_path)])
    probe = subprocess.run(
        [
            str(pairrun.SANDBOX_EXEC),
            "-p",
            profile,
            "/usr/bin/ruby",
            "--disable-gems",
            "-rsocket",
            "-e",
            (
                "p=ARGV[0]; server=UNIXServer.new(p); client=UNIXSocket.new(p); "
                "accepted=server.accept; client.write('x'); abort unless accepted.read(1)=='x'"
                "; pid=spawn('/bin/sleep','5'); Process.kill('TERM',pid); Process.wait(pid)"
            ),
            str(socket_path),
        ],
        capture_output=True,
        text=True,
        cwd=tmp_path,
    )
    assert probe.returncode == 0, probe.stderr
    denied_probe = subprocess.run(
        [
            str(pairrun.SANDBOX_EXEC),
            "-p",
            profile,
            "/usr/bin/ruby",
            "--disable-gems",
            "-rsocket",
            "-e",
            "UNIXServer.new(ARGV[0])",
            str(denied_socket_path),
        ],
        capture_output=True,
        text=True,
        cwd=tmp_path,
    )
    assert denied_probe.returncode != 0
    assert not denied_socket_path.exists()


# --- G0: W0-A comparison-contract cutover (schema v3) ---

SCHEMA_DIR = Path(ev.__file__).resolve().parent
G0_SCHEMAS = (
    "runner.schema.json",
    "suite.schema.json",
    "pair-spec.schema.json",
    "admission.schema.json",
    "experiment-custody.schema.json",
    "run-manifest.schema.json",
    "verdict.schema.json",
)


def _load_schema(name: str) -> dict:
    return json.loads((SCHEMA_DIR / name).read_text(encoding="utf-8"))


def _fake_sha(seed: str) -> str:
    return ev.digest(f"g0-seed-{seed}".encode())


def test_g0_schema_files_are_closed():
    """Every object node constrains its values: false or a value schema, never open."""
    for name in G0_SCHEMAS:
        schema = _load_schema(name)
        open_nodes = []

        def walk(node, path="$", open_nodes=open_nodes):
            if isinstance(node, dict):
                if node.get("type") == "object":
                    guard = node.get("additionalProperties")
                    if guard is not False and not isinstance(guard, dict):
                        open_nodes.append(path)
                for key, value in node.items():
                    walk(value, f"{path}.{key}")
            elif isinstance(node, list):
                for index, value in enumerate(node):
                    walk(value, f"{path}[{index}]")

        walk(schema)
        assert not open_nodes, f"{name} has unconstrained objects: {open_nodes}"


def test_g0_receipt_shape_matches_canonical_receipt_schema():
    canonical = json.loads(
        Path("tools/ci/verification-receipt.schema.json").read_text(encoding="utf-8")
    )
    embedded = _load_schema("run-manifest.schema.json")["$defs"]["verification_receipt"]
    for key in ("type", "additionalProperties", "required", "properties", "oneOf"):
        assert embedded[key] == canonical[key], f"receipt $def drifted on {key}"


def test_g0_pair_spec_receipts_are_paths_not_content():
    pair_full = _load_schema("pair-spec.schema.json")
    manifest_full = _load_schema("run-manifest.schema.json")
    pair_receipts = pair_full["properties"]["receipts"]
    manifest_artifacts = manifest_full["properties"]["artifacts"]
    generated = {"contract_execution_logs", "sdk_execution_logs"}
    assert sorted(pair_receipts["properties"]) == sorted(set(pairrun.RECEIPT_KEYS) - generated)
    # Every spec receipt path lands on a manifest artifact of the same name.
    for key in pairrun.RECEIPT_KEYS:
        assert key in manifest_artifacts["properties"], key
        if key not in generated:
            assert pair_receipts["properties"][key] == {"type": "string", "minLength": 1}
    # Evidence content is rejected in the spec: paths only.
    spec = _g0_spec()
    spec["receipts"] = {"contract_python_results": "/tmp/results.json"}
    jsonschema.validate(spec, pair_full)
    spec["receipts"] = {"contract_python_results": {"passed": 10}}
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(spec, pair_full)
    spec["receipts"] = {"pair": {"mapping_proof_clean": True}}
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(spec, pair_full)


def _v3_contract(top_k: int = 10) -> dict:
    return {
        "top_k": top_k,
        "tokenizer": ev.TOKENIZER,
        "tokenizer_budget_version": ev.TOKENIZER_BUDGET_VERSION,
        "output_unit_policy": "rank_prefix",
        "span_unit": ev.SPAN_UNIT,
    }


def _v3_capture(system: str, *, current: bool = False) -> dict:
    base = {
        "system": system,
        "chunk_strategy": "whole_file" if system == "quanta" else "semble_native",
        "chunk_config": {},
        "runner_binary": {"name": f"{system}-runner", "digest": _fake_sha(f"{system}-bin")},
        "generation": 7 if system == "quanta" else 0,
        "receipt_digest": _fake_sha(f"{system}-receipt"),
        "activation_digest": _fake_sha(f"{system}-activation"),
        "model": "lex" if system == "quanta" else "potion-code-16M-v2",
        "model_revision": "r1",
    }
    base["searchd_binary"] = {"binary_digest": _fake_sha("searchd")} if system == "quanta" else None
    if current:
        profile = (
            qp.execution_profile("native")
            if system == "quanta"
            else semble_adapter.execution_profile("native-default", None)
        )
        base["execution_profile"] = profile
        base["execution_profile_sha256"] = ev.digest(ev.canonical(profile))
    return base


def fixture_v3(tmp_path: Path, *, answerable_only: bool = False, blinding: str = "isolated"):
    files = {
        "a.txt": b"alpha one\nalpha two\nalpha three\nalpha four\n",
        "b.txt": b"beta one\nbeta two\n",
        "excluded.txt": b"excluded one\n",
    }
    repo, commit = _write_repo(tmp_path, files)

    def gold(path: str, start: int, end: int, grade: int | None = None):
        file_sha, block_sha, _ = _span_meta(files[path], start, end)
        start_byte, end_byte = _byte_span(files[path], start, end)
        label: dict = {
            "path": path,
            "start_byte": start_byte,
            "end_byte": end_byte,
            "start_line": start,
            "end_line": end,
            "file_sha256": file_sha,
            "block_sha256": block_sha,
        }
        if grade is not None:
            label["grade"] = grade
        return label

    def cand(path: str, start: int, end: int, rank: int):
        file_sha, block_sha, tokens = _span_meta(files[path], start, end)
        start_byte, end_byte = _byte_span(files[path], start, end)
        return {
            "path": path,
            "start_byte": start_byte,
            "end_byte": end_byte,
            "start_line": start,
            "end_line": end,
            "file_sha256": file_sha,
            "block_sha256": block_sha,
            "tokens": tokens,
            "rank": rank,
        }

    q1 = "find alpha two and beta one"
    if answerable_only:
        q2 = "find alpha three"
        tasks = [
            {
                "task_id": "T1",
                "split": "eval",
                "query": q1,
                "query_sha256": ev.digest(q1.encode()),
                "query_family_id": "fam-alpha-beta",
                "answerable": True,
                "category": "symbol",
                "gold": [gold("a.txt", 2, 2, 3), gold("b.txt", 1, 1, 1)],
            },
            {
                "task_id": "T2",
                "split": "eval",
                "query": q2,
                "query_sha256": ev.digest(q2.encode()),
                "query_family_id": "fam-alpha-three",
                "answerable": True,
                "category": "semantic",
                "gold": [gold("a.txt", 3, 3, 2)],
            },
        ]
    else:
        q2 = "find the nonexistent adapter"
        tasks = [
            {
                "task_id": "T1",
                "split": "eval",
                "query": q1,
                "query_sha256": ev.digest(q1.encode()),
                "query_family_id": "fam-alpha-beta",
                "answerable": True,
                "category": "symbol",
                "gold": [gold("a.txt", 2, 2, 3), gold("b.txt", 1, 1, 1)],
            },
            {
                "task_id": "T2",
                "split": "eval",
                "query": q2,
                "query_sha256": ev.digest(q2.encode()),
                "query_family_id": "fam-no-answer",
                "answerable": False,
                "gold": [],
            },
        ]
    universe_entries = [
        {"path": "a.txt", "file_sha256": ev.digest(files["a.txt"])},
        {"path": "b.txt", "file_sha256": ev.digest(files["b.txt"])},
    ]
    suite = {
        "schema_version": 3,
        "suite_id": "fixture-v3",
        "repository_commit": commit,
        "comparison_contract": _v3_contract(),
        "routes": ["lexical", "hybrid"],
        "file_universe": universe_entries,
        "file_universe_digest": ev.universe_digest(universe_entries),
        "tasks": tasks,
    }
    _, pack, _ = ev.validate_suite(repo, suite)

    # RBR-02: current records are v5 — every result binds the query
    # identity of the single native plan shared by all routes.
    identities = {
        task["task_id"]: qp.derive_query_identity("native", task["query"]) for task in pack["tasks"]
    }

    def result(task_id, route, status, spans, latency=1.5, error=None):
        return {
            "task_id": task_id,
            "route": route,
            "status": status,
            "candidates": [cand(p, s, e, i + 1) for i, (p, s, e) in enumerate(spans)],
            "query_identity": identities[task_id],
            "timings": {"query_latency_ms": latency},
            "error": error,
        }

    if answerable_only:
        results = [
            result("T1", "lexical", "success", [("a.txt", 3, 3), ("b.txt", 1, 1)]),
            result("T1", "hybrid", "success", [("a.txt", 2, 2), ("a.txt", 3, 3), ("b.txt", 1, 1)]),
            result("T2", "lexical", "success", [("a.txt", 3, 3)]),
            result("T2", "hybrid", "success", [("a.txt", 4, 4), ("a.txt", 3, 3)]),
        ]
    else:
        results = [
            result("T1", "lexical", "success", [("a.txt", 3, 3), ("b.txt", 1, 1)]),
            result("T1", "hybrid", "success", [("a.txt", 2, 2), ("a.txt", 3, 3), ("b.txt", 1, 1)]),
            result("T2", "lexical", "success", [("a.txt", 1, 1)]),
            result("T2", "hybrid", "abstained", []),
        ]
    run = {
        "schema_version": 5,
        "query_pack_sha256": ev.digest(ev.canonical(pack)),
        "comparison_contract": _v3_contract(),
        "runner": {
            "name": "recorded-search-runner",
            "revision": "runner@abc",
            "run_id": "run-v3-1",
            "tokenizer": ev.TOKENIZER,
            "tokenizer_budget_version": ev.TOKENIZER_BUDGET_VERSION,
            "gold_access": False,
            "blinding": blinding,
            "isolation_method": "separate suite access; runner cannot read suite path",
            "access_block_log": "verified EACCES on suite path for runner uid",
        },
        "captures": {"q0": _v3_capture("quanta", current=True)},
        "route_provenance": {
            "lexical": {"capture_id": "q0"},
            "hybrid": {"capture_id": "q0"},
        },
        "results": results,
    }
    suite_path = tmp_path / "suite_v3.json"
    runner_path = tmp_path / "run_v3.json"
    return repo, suite, run, suite_path, runner_path, files


def record_v3(repo, suite, run, suite_path, runner_path):
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    runner_path.write_text(json.dumps(run), encoding="utf-8")
    return ev.load_evidence(repo, suite_path, runner_path)


def test_cross_suite_experiment_custody_rejects_file_family_query_and_digest_leakage(tmp_path):
    repo, suite, _run, _sp, _rp, files = fixture_v3(tmp_path)
    development = json.loads(json.dumps(suite))
    holdout = json.loads(json.dumps(suite))
    development["suite_id"] = "development"
    holdout["suite_id"] = "holdout"
    development["file_universe"] = [suite["file_universe"][0]]
    holdout["file_universe"] = [suite["file_universe"][1]]
    development["file_universe_digest"] = ev.universe_digest(development["file_universe"])
    holdout["file_universe_digest"] = ev.universe_digest(holdout["file_universe"])
    development["tasks"] = [json.loads(json.dumps(suite["tasks"][0]))]
    development["tasks"][0]["gold"] = [suite["tasks"][0]["gold"][0]]
    holdout["tasks"] = [json.loads(json.dumps(suite["tasks"][0]))]
    holdout["tasks"][0]["task_id"] = "H1"
    holdout["tasks"][0]["query"] = "Find the beta implementation in b.txt"
    holdout["tasks"][0]["query_sha256"] = ev.digest(holdout["tasks"][0]["query"].encode())
    holdout["tasks"][0]["query_family_id"] = "holdout-beta"
    holdout["tasks"][0]["gold"] = [suite["tasks"][0]["gold"][1]]

    def custody(dev, held):
        return {
            "schema_version": 1,
            "source_revision": "a" * 40,
            "repository_commit": suite["repository_commit"],
            "development_suite_sha256": ev.digest(ev.canonical(dev)),
            "holdout_suite_sha256": ev.digest(ev.canonical(held)),
        }

    frozen = custody(development, holdout)
    jsonschema.validate(frozen, _load_schema("experiment-custody.schema.json"))
    assert ev.validate_experiment_custody(repo, frozen, development, holdout) == frozen
    with pytest.raises(ev.EvidenceError, match="unsupported experiment custody schema"):
        ev.validate_experiment_custody(
            repo, dict(frozen, schema_version=True), development, holdout
        )
    shared_development = json.loads(json.dumps(development))
    shared_holdout = json.loads(json.dumps(holdout))
    for split_suite in (shared_development, shared_holdout):
        split_suite["file_universe"] = suite["file_universe"]
        split_suite["file_universe_digest"] = suite["file_universe_digest"]
    ev.validate_experiment_custody(
        repo, custody(shared_development, shared_holdout), shared_development, shared_holdout
    )

    same_file = json.loads(json.dumps(holdout))
    same_file["file_universe"] = development["file_universe"]
    same_file["file_universe_digest"] = development["file_universe_digest"]
    file_sha, block_sha, _ = _span_meta(files["a.txt"], 3, 3)
    byte_start, byte_end = _byte_span(files["a.txt"], 3, 3)
    same_file["tasks"][0]["gold"] = [
        {
            "path": "a.txt",
            "start_byte": byte_start,
            "end_byte": byte_end,
            "start_line": 3,
            "end_line": 3,
            "file_sha256": file_sha,
            "block_sha256": block_sha,
            "grade": 3,
        }
    ]
    with pytest.raises(ev.EvidenceError, match="cross-suite file leakage"):
        ev.validate_experiment_custody(
            repo, custody(development, same_file), development, same_file
        )

    same_family = json.loads(json.dumps(holdout))
    same_family["tasks"][0]["query_family_id"] = development["tasks"][0]["query_family_id"]
    with pytest.raises(ev.EvidenceError, match="cross-suite query family leakage"):
        ev.validate_experiment_custody(
            repo, custody(development, same_family), development, same_family
        )

    same_query = json.loads(json.dumps(holdout))
    same_query["tasks"][0]["query"] = development["tasks"][0]["query"]
    same_query["tasks"][0]["query_sha256"] = development["tasks"][0]["query_sha256"]
    with pytest.raises(ev.EvidenceError, match="query leakage/duplication"):
        ev.validate_experiment_custody(
            repo, custody(development, same_query), development, same_query
        )

    swapped = dict(frozen, development_suite_sha256=frozen["holdout_suite_sha256"])
    with pytest.raises(ev.EvidenceError, match="development suite differs"):
        ev.validate_experiment_custody(repo, swapped, development, holdout)


def test_v3_freeze_pack_carries_contract_and_is_blind(tmp_path):
    repo, suite, _run, _sp, _rp, _files = fixture_v3(tmp_path)
    _, pack, _ = ev.validate_suite(repo, suite)
    assert pack["schema_version"] == 3
    assert pack["comparison_contract"] == suite["comparison_contract"]
    assert sorted(pack) == [
        "comparison_contract",
        "file_universe",
        "file_universe_digest",
        "repository_commit",
        "routes",
        "schema_version",
        "suite_commitment_sha256",
        "suite_id",
        "tasks",
        "tokenizer",
        "tokenizer_budget_version",
    ]
    assert pack["file_universe_digest"] == suite["file_universe_digest"]
    for task in pack["tasks"]:
        assert sorted(task) == ["query", "query_sha256", "task_id"]
    rendered = json.dumps(pack)
    assert "answerable" not in rendered and "gold" not in rendered and "grade" not in rendered


def test_v3_load_evaluate_roundtrip_and_schema_conformance(tmp_path):
    repo, suite, run, suite_path, runner_path, _files = fixture_v3(tmp_path)
    jsonschema.validate(suite, _load_schema("suite.schema.json"))
    jsonschema.validate(run, _load_schema("runner.schema.json"))
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    report = ev.evaluate(loaded_suite, pack, loaded_run, "lexical", "hybrid")
    assert report["schema_version"] == 3
    assert report["comparison_contract"] == suite["comparison_contract"]
    assert report["captures"] == run["captures"]
    assert report["route_provenance"] == run["route_provenance"]
    assert report["sample_count"] == 2


def test_v5_runner_schema_binds_query_identity_and_keeps_v3_v4_historical(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    jsonschema.validate(run, _load_schema("runner.schema.json"))
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    assert loaded_run["schema_version"] == 5
    assert loaded_run["captures"]["q0"]["execution_profile"] == qp.execution_profile("native")

    # New producer evidence is atomic at record level, including routes with
    # empty candidate lists. Historical v5 artifacts without the marker remain
    # readable but cannot claim the new indexed-span diagnostic.
    witnessed = json.loads(json.dumps(run))
    witnessed["span_accounting_version"] = 1
    for row in witnessed["results"]:
        for candidate in row["candidates"]:
            candidate["span_accounting"] = {
                "unit_kind": "chunk",
                "unit_id": f"{row['task_id']}:{row['route']}:{candidate['rank']}",
                "producer_identity": "whole_file",
                "indexed_start_byte": candidate["start_byte"],
                "indexed_end_byte": candidate["end_byte"],
                "sdk_start_line": candidate["start_line"],
                "sdk_end_line": candidate["end_line"],
                "extra_context_bytes": 0,
            }
    jsonschema.validate(witnessed, _load_schema("runner.schema.json"))
    observed_suite, observed_pack, observed_run = record_v3(
        repo, suite, witnessed, suite_path, runner_path
    )
    observed = ev.evaluate(observed_suite, observed_pack, observed_run, "lexical", "hybrid")
    assert observed["span_accounting"]["routes"]["lexical"]["status"] == "observed"
    for mutation, match in (
        (lambda r: r.pop("span_accounting_version"), "lacks record protocol"),
        (
            lambda r: r["results"][0]["candidates"][0].pop("span_accounting"),
            "missing published-unit",
        ),
        (
            lambda r: r["results"][0]["candidates"][0]["span_accounting"].update(
                producer_identity="forged"
            ),
            "producer differs",
        ),
    ):
        tampered = json.loads(json.dumps(witnessed))
        mutation(tampered)
        with pytest.raises(ev.EvidenceError, match=match):
            record_v3(repo, suite, tampered, suite_path, runner_path)

    # v3 records stay loadable immutable history (no policy identity).
    historical = json.loads(json.dumps(run))
    historical["schema_version"] = 3
    for capture in historical["captures"].values():
        capture.pop("execution_profile")
        capture.pop("execution_profile_sha256")
    for row in historical["results"]:
        row.pop("query_identity")
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    runner_path.write_text(json.dumps(historical), encoding="utf-8")
    _, _, legacy_run = ev.load_evidence(repo, suite_path, runner_path)
    assert legacy_run["schema_version"] == 3

    historical_v4 = json.loads(json.dumps(run))
    historical_v4["schema_version"] = 4
    for capture in historical_v4["captures"].values():
        capture.pop("execution_profile")
        capture.pop("execution_profile_sha256")
    historical_v4["runner"]["query_input_policy"] = {
        "policy": "native",
        "config": {},
        "policy_config_sha256": ev.digest(qp.policy_config_canonical_v4("native").encode()),
        "planning_cost_in_latency": False,
    }
    runner_path.write_text(json.dumps(historical_v4), encoding="utf-8")
    _, _, legacy_v4_run = ev.load_evidence(repo, suite_path, runner_path)
    assert legacy_v4_run["schema_version"] == 4

    # Unknown future stamps still refuse.
    future = json.loads(json.dumps(run))
    future["schema_version"] = 6
    runner_path.write_text(json.dumps(future), encoding="utf-8")
    with pytest.raises(ev.EvidenceError, match="unsupported runner schema"):
        ev.load_evidence(repo, suite_path, runner_path)


def test_v5_capture_schema_rejects_zero_generation_and_cross_system_profiles(tmp_path):
    _repo, _suite, run, _suite_path, _runner_path, _ = fixture_v3(tmp_path)
    schema = _load_schema("runner.schema.json")

    zero_generation = json.loads(json.dumps(run))
    zero_generation["captures"]["q0"]["generation"] = 0
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(zero_generation, schema)
    with pytest.raises(ev.EvidenceError, match="positive for quanta"):
        ev.validate_capture(zero_generation["captures"]["q0"], "capture", version=5)

    for system, wrong_profile in (
        ("quanta", semble_adapter.execution_profile("native-default", None)),
        ("semble", qp.execution_profile("native")),
    ):
        capture = _v3_capture(system, current=True)
        capture["execution_profile"] = wrong_profile
        capture["execution_profile_sha256"] = ev.digest(ev.canonical(wrong_profile))
        malformed = json.loads(json.dumps(run))
        malformed["captures"]["q0"] = capture
        with pytest.raises(jsonschema.ValidationError):
            jsonschema.validate(malformed, schema)
        with pytest.raises(ev.EvidenceError):
            ev.validate_capture(capture, "capture", version=5)


@pytest.mark.parametrize(
    "mutation,match",
    [
        (
            lambda s, r, f: r["captures"]["q0"]["execution_profile"].update(
                policy="natural_language"
            ),
            "frozen Quanta profile",
        ),
        (
            lambda s, r, f: r["captures"]["q0"]["execution_profile"].update(policy="telepathy"),
            "policy is unknown",
        ),
        (
            lambda s, r, f: r["captures"]["q0"].update(execution_profile_sha256="0" * 64),
            "execution_profile_sha256 mismatch",
        ),
        (
            lambda s, r, f: r["captures"]["q0"]["execution_profile"].update(
                planning_cost_in_latency=True
            ),
            "frozen Quanta profile",
        ),
        (
            lambda s, r, f: r["captures"]["q0"]["execution_profile"].update(
                config={"max_tokens": 4}
            ),
            "frozen Quanta profile",
        ),
        (lambda s, r, f: r["captures"]["q0"].pop("execution_profile"), "missing/unknown fields"),
        (lambda s, r, f: r["results"][0].pop("query_identity"), "missing fields"),
        (
            lambda s, r, f: r["results"][0]["query_identity"].update(
                effective_lexical_request_sha256="0" * 64
            ),
            "does not match the independently re-derived plan",
        ),
        (
            lambda s, r, f: r["results"][0]["query_identity"].update(
                original_query_sha256="0" * 64
            ),
            "does not match the independently re-derived plan",
        ),
        (
            lambda s, r, f: r["results"][0]["query_identity"].update(semantic_text_sha256="b" * 64),
            "does not match the independently re-derived plan",
        ),
    ],
)
def test_v5_query_identity_tampering_is_rejected(tmp_path, mutation, match):
    repo, suite, run, suite_path, runner_path, files = fixture_v3(tmp_path)
    mutation(suite, run, files)
    with pytest.raises(ev.EvidenceError, match=match):
        record_v3(repo, suite, run, suite_path, runner_path)


def test_v5_literal_and_nl_policies_replay_through_the_python_oracle(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    _, pack, _ = ev.validate_suite(repo, suite)
    # Re-stamp the record as a literal-policy capture: identity digests are
    # re-derived by the evaluator's independent Python planner.
    profile = qp.execution_profile("literal")
    run["captures"]["q0"]["execution_profile"] = profile
    run["captures"]["q0"]["execution_profile_sha256"] = ev.digest(ev.canonical(profile))
    for row in run["results"]:
        query = next(t["query"] for t in pack["tasks"] if t["task_id"] == row["task_id"])
        row["query_identity"] = qp.derive_query_identity("literal", query)
    _suite, _pack, loaded = record_v3(repo, suite, run, suite_path, runner_path)
    assert loaded["captures"]["q0"]["execution_profile"]["policy"] == "literal"

    # An effective-request digest that does not match the literal plan
    # (here: natural-language plan digests) is rejected.
    for row in run["results"]:
        query = next(t["query"] for t in pack["tasks"] if t["task_id"] == row["task_id"])
        row["query_identity"] = qp.derive_query_identity("natural_language", query)
    profile = qp.execution_profile("natural_language")
    run["captures"]["q0"]["execution_profile"] = profile
    run["captures"]["q0"]["execution_profile_sha256"] = ev.digest(ev.canonical(profile))
    # The NL plan digests now agree, so this must load: the oracle accepts
    # any of the three canonical policies with self-consistent evidence.
    _suite, _pack, loaded_nl = record_v3(repo, suite, run, suite_path, runner_path)
    assert loaded_nl["captures"]["q0"]["execution_profile"]["policy"] == "natural_language"


def test_exact_symbol_profile_refuses_lexical_route_record(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    profile = qp.execution_profile("exact_symbol_name")
    run["captures"]["q0"]["execution_profile"] = profile
    run["captures"]["q0"]["execution_profile_sha256"] = ev.digest(ev.canonical(profile))
    with pytest.raises(ev.EvidenceError, match="requires only the symbol route"):
        record_v3(repo, suite, run, suite_path, runner_path)


def test_exact_symbol_profile_refuses_mixed_route_record(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    suite["routes"] = ["symbol", "lexical"]
    for task, name in zip(suite["tasks"], ["AlphaOne", "BetaTwo"], strict=True):
        task["query"] = name
        task["query_sha256"] = ev.digest(name.encode())
    _, pack, _ = ev.validate_suite(repo, suite)
    run["query_pack_sha256"] = ev.digest(ev.canonical(pack))
    exact = qp.execution_profile("exact_symbol_name")
    run["captures"]["q0"]["execution_profile"] = exact
    run["captures"]["q0"]["execution_profile_sha256"] = ev.digest(ev.canonical(exact))
    run["captures"]["q1"] = copy.deepcopy(run["captures"]["q0"])
    native = qp.execution_profile("native")
    run["captures"]["q1"]["execution_profile"] = native
    run["captures"]["q1"]["execution_profile_sha256"] = ev.digest(ev.canonical(native))
    run["route_provenance"] = {
        "symbol": {"capture_id": "q0"},
        "lexical": {"capture_id": "q1"},
    }
    queries = {task["task_id"]: task["query"] for task in pack["tasks"]}
    for result in run["results"]:
        if result["route"] == "hybrid":
            result["route"] = "symbol"
        policy = "exact_symbol_name" if result["route"] == "symbol" else "native"
        result["query_identity"] = qp.derive_query_identity(policy, queries[result["task_id"]])
    with pytest.raises(ev.EvidenceError, match="requires only the symbol route"):
        record_v3(repo, suite, run, suite_path, runner_path)


@pytest.mark.parametrize("old_version", [1, 2])
def test_current_refuses_old_or_unknown_artifact_stamps(tmp_path, old_version):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    old_suite = dict(suite, schema_version=old_version)
    with pytest.raises(ev.EvidenceError, match="unsupported suite schema"):
        ev.validate_suite(repo, old_suite)
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(old_suite, _load_schema("suite.schema.json"))

    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    old_run = dict(run, schema_version=old_version)
    runner_path.write_text(json.dumps(old_run), encoding="utf-8")
    with pytest.raises(ev.EvidenceError, match="unsupported runner schema"):
        ev.load_evidence(repo, suite_path, runner_path)
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(old_run, _load_schema("runner.schema.json"))


def test_current_refuses_legacy_shape_even_with_current_stamp(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    old_suite = {
        "schema_version": ev.SCHEMA_VERSION,
        "suite_id": suite["suite_id"],
        "repository_commit": suite["repository_commit"],
        "routes": suite["routes"],
        "tasks": suite["tasks"],
    }
    with pytest.raises(ev.EvidenceError, match="missing fields"):
        ev.validate_suite(repo, old_suite)
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    old_run = {
        "schema_version": ev.SCHEMA_VERSION,
        "query_pack_sha256": run["query_pack_sha256"],
        "runner": run["runner"],
        "results": run["results"],
    }
    runner_path.write_text(json.dumps(old_run), encoding="utf-8")
    with pytest.raises(ev.EvidenceError, match="missing fields"):
        ev.load_evidence(repo, suite_path, runner_path)


def test_v3_refuses_unknown_fields(tmp_path):
    repo, suite, run, suite_path, runner_path, _files = fixture_v3(tmp_path)

    def attempt(mutator, match):
        mutated_suite, mutated_run = mutator(
            json.loads(json.dumps(suite)), json.loads(json.dumps(run))
        )
        suite_path.write_text(json.dumps(mutated_suite), encoding="utf-8")
        runner_path.write_text(json.dumps(mutated_run), encoding="utf-8")
        with pytest.raises(ev.EvidenceError, match=match):
            ev.load_evidence(repo, suite_path, runner_path)

    attempt(lambda s, r: (dict(s, smuggled=1), r), "missing/unknown fields")
    attempt(
        lambda s, r: (dict(s, comparison_contract=dict(s["comparison_contract"], smuggled=1)), r),
        "missing/unknown fields",
    )
    attempt(lambda s, r: (s, dict(r, smuggled=1)), "missing/unknown fields")
    attempt(
        lambda s, r: (s, dict(r, runner=dict(r["runner"], smuggled=1))), "missing/unknown fields"
    )

    def capture_extra(s, r):
        r["captures"]["q0"]["smuggled"] = 1
        return s, r

    attempt(capture_extra, "missing/unknown fields")

    def config_extra(s, r):
        r["captures"]["q0"]["chunk_config"]["smuggled"] = 1
        return s, r

    attempt(config_extra, "missing/unknown fields")

    def route_model_echo(s, r):
        # Model facts live in the capture; a route-level echo is an unknown field.
        r["route_provenance"]["lexical"]["model"] = "lex"
        return s, r

    attempt(route_model_echo, "missing/unknown fields")

    def result_extra(s, r):
        r["results"][0]["smuggled"] = 1
        return s, r

    attempt(result_extra, "missing/unknown fields")

    def timings_extra(s, r):
        r["results"][0]["timings"]["smuggled"] = 1
        return s, r

    attempt(timings_extra, "missing/unknown fields")


def test_v3_nullable_timing_semantics(tmp_path):
    from tools.benchmark.retrieval.finite_json import is_finite_json_number

    for value in (10**400, -(10**400), float("nan"), float("inf"), -float("inf"), True, None):
        assert not is_finite_json_number(value)
        with pytest.raises(ev.EvidenceError, match="finite number"):
            ev.finite_timing(value, "probe")
        if value is not None:
            with pytest.raises(pairrun.RunError):
                pairrun._sample_value(value, "probe")
    assert is_finite_json_number(sys.float_info.max)
    assert is_finite_json_number(-sys.float_info.max)
    assert not is_finite_json_number(int(sys.float_info.max) + 1)
    assert not is_finite_json_number(-int(sys.float_info.max) - 1)
    assert ev.finite_timing(1, "probe") == 1.0
    assert ev.finite_timing(0.5, "probe") == 0.5
    assert pairrun._sample_value(None, "probe") is None

    repo, suite, run, suite_path, runner_path, _files = fixture_v3(tmp_path, answerable_only=True)
    run["results"][0]["timings"]["query_latency_ms"] = None
    run["results"][1]["timings"]["query_latency_ms"] = 0
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    report = ev.evaluate(loaded_suite, pack, loaded_run, "lexical", "hybrid")
    lexical_mean = report["budgets"]["2000"]["routes"]["lexical"]["mean_query_latency_ms"]
    assert lexical_mean == pytest.approx(1.5)
    hybrid_mean = report["budgets"]["2000"]["routes"]["hybrid"]["mean_query_latency_ms"]
    assert hybrid_mean == pytest.approx(0.75)
    rows = {(row["task_id"], row["route"]): row for row in report["per_query"]}
    assert rows[("T1", "lexical")]["query_latency_ms"] is None
    assert rows[("T1", "hybrid")]["query_latency_ms"] == 0
    with pytest.raises(ev.EvidenceError, match="timing must be a finite number"):
        ev.nullable_timing(float("inf"), "probe")
    with pytest.raises(ev.EvidenceError, match="timing must be a finite number"):
        ev.nullable_timing("1.5", "probe")
    with pytest.raises(ev.EvidenceError, match="timing must be a finite number"):
        ev.nullable_timing(-1, "probe")
    # All-null latencies report honestly instead of inventing a zero mean.
    for row in run["results"]:
        row["timings"]["query_latency_ms"] = None
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    report = ev.evaluate(loaded_suite, pack, loaded_run, "lexical", "hybrid")
    assert (
        report["budgets"]["2000"]["routes"]["lexical"]["mean_query_latency_ms"] == "not_applicable"
    )


def test_v3_capture_reference_integrity(tmp_path):
    repo, suite, run, suite_path, runner_path, _files = fixture_v3(tmp_path)

    def attempt(mutator, match):
        mutated = json.loads(json.dumps(run))
        mutator(mutated)
        suite_path.write_text(json.dumps(suite), encoding="utf-8")
        runner_path.write_text(json.dumps(mutated), encoding="utf-8")
        with pytest.raises(ev.EvidenceError, match=match):
            ev.load_evidence(repo, suite_path, runner_path)

    def dangling(run):
        run["route_provenance"]["lexical"]["capture_id"] = "ghost"

    attempt(dangling, "unknown capture_id")
    attempt(lambda r: r.update(captures={}), "captures must be a nonempty object")

    def semble_searchd(run):
        run["captures"]["q0"] = _v3_capture("semble", current=True)
        run["captures"]["q0"]["searchd_binary"] = {"binary_digest": _fake_sha("x")}

    attempt(semble_searchd, "must be null for semble captures")

    def semble_generation(run):
        run["captures"]["q0"] = _v3_capture("semble", current=True)
        run["captures"]["q0"]["generation"] = 7

    attempt(semble_generation, "must be 0 for semble captures")

    def quanta_searchd_null(run):
        run["captures"]["q0"]["searchd_binary"] = None

    attempt(quanta_searchd_null, "must be an object")

    def legacy_syntax(run):
        run["captures"]["q0"]["chunk_strategy"] = "syntax"

    attempt(legacy_syntax, "not a frozen v3 strategy")

    def legacy_fixed_window(run):
        run["captures"]["q0"]["chunk_strategy"] = "fixed_window"

    attempt(legacy_fixed_window, "not a frozen v3 strategy")

    def bad_system(run):
        run["captures"]["q0"]["system"] = "other-engine"

    attempt(bad_system, "must be quanta or semble")

    def bad_binary_digest(run):
        run["captures"]["q0"]["runner_binary"]["digest"] = "unresolved"

    attempt(bad_binary_digest, "must be a lowercase sha256")

    def bad_receipt_digest(run):
        run["captures"]["q0"]["receipt_digest"] = "0" * 63

    attempt(bad_receipt_digest, "must be a lowercase sha256")


def test_v3_contract_binding_per_field(tmp_path):
    repo, suite, run, suite_path, runner_path, _files = fixture_v3(tmp_path)
    # top_k drift between record and pack refuses the record.
    drifted = json.loads(json.dumps(run))
    drifted["comparison_contract"]["top_k"] = 5
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    runner_path.write_text(json.dumps(drifted), encoding="utf-8")
    with pytest.raises(ev.EvidenceError, match="comparison contract differs"):
        ev.load_evidence(repo, suite_path, runner_path)
    # Const fields cannot drift silently: any deviation fails contract validation.
    for field, value in (
        ("tokenizer", "other-tok"),
        ("tokenizer_budget_version", "qb-v9"),
        ("output_unit_policy", "rank_prefix_extended"),
        ("span_unit", "line_span_v1"),
    ):
        mutated = json.loads(json.dumps(run))
        mutated["comparison_contract"][field] = value
        runner_path.write_text(json.dumps(mutated), encoding="utf-8")
        with pytest.raises(ev.EvidenceError, match="record.comparison_contract"):
            ev.load_evidence(repo, suite_path, runner_path)
    # The suite side is pinned too: a doctored suite changes the pack,
    # so the pack binding fires before contract comparison is even reached.
    mutated_suite = json.loads(json.dumps(suite))
    mutated_suite["comparison_contract"]["top_k"] = 5
    suite_path.write_text(json.dumps(mutated_suite), encoding="utf-8")
    runner_path.write_text(json.dumps(run), encoding="utf-8")
    with pytest.raises(ev.EvidenceError, match="query pack hash mismatch"):
        ev.load_evidence(repo, suite_path, runner_path)


def _g0_spec() -> dict:
    return {
        "spec_version": 2,
        "repo": "/tmp/repo",
        "manifest": "/tmp/manifest.json",
        "suite": "/tmp/suite.json",
        "query_pack": "/tmp/pack.json",
        "execution_profiles": {
            "quanta": qp.execution_profile("native"),
            "semble": semble_adapter.execution_profile("native-default", None),
        },
        "top_k": 10,
        "output_root": "/tmp/out",
        "runner_binary": "/tmp/runner",
        "strategies": [{"name": "whole_file"}],
        "searchd_binary": "/tmp/searchd",
        "searchd_expected_sha256": _fake_sha("searchd"),
        "semble_python": "/tmp/venv/bin/python",
        "semble_lockfile": "/tmp/semble-lock.txt",
        "semble_lockfile_sha256": _fake_sha("lock"),
        "cache_regime": "true_process_cold",
        "claims": {"quality": False, "speed": False, "same_model": False, "incremental": False},
    }


def test_v3_pair_spec_schema():
    schema = _load_schema("pair-spec.schema.json")
    jsonschema.validate(_g0_spec(), schema)
    reused = {**_g0_spec(), "source_closure_reuse": "/tmp/prior-closure.json"}
    jsonschema.validate(reused, schema)
    for invalid_claim in ("quality", "speed", "same_model", "incremental"):
        claimed = json.loads(json.dumps(reused))
        claimed["claims"][invalid_claim] = True
        with pytest.raises(jsonschema.ValidationError):
            jsonschema.validate(claimed, schema)
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate({**reused, "scope": "qualified"}, schema)

    def invalid(mutator):
        spec = json.loads(json.dumps(_g0_spec()))
        mutator(spec)
        with pytest.raises(jsonschema.ValidationError):
            jsonschema.validate(spec, schema)

    invalid(lambda s: s.update(runner_revision="derived-is-forbidden"))
    invalid(lambda s: s["claims"].update(speed="false"))
    invalid(lambda s: s.pop("searchd_expected_sha256"))
    invalid(lambda s: s["strategies"].append({"name": "syntax"}))
    invalid(lambda s: s.update(evidence={"pair": {"mapping_proof_clean": True}}))
    invalid(lambda s: s.update(top_k=0))
    invalid(lambda s: s.update(semble_lockfile=""))
    invalid(lambda s: s.update(cache_regime="lukewarm"))
    invalid(lambda s: s.update(semble_repetitions=2))
    invalid(lambda s: s.update(semble_warmup_passes=0))

    qualified = _g0_spec()
    qualified["scope"] = "qualified"
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(qualified, schema)
    qualified["admission"] = {
        "manifest": "/tmp/admission.json",
        "experiment_custody": "/tmp/experiment-custody.json",
        "development_suite": "/tmp/development-suite.json",
        "license_receipt": "/tmp/license.json",
        "annotation_receipts": ["/tmp/a.json", "/tmp/b.json"],
        "adjudication_receipt": "/tmp/adjudication.json",
    }
    jsonschema.validate(qualified, schema)

    diagnostic_v2 = _g0_spec()
    diagnostic_v2["embedder"] = "potion-code-full-v2"
    jsonschema.validate(diagnostic_v2, schema)
    diagnostic_v2["scope"] = "qualified"
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(diagnostic_v2, schema)
    diagnostic_v2["scope"] = "exploratory"
    diagnostic_v2["claims"]["quality"] = True
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(diagnostic_v2, schema)


def test_v3_spec_accepts_lockfile_path(tmp_path):
    spec_path = tmp_path / "spec.json"
    spec_path.write_text(json.dumps(_g0_spec()), encoding="utf-8")
    loaded = pairrun.load_spec(spec_path)
    assert loaded["semble_lockfile"] == "/tmp/semble-lock.txt"
    diagnostic_v2 = _g0_spec()
    diagnostic_v2["embedder"] = "potion-code-full-v2"
    spec_path.write_text(json.dumps(diagnostic_v2), encoding="utf-8")
    assert pairrun.load_spec(spec_path)["embedder"] == "potion-code-full-v2"
    for mutation in (
        {"scope": "qualified"},
        {"claims": {**diagnostic_v2["claims"], "quality": True}},
        {"claims": {**diagnostic_v2["claims"], "speed": True}},
        {"claims": {**diagnostic_v2["claims"], "same_model": True}},
    ):
        spec_path.write_text(json.dumps({**diagnostic_v2, **mutation}), encoding="utf-8")
        with pytest.raises(pairrun.RunError, match="exploratory diagnostic only"):
            pairrun.load_spec(spec_path)
    assert (
        pairrun.hybrid_fetch_policy_configuration(
            loaded.get("experimental_hybrid_fetch_floor", "100")
        )["floor"]
        == 100
    )
    for floor in ("25", "50", "100"):
        observed = {**_g0_spec(), "experimental_hybrid_fetch_floor": floor}
        jsonschema.validate(observed, _load_schema("pair-spec.schema.json"))
        spec_path.write_text(json.dumps(observed), encoding="utf-8")
        assert pairrun.load_spec(spec_path)["experimental_hybrid_fetch_floor"] == floor
    for floor in (None, False, 25, 25.0, [], {}, "0", "250", "025", "25 ", ""):
        observed = {**_g0_spec(), "experimental_hybrid_fetch_floor": floor}
        spec_path.write_text(json.dumps(observed), encoding="utf-8")
        with pytest.raises(pairrun.RunError, match="exactly 25, 50 or 100"):
            pairrun.load_spec(spec_path)
        with pytest.raises(jsonschema.ValidationError):
            jsonschema.validate(observed, _load_schema("pair-spec.schema.json"))
    assert (
        pairrun.server_observation_configuration(loaded.get("query_stage_observation", "enabled"))[
            "query_stages"
        ]
        == "enabled"
    )
    for policy in ("enabled", "disabled"):
        observed = {**_g0_spec(), "query_stage_observation": policy}
        jsonschema.validate(observed, _load_schema("pair-spec.schema.json"))
        spec_path.write_text(json.dumps(observed), encoding="utf-8")
        assert pairrun.load_spec(spec_path)["query_stage_observation"] == policy
    for policy in (None, False, 0, [], {}, "off", "Disabled", "disabled ", ""):
        observed = {**_g0_spec(), "query_stage_observation": policy}
        spec_path.write_text(json.dumps(observed), encoding="utf-8")
        with pytest.raises(pairrun.RunError, match="exactly enabled or disabled"):
            pairrun.load_spec(spec_path)
        with pytest.raises(jsonschema.ValidationError):
            jsonschema.validate(observed, _load_schema("pair-spec.schema.json"))
    spec_with_timeout = _g0_spec()
    spec_with_timeout["io_timeout_secs"] = 600
    spec_path.write_text(json.dumps(spec_with_timeout), encoding="utf-8")
    assert pairrun.load_spec(spec_path)["io_timeout_secs"] == 600
    bad_timeout = _g0_spec()
    bad_timeout["io_timeout_secs"] = 0
    spec_path.write_text(json.dumps(bad_timeout), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="spec.io_timeout_secs"):
        pairrun.load_spec(spec_path)
    bad = _g0_spec()
    bad["semble_lockfile"] = ""
    spec_path.write_text(json.dumps(bad), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="spec.semble_lockfile must be a nonempty string"):
        pairrun.load_spec(spec_path)
    bad = _g0_spec()
    bad["cache_regime"] = "lukewarm"
    spec_path.write_text(json.dumps(bad), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="spec.cache_regime must be"):
        pairrun.load_spec(spec_path)
    for removed_key in ("semble_repetitions", "semble_warmup_passes"):
        bad = _g0_spec()
        bad[removed_key] = 2
        spec_path.write_text(json.dumps(bad), encoding="utf-8")
        with pytest.raises(pairrun.RunError, match="unknown keys"):
            pairrun.load_spec(spec_path)


def test_quanta_encoder_selector_binds_semantic_capture_revision():
    model = "model2vec:minishlab/potion-code-16M-v2"
    prefix = "e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b:model2vec-rs-0.3.0:fancy-regex:"

    def captures(route, revision):
        return {
            "record": {
                "system": "quanta",
                "run": {
                    "route_provenance": {route: {"capture_id": "capture"}},
                    "captures": {"capture": {"model": model, "model_revision": revision}},
                },
            }
        }

    v1 = prefix + "full-length-v1"
    v2 = prefix + "full-length-v2"
    assert pairrun._quanta_semantic_capture_identity_matches(
        captures("semantic", v1), "potion-code", {"semantic"}
    )
    assert pairrun._quanta_semantic_capture_identity_matches(
        captures("semantic", v2), "potion-code-full-v2", {"semantic"}
    )
    for missing in (None, "not-applicable", v1):
        assert not pairrun._quanta_semantic_capture_identity_matches(
            captures("semantic", missing), "potion-code-full-v2", {"semantic"}
        )
    assert pairrun._quanta_semantic_capture_identity_matches(
        captures("lexical", "not-applicable"), "potion-code-full-v2", {"lexical"}
    )
    assert not pairrun._quanta_semantic_capture_identity_matches(
        captures("lexical", "not-applicable"), "potion-code-full-v2", {"semantic"}
    )
    with pytest.raises(pairrun.RunError, match="exploratory diagnostic only"):
        pairrun.run_pair({"embedder": "potion-code-full-v2", "scope": "qualified"})

    def record(route, model, revision):
        return {
            "route_provenance": {route: {"capture_id": route}},
            "captures": {route: {"model": model, "model_revision": revision}},
        }

    lexical = record("lexical", "none:lexical", "not-applicable")
    symbol = record("symbol", "none:symbol", "not-applicable")
    semantic = record("semantic", model, v1)
    assert pairrun._quanta_admission_model_revision([lexical]) == "not-applicable"
    assert pairrun._quanta_admission_model_revision([lexical, symbol]) == "not-applicable"
    assert pairrun._quanta_admission_model_revision([lexical, semantic]) == v1
    assert pairrun._quanta_admission_model_revision([semantic, copy.deepcopy(semantic)]) == v1
    invalid = [
        [],
        [{}],
        [record("unknown", model, v1)],
        [record("lexical", model, "not-applicable")],
        [record("lexical", "none:lexical", v1)],
        [record("semantic", "none:lexical", "not-applicable")],
        [record("semantic", model, None)],
        [record("semantic", model, " ")],
        [record("semantic", model, "not-applicable")],
        [record("semantic", model, " not-applicable ")],
        [semantic, record("hybrid", model, v2)],
        [semantic, record("hybrid", "other-model", v1)],
    ]
    unrouted = copy.deepcopy(lexical)
    unrouted["captures"]["unused"] = {"model": model, "model_revision": v1}
    invalid.append([unrouted])
    missing = copy.deepcopy(lexical)
    missing["route_provenance"]["lexical"]["capture_id"] = "absent"
    invalid.append([missing])
    for records in invalid:
        with pytest.raises(pairrun.RunError, match="Quanta.*model"):
            pairrun._quanta_admission_model_revision(records)


def test_retrieval_recipes_download_nothing():
    # T14: benchmark recipes never implicitly fetch models, packages or
    # repos; every retrieval/benchmark-prep recipe body is scanned, so a
    # future recipe that downloads fails this test too.
    root = Path(pairrun.__file__).resolve().parents[3]
    bodies = {}
    current = None
    for line in (root / "Justfile").read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        is_header = (
            line
            and not line[0].isspace()
            and stripped.endswith(":")
            and ":=" not in line
            and not stripped.startswith(("#", "set ", "export ", "import "))
        )
        if is_header:
            current = stripped[:-1].split()[0]
            bodies[current] = []
        elif current is not None:
            bodies[current].append(line)
    targets = [
        name for name in bodies if name.startswith("retrieval") or name.startswith("benchmark-prep")
    ]
    assert "retrieval-sdk-proof" in targets
    assert "benchmark-prep-local" in targets
    verbs = (
        "pip install",
        "pip download",
        "uv pip",
        "curl ",
        "wget ",
        "cargo install",
        "git clone",
        "snapshot_download",
        "huggingface_hub",
    )
    for name in targets:
        text = "\n".join(bodies[name]).lower()
        for verb in verbs:
            assert verb not in text, f"{name} downloads via {verb}"


def test_benchmark_prep_does_not_repeat_retrieval_contracts():
    root = Path(pairrun.__file__).resolve().parents[3]

    def recipe(*args: str) -> str:
        completed = subprocess.run(
            ["just", "--dry-run", *args],
            cwd=root,
            check=True,
            capture_output=True,
            text=True,
        )
        return completed.stdout + completed.stderr

    def pytest_invocations(rendered: str) -> list[list[str]]:
        commands = [shlex.split(line) for line in rendered.splitlines()]
        return [
            argv
            for argv in commands
            if any(
                argv[index : index + 3] == ["python", "-m", "pytest"]
                for index in range(len(argv) - 2)
            )
        ]

    prep = recipe("benchmark-prep-local")
    control = recipe("benchmark-control-contract-local")
    proof = recipe("retrieval-contract-proof", "/tmp/retrieval-proof")
    local = recipe("retrieval-contract-local")
    # just's dry-run prints nested just calls without expanding their recipe.
    # Prove the actual delegation and inspect its leaf command separately.
    assert prep.count("uv run --frozen --extra dev just benchmark-control-contract-local") == 1
    control_pytest = pytest_invocations(control)
    local_pytest = pytest_invocations(local)
    assert control_pytest
    assert len(local_pytest) == 1
    retrieval_test = "tools/ci/tests/test_retrieval_benchmark.py"
    oracle_suite_test = "tools/ci/tests/test_source_oracle_suite.py"
    for test_path in (retrieval_test, oracle_suite_test):
        assert all(test_path not in argv for argv in pytest_invocations(prep) + control_pytest)
        assert local_pytest[0].count(test_path) == 1
    assert "test -p quanta-index-retrieval-bench" not in prep + control
    assert "portable_proof.py run --rail contract" in proof
    assert "-q" in local_pytest[0]
    assert "--test chunking_contract" in local
    assert "--test l5_parser_regressions" in local
    portable_source = (root / "tools/benchmark/retrieval/portable_proof.py").read_text()
    assert "proof_inventory.PYTHON_SELECTORS" in portable_source
    selectors = [
        [item.value for item in node.value.elts if isinstance(item, ast.Constant)]
        for node in ast.walk(ast.parse(portable_source))
        if isinstance(node, ast.Assign)
        and any(isinstance(target, ast.Name) and target.id == "selector" for target in node.targets)
        and isinstance(node.value, ast.List)
    ]
    assert (
        sum(
            ["--test", "chunking_contract"] == selector[index : index + 2]
            and ["--test", "l5_parser_regressions"] == selector[index + 2 : index + 4]
            for selector in selectors
            for index in range(len(selector) - 3)
        )
        == 2
    )


def test_retrieval_verdict_recipe_matches_cli_parser():
    root = Path(pairrun.__file__).resolve().parents[3]
    values = ("/tmp/repo", "/tmp/suite.json", "/tmp/run-manifest.json", "/tmp/verdict.json")
    completed = subprocess.run(
        ["just", "--dry-run", "retrieval-verdict", *values],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    )
    rendered = completed.stderr.strip() or completed.stdout.strip()
    command = shlex.split(rendered)
    assert command[:7] == [
        "uv",
        "run",
        "--frozen",
        "--extra",
        "dev",
        "python",
        "tools/benchmark/retrieval/run.py",
    ]
    parsed = pairrun.build_parser().parse_args(command[7:])
    assert (parsed.command, parsed.repo, parsed.suite, parsed.run_manifest, parsed.out) == (
        "verdict",
        *values,
    )


def _g0_manifest() -> dict:
    return {
        "manifest_version": 2,
        "blinding": "attested",
        "isolation_method": "m",
        "access_block_log": "l",
        "scope": "exploratory",
        "claims": {"quality": False, "speed": False, "same_model": False, "incremental": False},
        "repetitions": 1,
        "evidence": {
            "pair": {"mapping_proof_digest": _fake_sha("mapping")},
            "perf": {
                "observations_floor": 0,
                "fresh_roots": 1,
                "phase_boundaries": False,
                "resource_accounting": True,
            },
        },
        "host": {
            "start_digest": _fake_sha("hs"),
            "end_digest": _fake_sha("he"),
            "cache_regime": "true_process_cold",
        },
        "artifacts": {
            "suite": "suite.json",
            "query_pack": "pack.json",
            "corpus_manifest": "manifest.json",
            "mapping_proof": "mapping-proof.json",
            "latency_matrix": "latency-matrix.json",
            "host_start": "host-start.json",
            "host_end": "host-end.json",
            "host_profile": "host-profile.json",
            "records": ["lex.json", "sem.json"],
            "reports": [],
            "quanta_manifests": [],
            "semble_adapter_manifest": "adapter-manifest.json",
            "semble_lockfile": "lockfile.txt",
            "semble_native": ["native.json"],
            "semble_model_cache_manifests": ["model-cache-manifest.json"],
            "phase_metrics": ["phase.json"],
            "phase_metrics_digests": {"phase.json": _fake_sha("phase")},
            "symbol_preflights": ["symbol-preflight.json"],
            "resource_metrics": ["resource.json"],
            "protocol_lock": "protocol-lock.json",
            "driver_source_closure": "driver-source-closure.json",
        },
        "provenance": {
            "quanta": {
                "source_sha": "a" * 40,
                "source_closure_digest": _fake_sha("closure"),
                "binary_digest": _fake_sha("qb"),
                "embedder": "potion-code",
            },
            "semble": {
                "revision": "0.6.0",
                "lockfile_digest": _fake_sha("lock"),
                "interpreter_digest": _fake_sha("py"),
                "model_asset_digest": _fake_sha("m"),
            },
            "corpus": {"digest": _fake_sha("c"), "path_sha_diff_digest": _fake_sha("d")},
            "suite": {
                "suite_digest": _fake_sha("s"),
                "query_pack_digest": _fake_sha("p"),
                "tokenizer_budget_version": "qb-v1",
            },
            "host": {"profile_digest": _fake_sha("hp"), "check_record_digest": _fake_sha("h")},
            "admission": {"manifest_digest": None},
        },
    }


def test_current_manifest_schema():
    schema = _load_schema("run-manifest.schema.json")
    jsonschema.validate(_g0_manifest(), schema)
    diagnostic_v2 = _g0_manifest()
    diagnostic_v2["provenance"]["quanta"]["embedder"] = "potion-code-full-v2"
    jsonschema.validate(diagnostic_v2, schema)
    pairrun._validate_manifest_shape(diagnostic_v2)
    diagnostic_v2["claims"]["quality"] = True
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(diagnostic_v2, schema)
    with pytest.raises(pairrun.RunError, match="exploratory diagnostic only"):
        pairrun._validate_manifest_shape(diagnostic_v2)

    def invalid(mutator):
        manifest = json.loads(json.dumps(_g0_manifest()))
        mutator(manifest)
        with pytest.raises(jsonschema.ValidationError):
            jsonschema.validate(manifest, schema)

    invalid(lambda m: m["claims"].update(speed="false"))
    invalid(lambda m: m.update(smuggled=True))
    invalid(lambda m: m["evidence"].pop("pair"))
    invalid(lambda m: m["provenance"]["semble"].update(revision="0.7.0"))
    invalid(lambda m: m["provenance"]["quanta"].update(source_sha="unresolved"))
    invalid(lambda m: m["provenance"]["quanta"].pop("source_closure_digest"))
    invalid(lambda m: m["provenance"]["quanta"].update(source_closure_digest=None))
    invalid(lambda m: m["provenance"]["quanta"].update(embedder="openai"))
    invalid(lambda m: m["provenance"]["quanta"].pop("embedder"))
    invalid(lambda m: m["host"].update(cache_regime="lukewarm"))
    invalid(lambda m: m["host"].pop("cache_regime"))

    bound_exploratory = json.loads(json.dumps(_g0_manifest()))
    jsonschema.validate(bound_exploratory, schema)
    pairrun._validate_manifest_shape(bound_exploratory)
    for mutator in (
        lambda value: value["artifacts"].pop("driver_source_closure"),
        lambda value: value["provenance"]["quanta"].update(source_closure_digest=None),
    ):
        mutant = json.loads(json.dumps(bound_exploratory))
        mutator(mutant)
        with pytest.raises(jsonschema.ValidationError):
            jsonschema.validate(mutant, schema)
        with pytest.raises(pairrun.RunError, match="artifacts|source closure"):
            pairrun._validate_manifest_shape(mutant)

    qualified = json.loads(json.dumps(_g0_manifest()))
    qualified["scope"] = "qualified"
    qualified["artifacts"].update(
        {
            "admission_manifest": "admission.json",
            "experiment_custody": "experiment-custody.json",
            "development_suite": "development-suite.json",
            "license_receipt": "license.json",
            "annotation_receipts": ["annotation-a.json", "annotation-b.json"],
            "adjudication_receipt": "adjudication.json",
        }
    )
    qualified["provenance"]["admission"]["manifest_digest"] = _fake_sha("admission")
    qualified["provenance"]["quanta"]["source_closure_digest"] = _fake_sha("closure")
    jsonschema.validate(qualified, schema)
    for mutator in (
        lambda value: value["artifacts"].pop("driver_source_closure"),
        lambda value: value["provenance"]["quanta"].update(source_closure_digest=None),
        lambda value: value["provenance"]["admission"].update(manifest_digest=None),
    ):
        mutant = json.loads(json.dumps(qualified))
        mutator(mutant)
        with pytest.raises(jsonschema.ValidationError):
            jsonschema.validate(mutant, schema)


def _g0_verdict() -> dict:
    states = ["CONTRACT_GREEN", "SDK_PATH_GREEN", "PAIR_VALID", "PERF_QUALIFIED", "QUALITY_DELTA"]
    return {
        "verdict_version": 2,
        "os_portability": {
            "qualified": False,
            "reason": "execution_os_tool_identity_unverified",
        },
        "states": {name: "not_run" for name in states},
        "state_evidence": {
            name: {"reason": "no_evidence", "proof_digest": None} for name in states
        },
        "blinding": "attested",
        "isolation_method": "m",
        "access_block_log": "l",
        "missing_t_ids": ["T00"],
        "not_applicable_t_ids": ["T15", "T16"],
        "failure_class": "none",
        "provenance": {
            "quanta": {
                "source_sha": "a" * 40,
                "binary_digest": _fake_sha("qb"),
                "embedder": "potion-code",
            },
            "semble": {"revision": "0.6.0", "lockfile_digest": _fake_sha("lock")},
            "corpus": {"digest": _fake_sha("c"), "path_sha_diff_digest": _fake_sha("d")},
            "suite": {
                "suite_digest": _fake_sha("s"),
                "query_pack_digest": _fake_sha("p"),
                "tokenizer_budget_version": "qb-v1",
            },
            "host": {"profile_digest": _fake_sha("hp"), "check_record_digest": _fake_sha("h")},
            "admission": {"manifest_digest": None},
        },
        "counts": {"selected": 0, "executed": 0, "passed": 0, "failed": 0},
        "comparisons": [
            {
                "strategy": "whole_file",
                "baseline_route": "semble-hybrid",
                "candidate_route": "lexical",
                "primary_metric": "recall_at_10",
                "primary_delta": 0.0,
                "record_digest": _fake_sha("rec"),
                "report_digest": _fake_sha("rep"),
            },
        ],
    }


def test_v3_verdict_schema():
    schema = _load_schema("verdict.schema.json")
    jsonschema.validate(_g0_verdict(), schema)

    def invalid(mutator):
        verdict = json.loads(json.dumps(_g0_verdict()))
        mutator(verdict)
        with pytest.raises(jsonschema.ValidationError):
            jsonschema.validate(verdict, schema)

    invalid(lambda v: v.update(verdict_version=1))
    invalid(lambda v: v.update(primary_delta=0.0))
    invalid(lambda v: v["comparisons"][0].pop("primary_delta"))
    invalid(lambda v: v["states"].update(PAIR_VALID="maybe"))
    invalid(lambda v: v["provenance"].update(quanta_source_sha="a" * 40))
    invalid(lambda v: v.update(failure_class="typo"))
    invalid(lambda v: v["provenance"]["quanta"].update(embedder="openai"))


# --- Item 1: byte spans, universe binding, split custody (strict §1) ---


def _repack(repo, suite, run):
    _, pack, _ = ev.validate_suite(repo, suite)
    run = json.loads(json.dumps(run))
    run["query_pack_sha256"] = ev.digest(ev.canonical(pack))
    return pack, run


def _v3_block(files, path, start, end, *, tokens=None, rank=None, grade=None):
    file_sha, block_sha, counted = _span_meta(files[path], start, end)
    start_byte, end_byte = _byte_span(files[path], start, end)
    item = {
        "path": path,
        "start_byte": start_byte,
        "end_byte": end_byte,
        "start_line": start,
        "end_line": end,
        "file_sha256": file_sha,
        "block_sha256": block_sha,
    }
    if tokens is not None:
        item["tokens"] = counted
    if rank is not None:
        item["rank"] = rank
    if grade is not None:
        item["grade"] = grade
    return item


def test_v3_byte_span_verification(tmp_path):
    repo, suite, run, suite_path, runner_path, files = fixture_v3(tmp_path, answerable_only=True)

    def attempt(mutator, match):
        mutated = json.loads(json.dumps(run))
        mutator(mutated["results"][0]["candidates"][0])
        suite_path.write_text(json.dumps(suite), encoding="utf-8")
        runner_path.write_text(json.dumps(mutated), encoding="utf-8")
        with pytest.raises(ev.EvidenceError, match=match):
            ev.load_evidence(repo, suite_path, runner_path)

    attempt(lambda c: c.update(end_byte=c["end_byte"] - 1), "byte span disagrees")
    attempt(lambda c: c.update(start_byte=c["start_byte"] + 1), "byte span disagrees")
    attempt(lambda c: c.update(end_byte=10**9), "byte span runs past EOF")
    attempt(lambda c: c.update(start_byte=c["end_byte"]), "empty byte span")
    attempt(lambda c: c.update(start_byte=-1), "must be an integer >= 0")

    mutated_suite = json.loads(json.dumps(suite))
    mutated_suite["tasks"][0]["gold"][0]["end_byte"] += 5
    with pytest.raises(ev.EvidenceError, match="byte span disagrees"):
        ev.validate_suite(repo, mutated_suite)

    # A byte span cutting a UTF-8 boundary is refused, not decoded lossily.
    repo2, commit2 = _write_repo(tmp_path / "uni", {"u.txt": "aé\nb\n".encode()})
    source = ev.SourceSnapshot(repo2, commit2)
    bad = {
        "path": "u.txt",
        "start_byte": 0,
        "end_byte": 2,
        "start_line": 1,
        "end_line": 1,
        "file_sha256": ev.digest("aé\nb\n".encode()),
        "block_sha256": "0" * 64,
        "tokens": 1,
        "rank": 1,
    }
    with pytest.raises(ev.EvidenceError, match="cuts a UTF-8 boundary"):
        ev.block(source, bad, "probe", candidate=True)
    good = dict(bad, end_byte=4, block_sha256=ev.digest("aé\n".encode()), tokens=2)
    assert ev.block(source, good, "probe", candidate=True)["tokens"] == 2
    accounting = {
        "unit_kind": "chunk",
        "unit_id": "indexed-é",
        "producer_identity": "fixed_window_strict",
        "indexed_start_byte": 1,
        "indexed_end_byte": 3,
        "sdk_start_line": 1,
        "sdk_end_line": 1,
        "extra_context_bytes": 2,
    }
    witnessed = dict(good, span_accounting=accounting)
    assert ev.block(source, witnessed, "probe", candidate=True, allow_span_accounting=True)
    with pytest.raises(ev.EvidenceError, match="unknown fields"):
        ev.block(source, witnessed, "probe", candidate=True)
    for change, match in (
        ({"indexed_end_byte": 2}, "UTF-8 boundary"),
        ({"extra_context_bytes": 1}, "context expansion"),
        ({"sdk_start_line": 0}, "SDK line span"),
        ({"unit_kind": "symbol", "sdk_start_line": 0, "sdk_end_line": 0}, "SDK line span"),
    ):
        mutant = dict(good, span_accounting=dict(accounting, **change))
        with pytest.raises(ev.EvidenceError, match=match):
            ev.block(source, mutant, "probe", candidate=True, allow_span_accounting=True)


def test_v3_byte_coverage_decides_credit():
    gold = {"path": "a", "start_byte": 10, "end_byte": 20, "start_line": 2, "end_line": 2}
    same = dict(gold)
    assert ev.covers(same, gold) is True
    subset = dict(gold, start_byte=12, end_byte=15)
    assert ev.covers(subset, gold) is False
    superset = dict(gold, start_byte=5, end_byte=25)
    assert ev.covers(superset, gold) is True
    shifted = dict(gold, start_byte=15, end_byte=25)
    assert ev.covers(shifted, gold) is False
    assert ev.covers(dict(gold, path="b"), gold) is False

    # The new diagnostic must not mistake the old scored-line projection
    # for the published indexed span or silently use a partial row set.
    context_bytes = 1_000_000
    candidate = {
        "path": "a",
        "start_byte": 0,
        "end_byte": context_bytes,
        "tokens": 500_000,
        "rank": 1,
        "span_accounting": {
            "unit_id": "fixed-10-byte-span",
            "indexed_start_byte": 10,
            "indexed_end_byte": 20,
            "sdk_start_line": 1,
            "sdk_end_line": 1,
            "extra_context_bytes": context_bytes - 10,
        },
    }
    row = {"task_id": "T", "route": "q", "status": "success", "candidates": [candidate]}
    run = {
        "schema_version": 5,
        "span_accounting_version": 1,
        "route_provenance": {"q": {"capture_id": "cap"}},
        "captures": {"cap": {"system": "quanta"}},
        "results": [row],
    }
    tasks = {"T": {"gold": [gold]}}
    report = ev.indexed_span_diagnostics(run, {("T", "q"): row}, tasks)
    assert report["routes"]["q"]["mean"] == {
        "rank_only_hit_at_1": 1.0,
        "exact_index_span_mrr_at_10": 1.0,
        "exact_index_span_recall_at_10": 1.0,
        "scored_context_bytes_at_10": 1_000_000.0,
        "scored_context_tokens_at_10": 500_000.0,
        "indexed_bytes_at_10": 10.0,
        "extra_context_bytes_at_10": 999_990.0,
    }
    context = report["per_candidate"][0]
    assert context["indexed_bytes"] == 10
    assert context["sdk_line_span_bytes"] == 1_000_000
    assert context["scored_projection_bytes"] == 1_000_000
    assert context["scored_to_indexed_expansion_ratio"] == 100_000.0
    assert context["sdk_to_indexed_expansion_ratio"] == 100_000.0
    exact = json.loads(json.dumps(candidate))
    exact["start_byte"] = 10
    exact["end_byte"] = 20
    exact["tokens"] = 5
    exact["span_accounting"]["extra_context_bytes"] = 0
    exact["span_accounting"]["sdk_start_line"] = 0
    exact["span_accounting"]["sdk_end_line"] = 0
    row["candidates"] = [exact]
    exact_report = ev.indexed_span_diagnostics(run, {("T", "q"): row}, tasks)
    for metric in (
        "rank_only_hit_at_1",
        "exact_index_span_mrr_at_10",
        "exact_index_span_recall_at_10",
    ):
        assert exact_report["routes"]["q"]["mean"][metric] == report["routes"]["q"]["mean"][metric]
    assert exact_report["per_candidate"][0]["scored_projection_bytes"] == 10
    assert exact_report["per_candidate"][0]["scored_to_indexed_expansion_ratio"] == 1.0
    assert exact_report["per_candidate"][0]["sdk_line_span_bytes"] is None
    miss = json.loads(json.dumps(candidate))
    miss["span_accounting"]["indexed_start_byte"] = 12
    miss["span_accounting"]["extra_context_bytes"] = context_bytes - 8
    assert ev.covers(miss, gold) is True
    row["candidates"] = [miss]
    report = ev.indexed_span_diagnostics(run, {("T", "q"): row}, tasks)
    assert report["routes"]["q"]["mean"]["rank_only_hit_at_1"] == 0.0
    assert report["routes"]["q"]["mean"]["exact_index_span_recall_at_10"] == 0.0
    partial = dict(miss)
    partial.pop("span_accounting")
    row["candidates"] = [miss, dict(partial, rank=2)]
    with pytest.raises(ev.EvidenceError, match="partial indexed span evidence"):
        ev.indexed_span_diagnostics(run, {("T", "q"): row}, tasks)


def test_v3_partial_bytes_earn_no_credit(tmp_path):
    repo, suite, run, suite_path, runner_path, files = fixture_v3(tmp_path, answerable_only=True)
    suite["tasks"][0]["gold"] = [_v3_block(files, "a.txt", 2, 3, grade=3)]
    run["results"][0]["candidates"] = [_v3_block(files, "a.txt", 2, 2, tokens=True, rank=1)]
    _pack, run = _repack(repo, suite, run)
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    report = ev.evaluate(loaded_suite, pack, loaded_run, "lexical", "hybrid")
    rows = {(r["task_id"], r["route"]): r for r in report["per_query"]}
    assert rows[("T1", "lexical")]["chunk_recall_at_10"] == 0.0
    run["results"][0]["candidates"] = [_v3_block(files, "a.txt", 2, 3, tokens=True, rank=1)]
    _pack, run = _repack(repo, suite, run)
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    report = ev.evaluate(loaded_suite, pack, loaded_run, "lexical", "hybrid")
    rows = {(r["task_id"], r["route"]): r for r in report["per_query"]}
    assert rows[("T1", "lexical")]["chunk_recall_at_10"] == 1.0


def test_independent_file_ndcg_uses_file_grades_once_per_file():
    judgments = [
        {"path": "a.go", "grade": 3},
        {"path": "b.go", "grade": 1},
        {"path": "c.go", "grade": 0},
    ]
    ranked = [{"path": path} for path in ("a.go", "a.go", "b.go")]
    ideal = 7 + 1 / math.log2(3)
    assert ev.file_ndcg_at_k(ranked, judgments, 10) == pytest.approx((7 + 1 / math.log2(4)) / ideal)
    assert ev.file_ndcg_at_k([{"path": "a.go"}, {"path": "b.go"}], judgments, 10) == 1.0
    assert ev.file_ndcg_at_k([{"path": "b.go"}, {"path": "a.go"}], judgments, 10) == pytest.approx(
        (1 + 7 / math.log2(3)) / ideal
    )
    assert ev.file_ndcg_at_k([{"path": "c.go"}, {"path": "a.go"}], judgments, 10) < 1.0
    rounded = [{"path": str(index), "grade": grade} for index, grade in enumerate((1, 1, 1, 3))]
    ideal_order = [{"path": str(index)} for index in (3, 0, 1, 2)]
    assert ev.file_ndcg_at_k(ideal_order, rounded, 10) == 1.0


def test_declaration_judgment_requires_published_symbol_span_not_returned_context():
    source = b"func A() {} ; func B() {}\n"  # Both declarations share the returned line.
    first = source.index(b"A()")
    second = source.index(b"B()")
    gold = [{"path": "a.go", "start_byte": first, "end_byte": first + 1, "grade": 3}]
    item = {
        "path": "a.go",
        "start_byte": 0,
        "end_byte": len(source),
        "span_accounting": {
            "unit_kind": "symbol",
            "unit_id": "other-declaration",
            "indexed_start_byte": second,
            "indexed_end_byte": second + 1,
        },
    }
    assert ev.declaration_recall_at_k([item], gold, 10) == 0.0
    assert ev.declaration_mrr_at_k([item], gold, 10) == 0.0
    own = copy.deepcopy(item)
    own["span_accounting"].update(
        unit_id="gold-declaration", indexed_start_byte=first, indexed_end_byte=first + 1
    )
    assert ev.declaration_recall_at_k([item, own, own], gold, 10) == 1.0
    assert ev.declaration_mrr_at_k([item, own], gold, 10) == 0.5
    chunk = copy.deepcopy(own)
    chunk["span_accounting"]["unit_kind"] = "chunk"
    assert ev.declaration_recall_at_k([chunk], gold, 10) == 0.0
    unidentified = copy.deepcopy(own)
    unidentified["span_accounting"]["unit_id"] = ""
    assert ev.declaration_recall_at_k([unidentified], gold, 10) == 0.0

    own["rank"] = 1
    suite = {"comparison_contract": {"top_k": 10}, "routes": ["symbol"]}
    run = {
        "span_accounting_version": 1,
        "route_provenance": {"symbol": {"capture_id": "q0"}},
        "captures": {
            "q0": {
                "system": "quanta",
                "execution_profile": {"policy": "exact_symbol_name"},
            }
        },
    }
    tasks = {"T": {"answerable": True, "declaration_judgments": gold}}
    results = {("T", "symbol"): {"status": "success", "rank_unit": "symbol", "candidates": [own]}}
    diagnostic = ev.judgment_diagnostics(suite, run, results, tasks, "symbol", None)
    assert diagnostic is not None
    assert diagnostic["declaration_judgments"]["routes"]["symbol"]["eligible_task_ids"] == ["T"]
    assert diagnostic["declaration_judgments"]["routes"]["symbol"]["operational_mean"] == {
        "recall_at_10": 1.0,
        "mrr_at_10": 1.0,
    }
    # Older records may collapse same-line declarations; policy is not proof
    # that the scored ranks are independent published symbol units.
    legacy_results = copy.deepcopy(results)
    del legacy_results[("T", "symbol")]["rank_unit"]
    legacy = ev.judgment_diagnostics(suite, run, legacy_results, tasks, "symbol", None)
    legacy_route = legacy["declaration_judgments"]["routes"]["symbol"]
    assert legacy_route["eligible_task_ids"] == []
    assert legacy_route["excluded"] == [{"task_id": "T", "reason": "rank_unit_mismatch"}]

    results[("T", "symbol")] = {
        "status": "abstained",
        "rank_unit": "symbol",
        "candidates": [],
    }
    exhausted = ev.judgment_diagnostics(suite, run, results, tasks, "symbol", None)
    route = exhausted["declaration_judgments"]["routes"]["symbol"]
    assert route["eligible_task_ids"] == ["T"]
    assert route["conditional_mean"] == {"recall_at_10": 0.0, "mrr_at_10": 0.0}


def test_literal_file_policy_binds_projection_and_raw_identity():
    raw = "writeContentType"
    request = 'select:file "writeContentType"'
    assert qp.plan_lexical_request("literal_file", raw) == request
    assert qp.policy_config_canonical("literal_file") == (
        '{"escaping":"lq-norm-phrase-v1","policy":"literal_file","projection":"file"}'
    )
    assert qp.execution_profile("literal_file") == {
        "profile_id": "quanta-literal-file-v1",
        "policy": "literal_file",
        "config": {},
        "planning_cost_in_latency": False,
    }
    assert qp.derive_query_identity("literal_file", raw) == {
        "original_query_sha256": hashlib.sha256(raw.encode()).hexdigest(),
        "effective_lexical_request_sha256": hashlib.sha256(request.encode()).hexdigest(),
        "semantic_text_sha256": hashlib.sha256(raw.encode()).hexdigest(),
    }
    assert qp.plan_lexical_request("literal", raw) == '"writeContentType"'


def test_literal_file_policy_refuses_unindexable_input_and_v4_replay():
    with pytest.raises(qp.QueryPlanError, match="no tokens"):
        qp.plan_lexical_request("literal_file", "---")
    with pytest.raises(qp.QueryPlanError, match="unsupported v4"):
        qp.derive_query_identity_v4("literal_file", "writeContentType")


def test_pair_spec_refuses_diagnostic_rank_profiles_before_quality_gate(tmp_path):
    spec_path = tmp_path / "pair-spec.json"
    for policy in ("literal_file", "exact_symbol_name"):
        spec = _g0_spec()
        spec["execution_profiles"]["quanta"] = qp.execution_profile(policy)
        spec_path.write_text(json.dumps(spec), encoding="utf-8")
        with pytest.raises(pairrun.RunError, match="diagnostic rank profile"):
            pairrun.load_spec(spec_path)


def test_exact_symbol_profile_admits_only_standalone_symbol_capture(tmp_path, monkeypatch):
    spec_path = tmp_path / "symbol-spec.json"
    spec = _g0_spec()
    spec["execution_profiles"] = {"quanta": qp.execution_profile("exact_symbol_name")}
    spec["routes"] = ["symbol"]
    spec["candidate_route"] = "symbol"
    spec_path.write_text(json.dumps(spec), encoding="utf-8")
    assert pairrun.load_spec(spec_path, standalone_quanta=True)["routes"] == ["symbol"]
    observed = []
    monkeypatch.setattr(
        pairrun,
        "run_quanta",
        lambda loaded, _spec_dir: (
            observed.append(loaded["execution_profiles"]["quanta"]["policy"]) or 0
        ),
    )
    assert pairrun.cmd_quanta(SimpleNamespace(spec=str(spec_path))) == 0
    assert observed == ["exact_symbol_name"]
    with pytest.raises(pairrun.RunError, match="diagnostic rank profile"):
        pairrun.load_spec(spec_path)

    invalid = copy.deepcopy(spec)
    invalid["routes"] = ["lexical"]
    spec_path.write_text(json.dumps(invalid), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match=r"requires \['symbol'\] route"):
        pairrun.load_spec(spec_path, standalone_quanta=True)

    invalid = copy.deepcopy(spec)
    invalid["execution_profiles"]["semble"] = semble_adapter.execution_profile("lexical-file", None)
    spec_path.write_text(json.dumps(invalid), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="cannot include Semble"):
        pairrun.load_spec(spec_path, standalone_quanta=True)

    invalid = copy.deepcopy(spec)
    invalid["claims"]["quality"] = True
    spec_path.write_text(json.dumps(invalid), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="cannot carry qualified claims"):
        pairrun.load_spec(spec_path, standalone_quanta=True)


def test_literal_file_diagnostic_profile_admits_standalone_lexical_only(tmp_path):
    spec = _g0_spec()
    spec["execution_profiles"] = {"quanta": qp.execution_profile("literal_file")}
    spec["routes"] = ["lexical"]
    spec["candidate_route"] = "lexical"
    path = tmp_path / "literal-spec.json"
    path.write_text(json.dumps(spec), encoding="utf-8")
    assert pairrun.load_spec(path, standalone_quanta=True)["routes"] == ["lexical"]
    spec["scope"] = "qualified"
    path.write_text(json.dumps(spec), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="cannot carry qualified claims"):
        pairrun.load_spec(path, standalone_quanta=True)


@pytest.mark.parametrize(
    "policy",
    [
        "code_search_file",
        "code_search_typo_file",
        "code_search_components_file",
        "natural_language_file",
    ],
)
def test_code_search_file_pair_profile_admits_only_file_diagnostic(tmp_path, policy):
    spec_path = tmp_path / "pair-spec.json"
    spec = _g0_spec()
    spec["execution_profiles"]["quanta"] = qp.execution_profile(policy)
    spec["execution_profiles"]["semble"] = semble_adapter.execution_profile("lexical-file", None)
    spec["routes"] = ["lexical"]
    jsonschema.validate(spec, _load_schema("pair-spec.schema.json"))
    spec_path.write_text(json.dumps(spec), encoding="utf-8")
    assert pairrun.load_spec(spec_path)["execution_profiles"]["quanta"]["policy"] == policy

    for change, match in (
        (
            lambda row: row["execution_profiles"].update(
                semble=semble_adapter.execution_profile("lexical-only", None)
            ),
            "Semble lexical-file",
        ),
        (lambda row: row.update(routes=["lexical", "hybrid"]), "lexical-only Quanta"),
        (lambda row: row["claims"].update(quality=True), "cannot carry claims"),
        (lambda row: row.update(scope="qualified"), "a quality claim"),
    ):
        forged = copy.deepcopy(spec)
        change(forged)
        spec_path.write_text(json.dumps(forged), encoding="utf-8")
        with pytest.raises(pairrun.RunError, match=match):
            pairrun.load_spec(spec_path)
    qualified = copy.deepcopy(spec)
    qualified["scope"] = "qualified"
    qualified["claims"]["quality"] = True
    qualified["admission"] = {
        "manifest": "/tmp/admission.json",
        "split_manifest": "/tmp/split.json",
        "split_releases": "/tmp/releases.json",
        "license_receipt": "/tmp/license.json",
        "annotation_receipts": ["/tmp/annotation-1.json", "/tmp/annotation-2.json"],
        "adjudication_receipt": "/tmp/adjudication.json",
    }
    jsonschema.validate(qualified, _load_schema("pair-spec.schema.json"))
    spec_path.write_text(json.dumps(qualified), encoding="utf-8")
    if policy in ("code_search_file", "natural_language_file"):
        assert pairrun.load_spec(spec_path) == qualified
        assert pairrun._validate_file_pair_contract(qualified, paired=True) is True
        local_admission = copy.deepcopy(qualified)
        del local_admission["admission"]["split_manifest"]
        del local_admission["admission"]["split_releases"]
        local_admission["admission"].update(
            development_suite="/tmp/development.json", experiment_custody="/tmp/custody.json"
        )
        spec_path.write_text(json.dumps(local_admission), encoding="utf-8")
        with pytest.raises(pairrun.RunError, match="repository-disjoint admission"):
            pairrun.load_spec(spec_path)
        for entry in (
            pairrun.run_pair,
            lambda spec: pairrun._run_pair_staged(spec, tmp_path / "stage"),
        ):
            with pytest.raises(pairrun.RunError, match="repository-disjoint admission"):
                entry(local_admission)
        assert not (tmp_path / "stage").exists()
    else:
        with pytest.raises(pairrun.RunError, match="requires code_search_file"):
            pairrun.load_spec(spec_path)
        for entry in (
            pairrun.run_pair,
            lambda spec: pairrun._run_pair_staged(spec, tmp_path / "stage"),
        ):
            with pytest.raises(pairrun.RunError, match="requires code_search_file"):
                entry(qualified)
        assert not (tmp_path / "stage").exists()


def test_natural_language_file_planner_has_distinct_scored_file_contract():
    raw = "Find retry handling"
    policy = "natural_language_file"
    assert qp.plan_lexical_request(policy, raw) == 'select:file case:no find OR retry OR handling'
    assert qp.plan_lexical_request("natural_language", raw) == 'case:no find OR retry OR handling'
    assert qp.plan_lexical_request(policy, 'The the AND and repo:x "retry"') == (
        'select:file case:no the OR and OR repo OR x OR retry'
    )
    assert qp.plan_lexical_request(policy, 'foo-bar/path.go') == (
        'select:file case:no foo OR bar OR path OR go'
    )
    assert qp.derive_query_identity(policy, raw) != qp.derive_query_identity(
        "natural_language", raw
    )
    assert qp.FILE_PROJECTION_ORDERING[policy] == qp.ORDERING_SCORE_DESC
    assert qp.QUANTA_EVALUATION_POLICIES[qp.NATURAL_LANGUAGE_FILE_SEARCH] == frozenset((policy,))
    assert qp.execution_profile(policy)["config"] == qp.DEFAULT_NL_CONFIG
    for raw in ("--- ... ///", " ".join(f"a{i}" for i in range(33)), "x" * 97):
        with pytest.raises(qp.QueryPlanError):
            qp.plan_lexical_request(policy, raw)


def test_bounded_natural_language_budget_binds_request_profile_and_replay():
    query = " ".join(f"w{i:02}" for i in range(48))
    config = {**qp.DEFAULT_NL_CONFIG, "max_tokens": 64}
    with pytest.raises(qp.QueryPlanError, match="48 tokens.*max 32"):
        qp.plan_lexical_request("natural_language_file", query)
    request = qp.plan_lexical_request("natural_language_file", query, config)
    assert request == "select:file case:no " + " OR ".join(f"w{i:02}" for i in range(48))
    assert hashlib.sha256(request.encode()).hexdigest() == (
        "0dfa2879b01ca70af9c6006b94aa9a933bfb68ffef16bff313b658361350964a"
    )
    identity = qp.derive_query_identity("natural_language_file", query, config)
    assert identity["effective_lexical_request_sha256"] == (
        "0dfa2879b01ca70af9c6006b94aa9a933bfb68ffef16bff313b658361350964a"
    )
    profile = qp.execution_profile("natural_language_file", config)
    assert ev.validate_execution_profile(profile, "quanta", "profile") == profile
    assert qp.execution_profile_sha256("natural_language_file", config) != (
        qp.execution_profile_sha256("natural_language_file")
    )
    boundary = " ".join(f"w{i:02}" for i in range(64))
    assert qp.plan_lexical_request("natural_language_file", boundary, config).count(" OR ") == 63
    with pytest.raises(qp.QueryPlanError, match="65 tokens.*max 64"):
        qp.plan_lexical_request("natural_language_file", boundary + " w64", config)
    altered = copy.deepcopy(profile)
    altered["config"]["max_tokens"] = 65
    with pytest.raises(ev.EvidenceError, match="config is invalid"):
        ev.validate_execution_profile(altered, "quanta", "profile")


@pytest.mark.parametrize(
    "config",
    [
        {"max_token_chars": 96, "max_tokens": True, "min_token_chars": 1},
        {"max_token_chars": 96, "max_tokens": 0, "min_token_chars": 1},
        {"max_token_chars": 96, "max_tokens": 65, "min_token_chars": 1},
        {"max_token_chars": 97, "max_tokens": 48, "min_token_chars": 1},
        {"max_token_chars": 96, "max_tokens": 48},
        {"max_token_chars": 96, "max_tokens": 48, "min_token_chars": 1, "extra": 1},
        [96, 48, 1],
    ],
)
def test_bounded_natural_language_budget_refuses_malformed_config(config):
    with pytest.raises(qp.QueryPlanError):
        qp.execution_profile("natural_language_file", config)
    with pytest.raises(qp.QueryPlanError):
        qp.plan_lexical_request("natural_language_file", "find retry", config)


def test_bounded_natural_language_budget_refuses_non_nl_profile_and_qualified_scope(tmp_path):
    with pytest.raises(qp.QueryPlanError, match="does not accept"):
        qp.execution_profile("keyword_file", {**qp.DEFAULT_NL_CONFIG, "max_tokens": 48})
    spec = _g0_spec()
    spec["scope"] = "exploratory"
    spec["routes"] = ["lexical"]
    spec["execution_profiles"] = {
        "quanta": qp.execution_profile(
            "natural_language_file", {**qp.DEFAULT_NL_CONFIG, "max_tokens": 64}
        ),
        "semble": semble_adapter.execution_profile("lexical-file", None),
    }
    path = tmp_path / "spec.json"
    path.write_text(json.dumps(spec), encoding="utf-8")
    jsonschema.validate(spec, _load_schema("pair-spec.schema.json"))
    jsonschema.validate(
        spec["execution_profiles"]["quanta"],
        _load_schema("runner.schema.json")["$defs"]["execution_profile"],
    )
    assert pairrun.load_spec(path)["execution_profiles"]["quanta"]["config"]["max_tokens"] == 64
    invalid = copy.deepcopy(spec)
    invalid["execution_profiles"]["quanta"]["config"]["max_tokens"] = 65
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(invalid, _load_schema("pair-spec.schema.json"))
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(
            invalid["execution_profiles"]["quanta"],
            _load_schema("runner.schema.json")["$defs"]["execution_profile"],
        )
    spec["scope"] = "qualified"
    path.write_text(json.dumps(spec), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="requires exploratory scope"):
        pairrun.load_spec(path)
    spec["scope"] = "exploratory"
    spec["claims"]["quality"] = True
    path.write_text(json.dumps(spec), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="requires exploratory scope"):
        pairrun.load_spec(path)


def test_natural_language_file_contract_binds_intent_policy_and_unit(tmp_path):
    repo, suite, run, suite_path, runner_path = _file_projection_run(
        tmp_path, "natural_language_file", queries=["find alphaTwo", "find alphaThree"]
    )
    for task in suite["tasks"]:
        task["query_intent"] = "semantic_intent"
        task["evaluation_contract"] = {
            "request_mode": qp.NATURAL_LANGUAGE_FILE_SEARCH,
            "gold_unit": "distinct_file",
            "result_unit": "distinct_file",
        }
    for row in run["results"]:
        row["score_evidence"] = "native_sdk_score_v1"
        for candidate in row["candidates"]:
            candidate["score"] = 1.0
    _pack, run = _repack(repo, suite, run)
    jsonschema.validate(suite, _load_schema("suite.schema.json"))
    jsonschema.validate(run, _load_schema("runner.schema.json"))
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    assert loaded_run["results"][0]["rank_unit"] == "distinct_file"
    assert loaded_run["results"][0]["candidates"][0]["span_accounting"]["unit_kind"] == "chunk"
    assert (
        ev.evaluate_diagnostic(loaded_suite, pack, loaded_run)["evaluation_contract"]
        == (suite["tasks"][0]["evaluation_contract"])
    )
    wrong_policy = copy.deepcopy(run)
    wrong_policy["captures"]["q0"]["execution_profile"] = qp.execution_profile("code_search_file")
    wrong_policy["captures"]["q0"]["execution_profile_sha256"] = ev.digest(
        ev.canonical(wrong_policy["captures"]["q0"]["execution_profile"])
    )
    with pytest.raises(ev.EvidenceError, match="request mode differs from bound product policy"):
        record_v3(repo, suite, wrong_policy, suite_path, runner_path)
    wrong_intent = copy.deepcopy(suite)
    wrong_intent["tasks"][0]["query_intent"] = "bare_symbol"
    with pytest.raises(ev.EvidenceError, match="requires independently judged semantic_intent"):
        ev.validate_suite(repo, wrong_intent)

    # A structurally source-valid CodeSearch file identity cannot be substituted
    # for the Native select:file representative returned by this policy.
    _, _, file_run, _, _ = _file_projection_run(tmp_path / "code-file", "code_search_file")
    forged_identity = copy.deepcopy(run)
    forged_identity["results"][0]["candidates"] = file_run["results"][0]["candidates"]
    for candidate in forged_identity["results"][0]["candidates"]:
        candidate["score"] = 1.0
    with pytest.raises(ev.EvidenceError, match="other profiles cannot claim it"):
        record_v3(repo, suite, forged_identity, suite_path, runner_path)


@pytest.mark.parametrize(
    "policy",
    ["code_search_exact_content_file", "code_search_typo_file", "code_search_components_file"],
)
def test_qualified_file_stage_refuses_nondefault_mode_before_capture(tmp_path, policy):
    spec = {
        "scope": "qualified",
        "execution_profiles": {
            "quanta": qp.execution_profile(policy),
            "semble": semble_adapter.execution_profile("lexical-file", None),
        },
    }
    with pytest.raises(pairrun.RunError, match="requires code_search_file"):
        pairrun._run_pair_staged(spec, tmp_path)
    assert not list(tmp_path.iterdir())


@pytest.mark.parametrize(
    "policy",
    ["code_search_exact_content_file", "code_search_typo_file", "code_search_components_file"],
)
def test_verdict_never_qualifies_nondefault_file_policy(tmp_path, policy):
    st = _pair_stage(tmp_path, claims={"quality": True})
    lock_path = st["stage"] / "protocol-lock.json"
    lock = json.loads(lock_path.read_text())
    lock["execution_profiles"]["quanta"]["policy"] = policy
    lock["execution_profiles_sha256"] = ev.digest(ev.canonical(lock["execution_profiles"]))
    lock_path.write_text(json.dumps(lock))
    verdict = _stage_verdict(st)
    assert verdict["states"]["QUALITY_DELTA"] == "fail"
    assert verdict["state_evidence"]["QUALITY_DELTA"]["reason"] == "diagnostic_rank_profile"


def test_verdict_quality_refuses_diagnostic_rank_profile(tmp_path, monkeypatch):
    st = _pair_stage(tmp_path, claims={"quality": True})
    monkeypatch.setattr(pairrun, "PAIR_CONTEXT_QUALITY_POLICIES", frozenset())
    verdict = _stage_verdict(st)
    assert verdict["states"]["QUALITY_DELTA"] == "fail"
    assert verdict["state_evidence"]["QUALITY_DELTA"]["reason"] == "diagnostic_rank_profile"
    assert verdict["failure_class"] == "scoring"


def test_current_native_refuses_unbound_file_projection_but_v4_replays():
    with pytest.raises(qp.QueryPlanError, match="requires an explicit rank profile"):
        qp.plan_lexical_request("native", "select:file Next")
    with pytest.raises(qp.QueryPlanError, match="requires an explicit rank profile"):
        qp.plan_lexical_request("native", "case:yes select:path Next")
    with pytest.raises(qp.QueryPlanError, match="requires an explicit rank profile"):
        qp.plan_lexical_request("native", "file:src/lib.rs select:file needle")
    for query in ("type:path needle", "type:repo needle"):
        with pytest.raises(qp.QueryPlanError, match="requires an explicit rank profile"):
            qp.plan_lexical_request("native", query)
    assert qp.plan_lexical_request("native", '"select:file Next"') == '"select:file Next"'
    assert qp.plan_lexical_request("native", "Next") == "Next"
    legacy = qp.derive_query_identity_v4("native", "select:file Next")
    assert legacy["effective_lexical_request_sha256"] == ev.digest(b"select:file Next")


def test_runner_schema_admits_only_the_named_file_profile():
    profile_schema = _load_schema("runner.schema.json")["$defs"]["execution_profile"]
    profile = qp.execution_profile("literal_file")
    jsonschema.validate(profile, profile_schema)
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate({**profile, "profile_id": "quanta-literal-v1"}, profile_schema)


def test_independent_judgments_are_source_bound_reviewed_and_blinded(tmp_path):
    repo, suite, _run, _sp, _rp, files = fixture_v3(tmp_path, answerable_only=True)
    suite["diagnostic_policy"] = ev.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
    for task in suite["tasks"]:
        task["label_review"] = {
            "assessment": "reviewed_ambiguous",
            "reviewer_id": "fixture-reviewer",
            "evidence_sha256": ev.digest(b"independent judgment review"),
        }
        task["judgment_policy"] = ev.UNJUDGED_POLICY
        task["file_judgments"] = [
            {"path": "a.txt", "file_sha256": ev.digest(files["a.txt"]), "grade": 3},
            {"path": "b.txt", "file_sha256": ev.digest(files["b.txt"]), "grade": 0},
        ]
        start, end = _byte_span(files["a.txt"], 2, 2)
        task["declaration_judgments"] = [
            {
                "path": "a.txt",
                "start_byte": start,
                "end_byte": end,
                "file_sha256": ev.digest(files["a.txt"]),
                "grade": 3,
            }
        ]
    jsonschema.validate(suite, _load_schema("suite.schema.json"))
    _loaded, pack, _source = ev.validate_suite(repo, suite)
    assert "file_judgments" not in ev.canonical(pack).decode()
    assert "declaration_judgments" not in ev.canonical(pack).decode()
    assert "fixture-reviewer" not in ev.canonical(pack).decode()
    complete_suite = copy.deepcopy(suite)
    for task in complete_suite["tasks"]:
        task["judgment_policy"] = ev.COMPLETE_JUDGMENT_POLICY
    jsonschema.validate(complete_suite, _load_schema("suite.schema.json"))
    ev.validate_suite(repo, complete_suite)

    mutations = (
        (
            lambda s: s["tasks"][0]["file_judgments"][0].update(file_sha256="0" * 64),
            "file hash mismatch",
        ),
        (lambda s: s["tasks"][0]["file_judgments"][0].update(path="excluded.txt"), "file excluded"),
        (
            lambda s: s["tasks"][0]["declaration_judgments"][0].update(end_byte=999),
            "invalid source byte span",
        ),
        (lambda s: s["tasks"][0].pop("judgment_policy"), "explicit unjudged_zero_v1"),
        (
            lambda s: s["tasks"][0]["label_review"].update(assessment="unreviewed"),
            "reviewed label requires",
        ),
        (
            lambda s: s["tasks"][0]["file_judgments"].append(
                copy.deepcopy(s["tasks"][0]["file_judgments"][0])
            ),
            "duplicate file_judgments",
        ),
        (
            lambda s: s["tasks"][0].update(judgment_policy=ev.COMPLETE_JUDGMENT_POLICY),
            "mixed judgment_policy",
        ),
    )
    for mutate, error in mutations:
        bad = copy.deepcopy(suite)
        mutate(bad)
        with pytest.raises(ev.EvidenceError, match=error):
            ev.validate_suite(repo, bad)


def test_declaration_exclusions_require_refusal_and_raw_name_absence(tmp_path):
    files = {
        "good.py": b"def Target():\n    pass\n",
        "refused.py": b"def broken(\n",
    }
    repo, commit = _write_repo(tmp_path, files)
    universe = [
        {"path": path, "file_sha256": ev.digest(raw)} for path, raw in sorted(files.items())
    ]
    suite = {
        "schema_version": 3,
        "suite_id": "partial-census",
        "repository_commit": commit,
        "comparison_contract": _v3_contract(),
        "routes": ["lexical"],
        "file_universe": universe,
        "file_universe_digest": ev.universe_digest(universe),
        "diagnostic_policy": ev.OBSERVED_PREFIX_DIAGNOSTIC_POLICY,
        "tasks": [
            {
                "task_id": "T1",
                "split": "eval",
                "query": "Target",
                "query_sha256": ev.digest(b"Target"),
                "query_family_id": "target-family",
                "query_intent": "bare_symbol",
                "answerable": True,
                "gold": [_v3_block(files, "good.py", 1, 1, grade=3)],
                "judgment_policy": ev.SOURCE_ORACLE_JUDGMENT_POLICY,
                "source_oracle": {
                    "contract": "python_exact_local_name_v1",
                    "unit": "distinct_file",
                    "declaration_exclusions": ["refused.py"],
                },
                "file_judgments": [
                    {
                        "path": "good.py",
                        "file_sha256": ev.digest(files["good.py"]),
                        "grade": 3,
                    }
                ],
            }
        ],
    }
    jsonschema.validate(suite, _load_schema("suite.schema.json"))
    _loaded, pack, _source = ev.validate_suite(repo, suite)
    assert len(_loaded["file_universe"]) == 2
    assert "declaration_exclusions" not in ev.canonical(pack).decode()

    def rejected(change, match):
        bad = copy.deepcopy(suite)
        change(bad["tasks"][0])
        with pytest.raises(ev.EvidenceError, match=match):
            ev.validate_suite(repo, bad)

    rejected(lambda row: row["source_oracle"].pop("declaration_exclusions"), "parse error")
    rejected(
        lambda row: row["source_oracle"].update(declaration_exclusions=["good.py"]),
        "may contain a query match",
    )
    rejected(
        lambda row: row["source_oracle"].update(declaration_exclusions=["missing.py"]),
        "invalid declaration exclusions",
    )
    rejected(
        lambda row: row["source_oracle"].update(
            declaration_exclusions=["refused.py", "refused.py"]
        ),
        "invalid declaration exclusions",
    )
    rejected(
        lambda row: row["source_oracle"].update(contract="ascii_identifier_word_v1"),
        "invalid declaration exclusions",
    )
    rejected(
        lambda row: row["file_judgments"].clear(),
        "lacks a positive judgment",
    )
    refused_with_name = copy.deepcopy(suite)
    refused_with_name["tasks"][0]["query"] = "broken"
    refused_with_name["tasks"][0]["query_sha256"] = ev.digest(b"broken")
    with pytest.raises(ev.EvidenceError, match="may contain a query match"):
        ev.validate_suite(repo, refused_with_name)
    for contract, query, error in (
        ("ascii_content_absent_casefold_v1", "broken", "content absent contract found a match"),
        ("ascii_code_search_absent_casefold_v1", "refused", "path match"),
    ):
        absent = copy.deepcopy(suite)
        row = absent["tasks"][0]
        row.update(
            query=query,
            query_sha256=ev.digest(query.encode()),
            answerable=False,
            gold=[],
            file_judgments=[],
        )
        row["source_oracle"] = {"contract": contract, "unit": "distinct_file"}
        with pytest.raises(ev.EvidenceError, match=error):
            ev.validate_suite(repo, absent)


def test_source_oracle_recomputes_exhaustive_go_and_identifier_judgments(tmp_path):
    files = {
        "a.go": b"package demo\ntype Param struct{}\nfunc (p *Param) Next() {}\n// 1Param is not an identifier word.\n",
        "b.go": b"package demo\n// Param is a use site.\nfunc Next() {}\n",
        "c.go": b"package demo\n// ParamExtra and param are different names.\n",
        "d.go": b"package demo\n// 1Param is not an identifier word.\n",
    }
    repo, commit = _write_repo(tmp_path, files)
    universe = [
        {"path": path, "file_sha256": ev.digest(raw)} for path, raw in sorted(files.items())
    ]
    suite = {
        "schema_version": 3,
        "suite_id": "source-oracle-contract",
        "repository_commit": commit,
        "comparison_contract": _v3_contract(),
        "routes": ["lexical"],
        "file_universe": universe,
        "file_universe_digest": ev.universe_digest(universe),
        "diagnostic_policy": ev.OBSERVED_PREFIX_DIAGNOSTIC_POLICY,
        "tasks": [],
    }

    def task(query, contract, unit, judgments, gold_path="a.go", gold_line=2):
        return {
            "task_id": "T1",
            "split": "eval",
            "query": query,
            "query_sha256": ev.digest(query.encode()),
            "query_family_id": "oracle-family",
            "query_intent": "bare_symbol",
            "answerable": bool(judgments),
            "gold": [_v3_block(files, gold_path, gold_line, gold_line, grade=3)]
            if judgments
            else [],
            "judgment_policy": ev.SOURCE_ORACLE_JUDGMENT_POLICY,
            "source_oracle": {"contract": contract, "unit": unit},
            "declaration_judgments" if unit == "symbol" else "file_judgments": judgments,
        }

    def file_row(path):
        return {"path": path, "file_sha256": ev.digest(files[path]), "grade": 3}

    def declaration_row(path, definition):
        start = files[path].index(definition)
        return {**file_row(path), "start_byte": start, "end_byte": start + len(definition)}

    cases = [
        task("Param", "go_exact_local_name_v3", "distinct_file", [file_row("a.go")]),
        task(
            "Param",
            "ascii_identifier_word_v1",
            "distinct_file",
            [file_row("a.go"), file_row("b.go")],
        ),
        task(
            "Next",
            "go_exact_local_name_v3",
            "symbol",
            [
                declaration_row("a.go", b"func (p *Param) Next() {}"),
                declaration_row("b.go", b"func Next() {}"),
            ],
            gold_line=3,
        ),
        task("NoSuchName", "go_exact_local_name_v3", "symbol", []),
    ]
    for candidate in cases:
        suite["tasks"] = [candidate]
        jsonschema.validate(suite, _load_schema("suite.schema.json"))
        _loaded, pack, _source = ev.validate_suite(repo, suite)
        assert "source_oracle" not in ev.canonical(pack).decode()
        assert "judgments" not in ev.canonical(pack).decode()

    index = ev.source_oracle.SourceOracleIndex(
        {path: (raw, ev.digest(raw)) for path, raw in files.items()}, {"Param", "param"}
    )
    assert [
        row["path"]
        for row in index.expected_rows("ascii_identifier_word_v1", "Param", "distinct_file")
    ] == ["a.go", "b.go"]
    assert [
        row["path"]
        for row in index.expected_rows("ascii_identifier_word_v1", "param", "distinct_file")
    ] == ["c.go"]
    assert not ev.source_oracle.has_identifier_word_in_span(b"1Param", b"Param", 1, 6)
    assert not ev.source_oracle.has_identifier_word_in_span(b"ParamExtra", b"Param", 0, 5)
    assert ev.source_oracle.has_identifier_word_in_span(b" Param ", b"Param", 1, 6)
    assert index.expected_rows("go_exact_local_name_v3", "param", "symbol") == []
    invalid_go = b"package demo\nfunc Next(\n"
    invalid_index = ev.source_oracle.SourceOracleIndex(
        {"invalid.go": (invalid_go, ev.digest(invalid_go))}, {"Next"}
    )
    with pytest.raises(ev.source_oracle.SourceOracleError, match="parse error"):
        invalid_index.expected_rows("go_exact_local_name_v3", "Next", "symbol")

    suite["tasks"] = [cases[2]]
    bad_declaration = copy.deepcopy(suite)
    bad_declaration["tasks"][0]["declaration_judgments"].pop()
    with pytest.raises(ev.EvidenceError, match="differ from frozen source"):
        ev.validate_suite(repo, bad_declaration)

    suite["tasks"] = [cases[1]]
    diagnostic = ev.judgment_diagnostics(
        suite,
        {
            "route_provenance": {"lexical": {"capture_id": "q0"}},
            "captures": {"q0": {"system": "quanta"}},
        },
        {
            ("T1", "lexical"): {
                "status": "success",
                "rank_unit": "distinct_file",
                "candidates": [
                    {"path": path, "rank": rank}
                    for rank, path in enumerate(("c.go", "a.go", "b.go"), 1)
                ],
            }
        },
        {"T1": cases[1]},
        "lexical",
        None,
    )
    assert diagnostic["unjudged_policy"] == ev.SOURCE_ORACLE_JUDGMENT_POLICY
    route = diagnostic["file_judgments"]["routes"]["lexical"]
    assert route["eligible_task_ids"] == ["T1"]
    assert route["operational_mean"]["ndcg_at_10"] == pytest.approx(
        (7 / math.log2(3) + 7 / math.log2(4)) / (7 + 7 / math.log2(3))
    )

    mutations = (
        (lambda row: row["file_judgments"].pop(), "differ from frozen source"),
        (lambda row: row["file_judgments"].append(file_row("c.go")), "differ from frozen source"),
        (lambda row: row["file_judgments"].append(file_row("d.go")), "differ from frozen source"),
        (lambda row: row["file_judgments"][0].update(grade=2), "differ from frozen source"),
        (
            lambda row: row.update(label_review={"assessment": "unreviewed"}),
            "cannot claim human review",
        ),
        (
            lambda row: row.update(judgment_policy=ev.UNJUDGED_POLICY),
            "requires source_oracle_complete_v1",
        ),
        (lambda row: row.update(query_intent="semantic_intent"), "requires bare_symbol"),
        (lambda row: row.update(gold=[_v3_block(files, "c.go", 2, 2)]), "gold path contradicts"),
        (lambda row: row.update(gold=[_v3_block(files, "a.go", 1, 1)]), "gold span contradicts"),
        (lambda row: row.update(gold=[_v3_block(files, "a.go", 4, 4)]), "gold span contradicts"),
    )
    for mutate, error in mutations:
        bad = copy.deepcopy(suite)
        mutate(bad["tasks"][0])
        with pytest.raises(ev.EvidenceError, match=error):
            ev.validate_suite(repo, bad)

    bad = copy.deepcopy(suite)
    bad["tasks"][0]["source_oracle"]["unit"] = "symbol"
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(bad, _load_schema("suite.schema.json"))
    with pytest.raises(ev.EvidenceError, match="judgment unit mismatch"):
        ev.validate_suite(repo, bad)

    reviewed = suite["tasks"][0]
    review_receipt = {
        "schema_version": 1,
        "reviewer_id": "fixture-reviewer",
        "suite_sha256": ev.digest(ev.canonical(suite)),
        "reviews": [
            {
                "task_id": reviewed["task_id"],
                "query_sha256": reviewed["query_sha256"],
                "labels": pairrun._gold_review_labels(reviewed),
                "rationale": "fixture review",
            }
        ],
    }
    with pytest.raises(pairrun.RunError, match="mechanical source-oracle labels"):
        pairrun._validate_gold_review_receipt(
            review_receipt,
            role="annotation 1",
            reviewer_id="fixture-reviewer",
            suite_sha256=review_receipt["suite_sha256"],
            suite=suite,
            repo=repo,
        )
    mechanical_only_receipt = {
        "schema_version": 2,
        "reviewer_id": "fixture-reviewer",
        "suite_sha256": review_receipt["suite_sha256"],
        "reviews": [],
    }
    with pytest.raises(pairrun.RunError, match="lacks human-reviewed tasks"):
        pairrun._validate_gold_review_receipt(
            mechanical_only_receipt,
            role="annotation 1",
            reviewer_id="fixture-reviewer",
            suite_sha256=mechanical_only_receipt["suite_sha256"],
            suite=suite,
            repo=repo,
            allow_mixed_source_oracle=True,
        )

    subjective = task(
        "Next",
        "go_exact_local_name_v3",
        "distinct_file",
        [file_row("b.go")],
        gold_path="b.go",
        gold_line=2,
    )
    subjective["task_id"] = "T2"
    subjective["query_family_id"] = "reviewed-family"
    del subjective["source_oracle"]
    subjective["judgment_policy"] = ev.COMPLETE_JUDGMENT_POLICY
    subjective["label_review"] = {
        "assessment": "reviewed_unambiguous",
        "reviewer_id": "fixture-reviewer",
        "evidence_sha256": ev.digest(b"independent review evidence"),
    }
    objective_no_answer = task(
        "NoSuchName", "ascii_content_absent_casefold_v1", "distinct_file", []
    )
    objective_no_answer["task_id"] = "T3"
    objective_no_answer["query_family_id"] = "objective-no-answer-family"
    suite["tasks"] = [cases[1], subjective, objective_no_answer]
    jsonschema.validate(suite, _load_schema("suite.schema.json"))
    ev.validate_suite(repo, suite)
    mixed_receipt = {
        "schema_version": 2,
        "reviewer_id": "fixture-reviewer",
        "suite_sha256": ev.digest(ev.canonical(suite)),
        "reviews": [
            {
                "task_id": subjective["task_id"],
                "query_sha256": subjective["query_sha256"],
                "labels": pairrun._gold_review_labels(subjective),
                "rationale": "Source-backed independent file review.",
            }
        ],
    }
    pairrun._validate_gold_review_receipt(
        mixed_receipt,
        role="annotation 1",
        reviewer_id="fixture-reviewer",
        suite_sha256=mixed_receipt["suite_sha256"],
        suite=suite,
        repo=repo,
        allow_mixed_source_oracle=True,
    )
    adjudication = copy.deepcopy(mixed_receipt)
    adjudication["annotation_receipt_sha256"] = ["a" * 64, "b" * 64]
    pairrun._validate_gold_review_receipt(
        adjudication,
        role="adjudication",
        reviewer_id="fixture-reviewer",
        suite_sha256=mixed_receipt["suite_sha256"],
        suite=suite,
        repo=repo,
        annotation_digests=["a" * 64, "b" * 64],
        allow_mixed_source_oracle=True,
    )
    wrong_version = copy.deepcopy(mixed_receipt)
    wrong_version["schema_version"] = 1
    with pytest.raises(pairrun.RunError, match="schema version mismatch"):
        pairrun._validate_gold_review_receipt(
            wrong_version,
            role="annotation 1",
            reviewer_id="fixture-reviewer",
            suite_sha256=mixed_receipt["suite_sha256"],
            suite=suite,
            repo=repo,
            allow_mixed_source_oracle=True,
        )
    with pytest.raises(pairrun.RunError, match="schema version mismatch"):
        pairrun._validate_gold_review_receipt(
            mixed_receipt,
            role="annotation 1",
            reviewer_id="fixture-reviewer",
            suite_sha256=mixed_receipt["suite_sha256"],
            suite=suite,
            repo=repo,
        )
    missing_review = copy.deepcopy(mixed_receipt)
    missing_review["reviews"] = []
    with pytest.raises(pairrun.RunError, match="task coverage mismatch"):
        pairrun._validate_gold_review_receipt(
            missing_review,
            role="annotation 1",
            reviewer_id="fixture-reviewer",
            suite_sha256=mixed_receipt["suite_sha256"],
            suite=suite,
            repo=repo,
            allow_mixed_source_oracle=True,
        )
    adjudication["reviews"][0]["labels"]["file_judgments"][0]["grade"] = 2
    with pytest.raises(pairrun.RunError, match="adjudication differs from suite gold"):
        pairrun._validate_gold_review_receipt(
            adjudication,
            role="adjudication",
            reviewer_id="fixture-reviewer",
            suite_sha256=mixed_receipt["suite_sha256"],
            suite=suite,
            repo=repo,
            annotation_digests=["a" * 64, "b" * 64],
            allow_mixed_source_oracle=True,
        )


def test_cross_suite_custody_counts_positive_independent_file_judgments(tmp_path):
    repo, suite, _run, _sp, _rp, files = fixture_v3(tmp_path, answerable_only=True)
    development = copy.deepcopy(suite)
    holdout = copy.deepcopy(suite)
    development["suite_id"] = "development-reviewed-file"
    holdout["suite_id"] = "holdout-reviewed-file"
    development["tasks"] = [development["tasks"][0]]
    development["tasks"][0]["gold"] = [development["tasks"][0]["gold"][0]]
    holdout["tasks"] = [holdout["tasks"][1]]
    holdout["tasks"][0]["gold"] = [_v3_block(files, "b.txt", 1, 1, grade=2)]
    for candidate in (development, holdout):
        candidate["diagnostic_policy"] = ev.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
        task = candidate["tasks"][0]
        task["label_review"] = {
            "assessment": "reviewed_unambiguous",
            "reviewer_id": "fixture-reviewer",
            "evidence_sha256": ev.digest(b"independent review"),
        }
        task["judgment_policy"] = ev.UNJUDGED_POLICY
        task["file_judgments"] = [
            {"path": "b.txt", "file_sha256": ev.digest(files["b.txt"]), "grade": 3}
        ]
    manifest = {
        "schema_version": 1,
        "source_revision": "a" * 40,
        "repository_commit": suite["repository_commit"],
        "development_suite_sha256": ev.digest(ev.canonical(development)),
        "holdout_suite_sha256": ev.digest(ev.canonical(holdout)),
    }
    with pytest.raises(ev.EvidenceError, match="cross-suite file leakage"):
        ev.validate_experiment_custody(repo, manifest, development, holdout)


def test_train_eval_split_blocks_independent_file_judgment_leakage(tmp_path):
    repo, suite, _run, _sp, _rp, files = fixture_v3(tmp_path, answerable_only=True)
    suite["diagnostic_policy"] = ev.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
    suite["tasks"][0]["gold"] = [suite["tasks"][0]["gold"][0]]  # eval gold a.txt
    suite["tasks"][1]["split"] = "train"
    suite["tasks"][1]["gold"] = [_v3_block(files, "b.txt", 1, 1, grade=2)]
    for task in suite["tasks"]:
        task["label_review"] = {
            "assessment": "reviewed_unambiguous",
            "reviewer_id": "fixture-reviewer",
            "evidence_sha256": ev.digest(b"reviewed fixture"),
        }
        task["judgment_policy"] = ev.UNJUDGED_POLICY
        task["file_judgments"] = [
            {"path": "b.txt", "file_sha256": ev.digest(files["b.txt"]), "grade": 3}
        ]
    with pytest.raises(ev.EvidenceError, match="independent judgment file leakage"):
        ev.validate_suite(repo, suite)


def test_complete_ranked_pool_excludes_unjudged_file_and_declaration_results():
    file_suite = {"comparison_contract": {"top_k": 10}, "routes": ["lexical"]}
    file_run = {
        "route_provenance": {"lexical": {"capture_id": "q0"}},
        "captures": {"q0": {"system": "quanta"}},
    }
    file_task = {
        "answerable": True,
        "judgment_policy": ev.COMPLETE_JUDGMENT_POLICY,
        "file_judgments": [{"path": "answer.go", "grade": 3}],
    }
    file_result = {
        ("F", "lexical"): {
            "status": "success",
            "rank_unit": "distinct_file",
            "candidates": [
                {"path": "answer.go", "rank": 1},
                {"path": "unknown.go", "rank": 2},
            ],
        }
    }
    file_report = ev.judgment_diagnostics(
        file_suite, file_run, file_result, {"F": file_task}, "lexical", None
    )
    assert file_report["unjudged_policy"] == ev.COMPLETE_JUDGMENT_POLICY
    assert file_report["file_judgments"]["routes"]["lexical"]["excluded"] == [
        {"task_id": "F", "reason": "unjudged_ranked_file"}
    ]
    assert set(file_report["file_judgments"]["routes"]["lexical"]["operational_mean"].values()) == {
        ev.NOT_APPLICABLE
    }
    file_task["file_judgments"].append({"path": "unknown.go", "grade": 0})
    file_report = ev.judgment_diagnostics(
        file_suite, file_run, file_result, {"F": file_task}, "lexical", None
    )
    assert file_report["file_judgments"]["routes"]["lexical"]["eligible_task_ids"] == ["F"]
    assert (
        file_report["file_judgments"]["routes"]["lexical"]["operational_mean"]["ndcg_at_10"] == 1.0
    )
    file_task["file_judgments"].pop()
    file_task["judgment_policy"] = ev.UNJUDGED_POLICY
    legacy = ev.judgment_diagnostics(
        file_suite, file_run, file_result, {"F": file_task}, "lexical", None
    )
    assert legacy["file_judgments"]["routes"]["lexical"]["eligible_task_ids"] == ["F"]
    assert legacy["file_judgments"]["routes"]["lexical"]["operational_mean"]["ndcg_at_10"] == 1.0

    symbol_suite = {"comparison_contract": {"top_k": 10}, "routes": ["symbol"]}
    symbol_run = {
        "span_accounting_version": 1,
        "route_provenance": {"symbol": {"capture_id": "q0"}},
        "captures": {"q0": {"system": "quanta"}},
    }
    symbol_task = {
        "answerable": True,
        "judgment_policy": ev.COMPLETE_JUDGMENT_POLICY,
        "declaration_judgments": [
            {"path": "answer.go", "start_byte": 10, "end_byte": 20, "grade": 3}
        ],
    }

    def symbol_candidate(path, start, end, unit_id):
        return {
            "path": path,
            "rank": 1 if unit_id == "answer" else 2,
            "span_accounting": {
                "unit_kind": "symbol",
                "unit_id": unit_id,
                "indexed_start_byte": start,
                "indexed_end_byte": end,
            },
        }

    symbol_result = {
        ("D", "symbol"): {
            "status": "success",
            "rank_unit": "symbol",
            "candidates": [
                symbol_candidate("answer.go", 10, 20, "answer"),
                symbol_candidate("other.go", 30, 40, "other"),
            ],
        }
    }
    symbol_report = ev.judgment_diagnostics(
        symbol_suite, symbol_run, symbol_result, {"D": symbol_task}, "symbol", None
    )
    assert symbol_report["declaration_judgments"]["routes"]["symbol"]["excluded"] == [
        {"task_id": "D", "reason": "unjudged_ranked_declaration"}
    ]
    assert set(
        symbol_report["declaration_judgments"]["routes"]["symbol"]["operational_mean"].values()
    ) == {ev.NOT_APPLICABLE}
    symbol_task["declaration_judgments"].append(
        {"path": "other.go", "start_byte": 30, "end_byte": 40, "grade": 0}
    )
    symbol_report = ev.judgment_diagnostics(
        symbol_suite, symbol_run, symbol_result, {"D": symbol_task}, "symbol", None
    )
    assert symbol_report["declaration_judgments"]["routes"]["symbol"]["eligible_task_ids"] == ["D"]


def test_incomplete_judgments_do_not_become_operational_search_failures():
    tasks = {
        task_id: {
            "answerable": True,
            "judgment_policy": ev.COMPLETE_JUDGMENT_POLICY,
            "file_judgments": [{"path": "answer.go", "grade": 3}],
        }
        for task_id in ("judged", "unknown")
    }
    suite = {"comparison_contract": {"top_k": 10}, "routes": ["lexical"]}
    run = {
        "route_provenance": {"lexical": {"capture_id": "q0"}},
        "captures": {"q0": {"system": "quanta"}},
    }
    results = {
        (task_id, "lexical"): {
            "status": "success",
            "rank_unit": "distinct_file",
            "candidates": [{"path": path, "rank": 1}],
        }
        for task_id, path in (("judged", "answer.go"), ("unknown", "unknown.go"))
    }

    def scores():
        return ev.judgment_diagnostics(suite, run, results, tasks, "lexical", None)[
            "file_judgments"
        ]["routes"]["lexical"]

    report = scores()
    assert report["eligible_task_ids"] == ["judged"]
    assert report["coverage"] == 0.5
    assert report["conditional_mean"]["ndcg_at_10"] == 1.0
    assert set(report["operational_mean"].values()) == {ev.NOT_APPLICABLE}
    assert report["operational_unavailable_reason"] == "incomplete_ranked_judgments"

    tasks["unknown"]["file_judgments"].append({"path": "unknown.go", "grade": 0})
    judged = scores()
    assert judged["operational_mean"]["ndcg_at_10"] == 0.5
    assert judged["conditional_mean"]["ndcg_at_10"] == 0.5
    assert "operational_unavailable_reason" not in judged

    tasks["unknown"]["file_judgments"].pop()
    results[("unknown", "lexical")].update(status="timeout", candidates=[])
    failed = scores()
    assert failed["operational_mean"]["ndcg_at_10"] == 0.5
    assert failed["conditional_mean"]["ndcg_at_10"] == 1.0
    assert failed["excluded"] == [{"task_id": "unknown", "reason": "execution_status_timeout"}]
    assert "operational_unavailable_reason" not in failed


def test_independent_file_quality_uses_common_eligible_cohort():
    tasks = {
        "easy": {
            "answerable": True,
            "category": "easy",
            "file_judgments": [{"path": "gold-easy.go", "grade": 3}],
        },
        "hard": {
            "answerable": True,
            "category": "hard",
            "file_judgments": [{"path": "gold-hard.go", "grade": 3}],
        },
    }
    suite = {"comparison_contract": {"top_k": 10}, "routes": ["q", "s"]}
    run = {
        "route_provenance": {"q": {"capture_id": "q0"}, "s": {"capture_id": "s0"}},
        "captures": {"q0": {"system": "quanta"}, "s0": {"system": "semble"}},
    }

    def row(task_id, route, status, paths):
        return {
            "task_id": task_id,
            "route": route,
            "status": status,
            "rank_unit": "distinct_file",
            "candidates": [{"path": path, "rank": rank} for rank, path in enumerate(paths, 1)],
        }

    filler = [f"filler-{index}.go" for index in range(9)]
    results = {
        ("easy", "q"): row("easy", "q", "capped", ["gold-easy.go", *filler]),
        ("easy", "s"): row("easy", "s", "success", ["gold-easy.go", *filler]),
        ("hard", "q"): row("hard", "q", "capped", ["gold-hard.go", *filler[:1]]),
        ("hard", "s"): row("hard", "s", "success", [*filler, "gold-hard.go"]),
    }
    results[("easy", "s")].pop("rank_unit")
    results[("hard", "s")].pop("rank_unit")
    report = ev.judgment_diagnostics(suite, run, results, tasks, "q", "s")
    assert report is not None
    file_view = report["file_judgments"]
    q = file_view["routes"]["q"]
    assert q["eligible_task_ids"] == ["easy"]
    assert q["excluded"] == [{"task_id": "hard", "reason": "insufficient_depth_without_exhaustion"}]
    assert q["coverage"] == 0.5
    assert q["operational_mean"]["ndcg_at_10"] == 0.5
    assert q["conditional_mean"]["ndcg_at_10"] == 1.0
    comparison = file_view["comparison"]
    assert file_view["routes"]["s"]["excluded"] == [
        {"task_id": "easy", "reason": "rank_unit_mismatch"},
        {"task_id": "hard", "reason": "rank_unit_mismatch"},
    ]
    assert comparison["eligible_task_ids"] == []
    assert comparison["sample_count"] == 0
    assert comparison["coverage"] == 0.0
    assert comparison["delta"]["ndcg_at_10"] == ev.NOT_APPLICABLE
    results[("hard", "q")]["status"] = "success"  # Quanta source binds success to exhaustion.
    full = ev.judgment_diagnostics(suite, run, results, tasks, "q", "s")
    assert full is not None
    assert full["file_judgments"]["routes"]["q"]["eligible_task_ids"] == ["easy", "hard"]
    assert full["file_judgments"]["comparison"]["eligible_task_ids"] == []


def test_literal_file_record_binds_rank_unit_and_independent_score(tmp_path):
    repo, suite, run, suite_path, runner_path, files = fixture_v3(tmp_path, answerable_only=True)
    suite["routes"] = ["lexical", "reference"]
    suite["diagnostic_policy"] = ev.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
    for task in suite["tasks"]:
        task["label_review"] = {
            "assessment": "reviewed_unambiguous",
            "reviewer_id": "fixture-reviewer",
            "evidence_sha256": ev.digest(b"reviewed fixture"),
        }
        task["judgment_policy"] = ev.UNJUDGED_POLICY
        task["file_judgments"] = [
            {"path": "a.txt", "file_sha256": ev.digest(files["a.txt"]), "grade": 3},
            {"path": "b.txt", "file_sha256": ev.digest(files["b.txt"]), "grade": 1},
        ]
    profile = qp.execution_profile("literal_file")
    run["captures"]["q0"]["execution_profile"] = profile
    run["captures"]["q0"]["execution_profile_sha256"] = ev.digest(ev.canonical(profile))
    run["captures"]["s0"] = _v3_capture("semble", current=True)
    run["route_provenance"] = {
        "lexical": {"capture_id": "q0"},
        "reference": {"capture_id": "s0"},
    }
    results = []
    for row in run["results"]:
        row = copy.deepcopy(row)
        task = next(task for task in suite["tasks"] if task["task_id"] == row["task_id"])
        if row["route"] == "lexical":
            row["rank_unit"] = "distinct_file"
            row["query_identity"] = qp.derive_query_identity("literal_file", task["query"])
        else:
            row["route"] = "reference"
            row["candidates"] = [item for item in row["candidates"] if item["path"] == "a.txt"][:1]
            digest = ev.digest(task["query"].encode())
            row["query_identity"] = {
                "original_query_sha256": digest,
                "submitted_query_sha256": digest,
            }
        results.append(row)
    run["results"] = results
    _pack, run = _repack(repo, suite, run)
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    report = ev.evaluate(loaded_suite, pack, loaded_run, "lexical", "reference", strict_k=True)
    independent = report["judgment_metrics"]["file_judgments"]
    assert independent["routes"]["lexical"]["eligible_task_ids"] == ["T1", "T2"]
    assert independent["routes"]["lexical"]["operational_mean"]["ndcg_at_10"] > 0
    assert independent["routes"]["reference"]["eligible_task_ids"] == []

    single_suite = copy.deepcopy(suite)
    single_suite["routes"] = ["lexical"]
    single_run = copy.deepcopy(run)
    single_run["route_provenance"] = {"lexical": {"capture_id": "q0"}}
    single_run["results"] = [row for row in single_run["results"] if row["route"] == "lexical"]
    _single_pack, single_run = _repack(repo, single_suite, single_run)
    single_suite_path = tmp_path / "single-suite.json"
    single_run_path = tmp_path / "single-run.json"
    record_v3(repo, single_suite, single_run, single_suite_path, single_run_path)
    diagnostic_path = tmp_path / "single-diagnostic.json"
    assert (
        ev.main(
            [
                "evaluate-diagnostic",
                "--repo",
                str(repo),
                "--suite",
                str(single_suite_path),
                "--runner",
                str(single_run_path),
                "--output",
                str(diagnostic_path),
            ]
        )
        == 0
    )
    diagnostic = json.loads(diagnostic_path.read_text())
    assert diagnostic["report_scope"] == "single_route_independent_judgment_diagnostic_v1"
    assert diagnostic["status"] == "diagnostic_unqualified"
    assert diagnostic["qualification"] == ev.NOT_APPLICABLE
    assert diagnostic["paired_comparison"] == ev.NOT_APPLICABLE
    assert diagnostic["quality_delta_gate"] == ev.NOT_APPLICABLE
    assert diagnostic["judgment_metrics"]["file_judgments"]["comparison"] == ev.NOT_APPLICABLE
    assert (
        diagnostic["judgment_metrics"]["file_judgments"]["routes"]["lexical"]["eligible_count"] == 2
    )
    assert diagnostic["no_answer"]["sample_count"] == 0
    assert diagnostic["runner_record_sha256"] == ev.digest(ev.canonical(single_run))

    duplicate = copy.deepcopy(run)
    repeated = copy.deepcopy(duplicate["results"][0]["candidates"][0])
    repeated["rank"] = 3
    repeated.update(_v3_block(files, "a.txt", 4, 4, tokens=True, rank=3))
    duplicate["results"][0]["candidates"].append(repeated)
    with pytest.raises(ev.EvidenceError, match="duplicate file in distinct_file result"):
        record_v3(repo, suite, duplicate, suite_path, runner_path)
    missing_unit = copy.deepcopy(run)
    missing_unit["results"][0].pop("rank_unit")
    with pytest.raises(ev.EvidenceError, match="requires lexical distinct_file"):
        record_v3(repo, suite, missing_unit, suite_path, runner_path)
    forged_reference = copy.deepcopy(run)
    forged_reference["results"][1]["rank_unit"] = "distinct_file"
    with pytest.raises(ev.EvidenceError, match="Semble capture has no verified distinct_file"):
        record_v3(repo, suite, forged_reference, suite_path, runner_path)


@pytest.mark.parametrize("status", ["abstained", "error", "timeout", "capped"])
def test_exhausted_empty_answerable_query_stays_in_quality_denominator(tmp_path, status):
    repo, suite, run, suite_path, runner_path, files = fixture_v3(tmp_path, answerable_only=True)
    suite["routes"] = ["lexical"]
    suite["diagnostic_policy"] = ev.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
    for task in suite["tasks"]:
        task["label_review"] = {
            "assessment": "reviewed_unambiguous",
            "reviewer_id": "fixture-reviewer",
            "evidence_sha256": ev.digest(b"independent fixed file judgment"),
        }
        task["judgment_policy"] = ev.UNJUDGED_POLICY
        task["file_judgments"] = [
            {"path": "a.txt", "file_sha256": ev.digest(files["a.txt"]), "grade": 3},
            {"path": "b.txt", "file_sha256": ev.digest(files["b.txt"]), "grade": 0},
        ]
    profile = qp.execution_profile("literal_file")
    run["captures"]["q0"]["execution_profile"] = profile
    run["captures"]["q0"]["execution_profile_sha256"] = ev.digest(ev.canonical(profile))
    run["route_provenance"] = {"lexical": {"capture_id": "q0"}}
    run["results"] = [row for row in run["results"] if row["route"] == "lexical"]
    for task, row in zip(suite["tasks"], run["results"], strict=True):
        row["rank_unit"] = "distinct_file"
        row["query_identity"] = qp.derive_query_identity("literal_file", task["query"])
    missing = run["results"][1]
    missing["status"] = status
    missing["candidates"] = (
        [_v3_block(files, "b.txt", 1, 1, tokens=True, rank=1)] if status == "capped" else []
    )
    missing["error"] = (
        {"code": status, "message": "execution failed"} if status in ("error", "timeout") else None
    )
    _pack, run = _repack(repo, suite, run)
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    report = ev.evaluate_diagnostic(loaded_suite, pack, loaded_run)
    route = report["judgment_metrics"]["file_judgments"]["routes"]["lexical"]
    assert route["operational_mean"]["ndcg_at_10"] == 0.5
    assert route["eligible_task_ids"] == (["T1", "T2"] if status == "abstained" else ["T1"])
    assert route["conditional_mean"]["ndcg_at_10"] == (0.5 if status == "abstained" else 1.0)


def test_single_route_diagnostic_keeps_no_answer_status_distinct():
    suite = {
        "schema_version": 3,
        "suite_id": "synthetic-single",
        "repository_commit": "a" * 40,
        "comparison_contract": {"top_k": 10},
        "routes": ["lexical"],
        "tasks": [
            {
                "task_id": "A",
                "split": "eval",
                "answerable": True,
                "file_judgments": [{"path": "a.go", "grade": 3}],
            },
            {"task_id": "N", "split": "eval", "answerable": False, "file_judgments": []},
        ],
    }
    run = {
        "comparison_contract": {"top_k": 10},
        "route_provenance": {"lexical": {"capture_id": "q0"}},
        "captures": {"q0": {"system": "quanta"}},
        "runner": {},
        "results": [
            {
                "task_id": "A",
                "route": "lexical",
                "status": "success",
                "rank_unit": "distinct_file",
                "candidates": [{"path": "a.go", "rank": 1}],
            },
            {
                "task_id": "N",
                "route": "lexical",
                "status": "timeout",
                "candidates": [],
            },
        ],
    }
    diagnostic = ev.evaluate_diagnostic(suite, {}, run)
    assert diagnostic["reference_contracts"] == [
        {
            "source_oracle_contract": "not_declared",
            "gold_unit": "not_declared",
            "query_intent": "not_declared",
        }
    ]
    assert diagnostic["no_answer"] == {
        "task_ids": ["N"],
        "sample_count": 1,
        "reference_contracts": [
            {
                "source_oracle_contract": "not_declared",
                "gold_unit": "not_declared",
                "query_intent": "not_declared",
            }
        ],
        "abstained": 0,
        "abstention_rate": 0.0,
        "nonempty_results": 0,
        "nonempty_result_rate": 0.0,
        "status_counts": {"timeout": 1},
    }
    run["results"][1]["status"] = "abstained"
    assert ev.evaluate_diagnostic(suite, {}, run)["no_answer"]["abstention_rate"] == 1.0


def test_no_answer_diagnostic_separates_nonempty_results_from_failures():
    tasks = {task_id: {"answerable": False} for task_id in ("A", "B", "C")}
    results = {
        ("A", "lexical"): {"status": "success", "candidates": [{"path": "wrong.go", "rank": 1}]},
        ("B", "lexical"): {"status": "abstained", "candidates": []},
        ("C", "lexical"): {"status": "timeout", "candidates": []},
    }
    summary = ev.no_answer_diagnostics(tasks, results, "lexical")
    assert summary == {
        "task_ids": ["A", "B", "C"],
        "sample_count": 3,
        "reference_contracts": [
            {
                "source_oracle_contract": "not_declared",
                "gold_unit": "not_declared",
                "query_intent": "not_declared",
            }
        ],
        "abstained": 1,
        "abstention_rate": 1 / 3,
        "nonempty_results": 1,
        "nonempty_result_rate": 1 / 3,
        "status_counts": {"success": 1, "abstained": 1, "timeout": 1},
    }
    tasks["A"]["source_oracle"] = {
        "contract": "ascii_content_absent_casefold_v1",
        "unit": "distinct_file",
    }
    scoped = ev.no_answer_diagnostics(tasks, results, "lexical")
    assert scoped["reference_contracts"] == [
        {
            "source_oracle_contract": "ascii_content_absent_casefold_v1",
            "gold_unit": "distinct_file",
            "query_intent": "not_declared",
        },
        {
            "source_oracle_contract": "not_declared",
            "gold_unit": "not_declared",
            "query_intent": "not_declared",
        },
    ]


def test_v3_universe_binding(tmp_path):
    repo, suite, _run, _sp, _rp, _files = fixture_v3(tmp_path)
    mutated = json.loads(json.dumps(suite))
    mutated["file_universe_digest"] = "0" * 64
    with pytest.raises(ev.EvidenceError, match="file universe digest mismatch"):
        ev.validate_suite(repo, mutated)
    mutated = json.loads(json.dumps(suite))
    del mutated["file_universe"]
    with pytest.raises(ev.EvidenceError, match="missing fields"):
        ev.validate_suite(repo, mutated)
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(mutated, _load_schema("suite.schema.json"))


def _train_probe_task(files, task_id, query, family, gold_span):
    return {
        "task_id": task_id,
        "split": "train",
        "query": query,
        "query_sha256": ev.digest(query.encode()),
        "query_family_id": family,
        "answerable": True,
        "gold": [_v3_block(files, *gold_span, grade=2)],
    }


def test_v3_query_family_split_separation(tmp_path):
    repo, suite, _run, _sp, _rp, files = fixture_v3(tmp_path)
    mutated = json.loads(json.dumps(suite))
    mutated["tasks"].append(
        _train_probe_task(
            files, "T0", "find alpha training probe", "fam-alpha-beta", ("a.txt", 1, 1)
        )
    )
    with pytest.raises(ev.EvidenceError, match="query family spans train and eval"):
        ev.validate_suite(repo, mutated)
    mutated["tasks"][-1]["query_family_id"] = "fam-train-only"
    ev.validate_suite(repo, mutated)


def test_v3_query_near_duplicates_refused(tmp_path):
    repo, suite, _run, _sp, _rp, files = fixture_v3(tmp_path)
    mutated = json.loads(json.dumps(suite))
    mutated["tasks"].append(
        _train_probe_task(
            files, "T0", "FIND  alpha TWO and BETA one!!", "fam-train-only", ("a.txt", 1, 1)
        )
    )
    with pytest.raises(ev.EvidenceError, match="normalized match"):
        ev.validate_suite(repo, mutated)
    mutated["tasks"][-1]["query"] = "find alpha two and beta one ok"
    mutated["tasks"][-1]["query_sha256"] = ev.digest(mutated["tasks"][-1]["query"].encode())
    with pytest.raises(ev.EvidenceError, match="near-duplicate"):
        ev.validate_suite(repo, mutated)


def test_query_pool_guard_reports_all_cross_pool_conflicts():
    references = [
        ("suite/S071", "TestMappingCustomArrayUnmarshalTextForm"),
        ("suite/S073", "TestBindingFormFilesMultipart"),
    ]
    candidates = [
        ("candidate/H071", "TestMappingCustomArrayUnmarshalTextUri"),
        ("candidate/H069", "TestBindingFormFilesMultipartFail"),
        ("candidate/H090", "NovelSymbolIdentity"),
    ]
    report = pool_guard.scan(references, candidates)
    assert report["status"] == "fail"
    assert {(row["candidate_id"], row["reference_id"]) for row in report["conflicts"]} == {
        ("candidate/H071", "suite/S071"),
        ("candidate/H069", "suite/S073"),
    }
    assert (
        pool_guard.scan(references, [("candidate/F001", "NovelSymbolIdentity")])["status"] == "pass"
    )
    duplicate = pool_guard.scan(references, [("candidate/H001", "TESTBINDINGFORMFILESMULTIPART")])
    assert duplicate["conflicts"][0]["kind"] == "normalized_duplicate"


def test_query_pool_guard_cli_fails_closed_before_review(tmp_path):
    repo, suite, _run, suite_path, _rp, _files = fixture_v3(tmp_path)
    suite_path.write_text(json.dumps(suite), encoding="utf-8")

    def proposal(path, task_id, query):
        path.write_text(
            json.dumps(
                {
                    "proposal_id": task_id,
                    "query": query,
                    "stratum": "test_unique",
                    "status": "unreviewed_query_proposal",
                }
            )
            + "\n",
            encoding="utf-8",
        )

    candidate = tmp_path / "candidate.jsonl"
    report_path = tmp_path / "guard.json"
    proposal(candidate, "H001", "find alpha two and beta one ok")
    args = [
        "--repo",
        str(repo),
        "--reference-suite",
        str(suite_path),
        "--candidate-proposals",
        str(candidate),
        "--output",
        str(report_path),
    ]
    assert pool_guard.main(args) == 2
    assert json.loads(report_path.read_text())["status"] == "fail"
    proposal(candidate, "F001", "NovelSymbolIdentity")
    assert pool_guard.main(args) == 2  # Existing output is never overwritten.
    assert json.loads(report_path.read_text())["status"] == "fail"
    fresh = tmp_path / "guard-pass.json"
    assert pool_guard.main([*args[:-1], str(fresh)]) == 0
    assert json.loads(fresh.read_text())["status"] == "pass"
    second_suite = tmp_path / "second-suite.json"
    second_suite.write_text(json.dumps(suite), encoding="utf-8")
    multi_suite = tmp_path / "multi-suite.json"
    assert (
        pool_guard.main(
            [
                *args[:-1],
                str(multi_suite),
                "--reference-suite",
                str(second_suite),
            ]
        )
        == 0
    )
    assert json.loads(multi_suite.read_text())["reference_count"] == 4
    candidate.write_text('{"proposal_id":"bad"}\n', encoding="utf-8")
    assert pool_guard.main([*args[:-1], str(tmp_path / "malformed.json")]) == 2
    candidate.write_text(
        '{"proposal_id":"H1","proposal_id":"H2","query":"novel",'
        '"stratum":"test","status":"unreviewed"}\n',
        encoding="utf-8",
    )
    assert pool_guard.main([*args[:-1], str(tmp_path / "duplicate-key.json")]) == 2
    proposal(candidate, "H003", "검색")
    assert pool_guard.main([*args[:-1], str(tmp_path / "empty-normalized.json")]) == 2


def test_query_pool_guard_rejects_candidate_already_marked_reviewed_or_searched():
    for status in ("reviewed", "already_searched", "unreviewed"):
        row = {
            "proposal_id": "H001",
            "query": "Describe request body binding in Gin",
            "stratum": "semantic_intent",
            "status": status,
        }
        raw = (json.dumps(row) + "\n").encode()
        with pytest.raises(ev.EvidenceError, match="invalid or duplicate proposal"):
            pool_guard._proposals(raw, "candidate.jsonl", require_unreviewed=True)
    row["status"] = "unreviewed_query_proposal"
    assert pool_guard._proposals(
        (json.dumps(row) + "\n").encode(), "candidate.jsonl", require_unreviewed=True
    ) == [("H001", "Describe request body binding in Gin")]


def test_v3_leakage_allowlist(tmp_path):
    repo, suite, _run, _sp, _rp, files = fixture_v3(tmp_path, answerable_only=True)
    mutated = json.loads(json.dumps(suite))
    mutated["tasks"][1]["split"] = "train"
    mutated["tasks"][1]["gold"] = [_v3_block(files, "a.txt", 2, 2, grade=2)]
    with pytest.raises(ev.EvidenceError, match="leakage across train/eval split"):
        ev.validate_suite(repo, mutated)
    mutated["leakage_allowlist"] = [
        {
            "path": "a.txt",
            "start_line": 3,
            "end_line": 3,
            "rationale_digest": _fake_sha("rationale"),
        },
    ]
    with pytest.raises(ev.EvidenceError, match="leakage across train/eval split"):
        ev.validate_suite(repo, mutated)
    mutated["leakage_allowlist"] = [
        {
            "path": "a.txt",
            "start_line": 2,
            "end_line": 2,
            "rationale_digest": _fake_sha("rationale"),
        },
    ]
    ev.validate_suite(repo, mutated)
    mutated["leakage_allowlist"][0]["rationale_digest"] = "zzz"
    with pytest.raises(ev.EvidenceError, match="must be a lowercase sha256"):
        ev.validate_suite(repo, mutated)


def test_v3_timeout_requires_measured_duration(tmp_path):
    repo, suite, run, suite_path, runner_path, _files = fixture_v3(tmp_path, answerable_only=True)
    mutated = json.loads(json.dumps(run))
    row = mutated["results"][0]
    row["status"] = "timeout"
    row["candidates"] = []
    row["timings"] = {"query_latency_ms": None}
    row["error"] = {"code": "deadline", "message": "timed out"}
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    runner_path.write_text(json.dumps(mutated), encoding="utf-8")
    with pytest.raises(ev.EvidenceError, match="measured duration"):
        ev.load_evidence(repo, suite_path, runner_path)
    row["timings"] = {"query_latency_ms": 5.0}
    runner_path.write_text(json.dumps(mutated), encoding="utf-8")
    ev.load_evidence(repo, suite_path, runner_path)
    row["status"] = "error"
    row["timings"] = {"query_latency_ms": None}
    runner_path.write_text(json.dumps(mutated), encoding="utf-8")
    ev.load_evidence(repo, suite_path, runner_path)


def test_v3_duplicate_candidate_byte_span_refused(tmp_path):
    repo, suite, run, suite_path, runner_path, _files = fixture_v3(tmp_path, answerable_only=True)
    mutated = json.loads(json.dumps(run))
    first = dict(mutated["results"][0]["candidates"][0], rank=3)
    mutated["results"][0]["candidates"].append(first)
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    runner_path.write_text(json.dumps(mutated), encoding="utf-8")
    with pytest.raises(ev.EvidenceError, match="duplicate candidate byte span"):
        ev.load_evidence(repo, suite_path, runner_path)


@pytest.mark.parametrize("version", [3, 4, 5])
def test_record_candidates_above_declared_top_k_are_refused(tmp_path, version):
    repo, suite, run, suite_path, runner_path, files = fixture_v3(tmp_path, answerable_only=True)
    spans = [("a.txt", start, end) for start in range(1, 5) for end in range(start, 5)]
    spans.append(("b.txt", 1, 1))
    run["results"][0]["candidates"] = [
        _v3_block(files, path, start, end, tokens=True, rank=index)
        for index, (path, start, end) in enumerate(spans, 1)
    ]
    run["schema_version"] = version
    if version < 5:
        for capture in run["captures"].values():
            capture.pop("execution_profile")
            capture.pop("execution_profile_sha256")
    if version == 3:
        for row in run["results"]:
            row.pop("query_identity")
    elif version == 4:
        run["runner"]["query_input_policy"] = {
            "policy": "native",
            "config": {},
            "policy_config_sha256": ev.digest(qp.policy_config_canonical_v4("native").encode()),
            "planning_cost_in_latency": False,
        }
    with pytest.raises(ev.EvidenceError, match="candidates exceed declared top_k"):
        record_v3(repo, suite, run, suite_path, runner_path)


def _exact_symbol_record_fixture(tmp_path):
    repo, suite, run, suite_path, runner_path, files = fixture_v3(tmp_path, answerable_only=True)
    suite["routes"] = ["symbol"]
    for task, name in zip(suite["tasks"], ("AlphaOne", "BetaTwo"), strict=True):
        task["query"] = name
        task["query_sha256"] = ev.digest(name.encode())
    _, pack, _ = ev.validate_suite(repo, suite)
    run["query_pack_sha256"] = ev.digest(ev.canonical(pack))
    profile = qp.execution_profile("exact_symbol_name")
    run["captures"]["q0"]["execution_profile"] = profile
    run["captures"]["q0"]["execution_profile_sha256"] = ev.digest(ev.canonical(profile))
    run["route_provenance"] = {"symbol": {"capture_id": "q0"}}
    run["results"] = [row for row in run["results"] if row["route"] == "lexical"]
    run["span_accounting_version"] = 1
    queries = {task["task_id"]: task["query"] for task in pack["tasks"]}
    for row in run["results"]:
        row["route"] = "symbol"
        row["query_identity"] = qp.derive_query_identity(
            "exact_symbol_name", queries[row["task_id"]]
        )
        for candidate in row["candidates"]:
            candidate["span_accounting"] = {
                "unit_kind": "symbol",
                "unit_id": f"{row['task_id']}:{candidate['rank']}",
                "producer_identity": "source-bound-symbols-v2",
                "indexed_start_byte": candidate["start_byte"],
                "indexed_end_byte": candidate["end_byte"],
                "sdk_start_line": candidate["start_line"],
                "sdk_end_line": candidate["end_line"],
                "extra_context_bytes": 0,
            }
    return repo, suite, run, suite_path, runner_path, files


@pytest.mark.parametrize("rank_unit", [None, "symbol"])
def test_exact_symbol_result_rank_unit_preserves_legacy_and_binds_new_records(tmp_path, rank_unit):
    repo, suite, run, suite_path, runner_path, _ = _exact_symbol_record_fixture(tmp_path)
    if rank_unit is not None:
        for row in run["results"]:
            row["rank_unit"] = rank_unit
    jsonschema.validate(run, _load_schema("runner.schema.json"))
    _, _, loaded = record_v3(repo, suite, run, suite_path, runner_path)
    assert len(loaded["results"]) == 2
    assert all(row.get("rank_unit") == rank_unit for row in loaded["results"])


@pytest.mark.parametrize(
    "profile,rank_unit",
    [
        ("exact_symbol_name", "distinct_file"),
        ("native", "symbol"),
        ("native", "distinct_file"),
        ("semble", "symbol"),
    ],
)
def test_result_rank_unit_cannot_promote_incompatible_capture(tmp_path, profile, rank_unit):
    fixture = _exact_symbol_record_fixture if profile == "exact_symbol_name" else fixture_v3
    repo, suite, run, suite_path, runner_path, _ = fixture(tmp_path)
    if profile == "semble":
        run["captures"]["q0"] = _v3_capture("semble", current=True)
        for row in run["results"]:
            query = next(t["query"] for t in suite["tasks"] if t["task_id"] == row["task_id"])
            raw_sha = ev.digest(query.encode())
            row["query_identity"] = {
                "original_query_sha256": raw_sha,
                "submitted_query_sha256": raw_sha,
            }
    run["results"][0]["rank_unit"] = rank_unit
    with pytest.raises(ev.EvidenceError, match="rank_unit|rank authority"):
        record_v3(repo, suite, run, suite_path, runner_path)


@pytest.mark.parametrize("fault", ["missing_protocol", "missing_unit", "chunk_unit"])
def test_explicit_symbol_rank_requires_published_symbol_authority(tmp_path, fault):
    repo, suite, run, suite_path, runner_path, _ = _exact_symbol_record_fixture(tmp_path)
    for row in run["results"]:
        row["rank_unit"] = "symbol"
    first = run["results"][0]["candidates"][0]
    if fault == "missing_protocol":
        run.pop("span_accounting_version")
    elif fault == "missing_unit":
        first.pop("span_accounting")
    else:
        first["span_accounting"].update(unit_kind="chunk", producer_identity="whole_file")
    with pytest.raises(
        ev.EvidenceError, match="span protocol|missing published-unit|published symbol unit"
    ):
        record_v3(repo, suite, run, suite_path, runner_path)


def test_v5_same_line_distinct_symbol_units_survive_record_validation(tmp_path):
    repo, suite, run, suite_path, runner_path, _files = _exact_symbol_record_fixture(tmp_path)
    for result in run["results"]:
        result["rank_unit"] = "symbol"
    row = run["results"][0]
    first = row["candidates"][0]
    second = copy.deepcopy(first)
    second["rank"] = 2
    for candidate, name, offset in ((first, "alpha", 0), (second, "three", 6)):
        candidate["span_accounting"] = {
            "unit_kind": "symbol",
            "unit_id": name,
            "producer_identity": "source-bound-symbols-v2",
            "indexed_start_byte": candidate["start_byte"] + offset,
            "indexed_end_byte": candidate["start_byte"] + offset + len(name),
            "sdk_start_line": candidate["start_line"],
            "sdk_end_line": candidate["end_line"],
            "extra_context_bytes": candidate["end_byte"] - candidate["start_byte"] - len(name),
        }
    row["candidates"] = [first, second]
    loaded_suite, loaded_pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    assert len(loaded_run["results"][0]["candidates"]) == 2
    assert loaded_run["results"][0]["rank_unit"] == "symbol"

    legacy = copy.deepcopy(run)
    for result in legacy["results"]:
        result.pop("rank_unit")
    _, _, legacy_run = record_v3(repo, suite, legacy, suite_path, runner_path)
    assert len(legacy_run["results"][0]["candidates"]) == 2

    native = copy.deepcopy(legacy)
    profile = qp.execution_profile("native")
    native["captures"]["q0"]["execution_profile"] = profile
    native["captures"]["q0"]["execution_profile_sha256"] = ev.digest(ev.canonical(profile))
    queries = {task["task_id"]: task["query"] for task in suite["tasks"]}
    for result in native["results"]:
        result["query_identity"] = qp.derive_query_identity("native", queries[result["task_id"]])
    with pytest.raises(ev.EvidenceError, match="duplicate candidate byte span"):
        record_v3(repo, suite, native, suite_path, runner_path)

    duplicate = copy.deepcopy(run)
    duplicate_row = duplicate["results"][0]
    duplicate_row["candidates"][1]["span_accounting"].update(
        indexed_start_byte=first["span_accounting"]["indexed_start_byte"],
        indexed_end_byte=first["span_accounting"]["indexed_end_byte"],
    )
    with pytest.raises(ev.EvidenceError, match="duplicate published symbol indexed span"):
        record_v3(repo, suite, duplicate, suite_path, runner_path)


# --- Cross-language fixtures (strict §1 independent oracles) ---

FIXTURES_DIR = Path(ev.__file__).resolve().parent / "fixtures"


def _load_fixture(name: str):
    return json.loads((FIXTURES_DIR / name).read_text(encoding="utf-8"))


def test_fixture_canonical_json_vectors():
    vectors = _load_fixture("canonical-json-vectors.json")
    assert len(vectors) >= 5
    for vector in vectors:
        assert ev.canonical(vector["value"]).decode("utf-8") == vector["canonical"], vector["name"]


def test_fixture_tokenizer_vectors():
    vectors = _load_fixture("tokenizer-vectors.json")
    assert len(vectors) >= 5
    for vector in vectors:
        found = ev.TOKEN_RE.findall(vector["text"])
        assert found == vector["tokens"], vector["text"]
        assert len(found) == vector["count"], vector["text"]


def test_fixture_span_vectors(tmp_path):
    vectors = _load_fixture("span-vectors.json")
    assert len(vectors) >= 5
    files = {f"{v['name']}.txt": v["file"].encode("utf-8") for v in vectors}
    repo, commit = _write_repo(tmp_path, files)
    source = ev.SourceSnapshot(repo, commit)
    for vector in vectors:
        name = f"{vector['name']}.txt"
        raw = files[name]
        item = {
            "path": name,
            "start_byte": vector["start_byte"],
            "end_byte": vector["end_byte"],
            "start_line": vector["start_line"],
            "end_line": vector["end_line"],
            "file_sha256": ev.digest(raw),
            "block_sha256": vector["block_sha256"],
            "tokens": vector["tokens"],
            "rank": 1,
        }
        checked = ev.block(source, item, "fixture " + vector["name"], candidate=True)
        assert checked["tokens"] == vector["tokens"]
        if item["end_byte"] < len(raw):
            mutated = dict(
                item,
                end_byte=item["end_byte"] + 1,
                block_sha256=ev.digest(raw[item["start_byte"] : item["end_byte"] + 1]),
            )
            match = "byte span disagrees"
        else:
            mutated = dict(item, end_byte=item["end_byte"] + 1)
            match = "byte span runs past EOF"
        with pytest.raises(ev.EvidenceError, match=match):
            ev.block(source, mutated, "fixture " + vector["name"], candidate=True)


def test_fixture_split_leakage_mutants():
    mutants = _load_fixture("split-leakage-mutants.json")
    assert len(mutants) >= 5
    for mutant in mutants:
        labels = {
            "train": {tuple(span) for span in mutant["train"]},
            "eval": {tuple(span) for span in mutant["eval"]},
        }
        allowlist = frozenset(tuple(span) for span in mutant["allowlist"])
        if mutant["expect_ok"]:
            ev._check_split_leakage(labels, allowlist)
        else:
            with pytest.raises(ev.EvidenceError, match="leakage across train/eval split"):
                ev._check_split_leakage(labels, allowlist)


def test_conditional_vector_replay_checks_full_vector_and_batch_permutation():
    # Independent unit oracle: equal unit basis vectors have cosine one.
    inputs = ["first", "second"]
    basis = [1.0] + [0.0] * 255
    observed = {
        "schema_version": 1,
        "model_id": "model2vec:minishlab/potion-code-16M-v2",
        "model_revision": "e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b:model2vec-rs-0.3.0:fancy-regex:full-length-v2",
        "dimension": 256,
        "normalization": "l2_unit",
        "max_length": None,
        "inputs": inputs,
        "vectors": [basis[:], basis[:]],
        "reversed_vectors": [basis[:], basis[:]],
    }
    baseline = {
        "schema_version": parity_reference.SCHEMA_VERSION,
        "profile": parity_reference.REFERENCE_PROFILE,
        "library": {"model2vec": "0.9.0"},
        "model": {
            "id": "minishlab/potion-code-16M-v2",
            "revision": parity_reference.MODEL_REVISION,
            "dir_name": "pinned",
            "safetensors_sha256": parity_reference.PINNED_ASSET_SHA256["model.safetensors"],
            "tokenizer_sha256": parity_reference.PINNED_ASSET_SHA256["tokenizer.json"],
            "config_sha256": parity_reference.PINNED_ASSET_SHA256["config.json"],
        },
        "policy": {
            "max_length": None,
            "tokenizer_embedded_truncation_disabled": True,
            "normalization": "approx-unit-fp16 (rail L2-normalizes both sides)",
        },
        "inputs": inputs,
        "vectors": [basis[:], basis[:]],
        "norms": [1.0, 1.0],
        "pairwise_cosine_upper": [[1.0], []],
        "dimension": 256,
    }
    assert cp.model_rows(observed, baseline, inputs)[1] == 2
    capped_reference = json.loads(json.dumps(baseline))
    capped_reference["policy"]["tokenizer_embedded_truncation_disabled"] = False
    with pytest.raises(ValueError, match="encoder policy/model identity drift"):
        cp.model_rows(observed, capped_reference, inputs)
    for bad in (True, float("nan"), float("inf"), 10**400, 1.1):
        mutant = json.loads(json.dumps(baseline))
        mutant["pairwise_cosine_upper"][0][0] = bad
        with pytest.raises(ValueError):
            cp.model_rows(observed, mutant, inputs)
    for field in ("vectors", "norms"):
        mutant = json.loads(json.dumps(baseline))
        if field == "vectors":
            mutant[field][0][0] = 10**400
        else:
            mutant[field][0] = 10**400
        with pytest.raises(ValueError):
            cp.model_rows(observed, mutant, inputs)
    mutant = json.loads(json.dumps(observed))
    mutant["vectors"][0][0] = 10**400
    with pytest.raises(ValueError):
        cp.model_rows(mutant, baseline, inputs)
    for bad in (256.0, True):
        for source in ("observed", "reference"):
            actual, expected = json.loads(json.dumps(observed)), json.loads(json.dumps(baseline))
            (actual if source == "observed" else expected)["dimension"] = bad
            with pytest.raises(ValueError):
                cp.model_rows(actual, expected, inputs)

    bundle = _conditional_vector_context_unit_bundle(observed, baseline)
    assert cp.validate_results(bundle, "model_vectors") == bundle
    for mutate in (
        lambda records: records.append(
            {
                "route_provenance": {"semantic": {"capture_id": "wrong"}},
                "captures": {
                    "wrong": {"system": "quanta", "model": "wrong-model", "model_revision": "wrong"}
                },
            }
        ),
        lambda records: records[0]["route_provenance"].update(semantic={"capture_id": "missing"}),
        lambda records: records[0].pop("route_provenance"),
        lambda records: records[0]["route_provenance"].update(semantic={"capture_id": True}),
        lambda records: records[0]["route_provenance"].clear(),
        lambda records: records[0]["route_provenance"].update(
            semantic={"capture_id": "quanta", "extra": True}
        ),
    ):
        mutant = json.loads(json.dumps(bundle))
        context = mutant["execution_context"]
        records = cp.load(cp.decode(context["records"]))
        mutate(records)
        context["records"] = cp.artifact(cp.canonical(records))
        identities = sorted(
            set(
                (capture["system"], capture["model"], capture["model_revision"])
                for record in records
                for capture in record["captures"].values()
            )
        )
        mutant["identity"]["model_sha256"] = cp.sha(cp.canonical(identities))
        mutant["execution_receipt"].update(
            model_sha256=mutant["identity"]["model_sha256"],
            context_sha256=cp.sha(cp.canonical(context)),
        )
        with pytest.raises(ValueError):
            cp.validate_results(mutant, "model_vectors")
    mutant = json.loads(json.dumps(bundle))
    mutant["raw_proof"]["rows"][0]["observed_vector"][0] = True
    with pytest.raises(ValueError, match="summary"):
        cp.validate_results(mutant, "model_vectors")
    for mutate in (
        lambda context: context["run"]["argv"].__setitem__(3, "potion-code"),
        lambda context: context["reference_run"].update(argv=[]),
        lambda context: context["reference_run"]["argv"].__setitem__(3, "/wrong/model"),
        lambda context: context["reference_run"]["argv"].__setitem__(7, "/wrong/inputs.json"),
        lambda context: context["reference_run"].update(script_sha256="d" * 64),
        lambda context: context["reference_run"].update(output_sha256="d" * 64),
        lambda context: context["reference_run"].update(environment={}),
    ):
        mutant = json.loads(json.dumps(bundle))
        mutate(mutant["execution_context"])
        mutant["execution_receipt"]["context_sha256"] = cp.sha(
            cp.canonical(mutant["execution_context"])
        )
        with pytest.raises(ValueError):
            cp.validate_results(mutant, "model_vectors")
    mutant = json.loads(json.dumps(observed))
    mutant["vectors"][0][255] = 0.01
    assert cp.model_rows(mutant, baseline, inputs)[1] < 2
    mutant = json.loads(json.dumps(observed))
    mutant["reversed_vectors"][0][255] = 0.0001
    assert cp.model_rows(mutant, baseline, inputs)[1] == 1
    for mutate in (
        lambda value: value["vectors"][0].pop(),
        lambda value: value.update(inputs=["other", "second"]),
        lambda value: value.update(max_length=512),
        lambda value: value.update(model_revision="wrong"),
    ):
        mutant = json.loads(json.dumps(observed))
        mutate(mutant)
        with pytest.raises(ValueError):
            cp.model_rows(mutant, baseline, inputs)
    encoded = cp.artifact(cp.canonical(observed))
    assert cp.load(cp.decode(encoded)) == observed
    encoded["sha256"] = "0" * 64
    with pytest.raises(ValueError, match="digest"):
        cp.decode(encoded)
    with pytest.raises(ValueError, match="duplicate"):
        cp.load(b'{"status":"pass","status":"pass"}')


def _conditional_incremental_unit_oracle():
    target = {key: "fixed" for key in cp.SEMANTIC_COLUMNS}
    target.update(
        embedding_id="target",
        record_id="record-target",
        owner_id="target-owner",
        owner_kind="Chunk",
        corpus_kind="RawCodeFallback",
        vector=[1.0, 0.0],
        generated=False,
        source_role="RawFallbackText",
        capability_status="Full",
        language="rust",
        start_byte=0,
        end_byte=14,
        view_kind="raw_chunk",
        card_schema_version=0,
        start_line=1,
        end_line=1,
        parent_owner_id=None,
        package=None,
        symbol_kind=None,
        visibility=None,
        snippet="golden payload",
    )
    sentinel = {
        **target,
        "embedding_id": "sentinel",
        "record_id": "record-sentinel",
        "owner_id": "unaffected-owner",
        "owner_kind": "Module",
        "corpus_kind": "ModuleCard",
    }

    def scope(records, members=None):
        return {
            "scope": {"doc_surface": "Chunk", "repo_relative_path": "fixed"},
            "scope_digest": "fixed",
            "embeddings": records,
            "cluster_memberships": []
            if members is None
            else [
                {
                    "cluster_record_id": "record-target",
                    "authority_digest": "fixed",
                    "members": members,
                }
            ],
        }

    def batch(generation, scopes, mode="ReplaceGeneration"):
        return {
            "generation": generation,
            "batch_digest": str(generation),
            "manifest_digest": str(generation),
            "repo_id": "repo",
            "revision_id": "rev",
            "model_contract": {
                "model_id": "fixed",
                "model_version": "1",
                "dimension": 2,
                "normalization": "L2Unit",
                "distance_metric": "Cosine",
                "policy_digest": "fixed-policy",
                "view_policy_digest": None,
            },
            "seal": True,
            "mode": mode,
            "base_generation": 1 if mode == "Delta" else None,
            "required_corpora": [],
            "corpus_policy_digest": None,
            "replace_scopes": scopes,
            "tombstone_scopes": [],
            "clear_surfaces": [],
        }

    def golden_state(item):
        rows = [
            {key: row[key] for key in cp.SEMANTIC_COLUMNS}
            for scope in item["replace_scopes"]
            for row in scope["embeddings"]
        ]
        members = []
        for current in item["replace_scopes"]:
            for replacement in current["cluster_memberships"]:
                member = replacement["members"][0]
                members.append(
                    {
                        "cluster_record_id": "record-target",
                        "authority_digest": "fixed",
                        "owner_kind": "Module",
                        "owner_id": "target-owner",
                        "member_symbol_id": member,
                        "ordinal": 0,
                        "member_count": 1,
                        "membership_content_digest": DIGESTS[member],
                    }
                )
        return {
            "semantic": {"count": len(rows), "rows": sorted(rows, key=cp.canonical)},
            "membership": {"count": len(members), "rows": members},
        }

    DIGESTS = {
        "old-member": "sha256:aae3fb26226d55e06db18f86f80f816819147f0d38931bb199e529dc80fc9c49",
        "new-member": "sha256:fb585be42421ca046d3ddaddab8a902818db2469bb9cfc9d1352a140b1990383",
    }
    cases, outputs = [], []
    for kind in sorted(cp.REQUIRED_INCREMENTAL_CASES):
        current = dict(target)
        if kind == "membership_replace":
            current.update(owner_kind="Module", corpus_kind="ClusterCard")
        before = batch(
            1,
            [scope([current, sentinel], ["old-member"] if kind == "membership_replace" else None)],
        )
        delta = batch(2, [], "Delta")
        if kind == "append":
            new = {
                **current,
                "embedding_id": "new",
                "record_id": "record-new",
                "owner_id": "new-owner",
            }
            delta["replace_scopes"] = [scope([new])]
            fresh = batch(3, [scope([current, new, sentinel])])
        elif kind == "replace":
            changed = {**current, "snippet": "replaced payload", "vector": [0.0, 1.0]}
            delta["replace_scopes"] = [scope([changed])]
            fresh = batch(3, [scope([changed, sentinel])])
        elif kind == "membership_replace":
            delta["replace_scopes"] = [scope([current], ["new-member"])]
            fresh = batch(3, [scope([current, sentinel], ["new-member"])])
        else:
            fresh = batch(3, [scope([sentinel])])
            if kind == "clear_surface":
                delta["clear_surfaces"] = ["Chunk"]
            else:
                delta["tombstone_scopes"] = [
                    {
                        "semantic_scope": {
                            "corpus_kind": current["corpus_kind"],
                            "owner_kind": current["owner_kind"],
                            "owner_id": current["owner_id"],
                        }
                    }
                ]
        case = {"case_id": kind, "before": before, "fresh": fresh, "delta": delta}
        cases.append(case)
        receipts = {}
        for key in ("fresh", "before", "delta"):
            item = case[key]
            receipts[key] = {
                field: item[field] for field in ("generation", "batch_digest", "manifest_digest")
            }
            count = len(item["replace_scopes"])
            receipts[key].update(
                windows=count,
                replace_scopes=count,
                rows=sum(len(scope["embeddings"]) for scope in item["replace_scopes"]),
                stages={
                    "owner_scopes": count,
                    "windows": count,
                    **dict.fromkeys(
                        [
                            "semantic_delete_calls",
                            "semantic_delete_commits",
                            "membership_delete_calls",
                            "membership_delete_commits",
                            "semantic_append_calls",
                            "membership_append_calls",
                        ],
                        0,
                    ),
                    "durations": {
                        **dict.fromkeys(
                            [
                                "total",
                                "prepare",
                                "promotion",
                                "clear_surfaces",
                                "stream",
                                "semantic_delete",
                                "membership_delete",
                                "semantic_append",
                                "membership_append",
                                "tombstones",
                                "seal",
                            ],
                            0,
                        ),
                        "embedding": None,
                    },
                },
            )
            receipts[key]["stages"]["semantic_append_calls"] = sum(
                bool(scope["embeddings"]) for scope in item["replace_scopes"]
            )
            receipts[key]["stages"]["membership_append_calls"] = sum(
                any(membership["members"] for membership in scope["cluster_memberships"])
                for scope in item["replace_scopes"]
            )
            # Hand-counted default resident recipe: every nonempty fixture
            # fits one 1024-owner/32-MiB window, including 2/3-owner scopes.
            # Delta clear/tombstone cases instead issue one native mutation.
            # Logical calls remain one fragment or mutation, not owner count.
            stages = receipts[key]["stages"]
            stages["semantic_delete_calls"] = stages["membership_delete_calls"] = 1
            stages["semantic_delete_commits"] = 1
            stages["membership_delete_commits"] = int(
                kind == "membership_replace" or kind == "clear_surface" and key == "delta"
            )
        outputs.append(
            {
                "case_id": kind,
                "fresh": golden_state(fresh),
                "before": golden_state(before),
                "incremental": golden_state(fresh),
                "receipts": receipts,
            }
        )
    return {"schema_version": 1, "cases": cases}, {"schema_version": 1, "cases": outputs}


def _conditional_context_unit_bundle(plan, observed):
    from tools.ci.tests.test_conditional_tool_execution import add_custody_unit_fixture

    # This fixture checks replay and substitution refusal only. It is not
    # execution evidence and intentionally fails current-source verification.
    revision, repository = "a" * 40, "b" * 40
    files = [{"path": path, "sha256": cp.sha(path.encode())} for path in ("Cargo.lock", "uv.lock")]
    closure = {
        "schema_version": source_closure.SCHEMA_VERSION,
        "profile": "retrieval",
        "revision": revision,
        "roots": ["crates"],
        "files": files,
    }
    closure["digest"] = source_closure._digest(closure)
    records = [{"captures": {"raw": {"system": "quanta", "model": "fixed", "model_revision": "1"}}}]
    identity = {
        "source_revision": revision,
        "repository_commit": repository,
        "model_sha256": cp.sha(cp.canonical([("quanta", "fixed", "1")])),
        "dependency_sha256": cp.sha(b"lock"),
    }
    custody = {
        "cwd": str(cp.ROOT),
        "inherited_environment": {},
        "environment": {"CARGO_NET_OFFLINE": "true"},
        "environment_sha256": portable_proof._environment_digest({"CARGO_NET_OFFLINE": "true"}),
    }
    binary = str(cp.ROOT / "target/proof-binary")
    events = [
        {
            "reason": "compiler-artifact",
            "target": {"name": "quanta-index-incremental-proof"},
            "executable": binary,
        },
        {"reason": "build-finished", "success": True},
    ]
    context = {
        "source_closure": closure,
        "cargo_lock_sha256": files[0]["sha256"],
        "uv_lock_sha256": files[1]["sha256"],
        "suite": cp.artifact(cp.canonical({"repository_commit": repository})),
        "corpus": cp.artifact(cp.canonical({"repository_commit": repository})),
        "records": cp.artifact(cp.canonical(records)),
        "semble_lockfile": cp.artifact(b"lock"),
        "inputs": cp.artifact(cp.canonical(plan)),
        "reference": None,
        "observed": cp.artifact(cp.canonical(observed)),
        "build": {
            **custody,
            "argv": [
                str(cp.ROOT / "scripts/cargow"),
                "--lane",
                "test-daemon-lane",
                "build",
                "-p",
                "quanta-index-semantic",
                "--bin",
                "quanta-index-incremental-proof",
                "--features",
                "proof",
                "--locked",
                "--message-format=json",
            ],
            "exit_code": 0,
            "stdout": cp.artifact(b"\n".join(cp.canonical(event) for event in events)),
            "stderr": cp.artifact(b""),
        },
        "run": {
            **custody,
            "argv": [binary, "/tmp/proof/inputs.json", "/tmp/proof/fresh-state"],
            "exit_code": 0,
            "stdout": cp.artifact(cp.canonical(observed)),
            "stderr": cp.artifact(b""),
            "executable_sha256": "c" * 64,
        },
        "reference_run": None,
        "binary_sha256": "c" * 64,
        "environment": {"python_version": "3.13.9", "relevant": {}},
    }
    raw, passed = cp.rederive("incremental_rows", context)
    command = "retrieval-conditional-proof-v2:incremental_rows"
    bundle = {
        "schema_version": 2,
        "command": command,
        "status": "pass",
        "selected": 5,
        "executed": 5,
        "passed": passed,
        "failed": 5 - passed,
        "identity": identity,
        "raw_proof": raw,
        "execution_context": context,
        "execution_receipt": {
            "schema_version": 2,
            "command": command,
            "exit_code": 0,
            **identity,
            "runner_binary_sha256": "c" * 64,
            "raw_sha256": cp.sha(cp.canonical(raw)),
            "context_sha256": cp.sha(cp.canonical(context)),
        },
    }
    with tempfile.TemporaryDirectory(prefix="qi-conditional-unit-context-") as directory:
        return add_custody_unit_fixture(bundle, Path(directory))


def _conditional_vector_context_unit_bundle(observed, baseline):
    from tools.ci.tests.test_conditional_tool_execution import add_custody_unit_fixture

    plan, incremental = _conditional_incremental_unit_oracle()
    bundle = _conditional_context_unit_bundle(plan, incremental)
    context = bundle["execution_context"]
    texts = parity_reference.INPUTS + ["frozen query"]
    actual = json.loads(json.dumps(observed))
    reference = json.loads(json.dumps(baseline))
    for value in (actual, reference):
        value["inputs"] = texts
        value["vectors"] = [value["vectors"][0]] * len(texts)
    actual["reversed_vectors"] = actual["vectors"]
    reference["norms"] = [1.0] * len(texts)
    reference["pairwise_cosine_upper"] = [[1.0] * (len(texts) - i - 1) for i in range(len(texts))]
    context["suite"] = cp.artifact(
        cp.canonical(
            {
                "repository_commit": bundle["identity"]["repository_commit"],
                "tasks": [{"query": "frozen query"}],
            }
        )
    )
    models = [
        ("quanta", actual["model_id"], actual["model_revision"]),
        ("semble", parity_reference.MODEL_ID, parity_reference.MODEL_REVISION),
    ]
    records = [
        {
            "route_provenance": {"semantic": {"capture_id": system}},
            "captures": {system: {"system": system, "model": model, "model_revision": revision}},
        }
        for system, model, revision in models
    ]
    context["records"] = cp.artifact(cp.canonical(records))
    bundle["identity"]["model_sha256"] = cp.sha(cp.canonical(models))
    context["inputs"], context["observed"], context["reference"] = [
        cp.artifact(cp.canonical(value)) for value in (texts, actual, reference)
    ]
    context["build"]["argv"] = [
        str(cp.ROOT / "scripts/cargow"),
        "--lane",
        "test-daemon-lane",
        "build",
        "-p",
        "quanta-index-embed",
        "--bin",
        "quanta-index-vector-proof",
        "--locked",
        "--message-format=json",
    ]
    binary = context["run"]["argv"][0]
    context["build"]["stdout"] = cp.artifact(
        b"\n".join(
            cp.canonical(event)
            for event in [
                {
                    "reason": "compiler-artifact",
                    "target": {"name": "quanta-index-vector-proof"},
                    "executable": binary,
                },
                {"reason": "build-finished", "success": True},
            ]
        )
    )
    context["run"].update(
        argv=[binary, "/tmp/model", "/tmp/proof/inputs.json", "potion-code-full-v2"],
        stdout=context["observed"],
    )
    script = "tools/benchmark/retrieval/parity_reference.py"
    closure = context["source_closure"]
    closure["files"].append({"path": script, "sha256": cp.sha(script.encode())})
    closure["files"].sort(key=lambda row: row["path"])
    closure["digest"] = source_closure._digest(
        {key: value for key, value in closure.items() if key != "digest"}
    )
    context["reference_run"] = {key: context["run"][key] for key in cp.COMMAND_IDENTITY}
    context["reference_run"].update(
        argv=[
            "/tmp/python",
            str(cp.ROOT / script),
            "--model-dir",
            "/tmp/model",
            "--out",
            "/tmp/proof/reference.json",
            "--inputs-json",
            "/tmp/proof/inputs.json",
            "--model-id",
            parity_reference.MODEL_ID,
        ],
        exit_code=0,
        stdout=cp.artifact(b""),
        stderr=cp.artifact(b""),
        output_sha256=context["reference"]["sha256"],
        interpreter_sha256="c" * 64,
        script_sha256=cp.sha(script.encode()),
    )
    raw, passed = cp.rederive("model_vectors", context)
    command = "retrieval-conditional-proof-v2:model_vectors"
    bundle.update(
        command=command,
        selected=len(texts),
        executed=len(texts),
        passed=passed,
        failed=len(texts) - passed,
        raw_proof=raw,
    )
    bundle["execution_receipt"] = {
        "schema_version": 2,
        "command": command,
        "exit_code": 0,
        **bundle["identity"],
        "runner_binary_sha256": context["binary_sha256"],
        "raw_sha256": cp.sha(cp.canonical(raw)),
        "context_sha256": cp.sha(cp.canonical(context)),
    }
    with tempfile.TemporaryDirectory(prefix="qi-conditional-unit-vector-") as directory:
        return add_custody_unit_fixture(bundle, Path(directory))


def test_conditional_incremental_replay_compares_payload_membership_and_empty_state(
    monkeypatch, tmp_path, proof_actor_environment
):
    plan, output = _conditional_incremental_unit_oracle()
    assert [case["before"]["semantic"]["count"] for case in output["cases"]] == [2, 2, 2, 2, 2]
    assert [case["fresh"]["semantic"]["count"] for case in output["cases"]] == [3, 1, 2, 2, 1]
    # Fifteen small receipts: 13 replacement windows plus one clear and
    # one tombstone. Only the cluster case's three windows and the clear
    # mutation issue membership-table native deletes.
    stages = [
        case["receipts"][batch]["stages"]
        for case in output["cases"]
        for batch in ("before", "fresh", "delta")
    ]
    assert sum(stage["windows"] for stage in stages) == 13
    assert sum(stage["semantic_delete_commits"] for stage in stages) == 15
    assert sum(stage["membership_delete_commits"] for stage in stages) == 4
    assert all(
        stage["semantic_delete_calls"] == stage["membership_delete_calls"] == 1 for stage in stages
    )
    assert cp.incremental_rows(output, plan)[1] == 5
    # Synthetic input/raw substitutions, not native execution evidence:
    # agreeing rows and operation counts cannot override stream admission.
    for mutation in (
        "duplicate-clear",
        "unsorted-clear",
        "duplicate-tombstone",
        "duplicate-record_id",
    ):
        bad_plan, bad_output = json.loads(json.dumps(plan)), json.loads(json.dumps(output))
        if mutation != "duplicate-record_id":
            kind = "tombstone" if mutation == "duplicate-tombstone" else "clear_surface"
            index = next(
                index for index, case in enumerate(bad_plan["cases"]) if case["case_id"] == kind
            )
            delta = bad_plan["cases"][index]["delta"]
            stages = bad_output["cases"][index]["receipts"]["delta"]["stages"]
            if mutation == "duplicate-tombstone":
                delta["tombstone_scopes"] *= 2
            else:
                delta["clear_surfaces"] = (
                    ["Chunk", "Chunk"] if mutation == "duplicate-clear" else ["Chunk", "File"]
                )
                stages["semantic_delete_commits"] = stages["membership_delete_commits"] = 2
            stages["semantic_delete_calls"] = stages["membership_delete_calls"] = 2
        else:
            for case in bad_plan["cases"]:
                for batch in ("before", "fresh", "delta"):
                    for scope in case[batch]["replace_scopes"]:
                        for row in scope["embeddings"]:
                            if row["owner_id"] == "unaffected-owner":
                                row["record_id"] = "record-target"
            for case in bad_output["cases"]:
                for batch in ("before", "fresh", "incremental"):
                    for row in case[batch]["semantic"]["rows"]:
                        if row["owner_id"] == "unaffected-owner":
                            row["record_id"] = "record-target"
                    case[batch]["semantic"]["rows"].sort(key=cp.canonical)
        with pytest.raises(ValueError, match="clear surfaces|scope authority"):
            cp.incremental_rows(bad_output, bad_plan)
    # Owner groups may contain multiple distinct records within one scope;
    # the same owner cannot be replaced across different scopes.
    admitted = json.loads(json.dumps(plan["cases"][0]["before"]))
    record = {
        **admitted["replace_scopes"][0]["embeddings"][0],
        "record_id": "record-extra",
        "embedding_id": "embedding-extra",
    }
    admitted["replace_scopes"][0]["embeddings"].append(record)
    assert cp.input_state(admitted)["semantic"]["count"] == 3
    repeated = json.loads(json.dumps(plan["cases"][0]["before"]))
    repeated["replace_scopes"].append({**repeated["replace_scopes"][0], "embeddings": [record]})
    with pytest.raises(ValueError, match="repeated replace owners"):
        cp.input_state(repeated)
    clear_batch = next(
        case["delta"] for case in plan["cases"] if case["case_id"] == "clear_surface"
    )
    for conflict in ("scope", "owner", "tombstone"):
        malformed = json.loads(json.dumps(clear_batch))
        source = json.loads(json.dumps(plan["cases"][0]["before"]["replace_scopes"][0]))
        if conflict == "scope":
            source["embeddings"] = []
            malformed["replace_scopes"] = [source]
        elif conflict == "owner":
            source["scope"]["doc_surface"] = "File"
            malformed["replace_scopes"] = [source]
        else:
            malformed["tombstone_scopes"] = [
                {
                    "semantic_scope": {
                        key: source["embeddings"][0][key]
                        for key in ("corpus_kind", "owner_kind", "owner_id")
                    }
                }
            ]
        with pytest.raises(ValueError, match="scope authority"):
            cp.input_state(malformed)
    malformed = json.loads(json.dumps(plan["cases"][0]["before"]))
    malformed["tombstone_scopes"] = [
        {
            "semantic_scope": {
                key: malformed["replace_scopes"][0]["embeddings"][0][key]
                for key in ("corpus_kind", "owner_kind", "owner_id")
            }
        }
    ]
    with pytest.raises(ValueError, match="tombstone/replace conflicts"):
        cp.input_state(malformed)
    # Native validated RepoId/RevisionId are not arbitrary text cells. Raw
    # rows and counters can agree while the input remains inadmissible.
    for field in ("repo_id", "revision_id"):
        for value in ("", "x" * 513, "\x00", "\x85", "e\u0301", "é" * 257):
            malformed = json.loads(json.dumps(plan))
            for case in malformed["cases"]:
                for batch in ("before", "fresh", "delta"):
                    case[batch][field] = value
            with pytest.raises(ValueError, match="canonical repository identity"):
                cp.incremental_rows(output, malformed)
        for value in ("x" * 512, "é" * 256):
            cp.native_repository_identity(value, field)
    # Native LanguageCode syntax and nullable closed SymbolKindCode must also
    # survive input/raw agreement. These used to earn 5/5 despite being
    # impossible to deserialize through the real EmbeddingRecord boundary.
    for field, values in (
        ("language", ("", "Rust", "rust.js", "러스트", "1rust", "rust\x00", None, True, [])),
        ("symbol_kind", ("", "Function", "bogus", "type-alias", True, [])),
    ):
        for value in values:
            bad_plan, bad_output = copy.deepcopy(plan), copy.deepcopy(output)
            for case in bad_plan["cases"]:
                for batch in ("before", "fresh", "delta"):
                    for scope in case[batch]["replace_scopes"]:
                        for row in scope["embeddings"]:
                            row[field] = value
            for case in bad_output["cases"]:
                for batch in ("before", "fresh", "incremental"):
                    for row in case[batch]["semantic"]["rows"]:
                        row[field] = value
                    case[batch]["semantic"]["rows"].sort(key=cp.canonical)
            with pytest.raises(ValueError, match="language code|symbol_kind"):
                cp.incremental_rows(bad_output, bad_plan)
    for language in ("rust", "c++", "objective-c", "qglang2", "m+v_1", "r" * 4096):
        cp.native_language_code(language)
    for symbol_kind in (
        None,
        "function",
        "method",
        "class",
        "struct",
        "enum",
        "trait",
        "interface",
        "variable",
        "constant",
        "module",
        "macro",
        "type_alias",
    ):
        admitted = copy.deepcopy(plan["cases"][0]["before"])
        for row in admitted["replace_scopes"][0]["embeddings"]:
            row["symbol_kind"] = symbol_kind
        assert cp.input_state(admitted)["semantic"]["count"] == 2
    # Input-only DTO fields must be Rust String values even when the logical
    # projection drops them. Escaped lone surrogates are legal Python JSON.
    for field in ("view_kind", "scope_digest", "corpus_policy_digest"):
        for value in ("\ud800", "\udfff", "x\ud800y"):
            malformed = copy.deepcopy(plan)
            for case in malformed["cases"]:
                for batch in ("before", "fresh", "delta"):
                    native_batch = case[batch]
                    if field == "corpus_policy_digest":
                        native_batch[field] = value
                    for scope in native_batch["replace_scopes"]:
                        if field == "scope_digest":
                            scope[field] = value
                        elif field == "view_kind":
                            for row in scope["embeddings"]:
                                row[field] = value
            malformed = json.loads(json.dumps(malformed))
            with pytest.raises(ValueError, match="valid UTF-8"):
                cp.incremental_rows(output, malformed)
    # Valid input must not mask inadmissible strings in any raw state table.
    # Check ordinary and nullable text cells before row-set comparison, so a
    # mismatch elsewhere cannot supply the rejection for this regression.
    for state in ("before", "fresh", "incremental"):
        for field in ("snippet", "package", "visibility"):
            for value in ("\ud800", "\udfff"):
                malformed_output = copy.deepcopy(output)
                rows = malformed_output["cases"][0][state]["semantic"]["rows"]
                assert rows
                rows[0][field] = value
                with pytest.raises(ValueError, match="valid UTF-8"):
                    cp.incremental_rows(malformed_output, plan)
    for value in ("", "\x00", "é", "e\u0301", json.loads('"\\ud83d\\ude00"')):
        admitted = copy.deepcopy(plan["cases"][0]["before"])
        admitted["corpus_policy_digest"] = value
        for scope in admitted["replace_scopes"]:
            scope["scope_digest"] = value
            for row in scope["embeddings"]:
                row["view_kind"] = value
        assert cp.input_state(admitted)["semantic"]["count"] == 2
    deeply_malformed = "text"
    for _ in range(2000):
        deeply_malformed = [deeply_malformed]
    malformed = copy.deepcopy(plan["cases"][0]["before"])
    malformed["replace_scopes"][0]["embeddings"][0]["view_kind"] = deeply_malformed
    with pytest.raises(ValueError, match="bytes/view"):
        cp.input_state(malformed)
    for field in ("required_corpora", "corpus_policy_digest"):
        malformed = json.loads(json.dumps(plan["cases"][0]["before"]))
        del malformed[field]
        with pytest.raises(ValueError, match="native incremental batch"):
            cp.input_state(malformed)
    # Native SemanticTombstoneScopeVisitor rejects an empty owner identity.
    tombstone_batch = next(
        case["delta"] for case in plan["cases"] if case["case_id"] == "tombstone"
    )
    for owner_id in (None, "", True, []):
        malformed = json.loads(json.dumps(tombstone_batch))
        malformed["tombstone_scopes"][0]["semantic_scope"]["owner_id"] = owner_id
        with pytest.raises(ValueError, match="nonempty string"):
            cp.input_state(malformed)
    for field in ("scope", "scope_digest"):
        malformed = json.loads(json.dumps(plan["cases"][0]["before"]))
        del malformed["replace_scopes"][0][field]
        with pytest.raises(ValueError, match="incremental replace scope"):
            cp.input_state(malformed)
    for value in (None, [], {"doc_surface": "UnknownSurface", "repo_relative_path": "fixed"}):
        malformed = json.loads(json.dumps(plan["cases"][0]["before"]))
        malformed["replace_scopes"][0]["scope"] = value
        with pytest.raises(ValueError, match="scope"):
            cp.input_state(malformed)
    # Consistent raw/input substitution cannot admit a value that native
    # OwnerDocKind or SemanticCorpusKindV1 deserialization would refuse.
    for field, value in (
        ("owner_kind", "UnknownOwner"),
        ("corpus_kind", "UnknownCorpus"),
        ("source_role", "UnknownRole"),
        ("capability_status", "UnknownCapability"),
    ):
        bad_plan, bad_output = json.loads(json.dumps(plan)), json.loads(json.dumps(output))
        for case in bad_plan["cases"]:
            for batch in ("before", "fresh", "delta"):
                for scope in case[batch]["replace_scopes"]:
                    for row in scope["embeddings"]:
                        if row["owner_id"] == "unaffected-owner":
                            row[field] = value
        for case in bad_output["cases"]:
            for batch in ("before", "fresh", "incremental"):
                for row in case[batch]["semantic"]["rows"]:
                    if row["owner_id"] == "unaffected-owner":
                        row[field] = value
        with pytest.raises(ValueError, match="native enum"):
            cp.incremental_rows(bad_output, bad_plan)
    # Rust ClusterMembershipReplaceV1 admits only a bounded, nonempty,
    # sorted unique array of nonempty SymbolIds, attached to a ClusterCard.
    membership_batch = plan["cases"][2]["before"]
    for members in (
        None,
        [],
        "ab",
        [7],
        [True],
        [{}],
        [""],
        ["a", "a"],
        ["z", "a"],
        [f"member-{index:05}" for index in range(4097)],
    ):
        malformed = json.loads(json.dumps(membership_batch))
        malformed["replace_scopes"][0]["cluster_memberships"][0]["members"] = members
        with pytest.raises(ValueError, match="membership"):
            cp.input_state(malformed)
    for memberships in (
        None,
        {},
        [],
        membership_batch["replace_scopes"][0]["cluster_memberships"] * 2,
    ):
        malformed = json.loads(json.dumps(membership_batch))
        malformed["replace_scopes"][0]["cluster_memberships"] = memberships
        with pytest.raises(ValueError, match="membership"):
            cp.input_state(malformed)
    wrong_corpus = json.loads(json.dumps(membership_batch))
    wrong_corpus["replace_scopes"][0]["embeddings"][0]["corpus_kind"] = "ModuleCard"
    with pytest.raises(ValueError, match="authoritative"):
        cp.input_state(wrong_corpus)
    canonical_members = json.loads(json.dumps(membership_batch))
    canonical_members["replace_scopes"][0]["cluster_memberships"][0]["members"] = [
        f"member-{index:05}" for index in range(4096)
    ]
    assert cp.input_state(canonical_members)["membership"]["count"] == 4096
    # Empty memberships used to earn 5/5 when full before rows and append
    # counts were forged consistently; native admission forbids that input.
    empty_plan, empty_output = json.loads(json.dumps(plan)), json.loads(json.dumps(output))
    empty_plan["cases"][2]["before"]["replace_scopes"][0]["cluster_memberships"][0]["members"] = []
    empty_output["cases"][2]["before"]["membership"] = {"count": 0, "rows": []}
    empty_output["cases"][2]["receipts"]["before"]["stages"]["membership_append_calls"] = 0
    with pytest.raises(ValueError, match="membership"):
        cp.incremental_rows(empty_output, empty_plan)
    # Malformed captures are refused before source closure, Rust execution,
    # or creation of a proof output directory, using the consumer's contract.
    from argparse import Namespace

    suite_path, corpus_path, record_path = [
        tmp_path / name for name in ("suite.json", "corpus.json", "record.json")
    ]
    suite_path.write_text("{}")
    corpus_path.write_text("{}")
    malformed_records = (
        None,
        [],
        {},
        {"captures": []},
        {"captures": {}},
        {"captures": {"raw": None}},
        {"captures": {"raw": {"system": 7}}},
    )
    args = Namespace(
        kind="incremental_rows",
        suite=suite_path,
        corpus=corpus_path,
        records=[record_path],
        out=tmp_path / "proof",
    )
    for record in malformed_records:
        record_path.write_bytes(cp.canonical(record))
        with pytest.raises(ValueError, match="typed capture/model"):
            cp.produce(args)
        assert not args.out.exists()
    with_empty_scopes = json.loads(json.dumps(plan))
    for case in with_empty_scopes["cases"]:
        for batch in ("before", "fresh", "delta"):
            case[batch]["replace_scopes"].append(
                {
                    "scope": {"doc_surface": "File", "repo_relative_path": "fixed"},
                    "scope_digest": "fixed",
                    "embeddings": [],
                    "cluster_memberships": [],
                }
            )
    # ResidentScopeSource issues owner groups only; an empty input scope is
    # absent from the execution tally and leaves every full logical row intact.
    assert cp.incremental_rows(output, with_empty_scopes)[1] == 5
    for field, value in (
        ("dimension", 99),
        ("dimension", True),
        ("dimension", 2.0),
        ("dimension", 0),
        ("normalization", "unknown"),
        ("distance_metric", "Dot"),
        ("policy_digest", ""),
        ("model_version", 1),
    ):
        bad_plan = json.loads(json.dumps(plan))
        for case in bad_plan["cases"]:
            for batch in ("before", "fresh", "delta"):
                case[batch]["model_contract"][field] = value
        with pytest.raises(ValueError, match="model|dimension"):
            cp.incremental_rows(output, bad_plan)
    for vector in ([True, 0.0], [1e39, 0.0], [10**400, 0.0], [0.0, 0.0], [2.0, 0.0], [1.0]):
        bad_plan = json.loads(json.dumps(plan))
        bad_plan["cases"][0]["before"]["replace_scopes"][0]["embeddings"][0]["vector"] = vector
        with pytest.raises(ValueError, match="vector"):
            cp.incremental_rows(output, bad_plan)
    unconstrained = json.loads(json.dumps(plan["cases"][0]["before"]))
    unconstrained["model_contract"]["normalization"] = "None"
    unconstrained["replace_scopes"][0]["embeddings"][0]["vector"] = [2.0, 0.0]
    assert cp.input_state(unconstrained)["semantic"]["count"] == 2
    wrong_dimension = json.loads(json.dumps(output))
    wrong_dimension["cases"][0]["incremental"]["semantic"]["rows"][0]["vector"] = [1.0]
    with pytest.raises(ValueError, match="dimension"):
        cp.incremental_rows(wrong_dimension, plan)
    huge_raw = json.loads(json.dumps(output))
    huge_raw["cases"][0]["incremental"]["semantic"]["rows"][0]["vector"] = [10**400, 0.0]
    with pytest.raises(ValueError, match="finite full vector"):
        cp.incremental_rows(huge_raw, plan)
    bundle = _conditional_context_unit_bundle(plan, output)
    assert cp.validate_results(bundle, "incremental_rows") == bundle
    for field, value in (("schema_version", 2.0), ("exit_code", False), ("exit_code", 0.0)):
        mutant = json.loads(json.dumps(bundle))
        mutant["execution_receipt"][field] = value
        with pytest.raises(ValueError):
            cp.validate_results(mutant, "incremental_rows")
    original_events = cp.decode(bundle["execution_context"]["build"]["stdout"]).splitlines()
    for events in (
        original_events + [cp.canonical({"reason": "build-finished", "success": False})],
        original_events + [original_events[-1]],
        original_events + [cp.canonical({"reason": "compiler-message"})],
    ):
        mutant = json.loads(json.dumps(bundle))
        mutant["execution_context"]["build"]["stdout"] = cp.artifact(b"\n".join(events))
        mutant["execution_receipt"]["context_sha256"] = cp.sha(
            cp.canonical(mutant["execution_context"])
        )
        with pytest.raises(ValueError):
            cp.validate_results(mutant, "incremental_rows")
    for case in plan["cases"]:
        vacuous = json.loads(json.dumps(case))
        vacuous["before"]["replace_scopes"] = vacuous["fresh"]["replace_scopes"]
        with pytest.raises(ValueError, match="vacuous"):
            cp.operation_oracle(vacuous)
        removed_sentinel = json.loads(json.dumps(case))
        for scope in removed_sentinel["fresh"]["replace_scopes"]:
            scope["embeddings"] = [
                row for row in scope["embeddings"] if row["owner_id"] != "unaffected-owner"
            ]
        with pytest.raises(ValueError, match="unaffected-owner"):
            cp.operation_oracle(removed_sentinel)

    def reject_fixture_source(repo, closure):
        assert repo == cp.ROOT
        assert closure == bundle["execution_context"]["source_closure"]
        raise source_closure.ClosureError("synthetic fixture is not source evidence")

    with monkeypatch.context() as patch:
        patch.setattr(source_closure, "verify_manifest", reject_fixture_source)
        with pytest.raises(ValueError, match="not source evidence"):
            cp.validate_results(bundle, "incremental_rows", verify_source=True)
    for mutate in (
        lambda context: context["build"].update(argv=[]),
        lambda context: context["run"].update(argv=[]),
        lambda context: context["run"].update(cwd="/tmp/wrong-source"),
        lambda context: context["run"].update(environment={"CARGO_NET_OFFLINE": "false"}),
        lambda context: context["run"].update(stdout=cp.artifact(b"{}")),
        lambda context: context["build"].update(stdout=cp.artifact(b"{}")),
        lambda context: context.update(binary_sha256="d" * 64),
        lambda context: context.update(uv_lock_sha256="d" * 64),
        lambda context: context.update(inputs=cp.artifact(b"{}")),
        lambda context: context.update(records=cp.artifact(cp.canonical([]))),
        lambda context: context.update(records=cp.artifact(cp.canonical([{"captures": []}]))),
        lambda context: context["build"].update(stdout=cp.artifact(b"[]")),
        lambda context: context["build"].update(
            stdout=cp.artifact(b'{"reason":"compiler-artifact","target":[]}')
        ),
        lambda context: context["environment"].update(python_version=3),
        lambda context: context["environment"].update(relevant=[]),
    ):
        mutant = json.loads(json.dumps(bundle))
        mutate(mutant["execution_context"])
        mutant["execution_receipt"]["context_sha256"] = cp.sha(
            cp.canonical(mutant["execution_context"])
        )
        with pytest.raises((ValueError, KeyError)):
            cp.validate_results(mutant, "incremental_rows")
    for field, value in (("start_line", True), ("start_line", 1.0), ("generated", 0)):
        mutant = json.loads(json.dumps(output))
        mutant["cases"][0]["incremental"]["semantic"]["rows"][0][field] = value
        with pytest.raises(ValueError):
            cp.incremental_rows(mutant, plan)
    for field, value in (("ordinal", False), ("member_count", 1.0)):
        mutant = json.loads(json.dumps(output))
        mutant["cases"][2]["incremental"]["membership"]["rows"][0][field] = value
        with pytest.raises(ValueError):
            cp.incremental_rows(mutant, plan)
    for value in (1.0, True, -1, 2**64):
        mutant = json.loads(json.dumps(output))
        mutant["cases"][0]["receipts"]["before"]["generation"] = value
        with pytest.raises(ValueError):
            cp.incremental_rows(mutant, plan)
        for batch, field in (("before", "generation"), ("delta", "base_generation")):
            bad_plan = json.loads(json.dumps(plan))
            bad_plan["cases"][0][batch][field] = value
            with pytest.raises(ValueError):
                cp.incremental_rows(output, bad_plan)
    receipt = output["cases"][0]["receipts"]["before"]
    # Reject historical per-owner commit counts even when all full rows
    # and logical-call/append receipts remain unchanged and self-consistent.
    for case_id, batch, legacy_commits in (
        ("append", "before", 2),
        ("append", "fresh", 3),
        ("membership_replace", "fresh", 2),
        ("replace", "fresh", 2),
    ):
        mutant = json.loads(json.dumps(output))
        changed = next(case for case in mutant["cases"] if case["case_id"] == case_id)
        changed["receipts"][batch]["stages"]["semantic_delete_commits"] = legacy_commits
        with pytest.raises(ValueError, match="delete counts"):
            cp.incremental_rows(mutant, plan)
    for case in output["cases"]:
        for batch in ("before", "fresh", "delta"):
            for field in (
                "semantic_delete_calls",
                "semantic_delete_commits",
                "membership_delete_calls",
                "membership_delete_commits",
            ):
                for difference in (-1, 1):
                    if case["receipts"][batch]["stages"][field] + difference < 0:
                        continue
                    mutant = json.loads(json.dumps(output))
                    changed = next(
                        item for item in mutant["cases"] if item["case_id"] == case["case_id"]
                    )
                    changed["receipts"][batch]["stages"][field] += difference
                    with pytest.raises(ValueError, match="delete counts"):
                        cp.incremental_rows(mutant, plan)
    for field, value in (
        ("windows", 0),
        ("windows", 2),
        ("semantic_append_calls", 0),
        ("semantic_append_calls", 2),
        ("membership_append_calls", 1),
    ):
        mutant = json.loads(json.dumps(output))
        changed = mutant["cases"][0]["receipts"]["before"]
        changed["stages"][field] = value
        if field == "windows":
            changed[field] = value
        with pytest.raises(ValueError, match="execution counts"):
            cp.incremental_rows(mutant, plan)
    for path in (
        ("windows",),
        ("replace_scopes",),
        ("rows",),
        *(("stages", key) for key in receipt["stages"] if key != "durations"),
        *(
            ("stages", "durations", key)
            for key in receipt["stages"]["durations"]
            if key != "embedding"
        ),
    ):
        for value in (False, 0.0, -1, 2**64):
            mutant = json.loads(json.dumps(output))
            target = mutant["cases"][0]["receipts"]["before"]
            for key in path[:-1]:
                target = target[key]
            target[path[-1]] = value
            with pytest.raises(ValueError):
                cp.incremental_rows(mutant, plan)
    for field, value in (("snippet", "changed payload"), ("vector", [0.0, 1.0])):
        mutant = json.loads(json.dumps(output))
        mutant["cases"][0]["incremental"]["semantic"]["rows"][0][field] = value
        assert cp.incremental_rows(mutant, plan)[1] == 4
    mutant = json.loads(json.dumps(output))
    mutant["cases"][2]["incremental"]["membership"]["rows"][0]["member_symbol_id"] = "wrong-symbol"
    assert cp.incremental_rows(mutant, plan)[1] == 4
    for mutate in (
        lambda value: value["cases"].pop(),
        lambda value: value["cases"][0]["incremental"]["semantic"].update(count=2),
        lambda value: value["cases"][0]["incremental"]["semantic"]["rows"][0].pop("vector"),
        lambda value: value["cases"][0]["receipts"]["delta"].update(batch_digest="wrong"),
        lambda value: value["cases"][0]["fresh"]["semantic"]["rows"][0].update(
            snippet="forged oracle"
        ),
    ):
        mutant = json.loads(json.dumps(output))
        mutate(mutant)
        with pytest.raises(ValueError):
            cp.incremental_rows(mutant, plan)
    # v1's self-consistent IDs/counts still cannot become v2 execution custody.
    with pytest.raises(ValueError, match="missing or unexpected"):
        cp.validate_results(
            _bound_conditional_results("incr-cmd", "incremental_rows"), "incremental_rows"
        )


def test_query_clock_overhead_replay_requires_identical_answers_and_coverage(tmp_path):
    stage = _pair_stage(tmp_path)
    path = next(stage["stage"].glob("rep-00/quanta/strategy-*/retrieval-diagnostic.json"))
    record = json.loads(path.with_name("record.json").read_text())
    record["span_accounting_version"] = 1
    suite = json.loads(stage["suite_path"].read_text())
    pack = json.loads((stage["stage"] / "query-pack.json").read_text())
    pack, _ = pairrun.project_pack_and_suite(pack, suite, sorted(record["route_provenance"]))
    diagnostic = json.loads(path.read_text())
    diagnostic["schema_version"] = 5
    diagnostic["server_observation"] = pairrun.server_observation_configuration("enabled")
    diagnostic["ingest"] = _diagnostic_ingest_fixture(record)
    diagnostic["record_sha256"] = cp.sha(cp.canonical(record))
    off = json.loads(json.dumps(diagnostic))
    off["server_observation"] = pairrun.server_observation_configuration("disabled")
    for row in off["results"]:
        row["response"]["explanation"]["stage_timings"] = None
    phases = json.loads(path.with_name("phase-metrics.json").read_text())
    phases["record_sha256"] = diagnostic["record_sha256"]
    phases["measurement_repetitions"] = 2
    phases["query_protocol"] = pairrun.build_query_protocol(phases["query_schedule"], 0, 1, 2)
    phases["warm_latencies_ms"] = {}
    for row in record["results"]:
        phases["warm_latencies_ms"].setdefault(row["route"], {})[row["task_id"]] = [4.0, 6.0]
    off_phases = json.loads(json.dumps(phases))
    for tasks in off_phases["warm_latencies_ms"].values():
        for task in tasks:
            tasks[task] = [2.0, 3.0]
    for value in (phases, off_phases):
        value["phases_ms"]["warm_query"] = sum(
            sum(samples)
            for tasks in value["warm_latencies_ms"].values()
            for samples in tasks.values()
        )
        value["total_ms"] = sum(value["phases_ms"].values())
    result = overhead.compare(record, record, phases, off_phases, diagnostic, off, pack)
    assert result["schema_version"] == 2
    assert result["scorer_identity"] == "query-stage-clock-overhead-v2"
    assert result["status"] == "diagnostic_unqualified"
    assert result["measurement_scope"] == "plane_stage_and_response_trace_observation"
    assert result["backend_clock_reads"] == "enabled_in_both_arms"
    assert result["qualification_limits"] == [
        "not_total_instrumentation_overhead",
        "not_ipc_attribution",
        "host_and_repetition_qualification_not_established",
    ]
    assert all(
        row["on_median_ms"] == 5.0
        and row["off_median_ms"] == 2.5
        and row["delta_ms"] == 2.5
        and row["relative_delta"] == 1.0
        for row in result["rows"]
    )
    for invalid in (10**400, -(10**400), float("nan"), float("inf"), -float("inf"), True, None):
        forged = json.loads(json.dumps(off_phases))
        next(iter(next(iter(forged["warm_latencies_ms"].values())).values()))[0] = invalid
        with pytest.raises((ValueError, pairrun.RunError)):
            overhead.compare(record, record, phases, forged, diagnostic, off, pack)
    for mutate in (
        lambda value: value.update(query_schedule="wrong"),
        lambda value: value.update(measurement_repetitions=3),
        lambda value: next(iter(value["warm_latencies_ms"].values())).pop(
            next(iter(next(iter(value["warm_latencies_ms"].values()))))
        ),
        lambda value: next(iter(value["warm_latencies_ms"].values())).__setitem__(
            record["results"][0]["task_id"], [float("nan"), 2.0]
        ),
    ):
        mutant = json.loads(json.dumps(off_phases))
        mutate(mutant)
        with pytest.raises((ValueError, pairrun.RunError)):
            overhead.compare(record, record, phases, mutant, diagnostic, off, pack)
    with pytest.raises(ValueError, match="observation policy"):
        overhead.compare(record, record, phases, off_phases, diagnostic, diagnostic, pack)
    mutant = json.loads(json.dumps(record))
    mutant["results"][0]["candidates"] = []
    with pytest.raises((ValueError, pairrun.RunError)):
        overhead.compare(record, mutant, phases, off_phases, diagnostic, off, pack)
    # The runner record omits page/continuation and lane details. Each
    # diagnostic can be individually valid while the on/off pair diverges.
    mutants = []
    changed = json.loads(json.dumps(off))
    changed["results"][0]["response"]["window"]["coverage"]["examined"] = {"kind": "unknown"}
    mutants.append(changed)
    changed = json.loads(json.dumps(off))
    changed["results"][0]["response"]["explanation"]["strategy"] = "forged"
    mutants.append(changed)
    changed = json.loads(json.dumps(off))
    changed["results"][0]["response"]["window"]["coverage"]["lanes"][0]["filtered_out"] = 1
    mutants.append(changed)
    changed = json.loads(json.dumps(off))
    candidate_row = next(row for row in changed["results"] if row["candidates"])
    candidate_row["candidates"][0]["candidate_id"] = "forged-candidate"
    mutants.append(changed)
    changed = json.loads(json.dumps(off))
    changed["results"].reverse()
    mutants.append(changed)
    changed = json.loads(json.dumps(off))
    changed["results"].pop()
    mutants.append(changed)
    changed = json.loads(json.dumps(off))
    changed["results"][1] = json.loads(json.dumps(changed["results"][0]))
    mutants.append(changed)
    for changed in mutants:
        with pytest.raises((ValueError, pairrun.RunError)):
            overhead.compare(record, record, phases, off_phases, diagnostic, changed, pack)
    # Request IDs are transport-local; continuation is an observable page fact.
    different_ids = json.loads(json.dumps(off))
    for row in different_ids["results"]:
        row["response"]["explanation"]["request_id"] += 100
    assert overhead.compare(record, record, phases, off_phases, diagnostic, different_ids, pack)[
        "rows"
    ]
    capped_record = json.loads(json.dumps(record))
    capped_record["results"][0]["status"] = "capped"
    capped_on = json.loads(json.dumps(diagnostic))
    capped_off = json.loads(json.dumps(off))
    for capture in (capped_on, capped_off):
        capture["record_sha256"] = cp.sha(cp.canonical(capped_record))
        row = capture["results"][0]
        row["status"] = "capped"
        window = row["response"]["window"]
        window["candidate_count"] = {"kind": "at_least", "value": window["returned"] + 1}
        window["outcome"] = {"kind": "lower_bound", "continuation": True}
        window["coverage"].pop("exhaustion_proof")
    capped_phases = json.loads(json.dumps(phases))
    capped_phases["record_sha256"] = capped_on["record_sha256"]
    capped_off_phases = json.loads(json.dumps(off_phases))
    capped_off_phases["record_sha256"] = capped_on["record_sha256"]
    assert overhead.compare(
        capped_record, capped_record, capped_phases, capped_off_phases, capped_on, capped_off, pack
    )["rows"]
    capped_off["results"][0]["response"]["window"]["outcome"]["continuation"] = False
    with pytest.raises(ValueError, match="observable diagnostic"):
        overhead.compare(
            capped_record,
            capped_record,
            capped_phases,
            capped_off_phases,
            capped_on,
            capped_off,
            pack,
        )


def test_query_clock_overhead_v7_preserves_current_diagnostic_and_hybrid_policy(tmp_path):
    stage = _pair_stage(tmp_path, diagnostic_version=7)
    path = next(stage["stage"].glob("rep-00/quanta/strategy-*/retrieval-diagnostic.json"))
    record = json.loads(path.with_name("record.json").read_text())
    record["span_accounting_version"] = 1
    suite = json.loads(stage["suite_path"].read_text())
    pack = json.loads((stage["stage"] / "query-pack.json").read_text())
    pack, _ = pairrun.project_pack_and_suite(pack, suite, sorted(record["route_provenance"]))
    on = json.loads(path.read_text())
    on["record_sha256"] = cp.sha(cp.canonical(record))
    off = json.loads(json.dumps(on))
    off["server_observation"] = pairrun.server_observation_configuration("disabled")
    for row in off["results"]:
        row["response"]["explanation"]["stage_timings"] = None
    for name, value in (("candidate_ns", 100), ("sort_page_ns", 20), ("preview_ns", 5)):
        on["results"][0]["response"]["explanation"]["planner_trace"].append(
            {"stage": "merge", "detail": f"code_search.execution.{name}={value}"}
        )
    phases = json.loads(path.with_name("phase-metrics.json").read_text())
    phases["record_sha256"] = on["record_sha256"]
    phases["measurement_repetitions"] = 2
    phases["query_protocol"] = pairrun.build_query_protocol(phases["query_schedule"], 0, 1, 2)
    phases["warm_latencies_ms"] = {
        row["route"]: {row["task_id"]: [4.0, 6.0] for row in record["results"]}
        for row in record["results"]
    }
    phases["phases_ms"]["warm_query"] = 10.0 * len(record["results"])
    phases["total_ms"] = sum(
        value
        for key, value in phases["phases_ms"].items()
        if key not in ("sdk_publish", "sdk_activate")
    )
    off_phases = json.loads(json.dumps(phases))
    off_phases["warm_latencies_ms"] = {
        route: {task_id: [2.0, 3.0] for task_id in rows}
        for route, rows in phases["warm_latencies_ms"].items()
    }
    off_phases["phases_ms"]["warm_query"] = 5.0 * len(record["results"])
    off_phases["total_ms"] = sum(
        value
        for key, value in off_phases["phases_ms"].items()
        if key not in ("sdk_publish", "sdk_activate")
    )
    assert overhead.compare(record, record, phases, off_phases, on, off, pack)["rows"]
    forged = json.loads(json.dumps(off))
    forged["hybrid_fetch_policy"] = pairrun.hybrid_fetch_policy_configuration("25")
    with pytest.raises(ValueError, match="hybrid fetch policy"):
        overhead.compare(record, record, phases, off_phases, on, forged, pack)


@pytest.mark.parametrize("rank_unit", ["distinct_file", None])
def test_current_v7_diagnostic_replays_native_file_or_chunk_projection(tmp_path, rank_unit):
    stage = _pair_stage(tmp_path, diagnostic_version=7)
    path = next(stage["stage"].glob("rep-00/quanta/strategy-*/retrieval-diagnostic.json"))
    record = json.loads(path.with_name("record.json").read_text())
    diagnostic = json.loads(path.read_text())
    suite = json.loads(stage["suite_path"].read_text())
    pack = json.loads((stage["stage"] / "query-pack.json").read_text())
    pack, _ = pairrun.project_pack_and_suite(pack, suite, ["lexical"])
    scored = record["results"][0]
    row = diagnostic["results"][0]
    if rank_unit is not None:
        scored["rank_unit"] = rank_unit
    hits = []
    assert len(scored["candidates"]) == len(row["candidates"])
    for rank, (candidate, observed) in enumerate(zip(scored["candidates"], row["candidates"]), 1):
        if rank_unit == "distinct_file":
            source = (stage["stage"] / "runner-corpus" / candidate["path"]).read_bytes()
            candidate["start_byte"] = 0
            candidate["end_byte"] = len(source)
            candidate["start_line"] = 1
            candidate["end_line"] = len(source.splitlines())
            candidate["block_sha256"] = ev.digest(source)
            observed["start_line"] = candidate["start_line"]
            observed["end_line"] = candidate["end_line"]
        unit_id = f"{rank_unit or 'chunk'}:{rank}"
        candidate["span_accounting"] = {
            "unit_kind": "file" if rank_unit else "chunk",
            "unit_id": unit_id,
        }
        observed["candidate_id"] = unit_id
        hits.append(
            {
                "candidate_id": unit_id,
                "path": candidate["path"],
                "start_byte": candidate["start_byte"],
                "end_byte": candidate["end_byte"],
                "scored_rank": rank,
            }
        )
    row["response"]["native_projection"] = {
        "policy": "first-source-span-v1",
        "hits": hits,
    }
    diagnostic["record_sha256"] = cp.sha(cp.canonical(record))
    assert pairrun.validate_retrieval_diagnostic(
        diagnostic, record, diagnostic["record_sha256"], pack
    )
    forged = json.loads(json.dumps(diagnostic))
    forged["results"][0]["response"]["native_projection"]["hits"][0]["scored_rank"] = 2
    with pytest.raises(pairrun.RunError, match="native projection"):
        pairrun.validate_retrieval_diagnostic(forged, record, diagnostic["record_sha256"], pack)


def test_query_clock_comparator_excludes_only_policy_controlled_code_search_clocks():
    trace = [
        {"stage": "merge", "detail": "code_search.execution.mode=ordinary"},
        {"stage": "merge", "detail": "code_search.execution.candidate_ns=123"},
        {"stage": "merge", "detail": "code_search.execution.sort_page_ns=456"},
        {"stage": "merge", "detail": "code_search.execution.preview_ns=789"},
        {"stage": "merge", "detail": "code_search.execution.posting_probes=3"},
        {"stage": "merge", "detail": "code_search.execution.candidate_ns_extra=1"},
        {"stage": "merge", "detail": "hybrid.initial_fetch=10"},
    ]
    observed = overhead._without_code_search_work_clocks(trace)
    assert observed == [trace[0], *trace[4:]]
    assert trace[1:4] != observed[1:4]
    with pytest.raises(ValueError, match="disabled query observation"):
        overhead._without_code_search_work_clocks(trace, allow_clocks=False)
    for malformed in (
        [*trace, trace[1]],
        [{**trace[1], "detail": "code_search.execution.candidate_ns=bad"}],
        [{**trace[1], "detail": "code_search.execution.candidate_ns="}],
    ):
        with pytest.raises(ValueError, match="work clock"):
            overhead._without_code_search_work_clocks(malformed)


def test_query_clock_comparator_validates_nested_typo_clocks_without_erasing_counts():
    trace = [
        {"stage": "merge", "detail": "code_search.execution.mode=typo_fallback"},
        {"stage": "merge", "detail": "code_search.execution.candidate_ns=100"},
        {"stage": "merge", "detail": "code_search.execution.sort_page_ns=10"},
        {"stage": "merge", "detail": "code_search.execution.preview_ns=20"},
        {"stage": "merge", "detail": "code_search.execution.typo_shortlist_admission_ns=10"},
        {"stage": "merge", "detail": "code_search.execution.typo_source_token_scan_ns=60"},
        {"stage": "merge", "detail": "code_search.execution.typo_materialize_ns=20"},
        {"stage": "merge", "detail": "code_search.execution.typo_token_comparisons=17"},
    ]
    assert overhead._without_code_search_work_clocks(trace) == [trace[0], trace[-1]]
    # The remaining 10 ns can contain the preceding ordinary pass and clock overhead.
    with pytest.raises(ValueError, match="disabled query observation"):
        overhead._without_code_search_work_clocks(trace, allow_clocks=False)
    for mutant in (
        trace[:6] + trace[7:],
        trace[:1] + trace[2:],
        [{"stage": "merge", "detail": "code_search.execution.mode=ordinary"}, *trace[1:]],
        [trace[0], *trace],
        [
            *trace[:5],
            {"stage": "merge", "detail": "code_search.execution.typo_source_token_scan_ns=71"},
            *trace[6:],
        ],
    ):
        with pytest.raises(ValueError, match="clock hierarchy"):
            overhead._without_code_search_work_clocks(mutant)
    for value in ("-1", "True", str(1 << 64), "0" * 21):
        with pytest.raises(ValueError, match="work clock"):
            overhead._without_code_search_work_clocks(
                [{"stage": "merge", "detail": f"code_search.execution.candidate_ns={value}"}]
            )


def test_v8_authority_stage_replay_preserves_nullable_children_and_phase_contract(tmp_path):
    stage = _pair_stage(tmp_path, diagnostic_version=8)
    path = next(stage["stage"].glob("rep-00/quanta/strategy-*/retrieval-diagnostic.json"))
    record = json.loads(path.with_name("record.json").read_text())
    diagnostic = json.loads(path.read_text())
    pack = json.loads((stage["stage"] / "query-pack.json").read_text())
    suite = json.loads(stage["suite_path"].read_text())
    pack, _ = pairrun.project_pack_and_suite(pack, suite, ["lexical"])
    assert pairrun.validate_retrieval_diagnostic(
        diagnostic, record, pairrun.sha_file(path.with_name("record.json")), pack
    )
    verdict = _stage_verdict(stage)
    assert verdict["states"]["PAIR_VALID"] == "pass", verdict["state_evidence"]["PAIR_VALID"]
    base = diagnostic["ingest"]
    for children in ((None, 3, 4), (None, None, None), (0, 0, 0)):
        raw = copy.deepcopy(base)
        raw["observation"]["lexical_stages"].update(
            zip(
                (
                    "text_authority_collect_ns",
                    "text_authority_shard_build_ns",
                    "text_authority_publish_ns",
                ),
                children,
            )
        )
        pairrun._validate_ingest_diagnostic(
            raw, record, lexical_stage_contract=True, detailed_authority=True
        )
    for children in ((2, None, 4), (2, None, None), (2, 3, 6), (True, 3, 4), (-1, 3, 4)):
        raw = copy.deepcopy(base)
        raw["observation"]["lexical_stages"].update(
            zip(
                (
                    "text_authority_collect_ns",
                    "text_authority_shard_build_ns",
                    "text_authority_publish_ns",
                ),
                children,
            )
        )
        with pytest.raises(pairrun.RunError):
            pairrun._validate_ingest_diagnostic(
                raw, record, lexical_stage_contract=True, detailed_authority=True
            )
    missing = copy.deepcopy(base)
    del missing["observation"]["lexical_stages"]["text_authority_publish_ns"]
    with pytest.raises(pairrun.RunError, match="ingest lexical stages must hold exactly"):
        pairrun._validate_ingest_diagnostic(
            missing, record, lexical_stage_contract=True, detailed_authority=True
        )
    with pytest.raises(pairrun.RunError):
        pairrun._validate_ingest_diagnostic(base, record, lexical_stage_contract=True)

    # A valid historical phase cannot satisfy a current producer's SDK timing
    # contract, even when all capture byte bindings are updated consistently.
    phase_path = path.with_name("phase-metrics.json")
    phase = json.loads(phase_path.read_text())
    phase["schema_version"] = 3
    for observation in phase["query_timing"]["observations"]:
        for key in ("sdk_execute_ns", "sdk_post_execute_ns", "runner_result_materialize_ns"):
            del observation[key]
    pairrun._validate_phase_metrics(phase, "historical phase fixture")
    phase_path.write_text(json.dumps(phase))
    manifest_path = path.parent.parent / "quanta-manifest.json"
    manifest = json.loads(manifest_path.read_text())
    manifest["runs"][0]["phase_metrics_digest"] = pairrun.sha_file(phase_path)
    manifest_path.write_text(json.dumps(manifest))
    _rebind_phase_metrics_digests(stage, phase_path)
    verdict = _stage_verdict(stage)
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert "protocol v6 requires measured Quanta phase schema v4" in str(
        verdict["state_evidence"]["PAIR_VALID"]
    )


@pytest.mark.parametrize("parameter", ["timeout_secs", "cleanup_timeout_secs"])
@pytest.mark.parametrize(
    "invalid", [10**400, -(10**400), float("inf"), float("nan"), True, 0, -1, None]
)
def test_native_linux_monitor_numeric_preflight_refuses_without_conversion(parameter, invalid):
    from tools.benchmark.retrieval import linux_process

    kwargs = {"timeout_secs": 1, parameter: invalid}
    with pytest.raises(linux_process.ProcessError, match="invalid command or timeout"):
        linux_process.run(["fixture"], **kwargs)


@pytest.mark.parametrize(
    "invalid", [10**400, -(10**400), float("inf"), float("nan"), True, 0, -1, None]
)
def test_native_windows_monitor_numeric_preflight_always_closes(invalid):
    from tools.benchmark.retrieval import windows_job

    job = object.__new__(windows_job.OwnedWindowsProcess)
    job._closed = False
    closed = []
    job.close = lambda: closed.append(True)
    with pytest.raises(windows_job.JobError, match="monitor timeout and sample interval"):
        job.monitor(timeout_secs=invalid)
    assert closed == [True]


def test_native_windows_monitor_numeric_preflight_reports_cleanup_failure():
    from tools.benchmark.retrieval import windows_job

    job = object.__new__(windows_job.OwnedWindowsProcess)
    job._closed = False
    closed = []

    def fail_close():
        closed.append(True)
        raise windows_job.JobError("fixture cleanup failure")

    job.close = fail_close
    with pytest.raises(
        windows_job.JobError,
        match="invalid monitor parameters; cleanup failed: fixture cleanup failure",
    ):
        job.monitor(timeout_secs=10**400)
    assert closed == [True]


@pytest.mark.parametrize("valid", [1, 0.25, sys.float_info.max])
def test_native_linux_monitor_numeric_preflight_accepts_finite_positive(valid, monkeypatch):
    from tools.benchmark.retrieval import linux_process

    sentinel = object()
    monkeypatch.setattr(linux_process, "_run_cgroup", lambda *args, **kwargs: sentinel)
    assert (
        linux_process.run(
            ["fixture"], timeout_secs=valid, cleanup_timeout_secs=valid, qualified=True
        )
        is sentinel
    )


@pytest.mark.parametrize("valid", [1, 0.25, sys.float_info.max])
def test_native_windows_monitor_numeric_preflight_accepts_finite_positive(valid):
    from tools.benchmark.retrieval import windows_job

    job = object.__new__(windows_job.OwnedWindowsProcess)
    job._closed = False
    job.pid = 123
    job.stdout_path = job.stderr_path = None
    initial = windows_job.JobSample(1, 10, 1, 1, 1, 1, 20)
    final = windows_job.JobSample(2, 10, 1, 1, 1, 0)
    closed = []
    job.sample = lambda: initial
    job.wait = lambda _: 0

    def close():
        closed.append(True)
        return final

    job.close = close
    result = job.monitor(timeout_secs=valid)
    assert result.root_exit_code == 0 and result.cleanup_complete
    assert closed == [True]


def _file_projection_run(tmp_path, policy, *, reverse=False, ordering="derive", queries=None):
    repo, suite, run, suite_path, runner_path, files = fixture_v3(tmp_path, answerable_only=True)
    suite["routes"] = ["lexical"]
    suite["diagnostic_policy"] = ev.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
    for index, task in enumerate(suite["tasks"]):
        if queries is not None:
            task["query"] = queries[index]
            task["query_sha256"] = ev.digest(task["query"].encode())
        task["label_review"] = {
            "assessment": "reviewed_unambiguous",
            "reviewer_id": "fixture-reviewer",
            "evidence_sha256": ev.digest(b"file projection fixture"),
        }
        task["judgment_policy"] = ev.UNJUDGED_POLICY
        task["file_judgments"] = [
            {"path": "a.txt", "file_sha256": ev.digest(files["a.txt"]), "grade": 3},
            {"path": "b.txt", "file_sha256": ev.digest(files["b.txt"]), "grade": 1},
        ]
    profile = qp.execution_profile(policy)
    run["captures"]["q0"]["execution_profile"] = profile
    run["captures"]["q0"]["execution_profile_sha256"] = ev.digest(ev.canonical(profile))
    if policy == "code_search_exact_content_file":
        run["captures"]["q0"]["source_repo_id"] = "bench-repo"
        run["captures"]["q0"]["source_revision_id"] = suite["repository_commit"]
    run["route_provenance"] = {"lexical": {"capture_id": "q0"}}
    run["results"] = [row for row in run["results"] if row["route"] == "lexical"]
    if policy in (
        "code_search_file",
        "code_search_exact_content_file",
        "code_search_typo_file",
        "code_search_components_file",
        "natural_language_file",
    ):
        run["span_accounting_version"] = 1
    for task, row in zip(suite["tasks"], run["results"], strict=True):
        row["rank_unit"] = "distinct_file"
        if ordering == "derive":
            row["ordering"] = qp.FILE_PROJECTION_ORDERING[policy]
        elif ordering is not None:
            row["ordering"] = ordering
        row["query_identity"] = qp.derive_query_identity(policy, task["query"])
        by_file = {}
        for item in row["candidates"]:
            by_file.setdefault(item["path"], item)
        if policy in (
            "code_search_file",
            "code_search_exact_content_file",
            "code_search_typo_file",
            "code_search_components_file",
        ):
            repo_bytes = b"bench-repo"
            for path, item in by_file.items():
                path_bytes = path.encode("utf-8")
                source = files[path]
                file_hash = ev.digest(source)
                identity = (
                    b"quanta-index:code-search-file:v1\x00"
                    + len(repo_bytes).to_bytes(8, "little")
                    + repo_bytes
                    + len(path_bytes).to_bytes(8, "little")
                    + path_bytes
                )
                item.update(
                    start_byte=0,
                    end_byte=len(source),
                    start_line=1,
                    end_line=len(source.splitlines()),
                    file_sha256=file_hash,
                    block_sha256=file_hash,
                    tokens=len(ev.TOKEN_RE.findall(path)),
                    span_accounting={
                        "unit_kind": "file",
                        "unit_id": "file:" + ev.digest(identity),
                        "producer_identity": "code-search-file-v1",
                        "indexed_start_byte": 0,
                        "indexed_end_byte": len(source),
                        "sdk_start_line": 0,
                        "sdk_end_line": 0,
                        "extra_context_bytes": 0,
                        "source_repo_id": repo_bytes.decode(),
                        "source_revision_id": (
                            suite["repository_commit"]
                            if policy == "code_search_exact_content_file"
                            else "bench-revision"
                        ),
                        "preview_kind": "path",
                        "preview_start_byte": None,
                        "preview_end_byte": None,
                        "snippet_sha256": ev.digest(path_bytes),
                    },
                )
        if policy == "natural_language_file":
            for item in by_file.values():
                item["span_accounting"] = {
                    "unit_kind": "chunk",
                    "unit_id": f"{task['task_id']}:{item['rank']}",
                    "producer_identity": run["captures"]["q0"]["chunk_strategy"],
                    "indexed_start_byte": item["start_byte"],
                    "indexed_end_byte": item["end_byte"],
                    "sdk_start_line": item["start_line"],
                    "sdk_end_line": item["end_line"],
                    "extra_context_bytes": 0,
                }
        chosen = [by_file[path] for path in sorted(by_file, reverse=reverse)]
        for rank, item in enumerate(chosen, 1):
            item["rank"] = rank
        row["candidates"] = chosen
    _pack, run = _repack(repo, suite, run)
    return repo, suite, run, suite_path, runner_path


def _file_review_capture_fixture(tmp_path):
    fixture = _file_projection_run(
        tmp_path, "natural_language_file", queries=["find alphaTwo", "find alphaThree"]
    )
    for row in fixture[2]["results"]:
        row["score_evidence"] = "native_sdk_score_v1"
        for rank, candidate in enumerate(row["candidates"]):
            candidate["score"] = float(len(row["candidates"]) - rank)
    return fixture


def test_capture_review_pool_blinds_native_files_and_preserves_empty_results(tmp_path):
    from tools.benchmark.retrieval import holdout_review

    repo, suite, run, suite_path, record_path = _file_review_capture_fixture(tmp_path)
    run["results"][1]["status"] = "abstained"
    run["results"][1]["candidates"] = []
    _checked, original_pack, _record = record_v3(repo, suite, run, suite_path, record_path)
    raw = record_path.read_bytes()
    pack, pool, custody = holdout_review.capture_review_pool(
        repo, suite_path, record_path, pool_id="observed-native-file-route"
    )
    assert pack == original_pack
    assert pool == {
        "pool_id": "observed-native-file-route",
        "kind": "retrieval",
        "tasks": {
            suite["tasks"][0]["task_id"]: [
                {"path": path, "file_sha256": ev.digest((repo / path).read_bytes())}
                for path in ("a.txt", "b.txt")
            ],
            suite["tasks"][1]["task_id"]: [],
        },
    }
    assert custody["record_bytes_sha256"] == ev.digest(raw)
    assert custody["qualified"] is custody["pool_execution_attested"] is False
    assert record_path.read_bytes() == raw


@pytest.mark.parametrize("fault", ["record_commitment", "source_digest", "input_race"])
def test_capture_review_pool_refuses_unbound_inputs(tmp_path, monkeypatch, fault):
    from tools.benchmark.retrieval import holdout_review

    repo, suite, run, suite_path, record_path = _file_review_capture_fixture(tmp_path)
    record_v3(repo, suite, run, suite_path, record_path)
    if fault == "input_race":
        original = ev.load_evidence

        def raced(*args):
            loaded = original(*args)
            record_path.write_bytes(record_path.read_bytes() + b"\n")
            return loaded

        monkeypatch.setattr(ev, "load_evidence", raced)
    else:
        payload = json.loads(record_path.read_text())
        if fault == "record_commitment":
            payload["query_pack_sha256"] = "0" * 64
        else:
            payload["results"][0]["candidates"][0]["file_sha256"] = "0" * 64
        record_path.write_bytes(ev.canonical(payload))
    with pytest.raises(ev.EvidenceError):
        holdout_review.capture_review_pool(repo, suite_path, record_path, pool_id="captured")


def test_capture_review_pool_refuses_chunk_collapse_or_multi_route_pool(tmp_path):
    from tools.benchmark.retrieval import holdout_review

    repo, suite, run, suite_path, record_path, _files = fixture_v3(tmp_path)
    record_v3(repo, suite, run, suite_path, record_path)
    with pytest.raises(ev.EvidenceError, match="one recorded route"):
        holdout_review.capture_review_pool(repo, suite_path, record_path, pool_id="captured")
    suite["routes"] = ["lexical"]
    run["route_provenance"] = {"lexical": {"capture_id": "q0"}}
    run["results"] = [row for row in run["results"] if row["route"] == "lexical"]
    _pack, run = _repack(repo, suite, run)
    record_v3(repo, suite, run, suite_path, record_path)
    with pytest.raises(ev.EvidenceError, match="native distinct_file"):
        holdout_review.capture_review_pool(repo, suite_path, record_path, pool_id="captured")


def test_file_projection_policies_bind_ordering_and_interpret_metrics(tmp_path):
    keyword = ["alphaTwo", "alphaThree"]
    repo, suite, run, suite_path, runner_path = _file_projection_run(
        tmp_path / "kw", "keyword_file", reverse=True, queries=keyword
    )
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    route = ev.evaluate_diagnostic(loaded_suite, pack, loaded_run)["judgment_metrics"][
        "file_judgments"
    ]["routes"]["lexical"]
    # A scored ranking may place b.txt first; its NDCG is a ranking number.
    assert route["ordering"] == "score_desc_path_tiebreak"
    assert route["rank_metric_interpretation"] == "scored_ranking"
    assert route["conditional_mean"]["hit_at_10"] == 1.0

    repo, suite, run, suite_path, runner_path = _file_projection_run(
        tmp_path / "sub", "substring_file"
    )
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    route = ev.evaluate_diagnostic(loaded_suite, pack, loaded_run)["judgment_metrics"][
        "file_judgments"
    ]["routes"]["lexical"]
    assert route["ordering"] == "path_order_constant_score"
    assert route["rank_metric_interpretation"] == "observed_path_order_prefix"
    assert route["conditional_mean"]["hit_at_10"] == 1.0
    assert 0 < route["conditional_mean"]["recall_at_10"] <= 1.0


@pytest.mark.parametrize(
    ("policy", "queries", "reverse", "ordering", "match"),
    [
        # A path-ordered restriction returned out of path order is forged.
        ("substring_file", None, True, "derive", "not in path order"),
        ("literal_file", None, True, "derive", "not in path order"),
        # The ordering is derived from the request policy and cannot be relabeled.
        (
            "keyword_file",
            ["alphaTwo", "alphaThree"],
            False,
            "path_order_constant_score",
            "ordering must be",
        ),
        ("substring_file", None, False, "score_desc_path_tiebreak", "ordering must be"),
        ("literal_file", None, False, "score_desc_path_tiebreak", "ordering must be"),
        # New policies must bind it; only historical literal_file records may omit it.
        ("keyword_file", ["alphaTwo", "alphaThree"], False, None, "ordering must be"),
        ("substring_file", None, False, None, "ordering must be"),
        ("keyword_file", ["alphaTwo", "alphaThree"], False, "unknown_order", "ordering must be"),
    ],
)
def test_file_projection_refuses_forged_or_missing_ordering(
    tmp_path, policy, queries, reverse, ordering, match
):
    repo, suite, run, suite_path, runner_path = _file_projection_run(
        tmp_path, policy, reverse=reverse, ordering=ordering, queries=queries
    )
    with pytest.raises(ev.EvidenceError, match=match):
        record_v3(repo, suite, run, suite_path, runner_path)


def test_file_projection_refuses_wrong_unit_identity_and_chunk_ordering(tmp_path):
    keyword = ["alphaTwo", "alphaThree"]
    repo, suite, run, suite_path, runner_path = _file_projection_run(
        tmp_path / "unit", "keyword_file", queries=keyword
    )
    wrong_unit = copy.deepcopy(run)
    wrong_unit["results"][0]["rank_unit"] = "symbol"
    with pytest.raises(ev.EvidenceError, match="requires lexical distinct_file"):
        record_v3(repo, suite, wrong_unit, suite_path, runner_path)
    wrong_case = copy.deepcopy(run)
    # A literal_file request identity under a keyword profile does not re-derive.
    wrong_case["results"][0]["query_identity"] = qp.derive_query_identity(
        "literal_file", suite["tasks"][0]["query"]
    )
    with pytest.raises(ev.EvidenceError, match="independently re-derived plan"):
        record_v3(repo, suite, wrong_case, suite_path, runner_path)
    duplicate = copy.deepcopy(run)
    extra = copy.deepcopy(duplicate["results"][0]["candidates"][0])
    extra["rank"] = len(duplicate["results"][0]["candidates"]) + 1
    duplicate["results"][0]["candidates"].append(extra)
    with pytest.raises(ev.EvidenceError):
        record_v3(repo, suite, duplicate, suite_path, runner_path)
    forged_empty = copy.deepcopy(run)
    forged_empty["results"][0]["status"] = "abstained"
    with pytest.raises(ev.EvidenceError, match="non-success result cannot contain candidates"):
        record_v3(repo, suite, forged_empty, suite_path, runner_path)
    errored_empty = copy.deepcopy(run)
    errored_empty["results"][0].update(
        status="abstained", candidates=[], error={"code": "x", "message": "y"}
    )
    with pytest.raises(ev.EvidenceError, match="error must be null for abstained"):
        record_v3(repo, suite, errored_empty, suite_path, runner_path)

    repo, suite, run, suite_path, runner_path, _files = fixture_v3(
        tmp_path / "chunk", answerable_only=True
    )
    run["results"][0]["ordering"] = "score_desc_path_tiebreak"
    _pack, run = _repack(repo, suite, run)
    with pytest.raises(ev.EvidenceError, match="ordering applies only to file projections"):
        record_v3(repo, suite, run, suite_path, runner_path)


def test_file_projection_policies_have_independent_python_request_goldens():
    assert qp.plan_lexical_request("keyword_file", "writeContentType") == (
        "select:file case:yes writeContentType"
    )
    assert qp.plan_lexical_request("substring_file", "ContentTy") == (
        "select:file case:yes 'ContentTy'"
    )
    assert ev.digest(qp.policy_config_canonical("keyword_file").encode()) == (
        "595ce53233c77d2b3e31493f5c5baf82bf973a050e28a248e1bddf4279233724"
    )
    assert ev.digest(qp.policy_config_canonical("substring_file").encode()) == (
        "d4b7b04281539571608ca48787e034cc9f035c1264bc4ac8b82729a7b2103ea5"
    )
    assert qp.execution_profile_sha256("keyword_file") == (
        "bc30cff5252dbfd1f0eb09d2825da6a77b3c4bb9a7431c9f60e661aaf8325c13"
    )
    assert qp.execution_profile_sha256("substring_file") == (
        "0bdb593c7b7cb321882500dc1401ae84d893841cb6f1b3ddf4f34e3f0d47ffa5"
    )
    for raw in ["", "1abc", "a b", "a-b", "a:b", "Café", "AND", "OR", "NOT", "a" * 257]:
        with pytest.raises(qp.QueryPlanError):
            qp.plan_lexical_request("keyword_file", raw)
    for raw in ["x) OR (y", "select:repo", '"q"', "a\\b", "Café", "64Sl"]:
        assert qp.plan_lexical_request("substring_file", raw) == f"select:file case:yes '{raw}'"
    for raw in ["ab", "", "a'b", "ab\n", "a\x00b", "é", "x" * 257]:
        with pytest.raises(qp.QueryPlanError):
            qp.plan_lexical_request("substring_file", raw)


def test_unplannable_query_is_a_typed_refusal_not_a_traceback(tmp_path):
    repo, suite, run, suite_path, runner_path = _file_projection_run(tmp_path, "substring_file")
    suite["tasks"][0]["query"] = "ab"
    suite["tasks"][0]["query_sha256"] = ev.digest(b"ab")
    _pack, run = _repack(repo, suite, run)
    with pytest.raises(ev.EvidenceError, match="cannot be planned under substring_file"):
        record_v3(repo, suite, run, suite_path, runner_path)


def test_route_ordering_refuses_a_relabeled_row_directly():
    run = {
        "route_provenance": {"lexical": {"capture_id": "q0"}},
        "captures": {"q0": {"execution_profile": qp.execution_profile("keyword_file")}},
    }
    good = {("T1", "lexical"): {"ordering": "score_desc_path_tiebreak"}}
    assert ev._route_ordering(run, "lexical", good, ["T1"]) == "score_desc_path_tiebreak"
    forged = {("T1", "lexical"): {"ordering": "path_order_constant_score"}}
    with pytest.raises(ev.EvidenceError, match="ordering differs from its policy contract"):
        ev._route_ordering(run, "lexical", forged, ["T1"])
    chunk = {**run, "captures": {"q0": {"execution_profile": qp.execution_profile("native")}}}
    assert ev._route_ordering(chunk, "lexical", good, ["T1"]) == "not_a_file_projection"


def test_keyword_file_native_score_evidence_is_complete_and_ordered(tmp_path):
    repo, suite, run, suite_path, runner_path = _file_projection_run(
        tmp_path, "keyword_file", queries=["alphaTwo", "alphaThree"]
    )
    for row in run["results"]:
        row["score_evidence"] = "native_sdk_score_v1"
        for index, candidate in enumerate(row["candidates"]):
            candidate["score"] = float(len(row["candidates"]) - index)
    _pack, run = _repack(repo, suite, run)
    schema = _load_schema("runner.schema.json")
    jsonschema.validate(run, schema)
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    route = ev.evaluate_diagnostic(loaded_suite, pack, loaded_run)["judgment_metrics"][
        "file_judgments"
    ]["routes"]["lexical"]
    assert route["score_evidence"] == "native_sdk_score_v1"

    def refused(change, match):
        forged = copy.deepcopy(run)
        change(forged["results"][0])
        with pytest.raises(ev.EvidenceError, match=match):
            record_v3(repo, suite, forged, suite_path, runner_path)

    refused(lambda row: row["candidates"][0].pop("score"), "missing native SDK score")
    missing = copy.deepcopy(run)
    missing["results"][0]["candidates"][0].pop("score")
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(missing, schema)
    refused(lambda row: row["candidates"][0].update(score=True), "score must be finite")
    refused(lambda row: row["candidates"][0].update(score=float("inf")), "non-finite JSON")
    refused(
        lambda row: row["candidates"][-1].update(score=100.0),
        "score/path order is invalid",
    )
    refused(lambda row: row.update(score_evidence="unknown"), "score evidence requires")
    refused(lambda row: row.pop("score_evidence"), "missing/unknown fields")
    mixed = copy.deepcopy(run)
    mixed["results"][1].pop("score_evidence")
    for candidate in mixed["results"][1]["candidates"]:
        candidate.pop("score")
    with pytest.raises(ev.EvidenceError, match="mixes score evidence states"):
        record_v3(repo, suite, mixed, suite_path, runner_path)


def test_component_file_policy_rederives_request_and_refuses_noncanonical_input():
    policy = "code_search_components_file"
    request = qp.plan_lexical_request(policy, "clean up")
    assert request == 'components:"clean up"'
    assert qp.execution_profile(policy)["profile_id"] == "quanta-code-search-components-file-v1"
    assert qp.FILE_PROJECTION_ORDERING[policy] == qp.ORDERING_SCORE_DESC
    assert qp.derive_query_identity(policy, "clean up") != qp.derive_query_identity(
        "code_search_file", "clean up"
    )
    assert qp.QUANTA_EVALUATION_POLICIES["explicit_symbol_components"] == frozenset((policy,))
    for raw in ("clean", "clean  up", "Clean up", "clean_up", "clean\tup", "clean up ", "café up"):
        with pytest.raises(qp.QueryPlanError):
            qp.plan_lexical_request(policy, raw)


def test_code_search_file_policy_binds_syntax_scores_and_file_unit(tmp_path):
    assert qp.plan_lexical_request("code_search_file", "writeContentType") == "writeContentType"
    assert qp.plan_lexical_request("code_search_file", "Go To") == "Go To"
    assert qp.plan_lexical_request("code_search_file", "64Sl") == "64Sl"
    assert qp.plan_lexical_request("code_search_file", " ".join(["a"] * 32)) == " ".join(["a"] * 32)
    assert qp.plan_lexical_request("code_search_file", "a" * 256) == "a" * 256
    assert (
        qp.derive_query_identity("code_search_file", "writeContentType")[
            "effective_lexical_request_sha256"
        ]
        == "828e78026cd79b527cc0956b3be52fdf0ebd8b110071f5c309963af3bf719480"
    )
    assert qp.derive_query_identity(
        "code_search_file", "writeContentType"
    ) != qp.derive_query_identity("native", "writeContentType")
    for raw in (
        "",
        "select:file",
        "a-b",
        "Café",
        "x" * 16385,
        "foo\x1cbar",
        "foo\x1fbar",
        " ".join(["a"] * 33),
        "a" * 257,
    ):
        with pytest.raises(qp.QueryPlanError):
            qp.plan_lexical_request("code_search_file", raw)
    assert qp.plan_lexical_request("code_search_file", "foo\tbar\nqux") == "foo\tbar\nqux"

    repo, suite, run, suite_path, runner_path = _file_projection_run(
        tmp_path, "code_search_file", queries=["alphaTwo", "alphaThree"]
    )
    for row in run["results"]:
        row["score_evidence"] = "native_sdk_score_v1"
        for index, candidate in enumerate(row["candidates"]):
            candidate["score"] = float(len(row["candidates"]) - index)
    _pack, run = _repack(repo, suite, run)
    jsonschema.validate(run, _load_schema("runner.schema.json"))
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    route = ev.evaluate_diagnostic(loaded_suite, pack, loaded_run)["judgment_metrics"][
        "file_judgments"
    ]["routes"]["lexical"]
    assert (route["rank_unit"], route["ordering"], route["score_evidence"]) == (
        "distinct_file",
        "score_desc_path_tiebreak",
        "native_sdk_score_v1",
    )

    source_bound = copy.deepcopy(run)
    source_bound["captures"]["q0"].update(
        source_repo_id="bench-repo", source_revision_id="bench-revision"
    )
    jsonschema.validate(source_bound, _load_schema("runner.schema.json"))
    record_v3(repo, suite, source_bound, suite_path, runner_path)
    for target in ("capture", "candidate"):
        forged_pin = copy.deepcopy(source_bound)
        if target == "capture":
            forged_pin["captures"]["q0"]["source_revision_id"] = "other-revision"
        else:
            forged_pin["results"][0]["candidates"][0]["span_accounting"]["source_revision_id"] = (
                "other-revision"
            )
        with pytest.raises(ev.EvidenceError, match="source pin differs from capture"):
            record_v3(repo, suite, forged_pin, suite_path, runner_path)

    forged = copy.deepcopy(run)
    forged["results"][0]["query_identity"] = qp.derive_query_identity(
        "native", suite["tasks"][0]["query"]
    )
    with pytest.raises(ev.EvidenceError, match="independently re-derived plan"):
        record_v3(repo, suite, forged, suite_path, runner_path)
    missing = copy.deepcopy(run)
    missing["results"][0].pop("score_evidence")
    for candidate in missing["results"][0]["candidates"]:
        candidate.pop("score")
    with pytest.raises(ev.EvidenceError, match="requires native SDK score evidence"):
        record_v3(repo, suite, missing, suite_path, runner_path)
    duplicate = copy.deepcopy(run)
    extra = copy.deepcopy(duplicate["results"][0]["candidates"][0])
    extra["rank"] = len(duplicate["results"][0]["candidates"]) + 1
    extra["score"] = 0.0
    duplicate["results"][0]["candidates"].append(extra)
    with pytest.raises(ev.EvidenceError, match="duplicate published unit ID"):
        record_v3(repo, suite, duplicate, suite_path, runner_path)
    no_proof = copy.deepcopy(run)
    no_proof.pop("span_accounting_version")
    with pytest.raises(ev.EvidenceError, match="requires source-bound file span evidence"):
        record_v3(repo, suite, no_proof, suite_path, runner_path)
    forged_unit = copy.deepcopy(run)
    forged_unit["results"][0]["candidates"][0]["span_accounting"]["unit_kind"] = "chunk"
    with pytest.raises(ev.EvidenceError):
        record_v3(repo, suite, forged_unit, suite_path, runner_path)


@pytest.mark.parametrize(
    ("policy", "mode"),
    [
        ("code_search_file", "default_file_search"),
        ("code_search_typo_file", "explicit_osa1_typo"),
        ("code_search_components_file", "explicit_symbol_components"),
    ],
)
def test_evaluation_contract_binds_file_request_gold_result_and_mrr(tmp_path, policy, mode):
    queries = (
        ["alpha two", "alpha three"]
        if policy == "code_search_components_file"
        else ["alphaTwo", "alphaThree"]
    )
    repo, suite, run, suite_path, runner_path = _file_projection_run(
        tmp_path, policy, reverse=True, queries=queries
    )
    for task in suite["tasks"]:
        task["query_intent"] = (
            "symbol_components" if mode == "explicit_symbol_components" else "bare_symbol"
        )
        task["evaluation_contract"] = {
            "request_mode": mode,
            "gold_unit": "distinct_file",
            "result_unit": "distinct_file",
        }
        task["file_judgments"][1]["grade"] = 0
    for row in run["results"]:
        row["score_evidence"] = "native_sdk_score_v1"
        for index, candidate in enumerate(row["candidates"]):
            candidate["score"] = float(len(row["candidates"]) - index)
    _pack, run = _repack(repo, suite, run)
    jsonschema.validate(suite, _load_schema("suite.schema.json"))
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    diagnostic = ev.evaluate_diagnostic(loaded_suite, pack, loaded_run)
    assert diagnostic["evaluation_contract"] == suite["tasks"][0]["evaluation_contract"]
    scores = diagnostic["judgment_metrics"]["file_judgments"]["per_query"]
    assert scores[0]["scores"]["mrr_at_10"] == 0.5
    assert scores[0]["scores"]["hit_at_10"] == 1.0
    assert scores[0]["scores"]["ndcg_at_10"] == pytest.approx(1 / math.log2(3))
    with pytest.raises(ev.EvidenceError, match="context metrics are undefined"):
        ev.evaluate(loaded_suite, pack, loaded_run, "lexical", "hybrid")

    wrong_policy = copy.deepcopy(suite)
    alternate = "explicit_osa1_typo" if mode == "default_file_search" else "default_file_search"
    for task in wrong_policy["tasks"]:
        task["evaluation_contract"]["request_mode"] = alternate
    _pack, wrong_run = _repack(repo, wrong_policy, run)
    with pytest.raises(ev.EvidenceError, match="request mode differs from bound product policy"):
        record_v3(repo, wrong_policy, wrong_run, suite_path, runner_path)


def test_default_file_contract_accepts_bound_semble_pair(tmp_path):
    repo, suite, run, suite_path, runner_path = _file_projection_run(
        tmp_path, "code_search_file", queries=["alphaTwo", "alphaThree"]
    )
    suite["routes"] = ["lexical", "semble-lexical-file"]
    for task in suite["tasks"]:
        task["query_intent"] = "bare_symbol"
        task["evaluation_contract"] = {
            "request_mode": "default_file_search",
            "gold_unit": "distinct_file",
            "result_unit": "distinct_file",
        }
    semble_capture = _v3_capture("semble", current=True)
    semble_profile = semble_adapter.execution_profile("lexical-file", None)
    semble_capture["execution_profile"] = semble_profile
    semble_capture["execution_profile_sha256"] = ev.digest(ev.canonical(semble_profile))
    run["captures"]["s0"] = semble_capture
    run["route_provenance"]["semble-lexical-file"] = {"capture_id": "s0"}
    for row in list(run["results"]):
        row["score_evidence"] = "native_sdk_score_v1"
        for rank, candidate in enumerate(row["candidates"]):
            candidate["score"] = float(len(row["candidates"]) - rank)
        baseline = copy.deepcopy(row)
        baseline["route"] = "semble-lexical-file"
        query_sha = next(
            task["query_sha256"] for task in suite["tasks"] if task["task_id"] == row["task_id"]
        )
        baseline["query_identity"] = {
            "original_query_sha256": query_sha,
            "submitted_query_sha256": query_sha,
        }
        baseline["ordering"] = "score_desc_native_tiebreak"
        baseline["score_evidence"] = "semble_bm25_score_v1"
        baseline["file_collection"] = {
            "indexed_chunks": 10,
            "matched_chunks": len(baseline["candidates"]),
            "matching_files": len(baseline["candidates"]),
        }
        for rank, candidate in enumerate(baseline["candidates"]):
            candidate.pop("span_accounting", None)
            candidate["tokens"] = len(ev.TOKEN_RE.findall((repo / candidate["path"]).read_text()))
            candidate["score"] = float(len(baseline["candidates"]) - rank)
        run["results"].append(baseline)
    _pack, run = _repack(repo, suite, run)
    record_v3(repo, suite, run, suite_path, runner_path)

    explicit = copy.deepcopy(suite)
    for task in explicit["tasks"]:
        task["evaluation_contract"]["request_mode"] = "explicit_osa1_typo"
    _pack, explicit_run = _repack(repo, explicit, run)
    with pytest.raises(ev.EvidenceError, match="explicit_osa1_typo has an unsupported route"):
        record_v3(repo, explicit, explicit_run, suite_path, runner_path)


def test_evaluation_contract_rejects_partial_mixed_and_wrong_units(tmp_path):
    repo, suite, _run, _suite_path, _runner_path = _file_projection_run(
        tmp_path, "code_search_file", queries=["alphaTwo", "alphaThree"]
    )
    contract = {
        "request_mode": "default_file_search",
        "gold_unit": "distinct_file",
        "result_unit": "distinct_file",
    }
    for task in suite["tasks"]:
        task["query_intent"] = "bare_symbol"
        task["evaluation_contract"] = copy.deepcopy(contract)
    schema = _load_schema("suite.schema.json")
    jsonschema.validate(suite, schema)
    ev.validate_suite(repo, suite)
    for field, value, match in (
        ("gold_unit", "symbol", "unit mismatch"),
        ("result_unit", "symbol", "unit mismatch"),
        ("request_mode", "declaration_navigation", "unit mismatch"),
    ):
        changed = copy.deepcopy(suite)
        changed["tasks"][0]["evaluation_contract"][field] = value
        with pytest.raises(jsonschema.ValidationError):
            jsonschema.validate(changed, schema)
        with pytest.raises(ev.EvidenceError, match=match):
            ev.validate_suite(repo, changed)
    partial = copy.deepcopy(suite)
    partial["tasks"][1].pop("evaluation_contract")
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(partial, schema)
    with pytest.raises(ev.EvidenceError, match="partial evaluation_contract coverage"):
        ev.validate_suite(repo, partial)
    mixed = copy.deepcopy(suite)
    mixed["tasks"][1]["evaluation_contract"]["request_mode"] = "explicit_osa1_typo"
    jsonschema.validate(mixed, schema)
    with pytest.raises(ev.EvidenceError, match="mixed request modes"):
        ev.validate_suite(repo, mixed)


def test_intended_name_source_oracle_uses_base_declaration_not_noisy_query(tmp_path):
    files = {
        "a.go": b"package demo\ntype Param struct{}\n",
        "b.go": b"package demo\nfunc Next() {}\n",
    }
    repo, commit = _write_repo(tmp_path, files)
    universe = [
        {"path": path, "file_sha256": ev.digest(raw)} for path, raw in sorted(files.items())
    ]
    task = {
        "task_id": "T1",
        "split": "eval",
        "query": "Pram",
        "query_sha256": ev.digest(b"Pram"),
        "query_family_id": "Param",
        "query_intent": "bare_symbol",
        "intended_name": "Param",
        "evaluation_contract": {
            "request_mode": "explicit_osa1_typo",
            "gold_unit": "distinct_file",
            "result_unit": "distinct_file",
        },
        "answerable": True,
        "gold": [_v3_block(files, "a.go", 2, 2, grade=3)],
        "judgment_policy": ev.SOURCE_ORACLE_JUDGMENT_POLICY,
        "source_oracle": {"contract": "go_exact_local_name_v3", "unit": "distinct_file"},
        "file_judgments": [{"path": "a.go", "file_sha256": ev.digest(files["a.go"]), "grade": 3}],
    }
    suite = {
        "schema_version": 3,
        "suite_id": "intended-name-fixture",
        "repository_commit": commit,
        "comparison_contract": _v3_contract(),
        "routes": ["lexical"],
        "file_universe": universe,
        "file_universe_digest": ev.universe_digest(universe),
        "diagnostic_policy": ev.OBSERVED_PREFIX_DIAGNOSTIC_POLICY,
        "tasks": [task],
    }
    schema = _load_schema("suite.schema.json")
    jsonschema.validate(suite, schema)
    _, pack, _ = ev.validate_suite(repo, suite)
    assert "Pram" in str(pack) and "Param" not in str(pack)
    for field, value, error in (
        ("intended_name", "Next", "one casefold OSA edit"),
        ("query", "Param", "one casefold OSA edit"),
    ):
        bad = copy.deepcopy(suite)
        bad["tasks"][0][field] = value
        if field == "query":
            bad["tasks"][0]["query_sha256"] = ev.digest(value.encode())
        with pytest.raises(ev.EvidenceError, match=error):
            ev.validate_suite(repo, bad)
    bad_gold = copy.deepcopy(suite)
    bad_gold["tasks"][0]["file_judgments"][0]["path"] = "b.go"
    bad_gold["tasks"][0]["file_judgments"][0]["file_sha256"] = ev.digest(files["b.go"])
    with pytest.raises(ev.EvidenceError, match="source oracle judgments differ"):
        ev.validate_suite(repo, bad_gold)
    collision = copy.deepcopy(suite)
    collision["tasks"][0]["query"] = "Next"
    collision["tasks"][0]["query_sha256"] = ev.digest(b"Next")
    with pytest.raises(ev.EvidenceError, match="one casefold OSA edit"):
        ev.validate_suite(repo, collision)
    collision_files = {**files, "c.go": b"package demo\n// Pram typo in content.\n"}
    collision_repo, collision_commit = _write_repo(tmp_path / "collision", collision_files)
    collision_suite = copy.deepcopy(suite)
    collision_suite["repository_commit"] = collision_commit
    collision_suite["file_universe"] = [
        {"path": path, "file_sha256": ev.digest(raw)}
        for path, raw in sorted(collision_files.items())
    ]
    collision_suite["file_universe_digest"] = ev.universe_digest(collision_suite["file_universe"])
    with pytest.raises(ev.EvidenceError, match="collides with a source identifier"):
        ev.validate_suite(collision_repo, collision_suite)


def test_intended_name_near_exclusion_replays_parser_refusal(tmp_path):
    files = {
        "main.go": b"package demo\nfunc Param() {}\n",
        "broken.go": b"package demo\nfunc Broken(",
    }
    repo, commit = _write_repo(tmp_path, files)
    universe = [
        {"path": path, "file_sha256": ev.digest(raw)} for path, raw in sorted(files.items())
    ]
    suite = {
        "schema_version": 3,
        "suite_id": "intended-name-refused-file",
        "repository_commit": commit,
        "comparison_contract": _v3_contract(),
        "routes": ["lexical"],
        "file_universe": universe,
        "file_universe_digest": ev.universe_digest(universe),
        "diagnostic_policy": ev.OBSERVED_PREFIX_DIAGNOSTIC_POLICY,
        "tasks": [
            {
                "task_id": "T1",
                "split": "eval",
                "query": "Paran",
                "query_sha256": ev.digest(b"Paran"),
                "query_family_id": "Param",
                "query_intent": "bare_symbol",
                "intended_name": "Param",
                "evaluation_contract": {
                    "request_mode": "explicit_osa1_typo",
                    "gold_unit": "distinct_file",
                    "result_unit": "distinct_file",
                },
                "answerable": True,
                "gold": [_v3_block(files, "main.go", 2, 2, grade=3)],
                "judgment_policy": ev.SOURCE_ORACLE_JUDGMENT_POLICY,
                "source_oracle": {
                    "contract": "go_exact_local_name_v3",
                    "unit": "distinct_file",
                    "declaration_exclusions": ["broken.go"],
                    "near_declaration_exclusions": ["broken.go"],
                },
                "file_judgments": [
                    {"path": "main.go", "file_sha256": ev.digest(files["main.go"]), "grade": 3}
                ],
            }
        ],
    }
    jsonschema.validate(suite, _load_schema("suite.schema.json"))
    _loaded, pack, _source = ev.validate_suite(repo, suite)
    assert "Paran" in str(pack) and "Param" not in str(pack)
    for field, error in (
        ("declaration_exclusions", "explicit query eligibility"),
        ("near_declaration_exclusions", "explicit query eligibility"),
    ):
        bad = copy.deepcopy(suite)
        bad["tasks"][0]["source_oracle"].pop(field)
        with pytest.raises(ev.EvidenceError, match=error):
            ev.validate_suite(repo, bad)
    bad = copy.deepcopy(suite)
    bad["tasks"][0]["source_oracle"]["near_declaration_exclusions"] = ["main.go"]
    with pytest.raises(ev.EvidenceError, match="may contain a query match"):
        ev.validate_suite(repo, bad)

    second = copy.deepcopy(suite["tasks"][0])
    second["task_id"] = "T2"
    second["query"] = "Parax"
    second["query_sha256"] = ev.digest(b"Parax")
    repeated_target = copy.deepcopy(suite)
    repeated_target["tasks"].append(second)
    _loaded, projected_pack, _source = ev.validate_suite(repo, repeated_target)
    assert [task["task_id"] for task in projected_pack["tasks"]] == ["T1", "T2"]

    conflicting = copy.deepcopy(repeated_target)
    conflicting["tasks"][1]["source_oracle"]["declaration_exclusions"] = ["main.go"]
    with pytest.raises(ev.EvidenceError, match="conflicting declaration exclusion query"):
        ev.validate_suite(repo, conflicting)


def test_declaration_navigation_contract_requires_symbol_policy(tmp_path):
    repo, suite, run, suite_path, runner_path, _files = _exact_symbol_record_fixture(tmp_path)
    suite["diagnostic_policy"] = ev.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
    for task, row in zip(suite["tasks"], run["results"], strict=True):
        task["query_intent"] = "bare_symbol"
        task["evaluation_contract"] = {
            "request_mode": "declaration_navigation",
            "gold_unit": "symbol",
            "result_unit": "symbol",
        }
        task["judgment_policy"] = ev.UNJUDGED_POLICY
        task["label_review"] = {
            "assessment": "reviewed_unambiguous",
            "reviewer_id": "fixture-reviewer",
            "evidence_sha256": ev.digest(b"declaration fixture"),
        }
        task["declaration_judgments"] = [
            {
                "path": item["path"],
                "file_sha256": item["file_sha256"],
                "start_byte": item["start_byte"],
                "end_byte": item["end_byte"],
                "grade": 3,
            }
            for item in row["candidates"]
        ]
        row["rank_unit"] = "symbol"
    _pack, run = _repack(repo, suite, run)
    jsonschema.validate(suite, _load_schema("suite.schema.json"))
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    assert (
        ev.evaluate_diagnostic(loaded_suite, pack, loaded_run)["evaluation_contract"]["result_unit"]
        == "symbol"
    )
    wrong = copy.deepcopy(run)
    wrong["results"][0]["rank_unit"] = "distinct_file"
    with pytest.raises(ev.EvidenceError, match="result_unit mismatch"):
        record_v3(repo, suite, wrong, suite_path, runner_path)


def test_exact_content_file_policy_binds_request_unit_score_and_source(tmp_path):
    policy = "code_search_exact_content_file"
    raw = 'say("can\'t\\skip")'
    request = qp.plan_lexical_request(policy, raw)
    assert request == 'content:"say(\\"can\'t\\\\skip\\")" case:yes'
    assert qp.execution_profile(policy)["profile_id"] == (
        "quanta-code-search-exact-content-file-v1"
    )
    assert qp.derive_query_identity(policy, raw)["effective_lexical_request_sha256"] == (
        qp.code_search_effective_request_sha256(request)
    )
    assert qp.derive_query_identity(policy, raw) != qp.derive_query_identity(
        "code_search_file", "alphaTwo"
    )
    for invalid in ("", "e\u0301", "line\nbreak", "x" * 257):
        with pytest.raises(qp.QueryPlanError):
            qp.plan_lexical_request(policy, invalid)

    repo, suite, run, suite_path, runner_path = _file_projection_run(
        tmp_path, policy, queries=[raw, "content:case:yes"]
    )
    for row in run["results"]:
        row["score_evidence"] = "native_sdk_score_v1"
        for index, candidate in enumerate(row["candidates"]):
            candidate["score"] = float(len(row["candidates"]) - index)
    _pack, run = _repack(repo, suite, run)
    jsonschema.validate(run, _load_schema("runner.schema.json"))
    missing_pin = copy.deepcopy(run)
    del missing_pin["captures"]["q0"]["source_revision_id"]
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(missing_pin, _load_schema("runner.schema.json"))
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    route = ev.evaluate_diagnostic(loaded_suite, pack, loaded_run)["judgment_metrics"][
        "file_judgments"
    ]["routes"]["lexical"]
    assert (route["rank_unit"], route["ordering"], route["score_evidence"]) == (
        "distinct_file",
        "score_desc_path_tiebreak",
        "native_sdk_score_v1",
    )

    forged = copy.deepcopy(run)
    forged["results"][0]["query_identity"] = qp.derive_query_identity(
        "code_search_file", "alphaTwo"
    )
    with pytest.raises(ev.EvidenceError, match="independently re-derived plan"):
        record_v3(repo, suite, forged, suite_path, runner_path)
    forged = copy.deepcopy(run)
    forged["results"][0]["rank_unit"] = "symbol"
    with pytest.raises(ev.EvidenceError, match="requires lexical distinct_file"):
        record_v3(repo, suite, forged, suite_path, runner_path)
    forged = copy.deepcopy(run)
    forged["results"][0]["candidates"][-1]["score"] = 100.0
    with pytest.raises(ev.EvidenceError, match="score/path order is invalid"):
        record_v3(repo, suite, forged, suite_path, runner_path)
    forged = copy.deepcopy(run)
    forged["results"][0]["candidates"][0]["span_accounting"]["source_repo_id"] = "wrong"
    with pytest.raises(ev.EvidenceError, match="file identity digest mismatch"):
        record_v3(repo, suite, forged, suite_path, runner_path)
    forged = copy.deepcopy(run)
    forged["results"][0]["candidates"][0]["span_accounting"]["source_revision_id"] = "wrong"
    with pytest.raises(ev.EvidenceError, match="source pin"):
        record_v3(repo, suite, forged, suite_path, runner_path)
    forged = copy.deepcopy(run)
    forged["captures"]["q0"]["source_revision_id"] = "wrong"
    with pytest.raises(ev.EvidenceError, match="frozen repository"):
        record_v3(repo, suite, forged, suite_path, runner_path)


def test_code_search_typo_file_policy_binds_distinct_request_and_file_unit(tmp_path):
    assert qp.plan_lexical_request("code_search_typo_file", "ljs") == "typo:ljs"
    assert qp.plan_lexical_request("code_search_typo_file", "load_jsom") == "typo:load_jsom"
    assert qp.execution_profile("code_search_typo_file")["profile_id"] == (
        "quanta-code-search-typo-file-v1"
    )
    assert ev.digest(qp.policy_config_canonical("code_search_typo_file").encode()) == (
        "652b78bd66c84f7019f496b80660b14ab8c60b7a24ff13545f603980e1315cae"
    )
    assert qp.execution_profile_sha256("code_search_typo_file") == (
        "2d6247a92e7e6693d3059ab191a5b0ef2fcb93a484c1099fa1c8bed9e3dc1066"
    )
    assert qp.execution_profile_sha256("code_search_file") == (
        "e39867e466be2ab7f4c5cb779f1fad338a280f5d6669a97c8ed8552486d5ff61"
    )
    assert (
        qp.derive_query_identity("code_search_typo_file", "load_jsom")[
            "effective_lexical_request_sha256"
        ]
        == "4a9305acacf83f7e71ac1af4e65d17ddcddd04499743146f34eb29d1c309965c"
    )
    assert qp.derive_query_identity(
        "code_search_typo_file", "load_jsom"
    ) != qp.derive_query_identity("code_search_file", "load_jsom")
    assert (
        qp.derive_query_identity("code_search_file", "writeContentType")[
            "effective_lexical_request_sha256"
        ]
        == "828e78026cd79b527cc0956b3be52fdf0ebd8b110071f5c309963af3bf719480"
    )
    for raw in (
        "",
        "ab",
        "1number",
        "load-jsom",
        "typo:load_jsom",
        "load_jsom extra",
        "Cafés",
        "a" * 65,
    ):
        with pytest.raises(qp.QueryPlanError):
            qp.plan_lexical_request("code_search_typo_file", raw)

    repo, suite, run, suite_path, runner_path = _file_projection_run(
        tmp_path, "code_search_typo_file", queries=["alphaTwp", "alphaThre"]
    )
    for row in run["results"]:
        row["score_evidence"] = "native_sdk_score_v1"
        for index, candidate in enumerate(row["candidates"]):
            candidate["score"] = float(len(row["candidates"]) - index)
    _pack, run = _repack(repo, suite, run)
    jsonschema.validate(run, _load_schema("runner.schema.json"))
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    route = ev.evaluate_diagnostic(loaded_suite, pack, loaded_run)["judgment_metrics"][
        "file_judgments"
    ]["routes"]["lexical"]
    assert (route["rank_unit"], route["ordering"], route["score_evidence"]) == (
        "distinct_file",
        "score_desc_path_tiebreak",
        "native_sdk_score_v1",
    )
    forged = copy.deepcopy(run)
    forged["results"][0]["query_identity"] = qp.derive_query_identity(
        "code_search_file", suite["tasks"][0]["query"]
    )
    with pytest.raises(ev.EvidenceError, match="independently re-derived plan"):
        record_v3(repo, suite, forged, suite_path, runner_path)


def test_code_search_typo_file_reports_answerable_and_no_answer_separately(tmp_path):
    repo, suite, run, suite_path, runner_path = _file_projection_run(
        tmp_path, "code_search_typo_file", queries=["alphaTwp", "alphaThre"]
    )
    no_answer_task = suite["tasks"][1]
    no_answer_task["answerable"] = False
    no_answer_task["gold"] = []
    no_answer_task["file_judgments"] = []
    for row in run["results"]:
        row["score_evidence"] = "native_sdk_score_v1"
        for index, candidate in enumerate(row["candidates"]):
            candidate["score"] = float(len(row["candidates"]) - index)
    _pack, run = _repack(repo, suite, run)
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    diagnostic = ev.evaluate_diagnostic(loaded_suite, pack, loaded_run)
    assert (
        diagnostic["judgment_metrics"]["file_judgments"]["routes"]["lexical"][
            "selected_answerable_tasks"
        ]
        == 1
    )
    assert diagnostic["no_answer"]["sample_count"] == 1
    assert diagnostic["no_answer"]["nonempty_results"] == 1
    assert (
        diagnostic["no_answer"]["reference_contracts"][0]["source_oracle_contract"]
        == "not_declared"
    )


def test_empty_file_candidate_is_source_bound_and_replayable():
    repo_id = b"bench-repo"
    path = b"empty.go"
    framed = (
        b"quanta-index:code-search-file:v1\x00"
        + len(repo_id).to_bytes(8, "little")
        + repo_id
        + len(path).to_bytes(8, "little")
        + path
    )
    empty_sha = hashlib.sha256(b"").hexdigest()
    row = {
        "path": path.decode(),
        "start_byte": 0,
        "end_byte": 0,
        "start_line": 0,
        "end_line": 0,
        "file_sha256": empty_sha,
        "block_sha256": empty_sha,
        "tokens": 3,
        "rank": 1,
        "score": 2.0,
        "span_accounting": {
            "unit_kind": "file",
            "unit_id": "file:" + hashlib.sha256(framed).hexdigest(),
            "producer_identity": "code-search-file-v1",
            "indexed_start_byte": 0,
            "indexed_end_byte": 0,
            "sdk_start_line": 0,
            "sdk_end_line": 0,
            "extra_context_bytes": 0,
            "source_repo_id": repo_id.decode(),
            "source_revision_id": "bench-revision",
            "preview_kind": "path",
            "preview_start_byte": None,
            "preview_end_byte": None,
            "snippet_sha256": hashlib.sha256(path).hexdigest(),
        },
    }

    class FrozenSource:
        def file(self, requested):
            assert requested == path.decode()
            return b"", [], empty_sha

    schema = _load_schema("runner.schema.json")
    jsonschema.Draft202012Validator(schema["$defs"]["candidate"]).validate(row)
    assert (
        ev.block(
            FrozenSource(),
            row,
            "empty file",
            candidate=True,
            universe={path.decode()},
            allow_span_accounting=True,
            allow_score=True,
        )
        == row
    )
    for change in (
        {"end_byte": 1},
        {"tokens": 0},
        {"start_line": 1},
        {"span_accounting": {**row["span_accounting"], "snippet_sha256": empty_sha}},
        {"span_accounting": {**row["span_accounting"], "preview_start_byte": 0}},
        {"span_accounting": {**row["span_accounting"], "unit_id": "file:" + "0" * 64}},
    ):
        forged = {**row, **change}
        with pytest.raises(ev.EvidenceError):
            ev.block(
                FrozenSource(),
                forged,
                "empty file",
                candidate=True,
                universe={path.decode()},
                allow_span_accounting=True,
                allow_score=True,
            )


def test_code_search_file_refuses_context_metric_from_full_file_identity():
    for policy in ("code_search_file", "natural_language_file"):
        run = {"captures": {"q0": {"execution_profile": qp.execution_profile(policy)}}}
        with pytest.raises(ev.EvidenceError, match="context metrics are undefined"):
            ev.evaluate({}, {}, run, "lexical", "semantic")


@pytest.mark.parametrize(
    ("policy", "queries"),
    [
        ("code_search_file", ["alphaTwo", "alphaThree"]),
        ("code_search_typo_file", ["alphaTwp", "alphaThre"]),
        ("natural_language_file", ["find alphaTwo", "find alphaThree"]),
    ],
)
def test_code_search_file_pair_reports_only_independent_file_judgments(tmp_path, policy, queries):
    repo, suite, run, _suite_path, _runner_path = _file_projection_run(
        tmp_path, policy, queries=queries
    )
    suite["routes"] = ["lexical", "semble-lexical-file"]
    run["captures"]["s0"] = {
        "system": "semble",
        "execution_profile": semble_adapter.execution_profile("lexical-file", None),
    }
    run["route_provenance"]["semble-lexical-file"] = {"capture_id": "s0"}
    for row in list(run["results"]):
        row["score_evidence"] = "native_sdk_score_v1"
        for rank, candidate in enumerate(row["candidates"]):
            candidate["score"] = float(len(row["candidates"]) - rank)
        baseline_row = copy.deepcopy(row)
        baseline_row["route"] = "semble-lexical-file"
        baseline_row["score_evidence"] = "semble_bm25_score_v1"
        baseline_row["ordering"] = "score_desc_native_tiebreak"
        baseline_row["file_collection"] = {"observed": True}
        run["results"].append(baseline_row)
    report = ev.evaluate_paired_file_diagnostic(suite, {}, run, "semble-lexical-file", "lexical")
    assert report["status"] == "diagnostic_unqualified"
    assert report["report_scope"] == "paired_independent_file_judgment_diagnostic_v1"
    assert report["reference_contracts"] == [
        {
            "source_oracle_contract": "not_declared",
            "gold_unit": "not_declared",
            "query_intent": "not_declared",
        }
    ]
    assert report["judgment_metrics"]["file_judgments"]["comparison"]["sample_count"] == 2
    assert report["no_answer"]["routes"]["lexical"]["sample_count"] == 0
    replayed = pairrun.replay_paired_file_diagnostic_report(
        suite, {}, run, report, "whole_file", "a" * 64
    )
    assert replayed["primary_metric"] == "diagnostic_file_ndcg_at_10"
    assert replayed["record_digest"] == ev.digest(ev.canonical(run))
    forged_report = copy.deepcopy(report)
    forged_report["judgment_metrics"]["file_judgments"]["comparison"]["sample_count"] = 3
    with pytest.raises(pairrun.RunError, match="differs from independent replay"):
        pairrun.replay_paired_file_diagnostic_report(
            suite, {}, run, forged_report, "whole_file", "a" * 64
        )
    forged_report = copy.deepcopy(report)
    forged_report["no_answer"]["routes"]["lexical"]["nonempty_results"] = 1
    with pytest.raises(pairrun.RunError, match="differs from independent replay"):
        pairrun.replay_paired_file_diagnostic_report(
            suite, {}, run, forged_report, "whole_file", "a" * 64
        )
    with pytest.raises(ev.EvidenceError, match="context metrics are undefined"):
        ev.evaluate(suite, {}, run, "semble-lexical-file", "lexical")
    with pytest.raises(ev.EvidenceError, match="authoritative labels"):
        ev.complete_scored_file_rows(suite, {}, run, "semble-lexical-file", "lexical")
    for task in suite["tasks"]:
        task["judgment_policy"] = ev.COMPLETE_JUDGMENT_POLICY
    complete_rows = ev.complete_scored_file_rows(suite, {}, run, "semble-lexical-file", "lexical")
    assert [task_id for task_id, _before, _after in complete_rows] == ["T1", "T2"]
    unjudged = copy.deepcopy(suite)
    for task in unjudged["tasks"]:
        task["file_judgments"] = task["file_judgments"][:1]
    with pytest.raises(ev.EvidenceError, match="ordered, judged"):
        ev.complete_scored_file_rows(unjudged, {}, run, "semble-lexical-file", "lexical")
    unscored = copy.deepcopy(run)
    for row in unscored["results"]:
        if row["route"] == "lexical":
            row["score_evidence"] = None
    with pytest.raises(ev.EvidenceError, match="ordered, judged"):
        ev.complete_scored_file_rows(suite, {}, unscored, "semble-lexical-file", "lexical")
    absent_query = "missingzz"
    suite["tasks"].append(
        {
            "task_id": "T3",
            "split": "eval",
            "query": absent_query,
            "query_sha256": ev.digest(absent_query.encode()),
            "query_family_id": "fam-missingzz",
            "query_intent": "bare_symbol",
            "answerable": False,
            "category": "no_answer",
            "gold": [],
            "judgment_policy": ev.SOURCE_ORACLE_JUDGMENT_POLICY,
            "source_oracle": {
                "contract": "ascii_content_absent_casefold_v1",
                "unit": "distinct_file",
            },
            "file_judgments": [],
        }
    )
    for route in ("lexical", "semble-lexical-file"):
        absent_row = copy.deepcopy(next(row for row in run["results"] if row["route"] == route))
        absent_row.update(task_id="T3", status="abstained", candidates=[])
        run["results"].append(absent_row)
    _validated_suite, pack, _source = ev.validate_suite(repo, suite)
    file_evidence = ev.evaluate_complete_scored_file_evidence(
        suite, pack, run, "semble-lexical-file", "lexical"
    )
    assert file_evidence["status"] == "evidence_unqualified"
    assert file_evidence["rank_metric_version"] == "file-judgments-complete-v1"
    assert file_evidence["rank_metrics"]["comparison"]["sample_count"] == 2
    assert (
        file_evidence["rank_metrics"]["comparison"]["no_answer_abstention_delta"]["sample_count"]
        == 1
    )
    assert (
        file_evidence["rank_metrics"]["comparison"]["no_answer_abstention_delta"]["mean_delta"]
        == 0.0
    )
    assert (
        len(ev.paired_query_family_rows(suite, file_evidence, "semble-lexical-file", "lexical"))
        == 2
    )
    file_replay = pairrun.replay_complete_scored_file_report(
        suite, pack, run, file_evidence, "whole_file", "a" * 64
    )
    assert file_replay["primary_metric"] == "file_ndcg_at_10"
    assert file_replay["sample_count"] == 2
    assert not pairrun._qualified_uncertainty(file_replay)
    assert not pairrun._qualified_cluster_uncertainty(file_replay)
    sufficiently_sampled_suite = copy.deepcopy(suite)
    sufficiently_sampled_run = copy.deepcopy(run)
    for index in range(4, 22):
        source_id = "T1" if index % 2 == 0 else "T2"
        source_task = next(task for task in suite["tasks"] if task["task_id"] == source_id)
        task = copy.deepcopy(source_task)
        task["task_id"] = f"T{index}"
        task["query"] = "v" + ev.digest(f"file-gate-variant-{index}".encode())[:20]
        task["query_sha256"] = ev.digest(task["query"].encode())
        task["query_family_id"] = f"file-family-{index}"
        sufficiently_sampled_suite["tasks"].append(task)
        for source_row in run["results"]:
            if source_row["task_id"] == source_id:
                result = copy.deepcopy(source_row)
                result["task_id"] = task["task_id"]
                if result["route"] == "lexical":
                    result["query_identity"] = qp.derive_query_identity(policy, task["query"])
                else:
                    result["query_identity"] = {
                        "original_query_sha256": task["query_sha256"],
                        "submitted_query_sha256": task["query_sha256"],
                    }
                sufficiently_sampled_run["results"].append(result)
    _validated_suite, sufficiently_sampled_pack, _source = ev.validate_suite(
        repo, sufficiently_sampled_suite
    )
    sufficiently_sampled_report = ev.evaluate_complete_scored_file_evidence(
        sufficiently_sampled_suite,
        sufficiently_sampled_pack,
        sufficiently_sampled_run,
        "semble-lexical-file",
        "lexical",
    )
    sufficiently_sampled_replay = pairrun.replay_complete_scored_file_report(
        sufficiently_sampled_suite,
        sufficiently_sampled_pack,
        sufficiently_sampled_run,
        sufficiently_sampled_report,
        "whole_file",
        "a" * 64,
    )
    assert sufficiently_sampled_replay["sample_count"] == 20
    assert pairrun._qualified_uncertainty(sufficiently_sampled_replay)
    assert pairrun._qualified_cluster_uncertainty(sufficiently_sampled_replay)
    forged_file_evidence = copy.deepcopy(file_evidence)
    forged_file_evidence["per_query"][0]["file_ndcg_at_10"] = 0.0
    with pytest.raises(pairrun.RunError, match="differs from independent replay"):
        pairrun.replay_complete_scored_file_report(
            suite, pack, run, forged_file_evidence, "whole_file", "a" * 64
        )
    reversed_baseline = copy.deepcopy(run)
    baseline_t1 = next(
        row
        for row in reversed_baseline["results"]
        if row["task_id"] == "T1" and row["route"] == "semble-lexical-file"
    )
    baseline_t1["candidates"].reverse()
    for rank, item in enumerate(baseline_t1["candidates"], 1):
        item["rank"] = rank
        item["score"] = float(3 - rank)
    reordered_evidence = ev.evaluate_complete_scored_file_evidence(
        suite, pack, reversed_baseline, "semble-lexical-file", "lexical"
    )
    baseline_score = next(
        row["file_ndcg_at_10"]
        for row in reordered_evidence["per_query"]
        if row["task_id"] == "T1" and row["route"] == "semble-lexical-file"
    )
    assert baseline_score == pytest.approx((1 + 7 / math.log2(3)) / (7 + 1 / math.log2(3)))
    failed_negative = copy.deepcopy(run)
    next(
        row
        for row in failed_negative["results"]
        if row["task_id"] == "T3" and row["route"] == "lexical"
    )["status"] = "error"
    with pytest.raises(ev.EvidenceError, match="failed no-answer observations"):
        ev.evaluate_complete_scored_file_evidence(
            suite, pack, failed_negative, "semble-lexical-file", "lexical"
        )
    without_negative = copy.deepcopy(suite)
    without_negative["tasks"].pop()
    with pytest.raises(ev.EvidenceError, match="needs no-answer controls"):
        ev.evaluate_complete_scored_file_evidence(
            without_negative, pack, run, "semble-lexical-file", "lexical"
        )
    run["results"][0]["rank_unit"] = "symbol"
    with pytest.raises(ev.EvidenceError, match="distinct-file results"):
        ev.evaluate_paired_file_diagnostic(suite, {}, run, "semble-lexical-file", "lexical")


def test_host_process_probe_exempts_only_owned_frozen_semble_run(monkeypatch):
    driver = os.getpid()
    adapter = Path("/tmp/frozen-semble-runner.pyz")
    ps_rows = "\n".join(
        [
            f"{driver} 1 Sun Oct  4 12:00:00 2026 python run.py pair",
            f"101 {driver} Sun Oct  4 12:00:01 2026 python {adapter} run --query-pack /tmp/pack.json --output-root /tmp/owned",
            f"102 1 Sun Oct  4 12:00:02 2026 python {adapter} run --query-pack /tmp/pack.json --output-root /tmp/foreign",
            f"103 {driver} Sun Oct  4 12:00:03 2026 cargo build",
            "104 1 Sun Oct  4 12:00:04 2026 pytest tests/",
        ]
    )

    def fake_run(command, **_kwargs):
        if command[0] == "ps":
            return SimpleNamespace(returncode=0, stdout=ps_rows)
        assert command[:2] == ["pgrep", "-f"]
        matches = {
            "cargo": "103\n",
            "semble": "101\n102\n",
            "pytest": "104\n",
        }.get(command[2], "")
        return SimpleNamespace(returncode=0 if matches else 1, stdout=matches)

    monkeypatch.setattr(pairrun.shutil, "which", lambda name: "/usr/bin/pgrep")
    monkeypatch.setattr(pairrun.subprocess, "run", fake_run)
    assert pairrun.find_competing_processes(owned_semble_adapter=adapter) == {
        "cargo": [103],
        "semble": [102],
        "pytest": [104],
    }
    assert pairrun.find_competing_processes()["semble"] == [101, 102]


def test_host_process_probe_keeps_owned_semble_run_clean(monkeypatch):
    driver = os.getpid()
    adapter = Path("/tmp/frozen-semble-runner.pyz")
    ps_rows = "\n".join(
        [
            f"{driver} 1 Sun Oct  4 12:00:00 2026 python run.py pair",
            f"101 {driver} Sun Oct  4 12:00:01 2026 python {adapter} run --query-pack /tmp/pack.json --output-root /tmp/owned",
        ]
    )

    def fake_run(command, **_kwargs):
        if command[0] == "ps":
            return SimpleNamespace(returncode=0, stdout=ps_rows)
        return SimpleNamespace(
            returncode=0 if command[2] == "semble" else 1,
            stdout="101\n" if command[2] == "semble" else "",
        )

    monkeypatch.setattr(pairrun.shutil, "which", lambda name: "/usr/bin/pgrep")
    monkeypatch.setattr(pairrun.subprocess, "run", fake_run)
    assert pairrun.find_competing_processes(owned_semble_adapter=adapter) == {"none": []}


def test_host_process_probe_does_not_exempt_reused_pid_or_wrong_adapter(monkeypatch):
    driver = os.getpid()
    adapter = Path("/tmp/frozen-semble-runner.pyz")
    ps_rows = "\n".join(
        [
            f"{driver} 1 Sun Oct  4 12:00:00 2026 python run.py pair",
            f"101 {driver} Sun Oct  4 12:00:01 2026 python /tmp/other-semble-runner.pyz run --query-pack /tmp/pack.json --output-root /tmp/other",
        ]
    )

    def fake_run(command, **_kwargs):
        if command[0] == "ps":
            return SimpleNamespace(returncode=0, stdout=ps_rows)
        return SimpleNamespace(
            returncode=0 if command[2] == "semble" else 1,
            stdout="101\n" if command[2] == "semble" else "",
        )

    monkeypatch.setattr(pairrun.shutil, "which", lambda name: "/usr/bin/pgrep")
    monkeypatch.setattr(pairrun.subprocess, "run", fake_run)
    assert pairrun.find_competing_processes(owned_semble_adapter=adapter) == {"semble": [101]}


@pytest.mark.parametrize("failure", ["pgrep", "ps", "race", "drift"])
def test_host_process_probe_fails_closed_when_process_identity_unavailable(monkeypatch, failure):
    adapter = Path("/tmp/frozen-semble-runner.pyz")
    semble_calls = 0

    def fake_run(command, **_kwargs):
        nonlocal semble_calls
        if command[0] == "ps":
            if failure == "ps":
                return SimpleNamespace(returncode=1, stdout="")
            if failure == "race":
                return SimpleNamespace(returncode=0, stdout="")
            driver = os.getpid()
            return SimpleNamespace(
                returncode=0,
                stdout="\n".join(
                    [
                        f"{driver} 1 Sun Oct  4 12:00:00 2026 python run.py pair",
                        f"101 {driver} Sun Oct  4 12:00:01 2026 python {adapter} run --query-pack /tmp/pack.json --output-root /tmp/owned",
                    ]
                ),
            )
        if failure == "pgrep":
            return SimpleNamespace(returncode=2, stdout="")
        if command[2] == "semble":
            semble_calls += 1
            if failure == "drift" and semble_calls == 2:
                return SimpleNamespace(returncode=1, stdout="")
            return SimpleNamespace(returncode=0, stdout="101\n")
        return SimpleNamespace(returncode=1, stdout="")

    monkeypatch.setattr(pairrun.shutil, "which", lambda name: "/usr/bin/pgrep")
    monkeypatch.setattr(pairrun.subprocess, "run", fake_run)
    expected = {"ps": "unavailable"} if failure in ("ps", "race") else {"pgrep": "unavailable"}
    assert pairrun.find_competing_processes(owned_semble_adapter=adapter) == expected
