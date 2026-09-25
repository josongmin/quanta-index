"""Adversarial offline Stream API adapter tests; no Sourcegraph service needed."""

from __future__ import annotations

import base64
import copy
import json
import unittest

from tools.benchmark.retrieval.sourcegraph import (
    CaptureError,
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

    def assert_refused(self, request: dict, raw: bytes, manifest: dict, universe: dict) -> None:
        with self.assertRaises(CaptureError):
            validate_capture(request, raw, manifest, universe)

    def test_malformed_event_and_duplicate_json_key_refused(self) -> None:
        for raw in (
            b"event: progress\ndata: {bad}\n\n" + event("done", {}),
            b'event: progress\ndata: {"done":false,"done":true,"skipped":[]}\n\n'
            + event("done", {}),
            b'event: progress\ndata: {"done":false,"skipped":[],"durationMs":NaN}\n\n'
            + event("done", {}),
            b"event: progress\ndata: {}\nextra: ignored\n\n" + event("done", {}),
        ):
            with self.subTest(raw=raw):
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
        for progress in (
            {"done": True, "skipped": [], "durationMs": 1},
            {"done": True, "skipped": [], "matchCount": True, "durationMs": 1},
            {"done": True, "skipped": [], "matchCount": 0, "durationMs": -1},
        ):
            with self.subTest(progress=progress):
                self.assert_refused(*inputs(event("progress", progress) + event("done", {})))
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
        for malformed in (
            {**hit("a.py", 1), "lineMatches": [None]},
            {
                **hit("a.py", 1),
                "lineMatches": [{"line": QUERY, "lineNumber": True, "offsetAndLengths": [[0, 1]]}],
            },
            {
                **hit("a.py", 1),
                "lineMatches": [{"line": QUERY, "lineNumber": 1, "offsetAndLengths": [[0, 0]]}],
            },
            {**hit("a.py", 1), "chunkMatches": [{"content": QUERY}]},
        ):
            with self.subTest(malformed=malformed):
                raw = (
                    event("matches", [malformed])
                    + event(
                        "progress", {"done": True, "skipped": [], "matchCount": 1, "durationMs": 1}
                    )
                    + event("done", {})
                )
                self.assert_refused(*inputs(raw))

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
        for query in ("foo OR bar", "foo and bar", "foo NOT bar", "foo -bar", "foo /bar/"):
            with self.subTest(query=query), self.assertRaises(CaptureError):
                query_expression(query, REPOSITORY, REVISION)
        self.assertIn("type:file", query_expression("foo bar", REPOSITORY, REVISION))


if __name__ == "__main__":
    unittest.main()
