"""Validate explicit file-RRF records without relabeling chunk retrieval.

The product owns ranking. This module checks the returned witnesses and their
source-bound record projection; the independent holdout scorer checks raw lane
pools separately. It does not infer candidates absent from the SDK response.
"""
from __future__ import annotations

import hashlib
import math
import struct
from pathlib import PurePosixPath

try:
    from tools.benchmark.retrieval import result_window
    from tools.benchmark.retrieval.finite_json import is_finite_json_number
except ModuleNotFoundError:  # Direct invocation of the sibling runner.
    import result_window
    from finite_json import is_finite_json_number

ROUTE = "hybrid_file"
ORDERING = "file_rrf_desc_lexical_presence_key_v1"
SCORE_EVIDENCE = "hybrid_file_rrf_v1"


class HybridFileEvidenceError(ValueError):
    pass


def first_file_witness_ranks_valid(
    positions: dict[int, int], returned: int, *, full_page: bool
) -> bool:
    """Check first occurrences visible in a bounded file projection."""
    if full_page and bool(positions) != bool(returned):
        return False
    previous_file = previous_chunk = 0
    for file_rank, chunk_rank in sorted(positions.items()):
        file_gap = file_rank - previous_file
        if (
            file_rank == 1 and chunk_rank != 1
            or chunk_rank - previous_chunk < file_gap
            or full_page and file_gap != 1
        ):
            return False
        previous_file, previous_chunk = file_rank, chunk_rank
    return True


def _need(condition: bool, message: str) -> None:
    if not condition:
        raise HybridFileEvidenceError("hybrid_file: " + message)


def _object(value: object, fields: set[str], where: str) -> dict:
    _need(isinstance(value, dict) and set(value) == fields, where + " fields differ")
    return value


def _integer(value: object, low: int, high: int, where: str) -> int:
    _need(type(value) is int and low <= value <= high, where + " is outside its bound")
    return value


def _number(value: object, where: str) -> float:
    _need(is_finite_json_number(value), where + " is not finite")
    return float(value)


def _score_f32_bytes(value: object, where: str) -> bytes:
    score = _number(value, where)
    try:
        packed = struct.pack("<f", score)
    except OverflowError as exc:
        raise HybridFileEvidenceError(f"hybrid_file: {where} exceeds finite f32") from exc
    _need(math.isfinite(struct.unpack("<f", packed)[0]), where + " exceeds finite f32")
    return packed


def _text(value: object, where: str) -> str:
    _need(isinstance(value, str) and bool(value.strip()) and "\0" not in value,
          where + " is invalid")
    return value


def _path(value: object) -> str:
    value = _text(value, "file path")
    parsed = PurePosixPath(value)
    _need(not parsed.is_absolute() and "\\" not in value and
          all(part not in ("", ".", "..") for part in value.split("/")),
          "file path is not canonical and relative")
    return value


def _source_revision(value: object, file: dict) -> dict | None:
    if value is None:
        return None
    source = _object(value, {"file", "revision_id", "source_sha256"}, "candidate source")
    _need(_object(source["file"], {"source_repo_id", "repo_relative_path"},
                  "candidate source file") == file, "candidate source file identity drift")
    _text(source["revision_id"], "candidate source revision")
    _pool_digest(source["source_sha256"], "candidate source hash")
    return source


def _preview_range(value: object, where: str) -> dict | None:
    if value is None:
        return None
    span = _object(value, {"start", "end"}, where)
    start = _integer(span["start"], 0, 2**64 - 1, where + " start")
    _integer(span["end"], start + 1, 2**64 - 1, where + " end")
    return span


def _preview(value: object, source: dict | None, file: dict,
             snippet: str, highlights: list[dict]) -> None:
    if value is None:
        return
    preview = _object(value, {"kind", "source", "chunk_start_byte", "original_focus",
                              "original_context", "normalized_focus",
                              "normalization_equivalent", "unavailable_reason"}, "preview")
    kind = preview["kind"]
    _need(kind in ("source_chunk", "source_file", "path", "synthetic_symbol_label"),
          "preview kind is invalid")
    bound_source = _source_revision(preview["source"], file)
    reason = preview["unavailable_reason"]
    _need(reason is None or reason in ("no_positive_witness", "source_not_provided",
                                       "work_budget", "focus_exceeds_budget",
                                       "unsupported_range"), "preview unavailable reason is invalid")
    _need(type(preview["normalization_equivalent"]) is bool,
          "preview normalization flag is invalid")
    base = preview["chunk_start_byte"]
    if base is not None:
        _integer(base, 0, 2**64 - 1, "preview chunk base")
    _need((base is None or kind == "source_chunk") and
          (base is None or bound_source is not None), "preview chunk base lacks source authority")
    focus = _preview_range(preview["original_focus"], "preview focus")
    context = _preview_range(preview["original_context"], "preview context")
    normalized = _preview_range(preview["normalized_focus"], "preview normalized focus")
    if reason is not None or kind not in ("source_chunk", "source_file"):
        _need(focus is None and context is None and normalized is None and
              not preview["normalization_equivalent"],
              "unavailable or non-source preview claims source spans")
        _need(reason is not None or bound_source is not None,
              "available preview lacks source identity")
    else:
        _need(bound_source is not None and focus is not None and context is not None and
              normalized is not None, "source preview lacks source or spans")
        _need(context["start"] <= focus["start"] and context["end"] >= focus["end"] and
              context["end"] - context["start"] == len(snippet.encode("utf-8")),
              "source preview context or emitted byte length differs")
        _need(base is None or base + context["end"] <= 2**64 - 1,
              "source preview byte offset overflows")
    _need(bound_source == source or (reason is not None and bound_source is None),
          "preview source differs from candidate source")
    if reason is not None:
        _need(not snippet and not highlights, "unavailable preview emits text or highlights")
    elif kind == "path":
        _need(snippet == file["repo_relative_path"], "path preview emits a different path")
    elif kind in ("source_chunk", "source_file"):
        start = focus["start"] - context["start"]
        length = focus["end"] - focus["start"]
        _need(highlights and highlights[0] == {"start": start, "len": length},
              "source preview focus differs from UTF-8 primary highlight")


def ordered_pool_sha256_v1(pin: dict, rows: list[dict]) -> list[int]:
    """Independent digest of an ordered bounded chunk prefix, before file rank."""
    pin = _object(pin, {"repo_id", "revision_id", "manifest_generation"}, "pool pin")
    repo = _text(pin["repo_id"], "pool repo")
    revision = _text(pin["revision_id"], "pool revision")
    generation = _integer(pin["manifest_generation"], 1, 2**64 - 1, "pool generation")
    _need(isinstance(rows, list) and len(rows) < 2**64, "pool rows are invalid")
    digest = hashlib.sha256()
    digest.update(b"quanta-index/hybrid-file/ordered-pool/v1\0")

    def string(value: str) -> None:
        encoded = value.encode("utf-8")
        digest.update(len(encoded).to_bytes(8, "little"))
        digest.update(encoded)

    string(repo)
    string(revision)
    digest.update(generation.to_bytes(8, "little"))
    digest.update(len(rows).to_bytes(8, "little"))
    for row in rows:
        row = _object(row, {"source_repo_id", "path", "candidate_id", "score"}, "pool row")
        string(_text(row["source_repo_id"], "pool source repo"))
        string(_path(row["path"]))
        string(_text(row["candidate_id"], "pool candidate id"))
        digest.update(_score_f32_bytes(row["score"], "pool score"))
    return list(digest.digest())


def _pool_digest(value: object, where: str) -> list[int]:
    _need(isinstance(value, list) and len(value) == 32 and
          all(type(byte) is int and 0 <= byte <= 255 for byte in value),
          where + " is not a SHA-256 byte array")
    return value


def _active_head(value: object, pin: dict) -> None:
    # The benchmark route always issues an active selector through the SDK.
    head = _object(value, {"generation", "activation_token"}, "selected active head")
    generation = _object(head["generation"], {"lexical", "semantic", "semantic_content"},
                         "active head generation")
    digests = []
    for lane, track in (("lexical", "Lexical"), ("semantic", "Semantic")):
        snapshot = _object(generation[lane], set(pin) | {"track", "manifest_digest"},
                           f"active head {lane} snapshot")
        _need(all(type(snapshot[field]) is type(expected) and snapshot[field] == expected
                  for field, expected in pin.items()) and snapshot["track"] == track,
              f"active head {lane} snapshot differs from response pin")
        digests.append(_text(snapshot["manifest_digest"], f"active head {lane} manifest digest"))
    _need(digests[0] == digests[1], "active head manifest digests differ")
    roots = _object(generation["semantic_content"], {"row_root_digest", "membership_root_digest"},
                    "active head semantic roots")
    for root in roots.values():
        _need(isinstance(root, str) and root.startswith("sha256:") and len(root) == 71
              and all(char in "0123456789abcdef" for char in root[7:]),
              "active head semantic root is not a canonical digest")
    token = _object(head["activation_token"], {"root_incarnation", "activation_sequence"},
                    "active head activation token")
    root = token["root_incarnation"]
    _need(isinstance(root, list) and len(root) == 16 and any(root)
          and all(type(byte) is int and 0 <= byte <= 255 for byte in root),
          "active head activation root is invalid")
    _integer(token["activation_sequence"], 1, 2**64 - 1, "active head activation sequence")


def validate_route_configuration(policy, routes, top_k, file_top_k) -> int | None:
    selected = isinstance(routes, (list, tuple, set, frozenset)) and ROUTE in routes
    if not selected:
        _need(file_top_k is None, "file_top_k requires the explicit hybrid_file route")
        return None
    _need(policy == "natural_language", "requires natural_language chunk query policy")
    lane_cap = _integer(top_k, 1, 10_000, "lane top_k")
    return _integer(file_top_k, 1, lane_cap, "explicit file_top_k")


def _candidate(value: object, pin: dict, file: dict) -> dict:
    fields = {"source_repo_id", "source", "preview", "candidate_id", "repo_id",
              "revision_id", "manifest_generation", "repo_relative_path", "start_line",
              "end_line", "score", "snippet", "snippet_hit_offset", "highlights"}
    c = _object(value, fields, "lane candidate")
    _need(c["source_repo_id"] == file["source_repo_id"] and
          c["repo_relative_path"] == file["repo_relative_path"] and
          c["repo_id"] == pin["repo_id"] and c["revision_id"] == pin["revision_id"] and
          c["manifest_generation"] == pin["manifest_generation"], "lane candidate pin/file drift")
    source = _source_revision(c["source"], file)
    _text(c["candidate_id"], "candidate id")
    _integer(c["manifest_generation"], 1, 2**64 - 1, "candidate generation")
    start = _integer(c["start_line"], 0, 2**32 - 1, "candidate start line")
    end = _integer(c["end_line"], start, 2**32 - 1, "candidate end line")
    _need(start > 0 or end == 0, "candidate line span is partially anchored")
    _score_f32_bytes(c["score"], "candidate score")
    _need(isinstance(c["snippet"], str), "candidate snippet is invalid")
    highlights = c["highlights"]
    _need(isinstance(highlights, list), "candidate highlights are invalid")
    encoded = c["snippet"].encode("utf-8")
    boundaries = {0}
    offset = 0
    for char in c["snippet"]:
        offset += len(char.encode("utf-8"))
        boundaries.add(offset)
    previous = None
    for h in highlights:
        h = _object(h, {"start", "len"}, "highlight")
        lo = _integer(h["start"], 0, min(len(encoded), 2**32 - 1), "highlight start")
        length = _integer(h["len"], 1, 2**32 - 1, "highlight length")
        _need(lo + length <= len(encoded) and lo in boundaries and lo + length in boundaries and
              (previous is None or previous < (lo, length)),
              "highlight is not sorted or UTF-8 aligned")
        previous = (lo, length)
    anchor = c["snippet_hit_offset"]
    if anchor is not None:
        _integer(anchor, 0, 2**32 - 1, "candidate highlight anchor")
    _need(anchor == (highlights[0]["start"] if highlights else None),
          "candidate highlight anchor differs")
    _preview(c["preview"], source, file, c["snippet"], highlights)
    return c


def validate_result(result: dict, capture: dict, lane_top_k: int,
                    *, declared_file_top_k: int | None = None) -> None:
    """Reject forged file evidence and false exhaustion; never score partial pools."""
    if result.get("route") != ROUTE:
        _need("hybrid_file_evidence" not in result and
              result.get("ordering") != ORDERING and
              result.get("score_evidence") != SCORE_EVIDENCE,
              "file fusion evidence is misplaced on another route")
        return
    _need(capture.get("system") == "quanta" and
          capture.get("execution_profile", {}).get("policy") == "natural_language",
          "requires the bound Quanta natural_language capture")
    _need(result.get("rank_unit") == "distinct_file" and result.get("ordering") == ORDERING and
          result.get("score_evidence") == SCORE_EVIDENCE, "result rank/score contract differs")
    rows = result.get("candidates")
    _need(isinstance(rows, list), "record candidates are invalid")
    evidence = result.get("hybrid_file_evidence")
    if evidence is None:
        _need("hybrid_file_evidence" not in result and not rows and
              result.get("status") in ("error", "timeout", "unavailable") and
              isinstance(result.get("error"), dict), "missing typed SDK response evidence")
        return
    evidence = _object(evidence, {"schema_version", "lane_top_k", "file_top_k", "response"},
                       "evidence")
    _need(type(evidence["schema_version"]) is int and evidence["schema_version"] == 1,
          "unsupported evidence version")
    lane_cap = _integer(lane_top_k, 1, 10_000, "declared lane cap")
    _need(type(evidence["lane_top_k"]) is int and evidence["lane_top_k"] == lane_cap,
          "lane cap differs from comparison contract")
    file_cap = _integer(evidence["file_top_k"], 1, lane_cap, "file cap")
    if declared_file_top_k is not None:
        _need(file_cap == _integer(declared_file_top_k, 1, lane_cap, "declared file cap"),
              "file cap differs from independently declared request cap")
    response = _object(evidence["response"], {"generation", "selected_active_head", "results",
                       "file_top_k", "bounded_file_count", "lexical_window", "dense_window",
                       "lexical_pool_sha256", "dense_pool_sha256",
                       "explanation"}, "response")
    _need(type(response["file_top_k"]) is int and response["file_top_k"] == file_cap,
          "response file cap echo differs")
    pin = _object(response["generation"], {"repo_id", "revision_id", "manifest_generation"}, "pin")
    _need(pin["repo_id"] == capture.get("source_repo_id") and
          pin["revision_id"] == capture.get("source_revision_id") and
          pin["manifest_generation"] == capture.get("generation"), "response/capture pin drift")
    _text(pin["repo_id"], "repo id")
    _text(pin["revision_id"], "revision id")
    _integer(pin["manifest_generation"], 1, 2**64 - 1, "response generation")
    _active_head(response["selected_active_head"], pin)
    pool_digests = [_pool_digest(response[lane + "_pool_sha256"], lane + " pool digest")
                    for lane in ("lexical", "dense")]
    windows = []
    for lane in ("lexical", "dense"):
        window = response[lane + "_window"]
        try:
            returned, exhausted, execution = result_window.typed_window(window, lane)
        except result_window.WindowEvidenceError as exc:
            raise HybridFileEvidenceError("hybrid_file: " + str(exc)) from exc
        _need(window.get("empty_provenance") != "logical_empty",
              "lane claims logical empty without constraints")
        _need(returned <= lane_cap, "lane exceeds chunk cap")
        tag = "hybrid_file." + lane
        _need(set(execution) == {tag} and (returned == 0 or execution[tag]),
              "window lacks the route's executed lane evidence")
        trace = window["coverage"]["lanes"][0]
        _need(trace["contributed"] == (returned > 0) and
              trace["candidates"].get("value", -1) >= returned,
              "lane contribution/count contradicts its returned pool")
        comparable = exhausted or (window["outcome"]["kind"] == "lower_bound" and
                                   window["outcome"]["continuation"] is True and returned == lane_cap)
        windows.append((returned, exhausted, comparable))
    hits = response["results"]
    _need(isinstance(hits, list) and len(hits) <= file_cap, "file result cap exceeded")
    bounded_file_count = _integer(response["bounded_file_count"], 0,
                                  sum(window[0] for window in windows), "bounded file count")
    _need((bounded_file_count == 0) == (all(window[0] == 0 for window in windows)) and
          len(hits) == min(file_cap, bounded_file_count),
          "bounded file count differs from returned prefix")
    both_exhausted = all(w[1] for w in windows)
    # A short nonempty file page can complete the bounded chunk union without
    # proving global file exhaustion. Its observed candidates remain capped.
    expected_status = (
        "error" if not all(w[2] for w in windows) else
        "abstained" if not hits and both_exhausted else
        "error" if not hits else
        "capped" if len(hits) == file_cap or not both_exhausted else
        "success"
    )
    _need(result.get("status") == expected_status, "status invents file exhaustion or complete pools")
    _need((not rows and isinstance(result.get("error"), dict)) if expected_status == "error"
          else len(rows) == len(hits) and result.get("error") is None,
          "status/candidate/error projection differs")
    seen_files = set()
    witness_positions: list[dict[int, int]] = [{}, {}]
    witness_candidates: list[dict[int, dict]] = [{}, {}]
    previous = None
    for index, hit in enumerate(hits):
        hit = _object(hit, {"file", "lexical", "dense", "file_rrf_score"}, "file hit")
        file = _object(hit["file"], {"source_repo_id", "repo_relative_path"}, "file key")
        path = _path(file["repo_relative_path"])
        _need(file["source_repo_id"] == pin["repo_id"], "foreign source repo in single-repo capture")
        key = (file["source_repo_id"], path)
        _need(key not in seen_files, "duplicate file key")
        seen_files.add(key)
        representative, score = None, 0.0
        source_hashes = []
        for lane_index, lane in enumerate(("lexical", "dense")):
            witness = hit[lane]
            if witness is None:
                continue
            witness = _object(witness, {"file_rank", "chunk_rank", "candidate"}, "lane witness")
            cr = _integer(witness["chunk_rank"], 1, windows[lane_index][0], "chunk rank")
            fr = _integer(witness["file_rank"], 1, cr, "file rank")
            _need(fr <= bounded_file_count, "file rank exceeds bounded file count")
            _need(fr not in witness_positions[lane_index], "duplicate lane rank")
            witness_positions[lane_index][fr] = cr
            candidate = _candidate(witness["candidate"], pin, file)
            witness_candidates[lane_index][cr] = candidate
            source = candidate["source"]
            if source is not None:
                source_hashes.append(source["source_sha256"])
            if representative is None:
                representative = candidate
            score += 1.0 / (60.0 + fr)
        _need(representative is not None, "file has no lane witness")
        observed_score = _number(hit["file_rrf_score"], "file RRF score")
        _need(score == observed_score, "RRF score differs from lane ranks")
        order = (-score, -(hit["lexical"] is not None), key[0].encode(), path.encode())
        _need(previous is None or previous <= order, "file RRF order is invalid")
        previous = order
        if expected_status != "error":
            row = rows[index]
            projection_error = "source-bound representative projection differs"
            _need(isinstance(row, dict) and row.get("path") == path and row.get("rank") == index + 1
                  and type(row.get("rank")) is int
                  and _number(row.get("score"), "record file score") == score,
                  projection_error)
            _need(all(bytes(source_hash).hex() == row.get("file_sha256")
                      for source_hash in source_hashes),
                  "source hash differs from proved file")
            sdk_span = (representative["start_line"], representative["end_line"])
            accounting = row.get("span_accounting")
            _need(isinstance(accounting, dict) and accounting.get("unit_kind") == "chunk"
                  and accounting.get("unit_id") == representative["candidate_id"], projection_error)
            recorded_span = (accounting.get("sdk_start_line"), accounting.get("sdk_end_line"))
            scored_span = (row.get("start_line"), row.get("end_line"))
            _need(all(type(line) is int for line in (*recorded_span, *scored_span))
                  and recorded_span == sdk_span
                  and 1 <= scored_span[0] <= scored_span[1] <= 2**32 - 1
                  and (sdk_span == (0, 0) or scored_span == sdk_span), projection_error)
    for index, lane in enumerate(("lexical", "dense")):
        returned = windows[index][0]
        _need(
            first_file_witness_ranks_valid(
                witness_positions[index], returned,
                full_page=len(hits) == bounded_file_count,
            ),
            f"{lane} witness ranks are invalid for the file page",
        )
        # One witness is emitted per file, so repeated chunks can make the
        # raw pool unreconstructible even when all fused files are returned.
        candidates = witness_candidates[index]
        if len(candidates) == returned:
            pool_rows = [
                {"source_repo_id": candidate["source_repo_id"],
                 "path": candidate["repo_relative_path"],
                 "candidate_id": candidate["candidate_id"], "score": candidate["score"]}
                for candidate in (candidates[rank] for rank in range(1, returned + 1))
            ]
            _need(pool_digests[index] == ordered_pool_sha256_v1(pin, pool_rows),
                  f"{lane} pool digest differs from witnessed chunk pool")
