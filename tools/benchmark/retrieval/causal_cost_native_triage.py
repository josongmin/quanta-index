"""Inspect an existing Native capture or product control directory offline."""

from __future__ import annotations

import hashlib
import math
from pathlib import Path
from typing import Any

from tools.benchmark.evidence import parse_json, read_control
from tools.benchmark.retrieval.causal_cost_native_ingest import build_native_ingest_report
from tools.benchmark.retrieval.causal_cost_native_refusal import find_native_refusal
from tools.benchmark.retrieval.causal_cost_native_staging import build_native_staging_report
from tools.benchmark.retrieval.causal_cost_timeline import (
    MAX_CAPTURE_BYTES,
    MAX_CONFIGURED_CAPTURE_BYTES,
    parse_stream,
)

TIERS = ("small", "medium", "large", "xlarge")
NATIVE_SUMMARY_BACKEND = "index-native-configured-daemon-uds"
PRIMARY_PHASES = (
    "startup",
    "profile_configure",
    "full_begin_input",
    "full_stage_sources",
    "full_event_ack",
    "first_search",
    "delta_event_ack",
    "noop_event_ack",
    "delete_event_ack",
    "reopen",
    "reopen_search",
)
EXECUTION_FIELDS = (
    "status",
    "product_status",
    "exit_code",
    "timed_out",
    "elapsed_seconds",
    "source_revision",
    "source_revision_after",
    "dirty_after",
    "binary_sha256",
    "native_daemon_sha256",
    "build_binding",
    "native_trace_mode",
    "reason",
    "producer_error",
)
CONTROL_FIELDS = ("status", "reason", "formal_timing_qualification", "diagnostic_only")
SETTLEMENT_BLOCKERS = frozenset(
    {"Incomplete"}
    | {
        f"Pending({name})"
        for name in (
            "TargetPin",
            "ExactFloor",
            "WorkBelowFloor",
            "TerminalWork",
            "TerminalCustody",
            "UnretiredView",
            "ReclaimQueue",
            "ObjectLifecycle",
            "UnreferencedObject",
        )
    }
)


def _object(value: object) -> dict:
    return value if isinstance(value, dict) else {}


def _read_bound(path: Path, issues: list[str], digest: str | None) -> bytes | None:
    if not path.exists():
        return None
    try:
        raw = read_control(path)
        if digest is not None and hashlib.sha256(raw).hexdigest() != digest:
            raise ValueError("SHA-256 differs from execution record")
        return raw
    except (OSError, ValueError) as error:
        issues.append(f"{path}: {error}")
        return None


def _record(path: Path, issues: list[str], *, digest: str | None = None) -> dict | None:
    raw = _read_bound(path, issues, digest)
    if raw is None:
        return None
    try:
        result = parse_json(raw.decode("utf-8"))
        if not isinstance(result, dict):
            raise ValueError("expected a JSON object")
        return result
    except ValueError as error:
        issues.append(f"{path}: {error}")
        return None


def _argument(execution: dict, name: str) -> str | None:
    command = execution.get("command")
    if isinstance(command, list) and command.count(name) == 1:
        index = command.index(name) + 1
        if index < len(command) and isinstance(command[index], str):
            return command[index]
    return None


def _phase_rows(phases: dict, names: tuple[str, ...]) -> dict:
    result = {}
    for name in names:
        row = phases.get(name)
        if not isinstance(row, dict):
            continue
        wall = row.get("wall_ms")
        if (
            type(wall) in (int, float)
            and 0 <= wall < math.inf
            and type(row.get("completed")) is bool
        ):
            projected = {"wall_ms": wall, "completed": row["completed"]}
            observer = row.get("observer_ms")
            if type(observer) in (int, float) and 0 <= observer < math.inf:
                projected["observer_ms"] = observer
            result[name] = projected
    return result


def _bounded_text(value: object, *, max_bytes: int = 256) -> str | None:
    if not isinstance(value, str):
        return None
    clean = " ".join("".join(char if char.isprintable() else " " for char in value).split())
    encoded = clean.encode("utf-8")
    if len(encoded) > max_bytes:
        return encoded[:max_bytes].decode("utf-8", errors="ignore") + "..."
    return clean or None


def _nonnegative_number(value: object) -> int | float | None:
    if type(value) in (int, float) and math.isfinite(value) and value >= 0:
        return value
    return None


def _nonnegative_int(value: object) -> int | None:
    return value if type(value) is int and value >= 0 else None


def _settlement_samples(raw: str, target: int, issues: list[str]) -> dict | None:
    """Keep the sampled reasons for this view sequence and the last log sample."""
    observed: dict[str, None] = {}
    last = None
    for line in raw.splitlines():
        if not line.startswith("{") or '"native_read_settlement_blocked"' not in line:
            continue
        try:
            row = _object(parse_json(line)) if len(line) <= 4096 else {}
        except ValueError:
            issues.append("malformed settlement blocker sample in harness stderr")
            continue
        if (
            row.get("event") != "native_read_settlement_blocked"
            or _nonnegative_int(row.get("target_sequence")) != target
        ):
            continue
        status = row.get("status")
        if not isinstance(status, str) or status not in SETTLEMENT_BLOCKERS:
            continue
        observed[status] = None
        last = {
            "status": status,
            **{
                key: _nonnegative_int(row.get(key))
                for key in (
                    "head_sequence",
                    "exact_floor",
                    "oldest_ready_sequence",
                )
            },
        }
    return (
        {"target_sequence": target, "observed": list(observed), "last_logged": last}
        if last
        else None
    )


def _mixed_failure(value: object) -> dict | None:
    if not isinstance(value, dict):
        return None
    retained = _object(value.get("retained_read"))
    settled_value = retained.get("settled_ms")
    settled_field_present = "settled_ms" in retained and (
        settled_value is None or _nonnegative_number(settled_value) is not None
    )
    result = {
        "first_failure_owner": _bounded_text(value.get("first_failure_owner"), max_bytes=64),
        "held_worker_error": _bounded_text(value.get("held_worker_error")),
        "pin_status": _bounded_text(retained.get("last_status"), max_bytes=64),
        "confirmed_last_pin": retained.get("confirmed_last_pin")
        if type(retained.get("confirmed_last_pin")) is bool
        else None,
        "confirmed_release_ms": _nonnegative_number(retained.get("confirmed_release_ms")),
        "last_settlement_state": _bounded_text(retained.get("last_settlement_state"), max_bytes=64),
        "settlement_polls": _nonnegative_int(retained.get("settlement_polls")),
        "settlement_head_sequence": _nonnegative_int(retained.get("settlement_head_sequence")),
        "settlement_exact_floor": _nonnegative_int(retained.get("settlement_exact_floor")),
        "settlement_has_remaining_target_pins": retained.get("settlement_has_remaining_target_pins")
        if type(retained.get("settlement_has_remaining_target_pins")) is bool
        else None,
        "settled_ms": _nonnegative_number(settled_value),
        "settled_field_present": settled_field_present,
    }
    has_observation = (
        any(item is not None for key, item in result.items() if key != "settled_field_present")
        or settled_field_present
    )
    return result if has_observation else None


def _trace_report(
    path: Path | None, mode: object, include_trace: bool, cap: int, binding: object
) -> dict:
    if mode == "off":
        return {"status": "OFF"}
    if path is None or not path.is_file():
        return {"status": "MISSING"}
    if path.parent.is_symlink() or path.is_symlink():
        return {"status": "INVALID", "error": "daemon trace path contains a symlink"}
    result = {"path": str(path), "bytes": path.stat().st_size}
    if not include_trace:
        return {**result, "status": "AVAILABLE"}
    if result["bytes"] > cap:
        return {**result, "status": "OVER_LIMIT", "max_capture_bytes": cap}
    try:
        with path.open("rb") as handle:
            parsed = parse_stream(handle, max_capture_bytes=cap)
        ingest = build_native_ingest_report(parsed, limit=3)
    except (OSError, ValueError) as error:
        return {**result, "status": "INVALID", "error": str(error)}
    requests = []
    for row in ingest["requests"]:
        stages = row["stages"]
        closed = sorted(
            (
                (stage, group["union_ns"])
                for stage, group in stages.items()
                if group["union_ns"] is not None
            ),
            key=lambda pair: (-pair[1], pair[0]),
        )[:5]
        requests.append(
            {
                "request_span_id": row["request_span_id"],
                "outcome": row["outcome"],
                "wall_ns": row["wall_ns"],
                "open_stages": [
                    stage
                    for stage, group in stages.items()
                    if group["count"] > group["closed_count"]
                ],
                "largest_stage_wall_unions_ns": dict(closed),
            }
        )
    if binding is None:
        binding_status = "UNBOUND"
        mismatches: list[str] = []
    else:
        bound = _object(binding)
        mismatches = [
            field
            for field, observed in (
                ("trace_path", str(path)),
                ("trace_bytes", result["bytes"]),
                ("trace_sha256", ingest["trace_sha256"]),
                ("capture_id", ingest["capture_id"]),
            )
            if type(bound.get(field)) is not type(observed) or bound.get(field) != observed
        ]
        binding_status = "MISMATCH" if mismatches else "BOUND"
    if binding_status == "UNBOUND":
        trace_status = "UNBOUND_DIAGNOSTIC"
    elif binding_status == "MISMATCH":
        trace_status = "BINDING_MISMATCH"
    elif ingest["status"] == "COMPLETE_DIAGNOSTIC":
        trace_status = "BOUND_COMPLETE_DIAGNOSTIC"
    else:
        trace_status = "BOUND_PARTIAL_DIAGNOSTIC"
    return {
        **result,
        "status": trace_status,
        "parse_status": ingest["status"],
        "capture_status": parsed["status"],
        "binding_status": binding_status,
        "binding_mismatch_fields": mismatches,
        "capture_id": ingest["capture_id"],
        "trace_sha256": ingest["trace_sha256"],
        "event_count": parsed["event_count"],
        "truncated_tail": parsed["truncated_tail"],
        "loss": {
            key: (parsed["footer"] or {}).get(key)
            for key in ("events_dropped", "events_unwritten", "open_spans")
        },
        "requests": requests if binding_status != "MISMATCH" else [],
        "matched_requests": ingest["matched_requests"] if binding_status != "MISMATCH" else None,
        "open_requests": ingest["open_requests"] if binding_status != "MISMATCH" else None,
    }


def build_native_triage_report(
    run_dir: Path, *, include_trace: bool = False, max_capture_bytes: int = MAX_CAPTURE_BYTES
) -> dict[str, Any]:
    """Report process, product, and capture observations from their saved authorities."""
    if not 1 <= max_capture_bytes <= MAX_CONFIGURED_CAPTURE_BYTES:
        raise ValueError(f"max capture bytes must be between 1 and {MAX_CONFIGURED_CAPTURE_BYTES}")
    supplied = run_dir.resolve()
    if supplied.name.endswith("-control"):
        root, control = supplied.with_name(supplied.name.removesuffix("-control")), supplied
    else:
        root, control = supplied, supplied.with_name(supplied.name + "-control")
    if not root.is_dir() and not control.is_dir():
        raise ValueError(f"Native run directory does not exist: {supplied}")
    issues: list[str] = []
    execution = _record(root / "execution.json", issues)
    terminal = _record(control / "terminal.json", issues)
    preflight = _record(control / "host-preflight.json", issues)
    if preflight and (
        preflight.get("kind") != "quanta-index-timing-preflight"
        or preflight.get("run_id") != root.name
    ):
        issues.append("host preflight identity differs from run directory")
        preflight = None
    if terminal and terminal.get("run") != root.name:
        issues.append("control terminal identity differs from run directory")
        terminal = None
    if execution and execution.get("backend") != "index-native":
        raise ValueError(f"execution is not a Native capture: {root}")
    execution = execution or {}
    tier = _argument(execution, "--tier") or (terminal or {}).get("tier")
    tier = tier if tier in TIERS else None
    mode = execution.get("native_trace_mode")
    result: dict[str, Any] = {
        "run_dir": str(root),
        "tier": tier,
        "execution": {key: execution.get(key) for key in EXECUTION_FIELDS} if execution else None,
        "control_terminal": {key: terminal.get(key) for key in CONTROL_FIELDS}
        if terminal
        else None,
        "host_preflight": (
            {
                "status": preflight.get("status"),
                **{
                    key: _object(preflight.get("host_contention")).get(key)
                    for key in ("one_minute_load", "one_minute_load_limit", "over_limit")
                },
                "foreign_rust_count": len(preflight["foreign_rust_processes"])
                if isinstance(preflight.get("foreign_rust_processes"), list)
                else None,
            }
            if preflight
            else None
        ),
        "phase_source": None,
        "phases": {},
        "failed_phase": None,
        "mixed_failure": None,
        "publication": None,
        "refusal": None,
        "staging": None,
        "issues": issues,
    }

    verified = (
        execution.get("product_status") == "PRODUCT_ARTIFACT_VERIFIED"
        and type(execution.get("exit_code")) is int
        and execution["exit_code"] == 0
        and execution.get("timed_out") is False
    )
    summary_path = root / "artifact/summary.json"
    manifest_path = root / "artifact/tier_manifest.json"
    refusal_path = root / "artifact/refusal.json"
    artifact, phase_data, failure = None, {}, {}
    if verified:
        summary_digest = execution.get("summary_sha256")
        manifest_digest = execution.get("tier_manifest_sha256")
        if isinstance(summary_digest, str) and summary_digest:
            artifact = _record(summary_path, issues, digest=summary_digest)
        if not artifact:
            issues.append("product summary unavailable or unbound")
        manifest = None
        if isinstance(manifest_digest, str) and manifest_digest:
            manifest = _record(manifest_path, issues, digest=manifest_digest)
        if not manifest:
            issues.append("tier manifest unavailable or unbound")
        if artifact and manifest:
            detail = _object(artifact.get("detail"))
            summary_tier = detail.get("tier")
            manifest_tiers = manifest.get("tiers")
            matching_tiers = (
                [
                    row
                    for row in manifest_tiers
                    if isinstance(row, dict) and row.get("tier") == summary_tier
                ]
                if isinstance(manifest_tiers, list)
                else []
            )
            if (
                not isinstance(execution.get("source_revision"), str)
                or _object(artifact.get("provenance")).get("git_head")
                != execution.get("source_revision")
                or detail.get("backend") != NATIVE_SUMMARY_BACKEND
                or summary_tier not in TIERS
                or (tier is not None and tier != summary_tier)
                or manifest.get("kind") != "quanta-index-scale-tier-manifest"
                or manifest.get("dimension") != "scale"
                or manifest.get("manifest_schema_version") != 2
                or len(matching_tiers) != 1
                or type(matching_tiers[0].get("total_files")) is not int
                or matching_tiers[0]["total_files"] <= 0
                or (
                    type(detail.get("file_count")) is int
                    and detail["file_count"] != matching_tiers[0]["total_files"]
                )
            ):
                issues.append("product source, tier or manifest shape differs from execution")
            else:
                if tier is None:
                    result["tier"] = summary_tier
                phase_data = _object(detail.get("phases"))
                result["phase_source"] = "product_summary"
                result["publication"] = _publication(
                    detail.get("publication_outcomes"), "full_event_ack"
                )
    elif execution and refusal_path.exists():
        artifact = _record(refusal_path, issues)
        if artifact and (
            artifact.get("kind") != "quanta-index-scale-index-native-failure"
            or not isinstance(execution.get("source_revision"), str)
            or _object(artifact.get("provenance")).get("git_head")
            != execution.get("source_revision")
        ):
            issues.append("failure artifact kind or source revision differs from execution")
            artifact = None
        if artifact:
            if tier is None and _object(artifact.get("source")).get("tier") in TIERS:
                tier = result["tier"] = artifact["source"]["tier"]
            failure = _object(artifact.get("failure"))
            phase_data = _object(failure.get("phase_prefix"))
            if isinstance(failure.get("stage"), str):
                result["failed_phase"] = failure["stage"]
            result["phase_source"] = "failure_prefix"
            result["mixed_failure"] = _mixed_failure(phase_data.get("mixed_load"))
            result["publication"] = _publication(
                phase_data.get("publication_outcomes"), failure.get("stage")
            )
    phase_names = PRIMARY_PHASES
    if result["failed_phase"] and result["failed_phase"] not in phase_names:
        phase_names += (result["failed_phase"],)
    result["phases"] = _phase_rows(phase_data, phase_names)
    if not verified and not result["phases"] and execution.get("timeline_sha256"):
        _observe_harness_prefix(root, execution, result)

    harness_digest = execution.get("stderr_sha256")
    harness = _read_log(root / "stderr", issues, digest=harness_digest)
    if harness is not None:
        try:
            result["staging"] = build_native_staging_report(harness)
        except ValueError as error:
            issues.append(f"harness staging markers: {error}")
        mixed_detail = _object(phase_data.get("mixed_load"))
        target = _nonnegative_int(
            _object(mixed_detail.get("retained_read")).get("initial_view_sequence")
        )
        if result["mixed_failure"] is not None and target is not None and harness_digest:
            samples = _settlement_samples(harness, target, issues)
            if samples is not None:
                result["mixed_failure"]["settlement_blocker_samples"] = samples

    child_dir = root / f"causal-timeline.jsonl.native-{tier}-children" if tier else None
    child_trace = child_dir / "first.jsonl" if child_dir else None
    first_binding = _object(execution.get("native_child_inputs")).get("first")
    if not verified and execution:
        publication = result["publication"] or {}
        operation_key = publication.get("operation_key")
        child_digest = _object(first_binding).get("stderr_sha256")
        child_log = (
            _read_log(child_dir / "first.stderr", issues, digest=child_digest)
            if child_dir
            else None
        )
        sources = [
            ("daemon_first_stderr", child_log, bool(child_digest)),
            ("harness_stderr", harness, bool(harness_digest)),
            ("failure_message", failure.get("message"), False),
        ]
        # Bench fixtures reuse operation identities. Prefer the bytes committed
        # by this execution before considering unbound evidence for context.
        for source, raw, bound in sorted(sources, key=lambda item: not item[2]):
            if not isinstance(raw, str):
                continue
            try:
                found = find_native_refusal(
                    raw, operation_key=operation_key if isinstance(operation_key, dict) else None
                )
            except ValueError as error:
                issues.append(f"daemon refusal in {source}: {error}")
                continue
            if found:
                result["refusal"] = {
                    **found,
                    "evidence": source,
                    "evidence_binding": "BOUND" if bound else "UNBOUND",
                }
                break
    if result["publication"]:
        result["publication"].pop("operation_key", None)

    result["trace"] = _trace_report(
        child_trace, mode, include_trace, max_capture_bytes, first_binding
    )
    if not execution:
        blocked = (
            not (root / "execution.json").exists()
            and (preflight or {}).get("status") == "blocked"
            and (terminal is None or terminal.get("status") == "BLOCKED")
        )
        if blocked:
            status = "HOST_PREFLIGHT_BLOCKED"
            result["trace"] = {"status": "NOT_RUN"}
        elif (terminal or {}).get("status") == "FAILED":
            status = "CONTROLLER_FAILED"
        else:
            status = "INCOMPLETE"
    elif execution.get("timed_out") is True:
        status = "OUTER_TIMEOUT"
    elif verified:
        status = (
            "PRODUCT_VERIFIED"
            if result["phase_source"] == "product_summary"
            else "ARTIFACT_MISMATCH"
        )
    elif (
        result["refusal"] is not None
        and result["refusal"]["operation_match"] == "matched"
        and result["refusal"]["evidence_binding"] == "BOUND"
    ):
        status = "WORK_REFUSED"
    elif type(execution.get("exit_code")) is int and execution["exit_code"] != 0:
        status = "PRODUCER_FAILED"
    else:
        status = "CAPTURE_REJECTED"
    result["status"] = status
    return result


def _read_log(path: Path, issues: list[str], *, digest: str | None = None) -> str | None:
    raw = _read_bound(path, issues, digest)
    return raw.decode("utf-8", errors="replace") if raw is not None else None


def _publication(rows: object, phase: str | None) -> dict | None:
    if not isinstance(rows, list) or phase is None:
        return None
    matches = [row for row in rows if isinstance(row, dict) and row.get("phase") == phase]
    if len(matches) != 1:
        return None
    return {
        key: matches[0].get(key)
        for key in (
            "phase",
            "submit_outcome",
            "completion",
            "last_status",
            "status_checks",
            "operation_key",
        )
    }


def _observe_harness_prefix(root: Path, execution: dict, result: dict) -> None:
    summary = _record(root / "timeline-summary.json", result["issues"])
    if not summary or summary.get("stream_sha256") != execution["timeline_sha256"]:
        return
    spans = summary.get("spans")
    if not isinstance(spans, list):
        return
    phase = next(
        (
            row
            for row in reversed(spans)
            if isinstance(row, dict)
            and row.get("stage") == "harness_phase"
            and isinstance(row.get("phase"), str)
        ),
        None,
    )
    if phase:
        result["last_harness_span"] = {
            key: phase.get(key) for key in ("phase", "duration_ns", "outcome")
        }


def format_native_triage(report: dict) -> str:
    """Give the failure location and evidence before detailed timings."""
    lines = [f"{report['status']}  {report['run_dir']}"]
    execution = report["execution"]
    if execution:
        lines.append(
            f"Producer exit={execution['exit_code']}  capture={execution['status']}  "
            f"product={execution['product_status']}  source={execution['source_revision']}"
        )
    terminal = report["control_terminal"]
    if terminal:
        timing = terminal["formal_timing_qualification"]
        lines.append(
            f"Control {terminal['status']}" + (f"  formal timing={timing}" if timing else "")
        )
        if terminal["reason"]:
            lines.append(str(terminal["reason"]))
    host = report["host_preflight"]
    if host:
        lines.append(
            f"Host preflight record {host['status']}  load={host['one_minute_load']} "
            f"limit={host['one_minute_load_limit']}  foreign Rust={host['foreign_rust_count']}"
        )
    if report["failed_phase"]:
        lines.append(f"Failed phase {report['failed_phase']}")
    if mixed := report["mixed_failure"]:
        if mixed["first_failure_owner"]:
            lines.append(f"Mixed first failure owner={mixed['first_failure_owner']}")
        if mixed["held_worker_error"]:
            lines.append(f"  Held worker error: {mixed['held_worker_error']}")
        if mixed["pin_status"] is not None:
            lines.append(
                f"  Pin status={mixed['pin_status']}  "
                f"last pin confirmed={mixed['confirmed_last_pin']}  "
                f"release_ms={mixed['confirmed_release_ms']}"
            )
        if mixed["last_settlement_state"] is not None:
            if mixed["settled_field_present"]:
                settled = "null" if mixed["settled_ms"] is None else mixed["settled_ms"]
            else:
                settled = "unknown"
            lines.append(
                f"  Settlement state={mixed['last_settlement_state']}  "
                f"polls={mixed['settlement_polls']}  settled_ms={settled}"
            )
        if any(
            mixed[key] is not None
            for key in (
                "settlement_head_sequence",
                "settlement_exact_floor",
                "settlement_has_remaining_target_pins",
            )
        ):
            remaining = mixed["settlement_has_remaining_target_pins"]
            remaining_text = str(remaining).lower() if remaining is not None else "unknown"
            lines.append(
                f"  Settlement head={mixed['settlement_head_sequence']}  "
                f"exact floor={mixed['settlement_exact_floor']}  "
                f"remaining target pins={remaining_text}"
            )
        if samples := mixed.get("settlement_blocker_samples"):
            last = samples["last_logged"]
            lines.append(f"  Observed settlement blockers {', '.join(samples['observed'])}")
            lines.append(
                f"  Last logged blocker {last['status']}  head={last['head_sequence']}  "
                f"oldest ready={last['oldest_ready_sequence']}"
            )
    if publication := report["publication"]:
        lines.append(
            f"Publication {publication['submit_outcome']}  completion={publication['completion']}  "
            f"last status={publication['last_status']}  status checks={publication['status_checks']}"
        )
    if last_span := report.get("last_harness_span"):
        lines.append(f"Last harness span {last_span['phase']}  outcome={last_span['outcome']}")
    if refusal := report["refusal"]:
        lines.append(
            f"Daemon refusal {refusal['first_denial'] or refusal['outcome']}  stage={refusal['failed_stage']}  "
            f"operation={refusal['operation_sequence']} ({refusal['operation_match']})  "
            f"log binding={refusal['evidence_binding']}"
        )
        if checkpoint := refusal["checkpoint"]:
            lines.append(f"  {checkpoint['file']}:{checkpoint['line']}:{checkpoint['column']}")
    for name, phase in report["phases"].items():
        state = "done" if phase["completed"] else "incomplete"
        observer = f"  observer={phase['observer_ms']:.2f} ms" if "observer_ms" in phase else ""
        lines.append(f"  {name:<23} {phase['wall_ms']:>10.2f} ms  {state}{observer}")
    if staging := report["staging"]:
        if staging["status"] != "NO_MARKERS":
            lines.append(
                f"Staging pages {staging['acked_pages']}/{staging['total_pages']}  "
                f"accepted={staging['last_accepted_count']}  open={staging['open_page']}"
            )
            if slowest := staging["slowest_ack"]:
                lines.append(
                    f"  Slowest staging page {slowest['page']}  {slowest['page_elapsed_ms']} ms"
                )
    trace = report["trace"]
    lines.append(f"Daemon trace {trace['status']}")
    if trace["status"] == "AVAILABLE":
        lines.append(f"  {trace['bytes']} bytes. Add --trace to inspect request costs.")
    elif trace["status"] == "OVER_LIMIT":
        lines.append(f"  Set --max-capture-bytes to at least {trace['bytes']} for this trace.")
    elif trace["status"] == "INVALID":
        lines.append(f"  {trace['error']}")
    elif trace["status"] in ("UNBOUND_DIAGNOSTIC", "BINDING_MISMATCH"):
        lines.append(f"  Parsed {trace['parse_status']}; run binding {trace['binding_status']}")
        if trace["binding_mismatch_fields"]:
            lines.append(
                f"  Mismatched binding fields: {', '.join(trace['binding_mismatch_fields'])}"
            )
    elif trace["status"].startswith("BOUND_"):
        lines.append(f"  Parsed {trace['parse_status']}; run binding verified")
    if loss := trace.get("loss"):
        lines.append(
            f"  Events {trace['event_count']}  dropped={loss['events_dropped']}  "
            f"unwritten={loss['events_unwritten']}  open spans={loss['open_spans']}  "
            f"truncated tail={trace['truncated_tail']}"
        )
    for request in trace.get("requests", []):
        lines.append(f"  Request {request['request_span_id']}  outcome={request['outcome']}")
        if request["open_stages"]:
            lines.append(f"    Open stages {', '.join(request['open_stages'])}")
        for stage, elapsed in request["largest_stage_wall_unions_ns"].items():
            lines.append(f"    {stage:<38} {elapsed / 1_000_000:.2f} ms inclusive")
    lines.extend(f"Input issue {issue}" for issue in report["issues"])
    return "\n".join(lines)
