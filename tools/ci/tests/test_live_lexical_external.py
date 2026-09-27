"""Local fake services prove that live rows come from HTTP/process responses."""

import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

import pytest

from tools.benchmark.retrieval import live_lexical_external as live
from tools.ci.tests.test_lexical_capture import inputs

pytest_plugins = ["tools.ci.tests.test_lexical_capture"]


def test_cs_process_refuses_excessive_output_and_timeout():
    with pytest.raises(ValueError, match="output exceeds"):
        live._process([sys.executable, "-c", "import sys; sys.stdout.write('x' * 17000000)"], 10)
    with pytest.raises(ValueError, match="timed out"):
        live._process([sys.executable, "-c", "import time; time.sleep(5)"], 1)


class SearchHandler(BaseHTTPRequestHandler):
    calls = []
    commit = ""

    def do_GET(self):
        parsed = urlsplit(self.path)
        self.calls.append(parsed.path)
        query = parse_qs(parsed.query)
        hit = "symbol_0" in str(query)
        if parsed.path == "/.api/search/stream":
            match = ({"type": "content", "path": "src/0.go", "repository": "benchmark/fixture",
                      "commit": self.commit, "lineMatches": [{"line": "func symbol_0() {}",
                      "lineNumber": 0, "offsetAndLengths": [[5, 8]]}]})
            body = (b"event: matches\ndata: " + json.dumps([match]).encode() + b"\n\n") if hit else b""
            body += (b"event: progress\ndata: " + json.dumps({
                "done": True, "skipped": [], "matchCount": int(hit), "durationMs": 1,
            }).encode() + b"\n\nevent: done\ndata: {}\n\n")
            content_type = "text/event-stream"
        elif parsed.path == "/api/v1/search":
            body = json.dumps({"time": 1, "resultCount": int(hit),
                               "results": {"/fixture/src/0.go": [1]} if hit else {},
                               "startDocument": 0, "endDocument": 0}).encode()
            content_type = "application/json"
        else:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def test_live_capture_makes_three_product_requests_and_retains_raw(tmp_path, lexical_release_seed):
    lexical_spec, paths = inputs(tmp_path, lexical_release_seed)
    corpus = json.loads(lexical_spec.read_text())["corpus"]
    SearchHandler.commit = json.loads(paths["suite"].read_text())["repository_commit"]
    binary = tmp_path / "cs"
    binary.write_text(
        f"#!{sys.executable}\nimport json, sys\nfrom pathlib import Path\n"
        "if '--version' in sys.argv:\n"
        "    print('cs-test')\n"
        "else:\n"
        "    root = Path(sys.argv[sys.argv.index('--dir') + 1])\n"
        "    hits = [{'location': str(root / 'src/0.go')}] if sys.argv[-1] == 'symbol_0' else []\n"
        "    print(json.dumps(hits) if hits else 'null')\n"
    )
    binary.chmod(0o755)
    SearchHandler.calls = []
    server = ThreadingHTTPServer(("127.0.0.1", 0), SearchHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        base = f"http://127.0.0.1:{server.server_address[1]}"
        spec = {
            "schema_version": 1, "corpus": corpus,
            "suite": str(paths["suite"]), "query_pack": str(paths["query_pack"]),
            "sourcegraph": {"base_url": base, "repository": "benchmark/fixture",
                            "server_image_digest": "a" * 64},
            "opengrok": {"base_url": base, "project": "fixture",
                         "server_image_digest": "b" * 64},
            "cs": {"binary": str(binary)}, "output_root": str(tmp_path / "live"),
        }
        spec_path = tmp_path / "live-spec.json"
        spec_path.write_text(json.dumps(spec))
        result = live.capture(spec_path)
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
    assert result["indexed_universe_attested"] is False
    assert len(SearchHandler.calls) == 40
    assert SearchHandler.calls.count("/.api/search/stream") == 20
    assert SearchHandler.calls.count("/api/v1/search") == 20
    root = Path(spec["output_root"])
    for name in ("sourcegraph", "opengrok", "cs"):
        rows = (root / f"{name}_rows.jsonl").read_text().splitlines()
        assert len(rows) == 20
        assert sum(json.loads(row)["file_hit_at_10"] for row in rows) == 1
    assert live.verify(root) == result
    assert (root / "sourcegraph" / "S00.stream").exists()
    assert (root / "opengrok" / "S00.json").exists()
    assert (root / "cs" / "S00.json").exists()
    summary_path = root / "capture.json"
    # A recomputed normalized digest cannot authorize paths absent from the
    # fixed native response. Exercise all live decoder owners, not only status.
    expected = live.lexical._tasks(json.loads(paths["suite"].read_bytes()),
                                   json.loads(paths["query_pack"].read_bytes()))
    universe = live.lexical._file_universe(json.loads(paths["suite"].read_bytes()),
                                         json.loads(paths["query_pack"].read_bytes()))
    for product in ("cs", "sourcegraph", "opengrok"):
        row_path = root / f"{product}_rows.jsonl"
        original = row_path.read_bytes()
        native_before = {name: (root / name).read_bytes()
                         for name in result["raw_capture_sha256"]}
        rows = [json.loads(line) for line in original.splitlines()]
        assert rows[1]["file_hit_at_10"] is False
        rows[1]["paths" if product == "cs" else "file_paths_top_10"] = rows[1]["gold_paths"]
        rows[1]["file_hit_at_10"] = True
        row_path.write_bytes(b"".join(json.dumps(row).encode() + b"\n" for row in rows))
        # The standalone diagnostic validates only normalized consistency.
        assert live.lexical.product_result(product, row_path, expected, universe)["hits"] == 2
        summary_path.write_text(json.dumps({**result, "rows_sha256": {
            **result["rows_sha256"], product: live._sha(row_path.read_bytes())}}))
        with pytest.raises(ValueError, match="external row disagrees with retained native response"):
            live.verify(root)
        assert all((root / name).read_bytes() == raw for name, raw in native_before.items())
        row_path.write_bytes(original)
        summary_path.write_text(json.dumps(result))
        assert live.verify(root) == result
    mutations = [
        {"schema_version": True}, {"tasks": True}, {"quality_qualified": True},
        {"exclusions": []}, {"python_executable_sha256": "0" * 64},
        {"python_version": "wrong-runtime"}, {"cs_binary_sha256": "0" * 64},
        {"cs_version": "wrong-version"}, {"server_image_digests_operator_supplied": {}},
        {"rows_sha256": {**result["rows_sha256"], "forged-product": "0" * 64}},
    ]
    for mutation in mutations:
        summary_path.write_text(json.dumps({**result, **mutation}))
        with pytest.raises(ValueError, match="unsupported capture metadata"):
            live.verify(root)
    summary_path.write_text(json.dumps(result))
    assert live.verify(root) == result
    for mutation in ({"schema_version": True}, {"index_universe_attested": 0}):
        summary_path.write_text(json.dumps({**result, "binding": {**result["binding"], **mutation}}))
        with pytest.raises(ValueError, match="capture binding differs"):
            live.verify(root)
    summary_path.write_text(json.dumps(result))
    row_path = root / "sourcegraph_rows.jsonl"
    original_rows = row_path.read_bytes()
    forged_rows = [json.loads(line) for line in original_rows.splitlines()]
    forged_rows[0]["http_status"] = 200.0
    row_path.write_text("".join(json.dumps(row) + "\n" for row in forged_rows))
    summary_path.write_text(json.dumps({**result, "rows_sha256": {
        **result["rows_sha256"], "sourcegraph": live._sha(row_path.read_bytes())}}))
    with pytest.raises(ValueError, match="failed request|external row disagrees"):
        live.verify(root)
    row_path.write_bytes(original_rows)
    summary_path.write_text(json.dumps(result))
    raw_path = root / "sourcegraph/S00.stream"
    raw_path.write_bytes(raw_path.read_bytes().replace(b"src/0.go", b"src/1.go"))
    with pytest.raises(ValueError, match="native bytes differ"):
        live.verify(root)
