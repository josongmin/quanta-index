"""Local fake services prove that live rows come from HTTP/process responses."""

import json
import selectors
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
    with pytest.raises(ValueError, match="output exceeds"):
        live._process(
            [
                sys.executable,
                "-c",
                "import sys; sys.stdout.write('x' * 9000000); sys.stdout.flush(); "
                "sys.stderr.write('y' * 9000000)",
            ],
            10,
        )
    with pytest.raises(ValueError, match="timed out"):
        live._process([sys.executable, "-c", "import time; time.sleep(5)"], 1)


@pytest.mark.parametrize("failure", [KeyboardInterrupt, OSError])
def test_cs_process_interrupt_reaps_child(monkeypatch, failure):
    original_selector = selectors.DefaultSelector
    original_popen = live.subprocess.Popen
    children = []

    class InterruptedSelector:
        def __enter__(self):
            self.inner = original_selector()
            return self

        def __exit__(self, *args):
            self.inner.close()

        def register(self, *args):
            return self.inner.register(*args)

        def get_map(self):
            return self.inner.get_map()

        def select(self, _timeout):
            raise failure("interrupted")

    def record_child(*args, **kwargs):
        child = original_popen(*args, **kwargs)
        children.append(child)
        return child

    monkeypatch.setattr(live.selectors, "DefaultSelector", InterruptedSelector)
    monkeypatch.setattr(live.subprocess, "Popen", record_child)
    with pytest.raises(failure, match="interrupted"):
        live._process([sys.executable, "-c", "import time; time.sleep(30)"], 10)
    assert len(children) == 1
    assert children[0].poll() == -live.signal.SIGKILL


def test_live_native_decoders_refuse_failure_and_partial_results(tmp_path):
    view = tmp_path / "view"
    (view / "src").mkdir(parents=True)
    source = view / "src" / "file.go"
    source.write_bytes(b"func symbol() {}\n")
    admitted = {"src/file.go": live._sha(source.read_bytes())}
    task = {"task_id": "S00", "query": "symbol"}
    gold = ["src/file.go"]
    valid_opengrok = {
        "time": 1,
        "resultCount": 1,
        "results": {"/fixture/src/file.go": [1]},
        "startDocument": 0,
        "endDocument": 0,
    }
    config = {"project": "fixture", "server_image_digest": "a" * 64}
    assert live._opengrok_response(
        config,
        task,
        gold,
        view,
        admitted,
        200,
        "application/json",
        json.dumps(valid_opengrok).encode(),
        1.0,
    )["file_paths_top_10"] == ["src/file.go"]
    assert (
        live._opengrok_response(
            config,
            task,
            gold,
            view,
            admitted,
            200,
            "application/json",
            b'{"time":1,"resultCount":0,"results":{},"startDocument":0,"endDocument":0}',
            1.0,
        )["file_paths_top_10"]
        == []
    )
    for status, body in (
        (500, valid_opengrok),
        (200, {**valid_opengrok, "endDocument": 1}),
        (200, {**valid_opengrok, "results": {"/other/src/file.go": [1]}}),
        (200, {"error": "backend failed"}),
    ):
        with pytest.raises(ValueError):
            live._opengrok_response(
                config,
                task,
                gold,
                view,
                admitted,
                status,
                "application/json",
                json.dumps(body).encode(),
                1.0,
            )

    assert live._cs_response(task, gold, view, admitted, 0, b"null", b"", 1.0)["paths"] == []
    valid_cs = json.dumps([{"location": str(source)}]).encode()
    assert live._cs_response(task, gold, view, admitted, 0, valid_cs, b"", 1.0)["paths"] == [
        "src/file.go"
    ]
    outside = tmp_path / "outside"
    outside.write_bytes(source.read_bytes())
    for code, stdout, stderr in (
        (1, valid_cs, b""),
        (0, valid_cs, b"failed"),
        (0, json.dumps([{"location": str(outside)}]).encode(), b""),
        (0, json.dumps([{"location": str(source)}] * 2).encode(), b""),
    ):
        with pytest.raises(ValueError):
            live._cs_response(task, gold, view, admitted, code, stdout, stderr, 1.0)


@pytest.mark.parametrize("status,body", [(404, b"missing"), (200, b"stale source")])
def test_opengrok_full_view_probe_refuses_missing_or_stale_indexed_document(
    tmp_path, monkeypatch, status, body
):
    view = tmp_path / "view"
    view.mkdir()
    (view / "file.go").write_bytes(b"current source")
    manifest = {"files": [{"path": "file.go", "file_sha256": live._sha(b"current source")}]}
    target = tmp_path / "probe"

    def fake_http(_config, endpoint, _params, _accept):
        if endpoint.endswith("/files"):
            return 200, "application/json", b'["/fixture/file.go"]', 1.0
        return status, "text/plain", body, 1.0

    monkeypatch.setattr(live, "_http", fake_http)
    with pytest.raises(ValueError, match="indexed document"):
        live._opengrok_indexed_view({"project": "fixture"}, manifest, view, target)
    assert (target / "000000.content").read_bytes() == body
    assert json.loads((target / "000000.transport.json").read_bytes())["path"] == "/fixture/file.go"


@pytest.mark.parametrize(
    "native,error",
    [
        (["/fixture/a.go"], "differs from release"),
        (["/fixture/a.go", "/fixture/b.go", "/fixture/extra.go"], "differs from release"),
        (["/fixture/a.go", "/fixture/a.go", "/fixture/b.go"], "duplicate paths"),
        (["/fixture/a.go", "/fixture/../b.go"], "noncanonical path"),
        (["/other/a.go", "/fixture/b.go"], "invalid project path"),
        (["/fixture/a.go", 3], "invalid project path"),
    ],
)
def test_opengrok_indexed_inventory_rejects_nonmatching_native_paths(native, error):
    manifest = {"files": [{"path": "a.go"}, {"path": "b.go"}]}
    with pytest.raises(ValueError, match=error):
        live._opengrok_indexed_inventory_response(
            {"project": "fixture"}, manifest, 200, "application/json", json.dumps(native).encode()
        )


def test_opengrok_indexed_inventory_accepts_exact_unsorted_native_paths():
    manifest = {"files": [{"path": "a.go"}, {"path": "b.go"}]}
    live._opengrok_indexed_inventory_response(
        {"project": "fixture"},
        manifest,
        200,
        "application/json",
        b'["/fixture/b.go","/fixture/a.go"]',
    )


def test_opengrok_indexed_inventory_requires_successful_json_response():
    manifest = {"files": [{"path": "a.go"}]}
    for status, content_type in ((401, "application/json"), (200, "text/plain")):
        with pytest.raises(ValueError, match="indexed file inventory is unavailable"):
            live._opengrok_indexed_inventory_response(
                {"project": "fixture"},
                manifest,
                status,
                content_type,
                b'["/fixture/a.go"]',
            )


def test_opengrok_full_view_probe_rejects_inventory_change_during_capture(tmp_path, monkeypatch):
    view = tmp_path / "view"
    view.mkdir()
    (view / "file.go").write_bytes(b"current source")
    manifest = {"files": [{"path": "file.go", "file_sha256": live._sha(b"current source")}]}
    inventory_calls = 0

    def fake_http(_config, endpoint, _params, _accept):
        nonlocal inventory_calls
        if endpoint.endswith("/files"):
            inventory_calls += 1
            paths = ["/fixture/file.go"]
            if inventory_calls == 2:
                paths.append("/fixture/extra.go")
            return 200, "application/json", json.dumps(paths).encode(), 1.0
        return 200, "text/plain", b"current source", 1.0

    monkeypatch.setattr(live, "_http", fake_http)
    target = tmp_path / "probe"
    with pytest.raises(ValueError, match="indexed file inventory differs from release manifest"):
        live._opengrok_indexed_view({"project": "fixture"}, manifest, view, target)
    assert inventory_calls == 2
    assert (target / "000000.content").read_bytes() == b"current source"
    assert json.loads((target / "indexed-files-after.json").read_bytes()) == [
        "/fixture/file.go",
        "/fixture/extra.go",
    ]


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
    view = None
    query_seen = False
    inventory_extra_after_query = False

    def do_GET(self):
        parsed = urlsplit(self.path)
        self.calls.append(parsed.path)
        query = parse_qs(parsed.query)
        hit = "symbol_0" in str(query)
        if parsed.path == "/.api/search/stream":
            match = {
                "type": "content",
                "path": "src/0.go",
                "repository": "benchmark/fixture",
                "commit": self.commit,
                "lineMatches": [
                    {"line": "func symbol_0() {}", "lineNumber": 0, "offsetAndLengths": [[5, 8]]}
                ],
            }
            body = (
                (b"event: matches\ndata: " + json.dumps([match]).encode() + b"\n\n") if hit else b""
            )
            body += (
                b"event: progress\ndata: "
                + json.dumps(
                    {
                        "done": True,
                        "skipped": [],
                        "matchCount": int(hit),
                        "durationMs": 1,
                    }
                ).encode()
                + b"\n\nevent: done\ndata: {}\n\n"
            )
            content_type = "text/event-stream"
        elif parsed.path == "/api/v1/search":
            type(self).query_seen = True
            body = json.dumps(
                {
                    "time": 1,
                    "resultCount": int(hit),
                    "results": {"/fixture/src/0.go": [1]} if hit else {},
                    "startDocument": 0,
                    "endDocument": 0,
                }
            ).encode()
            content_type = "application/json"
        elif parsed.path == "/api/v1/file/content":
            requested = query["path"][0]
            assert requested.startswith("/fixture/")
            body = (self.view / requested.removeprefix("/fixture/")).read_bytes()
            content_type = "text/plain"
        elif parsed.path == "/api/v1/projects/fixture/files":
            paths = [
                "/fixture/" + path.relative_to(self.view).as_posix()
                for path in self.view.rglob("*")
                if path.is_file()
            ]
            if self.inventory_extra_after_query and self.query_seen:
                paths.append("/fixture/extra.go")
            body = json.dumps(paths).encode()
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


@pytest.mark.parametrize("index_changes_during_queries", [False, True])
def test_live_capture_makes_three_product_requests_and_retains_raw(
    tmp_path, lexical_release_seed, index_changes_during_queries
):
    lexical_spec, paths = inputs(tmp_path, lexical_release_seed)
    corpus = json.loads(lexical_spec.read_text())["corpus"]
    SearchHandler.commit = json.loads(paths["suite"].read_text())["repository_commit"]
    SearchHandler.view = (
        Path(corpus["release_path"]) / "views" / corpus["repository"] / corpus["view"]
    )
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
    SearchHandler.query_seen = False
    SearchHandler.inventory_extra_after_query = index_changes_during_queries
    server = ThreadingHTTPServer(("127.0.0.1", 0), SearchHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        base = f"http://127.0.0.1:{server.server_address[1]}"
        spec = {
            "schema_version": 1,
            "corpus": corpus,
            "suite": str(paths["suite"]),
            "query_pack": str(paths["query_pack"]),
            "sourcegraph": {
                "base_url": base,
                "repository": "benchmark/fixture",
                "server_image_digest": "a" * 64,
            },
            "opengrok": {
                "base_url": base,
                "project": "fixture",
                "server_image_digest": "b" * 64,
                "indexed_view_probe": "full",
            },
            "cs": {"binary": str(binary)},
            "output_root": str(tmp_path / "live"),
        }
        spec_path = tmp_path / "live-spec.json"
        spec_path.write_text(json.dumps(spec))
        if index_changes_during_queries:
            with pytest.raises(
                ValueError, match="indexed file inventory differs from release manifest"
            ):
                live.capture(spec_path)
        else:
            result = live.capture(spec_path)
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
    if index_changes_during_queries:
        assert not Path(spec["output_root"]).exists()
        assert SearchHandler.calls.count("/api/v1/search") == 20
        stage = Path(spec["output_root"] + ".staging")
        assert (
            json.loads((stage / "opengrok-view-post/indexed-files-before.json").read_bytes())[-1]
            == "/fixture/extra.go"
        )
        return
    assert result["indexed_universe_attested"] is False
    assert result["opengrok_indexed_universe_attested"] is True
    file_count = len(json.loads(paths["suite"].read_text())["file_universe"])
    assert (
        result["opengrok_indexed_view_probe"]
        == "exact_indexed_inventory_and_served_bytes_bracketing_queries"
    )
    assert result["opengrok_indexed_view_files"] == file_count
    assert len(SearchHandler.calls) == 44 + 2 * file_count
    assert SearchHandler.calls.count("/.api/search/stream") == 20
    assert SearchHandler.calls.count("/api/v1/search") == 20
    assert SearchHandler.calls.count("/api/v1/file/content") == 2 * file_count
    assert SearchHandler.calls.count("/api/v1/projects/fixture/files") == 4
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
    expected = live.lexical._tasks(
        json.loads(paths["suite"].read_bytes()), json.loads(paths["query_pack"].read_bytes())
    )
    universe = live.lexical._file_universe(
        json.loads(paths["suite"].read_bytes()), json.loads(paths["query_pack"].read_bytes())
    )
    for product in ("cs", "sourcegraph", "opengrok"):
        row_path = root / f"{product}_rows.jsonl"
        original = row_path.read_bytes()
        native_before = {name: (root / name).read_bytes() for name in result["raw_capture_sha256"]}
        rows = [json.loads(line) for line in original.splitlines()]
        assert rows[1]["file_hit_at_10"] is False
        rows[1]["paths" if product == "cs" else "file_paths_top_10"] = rows[1]["gold_paths"]
        rows[1]["file_hit_at_10"] = True
        row_path.write_bytes(b"".join(json.dumps(row).encode() + b"\n" for row in rows))
        # The standalone diagnostic validates only normalized consistency.
        assert live.lexical.product_result(product, row_path, expected, universe)["hits"] == 2
        summary_path.write_text(
            json.dumps(
                {
                    **result,
                    "rows_sha256": {
                        **result["rows_sha256"],
                        product: live._sha(row_path.read_bytes()),
                    },
                }
            )
        )
        with pytest.raises(
            ValueError, match="external row disagrees with retained native response"
        ):
            live.verify(root)
        assert all((root / name).read_bytes() == raw for name, raw in native_before.items())
        row_path.write_bytes(original)
        summary_path.write_text(json.dumps(result))
        assert live.verify(root) == result
    mutations = [
        {"schema_version": True},
        {"tasks": True},
        {"quality_qualified": True},
        {"exclusions": []},
        {"python_executable_sha256": "0" * 64},
        {"python_version": "wrong-runtime"},
        {"cs_binary_sha256": "0" * 64},
        {"cs_version": "wrong-version"},
        {"server_image_digests_operator_supplied": {}},
        {"rows_sha256": {**result["rows_sha256"], "forged-product": "0" * 64}},
    ]
    for mutation in mutations:
        summary_path.write_text(json.dumps({**result, **mutation}))
        with pytest.raises(ValueError, match="unsupported capture metadata"):
            live.verify(root)
    summary_path.write_text(json.dumps(result))
    assert live.verify(root) == result
    for mutation in ({"schema_version": True}, {"index_universe_attested": 0}):
        summary_path.write_text(
            json.dumps({**result, "binding": {**result["binding"], **mutation}})
        )
        with pytest.raises(ValueError, match="capture binding differs"):
            live.verify(root)
    summary_path.write_text(json.dumps(result))
    row_path = root / "sourcegraph_rows.jsonl"
    original_rows = row_path.read_bytes()
    forged_rows = [json.loads(line) for line in original_rows.splitlines()]
    forged_rows[0]["http_status"] = 200.0
    row_path.write_text("".join(json.dumps(row) + "\n" for row in forged_rows))
    summary_path.write_text(
        json.dumps(
            {
                **result,
                "rows_sha256": {
                    **result["rows_sha256"],
                    "sourcegraph": live._sha(row_path.read_bytes()),
                },
            }
        )
    )
    with pytest.raises(ValueError, match="failed request|external row disagrees"):
        live.verify(root)
    row_path.write_bytes(original_rows)
    summary_path.write_text(json.dumps(result))
    probe_path = root / "opengrok-view-post/000000.content"
    original_probe = probe_path.read_bytes()
    probe_path.write_bytes(original_probe + b"changed")
    summary_path.write_text(
        json.dumps(
            {
                **result,
                "raw_capture_sha256": {
                    **result["raw_capture_sha256"],
                    "opengrok-view-post/000000.content": live._sha(probe_path.read_bytes()),
                },
            }
        )
    )
    with pytest.raises(ValueError, match="source bytes differ from release"):
        live.verify(root)
    probe_path.write_bytes(original_probe)
    summary_path.write_text(json.dumps(result))
    inventory_path = root / "opengrok-view-post/indexed-files-before.json"
    original_inventory = inventory_path.read_bytes()
    inventory_path.write_text(json.dumps(["/fixture/extra.go"]))
    summary_path.write_text(
        json.dumps(
            {
                **result,
                "raw_capture_sha256": {
                    **result["raw_capture_sha256"],
                    "opengrok-view-post/indexed-files-before.json": live._sha(
                        inventory_path.read_bytes()
                    ),
                },
            }
        )
    )
    with pytest.raises(ValueError, match="indexed file inventory differs from release manifest"):
        live.verify(root)
    inventory_path.write_bytes(original_inventory)
    summary_path.write_text(json.dumps(result))
    raw_path = root / "sourcegraph/S00.stream"
    raw_path.write_bytes(raw_path.read_bytes().replace(b"src/0.go", b"src/1.go"))
    with pytest.raises(ValueError, match="native bytes differ"):
        live.verify(root)
