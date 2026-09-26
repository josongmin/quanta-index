#!/usr/bin/env python3
"""Scaling experiment: indexed keyword query vs full-text scan, vs corpus size.

This is an **exploratory experiment, not a benchmark gate**. It is deliberately
NOT part of the JUN-08-001 3-layer model and writes nothing to the
committed baselines. The ADR forbids reporting DSL query latency against a
text-only engine *as a benchmark* because a daemon IPC round-trip and a `grep`
process answer different questions. This tool isolates the one comparison that
IS meaningful — how each approach scales with corpus size — by:

  * measuring the lexical index query **in-process** (the `scan_vs_index` Rust
    binary; no daemon, no IPC), so the number is the index lookup itself; and
  * timing `rg` / `grep` over the identical corpus bytes the binary writes.

It demonstrates: index query latency is ~flat in corpus size while a full scan
is linear, so there is a crossover beyond which the index wins per query. The
index's one-time build cost is reported separately (it is amortized over many
queries; a scan pays its full cost every time).

Provenance (QI-BB-010): the binary emits one ``BenchArtifactV1`` per scale —
the exact head of a clean worktree (resolved by the binary, never passed in),
the corpus digest over the bytes it wrote, the run configuration digest, the
host, its peak RSS, the build phase and the index's disk amplification. This
runner keeps every artifact verbatim under ``artifacts/experiments/scan-vs-index/``
beside the markdown report and adds only the scan timings, so the numbers in
the table are attributable to the head the artifacts name.
"""

from __future__ import annotations

import argparse
import json
import shlex
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path

NEEDLE = "parity_needle_alpha"
CURRENT_SCHEMA_VERSION = 2


@dataclass(frozen=True)
class ScanResult:
    p50_ms: float
    p95_ms: float


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--scales",
        default="2000,20000,100000",
        help="comma-separated chunk counts to sweep (default 2000,20000,100000)",
    )
    parser.add_argument("--chunk-bytes", type=int, default=512)
    parser.add_argument("--needle-count", type=int, default=10)
    parser.add_argument("--index-samples", type=int, default=50)
    parser.add_argument("--scan-samples", type=int, default=30)
    parser.add_argument(
        "--out",
        type=Path,
        default=Path("artifacts/experiments/scan-vs-index.md"),
        help="markdown report path (under gitignored artifacts/)",
    )
    parser.add_argument(
        "--artifact-dir",
        type=Path,
        default=Path("artifacts/experiments/scan-vs-index"),
        help="directory the per-scale BenchArtifactV1 files are kept in verbatim",
    )
    parser.add_argument(
        "--bin-cmd",
        default="cargo run --quiet --release -p quanta-index-scan-experiment --",
        help="command prefix that invokes the scan_vs_index binary",
    )
    return parser.parse_args()


def percentile(samples_ms: list[float], pct: float) -> float:
    if not samples_ms:
        raise ValueError("percentile requires at least one sample")
    ordered = sorted(samples_ms)
    rank = max(1, -(-pct * len(ordered) // 100))  # ceil(pct/100 * n)
    return ordered[min(len(ordered), rank) - 1]


def time_command(cmd: list[str], samples: int) -> ScanResult:
    subprocess.run(cmd, capture_output=True, check=False)  # warm fs cache
    times: list[float] = []
    for _ in range(samples):
        start = time.perf_counter()
        subprocess.run(cmd, capture_output=True, check=False)
        times.append((time.perf_counter() - start) * 1000.0)
    return ScanResult(p50_ms=percentile(times, 50), p95_ms=percentile(times, 95))


def run_index_probe(
    bin_cmd: list[str], out_dir: Path, chunks: int, args: argparse.Namespace
) -> dict:
    """One scale through the binary; the artifact it prints, checked for shape."""
    cmd = [
        *bin_cmd,
        "--out-dir",
        str(out_dir),
        "--chunks",
        str(chunks),
        "--chunk-bytes",
        str(args.chunk_bytes),
        "--needle-count",
        str(args.needle_count),
        "--samples",
        str(args.index_samples),
    ]
    proc = subprocess.run(cmd, capture_output=True, text=True, check=False)
    if proc.returncode != 0:
        raise RuntimeError(f"scan_vs_index failed:\n{proc.stderr.strip()}")
    artifact = json.loads(proc.stdout.strip().splitlines()[-1])
    if artifact.get("schema_version") != CURRENT_SCHEMA_VERSION:
        raise RuntimeError(
            f"scan_vs_index emitted schema_version {artifact.get('schema_version')!r}, "
            f"expected {CURRENT_SCHEMA_VERSION}"
        )
    for key in ("provenance", "host", "resources", "rows", "detail"):
        if key not in artifact:
            raise RuntimeError(f"scan_vs_index artifact has no `{key}`")
    return artifact


def table_row(artifact: dict, rg: ScanResult, grep: ScanResult) -> dict:
    detail = artifact["detail"]
    latency = artifact["rows"][0]["latency"]
    return {
        "chunks": detail["chunks"],
        "corpus_bytes": detail["corpus_bytes"],
        "index_bytes": detail["index_bytes"],
        "index_build_ms": detail["index_build_ms"],
        "index_query_p50_ms": latency["p50_ms"],
        "index_query_p95_ms": latency["p95_ms"],
        "index_query_p99_ms": latency["p99_ms"],
        "rg_p50_ms": rg.p50_ms,
        "rg_p95_ms": rg.p95_ms,
        "grep_p50_ms": grep.p50_ms,
        "grep_p95_ms": grep.p95_ms,
    }


def render_report(rows: list[dict], crossover: str, git_head: str, host: dict) -> str:
    lines = [
        "# Scan vs index — scaling experiment (NOT a benchmark gate)",
        "",
        "In-process lexical index query (no daemon/IPC) vs `rg`/`grep` full scan,",
        f"searching `{NEEDLE}` over identical corpus bytes. See the script header",
        "for why this is an experiment and not part of the DSL latency gate.",
        "",
        f"- git_head: `{git_head}`",
        f"- host: {host['os']}/{host['arch']}, {host['cpu_count']} cpus, "
        f"{host['mem_bytes'] / (1024**3):.1f} GiB, hostname_hash `{host['hostname_hash'][:23]}…`",
        "- per-scale `BenchArtifactV1` files (corpus/config digests, peak RSS, disk"
        " amplification) are beside this report",
        "",
        "| chunks | corpus MB | index build ms | index query p95 ms | rg p95 ms | grep p95 ms |",
        "| ------:| ---------:| --------------:| ------------------:| ---------:| -----------:|",
    ]
    for r in rows:
        mb = r["corpus_bytes"] / (1024 * 1024)
        lines.append(
            f"| {r['chunks']:>6} | {mb:>9.1f} | {r['index_build_ms']:>14.1f} "
            f"| {r['index_query_p95_ms']:>18.3f} | {r['rg_p95_ms']:>9.2f} "
            f"| {r['grep_p95_ms']:>11.2f} |"
        )
    lines += ["", f"**Crossover:** {crossover}", ""]
    return "\n".join(lines)


def main() -> int:
    args = parse_args()
    bin_cmd = shlex.split(args.bin_cmd)
    scales = [int(s) for s in args.scales.split(",") if s.strip()]

    rows: list[dict] = []
    heads: set[str] = set()
    host: dict | None = None
    args.artifact_dir.mkdir(parents=True, exist_ok=True)
    for chunks in scales:
        with tempfile.TemporaryDirectory(prefix="scan-vs-index-") as tmp:
            out_dir = Path(tmp)
            print(f"scale={chunks}: building corpus + index ...", file=sys.stderr)
            artifact = run_index_probe(bin_cmd, out_dir, chunks, args)
            heads.add(artifact["provenance"]["git_head"])
            host = artifact["host"]
            (args.artifact_dir / f"chunks-{chunks}.json").write_text(
                json.dumps(artifact, indent=2) + "\n", encoding="utf-8"
            )
            rg = time_command(
                ["rg", "--no-messages", "-g", "*.txt", NEEDLE, str(out_dir)],
                args.scan_samples,
            )
            grep = time_command(
                ["grep", "-rn", "--include=*.txt", NEEDLE, str(out_dir)],
                args.scan_samples,
            )
            row = table_row(artifact, rg, grep)
            rows.append(row)
            print(
                f"  corpus={row['corpus_bytes'] / 1048576:.1f}MB "
                f"index_build={row['index_build_ms']:.0f}ms "
                f"index_q_p95={row['index_query_p95_ms']:.3f}ms "
                f"rg_p95={rg.p95_ms:.2f}ms grep_p95={grep.p95_ms:.2f}ms",
                file=sys.stderr,
            )

    if len(heads) != 1 or host is None:
        print(
            f"ERROR: the sweep spans more than one head or no scale ran: {sorted(heads)}",
            file=sys.stderr,
        )
        return 2
    crossover = "index query p95 stayed below rg at every scale measured"
    for r in rows:
        if r["index_query_p95_ms"] < r["rg_p95_ms"]:
            crossover = (
                f"index query ({r['index_query_p95_ms']:.3f}ms) beats rg "
                f"({r['rg_p95_ms']:.2f}ms) at >= {r['chunks']} chunks "
                f"({r['corpus_bytes'] / 1048576:.1f}MB)"
            )
            break

    report = render_report(rows, crossover, next(iter(heads)), host)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(report + "\n", encoding="utf-8")
    print(report)
    print(
        f"\nwrote {args.out} and {len(rows)} artifact(s) under {args.artifact_dir}", file=sys.stderr
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
