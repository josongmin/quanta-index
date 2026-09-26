"""CLI for validating and summarizing recorded A/B/C agent outcomes."""

from __future__ import annotations

import argparse
import json
import re
import sys
from decimal import Decimal, InvalidOperation
from pathlib import Path
from typing import Any, Optional

# Script and imported adapter entrypoints use the same file-custody owner.
BENCHMARK_ROOT = Path(__file__).resolve().parents[1]
if str(BENCHMARK_ROOT) not in sys.path:
    sys.path.insert(0, str(BENCHMARK_ROOT))
from evidence import CONTROL_DOCUMENT_BYTES, EvidenceError, RawFile  # noqa: E402

ARMS = ("A", "B", "C")
ARM_KINDS = {"A": "no_index", "B": "production_router", "C": "oracle_gold_context"}
SHA256 = re.compile(r"^sha256:[0-9a-f]{64}$")
COMMIT = re.compile(r"^[0-9a-f]{40}$")
ROW_KEYS = {
    "schema_version",
    "task_id",
    "task_digest",
    "checkout_commit",
    "trial_id",
    "arm",
    "arm_kind",
    "arm_config_digest",
    "model_id",
    "model_revision",
    "model_config_digest",
    "scaffold_digest",
    "budget",
    "baseline_tests",
    "post_tests",
    "outcome_status",
    "trajectory",
    "usage",
}
BUDGET_KEYS = {"max_total_tokens", "max_tool_calls", "max_elapsed_ms", "max_cost_usd"}
USAGE_KEYS = {"input_tokens", "output_tokens", "tool_calls", "elapsed_ms", "cost_usd"}


class InvalidEvidence(ValueError):
    """The input cannot be used as benchmark evidence."""


def _object_no_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise InvalidEvidence(f"duplicate JSON key: {key!r}")
        result[key] = value
    return result


def _reject_constant(value: str) -> None:
    raise InvalidEvidence(f"non-finite JSON number: {value}")


def _keys(value: Any, expected: set, path: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != expected:
        raise InvalidEvidence(f"{path} must have exactly keys {sorted(expected)}")
    return value


def _string(value: Any, path: str) -> str:
    if not isinstance(value, str) or not value or value.strip() != value:
        raise InvalidEvidence(f"{path} must be a nonempty trimmed string")
    return value


def _digest(value: Any, path: str) -> str:
    if not isinstance(value, str) or SHA256.fullmatch(value) is None:
        raise InvalidEvidence(f"{path} must be sha256:<64 lowercase hex>")
    return value


def _integer(value: Any, path: str, positive: bool = False) -> int:
    if type(value) is not int or value < (1 if positive else 0):
        raise InvalidEvidence(
            "{} must be a {} integer".format(path, "positive" if positive else "nonnegative")
        )
    return value


def _money(value: Any, path: str) -> Decimal:
    if isinstance(value, bool) or not isinstance(value, (int, Decimal)):
        raise InvalidEvidence(f"{path} must be a nonnegative JSON number")
    try:
        amount = Decimal(value)
    except (InvalidOperation, TypeError, ValueError) as exc:
        raise InvalidEvidence(f"{path} must be a finite JSON number") from exc
    if not amount.is_finite() or amount < 0:
        raise InvalidEvidence(f"{path} must be a nonnegative finite JSON number")
    return amount


def _tests(value: Any, path: str) -> dict[str, str]:
    if not isinstance(value, dict) or not value:
        raise InvalidEvidence(f"{path} must contain test results")
    for test_id, status in value.items():
        _string(test_id, f"{path}.test_id")
        if status not in ("pass", "fail"):
            raise InvalidEvidence(f"{path}.{test_id} must be pass or fail")
    return value


def _trajectory(value: Any, path: str) -> tuple[int, int, Optional[int]]:  # noqa: UP045
    if not isinstance(value, list) or len(value) < 2:
        raise InvalidEvidence(f"{path} needs start and finish events")
    previous_ms = -1
    tool_calls = 0
    first_evidence: Optional[int] = None  # noqa: UP045 - Python 3.9
    seen_calls = set()
    seen_evidence = set()
    for index, event in enumerate(value):
        prefix = f"{path}[{index}]"
        if not isinstance(event, dict):
            raise InvalidEvidence(f"{prefix} must be an object")
        kind = event.get("kind")
        expected = {"seq", "elapsed_ms", "kind"}
        if kind == "tool_call":
            expected.add("call_id")
        if kind == "evidence":
            expected |= {"evidence_id", "source_call_id", "useful"}
        _keys(event, expected, prefix)
        if kind not in ("start", "finish", "tool_call", "evidence", "other"):
            raise InvalidEvidence(f"{prefix} has unknown kind")
        if _integer(event["seq"], prefix + ".seq") != index:
            raise InvalidEvidence(f"{prefix} has noncontiguous seq")
        elapsed = _integer(event["elapsed_ms"], prefix + ".elapsed_ms")
        if elapsed < previous_ms:
            raise InvalidEvidence(f"{prefix} is earlier than previous event")
        previous_ms = elapsed
        if index == 0 and (kind != "start" or elapsed != 0):
            raise InvalidEvidence(f"{path} must start at elapsed_ms=0")
        if index == len(value) - 1 and kind != "finish":
            raise InvalidEvidence(f"{path} must end with finish")
        if kind in ("start", "finish") and index not in (0, len(value) - 1):
            raise InvalidEvidence(f"{prefix} has misplaced boundary event")
        if kind == "tool_call":
            call_id = _string(event["call_id"], prefix + ".call_id")
            if call_id in seen_calls:
                raise InvalidEvidence(f"{prefix} has duplicate call_id")
            seen_calls.add(call_id)
            tool_calls += 1
        if kind == "evidence":
            evidence_id = _string(event["evidence_id"], prefix + ".evidence_id")
            if evidence_id in seen_evidence:
                raise InvalidEvidence(f"{prefix} has duplicate evidence_id")
            seen_evidence.add(evidence_id)
            source_call_id = _string(event["source_call_id"], prefix + ".source_call_id")
            if source_call_id not in seen_calls:
                raise InvalidEvidence(f"{prefix} references a missing or future tool call")
            if type(event["useful"]) is not bool:
                raise InvalidEvidence(f"{prefix}.useful must be boolean")
            if event["useful"] and first_evidence is None:
                first_evidence = elapsed
    return tool_calls, previous_ms, first_evidence


def _validate_row(row: Any, line: int) -> dict[str, Any]:
    prefix = f"line {line}"
    _keys(row, ROW_KEYS, prefix)
    if type(row["schema_version"]) is not int or row["schema_version"] != 1:
        raise InvalidEvidence(f"{prefix}: unsupported schema_version")
    for key in ("task_id", "trial_id", "model_id", "model_revision"):
        _string(row[key], prefix + "." + key)
    for key in ("task_digest", "arm_config_digest", "model_config_digest", "scaffold_digest"):
        _digest(row[key], prefix + "." + key)
    if (
        not isinstance(row["checkout_commit"], str)
        or COMMIT.fullmatch(row["checkout_commit"]) is None
    ):
        raise InvalidEvidence(f"{prefix}.checkout_commit must be a full lowercase Git SHA")
    if row["arm"] not in ARMS:
        raise InvalidEvidence(f"{prefix}.arm must be A, B or C")
    if row["arm_kind"] != ARM_KINDS[row["arm"]]:
        raise InvalidEvidence(f"{prefix}.arm_kind does not match arm {row['arm']}")
    if row["outcome_status"] != "completed":
        raise InvalidEvidence(f"{prefix}.outcome_status must be completed")

    budget = _keys(row["budget"], BUDGET_KEYS, prefix + ".budget")
    usage = _keys(row["usage"], USAGE_KEYS, prefix + ".usage")
    for key in ("max_total_tokens", "max_tool_calls", "max_elapsed_ms"):
        _integer(budget[key], prefix + ".budget." + key, positive=True)
    for key in ("input_tokens", "output_tokens", "tool_calls", "elapsed_ms"):
        _integer(usage[key], prefix + ".usage." + key)
    max_cost = _money(budget["max_cost_usd"], prefix + ".budget.max_cost_usd")
    cost = _money(usage["cost_usd"], prefix + ".usage.cost_usd")
    if usage["input_tokens"] + usage["output_tokens"] > budget["max_total_tokens"]:
        raise InvalidEvidence(f"{prefix}: token budget exceeded")
    if usage["tool_calls"] > budget["max_tool_calls"]:
        raise InvalidEvidence(f"{prefix}: tool-call budget exceeded")
    if usage["elapsed_ms"] > budget["max_elapsed_ms"]:
        raise InvalidEvidence(f"{prefix}: time budget exceeded")
    if cost > max_cost:
        raise InvalidEvidence(f"{prefix}: cost budget exceeded")

    before = _tests(row["baseline_tests"], prefix + ".baseline_tests")
    after = _tests(row["post_tests"], prefix + ".post_tests")
    if set(before) != set(after):
        raise InvalidEvidence(f"{prefix}: post_tests must cover exactly baseline_tests")
    if "pass" not in before.values() or "fail" not in before.values():
        raise InvalidEvidence(f"{prefix}: baseline needs both passing and failing tests")
    calls, elapsed, first_evidence = _trajectory(row["trajectory"], prefix + ".trajectory")
    if usage["tool_calls"] != calls or usage["elapsed_ms"] != elapsed:
        raise InvalidEvidence(f"{prefix}: usage tool_calls/elapsed_ms disagrees with trajectory")
    row["_first_evidence_ms"] = first_evidence
    return row


def _paired(rows: list[dict[str, Any]]) -> list[dict[str, dict[str, Any]]]:
    identities: dict[str, tuple[str, str]] = {}
    baselines: dict[str, dict[str, str]] = {}
    arm_configs: dict[str, str] = {}
    pairs: dict[tuple[str, str], dict[str, dict[str, Any]]] = {}
    for row in rows:
        arm = row["arm"]
        if arm in arm_configs and arm_configs[arm] != row["arm_config_digest"]:
            raise InvalidEvidence(f"arm {arm} has conflicting configuration digests")
        arm_configs[arm] = row["arm_config_digest"]
        task_id = row["task_id"]
        identity = (row["task_digest"], row["checkout_commit"])
        if task_id in identities and identities[task_id] != identity:
            raise InvalidEvidence(f"task_id {task_id!r} has conflicting immutable identity")
        identities[task_id] = identity
        if task_id in baselines and baselines[task_id] != row["baseline_tests"]:
            raise InvalidEvidence(f"task_id {task_id!r} has conflicting baseline tests")
        baselines[task_id] = row["baseline_tests"]
        key = (task_id, row["trial_id"])
        group = pairs.setdefault(key, {})
        if arm in group:
            raise InvalidEvidence(
                "duplicate trajectory for task/trial/arm {!r}/{!r}/{}".format(*key, arm)
            )
        group[arm] = row
    result: list[dict[str, dict[str, Any]]] = []
    if len(set(arm_configs.values())) != len(arm_configs):
        raise InvalidEvidence("A/B/C arms must have distinct configuration digests")
    for key in sorted(pairs):
        group = pairs[key]
        if set(group) != set(ARMS):
            raise InvalidEvidence(
                "task/trial {!r}/{!r} needs exactly A/B/C; found {}".format(*key, sorted(group))
            )
        reference = group["A"]
        for arm in ("B", "C"):
            current = group[arm]
            for field in (
                "task_digest",
                "checkout_commit",
                "model_id",
                "model_revision",
                "model_config_digest",
                "scaffold_digest",
                "budget",
                "baseline_tests",
            ):
                if current[field] != reference[field]:
                    raise InvalidEvidence(
                        "task/trial {!r}/{!r}: {} differs in arm {}".format(*key, field, arm)
                    )
        result.append(group)
    return result


def load_file(raw: RawFile) -> list[dict[str, dict[str, Any]]]:
    """Validate one committed JSONL stream; retain only bounded pair metadata."""

    def consume(lines):
        rows: list[dict[str, Any]] = []
        metadata_bytes = 0
        for line, raw_line in enumerate(lines, 1):
            try:
                text = raw_line.decode("utf-8")
            except UnicodeError as exc:
                raise InvalidEvidence(f"line {line}: invalid UTF-8: {exc}") from exc
            if not text.strip():
                raise InvalidEvidence(f"line {line}: blank JSONL row")
            try:
                row = json.loads(
                    text,
                    parse_float=Decimal,
                    parse_constant=_reject_constant,
                    object_pairs_hook=_object_no_duplicates,
                )
            except (json.JSONDecodeError, InvalidEvidence) as exc:
                raise InvalidEvidence(f"line {line}: {exc}") from exc
            checked = _validate_row(row, line)
            del checked["trajectory"]  # Keep only validated metrics and pair identity.
            # Pairing needs global identity and ordering, not raw trajectory
            # retention. Bound that control state independently of raw size.
            metadata_bytes += len(json.dumps(checked, default=str, separators=(",", ":")))
            if metadata_bytes > CONTROL_DOCUMENT_BYTES:
                raise InvalidEvidence("paired metadata exceeds explicit control byte limit")
            rows.append(checked)
        if not rows:
            raise InvalidEvidence("empty JSONL input")
        return _paired(rows)

    try:
        return raw.consume_lines(consume, max_line_bytes=CONTROL_DOCUMENT_BYTES)
    except (OSError, EvidenceError) as exc:
        raise InvalidEvidence(f"cannot read JSONL: {exc}") from exc


def load(path: Path) -> tuple[list[dict[str, dict[str, Any]]], str]:
    try:
        raw = RawFile.capture(path)
        return load_file(raw), raw.sha256
    except (OSError, EvidenceError) as exc:
        raise InvalidEvidence(f"cannot read JSONL: {exc}") from exc


def _metrics(row: dict[str, Any]) -> dict[str, Any]:
    before, after = row["baseline_tests"], row["post_tests"]
    failed = [test_id for test_id, status in before.items() if status == "fail"]
    passed = [test_id for test_id, status in before.items() if status == "pass"]
    ftp = sum(after[test_id] == "pass" for test_id in failed)
    ptp = sum(after[test_id] == "pass" for test_id in passed)
    return {
        "fail_to_pass": {"passed": ftp, "total": len(failed)},
        "pass_to_pass": {"passed": ptp, "total": len(passed)},
        "resolved": ftp == len(failed) and ptp == len(passed),
        "cost_usd": str(_money(row["usage"]["cost_usd"], "cost_usd")),
        "elapsed_ms": row["usage"]["elapsed_ms"],
        "tool_calls": row["usage"]["tool_calls"],
        "first_useful_evidence_ms": row["_first_evidence_ms"],
    }


def _rate(passed: int, total: int) -> str:
    return str(Decimal(passed) / Decimal(total))


def summarize(pairs: list[dict[str, dict[str, Any]]], input_digest: str) -> dict[str, Any]:
    pair_rows: list[dict[str, Any]] = []
    for group in pairs:
        a = group["A"]
        pair_rows.append(
            {
                "task_id": a["task_id"],
                "task_digest": a["task_digest"],
                "checkout_commit": a["checkout_commit"],
                "trial_id": a["trial_id"],
                "arm_config_digests": {arm: group[arm]["arm_config_digest"] for arm in ARMS},
                "arms": {arm: _metrics(group[arm]) for arm in ARMS},
            }
        )
    aggregate: dict[str, Any] = {}
    for arm in ARMS:
        metrics = [pair["arms"][arm] for pair in pair_rows]
        ftp = sum(metric["fail_to_pass"]["passed"] for metric in metrics)
        ftp_total = sum(metric["fail_to_pass"]["total"] for metric in metrics)
        ptp = sum(metric["pass_to_pass"]["passed"] for metric in metrics)
        ptp_total = sum(metric["pass_to_pass"]["total"] for metric in metrics)
        evidence = [
            metric["first_useful_evidence_ms"]
            for metric in metrics
            if metric["first_useful_evidence_ms"] is not None
        ]
        aggregate[arm] = {
            "fail_to_pass": {"passed": ftp, "total": ftp_total, "rate": _rate(ftp, ftp_total)},
            "pass_to_pass": {"passed": ptp, "total": ptp_total, "rate": _rate(ptp, ptp_total)},
            "resolved_pairs": sum(metric["resolved"] for metric in metrics),
            "total_pairs": len(metrics),
            "mean_cost_usd": str(
                sum(Decimal(metric["cost_usd"]) for metric in metrics) / len(metrics)
            ),
            "mean_elapsed_ms": str(
                Decimal(sum(metric["elapsed_ms"] for metric in metrics)) / len(metrics)
            ),
            "mean_tool_calls": str(
                Decimal(sum(metric["tool_calls"] for metric in metrics)) / len(metrics)
            ),
            "useful_evidence_pairs": len(evidence),
            "mean_first_useful_evidence_ms_when_present": (
                str(Decimal(sum(evidence)) / len(evidence)) if evidence else None
            ),
        }
    paired: dict[str, Any] = {}
    for contender, reference in (("B", "A"), ("C", "A"), ("C", "B")):
        wins = losses = ties = 0
        for pair in pair_rows:
            delta = int(pair["arms"][contender]["resolved"]) - int(
                pair["arms"][reference]["resolved"]
            )
            wins += delta > 0
            losses += delta < 0
            ties += delta == 0
        paired[f"{contender}_vs_{reference}"] = {"wins": wins, "losses": losses, "ties": ties}
    return {
        "schema_version": 1,
        "input_sha256": input_digest,
        "record_count": len(pairs) * len(ARMS),
        "pair_count": len(pairs),
        "aggregate": aggregate,
        "paired_resolved": paired,
        "pairs": pair_rows,
    }


def main(argv: Optional[list[str]] = None) -> int:  # noqa: UP045
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    for command in ("validate", "summarize"):
        subparsers.add_parser(command).add_argument(
            "input", type=Path, help="recorded JSONL trajectories"
        )
    args = parser.parse_args(argv)
    try:
        pairs, digest = load(args.input)
    except InvalidEvidence as exc:
        print(json.dumps({"status": "invalid", "error": str(exc)}, sort_keys=True), file=sys.stderr)
        return 2
    if args.command == "validate":
        output = {
            "status": "valid",
            "input_sha256": digest,
            "record_count": len(pairs) * 3,
            "pair_count": len(pairs),
        }
    else:
        output = summarize(pairs, digest)
    print(json.dumps(output, sort_keys=True, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
