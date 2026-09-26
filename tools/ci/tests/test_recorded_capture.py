"""Recorded input shape/metrics are reproducible, never authenticated by import."""

import copy
import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "tools/benchmark"))
import recorded_capture as capture
from evidence import RunStore, sample_evidence
from test_agent_outcome_benchmark import valid_rows


def agent_file(tmp_path, rows=None):
    path = tmp_path / "agent.jsonl"
    path.write_text(
        "".join(json.dumps(row) + "\n" for row in (valid_rows() if rows is None else rows))
    )
    return path


def scan_bytes(chunks=2000):
    return json.dumps(
        {
            "artifacts": [
                {
                    "schema_version": 2,
                    "dimension": "scan-vs-index",
                    "provenance": {"git_head": "a" * 40},
                    "detail": {"chunks": chunks},
                    "rows": [
                        {
                            "scenario_id": f"scan-vs-index.chunks{chunks}.index_query",
                            "latency": {"samples": 50, "p50_ms": 1.0, "p95_ms": 2.0, "p99_ms": 3.0},
                            "error_count": 0,
                            "timeout_count": 0,
                            "early_stop_reason": None,
                        }
                    ],
                }
            ]
        }
    ).encode()


def test_agent_metrics_preserve_denominators_and_absent_evidence(tmp_path):
    rows = valid_rows()
    rows[2]["trajectory"][2]["useful"] = False
    path = agent_file(tmp_path, rows)
    payload, summary = capture.agent_payload(path, path.read_bytes())
    assert payload["capture"] == "recorded_unauthenticated"
    assert payload["task_count"] == payload["pair_count"] == 1
    metrics = {row["name"]: row for row in payload["metrics"]}
    assert metrics["A.fail_to_pass"]["numerator"] == metrics["A.fail_to_pass"]["denominator"] == 1
    assert metrics["C.useful_evidence_coverage"]["value"] == 0
    assert "C.first_useful_evidence_ms_when_present" not in metrics
    assert summary["pairs"][0]["arms"]["C"]["first_useful_evidence_ms"] is None


@pytest.mark.parametrize(
    "mutation",
    [
        lambda rows: rows.pop(),
        lambda rows: rows.append(copy.deepcopy(rows[0])),
        lambda rows: rows[1].update(model_revision="wrong"),
        lambda rows: rows[1]["usage"].update(tool_calls=0),
    ],
)
def test_existing_agent_owner_refuses_bad_records(tmp_path, mutation):
    rows = valid_rows()
    mutation(rows)
    path = agent_file(tmp_path, rows)
    with pytest.raises(ValueError):
        capture.agent_payload(path, path.read_bytes())


def test_input_changed_between_snapshot_and_evaluation_refused(tmp_path):
    path = agent_file(tmp_path)
    raw = path.read_bytes()
    path.write_bytes(raw + b" ")
    with pytest.raises(ValueError):
        capture.agent_payload(path, raw)


def test_scan_import_preserves_every_row_and_never_invents_rg():
    payload = capture.scan_payload(scan_bytes())
    assert payload["diagnostic_only"] is True
    assert [p["metric"] for p in payload["points"]] == [
        "p50",
        "p95",
        "p99",
        "error_count",
        "timeout_count",
    ]
    assert [p["value"] for p in payload["points"]] == [1, 2, 3, 0, 0]
    assert not any("rg" in p["metric"] for p in payload["points"])


@pytest.mark.parametrize(
    "mutation",
    [
        lambda d: d["artifacts"].append(copy.deepcopy(d["artifacts"][0])),
        lambda d: d["artifacts"][0].update(dimension="wrong"),
        lambda d: d["artifacts"][0]["rows"][0].update(latency=None, early_stop_reason="not_run"),
        lambda d: d["artifacts"][0]["rows"][0]["latency"].update(p99_ms=float("nan")),
        lambda d: d["artifacts"][0]["provenance"].update(git_head="short"),
    ],
)
def test_scan_bad_native_inputs_refused(mutation):
    document = json.loads(scan_bytes())
    mutation(document)
    with pytest.raises(ValueError):
        capture.scan_payload(json.dumps(document).encode())


def test_complete_recorded_capture_and_replay_contract(tmp_path, monkeypatch):
    import subprocess

    import benchctl

    template = sample_evidence()
    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / "uv.lock").write_bytes(b"locked")
    monkeypatch.setattr(benchctl, "require_clean_worktree", lambda _repo: None)
    monkeypatch.setattr(
        benchctl, "resolve_checkout_head", lambda _repo: template["source"]["revision"]
    )
    monkeypatch.setattr(benchctl, "require_frozen_source", lambda *_args: None)
    monkeypatch.setattr(capture, "source_identity", lambda *_args: template["source"])
    registry = {"profiles": {"recorded": {"families": ["agent-outcome", "scan-vs-index"]}}}
    monkeypatch.setattr(capture, "registry_digest", lambda _registry: "sha256:" + "a" * 64)
    agent = agent_file(tmp_path)
    scan = tmp_path / "scan.json"
    scan.write_bytes(scan_bytes())
    root = tmp_path / "evidence"
    with pytest.raises(ValueError, match="authenticated imports are refused"):
        capture.capture(repo, root, registry, agent, scan, "authenticated")
    assert not (root / "profiles/recorded.json").exists()
    document = capture.capture(repo, root, registry, agent, scan, "recorded_unauthenticated")
    assert len(document["runs"]) == 2
    assert capture.validate(repo, root, registry) == document
    cli = Path(__file__).resolve().parents[2] / "benchmark/benchctl.py"
    for family in ("agent-outcome", "scan-vs-index"):
        completed = subprocess.run(
            [sys.executable, str(cli), "replay", "--family", family, "--evidence-root", str(root)],
            text=True,
            capture_output=True,
            check=False,
        )
        assert completed.returncode == 0, completed.stderr
        receipt = json.loads(completed.stdout)
        assert receipt["artifact_oracle"] == "pass"
        assert receipt["authenticity"] == "recorded_unauthenticated"
    store = RunStore(root)
    evidence = store.load(document["runs"][0]["run_id"])
    evidence["payload"]["capture"] = "authenticated"
    with pytest.raises(ValueError, match="differs from native recomputation"):
        capture.replay_run(store, evidence)


def test_missing_cli_inputs_refuse_before_source_or_producer(capsys, tmp_path):
    import benchctl

    assert benchctl.main(["run", "recorded", "--evidence-root", str(tmp_path / "evidence")]) == 2
    assert "requires --agent-recording and --scan-recording" in capsys.readouterr().err
    assert not (tmp_path / "evidence").exists()
