"""Independent query-reader mutations and loopback transport fixtures; no product or Java execution."""

import hashlib
import threading
import urllib.request
import zipfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from tools.benchmark.retrieval import live_lexical_external as live
from tools.benchmark.retrieval import opengrok_query_witness as witness

COMMIT = {
    "segmentsFile": "segments_3p",
    "generation": 133,
    "readerVersion": 971,
    "numDocs": 17615,
    "maxDoc": 17620,
    "fileNamesSha256": "a" * 64,
}
NONCE = "b" * 32
HEADER = f"v1:{NONCE}:bat,segments_3p,133,971,17615,17620,{'a' * 64}"


def test_nonce_binds_task_root_and_container():
    task = {"task_id": "q1", "query": "alpha"}
    config = {"project": "bat", "backend_snapshot": {"container_id": "1" * 64}}
    first = witness.request_nonce("/private/tmp/run-1", config, task)
    assert first != witness.request_nonce("/private/tmp/run-2", config, task)
    assert first != witness.request_nonce("/private/tmp/run-1", config, {**task, "task_id": "q2"})
    assert first != witness.request_nonce("/private/tmp/run-1", config, {**task, "query": "beta"})
    assert first != witness.request_nonce(
        "/private/tmp/run-1", {**config, "backend_snapshot": {"container_id": "2" * 64}}, task
    )


def test_exact_selected_reader_witness():
    result = witness.verify_header(
        [HEADER], nonce=NONCE, project="bat", native_commits={"bat": COMMIT}
    )
    assert result["scope"] == "selected_query_project_loaded_reader"
    assert result["commit"] == COMMIT


@pytest.mark.parametrize(
    "headers,nonce,project,commits",
    [
        (None, NONCE, "bat", {"bat": COMMIT}),
        ([], NONCE, "bat", {"bat": COMMIT}),
        ([HEADER, HEADER], NONCE, "bat", {"bat": COMMIT}),
        ([HEADER], "c" * 32, "bat", {"bat": COMMIT}),
        ([HEADER], NONCE, "other", {"other": COMMIT}),
        (
            [HEADER + ":other,segments_3p,133,971,17615,17620," + "a" * 64],
            NONCE,
            "bat",
            {"bat": COMMIT},
        ),
        ([HEADER], NONCE, "bat", {"bat": {**COMMIT, "generation": 134}}),
        ([HEADER], NONCE, "bat", {"bat": {**COMMIT, "fileNamesSha256": "c" * 64}}),
        ([HEADER], NONCE, "bat", {"bat": COMMIT, "other": COMMIT}),
        ([HEADER.replace(",133,", ",0133,")], NONCE, "bat", {"bat": COMMIT}),
        ([HEADER.replace(",133,", ",True,")], NONCE, "bat", {"bat": COMMIT}),
        ([HEADER.replace(",971,", ",971.0,")], NONCE, "bat", {"bat": COMMIT}),
        ([HEADER.replace("segments_3p,", ",")], NONCE, "bat", {"bat": COMMIT}),
        ([HEADER.rsplit(",", 1)[0]], NONCE, "bat", {"bat": COMMIT}),
        ([HEADER.replace("segments_3p", "segments_3q")], NONCE, "bat", {"bat": COMMIT}),
        ([HEADER + "\n"], NONCE, "bat", {"bat": COMMIT}),
    ],
)
def test_reader_witness_mutants_rejected(headers, nonce, project, commits):
    with pytest.raises(ValueError):
        witness.verify_header(headers, nonce=nonce, project=project, native_commits=commits)


@pytest.mark.parametrize(
    "field,value",
    [
        ("generation", 133.0),
        ("generation", True),
        ("readerVersion", 971.0),
        ("readerVersion", "971"),
        ("numDocs", 17615.0),
        ("numDocs", True),
        ("maxDoc", 17620.0),
        ("segmentsFile", None),
        ("fileNamesSha256", "A" * 64),
    ],
)
def test_native_commit_aliases_rejected_even_with_exact_header(field, value):
    with pytest.raises(ValueError, match="native reader commit"):
        witness.verify_header(
            [HEADER],
            nonce=NONCE,
            project="bat",
            native_commits={"bat": {**COMMIT, field: value}},
        )


def test_native_commit_missing_or_extra_field_rejected():
    for changed in (
        {key: value for key, value in COMMIT.items() if key != "segmentsFile"},
        {**COMMIT, "unknown": 0},
    ):
        with pytest.raises(ValueError, match="native reader commit shape"):
            witness.verify_header(
                [HEADER], nonce=NONCE, project="bat", native_commits={"bat": changed}
            )


def test_instrumented_war_rejects_unrelated_byte_change(tmp_path, monkeypatch):
    original_xml = (
        b'<web-app xmlns="https://jakarta.ee/xml/ns/jakartaee">'
        b"<context-param><param-name>CONFIGURATION</param-name>"
        b"<param-value>/var/opengrok/etc/configuration.xml</param-value>"
        b"</context-param></web-app>"
    )
    readonly_xml = (
        b'<web-app xmlns="https://jakarta.ee/xml/ns/jakartaee">'
        b"<context-param><param-name>CONFIGURATION</param-name>"
        b"<param-value>/opengrok/etc/configuration.xml</param-value>"
        b"</context-param><security-constraint><web-resource-collection>"
        b"<web-resource-name>deny API writes</web-resource-name>"
        b"<url-pattern>/api/*</url-pattern><http-method-omission>GET</http-method-omission>"
        b"</web-resource-collection><auth-constraint/></security-constraint></web-app>"
    )
    base = {"WEB-INF/web.xml": original_xml, "WEB-INF/classes/Other.class": b"original"}
    base.update({name: b"old" for name in witness.CLASS_FILES})
    modified = {**base, "WEB-INF/web.xml": readonly_xml}
    modified.update({name: b"new" for name in witness.CLASS_FILES})
    monkeypatch.setattr(
        witness,
        "PINNED_CLASS_SHA256",
        {name: hashlib.sha256(b"new").hexdigest() for name in witness.CLASS_FILES},
    )
    webapps = tmp_path / "webapps"
    etc = tmp_path / "etc"
    etc.mkdir()
    (etc / "configuration.xml").write_text("fixture")
    base_war = tmp_path / "source.war"
    instrumented_war = tmp_path / "instrumented.war"

    def write_installed():
        for name, data in modified.items():
            target = webapps / "ROOT" / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
        with zipfile.ZipFile(instrumented_war, "w") as archive:
            for name, data in modified.items():
                archive.writestr(name, data)

    with zipfile.ZipFile(base_war, "w") as archive:
        for name, data in base.items():
            archive.writestr(name, data)
    write_installed()
    auxiliary = {}
    for name in ("original_source", "patched_source", "source_patch"):
        path = tmp_path / name
        path.write_text(name)
        auxiliary[name] = str(path)
    config = {
        "readonly_service": {
            "webapps_root": str(webapps),
            "etc_root": str(etc),
            "source_war": str(base_war),
        },
        "query_reader_witness": {"instrumented_war": str(instrumented_war), **auxiliary},
    }
    assert live._opengrok_readonly_files(config)["instrumented_war_sha256"]
    one_class = next(iter(witness.CLASS_FILES))
    modified[one_class] = b"arbitrary substitute"
    write_installed()
    with pytest.raises(ValueError, match="class digest differs"):
        live._opengrok_readonly_files(config)
    modified[one_class] = b"new"
    modified["WEB-INF/classes/Other.class"] = b"mutated"
    write_installed()
    with pytest.raises(ValueError):
        live._opengrok_readonly_files(config)


def test_readonly_http_ignores_ambient_proxy_for_config_view_search_and_write(
    tmp_path, monkeypatch
):
    direct_calls = []
    direct_put_bodies = []
    proxy_calls = []

    class Direct(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_GET(self):
            direct_calls.append(("GET", self.path, self.headers.get(witness.REQUEST_HEADER)))
            self.send_response(200)
            if self.path.startswith("/api/v1/search?"):
                self.send_header(witness.HEADER, HEADER)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", "6")
            self.end_headers()
            self.wfile.write(b"direct")

        def do_PUT(self):
            direct_put_bodies.append(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
            direct_calls.append(("PUT", self.path, None))
            self.send_response(403)
            self.send_header("Content-Length", "6")
            self.end_headers()
            self.wfile.write(b"denied")

    class Proxy(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_GET(self):
            self.rfile.read(int(self.headers.get("Content-Length", "0")))
            proxy_calls.append(self.path)
            self.send_response(418)
            self.send_header("Content-Length", "5")
            self.end_headers()
            self.wfile.write(b"proxy")

        do_PUT = do_GET

    direct = ThreadingHTTPServer(("127.0.0.1", 0), Direct)
    proxy = ThreadingHTTPServer(("127.0.0.1", 0), Proxy)
    threads = [
        threading.Thread(target=server.serve_forever, daemon=True) for server in (direct, proxy)
    ]
    for thread in threads:
        thread.start()
    try:
        proxy_url = f"http://127.0.0.1:{proxy.server_address[1]}"
        monkeypatch.setenv("http_proxy", proxy_url)
        monkeypatch.setenv("no_proxy", "")
        monkeypatch.setattr(urllib.request, "getproxies", lambda: {"http": proxy_url})
        monkeypatch.setattr(urllib.request, "proxy_bypass", lambda _host: False)
        base = f"http://127.0.0.1:{direct.server_address[1]}"
        readonly = {
            "base_url": base,
            "readonly_service": {},
            "backend_snapshot": {"mount_destination": "/opengrok/data/index"},
        }
        for endpoint in ("/api/v1/configuration/dataRoot", "/api/v1/file/content"):
            assert live._http(readonly, endpoint, {}, "application/json")[:3] == (
                200,
                "application/json",
                b"direct",
            )
        status, content_type, raw, _elapsed, headers = live._opengrok_witness_http(
            readonly, "/api/v1/search", {"projects": "bat"}, NONCE
        )
        assert (status, content_type, raw, headers) == (
            200,
            "application/json",
            b"direct",
            [HEADER],
        )
        live._opengrok_write_denial_probe(readonly, tmp_path / "write", capture=True)
        assert [(method, path.split("?", 1)[0]) for method, path, _ in direct_calls] == [
            ("GET", "/api/v1/configuration/dataRoot"),
            ("GET", "/api/v1/file/content"),
            ("GET", "/api/v1/search"),
            ("PUT", "/api/v1/configuration/dataRoot"),
        ]
        assert direct_calls[2][2] == NONCE
        assert direct_put_bodies == [b"/opengrok/data"]
        assert proxy_calls == []
        assert live._http({"base_url": base}, "/sourcegraph", {}, "application/json")[0] == 418
        assert len(proxy_calls) == 1
    finally:
        for server in (direct, proxy):
            server.shutdown()
            server.server_close()
        for thread in threads:
            thread.join()
