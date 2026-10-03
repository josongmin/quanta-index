"""Local fake services prove that live rows come from HTTP/process responses."""

import hashlib
import json
import selectors
import socket
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

import pytest

from tools.benchmark.evidence import CONTROL_DOCUMENT_BYTES
from tools.benchmark.retrieval import live_lexical_external as live
from tools.ci.tests.test_lexical_capture import inputs

pytest_plugins = ["tools.ci.tests.test_lexical_capture"]


def test_external_row_replay_streams_large_jsonl_and_refuses_invalid_order(tmp_path):
    path = tmp_path / "sourcegraph_rows.jsonl"
    tasks = [{"task_id": f"S{index:02d}"} for index in range(20)]
    padding = "x" * 900_000
    with path.open("wb") as stream:
        for task in tasks:
            stream.write(json.dumps({**task, "padding": padding}).encode() + b"\n")
    assert path.stat().st_size > CONTROL_DOCUMENT_BYTES
    seen = []
    live._replay_rows(path, tasks, lambda task, row: seen.append(row["task_id"]))
    assert seen == [task["task_id"] for task in tasks]

    path.write_bytes(b'{"task_id":"S00"}\n{"task_id":"S00"}\n')
    with pytest.raises(ValueError, match="row order differs"):
        live._replay_rows(path, tasks[:2], lambda *_: None)
    path.write_bytes(b'{"task_id":"S00"}\n')
    with pytest.raises(ValueError, match=r"zip\(\) argument 2 is shorter"):
        live._replay_rows(path, tasks[:2], lambda *_: None)
    path.write_bytes(b'{"task_id":"S00"}\n{"task_id":"S01"}\n')
    with pytest.raises(ValueError, match=r"zip\(\) argument 2 is longer"):
        live._replay_rows(path, tasks[:1], lambda *_: None)
    path.write_bytes(b'{"task_id":"S00"}\n{"task_id":')
    with pytest.raises(ValueError, match="unsafe benchmark evidence"):
        live._replay_rows(path, tasks[:2], lambda *_: None)


def test_result_file_larger_than_control_document_is_hashed_as_payload(tmp_path):
    view = tmp_path / "view"
    (view / "src").mkdir(parents=True)
    payload = view / "src" / "large.go"
    digest = hashlib.sha256()
    with payload.open("wb") as stream:
        for _ in range(17):
            chunk = b"x" * (1024 * 1024)
            stream.write(chunk)
            digest.update(chunk)
    admitted = {"src/large.go": digest.hexdigest()}
    assert live._paths(["src/large.go"], admitted, view) == ["src/large.go"]


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


@pytest.mark.parametrize("query", ["ab", "two words", "word!", "A" * 65, "éclair"])
def test_cs_fuzzy_query_refuses_unsupported_shapes(query):
    with pytest.raises(ValueError, match="bare ASCII identifier"):
        live._cs_fuzzy_query(query)


def test_cs_fuzzy_native_request_and_separate_replay(tmp_path, lexical_release_seed):
    lexical_spec, paths = inputs(tmp_path, lexical_release_seed)
    suite = json.loads(paths["suite"].read_bytes())
    pack = json.loads(paths["query_pack"].read_bytes())
    suite["routes"] = ["lexical"]
    pack["routes"] = ["lexical"]
    for task in suite["tasks"]:
        task["evaluation_contract"] = {
            "request_mode": "explicit_osa1_typo",
            "gold_unit": "distinct_file",
            "result_unit": "distinct_file",
        }
    pack["suite_commitment_sha256"] = live._sha(live.lexical.canonical(suite))
    paths["suite"].write_bytes(live.lexical.canonical(suite))
    paths["query_pack"].write_bytes(live.lexical.canonical(pack))
    binary = tmp_path / "cs-fuzzy"
    binary.write_text(
        f"#!{sys.executable}\nimport json, sys\nfrom pathlib import Path\n"
        "if '--version' in sys.argv:\n"
        "    print('cs version 3.2.0')\n"
        "else:\n"
        "    assert sys.argv[-1].endswith('~1')\n"
        "    root = Path(sys.argv[sys.argv.index('--dir') + 1])\n"
        "    hits = [{'location': str(root / 'src/0.go')}] if sys.argv[-1] == 'symbol_0~1' else []\n"
        "    print(json.dumps(hits) if hits else 'null')\n"
    )
    binary.chmod(0o755)
    original = json.loads(lexical_spec.read_text())
    root = tmp_path / "cs-fuzzy-capture"
    spec = {
        "schema_version": 1,
        "capability": live.CS_FUZZY_CAPABILITY,
        "corpus": original["corpus"],
        "suite": str(paths["suite"]),
        "query_pack": str(paths["query_pack"]),
        "cs": {"binary": str(binary)},
        "output_root": str(root),
    }
    spec_path = tmp_path / "cs-fuzzy-spec.json"
    spec_path.write_text(json.dumps(spec))
    summary = live.capture_cs_fuzzy(spec_path)
    assert summary["tasks"] == 20
    assert summary["scoring_status"] == "not_scored"
    assert live.verify_cs_fuzzy(root) == summary
    rows = [json.loads(line) for line in (root / "cs_fuzzy_rows.jsonl").read_bytes().splitlines()]
    assert rows[0]["native_query"] == "symbol_0~1"
    assert rows[0]["paths"] == ["src/0.go"]
    assert "file_hit_at_10" not in rows[0]
    process = root / "cs/S00.process.json"
    terminal = json.loads(process.read_text())
    terminal["argv"][-1] = "symbol_0"
    process.write_text(json.dumps(terminal))
    summary["raw_capture_sha256"]["cs/S00.process.json"] = live._sha_file(process)
    (root / "capture.json").write_text(json.dumps(summary))
    with pytest.raises(ValueError, match="native argv"):
        live.verify_cs_fuzzy(root)


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
        "results": {
            "/fixture/src/file.go": [
                {"line": "func symbol() {}", "lineNumber": "1", "tag": "function"}
            ]
        },
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
    for hit in (
        {"line": "func symbol() {}", "lineNumber": "1", "tag": None},
        {"line": "func symbol() {}", "lineNumber": "1"},
    ):
        body = {**valid_opengrok, "results": {"/fixture/src/file.go": [hit]}}
        assert (
            live._opengrok_response(
                config,
                task,
                gold,
                view,
                admitted,
                200,
                "application/json",
                json.dumps(body).encode(),
                1.0,
            )["file_hit_at_10"]
            is True
        )
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
        (
            200,
            {
                **valid_opengrok,
                "results": {
                    "/other/src/file.go": valid_opengrok["results"]["/fixture/src/file.go"]
                },
            },
        ),
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


@pytest.mark.parametrize(
    "hits",
    [
        [None],
        [1],
        [{}],
        [{"line": "func symbol() {}"}],
        [{"line": "func symbol() {}", "lineNumber": 1}],
        [{"line": "func symbol() {}", "lineNumber": "0"}],
        [{"line": "func symbol() {}", "lineNumber": "1", "tag": 1}],
    ],
)
def test_opengrok_response_refuses_malformed_search_hits(tmp_path, hits):
    view = tmp_path / "view"
    (view / "src").mkdir(parents=True)
    source = view / "src/file.go"
    source.write_bytes(b"func symbol() {}\n")
    body = {
        "time": 1,
        "resultCount": 1,
        "results": {"/fixture/src/file.go": hits},
        "startDocument": 0,
        "endDocument": 0,
    }
    with pytest.raises(ValueError, match="malformed SearchHit"):
        live._opengrok_response(
            {"project": "fixture", "server_image_digest": "a" * 64},
            {"task_id": "S00", "query": "symbol"},
            ["src/file.go"],
            view,
            {"src/file.go": live._sha(source.read_bytes())},
            200,
            "application/json",
            json.dumps(body).encode(),
            1.0,
        )


@pytest.mark.parametrize("status,body", [(404, b"missing"), (200, b"stale source")])
def test_opengrok_full_view_probe_refuses_missing_or_stale_indexed_source(
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
        return status, "application/octet-stream", body, 1.0

    monkeypatch.setattr(live, "_http", fake_http)
    with pytest.raises(ValueError, match="indexed source"):
        live._opengrok_indexed_view({"project": "fixture"}, manifest, view, target)
    assert (target / "000000.content").read_bytes() == body
    assert json.loads((target / "000000.transport.json").read_bytes())["path"] == "/fixture/file.go"


def test_opengrok_full_view_probe_binds_index_uid_and_octet_source(tmp_path, monkeypatch):
    view = tmp_path / "view"
    view.mkdir()
    (view / "file.go").write_bytes(b"current source")
    manifest = {"files": [{"path": "file.go", "file_sha256": live._sha(b"current source")}]}
    calls = []

    def fake_http(_config, endpoint, _params, accept):
        calls.append((endpoint, accept))
        if endpoint.endswith("/files"):
            return 200, "application/json", b'["/fixture/file.go"]', 1.0
        assert accept == "application/octet-stream"
        return 200, "application/octet-stream", b"current source", 1.0

    monkeypatch.setattr(live, "_http", fake_http)
    target = tmp_path / "probe"
    live._opengrok_indexed_view({"project": "fixture"}, manifest, view, target)
    assert calls == [
        ("/api/v1/projects/fixture/files", "application/json"),
        ("/api/v1/file/content", "application/octet-stream"),
        ("/api/v1/projects/fixture/files", "application/json"),
    ]


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


def test_local_backend_snapshot_binds_container_image_mount_port_and_index(tmp_path, monkeypatch):
    root = tmp_path / "index"
    root.mkdir()
    (root / "shard.zoekt").write_bytes(b"original index")
    backend = {
        "root": str(root),
        "container_id": "a" * 64,
        "mount_destination": "/index",
        "container_port": "7080/tcp",
    }
    config = {
        "base_url": "http://127.0.0.1:8765",
        "server_image_digest": "b" * 64,
        "backend_snapshot": backend,
    }
    inspect = {
        "Id": backend["container_id"],
        "Image": "sha256:" + config["server_image_digest"],
        "State": {"Running": True, "Pid": 123, "StartedAt": "2026-10-02T00:00:00Z"},
        "RestartCount": 0,
        "Mounts": [{"Type": "bind", "Source": str(root), "Destination": "/index", "RW": False}],
        "NetworkSettings": {"Ports": {"7080/tcp": [{"HostIp": "127.0.0.1", "HostPort": "8765"}]}},
    }

    def fake_process(argv, timeout):
        assert argv[-1] == backend["container_id"] and timeout == 10
        return 0, json.dumps(inspect).encode(), b"", 1.0

    monkeypatch.setattr(live, "_process", fake_process)
    original = live._backend_snapshot(config)
    live._validate_backend_snapshot(config, original)
    assert original["files"] == [
        {"path": "shard.zoekt", "sha256": live._sha(b"original index"), "bytes": 14}
    ]
    baseline = json.loads(json.dumps(inspect))
    for mutate in (
        lambda value: value.update(Image="sha256:" + "c" * 64),
        lambda value: value["State"].update(Pid=0),
        lambda value: value["Mounts"][0].update(Source=str(tmp_path / "other")),
        lambda value: value["Mounts"][0].update(RW=True),
        lambda value: value["NetworkSettings"]["Ports"]["7080/tcp"][0].update(HostPort="8766"),
    ):
        changed = json.loads(json.dumps(baseline))
        mutate(changed)
        inspect.clear()
        inspect.update(changed)
        with pytest.raises(ValueError, match="backend container|backend index mount"):
            live._backend_snapshot(config)
        inspect.clear()
        inspect.update(json.loads(json.dumps(baseline)))
    (root / "shard.zoekt").write_bytes(b"changed index")
    assert live._backend_snapshot(config)["tree_sha256"] != original["tree_sha256"]
    (root / "linked").symlink_to(root / "shard.zoekt")
    with pytest.raises(ValueError, match="link or special file"):
        live._backend_snapshot(config)
    (root / "linked").unlink()
    (root / "shard.zoekt").unlink()
    root.rmdir()
    with pytest.raises(ValueError, match="existing canonical directory"):
        live._backend_snapshot(config)


def test_backend_tree_accepts_only_idle_zoekt_runtime_entries():
    temp_root = Path(tempfile.gettempdir()).resolve()
    with tempfile.TemporaryDirectory(prefix="qi-zoekt-", dir=temp_root) as directory:
        root = Path(directory)
        (root / "shard.zoekt").write_bytes(b"index")
        (root / ".indexserver.tmp").mkdir()
        (root / ".trash").mkdir()
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as server:
            server.bind(str(root / "indexserver.sock"))
            rows, _ = live._backend_tree(root)
            assert [row["path"] for row in rows] == ["shard.zoekt"]

            (root / ".indexserver.tmp" / "active.zoekt").write_bytes(b"partial")
            with pytest.raises(ValueError, match="transient index directory is nonempty"):
                live._backend_tree(root)
            (root / ".indexserver.tmp" / "active.zoekt").unlink()

            (root / "other.sock").symlink_to(root / "shard.zoekt")
            with pytest.raises(ValueError, match="link or special file"):
                live._backend_tree(root)


def test_backend_snapshot_refuses_process_restart_and_unbounded_index(tmp_path, monkeypatch):
    root = tmp_path / "index"
    root.mkdir()
    (root / "segment-a").write_bytes(b"abc")
    (root / "segment-b").write_bytes(b"def")
    backend = {"root": str(root)}
    config = {"backend_snapshot": backend}
    calls = 0

    def runtime(_config):
        nonlocal calls
        calls += 1
        return {"pid": calls}

    monkeypatch.setattr(live, "_backend_runtime", runtime)
    with pytest.raises(ValueError, match="changed during index hashing"):
        live._backend_snapshot(config)
    monkeypatch.setattr(live, "MAX_INDEX_FILES", 1)
    with pytest.raises(ValueError, match="exceeds 4096 files"):
        live._backend_tree(root)
    monkeypatch.setattr(live, "MAX_INDEX_FILES", 4096)
    monkeypatch.setattr(live, "MAX_INDEX_BYTES", 5)
    with pytest.raises(ValueError, match="exceeds 512 MiB"):
        live._backend_tree(root)


def test_backend_snapshot_spec_refuses_nonlocal_or_malformed_binding(tmp_path):
    valid = {
        "base_url": "http://127.0.0.1:8765",
        "repository": "benchmark/fixture",
        "server_image_digest": "a" * 64,
        "backend_snapshot": {
            "root": str(tmp_path),
            "container_id": "b" * 64,
            "mount_destination": "/index",
            "container_port": "7080/tcp",
        },
    }
    keys = {"base_url", "repository", "server_image_digest"}
    assert live._service(valid, keys, {"backend_snapshot"}) == valid
    for mutation in (
        {"base_url": "https://example.com"},
        {"backend_snapshot": {**valid["backend_snapshot"], "root": "relative"}},
        {"backend_snapshot": {**valid["backend_snapshot"], "container_id": "short"}},
        {"backend_snapshot": {**valid["backend_snapshot"], "container_port": "99999/tcp"}},
    ):
        with pytest.raises(ValueError, match="backend snapshot"):
            live._service({**valid, **mutation}, keys, {"backend_snapshot"})


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
        return 200, "application/octet-stream", b"current source", 1.0

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
    backend_mutation_path = None

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
            if self.backend_mutation_path is not None:
                self.backend_mutation_path.write_bytes(b"changed during search")
                type(self).backend_mutation_path = None
            body = json.dumps(
                {
                    "time": 1,
                    "resultCount": int(hit),
                    "results": {
                        "/fixture/src/0.go": [
                            {"line": "func symbol_0() {}", "lineNumber": "1", "tag": None}
                        ]
                    }
                    if hit
                    else {},
                    "startDocument": 0,
                    "endDocument": 0,
                }
            ).encode()
            content_type = "application/json"
        elif parsed.path == "/api/v1/file/content":
            requested = query["path"][0]
            assert requested.startswith("/fixture/")
            body = (self.view / requested.removeprefix("/fixture/")).read_bytes()
            content_type = "application/octet-stream"
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


@pytest.mark.parametrize(
    ("index_changes_during_queries", "unsupported_query", "backend_changes_during_queries"),
    [(False, False, False), (True, False, False), (False, True, False), (False, False, True)],
)
def test_live_capture_makes_three_product_requests_and_retains_raw(
    tmp_path,
    lexical_release_seed,
    index_changes_during_queries,
    unsupported_query,
    backend_changes_during_queries,
    monkeypatch,
):
    lexical_spec, paths = inputs(tmp_path, lexical_release_seed)
    if unsupported_query:
        suite = json.loads(paths["suite"].read_bytes())
        pack = json.loads(paths["query_pack"].read_bytes())
        for value in (suite, pack):
            value["tasks"][-1]["query"] = "not"
            value["tasks"][-1]["query_sha256"] = hashlib.sha256(b"not").hexdigest()
        pack["suite_commitment_sha256"] = live._sha(live.lexical.canonical(suite))
        paths["suite"].write_bytes(live.lexical.canonical(suite))
        paths["query_pack"].write_bytes(live.lexical.canonical(pack))
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
    SearchHandler.backend_mutation_path = None
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
        if not index_changes_during_queries and not unsupported_query:
            for name, container_port in (("sourcegraph", "7080/tcp"), ("opengrok", "8080/tcp")):
                backend_root = tmp_path / f"{name}-index"
                backend_root.mkdir()
                (backend_root / "segment.bin").write_bytes(name.encode())
                spec[name]["backend_snapshot"] = {
                    "root": str(backend_root),
                    "container_id": ("c" if name == "sourcegraph" else "d") * 64,
                    "mount_destination": "/index",
                    "container_port": container_port,
                }
            if backend_changes_during_queries:
                SearchHandler.backend_mutation_path = tmp_path / "sourcegraph-index/segment.bin"
            original_process = live._process

            def local_backend_process(argv, timeout):
                if argv[0] != "docker":
                    return original_process(argv, timeout)
                config = next(
                    value
                    for name in ("sourcegraph", "opengrok")
                    if (value := spec[name])["backend_snapshot"]["container_id"] == argv[-1]
                )
                backend = config["backend_snapshot"]
                inspect = {
                    "Id": backend["container_id"],
                    "Image": "sha256:" + config["server_image_digest"],
                    "State": {"Running": True, "Pid": 123, "StartedAt": "2026-10-02T00:00:00Z"},
                    "RestartCount": 0,
                    "Mounts": [
                        {
                            "Type": "bind",
                            "Source": backend["root"],
                            "Destination": "/index",
                            "RW": False,
                        }
                    ],
                    "NetworkSettings": {
                        "Ports": {
                            backend["container_port"]: [
                                {"HostIp": "127.0.0.1", "HostPort": str(server.server_address[1])}
                            ]
                        }
                    },
                }
                return 0, json.dumps(inspect).encode(), b"", 1.0

            monkeypatch.setattr(live, "_process", local_backend_process)
        spec_path = tmp_path / "live-spec.json"
        spec_path.write_text(json.dumps(spec))
        if index_changes_during_queries:
            with pytest.raises(
                ValueError, match="indexed file inventory differs from release manifest"
            ):
                live.capture(spec_path)
        elif backend_changes_during_queries:
            with pytest.raises(ValueError, match="backend process, mount or index changed"):
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
    if backend_changes_during_queries:
        assert not Path(spec["output_root"]).exists()
        stage = Path(spec["output_root"] + ".staging")
        before = json.loads((stage / "backend/sourcegraph-before.json").read_bytes())
        after = json.loads((stage / "backend/sourcegraph-after.json").read_bytes())
        assert before["tree_sha256"] != after["tree_sha256"]
        return
    assert result["indexed_universe_attested"] is False
    assert result["opengrok_indexed_universe_attested"] is False
    if not unsupported_query:
        assert set(result["backend_snapshot_sha256"]) == {"sourcegraph", "opengrok"}
    else:
        assert result["backend_snapshot_sha256"] == {}
    file_count = len(json.loads(paths["suite"].read_text())["file_universe"])
    assert (
        result["opengrok_indexed_view_probe"]
        == "exact_indexed_inventory_and_served_bytes_bracketing_queries"
    )
    assert result["opengrok_indexed_view_files"] == file_count
    assert len(SearchHandler.calls) == 44 + 2 * file_count - int(unsupported_query)
    assert SearchHandler.calls.count("/.api/search/stream") == 20 - int(unsupported_query)
    assert SearchHandler.calls.count("/api/v1/search") == 20
    assert SearchHandler.calls.count("/api/v1/file/content") == 2 * file_count
    assert SearchHandler.calls.count("/api/v1/projects/fixture/files") == 4
    root = Path(spec["output_root"])
    for name in ("sourcegraph", "opengrok", "cs"):
        rows = (root / f"{name}_rows.jsonl").read_text().splitlines()
        assert len(rows) == 20
        assert sum(json.loads(row).get("file_hit_at_10", False) for row in rows) == 1
    if unsupported_query:
        rows = [
            json.loads(row) for row in (root / "sourcegraph_rows.jsonl").read_text().splitlines()
        ]
        assert rows[-1]["status"] == "unsupported"
        assert rows[-1]["submitted_query"] == "not"
        scored = live.lexical.product_result(
            "sourcegraph",
            root / "sourcegraph_rows.jsonl",
            live.lexical._tasks(suite, pack),
            {row["path"] for row in suite["file_universe"]},
        )
        assert scored["capability_coverage"]["supported"] == 19
        assert scored["latency_ms"]["count"] == 19
    original_read = live._read_control_file

    def control_only(path):
        if path.name.endswith("_rows.jsonl"):
            raise AssertionError("external rows were read as a whole control document")
        return original_read(path)

    with monkeypatch.context() as patch:
        patch.setattr(live, "_read_control_file", control_only)
        assert live.verify(root) == result
    assert (root / "sourcegraph" / "S00.stream").exists()
    assert (root / "opengrok" / "S00.json").exists()
    assert (root / "cs" / "S00.json").exists()
    summary_path = root / "capture.json"
    if not unsupported_query:
        backend_path = root / "backend" / "sourcegraph-after.json"
        original_backend = backend_path.read_bytes()
        tampered = json.loads(original_backend)
        tampered["runtime"]["pid"] += 1
        backend_path.write_text(json.dumps(tampered))
        summary_path.write_text(
            json.dumps(
                {
                    **result,
                    "raw_capture_sha256": {
                        **result["raw_capture_sha256"],
                        "backend/sourcegraph-after.json": live._sha(backend_path.read_bytes()),
                    },
                }
            )
        )
        with pytest.raises(ValueError, match="backend snapshot changed during capture"):
            live.verify(root)
        backend_path.write_bytes(original_backend)
        summary_path.write_text(json.dumps(result))
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
        {"opengrok_indexed_universe_attested": True},
        {"indexed_universe_attested": True},
        {"rows_sha256": {**result["rows_sha256"], "forged-product": "0" * 64}},
    ]
    if not unsupported_query:
        mutations.append({"backend_snapshot_sha256": {}})
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
    with pytest.raises(ValueError, match="bytes differ from release"):
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
