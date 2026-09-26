"""Pure JSON admission for proof envelopes; I/O and schemas remain caller-owned."""

from __future__ import annotations

import json
import math
from typing import Any


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate proof JSON key: {key}")
        result[key] = value
    return result


def _reject_constant(value: str) -> None:
    raise ValueError(f"non-finite proof JSON value: {value}")


def _finite_float(value: str) -> float:
    parsed = float(value)
    if not math.isfinite(parsed):
        raise ValueError(f"non-finite proof JSON value: {value}")
    return parsed


def parse_proof_json(raw: str | bytes) -> Any:
    """Reject ambiguous keys and non-finite numbers before semantic validation."""
    try:
        return json.loads(
            raw,
            object_pairs_hook=_unique_object,
            parse_constant=_reject_constant,
            parse_float=_finite_float,
        )
    except (ValueError, RecursionError) as error:
        raise ValueError(f"invalid proof JSON: {error}") from error
