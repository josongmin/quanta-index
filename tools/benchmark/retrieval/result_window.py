"""Shared strict validation of product result-window evidence."""

_OUTCOME_FIELDS = {
    "exact_exhausted": {"kind"},
    "lower_bound": {"kind", "continuation"},
    "capped_unknown": {"kind", "cap"},
    "interrupted_partial": {"kind", "reason"},
    "approximate": {"kind", "method", "quality_contract"},
}
_EXHAUSTION_COUNT_FIELDS = {
    "probe_exhausted": "fetched",
    "exact_count": "total",
    "universe_scanned": "scanned",
}
_EMPTY_PROVENANCE = ("logical_empty", "available_empty", "filtered_empty", "zero_hit_executed")
_U32_MAX = 2**32 - 1
_U64_MAX = 2**64 - 1


class WindowEvidenceError(ValueError):
    pass


def _exact_keys(payload: object, keys: set[str], where: str) -> dict:
    if not isinstance(payload, dict) or set(payload) != keys:
        raise WindowEvidenceError(f"{where} must hold exactly {sorted(keys)}")
    return payload


def _is_wire_uint(value: object, maximum: int = _U64_MAX) -> bool:
    return type(value) is int and 0 <= value <= maximum


def typed_count(value: object, where: str) -> tuple[str, int]:
    count = _exact_keys(value, {"kind", "value"}, where)
    if count["kind"] not in ("exact", "at_least") or not _is_wire_uint(count["value"]):
        raise WindowEvidenceError(f"{where} is invalid")
    return count["kind"], count["value"]


def typed_window(value: object, where: str) -> tuple[int, bool, dict[str, bool]]:
    if not isinstance(value, dict):
        raise WindowEvidenceError(f"{where} is malformed")
    required = {"returned", "candidate_count", "outcome", "coverage"}
    if set(value) not in (required, required | {"empty_provenance"}):
        raise WindowEvidenceError(f"{where} fields are invalid")
    returned = value["returned"]
    if not _is_wire_uint(returned, _U32_MAX):
        raise WindowEvidenceError(f"{where}.returned is invalid")
    count_kind, count_value = typed_count(value["candidate_count"], f"{where}.candidate_count")
    if count_value < returned:
        raise WindowEvidenceError(f"{where} candidate count is below returned")
    outcome = value["outcome"]
    if not isinstance(outcome, dict) or not isinstance(outcome.get("kind"), str):
        raise WindowEvidenceError(f"{where}.outcome is malformed")
    outcome_kind = outcome["kind"]
    if set(outcome) != _OUTCOME_FIELDS.get(outcome_kind):
        raise WindowEvidenceError(f"{where}.outcome fields are invalid")
    coverage = _exact_keys(
        value["coverage"],
        {"examined", "lanes"}
        | (
            {"exhaustion_proof"}
            if isinstance(value["coverage"], dict) and "exhaustion_proof" in value["coverage"]
            else set()
        ),
        f"{where}.coverage",
    )
    examined = coverage["examined"]
    if not isinstance(examined, dict) or examined.get("kind") not in (
        "exact",
        "at_least",
        "unknown",
    ):
        raise WindowEvidenceError(f"{where}.coverage.examined is invalid")
    if examined["kind"] == "unknown":
        if set(examined) != {"kind"}:
            raise WindowEvidenceError(f"{where}.coverage.examined fields are invalid")
    else:
        typed_count(examined, f"{where}.coverage.examined")
    proof = coverage.get("exhaustion_proof")
    logical_empty = proof == {"kind": "logical_empty"}
    exhausted = outcome_kind == "exact_exhausted"
    if exhausted:
        if count_kind != "exact" or count_value != returned or not isinstance(proof, dict):
            raise WindowEvidenceError(f"{where} exact exhaustion evidence is invalid")
        if not logical_empty:
            proof_kind = proof.get("kind")
            proof_field = (
                _EXHAUSTION_COUNT_FIELDS.get(proof_kind) if isinstance(proof_kind, str) else None
            )
            if (
                proof_field is None
                or set(proof) != {"kind", proof_field}
                or type(proof[proof_field]) is not int
                or proof[proof_field] != returned
            ):
                raise WindowEvidenceError(f"{where}.coverage.exhaustion_proof is invalid")
    elif "exhaustion_proof" in coverage:
        raise WindowEvidenceError(f"{where} non-exhausted outcome carries an exhaustion proof")
    if outcome_kind == "lower_bound":
        if type(outcome["continuation"]) is not bool or (
            outcome["continuation"] and count_value <= returned
        ):
            raise WindowEvidenceError(f"{where} lower-bound outcome is invalid")
    elif outcome_kind == "capped_unknown":
        if not _is_wire_uint(outcome["cap"], _U32_MAX) or outcome["cap"] == 0:
            raise WindowEvidenceError(f"{where} capped outcome is invalid")
    elif outcome_kind == "interrupted_partial":
        if outcome["reason"] not in ("deadline", "cancelled", "examined_budget"):
            raise WindowEvidenceError(f"{where} interrupted outcome is invalid")
    elif outcome_kind == "approximate":
        quality = outcome["quality_contract"]
        if (
            outcome["method"] not in ("ann_search", "filtered_refill")
            or not isinstance(quality, dict)
            or set(quality) != {"examined_lower_bound"}
            or not _is_wire_uint(quality["examined_lower_bound"])
        ):
            raise WindowEvidenceError(f"{where} approximate outcome is invalid")
    empty = value.get("empty_provenance")
    if (returned == 0 and empty not in _EMPTY_PROVENANCE) or (
        returned > 0 and "empty_provenance" in value
    ):
        raise WindowEvidenceError(f"{where}.empty_provenance contradicts returned")
    if logical_empty != (empty == "logical_empty") or (
        logical_empty and examined != {"kind": "exact", "value": 0}
    ):
        raise WindowEvidenceError(f"{where} logical empty proof contradicts coverage or provenance")
    lanes = coverage["lanes"]
    if not isinstance(lanes, list):
        raise WindowEvidenceError(f"{where}.coverage.lanes is malformed")
    lane_execution: dict[str, bool] = {}
    for lane in lanes:
        if (
            not isinstance(lane, dict)
            or set(lane)
            - {"lane", "executed", "contributed", "filtered_out", "candidates", "cost", "profile"}
            or not {"lane", "executed", "contributed", "filtered_out", "candidates"}.issubset(lane)
        ):
            raise WindowEvidenceError(f"{where}.coverage lane fields are invalid")
        name = lane["lane"]
        if (
            not isinstance(name, str)
            or not name
            or name in lane_execution
            or type(lane["executed"]) is not bool
            or type(lane["contributed"]) is not bool
            or lane["contributed"]
            and not lane["executed"]
        ):
            raise WindowEvidenceError(f"{where}.coverage lane is invalid")
        if not _is_wire_uint(lane["filtered_out"]):
            raise WindowEvidenceError(f"{where}.coverage lane filtered_out is invalid")
        lane_count = typed_count(lane["candidates"], f"{where}.coverage lane candidates")
        if returned == 0 and lane["contributed"]:
            raise WindowEvidenceError(f"{where} empty window has a lane that contributed rows")
        if logical_empty and (lane["executed"] or lane["filtered_out"] or lane_count != ("exact", 0)):
            raise WindowEvidenceError(f"{where} logical empty window claims backend execution")
        if "cost" in lane and not _is_wire_uint(lane["cost"]):
            raise WindowEvidenceError(f"{where}.coverage lane cost is invalid")
        if "profile" in lane and (not isinstance(lane["profile"], str) or not lane["profile"]):
            raise WindowEvidenceError(f"{where}.coverage lane profile is invalid")
        lane_execution[name] = lane["executed"]
    return returned, exhausted, lane_execution
