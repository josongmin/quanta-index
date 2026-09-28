"""Adversarial offline Stream API adapter tests; no Sourcegraph service needed."""

from __future__ import annotations

import base64
import copy
import json
import unittest

from tools.benchmark.retrieval.sourcegraph import (
    CaptureError,
    file_extensions,
    query_expression,
    sha256,
    validate_capture,
)

REVISION = "a" * 40
REPOSITORY = "bench/example"
QUERY = "findSymbol"
FILES = [
    {"path": "a.py", "file_sha256": "1" * 64},
    {"path": "b.py", "file_sha256": "2" * 64},
]


def event(kind: str, data: object) -> bytes:
    return (
        f"event: {kind}\ndata: {json.dumps(data, sort_keys=True, separators=(',', ':'))}\n\n"
    ).encode()


def hit(path: str, line: int) -> dict:
    return {
        "type": "content",
        "repository": REPOSITORY,
        "commit": REVISION,
        "path": path,
        "lineMatches": [{"line": QUERY, "lineNumber": line, "offsetAndLengths": [[0, len(QUERY)]]}],
    }


def good_stream() -> bytes:
    return b"".join(
        [
            event("progress", {"done": False, "skipped": [], "matchCount": 0, "durationMs": 1}),
            event("matches", [hit("b.py", 2), hit("a.py", 3)]),
            event("matches", [hit("b.py", 8)]),
            event("progress", {"done": True, "skipped": [], "matchCount": 3, "durationMs": 2}),
            event("done", {}),
        ]
    )


def inputs(raw: bytes | None = None) -> tuple[dict, bytes, dict, dict]:
    raw = good_stream() if raw is None else raw
    request = {
        "capture_version": 1,
        "api_version": "V3",
        "endpoint": "/.api/search/stream",
        "query": QUERY,
        "query_sha256": sha256(QUERY.encode()),
        "request_query": query_expression(QUERY, REPOSITORY, REVISION),
        "repository": REPOSITORY,
        "revision": REVISION,
        "response_sha256": sha256(raw),
        "http_status": 200,
        "content_type": "text/event-stream",
        "server_image_digest": "e" * 64,
    }
    manifest = {"repository_commit": REVISION, "files": copy.deepcopy(FILES)}
    universe = {
        "proof_version": 1,
        "method": "operator_asserted_indexed_universe",
        "repository": REPOSITORY,
        "revision": REVISION,
        "files": copy.deepcopy(FILES),
    }
    return request, raw, manifest, universe


class SourcegraphCaptureTests(unittest.TestCase):
    def test_input_manifest_binding_does_not_claim_indexed_universe(self) -> None:
        request, raw, manifest, universe = inputs()
        universe["method"] = "input_manifest_only"
        request["request_query"] = query_expression(QUERY, REPOSITORY, REVISION,
                                                   [row["path"] for row in FILES])
        result = validate_capture(request, raw, manifest, universe)
        self.assertEqual(result["universe_binding_method"], "input_manifest_only")
        self.assertIn("indexed_universe_unattested", result["reason"])
        self.assertNotIn("indexed_universe_assertion_sha256", result)

    def test_preserves_raw_bytes_and_native_order_without_promotion(self) -> None:
        request, raw, manifest, universe = inputs()
        result = validate_capture(request, raw, manifest, universe)
        self.assertEqual(result["status"], "diagnostic_unqualified")
        self.assertEqual(result["rank_semantics"], "observed_stream_order_only")
        self.assertEqual(result["request"], request)
        self.assertEqual(base64.b64decode(result["raw_stream_base64"]), raw)
        self.assertEqual(
            [row["path"] for row in result["raw_match_order"]], ["b.py", "a.py", "b.py"]
        )
        self.assertEqual([row["path"] for row in result["file_order"]], ["b.py", "a.py"])
        self.assertEqual([row["first_match_rank"] for row in result["file_order"]], [1, 2])

    def test_extension_filter_postfilters_to_manifest_and_preserves_native_rank(self) -> None:
        extensions = file_extensions([row["path"] for row in FILES])
        raw = b"".join(
            [
                event("progress", {"done": False, "skipped": [], "matchCount": 0,
                                    "durationMs": 1}),
                event("matches", [
                    hit("outside.py", 1), hit("b.py", 2), hit("other.py", 3), hit("a.py", 4),
                ]),
                event("progress", {"done": True, "skipped": [], "matchCount": 4,
                                    "durationMs": 2}),
                event("done", {}),
            ]
        )
        request = {
            "capture_version": 2,
            "api_version": "V3",
            "endpoint": "/.api/search/stream",
            "query": QUERY,
            "query_sha256": sha256(QUERY.encode()),
            "file_filter_extensions": extensions,
            "request_query": query_expression(
                QUERY, REPOSITORY, REVISION, file_extensions_filter=extensions
            ),
            "repository": REPOSITORY,
            "revision": REVISION,
            "response_sha256": sha256(raw),
            "http_status": 200,
            "content_type": "text/event-stream",
            "server_image_digest": "e" * 64,
        }
        manifest = {"repository_commit": REVISION, "files": copy.deepcopy(FILES)}
        universe = {
            "proof_version": 1,
            "method": "input_manifest_postfiltered",
            "repository": REPOSITORY,
            "revision": REVISION,
            "files": copy.deepcopy(FILES),
        }

        result = validate_capture(request, raw, manifest, universe)

        self.assertEqual(result["adapter_version"], 2)
        self.assertEqual(
            result["rank_semantics"], "observed_stream_order_postfiltered_to_input_manifest"
        )
        self.assertEqual(result["native_match_count"], 4)
        self.assertEqual(result["out_of_manifest_match_count"], 2)
        self.assertEqual([row["path"] for row in result["raw_match_order"]], ["b.py", "a.py"])
        self.assertEqual([row["path"] for row in result["file_order"]], ["b.py", "a.py"])
        self.assertEqual([row["first_match_rank"] for row in result["file_order"]], [1, 2])
        self.assertEqual(
            [row["first_native_match_rank"] for row in result["file_order"]], [2, 4]
        )

    def test_extension_filter_refuses_results_outside_declared_suffixes(self) -> None:
        request, _, manifest, _ = inputs()
        extensions = file_extensions([row["path"] for row in FILES])
        raw = b"".join(
            [
                event("matches", [hit("outside.md", 1)]),
                event("progress", {"done": True, "skipped": [], "matchCount": 1,
                                    "durationMs": 1}),
                event("done", {}),
            ]
        )
        request.update(
            {
                "capture_version": 2,
                "file_filter_extensions": extensions,
                "request_query": query_expression(
                    QUERY, REPOSITORY, REVISION, file_extensions_filter=extensions
                ),
                "response_sha256": sha256(raw),
            }
        )
        universe = {
            "proof_version": 1,
            "method": "input_manifest_postfiltered",
            "repository": REPOSITORY,
            "revision": REVISION,
            "files": copy.deepcopy(FILES),
        }

        with self.assertRaisesRegex(CaptureError, "outside the declared extension filter"):
            validate_capture(request, raw, manifest, universe)

    def assert_refused(self, request: dict, raw: bytes, manifest: dict, universe: dict) -> None:
        with self.assertRaises(CaptureError):
            validate_capture(request, raw, manifest, universe)

    def test_malformed_event_refused(self) -> None:
        self.assert_refused(*inputs(b"event: progress\ndata: {bad}\n\n" + event("done", {})))

    def test_duplicate_json_key_refused(self) -> None:
        raw = b'event: progress\ndata: {"done":false,"done":true,"skipped":[]}\n\n'
        self.assert_refused(*inputs(raw + event("done", {})))

    def test_nonfinite_progress_refused(self) -> None:
        raw = b'event: progress\ndata: {"done":false,"skipped":[],"durationMs":NaN}\n\n'
        self.assert_refused(*inputs(raw + event("done", {})))

    def test_unknown_sse_field_refused(self) -> None:
        raw = b"event: progress\ndata: {}\nextra: ignored\n\n" + event("done", {})
        self.assert_refused(*inputs(raw))

    def test_duplicate_native_hit_refused(self) -> None:
        duplicate = event("matches", [hit("a.py", 1), hit("a.py", 1)])
        raw = (
            duplicate
            + event("progress", {"done": True, "skipped": [], "matchCount": 2, "durationMs": 2})
            + event("done", {})
        )
        self.assert_refused(*inputs(raw))

    def test_reordered_events_refused(self) -> None:
        raw = (
            event("progress", {"done": True, "skipped": [], "matchCount": 0, "durationMs": 1})
            + event("matches", [hit("a.py", 1)])
            + event("done", {})
        )
        self.assert_refused(*inputs(raw))

    def test_reordered_bytes_against_capture_digest_refused(self) -> None:
        request, raw, manifest, universe = inputs()
        altered = raw.replace(b'"path":"b.py"', b'"path":"a.py"', 1)
        self.assertNotEqual(raw, altered)
        self.assert_refused(request, altered, manifest, universe)

    def test_truncated_stream_or_skipped_results_refused(self) -> None:
        raw = good_stream()
        self.assert_refused(*inputs(raw[: -len(event("done", {}))]))
        skipped = event(
            "progress",
            {
                "done": True,
                "skipped": [{"reason": "shard-match-limit"}],
                "matchCount": 0,
                "durationMs": 1,
            },
        ) + event("done", {})
        self.assert_refused(*inputs(skipped))
        alert = (
            event("alert", {"title": "timeout"})
            + event("progress", {"done": True, "skipped": [], "matchCount": 0, "durationMs": 1})
            + event("done", {})
        )
        self.assert_refused(*inputs(alert))
        understated = (
            event("matches", [hit("a.py", 1)])
            + event("progress", {"done": True, "skipped": [], "matchCount": 0, "durationMs": 1})
            + event("done", {})
        )
        self.assert_refused(*inputs(understated))
        regressed = (
            event("progress", {"done": False, "skipped": [], "matchCount": 2, "durationMs": 1})
            + event("progress", {"done": True, "skipped": [], "matchCount": 1, "durationMs": 2})
            + event("done", {})
        )
        self.assert_refused(*inputs(regressed))

    def assert_progress_refused(self, progress: dict) -> None:
        self.assert_refused(*inputs(event("progress", progress) + event("done", {})))

    def test_progress_missing_match_count_refused(self) -> None:
        self.assert_progress_refused({"done": True, "skipped": [], "durationMs": 1})

    def test_progress_boolean_match_count_refused(self) -> None:
        self.assert_progress_refused(
            {"done": True, "skipped": [], "matchCount": True, "durationMs": 1}
        )

    def test_progress_negative_duration_refused(self) -> None:
        self.assert_progress_refused(
            {"done": True, "skipped": [], "matchCount": 0, "durationMs": -1}
        )

    def test_wrong_revision_refused_in_result_and_manifest(self) -> None:
        raw = good_stream().replace(REVISION.encode(), ("b" * 40).encode(), 1)
        self.assert_refused(*inputs(raw))
        request, raw, manifest, universe = inputs()
        manifest["repository_commit"] = "b" * 40
        self.assert_refused(request, raw, manifest, universe)

    def test_wrong_query_or_stream_hash_refused(self) -> None:
        request, raw, manifest, universe = inputs()
        request["query_sha256"] = "0" * 64
        self.assert_refused(request, raw, manifest, universe)
        request, raw, manifest, universe = inputs()
        request["request_query"] = request["request_query"].replace("count:all", "count:1")
        self.assert_refused(request, raw, manifest, universe)
        request, raw, manifest, universe = inputs()
        request["response_sha256"] = "0" * 64
        self.assert_refused(request, raw, manifest, universe)

    def test_missing_or_mismatched_indexed_universe_refused(self) -> None:
        request, raw, manifest, universe = inputs()
        universe["files"] = universe["files"][:-1]
        self.assert_refused(request, raw, manifest, universe)
        request, raw, manifest, universe = inputs()
        universe.pop("method")
        self.assert_refused(request, raw, manifest, universe)
        request, raw, manifest, universe = inputs()
        universe["proof_version"] = True
        self.assert_refused(request, raw, manifest, universe)

    def test_capture_version_boolean_and_empty_hit_evidence_refused(self) -> None:
        request, raw, manifest, universe = inputs()
        request["capture_version"] = True
        self.assert_refused(request, raw, manifest, universe)
        empty_hit = {**hit("a.py", 1), "lineMatches": []}
        raw = (
            event("matches", [empty_hit])
            + event("progress", {"done": True, "skipped": [], "matchCount": 1, "durationMs": 1})
            + event("done", {})
        )
        self.assert_refused(*inputs(raw))

    def assert_match_refused(self, malformed: dict) -> None:
        raw = (
            event("matches", [malformed])
            + event("progress", {"done": True, "skipped": [], "matchCount": 1, "durationMs": 1})
            + event("done", {})
        )
        self.assert_refused(*inputs(raw))

    def test_null_line_match_refused(self) -> None:
        self.assert_match_refused({**hit("a.py", 1), "lineMatches": [None]})

    def test_boolean_line_number_refused(self) -> None:
        self.assert_match_refused(
            {
                **hit("a.py", 1),
                "lineMatches": [{"line": QUERY, "lineNumber": True, "offsetAndLengths": [[0, 1]]}],
            }
        )

    def test_zero_width_match_refused(self) -> None:
        self.assert_match_refused(
            {
                **hit("a.py", 1),
                "lineMatches": [{"line": QUERY, "lineNumber": 1, "offsetAndLengths": [[0, 0]]}],
            }
        )

    def test_chunk_match_refused(self) -> None:
        self.assert_match_refused({**hit("a.py", 1), "chunkMatches": [{"content": QUERY}]})

    def test_non_content_or_outside_universe_refused(self) -> None:
        for changed in ({**hit("a.py", 1), "type": "path"}, hit("other.py", 1)):
            raw = (
                event("matches", [changed])
                + event("progress", {"done": True, "skipped": [], "matchCount": 1, "durationMs": 1})
                + event("done", {})
            )
            self.assert_refused(*inputs(raw))

    def test_query_filter_injection_refused(self) -> None:
        with self.assertRaises(CaptureError):
            query_expression("foo repo:other", REPOSITORY, REVISION)
        with self.assertRaises(CaptureError):
            query_expression("foo\nbar", REPOSITORY, REVISION)
        self.assertIn("type:file", query_expression("foo bar", REPOSITORY, REVISION))

    def test_extension_query_filter_is_short_and_closed(self) -> None:
        extensions = file_extensions([row["path"] for row in FILES])
        query = query_expression(
            QUERY, REPOSITORY, REVISION, file_extensions_filter=extensions
        )
        self.assertIn('file:"^(?:.*\\\\.(?:py))$"', query)
        with self.assertRaises(CaptureError):
            query_expression(
                QUERY, REPOSITORY, REVISION,
                file_paths=[row["path"] for row in FILES],
                file_extensions_filter=extensions,
            )
        with self.assertRaises(CaptureError):
            query_expression(QUERY, REPOSITORY, REVISION, file_extensions_filter=[".py", ".py"])

    def test_query_or_injection_refused(self) -> None:
        with self.assertRaises(CaptureError):
            query_expression("foo OR bar", REPOSITORY, REVISION)

    def test_query_and_injection_refused(self) -> None:
        with self.assertRaises(CaptureError):
            query_expression("foo and bar", REPOSITORY, REVISION)

    def test_query_not_injection_refused(self) -> None:
        with self.assertRaises(CaptureError):
            query_expression("foo NOT bar", REPOSITORY, REVISION)

    def test_query_negation_injection_refused(self) -> None:
        with self.assertRaises(CaptureError):
            query_expression("foo -bar", REPOSITORY, REVISION)

    def test_query_regex_injection_refused(self) -> None:
        with self.assertRaises(CaptureError):
            query_expression("foo /bar/", REPOSITORY, REVISION)


if __name__ == "__main__":
    unittest.main()
