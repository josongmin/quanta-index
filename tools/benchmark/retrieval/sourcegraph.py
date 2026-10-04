"""Offline, unqualified Sourcegraph Stream API capture validator.

This module consumes the documented V3 ``/.api/search/stream`` event format:
``event: <type>\ndata: <JSON>\n\n``. It never contacts Sourcegraph. The caller
must supply an independently captured request envelope, raw response bytes,
the RB corpus manifest, and an explicit assertion of the indexed universe.
The assertion is checked against the manifest but is not backend attestation;
no output of this adapter qualifies an RB-00..RB-06 pair or quality verdict.

Protocol reference: https://sourcegraph.com/docs/api/stream-api
Query filters: https://sourcegraph.com/docs/code-search/queries
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import re
from pathlib import Path, PurePosixPath
from typing import Any

HEX40 = re.compile(r"[0-9a-f]{40}\Z")
HEX64 = re.compile(r"[0-9a-f]{64}\Z")
BARE_KEYWORD_PATTERN = re.compile(r"[\w][\w .-]*\Z", re.UNICODE)
BOOLEAN_OPERATORS = frozenset({"and", "or", "not"})
EVENT_TYPES = frozenset({"matches", "progress", "filters", "alert", "done"})


class CaptureError(ValueError):
    """A capture is malformed, incomplete, or outside its declared inputs."""


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _object_pairs(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    output: dict[str, Any] = {}
    for key, value in pairs:
        if key in output:
            raise CaptureError(f"duplicate JSON key: {key}")
        output[key] = value
    return output


def _reject_constant(value: str) -> Any:
    raise CaptureError(f"non-finite JSON number: {value}")


def _json(data: bytes, where: str) -> Any:
    try:
        return json.loads(
            data.decode("utf-8", "strict"),
            object_pairs_hook=_object_pairs,
            parse_constant=_reject_constant,
        )
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise CaptureError(f"{where} is not strict UTF-8 JSON: {exc}") from exc


def _keys(value: Any, expected: set[str], where: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != expected:
        raise CaptureError(f"{where} must have exactly {sorted(expected)}")
    return value


def _hex(value: Any, pattern: re.Pattern[str], where: str) -> str:
    if not isinstance(value, str) or pattern.fullmatch(value) is None:
        raise CaptureError(f"{where} must be a lowercase hexadecimal digest")
    return value


def _path(value: Any) -> str:
    if (
        not isinstance(value, str)
        or not value
        or "\\" in value
        or "\x00" in value
        or value.startswith("/")
        or any(part in ("", ".", "..") for part in value.split("/"))
        or PurePosixPath(value).as_posix() != value
    ):
        raise CaptureError(f"unsafe or noncanonical result path: {value!r}")
    return value


def _files(value: Any, where: str) -> list[dict[str, str]]:
    if not isinstance(value, list) or not value:
        raise CaptureError(f"{where} must be a nonempty file list")
    result: list[dict[str, str]] = []
    for i, item in enumerate(value):
        item = _keys(item, {"path", "file_sha256"}, f"{where}[{i}]")
        result.append(
            {
                "path": _path(item["path"]),
                "file_sha256": _hex(item["file_sha256"], HEX64, f"{where}[{i}].file_sha256"),
            }
        )
    paths = [item["path"] for item in result]
    if paths != sorted(set(paths)):
        raise CaptureError(f"{where} paths must be sorted and unique")
    return result


def file_extensions(paths: list[str]) -> list[str]:
    """Return a stable extension filter for a nonempty canonical path set."""
    if not isinstance(paths, list) or not paths:
        raise CaptureError("file extension filter requires a nonempty path list")
    for path in paths:
        _path(path)
    extensions = sorted({PurePosixPath(path).suffix for path in paths})
    if any(re.fullmatch(r"\.[A-Za-z0-9]+", extension) is None for extension in extensions):
        raise CaptureError("manifest paths must have simple file extensions")
    return extensions


def keyword_pattern(query: str) -> str:
    """Compile plain query data into an AND of literal content terms.

    Retain the existing wire expression for safe bare terms. Otherwise quote
    each whitespace-delimited term through ``content:`` so caller data cannot
    become a filter, boolean operator, negation, regex or phrase expression.
    """
    if not isinstance(query, str) or not query.isprintable() or not query.strip():
        raise CaptureError("query must be nonempty printable single-line keyword data")
    terms = query.split()
    if BARE_KEYWORD_PATTERN.fullmatch(query) and not any(
        term.casefold() in BOOLEAN_OPERATORS or term.startswith("-") for term in terms
    ):
        return query
    return " ".join("content:" + json.dumps(term, ensure_ascii=False) for term in terms)


def query_expression(
    query: str,
    repository: str,
    revision: str,
    file_paths: list[str] | None = None,
    file_extensions_filter: list[str] | None = None,
) -> str:
    """Literal keyword AND lane, with no caller-provided query syntax."""
    pattern = keyword_pattern(query)
    if not isinstance(repository, str) or not re.fullmatch(
        r"[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)+", repository
    ):
        raise CaptureError("repository must be a canonical slash-delimited name")
    _hex(revision, HEX40, "revision")
    if file_paths is not None and file_extensions_filter is not None:
        raise CaptureError("choose exact file paths or a file extension filter")
    repo_regex = "^" + re.escape(repository) + "$"
    file_filter = ""
    if file_paths is not None:
        if not file_paths or file_paths != sorted(set(file_paths)):
            raise CaptureError("search file universe must be sorted and unique")
        expression = "^(?:" + "|".join(re.escape(_path(path)) for path in file_paths) + ")$"
        file_filter = " file:" + json.dumps(expression, ensure_ascii=False)
    elif file_extensions_filter is not None:
        if (
            not file_extensions_filter
            or file_extensions_filter != sorted(set(file_extensions_filter))
            or any(
                re.fullmatch(r"\.[A-Za-z0-9]+", extension) is None
                for extension in file_extensions_filter
            )
        ):
            raise CaptureError("file extensions must be sorted, unique suffixes")
        suffixes = "|".join(re.escape(extension[1:]) for extension in file_extensions_filter)
        expression = "^(?:.*\\.(?:" + suffixes + "))$"
        file_filter = " file:" + json.dumps(expression, ensure_ascii=False)
    return f"{pattern} repo:{repo_regex} rev:{revision}{file_filter} type:file patternType:keyword count:all"


def _events(raw: bytes) -> list[tuple[str, Any]]:
    if not raw or not raw.endswith(b"\n\n") or b"\r" in raw:
        raise CaptureError("stream must be nonempty LF-framed V3 events")
    frames = raw[:-2].split(b"\n\n")
    events: list[tuple[str, Any]] = []
    for i, frame in enumerate(frames):
        lines = frame.split(b"\n")
        if (
            len(lines) != 2
            or not lines[0].startswith(b"event: ")
            or not lines[1].startswith(b"data: ")
        ):
            raise CaptureError(f"event {i} is not the documented two-field frame")
        try:
            kind = lines[0][7:].decode("ascii", "strict")
        except UnicodeDecodeError as exc:
            raise CaptureError(f"event {i} type is not ASCII") from exc
        if kind not in EVENT_TYPES:
            raise CaptureError(f"unknown event type: {kind}")
        events.append((kind, _json(lines[1][6:], f"event {i} data")))
    return events


def normalize_stream(
    request: dict[str, Any],
    raw: bytes,
    admitted_by_path: dict[str, str],
    capture_version: int,
    *,
    evidence_hashes: bool = True,
) -> tuple[list[tuple[str, Any]], list[dict[str, Any]], list[dict[str, Any]], int, int]:
    """One canonical Sourcegraph stream decoder for timing and evidence replay."""
    events = _events(raw)
    if not events or events[-1] != ("done", {}):
        raise CaptureError("stream lacks a final empty done event")
    if sum(kind == "done" for kind, _ in events) != 1:
        raise CaptureError("stream has multiple done events")
    final_progress = False
    reported_match_count = 0
    matches: list[dict[str, Any]] = []
    seen_matches: set[str] = set()
    native_match_count = 0
    out_of_manifest_match_count = 0
    for i, (kind, data) in enumerate(events[:-1]):
        if kind == "alert":
            raise CaptureError(f"stream alert at event {i}; results may be partial")
        if kind == "progress":
            if (
                not isinstance(data, dict)
                or type(data.get("done")) is not bool
                or not isinstance(data.get("skipped"), list)
                or type(data.get("matchCount")) is not int
                or data["matchCount"] < 0
                or type(data.get("durationMs")) is not int
                or data["durationMs"] < 0
            ):
                raise CaptureError(f"malformed progress at event {i}")
            if data["skipped"]:
                raise CaptureError(f"stream reports skipped results at event {i}")
            if data["matchCount"] < reported_match_count:
                raise CaptureError(f"stream match count regressed at event {i}")
            reported_match_count = data["matchCount"]
            if data["done"]:
                if final_progress:
                    raise CaptureError("multiple final progress events")
                final_progress = True
            elif final_progress:
                raise CaptureError("progress regressed after final progress")
        elif kind == "matches":
            if final_progress:
                raise CaptureError("matches arrived after final progress")
            if not isinstance(data, list) or not data:
                raise CaptureError(f"empty or malformed matches event {i}")
            for hit in data:
                if not isinstance(hit, dict) or hit.get("type") != "content":
                    raise CaptureError("non-content result has no comparable file ranking")
                path = _path(hit.get("path"))
                if (
                    hit.get("repository") != request["repository"]
                    or hit.get("commit") != request["revision"]
                ):
                    raise CaptureError("result repository/revision differs from request")
                in_manifest = path in admitted_by_path
                if not in_manifest and capture_version == 1:
                    raise CaptureError(f"result outside admitted universe: {path}")
                if (
                    not in_manifest
                    and PurePosixPath(path).suffix not in request["file_filter_extensions"]
                ):
                    raise CaptureError(f"result outside the declared extension filter: {path}")
                line_matches = hit.get("lineMatches")
                if (
                    not isinstance(line_matches, list)
                    or not line_matches
                    or hit.get("chunkMatches") not in (None, [])
                ):
                    raise CaptureError("content result lacks comparable line matches")
                for line in line_matches:
                    if (
                        not isinstance(line, dict)
                        or not isinstance(line.get("line"), str)
                        or type(line.get("lineNumber")) is not int
                        or line["lineNumber"] < 0
                        or not isinstance(line.get("offsetAndLengths"), list)
                        or not line["offsetAndLengths"]
                        or any(
                            not isinstance(span, list)
                            or len(span) != 2
                            or type(span[0]) is not int
                            or span[0] < 0
                            or type(span[1]) is not int
                            or span[1] <= 0
                            for span in line["offsetAndLengths"]
                        )
                    ):
                        raise CaptureError("content result has malformed line matches")
                    # Sourcegraph LineMatch offsets and lengths count characters,
                    # not UTF-8 bytes. Byte length would admit spans past a
                    # non-ASCII line's end.
                    line_characters = len(line["line"])
                    if any(
                        offset + length > line_characters
                        for offset, length in line["offsetAndLengths"]
                    ):
                        raise CaptureError("content result has out-of-range line match")
                fingerprint_source = json.dumps(
                    hit,
                    sort_keys=True,
                    separators=(",", ":"),
                    ensure_ascii=False,
                    allow_nan=False,
                )
                if fingerprint_source in seen_matches:
                    raise CaptureError("duplicate native match row")
                seen_matches.add(fingerprint_source)
                fingerprint = sha256(fingerprint_source.encode()) if evidence_hashes else None
                native_match_count += 1
                if not in_manifest:
                    out_of_manifest_match_count += 1
                    continue
                matches.append(
                    {
                        "rank": len(matches) + 1,
                        "native_rank": native_match_count,
                        "path": path,
                        "file_sha256": admitted_by_path[path],
                        "raw_match_sha256": fingerprint,
                    }
                )
        elif kind == "filters":
            if final_progress or not isinstance(data, list):
                raise CaptureError("filters after final progress or malformed filters")
    if not final_progress:
        raise CaptureError("stream lacks final done=true progress")
    if reported_match_count < native_match_count:
        raise CaptureError("final match count is smaller than returned content rows")
    files: list[dict[str, Any]] = []
    seen_paths: set[str] = set()
    for hit in matches:
        if hit["path"] not in seen_paths:
            seen_paths.add(hit["path"])
            files.append(
                {
                    "rank": len(files) + 1,
                    "path": hit["path"],
                    "file_sha256": hit["file_sha256"],
                    "first_match_rank": hit["rank"],
                    "first_native_match_rank": hit["native_rank"],
                }
            )
    return events, matches, files, native_match_count, out_of_manifest_match_count


def validate_capture(
    request: dict[str, Any], raw: bytes, manifest: dict[str, Any], universe: dict[str, Any]
) -> dict[str, Any]:
    """Bind an offline stream to explicit inputs; emit diagnostic evidence only."""
    request_keys = {
        "capture_version",
        "api_version",
        "endpoint",
        "query",
        "query_sha256",
        "request_query",
        "repository",
        "revision",
        "response_sha256",
        "http_status",
        "content_type",
        "server_image_digest",
    }
    if not isinstance(request, dict):
        raise CaptureError("request must be an object")
    capture_version = request.get("capture_version")
    if capture_version == 1:
        request = _keys(request, request_keys, "request")
    elif capture_version == 2:
        request = _keys(request, request_keys | {"file_filter_extensions"}, "request")
    elif capture_version == 3:
        request = _keys(
            request, request_keys | {"file_filter_extensions", "source_revision"}, "request"
        )
    else:
        raise CaptureError("unsupported pinned V3 stream capture envelope")
    if type(capture_version) is not int or request["api_version"] != "V3":
        raise CaptureError("only the pinned V3 stream capture envelope is supported")
    if request["endpoint"] != "/.api/search/stream":
        raise CaptureError("unexpected Sourcegraph endpoint")
    if type(request["http_status"]) is not int or request["http_status"] != 200:
        raise CaptureError("Sourcegraph response did not have HTTP 200")
    if request["content_type"] != "text/event-stream":
        raise CaptureError("Sourcegraph response was not an event stream")
    _hex(request["server_image_digest"], HEX64, "server_image_digest")
    if request["query_sha256"] != sha256(request["query"].encode("utf-8")):
        raise CaptureError("raw query digest mismatch")
    if request["response_sha256"] != sha256(raw):
        raise CaptureError("raw stream digest mismatch")

    manifest = _keys(manifest, {"repository_commit", "files"}, "manifest")
    _hex(manifest["repository_commit"], HEX40, "manifest.repository_commit")
    source_revision = request["source_revision"] if capture_version == 3 else request["revision"]
    _hex(source_revision, HEX40, "source_revision")
    _hex(request["revision"], HEX40, "revision")
    if manifest["repository_commit"] != source_revision:
        raise CaptureError("manifest revision differs from request")
    admitted = _files(manifest["files"], "manifest.files")
    universe = _keys(
        universe,
        {"proof_version", "method", "repository", "revision", "files"},
        "indexed-universe assertion",
    )
    if (
        type(universe["proof_version"]) is not int
        or universe["proof_version"] != 1
        or universe["method"]
        not in (
            "operator_asserted_indexed_universe",
            "input_manifest_only",
            "input_manifest_postfiltered",
        )
    ):
        raise CaptureError("universe binding has no supported method")
    if capture_version == 1 and universe["method"] == "input_manifest_postfiltered":
        raise CaptureError("v1 captures cannot use manifest postfilter scope")
    if (
        universe["repository"] != request["repository"]
        or universe["revision"] != request["revision"]
    ):
        raise CaptureError("indexed-universe identity differs from request")
    asserted = _files(universe["files"], "indexed-universe.files")
    if asserted != admitted:
        raise CaptureError("asserted indexed path/SHA universe differs from admitted manifest")
    admitted_by_path = {row["path"]: row["file_sha256"] for row in admitted}
    if capture_version == 1:
        expected_query = query_expression(
            request["query"],
            request["repository"],
            request["revision"],
            [row["path"] for row in admitted]
            if universe["method"] == "input_manifest_only"
            else None,
        )
    elif universe["method"] == "input_manifest_postfiltered":
        expected_extensions = file_extensions([row["path"] for row in admitted])
        if request["file_filter_extensions"] != expected_extensions:
            raise CaptureError("file extension filter differs from the admitted manifest")
        expected_query = query_expression(
            request["query"],
            request["repository"],
            request["revision"],
            file_extensions_filter=expected_extensions,
        )
    else:
        raise CaptureError("v2 capture requires manifest postfilter scope")
    if request["request_query"] != expected_query:
        raise CaptureError(
            "sent query differs from the pinned keyword expression and file universe"
        )

    events, matches, files, native_match_count, out_of_manifest_match_count = normalize_stream(
        request, raw, admitted_by_path, capture_version
    )
    binding_digest = sha256(
        json.dumps(universe, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    )
    result = {
        "adapter_version": capture_version,
        "status": "diagnostic_unqualified",
        "reason": (
            "indexed_universe_is_operator_asserted_and_stream_request_authenticity_is_not_proven"
            if universe["method"] == "operator_asserted_indexed_universe"
            else "indexed_universe_unattested_and_stream_request_authenticity_is_not_proven"
        ),
        "api_version": "V3",
        "request": request,
        "rank_semantics": (
            "observed_stream_order_postfiltered_to_input_manifest"
            if capture_version in (2, 3)
            else "observed_stream_order_only"
        ),
        "query_sha256": request["query_sha256"],
        "repository": request["repository"],
        "revision": request["revision"],
        **({"source_revision": source_revision} if capture_version == 3 else {}),
        "manifest_sha256": sha256(
            json.dumps(manifest, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
        ),
        "universe_binding_method": universe["method"],
        "universe_binding_sha256": binding_digest,
        "raw_stream_sha256": request["response_sha256"],
        "raw_stream_base64": base64.b64encode(raw).decode("ascii"),
        "raw_match_order": matches,
        "file_order": files,
        "native_match_count": native_match_count,
        "out_of_manifest_match_count": out_of_manifest_match_count,
        "event_count": len(events),
    }
    if universe["method"] == "operator_asserted_indexed_universe":
        result["indexed_universe_assertion_sha256"] = binding_digest
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--request", type=Path, required=True)
    parser.add_argument("--stream", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--indexed-universe", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    if args.out.exists():
        parser.error("output already exists; refusing overwrite")
    try:
        result = validate_capture(
            _json(args.request.read_bytes(), "request"),
            args.stream.read_bytes(),
            _json(args.manifest.read_bytes(), "manifest"),
            _json(args.indexed_universe.read_bytes(), "indexed universe"),
        )
    except (CaptureError, OSError) as exc:
        parser.error(str(exc))
    args.out.write_text(
        json.dumps(result, sort_keys=True, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
