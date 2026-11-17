"""Summarize Record-only Native source staging markers from raw harness stderr.

This reports staging progress only. It does not establish an event ACK, search,
reopen, or product acceptance result.
"""

from __future__ import annotations

import json
from typing import Any

_EVENT = "native_stage_sources_page"
_PREFIX = '{"event":"native_stage_sources_page'
_PAGE_SIZE = 128  # quanta-index-contract::MAX_NATIVE_STAGE_SOURCES_INPUTS
_MAX_PAGES = 256  # xlarge: 64 repositories * 512 files / 128 inputs
_BEGIN_FIELDS = frozenset({"event", "state", "page", "total_pages", "sources", "stage_elapsed_ms"})
_ACK_FIELDS = frozenset(
    {
        "event",
        "state",
        "page",
        "total_pages",
        "accepted_total",
        "stage_elapsed_ms",
        "page_elapsed_ms",
    }
)


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _reject_nonfinite(value: str) -> None:
    raise ValueError(f"nonfinite JSON number: {value}")


def _integer(row: dict[str, Any], key: str, *, minimum: int = 0, maximum: int | None = None) -> int:
    value = row[key]
    if type(value) is not int or value < minimum or (maximum is not None and value > maximum):
        raise ValueError(f"invalid {key}: expected integral value in range")
    return value


def build_native_staging_report(raw_log: str) -> dict[str, Any]:
    """Validate ordered page begin/ACK records; keep incomplete prefixes explicit."""
    total_pages = None
    begun_pages = 0
    acked_pages = 0
    accepted_total = 0
    open_page = None
    open_sources = None
    open_begin_ms = None
    last_elapsed_ms = 0
    slowest_ack = None
    truncated_tail = False

    lines = raw_log.splitlines(keepends=True)
    for line_number, line in enumerate(lines, 1):
        if not line.startswith(_PREFIX):
            continue
        try:
            row = json.loads(
                line, object_pairs_hook=_unique_object, parse_constant=_reject_nonfinite
            )
        except json.JSONDecodeError as error:
            if line_number == len(lines) and not line.endswith(("\n", "\r")):
                truncated_tail = True
                break
            raise ValueError(f"malformed staging marker at line {line_number}") from error
        except ValueError as error:
            raise ValueError(f"malformed staging marker at line {line_number}") from error
        if not isinstance(row, dict) or row.get("event") != _EVENT:
            raise ValueError(f"invalid staging event at line {line_number}")
        state = row.get("state")
        if state == "begin":
            if row.keys() != _BEGIN_FIELDS:
                raise ValueError(f"invalid staging begin fields at line {line_number}")
        elif state == "ack":
            if row.keys() != _ACK_FIELDS:
                raise ValueError(f"invalid staging ACK fields at line {line_number}")
        else:
            raise ValueError(f"invalid staging state at line {line_number}")

        page = _integer(row, "page", minimum=1, maximum=_MAX_PAGES)
        pages = _integer(row, "total_pages", minimum=1, maximum=_MAX_PAGES)
        elapsed_ms = _integer(row, "stage_elapsed_ms")
        if page > pages or (total_pages is not None and pages != total_pages):
            raise ValueError(f"inconsistent staging page count at line {line_number}")
        if elapsed_ms < last_elapsed_ms:
            raise ValueError(f"regressing staging clock at line {line_number}")
        last_elapsed_ms = elapsed_ms
        total_pages = pages

        if state == "begin":
            sources = _integer(row, "sources", minimum=1, maximum=_PAGE_SIZE)
            if page != acked_pages + 1 or open_page is not None:
                raise ValueError(f"out-of-order staging begin at line {line_number}")
            if page < pages and sources != _PAGE_SIZE:
                raise ValueError(f"short nonfinal staging page at line {line_number}")
            open_page = page
            open_sources = sources
            open_begin_ms = elapsed_ms
            begun_pages += 1
        else:
            page_ms = _integer(row, "page_elapsed_ms")
            accepted = _integer(row, "accepted_total", minimum=1)
            if page != open_page or open_sources is None or open_begin_ms is None:
                raise ValueError(f"staging ACK without matching begin at line {line_number}")
            if page_ms != elapsed_ms - open_begin_ms:
                raise ValueError(f"inconsistent staging ACK duration at line {line_number}")
            if accepted != accepted_total + open_sources:
                raise ValueError(f"inconsistent staging accepted count at line {line_number}")
            accepted_total = accepted
            acked_pages += 1
            if slowest_ack is None or page_ms > slowest_ack["page_elapsed_ms"]:
                slowest_ack = {"page": page, "page_elapsed_ms": page_ms}
            open_page = None
            open_sources = None
            open_begin_ms = None

    if truncated_tail:
        status = "INCOMPLETE"
    elif total_pages is None:
        status = "NO_MARKERS"
    elif acked_pages == total_pages:
        status = "COMPLETE"
    else:
        status = "INCOMPLETE"
    return {
        "status": status,
        "total_pages": total_pages,
        "begun_pages": begun_pages,
        "acked_pages": acked_pages,
        "last_accepted_count": accepted_total if total_pages is not None else None,
        "open_page": open_page,
        "slowest_ack": slowest_ack,
        **({"truncated_tail": True} if truncated_tail else {}),
    }
