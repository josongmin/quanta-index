#!/usr/bin/env python3
"""Typed benchmark evidence (`BenchmarkEvidenceV1`) for the Python orchestrator.

This module is the Python half of the common evidence contract defined by the
Rust crate `benchmarks/bench-protocol`. Both sides must produce the *same*
canonical bytes for the same logical document; the committed cross-language
oracle is `benchmarks/bench-protocol/fixtures/`.

Canonical JSON rules (mirrored exactly in `benchmarks/bench-protocol/src/wire.rs`):

- object keys sorted by code point, no insignificant whitespace;
- strings escaped like `json.dumps(..., ensure_ascii=False)` with an explicit
  `\\u00xx` form for the remaining control characters;
- integers as integers; floats as the shortest round-trip decimal digits with
  an explicit fractional part and never in exponent notation;
- `null`/`true`/`false` lowercase.

Immutable runs live at `<root>/runs/<run-id>/`; `latest` is an advisory
pointer and never a baseline. Every refusal is typed and explicit: missing,
duplicated, stale, wrong-source, wrong-host, partial, timed-out or tampered
evidence never becomes a pass.
"""

from __future__ import annotations

import argparse
import decimal
import hashlib
import json
import os
import re
import stat
import sys
from pathlib import Path
from typing import Any

PROTOCOL = "BenchmarkEvidenceV1"
PROTOCOL_VERSION = 1
DIGEST_PREFIX = "sha256:"
EVIDENCE_FILE = "evidence.json"
RAW_DIR = "raw"

HOST_POLICIES = frozenset({"any", "local-diagnostic", "canonical-linux"})
SCOPES = frozenset({"diagnostic", "contract", "quality", "performance"})
STATUSES = frozenset({"pass", "fail", "unsupported", "not_run"})
PAYLOAD_KINDS = frozenset(
    {"micro", "latency", "load", "freshness", "retrieval", "agent_outcome", "recorded_experiment"}
)
UNITS = frozenset({"ms", "ratio", "count", "ns", "instructions", "qps", "bytes"})

HEX_RE = re.compile(r"^[0-9a-f]{40}$")
DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
RUN_ID_RE = re.compile(r"^[A-Za-z0-9._:-]{1,128}$")

ERROR_KEYS = {
    "envelope": {
        "protocol",
        "protocol_version",
        "run_id",
        "family",
        "profile",
        "case_id",
        "created_utc",
        "source",
        "build",
        "inputs",
        "host",
        "command",
        "boundary",
        "payload",
        "raw",
        "output_digest",
        "verdict",
        "digest",
    },
    "source": {
        "revision",
        "dirty",
        "dirty_paths_digest",
        "closure_profile",
        "closure_digest",
    },
    "build": {
        "toolchain",
        "target_triple",
        "lockfile_digest",
        "profile",
        "flags",
        "binaries",
    },
    "binary": {"name", "sha256"},
    "input": {"id", "availability", "digest", "reason"},
    "host": {"policy", "os", "arch", "cpu_count", "hostname_hash", "identity_digest", "lease"},
    "lease": {"mode", "observed_samples"},
    "command": {"argv", "cwd", "status", "exit_code", "timeout_seconds", "wall_ms"},
    "boundary": {"clock", "instrumentation", "start_event", "end_event"},
    "raw": {"path", "sha256", "bytes"},
    "verdict": {"scope", "status", "reason", "metrics"},
}


class EvidenceError(ValueError):
    """The evidence document, run store or canonical form is not admissible."""


# --------------------------------------------------------------------------
# Canonical JSON
# --------------------------------------------------------------------------


def _canonical_float(value: float) -> str:
    if value != value or value in (float("inf"), float("-inf")):
        raise EvidenceError("canonical JSON refuses non-finite numbers")
    text = repr(value)
    if "e" in text or "E" in text:
        text = format(decimal.Decimal(text), "f")
    if "." not in text:
        text += ".0"
    return text


_STRING_ESCAPES = {
    '"': '\\"',
    "\\": "\\\\",
    "\b": "\\b",
    "\f": "\\f",
    "\n": "\\n",
    "\r": "\\r",
    "\t": "\\t",
}


def _write_string(out: list[str], value: str) -> None:
    out.append('"')
    for character in value:
        escape = _STRING_ESCAPES.get(character)
        if escape is not None:
            out.append(escape)
        elif character < " ":
            out.append(f"\\u{ord(character):04x}")
        else:
            out.append(character)
    out.append('"')


def _write(out: list[str], value: object) -> None:
    if value is None:
        out.append("null")
    elif value is True:
        out.append("true")
    elif value is False:
        out.append("false")
    elif isinstance(value, int):
        out.append(str(value))
    elif isinstance(value, float):
        out.append(_canonical_float(value))
    elif isinstance(value, str):
        _write_string(out, value)
    elif isinstance(value, (list, tuple)):
        out.append("[")
        for index, item in enumerate(value):
            if index:
                out.append(",")
            _write(out, item)
        out.append("]")
    elif isinstance(value, dict):
        for key in value:
            if not isinstance(key, str):
                raise EvidenceError("canonical JSON object keys must be strings")
        out.append("{")
        for index, key in enumerate(sorted(value)):
            if index:
                out.append(",")
            _write_string(out, key)
            out.append(":")
            _write(out, value[key])
        out.append("}")
    else:
        raise EvidenceError(f"canonical JSON refuses value of type {type(value).__name__}")


def canonical_json(value: object) -> str:
    """Canonical JSON text for `value` (see the module docstring)."""
    out: list[str] = []
    _write(out, value)
    return "".join(out)


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def digest_bytes(data: bytes) -> str:
    return DIGEST_PREFIX + sha256_hex(data)


def digest_of(value: object) -> str:
    return digest_bytes(canonical_json(value).encode("utf-8"))


def is_digest(value: object) -> bool:
    return isinstance(value, str) and DIGEST_RE.fullmatch(value) is not None


def require_digest(field: str, value: object) -> str:
    if not is_digest(value):
        raise EvidenceError(f"invalid digest for {field}: {value!r}")
    assert isinstance(value, str)
    return value


# --------------------------------------------------------------------------
# Shape helpers
# --------------------------------------------------------------------------


def _fail(message: str) -> None:
    raise EvidenceError(message)


def _object(value: object, where: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        _fail(f"{where} must be an object")
    return value


def _exact_keys(value: object, keys: set[str], where: str) -> dict[str, Any]:
    table = _object(value, where)
    if set(table) != keys:
        _fail(f"{where} must contain exactly {sorted(keys)}, found {sorted(table)}")
    return table


def _list(value: object, where: str, *, nonempty: bool = False) -> list[Any]:
    if not isinstance(value, list) or (nonempty and not value):
        _fail(f"{where} must be a {'non-empty ' if nonempty else ''}array")
    return value


def _string(value: object, where: str) -> str:
    if not isinstance(value, str) or not value:
        _fail(f"{where} must be a non-empty string")
    return value


def _uint(value: object, where: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        _fail(f"{where} must be a non-negative integer")
    return value


def _number(value: object, where: str) -> float:
    if not isinstance(value, (int, float)) or isinstance(value, bool):
        _fail(f"{where} must be a number")
    return float(value)


def _reject_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    seen: dict[str, Any] = {}
    for key, value in pairs:
        if key in seen:
            raise EvidenceError(f"duplicate JSON key {key!r}")
        seen[key] = value
    return seen


def _relative_path(value: object, where: str) -> str:
    text = _string(value, where)
    if "\\" in text:
        _fail(f"{where} must not contain a backslash")
    pure = Path(text)
    if pure.is_absolute() or any(part in {"..", "."} for part in pure.parts):
        _fail(f"{where} escapes the run root: {text!r}")
    return text


def _run_id(value: object) -> str:
    text = _string(value, "run_id")
    if text == "latest" or text.startswith(".") or RUN_ID_RE.fullmatch(text) is None or ".." in text:
        _fail(f"invalid run id {text!r}")
    return text


def _unit(value: object, where: str) -> str:
    text = _string(value, where)
    if text not in UNITS:
        _fail(f"{where}: unit {text!r} is not in the contract vocabulary")
    return text


def _metric(value: object, where: str) -> None:
    table = _exact_keys(value, {"name", "unit", "value", "numerator", "denominator"}, where)
    _string(table["name"], f"{where}.name")
    _unit(table["unit"], f"{where}.unit")
    _number(table["value"], f"{where}.value")
    numerator = table["numerator"]
    denominator = table["denominator"]
    if numerator is None and denominator is None:
        return
    if numerator is None or denominator is None:
        _fail(f"{where}: numerator and denominator must be present together")
    _uint(numerator, f"{where}.numerator")
    _uint(denominator, f"{where}.denominator")
    if numerator > denominator:
        _fail(f"{where}: numerator {numerator} exceeds denominator {denominator}")


# --------------------------------------------------------------------------
# Typed payloads
# --------------------------------------------------------------------------


def _validate_micro(payload: dict[str, Any]) -> None:
    table = _exact_keys(
        payload,
        {"kind", "bench_id", "metric", "unit", "instrumentation", "statistic",
         "value", "iterations", "samples"},
        "payload",
    )
    _string(table["bench_id"], "payload.bench_id")
    _string(table["metric"], "payload.metric")
    instrumentation = _string(table["instrumentation"], "payload.instrumentation")
    if instrumentation == "wall":
        expected = "ns"
    elif instrumentation == "instructions":
        expected = "instructions"
    else:
        _fail(f"micro.instrumentation {instrumentation!r} is not wall or instructions")
    unit = _string(table["unit"], "payload.unit")
    if unit != expected:
        _fail(
            f"micro unit {unit!r} contradicts instrumentation {instrumentation!r}; "
            f"expected {expected!r}"
        )
    statistic = _string(table["statistic"], "payload.statistic")
    if statistic not in {"mean", "median", "min"}:
        _fail(f"micro.statistic {statistic!r} is not mean/median/min")
    _number(table["value"], "payload.value")
    _uint(table["iterations"], "payload.iterations")
    if _uint(table["samples"], "payload.samples") == 0:
        _fail("micro.samples must be at least 1")


def _validate_latency(payload: dict[str, Any]) -> None:
    table = _exact_keys(payload, {"kind", "rows", "errors", "timeouts", "drops"}, "payload")
    rows = _list(table["rows"], "payload.rows", nonempty=True)
    seen: set[str] = set()
    for index, raw in enumerate(rows):
        where = f"payload.rows[{index}]"
        row = _exact_keys(
            raw,
            {"case_id", "metric", "unit", "samples", "p50", "p95", "p99",
             "error_count", "timeout_count", "early_stop_reason"},
            where,
        )
        case_id = _string(row["case_id"], f"{where}.case_id")
        if case_id in seen:
            _fail(f"{where} repeats case_id {case_id!r}")
        seen.add(case_id)
        _string(row["metric"], f"{where}.metric")
        _unit(row["unit"], f"{where}.unit")
        samples = _uint(row["samples"], f"{where}.samples")
        percentiles = [row["p50"], row["p95"], row["p99"]]
        present = [value for value in percentiles if value is not None]
        for name, value in zip(("p50", "p95", "p99"), percentiles):
            if value is not None:
                _number(value, f"{where}.{name}")
        _uint(row["error_count"], f"{where}.error_count")
        _uint(row["timeout_count"], f"{where}.timeout_count")
        reason = row["early_stop_reason"]
        if reason is not None:
            _string(reason, f"{where}.early_stop_reason")
            if present:
                _fail(f"{where} is marked unmeasured ({reason!r}) but carries percentiles")
            continue
        if samples == 0:
            _fail(f"{where} has zero samples and no early_stop_reason")
        if row["p50"] is None:
            _fail(f"{where} has samples but no measured p50")
    _uint(table["errors"], "payload.errors")
    _uint(table["timeouts"], "payload.timeouts")
    _uint(table["drops"], "payload.drops")


def _validate_load(payload: dict[str, Any]) -> None:
    table = _exact_keys(
        payload, {"kind", "arrival", "generator_saturated", "points", "errors"}, "payload"
    )
    arrival = _string(table["arrival"], "payload.arrival")
    if arrival not in {"open_loop", "closed_loop"}:
        _fail(f"load.arrival {arrival!r} is not open_loop or closed_loop")
    if not isinstance(table["generator_saturated"], bool):
        _fail("payload.generator_saturated must be a boolean")
    points = _list(table["points"], "payload.points", nonempty=True)
    for index, raw in enumerate(points):
        where = f"payload.points[{index}]"
        point = _exact_keys(
            raw, {"label", "offered_rate", "completed_rate", "dropped", "timeouts"}, where
        )
        _string(point["label"], f"{where}.label")
        offered = point["offered_rate"]
        _number(point["completed_rate"], f"{where}.completed_rate")
        _uint(point["dropped"], f"{where}.dropped")
        _uint(point["timeouts"], f"{where}.timeouts")
        if arrival == "open_loop":
            if offered is None:
                _fail(
                    f"open-loop point {point['label']!r} has no offered_rate; closed-loop "
                    "throughput cannot be reported as offered capacity"
                )
            _number(offered, f"{where}.offered_rate")
        elif offered is not None:
            _fail(f"closed-loop point {point['label']!r} claims an offered_rate")
    _uint(table["errors"], "payload.errors")


def _validate_freshness(payload: dict[str, Any]) -> None:
    table = _exact_keys(
        payload, {"kind", "phases", "stale_hits", "generation"}, "payload"
    )
    phases = _list(table["phases"], "payload.phases", nonempty=True)
    seen: set[str] = set()
    for index, raw in enumerate(phases):
        where = f"payload.phases[{index}]"
        phase = _exact_keys(raw, {"name", "ms", "samples"}, where)
        name = _string(phase["name"], f"{where}.name")
        if name in seen:
            _fail(f"freshness phase {name!r} is duplicated; phases must stay distinct")
        seen.add(name)
        _number(phase["ms"], f"{where}.ms")
        _uint(phase["samples"], f"{where}.samples")
    _uint(table["stale_hits"], "payload.stale_hits")
    generation = table["generation"]
    if generation is not None:
        _string(generation, "payload.generation")


def _validate_retrieval(payload: dict[str, Any]) -> None:
    table = _exact_keys(
        payload,
        {"kind", "lane", "metric_space", "judgments", "unjudged", "rows",
         "universe_attested", "corpus_digest", "query_pack_digest"},
        "payload",
    )
    lane = _string(table["lane"], "payload.lane")
    if lane not in {"native_default", "controlled_mechanism"}:
        _fail(f"retrieval.lane {lane!r} is not native_default or controlled_mechanism")
    metric_space = _string(table["metric_space"], "payload.metric_space")
    if metric_space not in {"file", "line", "span", "context"}:
        _fail(f"retrieval.metric_space {metric_space!r} is not file/line/span/context")
    judgments = _string(table["judgments"], "payload.judgments")
    if judgments not in {"judged", "pooled", "mechanically_labeled"}:
        _fail(f"retrieval.judgments {judgments!r} is not judged/pooled/mechanically_labeled")
    if metric_space == "span" and judgments == "mechanically_labeled":
        _fail("file-only mechanical labels cannot become span judgments")
    _uint(table["unjudged"], "payload.unjudged")
    if not isinstance(table["universe_attested"], bool):
        _fail("payload.universe_attested must be a boolean")
    require_digest("retrieval.corpus_digest", table["corpus_digest"])
    require_digest("retrieval.query_pack_digest", table["query_pack_digest"])
    rows = _list(table["rows"], "payload.rows", nonempty=True)
    seen: set[str] = set()
    for index, raw in enumerate(rows):
        where = f"payload.rows[{index}]"
        row = _exact_keys(raw, {"query_id", "metric", "unit", "value", "state"}, where)
        query_id = _string(row["query_id"], f"{where}.query_id")
        if query_id in seen:
            _fail(f"{where} repeats query_id {query_id!r}")
        seen.add(query_id)
        _string(row["metric"], f"{where}.metric")
        _unit(row["unit"], f"{where}.unit")
        state = _string(row["state"], f"{where}.state")
        value = row["value"]
        if state in {"judged", "irrelevant", "no_answer"}:
            if value is None:
                _fail(f"{where} is {state!r} but carries no value")
            _number(value, f"{where}.value")
        elif state in {"unjudged", "timeout", "unsupported"}:
            if value is not None:
                _fail(f"{where} is {state!r} and must not carry a score")
        else:
            _fail(f"{where} has unknown state {state!r}")


def _validate_agent_outcome(payload: dict[str, Any]) -> None:
    table = _exact_keys(
        payload,
        {"kind", "task_count", "pair_count", "arms", "excluded_pairs", "unknown_pairs",
         "metrics", "capture", "input_digest"},
        "payload",
    )
    if table["arms"] != ["A", "B", "C"]:
        _fail(f"agent_outcome.arms must be exactly [A, B, C], found {table['arms']!r}")
    _uint(table["task_count"], "payload.task_count")
    if _uint(table["pair_count"], "payload.pair_count") == 0:
        _fail("agent_outcome.pair_count must be at least 1")
    _uint(table["excluded_pairs"], "payload.excluded_pairs")
    _uint(table["unknown_pairs"], "payload.unknown_pairs")
    for index, metric in enumerate(_list(table["metrics"], "payload.metrics")):
        _metric(metric, f"payload.metrics[{index}]")
    capture = _string(table["capture"], "payload.capture")
    if capture not in {"recorded_unauthenticated", "authenticated"}:
        _fail(f"agent_outcome.capture {capture!r} is not recorded_unauthenticated or authenticated")
    require_digest("agent_outcome.input_digest", table["input_digest"])


def _validate_recorded_experiment(payload: dict[str, Any]) -> None:
    table = _exact_keys(
        payload, {"kind", "experiment_id", "diagnostic_only", "points", "source_digest"}, "payload"
    )
    _string(table["experiment_id"], "payload.experiment_id")
    if table["diagnostic_only"] is not True:
        _fail(
            "recorded_experiment.diagnostic_only must be true; a recorded input is not a "
            "qualified measurement"
        )
    require_digest("recorded_experiment.source_digest", table["source_digest"])
    points = _list(table["points"], "payload.points", nonempty=True)
    for index, raw in enumerate(points):
        where = f"payload.points[{index}]"
        point = _exact_keys(raw, {"label", "metric", "unit", "value"}, where)
        _string(point["label"], f"{where}.label")
        _string(point["metric"], f"{where}.metric")
        _unit(point["unit"], f"{where}.unit")
        _number(point["value"], f"{where}.value")


_PAYLOAD_VALIDATORS = {
    "micro": _validate_micro,
    "latency": _validate_latency,
    "load": _validate_load,
    "freshness": _validate_freshness,
    "retrieval": _validate_retrieval,
    "agent_outcome": _validate_agent_outcome,
    "recorded_experiment": _validate_recorded_experiment,
}


def validate_payload(payload: object) -> str:
    """Validate a typed payload and return its kind."""
    table = _object(payload, "payload")
    kind = table.get("kind")
    if not isinstance(kind, str) or kind not in PAYLOAD_KINDS:
        _fail(f"payload.kind {kind!r} is not registered")
    _PAYLOAD_VALIDATORS[kind](table)
    return kind


# --------------------------------------------------------------------------
# Envelope validation
# --------------------------------------------------------------------------


def validate(evidence: object) -> dict[str, Any]:
    """Full structural validation of an evidence document."""
    record = _exact_keys(evidence, ERROR_KEYS["envelope"], "evidence")
    if record["protocol"] != PROTOCOL:
        _fail(f"unsupported protocol {record['protocol']!r}; expected {PROTOCOL!r}")
    if record["protocol_version"] != PROTOCOL_VERSION:
        _fail(f"unsupported protocol_version {record['protocol_version']!r}")
    _run_id(record["run_id"])
    _string(record["family"], "family")
    _string(record["profile"], "profile")
    if record["case_id"] is not None:
        _string(record["case_id"], "case_id")
    _string(record["created_utc"], "created_utc")

    source = _exact_keys(record["source"], ERROR_KEYS["source"], "source")
    revision = _string(source["revision"], "source.revision")
    if HEX_RE.fullmatch(revision) is None:
        _fail(f"source.revision {revision!r} is not a full lowercase Git SHA")
    if not isinstance(source["dirty"], bool):
        _fail("source.dirty must be a boolean")
    dirty_digest = source["dirty_paths_digest"]
    if source["dirty"]:
        if dirty_digest is None:
            _fail("a dirty source must carry source.dirty_paths_digest")
        require_digest("source.dirty_paths_digest", dirty_digest)
    elif dirty_digest is not None:
        _fail("a clean source must not carry source.dirty_paths_digest")
    _string(source["closure_profile"], "source.closure_profile")
    require_digest("source.closure_digest", source["closure_digest"])

    build = _exact_keys(record["build"], ERROR_KEYS["build"], "build")
    _string(build["toolchain"], "build.toolchain")
    _string(build["target_triple"], "build.target_triple")
    require_digest("build.lockfile_digest", build["lockfile_digest"])
    _string(build["profile"], "build.profile")
    for index, flag in enumerate(_list(build["flags"], "build.flags")):
        _string(flag, f"build.flags[{index}]")
    for index, raw in enumerate(_list(build["binaries"], "build.binaries")):
        where = f"build.binaries[{index}]"
        binary = _exact_keys(raw, ERROR_KEYS["binary"], where)
        _string(binary["name"], f"{where}.name")
        require_digest(f"{where}.sha256", binary["sha256"])

    for index, raw in enumerate(_list(record["inputs"], "inputs")):
        where = f"inputs[{index}]"
        input_record = _exact_keys(raw, ERROR_KEYS["input"], where)
        _string(input_record["id"], f"{where}.id")
        availability = input_record["availability"]
        if availability == "present":
            require_digest(f"{where}.digest", input_record["digest"])
        elif availability == "unavailable":
            if input_record["digest"] is not None:
                _fail(f"{where} is unavailable but carries a digest")
            _string(input_record["reason"], f"{where}.reason")
        else:
            _fail(f"{where} has unknown availability {availability!r}")

    host = _exact_keys(record["host"], ERROR_KEYS["host"], "host")
    policy = _string(host["policy"], "host.policy")
    if policy not in HOST_POLICIES:
        _fail(f"host.policy {policy!r} is not registered")
    if policy == "canonical-linux" and host["os"] != "linux":
        _fail(f"host policy canonical-linux cannot be satisfied on os {host['os']!r}")
    _string(host["os"], "host.os")
    _string(host["arch"], "host.arch")
    _uint(host["cpu_count"], "host.cpu_count")
    require_digest("host.hostname_hash", host["hostname_hash"])
    require_digest("host.identity_digest", host["identity_digest"])
    lease = _exact_keys(host["lease"], ERROR_KEYS["lease"], "host.lease")
    mode = _string(lease["mode"], "host.lease.mode")
    if mode not in {"exclusive", "shared", "none"}:
        _fail(f"host.lease.mode {mode!r} is not exclusive/shared/none")
    _uint(lease["observed_samples"], "host.lease.observed_samples")

    command = _exact_keys(record["command"], ERROR_KEYS["command"], "command")
    argv = _list(command["argv"], "command.argv", nonempty=True)
    for index, token in enumerate(argv):
        _string(token, f"command.argv[{index}]")
    _string(command["cwd"], "command.cwd")
    command_status = _string(command["status"], "command.status")
    exit_code = command["exit_code"]
    if command_status == "completed":
        if exit_code != 0:
            _fail(f"completed with exit_code {exit_code!r} is not admissible evidence")
    elif command_status in {"failed", "timeout", "interrupted"}:
        if record["verdict"]["status"] != "not_run":
            _fail(f"producer {command_status} cannot carry verdict {record['verdict']['status']!r}")
    else:
        _fail(f"command.status {command_status!r} is not admissible evidence")
    if _uint(command["timeout_seconds"], "command.timeout_seconds") == 0:
        _fail("command.timeout_seconds must be positive")
    _uint(command["wall_ms"], "command.wall_ms")

    boundary = _exact_keys(record["boundary"], ERROR_KEYS["boundary"], "boundary")
    for key in ("clock", "instrumentation", "start_event", "end_event"):
        _string(boundary[key], f"boundary.{key}")
    if command_status != "failed" and not isinstance(boundary["instrumentation"], str):
        _fail("boundary.instrumentation must be a string")

    kind = validate_payload(record["payload"])
    if kind == "micro" and boundary["instrumentation"] == "none":
        _fail("a micro payload must declare its instrumentation mode")

    raw = _list(record["raw"], "raw", nonempty=True)
    paths: set[str] = set()
    for index, item in enumerate(raw):
        where = f"raw[{index}]"
        reference = _exact_keys(item, ERROR_KEYS["raw"], where)
        path = _relative_path(reference["path"], f"{where}.path")
        if path in paths:
            _fail(f"duplicate raw reference {path!r}")
        paths.add(path)
        require_digest(f"{where}.sha256", reference["sha256"])
        _uint(reference["bytes"], f"{where}.bytes")

    require_digest("output_digest", record["output_digest"])

    verdict = _exact_keys(record["verdict"], ERROR_KEYS["verdict"], "verdict")
    scope = _string(verdict["scope"], "verdict.scope")
    if scope not in SCOPES:
        _fail(f"verdict.scope {scope!r} is not registered")
    if scope == "performance":
        if boundary["clock"] != "monotonic":
            _fail("performance scope requires a monotonic clock")
        if mode != "exclusive" or lease["observed_samples"] < 2:
            _fail(
                "performance scope requires an exclusive host lease with capture-time observations"
            )
    status = _string(verdict["status"], "verdict.status")
    if status not in STATUSES:
        _fail(f"verdict.status {status!r} is not registered")
    if status != "pass" and not (
        isinstance(verdict["reason"], str) and verdict["reason"].strip()
    ):
        _fail("a non-pass verdict must state a reason")
    if verdict["reason"] is not None:
        _string(verdict["reason"], "verdict.reason")
    for index, metric in enumerate(_list(verdict["metrics"], "verdict.metrics")):
        _metric(metric, f"verdict.metrics[{index}]")

    digest = record["digest"]
    if digest is not None:
        require_digest("digest", digest)
    return record


def body_value(evidence: dict[str, Any]) -> dict[str, Any]:
    """The document without its own `digest` field."""
    return {key: value for key, value in evidence.items() if key != "digest"}


def seal(evidence: dict[str, Any]) -> dict[str, Any]:
    """Validate then attach the canonical document digest."""
    validate(evidence)
    sealed = body_value(evidence)
    sealed["digest"] = digest_of(sealed)
    return sealed


def verify_digest(evidence: dict[str, Any]) -> str:
    declared = evidence.get("digest")
    require_digest("digest", declared)
    computed = digest_of(body_value(evidence))
    if computed != declared:
        raise EvidenceError(f"evidence digest mismatch: declared {declared}, computed {computed}")
    assert isinstance(declared, str)
    return declared


def open_evidence(text: str) -> dict[str, Any]:
    """Parse, digest-verify and validate a sealed evidence document."""
    try:
        record = json.loads(text, object_pairs_hook=_reject_duplicate_keys)
    except json.JSONDecodeError as exc:
        raise EvidenceError(f"malformed evidence JSON: {exc}") from exc
    validate(record)
    verify_digest(record)
    return record


def read_evidence(path: Path) -> dict[str, Any]:
    try:
        return open_evidence(path.read_text(encoding="utf-8"))
    except OSError as exc:
        raise EvidenceError(f"cannot read evidence {path}: {exc}") from exc


def to_canonical_json(evidence: dict[str, Any]) -> str:
    verify_digest(evidence)
    return canonical_json(evidence)


# --------------------------------------------------------------------------
# Immutable run store
# --------------------------------------------------------------------------


def _read_regular_file(path: Path) -> bytes:
    metadata = path.lstat()
    if stat.S_ISLNK(metadata.st_mode):
        raise EvidenceError(f"refusing symlink: {path}")
    if not stat.S_ISREG(metadata.st_mode):
        raise EvidenceError(f"not a regular file: {path}")
    return path.read_bytes()


def _write_atomic(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp-write")
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o644)
    with os.fdopen(descriptor, "wb") as handle:
        handle.write(data)
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(temporary, path)
    _sync_dir(path.parent)


def _sync_dir(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


class RunStore:
    """Immutable run storage rooted at an external benchmark root."""

    def __init__(self, root: Path) -> None:
        self.root = Path(root)

    @property
    def runs_dir(self) -> Path:
        return self.root / "runs"

    @property
    def staging_dir(self) -> Path:
        return self.root / ".staging"

    @property
    def baselines_dir(self) -> Path:
        return self.root / "baselines"

    @property
    def latest_path(self) -> Path:
        return self.root / "latest"

    def run_dir(self, run_id: str) -> Path:
        return self.runs_dir / _run_id(run_id)

    def stage(self, run_id: str) -> StagingRun:
        _run_id(run_id)
        if self.run_dir(run_id).exists():
            raise EvidenceError(f"run {run_id!r} already exists; run ids are immutable")
        path = self.staging_dir / run_id
        if path.exists():
            _remove_dir(path)
        path.mkdir(parents=True)
        return StagingRun(run_id=run_id, path=path)

    def promote(self, staged: StagingRun) -> dict[str, Any]:
        evidence = staged.read_evidence()
        self._verify_raw(staged.path, evidence)
        target = self.run_dir(staged.run_id)
        if target.exists():
            raise EvidenceError(f"run {staged.run_id!r} already exists; run ids are immutable")
        self.runs_dir.mkdir(parents=True, exist_ok=True)
        _sync_dir(staged.path)
        os.rename(staged.path, target)
        _sync_dir(self.runs_dir)
        digest = verify_digest(evidence)
        pointer = {
            "run_id": staged.run_id,
            "digest": digest,
            "family": evidence["family"],
            "profile": evidence["profile"],
            "updated_epoch_seconds": int(os.stat(target).st_mtime),
        }
        _write_atomic(self.latest_path, json.dumps(pointer, sort_keys=True).encode("utf-8"))
        return {"run_id": staged.run_id, "run_dir": target, "digest": digest}

    def load(self, run_id: str) -> dict[str, Any]:
        run_dir = self.run_dir(run_id)
        if not run_dir.is_dir():
            raise EvidenceError(f"missing run directory: runs/{run_id}")
        evidence = read_evidence(run_dir / EVIDENCE_FILE)
        if evidence["run_id"] != run_id:
            raise EvidenceError(
                f"run id mismatch: expected {run_id!r}, found {evidence['run_id']!r}"
            )
        self._verify_raw(run_dir, evidence)
        return evidence

    def read_latest(self) -> dict[str, Any] | None:
        if not self.latest_path.exists():
            return None
        payload = json.loads(_read_regular_file(self.latest_path).decode("utf-8"))
        if not isinstance(payload, dict):
            raise EvidenceError("latest pointer must be an object")
        return payload

    def admit_baseline(
        self, family: str, run_id: str, margin_ppm: int, uncertainty: str
    ) -> dict[str, Any]:
        if not uncertainty.strip():
            raise EvidenceError("baseline uncertainty method must be declared")
        evidence = self.load(run_id)
        if evidence["family"] != family:
            raise EvidenceError(
                f"run {run_id!r} belongs to family {evidence['family']!r}, not {family!r}"
            )
        if evidence["verdict"]["status"] != "pass":
            raise EvidenceError(
                f"run {run_id!r} has verdict {evidence['verdict']['status']!r}; "
                "only a passing run can be admitted"
            )
        record = {
            "family": family,
            "run_id": run_id,
            "digest": verify_digest(evidence),
            "host_policy": evidence["host"]["policy"],
            "closure_digest": evidence["source"]["closure_digest"],
            "input_digests": [
                item["digest"] for item in evidence["inputs"] if item["digest"] is not None
            ],
            "margin_ppm": margin_ppm,
            "uncertainty": uncertainty,
        }
        _write_atomic(
            self.baselines_dir / f"{family}.json",
            json.dumps(record, sort_keys=True, indent=2).encode("utf-8"),
        )
        return record

    def read_baseline(self, family: str) -> dict[str, Any] | None:
        path = self.baselines_dir / f"{family}.json"
        if not path.exists():
            return None
        payload = json.loads(_read_regular_file(path).decode("utf-8"))
        if not isinstance(payload, dict):
            raise EvidenceError("baseline record must be an object")
        return payload

    def collect(self, keep: list[str]) -> list[str]:
        retained = set(keep)
        if self.baselines_dir.is_dir():
            for path in sorted(self.baselines_dir.glob("*.json")):
                record = json.loads(_read_regular_file(path).decode("utf-8"))
                if not isinstance(record, dict) or not isinstance(record.get("run_id"), str):
                    raise EvidenceError(f"malformed baseline record: {path}")
                retained.add(record["run_id"])
        removed: list[str] = []
        if not self.runs_dir.is_dir():
            return removed
        for entry in sorted(self.runs_dir.iterdir()):
            if entry.name in retained:
                continue
            _remove_dir(entry)
            removed.append(entry.name)
        return removed

    def _verify_raw(self, run_dir: Path, evidence: dict[str, Any]) -> None:
        for reference in evidence["raw"]:
            path = run_dir / reference["path"]
            if path.is_symlink():
                raise EvidenceError(f"raw reference {reference['path']!r} is a symlink")
            if not path.is_file():
                raise EvidenceError(f"referenced raw file {reference['path']!r} is missing")
            data = path.read_bytes()
            if len(data) != reference["bytes"]:
                raise EvidenceError(
                    f"raw file {reference['path']!r} length mismatch: "
                    f"declared {reference['bytes']}, actual {len(data)}"
                )
            computed = digest_bytes(data)
            if computed != reference["sha256"]:
                raise EvidenceError(
                    f"raw file {reference['path']!r} digest mismatch: "
                    f"declared {reference['sha256']}, computed {computed}"
                )
        raw_root = run_dir / RAW_DIR
        if raw_root.is_dir():
            declared = {reference["path"] for reference in evidence["raw"]}
            for entry in sorted(raw_root.iterdir()):
                relative = f"{RAW_DIR}/{entry.name}"
                if relative not in declared:
                    raise EvidenceError(f"undeclared raw file {relative!r}")


def _remove_dir(path: Path) -> None:
    import shutil

    shutil.rmtree(path)


class StagingRun:
    """A staged, not-yet-admissible run."""

    def __init__(self, run_id: str, path: Path) -> None:
        self.run_id = run_id
        self.path = path

    def write_raw(self, relative: str, data: bytes) -> dict[str, Any]:
        _relative_path(relative, "raw path")
        target = self.path / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        descriptor = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o644)
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(data)
        return {"path": relative, "sha256": digest_bytes(data), "bytes": len(data)}

    def write_evidence(self, evidence: dict[str, Any]) -> None:
        if evidence.get("run_id") != self.run_id:
            raise EvidenceError(
                f"run id mismatch: expected {self.run_id!r}, found {evidence.get('run_id')!r}"
            )
        _write_atomic(
            self.path / EVIDENCE_FILE, to_canonical_json(evidence).encode("utf-8")
        )

    def read_evidence(self) -> dict[str, Any]:
        return read_evidence(self.path / EVIDENCE_FILE)

    def abort(self) -> None:
        _remove_dir(self.path)


# --------------------------------------------------------------------------
# Deterministic cross-language sample (mirrors src/sample.rs)
# --------------------------------------------------------------------------

SAMPLE_RUN_ID = "run-20260926T120000Z-a1b2c3d4"
SAMPLE_REVISION = "0123456789abcdef0123456789abcdef01234567"
SAMPLE_CREATED_UTC = "2026-09-26T12:00:00Z"
SAMPLE_RAW = b'{"sample":"warm-matrix","p50_ms":0.42}\n'


def sample_evidence() -> dict[str, Any]:
    """Deterministic sample document; must match the Rust fixture byte-for-byte."""
    return {
        "protocol": PROTOCOL,
        "protocol_version": PROTOCOL_VERSION,
        "run_id": SAMPLE_RUN_ID,
        "family": "dsl-warm",
        "profile": "dsl-authority",
        "case_id": None,
        "created_utc": SAMPLE_CREATED_UTC,
        "source": {
            "revision": SAMPLE_REVISION,
            "dirty": False,
            "dirty_paths_digest": None,
            "closure_profile": "benchmark-control-plane",
            "closure_digest": digest_bytes(b"closure-sample"),
        },
        "build": {
            "toolchain": "rustc 1.92.0",
            "target_triple": "aarch64-apple-darwin",
            "lockfile_digest": digest_bytes(b"lockfile-sample"),
            "profile": "bench",
            "flags": ["--locked"],
            "binaries": [
                {"name": "dsl_warm_matrix", "sha256": digest_bytes(b"binary-sample")}
            ],
        },
        "inputs": [
            {
                "id": "workspace-fixture",
                "availability": "present",
                "digest": digest_bytes(b"corpus-sample"),
                "reason": None,
            }
        ],
        "host": {
            "policy": "local-diagnostic",
            "os": "macos",
            "arch": "aarch64",
            "cpu_count": 10,
            "hostname_hash": digest_bytes(b"hostname-sample"),
            "identity_digest": digest_bytes(b"host-sample"),
            "lease": {"mode": "shared", "observed_samples": 1},
        },
        "command": {
            "argv": ["just", "rust-bench-dsl-warm"],
            "cwd": ".",
            "status": "completed",
            "exit_code": 0,
            "timeout_seconds": 1800,
            "wall_ms": 12345,
        },
        "boundary": {
            "clock": "monotonic",
            "instrumentation": "none",
            "start_event": "producer_exec",
            "end_event": "artifact_written",
        },
        "payload": {
            "kind": "latency",
            "rows": [
                {
                    "case_id": "lexical.keyword.native",
                    "metric": "p50",
                    "unit": "ms",
                    "samples": 200,
                    "p50": 0.42,
                    "p95": 0.55,
                    "p99": 0.61,
                    "error_count": 0,
                    "timeout_count": 0,
                    "early_stop_reason": None,
                }
            ],
            "errors": 0,
            "timeouts": 0,
            "drops": 0,
        },
        "raw": [
            {
                "path": "raw/warm-matrix.json",
                "sha256": digest_bytes(SAMPLE_RAW),
                "bytes": len(SAMPLE_RAW),
            }
        ],
        "output_digest": digest_bytes(b"normalized-sample"),
        "verdict": {
            "scope": "diagnostic",
            "status": "pass",
            "reason": None,
            "metrics": [
                {
                    "name": "p50_ms",
                    "unit": "ms",
                    "value": 0.42,
                    "numerator": None,
                    "denominator": None,
                }
            ],
        },
        "digest": None,
    }


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="BenchmarkEvidenceV1 contract tool")
    subparsers = parser.add_subparsers(dest="command", required=True)
    for name in ("canonical", "validate", "digest"):
        child = subparsers.add_parser(name)
        child.add_argument("path", type=Path)
    args = parser.parse_args(argv)
    try:
        evidence = read_evidence(args.path)
        if args.command == "canonical":
            sys.stdout.write(to_canonical_json(evidence) + "\n")
        elif args.command == "digest":
            sys.stdout.write(verify_digest(evidence) + "\n")
        else:
            sys.stdout.write(
                f"valid BenchmarkEvidenceV1: {evidence['family']} "
                f"{verify_digest(evidence)}\n"
            )
    except EvidenceError as error:
        sys.stderr.write(f"ERROR: {error}\n")
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
