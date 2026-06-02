#!/usr/bin/env python3
"""Scaling experiment: indexed keyword query vs full-text scan, vs corpus size.

This is an **exploratory experiment, not a benchmark gate**. It is deliberately
NOT part of the RFC-DSL-Benchmarking 3-layer model and writes nothing to the
committed baselines. The RFC forbids reporting DSL query latency against a
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
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path

NEEDLE = "parity_needle_alpha"


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
        "--bin-cmd",
        default="cargo run --quiet --release -p quanta-index-scan-experiment --",
        help="command prefix that invokes the scan_vs_index binary",
    )
    return parser.parse_args()


def percentile(samples_ms: list[float], pct: float) -> float:
    if not samples_ms:
        return 0.0
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


def run_index_probe(bin_cmd: list[str], out_dir: Path, args: argparse.Namespace) -> dict:
    cmd = [
        *bin_cmd,
        "--out-dir",
        str(out_dir),
        "--chunks",
        str(args.chunks),
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
    return json.loads(proc.stdout.strip().splitlines()[-1])


def render_report(rows: list[dict], crossover: str) -> str:
    lines = [
        "# Scan vs index — scaling experiment (NOT a benchmark gate)",
        "",
        "In-process lexical index query (no daemon/IPC) vs `rg`/`grep` full scan,",
        f"searching `{NEEDLE}` over identical corpus bytes. See the script header",
        "for why this is an experiment and not part of RFC-DSL-Benchmarking.",
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
    bin_cmd = args.bin_cmd.split()
    scales = [int(s) for s in args.scales.split(",") if s.strip()]

    rows: list[dict] = []
    for chunks in scales:
        args.chunks = chunks
        with tempfile.TemporaryDirectory(prefix="scan-vs-index-") as tmp:
            out_dir = Path(tmp)
            print(f"scale={chunks}: building corpus + index ...", file=sys.stderr)
            row = run_index_probe(bin_cmd, out_dir, args)
            rg = time_command(
                ["rg", "--no-messages", "-g", "*.txt", NEEDLE, str(out_dir)],
                args.scan_samples,
            )
            grep = time_command(
                ["grep", "-rn", "--include=*.txt", NEEDLE, str(out_dir)],
                args.scan_samples,
            )
            row["rg_p50_ms"] = rg.p50_ms
            row["rg_p95_ms"] = rg.p95_ms
            row["grep_p50_ms"] = grep.p50_ms
            row["grep_p95_ms"] = grep.p95_ms
            rows.append(row)
            print(
                f"  corpus={row['corpus_bytes'] / 1048576:.1f}MB "
                f"index_build={row['index_build_ms']:.0f}ms "
                f"index_q_p95={row['index_query_p95_ms']:.3f}ms "
                f"rg_p95={rg.p95_ms:.2f}ms grep_p95={grep.p95_ms:.2f}ms",
                file=sys.stderr,
            )

    crossover = "index query p95 stayed below rg at every scale measured"
    for r in rows:
        if r["index_query_p95_ms"] < r["rg_p95_ms"]:
            crossover = (
                f"index query ({r['index_query_p95_ms']:.3f}ms) beats rg "
                f"({r['rg_p95_ms']:.2f}ms) at >= {r['chunks']} chunks "
                f"({r['corpus_bytes'] / 1048576:.1f}MB)"
            )
            break

    report = render_report(rows, crossover)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(report + "\n", encoding="utf-8")
    print(report)
    print(f"\nwrote {args.out}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
