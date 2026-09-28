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


def test_sourcegraph_request_target_preflight_is_independent_of_corpus_size():
    manifest = {
        "repository_commit": "a" * 40,
        "files": [{"path": f"src/file-{index:05}.rs"} for index in range(10_000)],
    }
    config = {"repository": "benchmark/fixture"}
    tasks = [{"query": "symbolName"}]

    extensions, max_target_bytes = live._preflight_sourcegraph_request_targets(
        config, tasks, manifest
    )

    assert extensions == [".rs"]
    assert max_target_bytes < live.MAX_SOURCEGRAPH_REQUEST_TARGET_BYTES
    with pytest.raises(ValueError, match="8 KiB preflight limit"):
        live._preflight_sourcegraph_request_targets(
            config, [{"query": "x" * live.MAX_SOURCEGRAPH_REQUEST_TARGET_BYTES}], manifest
        )


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
    raw_path = root / "sourcegraph/S00.stream"
    raw_path.write_bytes(raw_path.read_bytes().replace(b"src/0.go", b"src/1.go"))
    with pytest.raises(ValueError, match="native bytes differ"):
        live.verify(root)
