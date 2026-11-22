"""Focused authority admission tests for the Native DSL cold collector."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
from types import SimpleNamespace

import pytest

BENCHMARK = Path(__file__).resolve().parents[2] / "benchmark"
COLLECTOR = BENCHMARK / "run_dsl_cold_matrix.py"


@pytest.fixture
def collector(monkeypatch):
    monkeypatch.syspath_prepend(str(BENCHMARK))
    spec = importlib.util.spec_from_file_location("native_dsl_authority_collector", COLLECTOR)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _digest(path: Path) -> str:
    return "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()


def _inputs(tmp_path: Path) -> tuple[argparse.Namespace, list[str], dict]:
    source = tmp_path / "source"
    source.mkdir()
    runner = tmp_path / "runner"
    daemon = tmp_path / "daemon"
    runner.write_bytes(b"pinned runner")
    daemon.write_bytes(b"pinned daemon")
    published = {"view": {"sequence": 5}, "cut": {"durable_epoch": 18}, "instance": [1] * 16}
    fixture = tmp_path / "history.json"
    fixture.write_text(json.dumps({
        "source_git_head": "a" * 40,
        "daemon": str(daemon), "daemon_sha256": _digest(daemon)[7:],
        "config_sha256": "b" * 64,
        "history": {"view": published["view"], "cut": published["cut"],
                    "instance": {"instance": published["instance"]}},
    }))
    args = argparse.Namespace(
        samples=20, out=tmp_path / "native.json", bin_mode="cold", bin_cwd=source,
        bin_head="a" * 40, bin_sha256=_digest(runner), native_artifact=True,
        native_authority=True, native_daemon=daemon, native_daemon_sha256=_digest(daemon),
        native_history_fixture=fixture, native_history_sha256=_digest(fixture),
    )
    return args, [str(runner)], published


@pytest.mark.parametrize("change,expected", [
    ({"native_artifact": False}, "--native-artifact"),
    ({"bin_cwd": None}, "frozen"),
    ({"bin_head": None}, "frozen"),
    ({"samples": 19}, "20 cold samples"),
])
def test_authority_preflight_requires_paired_frozen_linux_cold_inputs(
    collector, monkeypatch, tmp_path, change, expected,
):
    args, _, _ = _inputs(tmp_path)
    monkeypatch.setattr(collector.sys, "platform", "linux")
    for key, value in change.items():
        setattr(args, key, value)
    with pytest.raises(RuntimeError, match=expected):
        collector.require_native_authority(args)


def test_authority_preflight_rejects_non_linux_and_old_namespace(collector, monkeypatch, tmp_path):
    args, _, _ = _inputs(tmp_path)
    monkeypatch.setattr(collector.sys, "platform", "darwin")
    with pytest.raises(RuntimeError, match="Linux host"):
        collector.require_native_authority(args)
    del args.native_authority
    assert collector.require_native_authority(args) is False


@pytest.mark.parametrize("mode", ["warm", "cold"])
@pytest.mark.parametrize("source_head", [None, "b" * 40])
def test_authority_refuses_unbound_history_before_launch(
    collector, monkeypatch, tmp_path, mode, source_head,
):
    import benchctl

    args, bin_cmd, _ = _inputs(tmp_path)
    args.bin_mode = mode
    fixture = args.native_history_fixture
    payload = json.loads(fixture.read_text())
    if source_head is None:
        del payload["source_git_head"]
    else:
        payload["source_git_head"] = source_head
    fixture.write_text(json.dumps(payload))
    args.native_history_sha256 = _digest(fixture)
    monkeypatch.setattr(collector.sys, "platform", "linux")
    monkeypatch.setattr(benchctl, "require_frozen_source", lambda *_: None)
    monkeypatch.setattr(collector, "native_json", lambda *_: pytest.fail("runner launched"))
    args.out.write_text("previous capture")

    with pytest.raises(RuntimeError, match="History fixture source HEAD differs"):
        collector.measure_native(args, bin_cmd)
    assert args.out.read_text() == "previous capture"


@pytest.mark.parametrize("mode,fault,expected", [
    ("cold", None, None),
    ("warm", None, None),
    ("cold", "error_null", None),
    ("cold", "text_missing", "measured daemon RSS"),
    ("cold", "text_zero", "measured daemon RSS"),
    ("cold", "history_bool", "measured daemon RSS"),
    ("cold", "retained_missing", "measured daemon RSS"),
    ("cold", "error_positive", "client error has daemon RSS"),
])
def test_authority_aggregate_requires_every_daemon_report_rss(
    collector, monkeypatch, tmp_path, mode, fault, expected,
):
    args, bin_cmd, published = _inputs(tmp_path)
    args.bin_mode = mode
    monkeypatch.setattr(collector.sys, "platform", "linux")
    import benchctl

    monkeypatch.setattr(benchctl, "require_frozen_source", lambda *_: None)
    error_ids = [f"error.{n}" for n in range(5)]
    text_ids = [f"text.{n}" for n in range(9)] + error_ids
    history_ids = [f"history.{n}" for n in range(7)]
    retained_ids = [f"retained.{n}" for n in range(12)]
    roster = {"text": text_ids, "error": error_ids, "history": history_ids,
              "retained": retained_ids}
    calls: dict[str, int] = {}
    samples = 1 if mode == "cold" else 200

    def fake_native_json(_run, _description, *argv):
        if argv == ("--list-native",):
            return roster
        scenario = argv[-1]
        calls[scenario] = calls.get(scenario, 0) + 1
        first = calls[scenario] == 1
        kind = "error" if scenario in error_ids else scenario.split(".", 1)[0]
        report = {
            "schema": f"native-dsl-{kind}-samples-v{2 if kind == 'history' else 1}",
            "scenario_id": scenario, "passes": 1, "samples_per_pass": samples,
            "prime_queries": 0, "samples_ms": [1.0] * samples,
        }
        wrapper = {"daemon_sha256": args.native_daemon_sha256[7:],
                   "daemon_peak_rss_bytes": 4096}
        if kind == "error":
            report.update(result_shape="typed_error", typed_error_code="PARSE_FAIL",
                          error_count=samples, engine_touched=[], measurement_scope="client_preflight")
            wrapper.pop("daemon_peak_rss_bytes")
        elif kind == "history":
            report.update(published={**published, "session": [2] * 16, "member": [3] * 16},
                          observed_page={"view": published["view"],
                                         "instance": published["instance"],
                                         "session": [2] * 16, "member": [3] * 16,
                                         "read_epoch": 18, "matched": 1,
                                         "remaining": 1, "after": None})
            assert argv[0] == ("--native-history" if mode == "cold" else "--native-history-reopen")
            wrapper["schema"] = f"native-dsl-history-process-{mode}-v1"
            wrapper["config_sha256"] = "b" * 64
            wrapper["history"] = report
        elif kind == "retained":
            report.update(query={"scope": "repo"}, fixture_sha256="sha256:" + "f" * 64,
                          config_template_sha256="c" * 64,
                          published=[{"view": {"sequence": 1},
                                      "page": {"view": {"sequence": 1}}}])
        if kind != "history":
            wrapper.update(report)
        if first and fault == f"{kind}_missing":
            wrapper.pop("daemon_peak_rss_bytes", None)
        elif first and fault == "text_zero" and kind == "text":
            wrapper["daemon_peak_rss_bytes"] = 0
        elif first and fault == "history_bool" and kind == "history":
            wrapper["daemon_peak_rss_bytes"] = True
        elif first and fault == "error_positive" and kind == "error":
            wrapper["daemon_peak_rss_bytes"] = 4096
        elif first and fault == "error_null" and kind == "error":
            wrapper["daemon_peak_rss_bytes"] = None
        return wrapper

    monkeypatch.setattr(collector, "native_json", fake_native_json)

    def fake_assemble(_run, matrix, out, assembled_mode, head, qualification):
        raw = json.loads(matrix.read_text())
        assert (assembled_mode, head, qualification) == (mode, args.bin_head, "authority")
        assert raw["qualification"] == "authority"
        assert len(raw["scenarios"]) == 33
        assert all(len(row["reports"]) == (20 if mode == "cold" else 1) for row in raw["scenarios"])
        out.write_text(json.dumps(raw))

    monkeypatch.setattr(collector, "assemble_native", fake_assemble)
    args.out.write_text("previous capture")
    if expected:
        with pytest.raises(RuntimeError, match=expected):
            collector.measure_native(args, bin_cmd)
        assert args.out.read_text() == "previous capture"
    else:
        assert collector.measure_native(args, bin_cmd) == 33
        assert json.loads(args.out.read_text())["qualification"] == "authority"
        assert len(calls) == 33 and set(calls.values()) == {20 if mode == "cold" else 1}


@pytest.mark.parametrize("qualification,commitment,valid", [
    ("authority", True, True),
    ("diagnostic", True, False),
    ("authority", False, False),
])
def test_authority_assembly_checks_qualification_and_raw_matrix_commitment(
    collector, monkeypatch, tmp_path, qualification, commitment, valid,
):
    import compare_dsl_bench

    matrix = tmp_path / "matrix.json"
    matrix.write_text('{"qualification":"authority"}\n')
    artifact = tmp_path / "artifact.json"
    head = "a" * 40
    monkeypatch.setattr(compare_dsl_bench, "load_artifact", lambda *_args, **_kwargs:
                        SimpleNamespace(mode="cold", git_head=head,
                                        qualification=qualification))

    def run(*argv):
        assert argv == ("--assemble-native", "--input", str(matrix), "--out", str(artifact))
        artifact.write_text(json.dumps({"detail": {
            "native_matrix_sha256": _digest(matrix) if commitment else "sha256:" + "0" * 64,
        }}))
        return SimpleNamespace(returncode=0, stderr="")

    if valid:
        collector.assemble_native(run, matrix, artifact, "cold", head, "authority")
    else:
        with pytest.raises(RuntimeError, match="Native DSL artifact differs"):
            collector.assemble_native(run, matrix, artifact, "cold", head, "authority")
