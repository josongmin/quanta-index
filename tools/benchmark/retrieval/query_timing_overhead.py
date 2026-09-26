"""Compare explicit on/off query clock captures without qualifying performance.

Run identical frozen runner inputs with --query-stage-observation enabled/disabled, a fresh
state root for each, and the same query protocol. This replay refuses differing
results, model/input identities or repetition coverage. Values describe these
samples only; host quietness and interleaved repetitions remain separate gates.
"""
from __future__ import annotations

import argparse
import math
import statistics
from pathlib import Path

from tools.benchmark.retrieval import run as pairrun
from tools.benchmark.retrieval.conditional_proof import load, sha


def compare(on: dict, off: dict, on_phases: dict, off_phases: dict, on_diagnostic: dict, off_diagnostic: dict, pack: dict) -> dict:
    for record, phases, diagnostic, policy in ((on, on_phases, on_diagnostic, "enabled"), (off, off_phases, off_diagnostic, "disabled")):
        if record.get("schema_version") != 5 or record.get("span_accounting_version") != 1 \
            or diagnostic.get("schema_version") not in (5, 6) or phases.get("system") != "quanta" or not phases.get("query_protocol"):
            raise ValueError("overhead comparison requires v5 records and explicit on/off protocol")
        if diagnostic.get("server_observation") != pairrun.server_observation_configuration(policy):
            raise ValueError("overhead capture lacks its actual server observation policy")
        pairrun._validate_phase_metrics(phases, "overhead phase metrics")
        pairrun.validate_retrieval_diagnostic(diagnostic, record, phases["record_sha256"], pack)
    if on_diagnostic["schema_version"] != off_diagnostic["schema_version"] or (
        on_diagnostic["schema_version"] == 6
        and on_diagnostic["hybrid_fetch_policy"] != off_diagnostic["hybrid_fetch_policy"]
    ):
        raise ValueError("on/off hybrid fetch policy or diagnostic version differs")
    for key in ("strategy", "query_schedule", "query_protocol", "task_count", "route_count", "measurement_repetitions", "warmup_passes",
                "runner_binary_sha256", "file_count", "chunk_count", "symbol_count",
                "symbol_coverage", "symbol_unsupported_details"):
        if key not in on_phases or on_phases[key] != off_phases.get(key):
            raise ValueError(f"on/off frozen configuration differs: {key}")
    def identities(record):
        return sorted((capture["chunk_strategy"], str(capture["chunk_config"]),
                       capture["model"], capture["model_revision"],
                       capture["execution_profile_sha256"], capture["runner_binary"]["digest"],
                       capture["searchd_binary"]["binary_digest"])
                      for capture in record["captures"].values())
    if not on.get("captures") or identities(on) != identities(off) \
        or on.get("query_pack_sha256") != off.get("query_pack_sha256") \
        or not on.get("query_pack_sha256"):
        raise ValueError("on/off model/daemon/input identity differs")
    def answers(record):
        return [(row["route"], row["task_id"], row["status"], row["error"], row["candidates"])
                for row in record["results"]]
    if not on.get("results") or answers(on) != answers(off) \
        or any(row["status"] not in ("success", "capped", "abstained") for row in on["results"]):
        raise ValueError("on/off answer or ranking differs, or execution failed")
    summaries = []
    for route, tasks in on_phases["warm_latencies_ms"].items():
        if route not in off_phases["warm_latencies_ms"]:
            raise ValueError("off capture lacks measured route")
        for task_id, measured_on in tasks.items():
            measured_off = off_phases["warm_latencies_ms"][route].get(task_id)
            repetitions = on_phases["measurement_repetitions"]
            if not isinstance(measured_off, list) or len(measured_on) != repetitions or len(measured_off) != repetitions or repetitions < 2 \
                or any(type(value) not in (int, float) or not math.isfinite(value) or value <= 0
                       for value in measured_on + measured_off):
                raise ValueError("on/off sample coverage is missing or invalid")
            left, right = statistics.median(measured_on), statistics.median(measured_off)
            summaries.append({"route": route, "task_id": task_id, "samples_each": repetitions,
                "on_median_ms": left, "off_median_ms": right, "delta_ms": left-right,
                "relative_delta": left/right-1})
    on_keys = {(row["route"], row["task_id"]) for row in summaries}
    off_keys = {(route, task) for route, tasks in off_phases["warm_latencies_ms"].items() for task in tasks}
    expected = {(row["route"], row["task_id"]) for row in on["results"]}
    if len(on_keys) != len(summaries) or on_keys != off_keys or on_keys != expected:
        raise ValueError("on/off task inventory differs from actual executed rows")
    return {"schema_version": 1, "status": "diagnostic_unqualified",
            "scorer_identity": "query-stage-clock-overhead-v1", "rows": summaries}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("on-record", "off-record", "on-phases", "off-phases", "on-diagnostic", "off-diagnostic", "pack", "out"):
        parser.add_argument(f"--{name}", type=Path, required=True)
    args = parser.parse_args()
    try:
        paths = [args.on_record, args.off_record, args.on_phases, args.off_phases, args.on_diagnostic, args.off_diagnostic, args.pack]
        payloads = [path.read_bytes() for path in paths]
        phases = [load(payload) for payload in payloads[2:4]]
        if any(phase["record_sha256"] != sha(raw) for phase, raw in zip(phases, payloads[:2], strict=True)):
            raise ValueError("on/off phase record digest mismatch")
        result = compare(*(load(payload) for payload in payloads))
        from tools.benchmark.retrieval.conditional_proof import canonical
        with args.out.open("xb") as stream:
            stream.write(canonical({**result, "input_sha256": [sha(raw) for raw in payloads]}) + b"\n")
    except (ValueError, OSError, KeyError, TypeError, pairrun.RunError) as error:
        parser.exit(2, f"overhead replay refused: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
