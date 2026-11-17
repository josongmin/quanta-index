"""Tests for tools/benchmark/compare_dsl_bench.py and run_dsl_cold_matrix.py.

Covers the Layer-3 DSL query-latency regression gate over `BenchArtifactV1`
(schema 2, QI-BB-010):

1. the cold orchestrator collects one fresh-process sample per (scenario,
   sample) and hands them all to the binary's `--assemble`, never writing
   an artifact or a head itself
2. no-regression OK case (exit 0)
3. clear regression on the blocking metric: rel > 10% AND abs > threshold (exit 1)
4. AND-gate: rel exceeded but abs not exceeded -> OK
5. unmeasured current rows fail and unmeasured baselines are refused
6. standalone --update-baseline refuses receipt replay
7. mode mismatch -> exit 2
8. NEW scenarios require a reviewed baseline
9. MISSING scenario always fails
10. cold artifacts still require >=20 samples for tail checks
11. p95 is a looser blocking rail and p99 remains advisory
12. provenance gate: a current artifact not at HEAD, an old-schema artifact,
    a short/unknown head, a config-digest mismatch and a missing baseline
    are refused (exit 2), never compared
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import shlex
import subprocess
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
COMPARE_PATH = REPO_ROOT / "tools" / "benchmark" / "compare_dsl_bench.py"
COLD_MATRIX_PATH = REPO_ROOT / "tools" / "benchmark" / "run_dsl_cold_matrix.py"


def _load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


COMPARE = _load_module("compare_dsl_bench", COMPARE_PATH)


@pytest.mark.parametrize("source", ["path", "content"])
def test_comparator_refuses_oversized_native_control(tmp_path, source):
    path = tmp_path / "oversize.json"
    limit = COMPARE.CONTROL_DOCUMENT_BYTES
    with path.open("wb") as stream:
        stream.truncate(limit + 1)
    with pytest.raises(COMPARE.ArtifactRefused, match="exceeds"):
        COMPARE.load_artifact(
            path, role="current", content=" " * (limit + 1) if source == "content" else None
        )


def test_comparator_control_reader_refuses_symlink(tmp_path):
    real = tmp_path / "real"
    real.write_bytes(b"{}")
    alias = tmp_path / "alias"
    alias.symlink_to(real)
    with pytest.raises(COMPARE.ArtifactRefused, match="symlink"):
        COMPARE.load_artifact(alias, role="current")


def test_comparator_refuses_interrupted_baseline_pair(tmp_path: Path) -> None:
    baseline = tmp_path / "warm-matrix.json"
    marker = tmp_path / ".dsl-admission-pending"
    marker.write_text("interrupted", encoding="utf-8")
    with pytest.raises(COMPARE.ArtifactRefused, match="admission is incomplete"):
        COMPARE.require_no_pending_admission(baseline)


def test_native_diagnostic_cannot_be_admitted_as_a_complete_linux_baseline(tmp_path):
    path = tmp_path / "native-warm.json"
    payload = _artifact("warm", [_row("lexical.keyword.native", 1.0)])
    payload["detail"] = {"qualification": "diagnostic"}
    path.write_text(json.dumps(payload))
    artifact = COMPARE.load_artifact(path, role="baseline candidate")
    with pytest.raises(COMPARE.ArtifactRefused, match="diagnostic.*baseline"):
        COMPARE.require_complete_baseline_candidate(artifact)


@pytest.mark.parametrize("fault", [None, "source", "mode", "matrix", "qualification", "missing"])
def test_native_matrix_assembly_binds_the_rust_artifact_to_collected_bytes(tmp_path, monkeypatch, fault):
    monkeypatch.syspath_prepend(str(REPO_ROOT / "tools/benchmark"))
    matrix = tmp_path / "matrix.json"
    matrix.write_bytes(b'{"mode":"warm","qualification":"diagnostic"}\n')
    assembled = tmp_path / "artifact.json"
    matrix_sha = "sha256:" + hashlib.sha256(matrix.read_bytes()).hexdigest()
    payload = _artifact("cold" if fault == "mode" else "warm", [_row("lexical.keyword.native", 1.0)])
    if fault == "source":
        payload["provenance"]["git_head"] = OTHER_HEAD
    payload["detail"] = {
        "qualification": "authority" if fault == "qualification" else "diagnostic",
        "native_matrix_sha256": OTHER_DIGEST if fault == "matrix" else matrix_sha,
    }

    def run(*argv):
        assert argv == ("--assemble-native", "--input", str(matrix), "--out", str(assembled))
        if fault != "missing":
            assembled.write_text(json.dumps(payload))
        return subprocess.CompletedProcess(argv, 0, "", "")

    if fault is None:
        COLD_MATRIX.assemble_native(run, matrix, assembled, "warm", HEAD)
        assert json.loads(assembled.read_text()) == payload
    else:
        with pytest.raises(RuntimeError, match="Native DSL artifact"):
            COLD_MATRIX.assemble_native(run, matrix, assembled, "warm", HEAD)


COLD_MATRIX = _load_module("run_dsl_cold_matrix", COLD_MATRIX_PATH)

HEAD = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO_ROOT, text=True).strip()
COMPARATOR_CWD = REPO_ROOT
OTHER_HEAD = "fedcba9876543210fedcba9876543210fedcba98"
DIGEST = "sha256:" + "ab" * 32
OTHER_DIGEST = "sha256:" + "cd" * 32


@pytest.fixture(scope="module", autouse=True)
def frozen_comparator_checkout(tmp_path_factory):
    """Keep source-attribution tests independent of writers on shared main.

    The real CLI still resolves Git HEAD and rejects wrong-source artifacts.
    Only its Git working directory is pinned; no production guard is mocked.
    """
    repo = tmp_path_factory.mktemp("dsl-comparator") / "repo"
    subprocess.run(
        ["git", "clone", "--shared", "--no-checkout", "--quiet", str(REPO_ROOT), str(repo)],
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["git", "-C", str(repo), "update-ref", "--no-deref", "HEAD", HEAD],
        check=True,
        capture_output=True,
    )
    assert (
        subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
        == HEAD
    )
    with pytest.MonkeyPatch.context() as monkeypatch:
        monkeypatch.setattr(sys.modules[__name__], "COMPARATOR_CWD", repo)
        yield repo


def _row(
    scenario_id: str,
    p95: float | None,
    *,
    route_family: str = "lexical",
    syntax: str = "native",
    mode: str = "warm",
    result_shape: str = "candidates",
    early_stop_reason: str | None = None,
    samples: int = 200,
) -> dict:
    del mode  # the mode is the envelope's, not the row's, in schema 2
    latency = (
        None
        if p95 is None
        else {"p50_ms": p95 * 0.8, "p95_ms": p95, "p99_ms": p95 * 1.1, "samples": samples}
    )
    return {
        "scenario_id": scenario_id,
        "route_family": route_family,
        "syntax": syntax,
        "result_shape": result_shape,
        "latency": latency,
        "qps": None,
        "error_count": 0,
        "timeout_count": 0,
        "result_count": 3,
        "typed_error_code": None,
        "engine_touched": [route_family],
        "early_stop_reason": early_stop_reason,
    }


def _artifact(
    mode: str,
    rows: list[dict],
    *,
    head: str = HEAD,
    config_digest: str = DIGEST,
    corpus_digest: str = DIGEST,
    model_revision: str | None = None,
    host: dict | None = None,
    schema_version: int = 2,
) -> dict:
    return {
        "schema_version": schema_version,
        "dimension": f"dsl-{mode}",
        "mode": mode,
        "concurrency": 1,
        "provenance": {
            "git_head": head,
            "corpus_digest": corpus_digest,
            "config_digest": config_digest,
            "model_revision": model_revision,
        },
        "host": host
        or {
            "os": "linux",
            "arch": "x86_64",
            "cpu_count": 8,
            "mem_bytes": 1 << 34,
            "hostname_hash": DIGEST,
        },
        "resources": {"peak_rss_bytes": 1},
        "phases": {"build_ms": None, "update_ms": None, "gc_ms": None},
        "disk_amplification": None,
        "rows": rows,
        "detail": {},
    }


def _write_artifact(path: Path, mode: str, rows: list[dict], **overrides) -> None:
    path.write_text(
        json.dumps(_artifact(mode, rows, **overrides), indent=2) + "\n", encoding="utf-8"
    )


def _write_clean_preflight(path: Path, *, host: dict | None = None, status: str = "clean") -> None:
    host = host or {"os": "linux", "arch": "x86_64", "cpu_count": 8}
    host.setdefault("hostname_hash", DIGEST)
    host.setdefault("load_average", [1.0, 1.0, 1.0])
    path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "kind": "quanta-index-timing-preflight",
                "run_id": "benchctl:dsl-authority",
                "status": status,
                "host": host,
                "host_contention": {
                    "one_minute_load": host["load_average"][0],
                    "one_minute_load_limit": host["cpu_count"] * 0.5,
                    "over_limit": False,
                },
                "foreign_rust_processes": [],
            }
        ),
        encoding="utf-8",
    )


def _run(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(COMPARE_PATH), *args],
        cwd=COMPARATOR_CWD,
        check=False,
        capture_output=True,
        text=True,
    )


# ---------------------------------------------------------------------------
# The cold orchestrator hands every sample to the binary's assembler.
# ---------------------------------------------------------------------------

FAKE_BIN = """\
import json, sys
args = sys.argv[1:]
if args == ["--list"]:
    print(json.dumps([{"scenario_id": "lexical.keyword.native", "route_family": "lexical",
                       "syntax": "native", "expected_shape": "candidates"},
                      {"scenario_id": "history.commit.native", "route_family": "history",
                       "syntax": "native", "expected_shape": "commits"}]))
elif args[0] == "--scenario":
    scenario = args[1]
    stopped = scenario.startswith("history")
    print(json.dumps({"scenario_id": scenario, "route_family": scenario.split(".")[0],
                      "syntax": "native", "mode": "cold",
                      "result_shape": "typed_error" if stopped else "candidates",
                      "first_query_ms": None if stopped else 1.5, "result_count": None if stopped else 3,
                      "typed_error_code": "NOT_READY" if stopped else None, "engine_touched": [],
                      "early_stop_reason": "fixture_unavailable" if stopped else None,
                      "model_revision": None}))
elif args[0] == "--assemble":
    out = args[args.index("--out") + 1]
    samples = json.loads(sys.stdin.read())
    with open(out, "w") as f:
        json.dump({"received": samples, "samples_flag": args[args.index("--samples") + 1]}, f)
else:
    sys.exit(2)
"""


def test_cold_orchestrator_collects_fresh_samples_and_assembles_through_the_binary(
    tmp_path: Path,
) -> None:
    fake = tmp_path / "fake_cold_matrix.py"
    fake.write_text(FAKE_BIN, encoding="utf-8")
    out = tmp_path / "nested" / "cold-matrix.json"
    result = subprocess.run(
        [
            sys.executable,
            str(COLD_MATRIX_PATH),
            "--samples",
            "3",
            "--out",
            str(out),
            "--bin-cmd",
            f"{shlex.quote(sys.executable)} {shlex.quote(str(fake))}",
        ],
        cwd=REPO_ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    written = json.loads(out.read_text(encoding="utf-8"))
    assert written["samples_flag"] == "3"
    received = written["received"]
    lexical = [s for s in received if s["scenario_id"] == "lexical.keyword.native"]
    history = [s for s in received if s["scenario_id"] == "history.commit.native"]
    assert len(lexical) == 3, "three fresh-process samples for a measured scenario"
    assert len(history) == 1 and history[0]["early_stop_reason"] == "fixture_unavailable"
    assert "assembled 4 sample(s)" in result.stderr
    # The orchestrator never stamps a head or a git_rev of its own.
    assert "git" not in json.dumps(written)


def test_cold_orchestrator_refuses_a_null_latency_without_an_early_stop(tmp_path: Path) -> None:
    fake = tmp_path / "fake_cold_matrix.py"
    fake.write_text(
        FAKE_BIN.replace('"first_query_ms": None if stopped else 1.5', '"first_query_ms": None'),
        encoding="utf-8",
    )
    result = subprocess.run(
        [
            sys.executable,
            str(COLD_MATRIX_PATH),
            "--samples",
            "2",
            "--out",
            str(tmp_path / "cold-matrix.json"),
            "--bin-cmd",
            f"{shlex.quote(sys.executable)} {shlex.quote(str(fake))}",
        ],
        cwd=REPO_ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 2, result.stdout + result.stderr
    assert "null first_query_ms without early_stop_reason" in result.stderr


@pytest.mark.parametrize(
    ("mode", "fault"),
    [
        ("cold", None),
        ("cold", "binary"),
        ("cold", "checkout"),
        ("cold", "artifact"),
        ("cold", "source-drift"),
        ("warm", None),
        ("warm", "wrong-mode"),
        ("warm", "producer-exit"),
        ("warm", "cold-samples"),
    ],
)
def test_orchestrator_pins_a_frozen_legacy_producer(
    tmp_path: Path, mode: str, fault: str | None
) -> None:
    repo = tmp_path / "legacy"
    repo.mkdir()
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    subprocess.run(
        ["git", "-C", str(repo), "-c", "user.name=Fixture", "-c", "user.email=fixture@localhost",
         "commit", "--allow-empty", "-qm", "fixture"],
        check=True,
    )
    head = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
    artifact = _artifact(
        "cold" if fault == "wrong-mode" else mode,
        [_row("lexical.keyword.native", 1.0, samples=20)],
        head=OTHER_HEAD if fault == "artifact" else head,
    )
    if mode == "cold":
        body = FAKE_BIN.split('elif args[0] == "--assemble":')[0]
        body += 'elif args[0] == "--assemble":\n    sys.stdin.read()\n'
    else:
        body = 'import sys\nargs = sys.argv[1:]\nassert len(args) == 2 and args[0] == "--out"\n'
    write_artifact = (
        'pathlib.Path(args[args.index("--out") + 1]).write_text('
        + repr(json.dumps(artifact))
        + ")\n"
    )
    if mode == "cold":
        write_artifact = "    " + write_artifact
    if fault == "source-drift":
        write_artifact += '    pathlib.Path("changed-during-capture").touch()\n'
    if fault == "producer-exit":
        write_artifact += "sys.exit(17)\n"
    fake = tmp_path / "frozen-producer"
    fake.write_text(
        f"#!{sys.executable}\n"
        "import os, pathlib\n"
        'assert os.environ["QUANTA_INDEX_BENCH_DISABLE_QUERY_OBS"] == "1"\n'
        f"assert pathlib.Path.cwd().resolve() == pathlib.Path({str(repo.resolve())!r})\n"
        + body
        + write_artifact,
        encoding="utf-8",
    )
    fake.chmod(0o700)
    digest = "sha256:" + hashlib.sha256(fake.read_bytes()).hexdigest()
    out = tmp_path / "output.json"
    out.write_text("previous capture")
    sample_args = ["--samples", "1"] if mode == "cold" or fault == "cold-samples" else []
    result = subprocess.run(
        [
            sys.executable, str(COLD_MATRIX_PATH), *sample_args, "--out", str(out),
            "--bin-cmd", shlex.quote(str(fake)), "--bin-cwd", str(repo),
            "--bin-head", OTHER_HEAD if fault == "checkout" else head,
            "--bin-sha256", OTHER_DIGEST if fault == "binary" else digest,
            "--bin-mode", mode,
        ],
        cwd=REPO_ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if fault is None:
        assert result.returncode == 0, result.stderr
        assert json.loads(out.read_text()) == artifact
    else:
        assert result.returncode == 2, result.stderr
        assert out.read_text() == "previous capture"
        assert {
            "binary": "binary SHA-256 differs",
            "checkout": "HEAD changed during benchmark run",
            "artifact": "different commit or mode",
            "source-drift": "worktree is dirty",
            "wrong-mode": "different commit or mode",
            "producer-exit": "warm producer failed (exit 17)",
            "cold-samples": "--samples is only valid for the cold matrix",
        }[fault] in result.stderr


@pytest.mark.parametrize(
    ("mode", "fault"),
    [
        ("warm", None),
        ("cold", None),
        ("warm", "runner_digest"),
        ("cold", "daemon_digest"),
        ("cold", "history_digest"),
        ("warm", "foreign_history_daemon"),
        ("warm", "foreign_report"),
        ("cold", "omitted_scenario"),
        ("cold", "omitted_roster_item"),
        ("cold", "empty_roster"),
        ("cold", "warm_sample"),
        ("warm", "assembled"),
        ("cold", "foreign_assembly"),
    ],
)
def test_native_dsl_matrix_uses_pinned_binaries_and_complete_roster(tmp_path, mode, fault):
    def digest(path):
        return "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()

    assembly = fault in ("assembled", "foreign_assembly")
    source_args = []
    produced = None
    if assembly:
        repo = tmp_path / "source"
        repo.mkdir()
        subprocess.run(["git", "init", "-q", str(repo)], check=True)
        subprocess.run(
            ["git", "-C", str(repo), "-c", "user.name=Fixture", "-c", "user.email=fixture@localhost",
             "commit", "--allow-empty", "-qm", "fixture"], check=True,
        )
        head = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
        source_args = ["--native-artifact", "--bin-cwd", str(repo), "--bin-head", head]
        produced = _artifact(mode, [_row("text.1", 1.0)], head=OTHER_HEAD if fault == "foreign_assembly" else head)
        produced["detail"] = {"qualification": "diagnostic"}

    daemon = tmp_path / "daemon"
    daemon.write_text("pinned Native daemon")
    history = tmp_path / "history.json"
    history.write_text(json.dumps({
        "history": "pinned Published fixture",
        "daemon": str(daemon),
        "daemon_sha256": "a" * 64 if fault == "foreign_history_daemon" else digest(daemon)[7:],
        "config_sha256": "b" * 64,
    }))
    calls = tmp_path / "calls.jsonl"
    runner = tmp_path / "dsl-runner"
    text_ids = [f"text.{index}" for index in range(1, 8)]
    history_ids = [f"history.{index}" for index in range(1, 8)]
    runner.write_text(
        f"#!{sys.executable}\n"
        "import hashlib, json, pathlib, sys\n"
        "args = sys.argv[1:]\n"
        f"with pathlib.Path({str(calls)!r}).open('a') as log:\n"
        "    print(json.dumps(args), file=log)\n"
        f"mode, fault = {mode!r}, {fault!r}\n"
        f"daemon_sha = {digest(daemon).removeprefix('sha256:')!r}\n"
        f"text_ids, history_ids = {text_ids!r}, {history_ids!r}\n"
        f"produced = {produced!r}\n"
        'if args[0] == "--assemble-native":\n'
        '    assert args[1] == "--input" and args[3] == "--out"\n'
        '    produced["detail"]["native_matrix_sha256"] = "sha256:" + hashlib.sha256(pathlib.Path(args[2]).read_bytes()).hexdigest()\n'
        '    pathlib.Path(args[4]).write_text(json.dumps(produced))\n'
        '    sys.exit(0)\n'
        'if args == ["--list-native"]:\n'
        '    if fault == "omitted_roster_item":\n'
        '        history_ids.pop()\n'
        '    print(json.dumps({"text": text_ids, '
        '"history": [] if fault == "empty_roster" else history_ids}))\n'
        '    sys.exit(0)\n'
        "flag, source, scenario = args\n"
        'if fault == "omitted_scenario" and scenario == history_ids[0]:\n'
        '    sys.exit(0)\n'
        'warm = mode == "warm" or fault == "warm_sample"\n'
        'report = {"scenario_id": scenario, "passes": 1, "samples_per_pass": 2 if warm else 1, '
        '"prime_queries": 1 if warm else 0, "samples_ms": [2.0, 3.0] if warm else [2.0]}\n'
        'if flag == "--native-text":\n'
        '    report.update(schema="native-dsl-text-samples-v1", '
        'daemon_sha256="a" * 64 if fault == "foreign_report" else daemon_sha)\n'
        'else:\n'
        '    assert flag == ("--native-history-reopen" if mode == "warm" else "--native-history")\n'
        '    report["schema"] = "native-dsl-history-samples-v1"\n'
        '    report = {"schema": f"native-dsl-history-process-{mode}-v1", '
        '"daemon_sha256": daemon_sha, "config_sha256": "b" * 64, "history": report}\n'
        'print(json.dumps(report))\n',
        encoding="utf-8",
    )
    runner.chmod(0o700)
    out = tmp_path / "native.json"
    out.write_text("previous capture")
    result = subprocess.run(
        [
            sys.executable, str(COLD_MATRIX_PATH), "--out", str(out), "--bin-mode", mode,
            "--native-runner", str(runner),
            "--bin-sha256", OTHER_DIGEST if fault == "runner_digest" else digest(runner),
            "--native-daemon", str(daemon),
            "--native-daemon-sha256", OTHER_DIGEST if fault == "daemon_digest" else digest(daemon),
            "--native-history-fixture", str(history),
            "--native-history-sha256", OTHER_DIGEST if fault == "history_digest" else digest(history),
            *source_args,
            *(["--samples", "2"] if mode == "cold" else []),
        ],
        cwd=REPO_ROOT, check=False, capture_output=True, text=True,
    )
    if fault not in (None, "assembled"):
        assert result.returncode == 2, result.stderr
        assert {
            "runner_digest": "runner SHA-256 differs",
            "daemon_digest": "daemon SHA-256 differs",
            "history_digest": "History fixture SHA-256 differs",
            "foreign_history_daemon": "History fixture targets a different daemon",
            "foreign_report": "daemon SHA-256 differs",
            "omitted_scenario": "missing SDK report for history.1",
            "omitted_roster_item": "Native DSL History roster requires exactly 7 scenarios",
            "empty_roster": "Native DSL History roster requires exactly 7 scenarios",
            "warm_sample": "cold sample must contain one unprimed SDK query",
            "foreign_assembly": "Native DSL artifact differs",
        }[fault] in result.stderr
        assert out.read_text() == "previous capture"
        if fault.endswith("_digest") or fault == "foreign_history_daemon":
            assert not calls.exists()
        if fault in ("omitted_roster_item", "empty_roster"):
            assert [json.loads(line) for line in calls.read_text().splitlines()] == [["--list-native"]]
        return

    assert result.returncode == 0, result.stderr
    artifact = json.loads(out.read_text())
    if assembly:
        assert artifact["schema_version"] == 2
        assert artifact["provenance"]["git_head"] == head
        assert artifact["detail"]["qualification"] == "diagnostic"
        logged = [json.loads(line) for line in calls.read_text().splitlines()]
        assert logged[-1][0] == "--assemble-native"
        assert len(logged) == 16
        return
    assert artifact["schema"] == "native-dsl-matrix-v1"
    assert artifact["mode"] == mode
    assert artifact["daemon_sha256"] == digest(daemon)
    assert artifact["runner_sha256"] == digest(runner)
    assert artifact["history_fixture_sha256"] == digest(history)
    assert artifact["qualification"] == "diagnostic"
    assert [row["scenario_id"] for row in artifact["scenarios"]] == text_ids + history_ids
    expected_count = 1 if mode == "warm" else 2
    assert [len(row["reports"]) for row in artifact["scenarios"]] == [expected_count] * 14
    logged = [json.loads(line) for line in calls.read_text().splitlines()]
    assert logged.pop(0) == ["--list-native"]
    history_flag = "--native-history-reopen" if mode == "warm" else "--native-history"
    expected_calls = [
        [flag, str(path), scenario]
        for flag, path, ids in (("--native-text", daemon, text_ids), (history_flag, history, history_ids))
        for scenario in ids
        for _ in range(expected_count)
    ]
    assert logged == expected_calls


# ---------------------------------------------------------------------------
# Case: no regression -> OK exit 0
# ---------------------------------------------------------------------------


def test_no_regression_exits_zero(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(baseline, "warm", [_row("lexical.keyword.native", 1.00)])
    # +5% rel, +0.05ms abs -> under both legs
    _write_artifact(current, "warm", [_row("lexical.keyword.native", 1.05)])
    result = _run(str(baseline), str(current))
    assert result.returncode == 0, result.stdout + result.stderr
    assert "OK: no DSL latency regressions" in result.stdout
    assert "REGRESSION" not in result.stdout


# ---------------------------------------------------------------------------
# Case: clear regression -> exit 1
# ---------------------------------------------------------------------------


def test_clear_regression_exits_one(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(baseline, "warm", [_row("lexical.keyword.native", 10.00)])
    # +50% rel (>10%) and +5.0ms abs (>1.0ms) -> regression
    _write_artifact(current, "warm", [_row("lexical.keyword.native", 15.00)])
    result = _run(str(baseline), str(current))
    assert result.returncode == 1, result.stdout + result.stderr
    assert "REGRESSION" in result.stdout
    assert "lexical.keyword.native" in result.stdout
    assert "FAIL" in result.stdout
    assert "benchctl run dsl-authority --admit-baseline" in result.stdout


def test_warm_p95_only_drift_is_blocking(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    base = _row("lexical.keyword.native", 10.0)
    cur = _row("lexical.keyword.native", 25.0)
    base["latency"]["p50_ms"] = 5.0
    cur["latency"]["p50_ms"] = 5.4
    _write_artifact(baseline, "warm", [base])
    _write_artifact(current, "warm", [cur])
    result = _run(str(baseline), str(current))
    assert result.returncode == 1, result.stdout + result.stderr
    assert "REGRESSION lexical.keyword.native: p95" in result.stdout


def test_cold_p95_only_drift_is_blocking(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    base = _row("history.diff_added.native", 10.0, mode="cold", samples=20)
    cur = _row("history.diff_added.native", 25.0, mode="cold", samples=20)
    base["latency"]["p50_ms"] = 5.0
    cur["latency"]["p50_ms"] = 5.4
    _write_artifact(baseline, "cold", [base])
    _write_artifact(current, "cold", [cur])
    result = _run(str(baseline), str(current))
    assert result.returncode == 1, result.stdout + result.stderr
    assert "REGRESSION history.diff_added.native: p95" in result.stdout


def test_p95_requires_both_its_looser_relative_and_absolute_legs(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    base = _row("lexical.keyword.native", 20.0)
    cur = _row("lexical.keyword.native", 24.0)
    base["latency"]["p50_ms"] = 5.0
    cur["latency"]["p50_ms"] = 5.1
    _write_artifact(baseline, "warm", [base])
    _write_artifact(current, "warm", [cur])
    result = _run(str(baseline), str(current))
    assert result.returncode == 0, result.stdout + result.stderr
    assert "REGRESSION" not in result.stdout


def test_p99_only_drift_remains_advisory(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    base = _row("lexical.keyword.native", 10.0)
    cur = _row("lexical.keyword.native", 10.1)
    base["latency"]["p50_ms"] = cur["latency"]["p50_ms"] = 5.0
    base["latency"]["p95_ms"] = cur["latency"]["p95_ms"] = 10.0
    base["latency"]["p99_ms"] = 11.0
    cur["latency"]["p99_ms"] = 30.0
    _write_artifact(baseline, "warm", [base])
    _write_artifact(current, "warm", [cur])
    result = _run(str(baseline), str(current))
    assert result.returncode == 0, result.stdout + result.stderr
    assert "ADVISORY lexical.keyword.native: p99" in result.stdout


# ---------------------------------------------------------------------------
# Case: AND-gate -> rel exceeded but abs NOT -> OK
# ---------------------------------------------------------------------------


def test_rel_exceeded_but_not_abs_is_ok(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    # warm abs threshold is 1.0ms. base 0.40ms -> +50% rel but only +0.20ms abs.
    _write_artifact(baseline, "warm", [_row("lexical.sub.native", 0.40)])
    _write_artifact(current, "warm", [_row("lexical.sub.native", 0.60)])
    result = _run(str(baseline), str(current))
    assert result.returncode == 0, result.stdout + result.stderr
    assert "REGRESSION" not in result.stdout
    assert "OK" in result.stdout


def test_abs_exceeded_but_not_rel_is_ok(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    # base 100ms -> +2ms abs (>1.0) but only +2% rel (<10%) -> OK
    _write_artifact(baseline, "warm", [_row("lexical.big.native", 100.0)])
    _write_artifact(current, "warm", [_row("lexical.big.native", 102.0)])
    result = _run(str(baseline), str(current))
    assert result.returncode == 0, result.stdout
    assert "REGRESSION" not in result.stdout


# ---------------------------------------------------------------------------
# Case: early_stop_reason rows are not benchmark evidence
# ---------------------------------------------------------------------------


def test_early_stop_baseline_is_refused(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(
        baseline,
        "warm",
        [_row("structural.def.native", None, early_stop_reason="fixture_not_seeded")],
    )
    _write_artifact(
        current,
        "warm",
        [_row("structural.def.native", None, early_stop_reason="fixture_not_seeded")],
    )
    result = _run(str(baseline), str(current))
    assert result.returncode == 2, result.stdout + result.stderr
    assert "unmeasured" in result.stderr


def test_current_early_stop_fails_closed(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(baseline, "warm", [_row("history.commit.native", 1.00)])
    _write_artifact(
        current,
        "warm",
        [_row("history.commit.native", None, early_stop_reason="fixture_not_seeded")],
    )
    result = _run(str(baseline), str(current))
    assert result.returncode == 1, result.stdout + result.stderr
    assert "INVALID" in result.stdout


# ---------------------------------------------------------------------------
# Case: --update-baseline
# ---------------------------------------------------------------------------


def test_standalone_baseline_update_refuses_replayed_clean_receipt(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(baseline, "warm", [_row("lexical.keyword.native", 1.00)])
    _write_artifact(current, "warm", [_row("lexical.keyword.native", 10.00)])

    fail_result = _run(str(baseline), str(current))
    assert fail_result.returncode == 1, fail_result.stdout

    receipt = tmp_path / "preflight.json"
    _write_clean_preflight(receipt)
    update_result = _run(
        str(baseline), str(current), "--update-baseline", "--preflight-receipt", str(receipt)
    )
    assert update_result.returncode == 2, update_result.stdout + update_result.stderr
    assert "benchctl run dsl-authority --admit-baseline" in update_result.stderr
    assert baseline.read_text(encoding="utf-8") != current.read_text(encoding="utf-8")


def test_update_baseline_refuses_unmeasured_candidate(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(
        current,
        "warm",
        [_row("lexical.keyword.native", None, early_stop_reason="fixture_not_seeded")],
    )
    artifact = COMPARE.load_artifact(current, role="baseline candidate")
    with pytest.raises(COMPARE.ArtifactRefused, match="unmeasured"):
        COMPARE.require_complete_baseline_candidate(artifact)
    assert not baseline.exists()


def test_update_baseline_requires_clean_matching_preflight(tmp_path: Path) -> None:
    current = tmp_path / "current.json"
    _write_artifact(current, "warm", [_row("lexical.keyword.native", 1.00)])
    artifact = COMPARE.load_artifact(current, role="baseline candidate")
    with pytest.raises(COMPARE.ArtifactRefused, match="requires a preflight receipt"):
        COMPARE.require_clean_preflight(None, artifact)
    receipt = tmp_path / "preflight.json"
    _write_clean_preflight(receipt, status="blocked")
    with pytest.raises(COMPARE.ArtifactRefused, match="is not clean"):
        COMPARE.require_clean_preflight(receipt, artifact)
    _write_clean_preflight(receipt, host={"os": "linux", "arch": "x86_64", "cpu_count": 4})
    with pytest.raises(COMPARE.ArtifactRefused, match="does not match candidate host"):
        COMPARE.require_clean_preflight(receipt, artifact)


def test_update_baseline_refuses_overloaded_receipt_labeled_clean(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(current, "warm", [_row("lexical.keyword.native", 1.00)])
    receipt = tmp_path / "preflight.json"
    _write_clean_preflight(
        receipt,
        host={"os": "linux", "arch": "x86_64", "cpu_count": 8, "load_average": [20.0, 1.0, 1.0]},
    )

    artifact = COMPARE.load_artifact(current, role="baseline candidate")
    with pytest.raises(COMPARE.ArtifactRefused, match="host load"):
        COMPARE.require_clean_preflight(receipt, artifact)
    assert not baseline.exists()


# ---------------------------------------------------------------------------
# Case: mode mismatch -> exit 2
# ---------------------------------------------------------------------------


def test_mode_mismatch_exits_two(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(baseline, "warm", [_row("lexical.keyword.native", 1.00)])
    _write_artifact(current, "cold", [_row("lexical.keyword.native", 1.00, mode="cold")])
    result = _run(str(baseline), str(current))
    assert result.returncode == 2, result.stdout + result.stderr
    assert "mode mismatch" in result.stderr


# ---------------------------------------------------------------------------
# Provenance gate (QI-BB-010): stale, unattributed and old-schema artifacts
# are refused before any comparison.
# ---------------------------------------------------------------------------


def test_a_current_artifact_not_at_head_is_refused_as_stale(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(baseline, "warm", [_row("lexical.keyword.native", 1.00)])
    _write_artifact(current, "warm", [_row("lexical.keyword.native", 1.00)], head=OTHER_HEAD)
    result = _run(str(baseline), str(current))
    assert result.returncode == 2, result.stdout + result.stderr
    assert "stale artifact" in result.stderr and OTHER_HEAD in result.stderr
    # Not even --update-baseline records a stale artifact.
    update = _run(str(baseline), str(current), "--update-baseline")
    assert update.returncode == 2, update.stdout + update.stderr
    assert json.loads(baseline.read_text())["provenance"]["git_head"] == HEAD


def test_a_baseline_at_another_head_is_compared_not_refused(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(baseline, "warm", [_row("lexical.keyword.native", 1.00)], head=OTHER_HEAD)
    _write_artifact(current, "warm", [_row("lexical.keyword.native", 1.02)])
    result = _run(str(baseline), str(current))
    assert result.returncode == 0, result.stdout + result.stderr
    assert "OK" in result.stdout


def test_corpus_model_and_host_class_must_match_the_baseline(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(baseline, "warm", [_row("lexical.keyword.native", 1.00)])
    cases = (
        ({"corpus_digest": OTHER_DIGEST}, "corpus_digest mismatch"),
        ({"model_revision": "embed-v2"}, "model_revision mismatch"),
        (
            {
                "host": {
                    "os": "linux",
                    "arch": "x86_64",
                    "cpu_count": 16,
                    "mem_bytes": 1 << 34,
                    "hostname_hash": DIGEST,
                }
            },
            "host identity mismatch",
        ),
    )
    for overrides, expected in cases:
        _write_artifact(current, "warm", [_row("lexical.keyword.native", 1.00)], **overrides)
        result = _run(str(baseline), str(current))
        assert result.returncode == 2, result.stdout + result.stderr
        assert expected in result.stderr


def test_an_old_schema_artifact_is_refused(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    baseline.write_text(
        json.dumps({"schema_version": 1, "mode": "warm", "git_rev": "3148c02", "rows": []}),
        encoding="utf-8",
    )
    _write_artifact(current, "warm", [_row("lexical.keyword.native", 1.00)])
    result = _run(str(baseline), str(current))
    assert result.returncode == 2, result.stdout + result.stderr
    assert "schema_version 1, not 2" in result.stderr and "re-capture" in result.stderr
    _write_artifact(current, "warm", [_row("lexical.keyword.native", 1.00)], schema_version=1)
    result = _run(str(baseline), str(current))
    assert result.returncode == 2 and "current" in result.stderr


def test_a_short_or_unknown_head_is_refused(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(baseline, "warm", [_row("lexical.keyword.native", 1.00)])
    for bad in ("unknown", "3148c02"):
        _write_artifact(current, "warm", [_row("lexical.keyword.native", 1.00)], head=bad)
        result = _run(str(baseline), str(current))
        assert result.returncode == 2, result.stdout + result.stderr
        assert "not 40 lowercase hex" in result.stderr


def test_a_config_digest_mismatch_is_refused(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(
        baseline, "warm", [_row("lexical.keyword.native", 1.00)], config_digest=OTHER_DIGEST
    )
    _write_artifact(current, "warm", [_row("lexical.keyword.native", 1.00)])
    result = _run(str(baseline), str(current))
    assert result.returncode == 2, result.stdout + result.stderr
    assert "config_digest mismatch" in result.stderr


def test_a_missing_baseline_is_a_typed_refusal(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(current, "warm", [_row("lexical.keyword.native", 1.00)])
    result = _run(str(baseline), str(current))
    assert result.returncode == 2, result.stdout + result.stderr
    assert "no such artifact" in result.stderr
    assert "benchctl run dsl-authority --admit-baseline" in result.stderr


def test_cold_rows_with_insufficient_samples_fail_closed(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(
        baseline,
        "cold",
        [_row("history.diff_added.native", 10.0, mode="cold", samples=3)],
    )
    _write_artifact(
        current,
        "cold",
        [_row("history.diff_added.native", 11.0, mode="cold", samples=3)],
    )
    result = _run(str(baseline), str(current))
    assert result.returncode == 2, result.stdout + result.stderr
    assert "insufficient samples" in result.stdout


# ---------------------------------------------------------------------------
# Case: NEW scenario requires reviewed baseline
# ---------------------------------------------------------------------------


def test_new_scenario_fails_without_reviewed_baseline(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(baseline, "warm", [_row("lexical.keyword.native", 1.00)])
    _write_artifact(
        current,
        "warm",
        [
            _row("lexical.keyword.native", 1.00),
            _row("lexical.brand.new", 999.0),
        ],
    )
    result = _run(str(baseline), str(current))
    assert result.returncode == 1, result.stdout + result.stderr
    assert "lexical.brand.new" in result.stdout
    assert "NEW" in result.stdout
    assert "new without baseline" in result.stdout


# ---------------------------------------------------------------------------
# Case: MISSING scenario
# ---------------------------------------------------------------------------


def test_missing_scenario_fails_without_allow(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(
        baseline,
        "warm",
        [
            _row("lexical.keyword.native", 1.00),
            _row("lexical.gone.native", 2.00),
        ],
    )
    _write_artifact(current, "warm", [_row("lexical.keyword.native", 1.00)])
    result = _run(str(baseline), str(current))
    assert result.returncode == 1, result.stdout + result.stderr
    assert "MISSING" in result.stdout
    assert "lexical.gone.native" in result.stdout


def test_missing_unmeasured_baseline_scenario_is_refused(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(
        baseline,
        "warm",
        [
            _row("lexical.keyword.native", 1.00),
            _row("structural.def.native", None, early_stop_reason="fixture_not_seeded"),
        ],
    )
    _write_artifact(current, "warm", [_row("lexical.keyword.native", 1.00)])
    result = _run(str(baseline), str(current))
    assert result.returncode == 2, result.stdout + result.stderr
    assert "unmeasured" in result.stderr


def test_semantic_contract_change_is_refused(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(baseline, "warm", [_row("lexical.keyword.native", 1.00)])
    _write_artifact(
        current,
        "warm",
        [_row("lexical.keyword.native", 1.00, result_shape="typed_error")],
    )
    result = _run(str(baseline), str(current))
    assert result.returncode == 2, result.stdout + result.stderr
    assert "changed result_shape" in result.stderr


def test_result_count_or_engine_change_is_refused(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(baseline, "warm", [_row("lexical.keyword.native", 1.0)])
    changed = _row("lexical.keyword.native", 1.0)
    changed["result_count"] = 2
    _write_artifact(current, "warm", [changed])
    result = _run(str(baseline), str(current))
    assert result.returncode == 2
    assert "changed result_count" in result.stderr

    changed["result_count"] = 3
    changed["engine_touched"] = ["semantic"]
    _write_artifact(current, "warm", [changed])
    result = _run(str(baseline), str(current))
    assert result.returncode == 2
    assert "changed engine_touched" in result.stderr


def test_invalid_thresholds_are_refused(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(baseline, "warm", [_row("lexical.keyword.native", 1.0)])
    _write_artifact(current, "warm", [_row("lexical.keyword.native", 1.0)])
    for flag, value in (
        ("--rel-threshold", "nan"),
        ("--rel-threshold", "-1"),
        ("--abs-threshold-ms", "inf"),
        ("--p95-rel-threshold", "nan"),
        ("--p95-abs-threshold-ms", "-1"),
    ):
        result = _run(str(baseline), str(current), flag, value)
        assert result.returncode == 2, result.stdout + result.stderr
        assert "finite and non-negative" in result.stderr


def test_unordered_latency_is_refused(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(baseline, "warm", [_row("lexical.keyword.native", 1.0)])
    bad = _row("lexical.keyword.native", 1.0)
    bad["latency"]["p50_ms"] = 2.0
    _write_artifact(current, "warm", [bad])
    result = _run(str(baseline), str(current))
    assert result.returncode == 2, result.stdout + result.stderr
    assert "unordered latency percentiles" in result.stderr


def test_malformed_json_is_typed_refusal(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(baseline, "warm", [_row("lexical.keyword.native", 1.0)])
    current.write_text("{invalid", encoding="utf-8")
    result = _run(str(baseline), str(current))
    assert result.returncode == 2, result.stdout + result.stderr
    assert "cannot be decoded" in result.stderr


def test_macos_artifact_cannot_be_admitted_as_canonical_baseline(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(
        current,
        "warm",
        [_row("lexical.keyword.native", 1.0)],
        host={
            "os": "macos",
            "arch": "aarch64",
            "cpu_count": 8,
            "mem_bytes": 1 << 34,
            "hostname_hash": DIGEST,
        },
    )
    artifact = COMPARE.load_artifact(current, role="baseline candidate")
    with pytest.raises(COMPARE.ArtifactRefused, match="canonical Linux host"):
        COMPARE.require_complete_baseline_candidate(artifact)
    assert not baseline.exists()


if __name__ == "__main__":
    raise SystemExit(subprocess.call([sys.executable, "-m", "pytest", __file__, "-q"]))
