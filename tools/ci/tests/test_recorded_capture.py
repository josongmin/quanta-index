"""Recorded input shape/metrics are reproducible, never authenticated by import."""

import copy
import json
import subprocess
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "tools/benchmark"))
import recorded_capture as capture
from evidence import RawFile, RunStore, sample_evidence, write_raw_file
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
    payload, summary = capture.agent_payload(RawFile.capture(path))
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
        capture.agent_payload(RawFile.capture(path))


def test_input_changed_between_snapshot_and_evaluation_refused(tmp_path):
    path = agent_file(tmp_path)
    raw = RawFile.capture(path)
    path.write_bytes(path.read_bytes() + b" ")
    with pytest.raises(ValueError):
        capture.agent_payload(raw)


def test_scan_import_preserves_every_row_and_never_invents_rg(tmp_path):
    payload = capture.scan_payload(write_raw_file(tmp_path / "scan.json", [scan_bytes()]))
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


def test_scan_summary_refuses_oversize_before_decoding(tmp_path):
    import evidence

    path = tmp_path / "scan.json"
    with path.open("wb") as stream:
        stream.truncate(evidence.CONTROL_DOCUMENT_BYTES + 1)
    with pytest.raises(ValueError, match="control document exceeds"):
        capture.scan_payload(RawFile.capture(path))


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
def test_scan_bad_native_inputs_refused(tmp_path, mutation):
    document = json.loads(scan_bytes())
    mutation(document)
    with pytest.raises(ValueError):
        capture.scan_payload(
            write_raw_file(tmp_path / "scan.json", [json.dumps(document).encode()])
        )


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

    def fail_source(*_args):
        raise ValueError("source acquisition oracle failure")

    with monkeypatch.context() as patch:
        patch.setattr(capture, "source_identity", fail_source)
        with pytest.raises(ValueError, match="source acquisition oracle failure"):
            capture.capture(repo, root, registry, agent, scan, "recorded_unauthenticated")
    failures = list((root / "failures").glob("*.json"))
    assert len(failures) == 1
    failure = json.loads(failures[0].read_text())
    assert failure["status"] == "failed"
    assert failure["phase"] == "source"
    assert failure["observations"].get("source") is None
    assert failure["error"]["message"] == "source acquisition oracle failure"
    assert not (root / "profiles/recorded.json").exists()
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


def test_recorded_capture_never_materializes_raw_trajectory_input(tmp_path, monkeypatch):
    import benchctl
    import evidence

    source = sample_evidence()["source"]
    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / "uv.lock").write_bytes(b"locked")
    monkeypatch.setattr(benchctl, "require_clean_worktree", lambda _repo: None)
    monkeypatch.setattr(benchctl, "resolve_checkout_head", lambda _repo: source["revision"])
    monkeypatch.setattr(benchctl, "require_frozen_source", lambda *_args: None)
    monkeypatch.setattr(capture, "source_identity", lambda *_args: source)
    monkeypatch.setattr(capture, "registry_digest", lambda _registry: "sha256:" + "a" * 64)
    registry = {"profiles": {"recorded": {"families": ["agent-outcome", "scan-vs-index"]}}}
    agent = agent_file(tmp_path)
    scan = tmp_path / "scan.json"
    scan.write_bytes(scan_bytes())
    old = evidence._consume_regular_file

    class BoundedReader:
        def __init__(self, handle):
            self.handle = handle

        def __getattr__(self, name):
            return getattr(self.handle, name)

        def read(self, size=-1):
            assert 0 <= size <= evidence.IO_CHUNK_BYTES
            return self.handle.read(size)

        def readline(self, size=-1):
            assert 0 < size <= evidence.CONTROL_DOCUMENT_BYTES + 1
            return self.handle.readline(size)

    def no_whole_input(path, consume):
        if path.suffix == ".jsonl":
            return old(path, lambda handle: consume(BoundedReader(handle)))
        return old(path, consume)

    monkeypatch.setattr(evidence, "_consume_regular_file", no_whole_input)
    document = capture.capture(
        repo, tmp_path / "evidence", registry, agent, scan, "recorded_unauthenticated"
    )
    assert {row["family"] for row in document["runs"]} == {"agent-outcome", "scan-vs-index"}


@pytest.mark.parametrize("domain", ["agent", "cargo"])
def test_line_protocol_adapter_rss_does_not_scale_with_raw_payload(
    tmp_path, record_property, domain
):
    script = """
import json, resource, sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from evidence import RawFile
raw = RawFile.capture(Path(sys.argv[2]))
if sys.argv[3] == 'agent':
    from recorded_capture import agent_payload
    payload, summary = agent_payload(raw)
    count = summary['pair_count']
    assert summary['record_count'] == count * 3
    assert payload['excluded_pairs'] == payload['unknown_pairs'] == 0
else:
    from criterion_capture import _binary
    binary, features = _binary(raw, 'pipeline')
    assert str(binary) == '/fixture/pipeline' and features == ['feature-a']
    count = 1
peak = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
if sys.platform != 'darwin':
    peak *= 1024
print(json.dumps(dict(count=count, bytes=raw.size, peak_bytes=peak)))
"""
    measurements = []
    for pairs in (40, 680):
        path = tmp_path / f"{domain}-{pairs}.jsonl"
        with path.open("wb") as stream:
            for trial in range(pairs):
                for row in valid_rows():
                    if domain == "agent":
                        row["trial_id"] = f"trial-{trial}"
                        row["trajectory"][2]["evidence_id"] = "x" * 64000
                    else:
                        row = {"reason": "compiler-message", "message": "x" * 64000}
                    stream.write(json.dumps(row).encode() + b"\n")
            if domain == "cargo":
                stream.write(
                    b'{"reason":"compiler-artifact","target":{"name":"pipeline","kind":["bench"]},"executable":"/fixture/pipeline","features":["feature-a"]}\n'
                )
                stream.write(b'{"reason":"build-finished","success":true}\n')
        completed = subprocess.run(
            [
                sys.executable,
                "-I",
                "-c",
                script,
                str(Path(capture.__file__).parent),
                str(path),
                domain,
            ],
            capture_output=True,
            text=True,
            check=True,
            timeout=60,
        )
        result = json.loads(completed.stdout)
        assert result["count"] == (pairs if domain == "agent" else 1)
        assert result["bytes"] == path.stat().st_size
        assert result["peak_bytes"] > 0
        measurements.append(result)
        record_property(f"{domain}_{pairs}_bytes", result["bytes"])
        record_property(f"{domain}_{pairs}_peak_bytes", result["peak_bytes"])
    assert measurements[1]["bytes"] - measurements[0]["bytes"] > 100 * 1024 * 1024
    # Pair metadata legitimately grows; raw trajectory/compiler message retention
    # must not grow with >100 MiB of input. This is not whole-capture host admission.
    assert measurements[1]["peak_bytes"] - measurements[0]["peak_bytes"] < 32 * 1024 * 1024
