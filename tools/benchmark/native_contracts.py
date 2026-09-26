"""Pure contracts for native benchmark evidence emitted by public producers."""

from __future__ import annotations

import math
import re
from pathlib import Path

CONCURRENCY_COUNTS = (1, 8, 32)


def _non_negative_number(value: object) -> bool:
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and value >= 0
        and value != float("inf")
        and value == value
    )


def _qps_matches(qps: float, served: int, window: float) -> bool:
    try:
        expected = served / window
        return math.isfinite(expected) and math.isclose(qps, expected, rel_tol=1e-12, abs_tol=1e-12)
    except (OverflowError, ZeroDivisionError):
        return False


CONCURRENCY_ROUTES = ("lexical", "semantic", "hybrid", "symbol", "lexical_count")


def validate_concurrency(
    payload: dict, minimum: int = 16, artifact_path: Path | None = None
) -> list[str]:
    """Bind the public concurrency producer's full measurements to emitted rows.

    Routes partition fast requests; fast/slow summaries determine the rail
    verdict. Both representations are checked so neither can hide a partial
    or contradictory measurement in the other.
    """
    reasons: list[str] = []
    detail, rows = payload.get("detail"), payload.get("rows")
    if not isinstance(detail, dict) or not isinstance(rows, list):
        return ["concurrency: missing detail or rows"]
    if detail.get("passed") is not True:
        reasons.append("concurrency: required detail passed verdict is not true")
    for key, expected in (
        ("maximum_samples_per_worker", 100_000),
        ("measurement_timeout_secs", 600),
    ):
        if type(detail.get(key)) is not int or detail[key] != expected:
            reasons.append(f"concurrency: detail {key} differs from producer budget")
    if detail.get("client_counts") != list(CONCURRENCY_COUNTS) or any(
        type(c) is not int for c in detail.get("client_counts", [])
    ):
        reasons.append("concurrency: detail client_counts inventory mismatch")
    if detail.get("mixed_routes") != list(CONCURRENCY_ROUTES):
        reasons.append("concurrency: detail mixed_routes inventory mismatch")
    if (
        type(detail.get("minimum_row_samples")) is not int
        or detail["minimum_row_samples"] != minimum
    ):
        reasons.append("concurrency: detail minimum_row_samples differs from registry")
    measurements = detail.get("measurements")
    if not isinstance(measurements, list):
        return reasons + ["concurrency: measurements is not an array"]
    clients = [m.get("clients") if isinstance(m, dict) else None for m in measurements]
    if any(type(c) is not int for c in clients) or clients != list(CONCURRENCY_COUNTS):
        reasons.append("concurrency: measurement client inventory must be exactly 1, 8, 32")
    groups_by_client: dict[int, list[dict]] = {}
    for index, measurement in enumerate(measurements):
        where = f"concurrency: measurements[{index}]"
        if not isinstance(measurement, dict):
            reasons.append(f"{where} is not an object")
            continue
        missing = {"clients", "requests_per_client", "window_secs", "routes", "fast", "slow"} - set(
            measurement
        )
        if missing:
            reasons.append(f"{where} missing fields: {sorted(missing)}")
        c = measurement.get("clients")
        if type(c) is not int or c not in CONCURRENCY_COUNTS:
            continue
        planned, window = measurement.get("requests_per_client"), measurement.get("window_secs")
        if type(planned) is not int or not minimum * len(CONCURRENCY_ROUTES) <= planned <= 100_000:
            reasons.append(f"{where} requests_per_client is outside producer bounds")
        if not _non_negative_number(window) or window <= 0:
            reasons.append(f"{where} window_secs must be finite and positive")
        routes, fast, slow = (
            measurement.get("routes"),
            measurement.get("fast"),
            measurement.get("slow"),
        )
        if not isinstance(routes, list) or [
            r.get("label") if isinstance(r, dict) else None for r in routes
        ] != list(CONCURRENCY_ROUTES):
            reasons.append(f"{where} route inventory mismatch")
            routes = []
        if not isinstance(fast, dict) or fast.get("label") != "fast":
            reasons.append(f"{where} fast summary missing or mislabeled")
            fast = None
        if (c == 1 and slow is not None) or (
            c > 1 and (not isinstance(slow, dict) or slow.get("label") != "slow")
        ):
            reasons.append(f"{where} slow client inventory mismatch")
        groups = (
            routes
            + ([fast] if fast is not None else [])
            + ([slow] if isinstance(slow, dict) else [])
        )
        groups_by_client[c] = groups
        for group in groups:
            missing = {
                "label",
                "requests",
                "served",
                "error_count",
                "timeout_count",
                "qps",
                "latency",
                "error_codes",
                "last_result_count",
            } - set(group)
            if missing:
                reasons.append(f"{where} group missing fields: {sorted(missing)}")
            label = group.get("label")
            name = f"{where}.{label}"
            counts = [
                group.get(key) for key in ("requests", "served", "error_count", "timeout_count")
            ]
            limit = 100_000 if label == "slow" else c * 100_000
            valid_counts = all(type(value) is int and 0 <= value <= limit for value in counts)
            if not valid_counts:
                reasons.append(f"{name} counters must be non-negative integers")
            else:
                requests, served, errors, timeouts = counts
                if requests != served + errors + timeouts:
                    reasons.append(f"{name} request accounting mismatch")
                latency = group.get("latency")
                if (
                    not isinstance(latency, dict)
                    or latency.get("samples") != served + errors
                    or type(latency.get("samples")) is not int
                ):
                    reasons.append(f"{name} answered samples mismatch")
                if timeouts != 0:
                    reasons.append(f"{name} timeout contradicts required passed verdict")
                if _non_negative_number(window) and window > 0:
                    qps = group.get("qps")
                    if not _non_negative_number(qps) or not _qps_matches(qps, served, window):
                        reasons.append(f"{name} qps does not equal served / window_secs")
            latency = group.get("latency")
            if not isinstance(latency, dict) or set(latency) != {
                "p50_ms",
                "p95_ms",
                "p99_ms",
                "samples",
            }:
                reasons.append(f"{name} malformed latency summary")
            else:
                percentiles = [latency[key] for key in ("p50_ms", "p95_ms", "p99_ms")]
                if (
                    not all(_non_negative_number(value) for value in percentiles)
                    or not percentiles[0] <= percentiles[1] <= percentiles[2]
                ):
                    reasons.append(f"{name} malformed latency percentiles")
            if (
                not isinstance(latency, dict)
                or type(latency.get("samples")) is not int
                or latency["samples"] < minimum
            ):
                reasons.append(f"{name} needs at least {minimum} samples")
            last = group.get("last_result_count")
            if last is not None and (type(last) is not int or last < 0):
                reasons.append(f"{name} malformed last_result_count")
            codes = group.get("error_codes")
            if (
                not isinstance(codes, list)
                or any(not isinstance(code, str) or not code for code in codes)
                or len(set(codes)) != len(codes)
            ):
                reasons.append(f"{name} error_codes must be distinct non-empty strings")
            elif type(group.get("error_count")) is int and bool(codes) != (
                group["error_count"] > 0
            ):
                reasons.append(f"{name} error_codes disagree with error_count")
            elif type(group.get("error_count")) is int and len(codes) > group["error_count"]:
                reasons.append(f"{name} distinct error_codes outnumber typed error responses")
            if type(group.get("served")) is int and (group["served"] > 0) != (
                group.get("last_result_count") is not None
            ):
                reasons.append(f"{name} last_result_count disagrees with served count")
        if fast is not None and len(routes) == len(CONCURRENCY_ROUTES):
            for key in ("requests", "served", "error_count", "timeout_count"):
                values = [r.get(key) for r in routes]
                if all(type(v) is int and v >= 0 for v in values) and fast.get(key) != sum(values):
                    reasons.append(f"{where} fast {key} does not equal route sum")
            route_requests = [r.get("requests") for r in routes]
            if all(type(value) is int for value in route_requests) and (
                any(left < right for left, right in zip(route_requests, route_requests[1:]))
                or route_requests[0] - route_requests[-1] > c
            ):
                reasons.append(f"{where} route requests violate per-client round-robin partition")
            if (
                type(planned) is int
                and planned >= 0
                and all(type(value) is int for value in route_requests)
            ):
                for route_index, actual in enumerate(route_requests):
                    floor = c * (
                        planned // len(CONCURRENCY_ROUTES)
                        + int(route_index < planned % len(CONCURRENCY_ROUTES))
                    )
                    if actual < floor:
                        reasons.append(f"{where} route requests below planned round-robin share")
            route_codes = [r.get("error_codes") for r in routes]
            if (
                isinstance(fast.get("error_codes"), list)
                and all(
                    isinstance(codes, list) and all(isinstance(code, str) for code in codes)
                    for codes in route_codes
                )
                and all(isinstance(code, str) for code in fast["error_codes"])
            ):
                if set(fast["error_codes"]) != {code for codes in route_codes for code in codes}:
                    reasons.append(f"{where} fast error_codes do not equal route union")
            if (
                type(planned) is int
                and type(fast.get("requests")) is int
                and fast["requests"] < c * planned
            ):
                reasons.append(f"{where} fast requests below planned client budget")
    row_ids = [r.get("scenario_id") if isinstance(r, dict) else None for r in rows]
    identities = []
    for row_id in row_ids:
        match = (
            re.fullmatch(
                r"concurrency\.c(1|8|32)\.(lexical|semantic|hybrid|symbol|lexical_count|fast|slow)",
                row_id,
            )
            if isinstance(row_id, str)
            else None
        )
        identities.append(int(match[1]) if match else None)
    if not identities or any(c is None for c in identities) or len(set(identities)) != 1:
        return reasons + ["concurrency: row client identity missing or mixed"]
    selected = identities[0]
    expected_labels = list(CONCURRENCY_ROUTES) + ["fast"] + (["slow"] if selected > 1 else [])
    if row_ids != [f"concurrency.c{selected}.{label}" for label in expected_labels]:
        reasons.append("concurrency: canonical route/fast/slow row inventory mismatch")
    if type(payload.get("concurrency")) is not int or payload["concurrency"] != selected + int(
        selected > 1
    ):
        reasons.append("concurrency: envelope disagrees with fast/slow clients")
    if artifact_path is not None and artifact_path.name != f"summary-c{selected}.json":
        reasons.append("concurrency: filename disagrees with measured fast clients")
    groups = {
        g["label"]: g for g in groups_by_client.get(selected, []) if isinstance(g.get("label"), str)
    }
    for row in rows:
        if not isinstance(row, dict) or not isinstance(row.get("scenario_id"), str):
            continue
        label = row["scenario_id"].rsplit(".", 1)[-1]
        group = groups.get(label)
        if group is None:
            reasons.append("concurrency: row has no matching detail group")
            continue
        expected = {
            "latency": group.get("latency"),
            "qps": group.get("qps"),
            "error_count": group.get("error_count"),
            "timeout_count": group.get("timeout_count"),
            "result_count": group.get("last_result_count"),
            "typed_error_code": (group.get("error_codes") or [None])[0]
            if isinstance(group.get("error_codes"), list)
            else None,
            "route_family": label if label in ("semantic", "hybrid", "symbol") else "lexical",
            "syntax": "native",
            "engine_touched": [],
            "early_stop_reason": None,
            "result_shape": "candidates"
            if type(group.get("served")) is int and group["served"] > 0
            else "typed_error"
            if type(group.get("error_count")) is int and group["error_count"] > 0
            else "empty",
        }
        for key, value in expected.items():
            if row.get(key) != value:
                reasons.append(f"concurrency: {row['scenario_id']} {key} disagrees with detail")
    return reasons
