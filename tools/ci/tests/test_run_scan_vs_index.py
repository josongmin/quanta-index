"""Tests for tools/benchmark/run_scan_vs_index.py (QI-BB-010).

The runner used to be broken on HEAD: it never passed the `--source-fingerprint`
the binary required, so it failed before measuring anything. The binary now
resolves the head itself and emits a `BenchArtifactV1`; the runner must:

1. invoke the binary with exactly the flags it accepts (no fingerprint), once
   per scale, and keep every artifact verbatim beside the markdown report
2. refuse an artifact that is not schema 2 or that lacks the envelope
3. refuse a sweep whose artifacts name more than one head
"""

from __future__ import annotations

import importlib.util
import json
import shlex
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
RUNNER_PATH = REPO_ROOT / "tools" / "benchmark" / "run_scan_vs_index.py"

HEAD = "0123456789abcdef0123456789abcdef01234567"

# A stand-in for the `scan_vs_index` binary: the same argument surface as the
# Rust binary (unknown flags are refused), writing one `.txt` corpus file so
# `rg`/`grep` have bytes to scan, and printing a schema-2 artifact.
FAKE_BIN = """\
import json, os, sys
args = sys.argv[1:]
known = {"--out-dir", "--index-dir", "--artifact-out", "--chunks", "--chunk-bytes",
         "--needle-count", "--samples"}
flags = dict(zip(args[0::2], args[1::2]))
unknown = [flag for flag in args[0::2] if flag not in known]
if unknown or len(args) % 2:
    sys.stderr.write("scan_vs_index: unknown argument " + repr(unknown) + "\\n")
    sys.exit(1)
out_dir = flags["--out-dir"]
os.makedirs(out_dir, exist_ok=True)
with open(os.path.join(out_dir, "part-000000.txt"), "w") as f:
    f.write("parity_needle_alpha filler\\n")
chunks = int(flags["--chunks"])
head = os.environ.get("FAKE_HEAD", "__HEAD__")
if os.environ.get("FAKE_SPLIT_HEADS") and int(flags["--chunks"]) > 2:
    head = "fedcba9876543210fedcba9876543210fedcba98"
schema = int(os.environ.get("FAKE_SCHEMA", "2"))
print(json.dumps({
    "schema_version": schema, "dimension": "scan-vs-index", "mode": "warm", "concurrency": 1,
    "provenance": {"git_head": head, "corpus_digest": "sha256:" + "ab" * 32,
                   "config_digest": "sha256:" + "cd" * 32, "model_revision": None},
    "host": {"os": "linux", "arch": "x86_64", "cpu_count": 4, "mem_bytes": 2 ** 33,
             "hostname_hash": "sha256:" + "ef" * 32},
    "resources": {"peak_rss_bytes": 1}, "phases": {"build_ms": 5.0, "update_ms": None, "gc_ms": None},
    "disk_amplification": {"bytes_written": 20, "changed_bytes": 10, "ratio": 2.0},
    "rows": [{"scenario_id": "scan-vs-index.chunks" + str(chunks) + ".index_query", "route_family": "lexical",
              "syntax": "native", "result_shape": "candidates",
              "latency": {"p50_ms": 0.1, "p95_ms": 0.2, "p99_ms": 0.3, "samples": 5},
              "qps": None, "error_count": 0, "timeout_count": 0, "result_count": 10,
              "typed_error_code": None, "engine_touched": ["lexical-adapter-in-process"],
              "early_stop_reason": None}],
    "detail": {"chunks": chunks, "corpus_bytes": chunks * 10, "index_bytes": 20,
               "index_build_ms": 5.0, "needle_count": 10, "index_hits": 10, "files": 1,
               "index_query_samples": 5},
}))
""".replace("__HEAD__", HEAD)


def _write_fake(tmp_path: Path) -> str:
    fake = tmp_path / "fake_scan_vs_index.py"
    fake.write_text(FAKE_BIN, encoding="utf-8")
    return f"{shlex.quote(sys.executable)} {shlex.quote(str(fake))}"


def _run(
    tmp_path: Path, bin_cmd: str, env: dict[str, str] | None = None
) -> subprocess.CompletedProcess[str]:
    import os

    return subprocess.run(
        [
            sys.executable,
            str(RUNNER_PATH),
            "--scales",
            "2,4",
            "--index-samples",
            "2",
            "--scan-samples",
            "1",
            "--out",
            str(tmp_path / "report" / "scan-vs-index.md"),
            "--artifact-dir",
            str(tmp_path / "report" / "scan-vs-index"),
            "--bin-cmd",
            bin_cmd,
        ],
        cwd=REPO_ROOT,
        check=False,
        capture_output=True,
        text=True,
        env={**os.environ, **(env or {})},
    )


def test_the_runner_drives_the_binary_without_a_fingerprint_and_keeps_artifacts(
    tmp_path: Path,
) -> None:
    result = _run(tmp_path, _write_fake(tmp_path))
    assert result.returncode == 0, result.stdout + result.stderr
    report = (tmp_path / "report" / "scan-vs-index.md").read_text(encoding="utf-8")
    assert f"git_head: `{HEAD}`" in report
    assert "|      2 |" in report and "|      4 |" in report
    for chunks in (2, 4):
        kept = json.loads(
            (tmp_path / "report" / "scan-vs-index" / f"chunks-{chunks}.json").read_text()
        )
        assert kept["schema_version"] == 2
        assert kept["provenance"]["git_head"] == HEAD
        assert kept["detail"]["chunks"] == chunks


def test_the_runner_refuses_a_sweep_across_two_heads(tmp_path: Path) -> None:
    result = _run(tmp_path, _write_fake(tmp_path), env={"FAKE_SPLIT_HEADS": "1"})
    assert result.returncode == 2, result.stdout + result.stderr
    assert "more than one head" in result.stderr
    assert not (tmp_path / "report" / "scan-vs-index.md").exists()


def test_the_runner_refuses_an_old_schema_artifact(tmp_path: Path) -> None:
    result = _run(tmp_path, _write_fake(tmp_path), env={"FAKE_SCHEMA": "1"})
    assert result.returncode != 0
    assert "schema_version 1" in result.stderr


def test_the_runner_refuses_a_binary_that_demands_a_fingerprint(tmp_path: Path) -> None:
    # The old binary's behaviour: refuse without --source-fingerprint. The
    # runner must surface that as a failure, never invent a label.
    fake = tmp_path / "old_scan_vs_index.py"
    fake.write_text(
        'import sys\nsys.stderr.write("scan_vs_index: --source-fingerprint is required\\n")\nsys.exit(1)\n',
        encoding="utf-8",
    )
    result = _run(tmp_path, f"{shlex.quote(sys.executable)} {shlex.quote(str(fake))}")
    assert result.returncode != 0
    assert "--source-fingerprint is required" in result.stderr


def test_the_runner_module_loads_and_names_no_fingerprint_flag() -> None:
    spec = importlib.util.spec_from_file_location("run_scan_vs_index", RUNNER_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["run_scan_vs_index"] = module
    spec.loader.exec_module(module)
    assert "source-fingerprint" not in RUNNER_PATH.read_text(encoding="utf-8")
    assert module.CURRENT_SCHEMA_VERSION == 2
