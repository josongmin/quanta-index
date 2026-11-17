"""Extract one operation-bound Native refusal from retained daemon stderr.

The WorkBudget debug string is diagnostic text, not a stable wire schema.
Unrecognized fields remain absent rather than becoming an inferred cause.
"""

from __future__ import annotations

import json
import re
from typing import Any

from tools.benchmark.evidence import parse_json

_EVENT = "native_work_refusal"
_MAX_RECORD_BYTES = 256 * 1024
_DENIAL = re.compile(r'\bfirst_denial:\s*"([a-z_]+)"')
_STAGE = re.compile(r"\bfailed_stage:\s*Some\(([A-Za-z][A-Za-z0-9_]*)\)")
_LOCATION_BODY = (
    r'Location\s*\{\s*file:\s*"(?P<file>(?:\\.|[^"\\])*)",\s*'
    r"line:\s*(?P<line>\d+),\s*column:\s*(?P<column>\d+)\s*\}"
)
_CHECKPOINT_LOCATION = re.compile(r"\bdenied_checkpoint:\s*Some\(" + _LOCATION_BODY + r"\)")
_CHARGE_LOCATION = re.compile(
    r'\bdenied_charge:\s*Some\(DeniedCharge\s*\{\s*axis:\s*"'
    r'(?P<axis>reads|writes|bytes|mutations)",\s*used:\s*\d+,\s*'
    r"attempted:\s*\d+,\s*limit:\s*\d+,\s*overflow:\s*(?:true|false),\s*caller:\s*"
    + _LOCATION_BODY
    + r"\s*\}\)"
)


def _typed_equal(left: Any, right: Any) -> bool:
    if type(left) is not type(right):
        return False
    if isinstance(left, dict):
        return left.keys() == right.keys() and all(
            _typed_equal(value, right[key]) for key, value in left.items()
        )
    if isinstance(left, list):
        return len(left) == len(right) and all(_typed_equal(a, b) for a, b in zip(left, right))
    return left == right


def _checkpoint(debug: str) -> dict[str, str | int] | None:
    denial = _DENIAL.search(debug)
    if denial is None:
        return None
    if denial.group(1) in {"cancelled", "deadline"}:
        match = _CHECKPOINT_LOCATION.search(debug)
    else:
        # Budget charges retain their caller separately from cancellation and
        # deadline checkpoints. Do not adopt an unrelated or different-axis site.
        match = _CHARGE_LOCATION.search(debug)
        if match is not None and match["axis"] != denial.group(1):
            return None
    if match is None:
        return None
    try:
        file = json.loads('"' + match["file"] + '"')
    except ValueError:
        return None
    line, column = int(match["line"]), int(match["column"])
    if not isinstance(file, str) or len(file) > 4096 or line < 1 or column < 1:
        return None
    return {"file": file, "line": line, "column": column}


def find_native_refusal(raw: str, *, operation_key: dict | None) -> dict | None:
    """Return one exact-key refusal, or refuse an ambiguous selected log.

    Without an operation key the projection is explicitly unbound. Distinct
    records cannot be safely chosen by line order or by the most recent denial.
    """
    if operation_key is not None and not isinstance(operation_key, dict):
        raise TypeError("operation_key must be a dict or None")
    selected: tuple[dict, dict] | None = None
    for line_number, line in enumerate(raw.splitlines(), 1):
        if not line.lstrip().startswith("{") or f'"{_EVENT}"' not in line:
            continue
        if len(line.encode("utf-8")) > _MAX_RECORD_BYTES:
            raise ValueError(f"native refusal record exceeds bound at line {line_number}")
        try:
            record = parse_json(line)
        except ValueError as error:
            raise ValueError(f"malformed native refusal at line {line_number}") from error
        if not isinstance(record, dict) or record.get("event") != _EVENT:
            continue
        key = record.get("operation_key")
        if not isinstance(key, dict):
            raise ValueError(f"native refusal lacks typed operation key at line {line_number}")
        if operation_key is not None and not _typed_equal(key, operation_key):
            continue
        sequence = key.get("sequence")
        debug = record.get("budget_diagnostic")
        outcome = record.get("outcome")
        if (
            type(sequence) is not int
            or sequence < 0
            or not isinstance(debug, str)
            or not isinstance(outcome, str)
            or len(outcome) > 128
        ):
            raise ValueError(f"native refusal has invalid fields at line {line_number}")
        denial = _DENIAL.search(debug)
        stage = _STAGE.search(debug)
        projected = {
            "operation_match": "matched" if operation_key is not None else "unbound",
            "operation_sequence": sequence,
            "first_denial": denial.group(1) if denial is not None else None,
            "failed_stage": stage.group(1) if stage is not None else None,
            "checkpoint": _checkpoint(debug),
            "outcome": outcome,
        }
        if selected is None:
            selected = record, projected
        elif not _typed_equal(selected[0], record):
            raise ValueError("ambiguous native refusal records for selected operation")
    return None if selected is None else selected[1]
