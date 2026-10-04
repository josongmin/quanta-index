"""Local fake services prove that live rows come from HTTP/process responses."""

import copy
import hashlib
import json
import os
import selectors
import shutil
import socket
import subprocess
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


@pytest.mark.parametrize(
    ("query", "expected"),
    [
        ("alpha or beta", '"alpha" "or" "beta"'),
        ("AND\tNOT\nlow 123", '"AND" "NOT" "low" "123"'),
        (
            "path:pkg /name/ (x) token~1",
            '"path:pkg" "/name/" "(x)" "token~1"',
        ),
        ('quote"value', '/quote"value/'),
        ('a"b\\c/[x].*', r'/a"b\\c\/\[x\]\.\*/'),
        ("value\\tail", '"value\\tail"'),
    ],
)
def test_cs_nl_literal_query_has_independent_syntax_goldens(query, expected):
    assert live._cs_literal_query(query) == expected


@pytest.mark.parametrize("query", ["", " \t\n", "alpha\x00beta", None])
def test_cs_nl_literal_query_refuses_unrepresentable_data(query):
    with pytest.raises(ValueError, match="cs natural-language query"):
        live._cs_literal_query(query)


def test_cs_nl_capture_preserves_literal_request_and_argv(tmp_path, monkeypatch):
    task = {"task_id": "N1", "query": "alpha or beta path:missing"}
    effective = '"alpha" "or" "beta" "path:missing"'
    binary = tmp_path / "cs"
    target = tmp_path / "raw.json"
    calls = []

    def process(argv, timeout):
        calls.append(argv)
        assert argv[-1] == effective
        assert timeout == 60
        return 0, b"null", b"", 2.0

    monkeypatch.setattr(live, "_process", process)
    row = live._cs(binary, task, [], tmp_path, {}, target, literal_query=True)
    assert row["submitted_query"] == task["query"]
    assert row["request_query"] == effective
    assert row["request_mode"] == "natural_language_file_search"
    assert row["paths"] == []
    assert json.loads(target.with_suffix(".process.json").read_bytes())["argv"] == calls[0]


@pytest.mark.parametrize(
    ("query", "expected"),
    [
        ("math/rand and math/rand/v2?", r"math\/rand and math\/rand\/v2\?"),
        (
            'field:x +y (z) [a] {b} ^2 ~1 * ? ! && || \\"q"',
            r"field\:x \+y \(z\) \[a\] \{b\} \^2 \~1 \* \? \! \&\& \|\| \\\"q\"",
        ),
        ("AND\tOR\nNOT and or not", '"AND"\t"OR"\n"NOT" and or not'),
    ],
)
def test_opengrok_nl_literal_query_has_fixed_lucene_escape_goldens(query, expected):
    assert live._opengrok_query(query, literal_query=True) == expected
    assert live._opengrok_query(query) == query


def test_opengrok_nl_capture_preserves_submitted_and_effective_query(tmp_path, monkeypatch):
    query = "How does math/rand/v2 work?"
    effective = r"How does math\/rand\/v2 work\?"
    config = {"project": "fixture", "server_image_digest": "a" * 64}
    task = {"task_id": "N1", "query": query}
    raw = json.dumps(
        {"time": 1, "resultCount": 0, "results": {}, "startDocument": 0, "endDocument": 0}
    ).encode()
    calls = []

    def http(_config, endpoint, params, accept):
        calls.append((endpoint, params, accept))
        assert params["full"] == effective
        return 200, "application/json", raw, 2.0

    monkeypatch.setattr(live, "_http", http)
    row = live._opengrok(config, task, [], tmp_path, {}, tmp_path / "raw.json", literal_query=True)
    assert row["submitted_query"] == query
    assert row["request_query"] == effective
    assert row["request_mode"] == "natural_language_file_search"
    assert len(calls) == 1
    assert (tmp_path / "raw.json").read_bytes() == raw
    assert live._opengrok_response(
        config, task, [], tmp_path, {}, 200, "application/json", raw, 2.0, literal_query=True
    ) == {key: value for key, value in row.items() if key != "completed_response"}
    live._validate_completed_row(row, row["file_paths_top_10"])


def test_completed_clock_covers_request_and_normalization_but_excludes_persistence(
    tmp_path, monkeypatch
):
    ticks = [0]
    raw = json.dumps(
        {"time": 1, "resultCount": 0, "results": {}, "startDocument": 0, "endDocument": 0}
    ).encode()
    native_query = live._opengrok_query
    native_paths = live._opengrok_native_paths
    native_row = live._row
    native_sha = live._sha
    native_write = live._write

    def query(value, **kwargs):
        ticks[0] += 2_000_000
        return native_query(value, **kwargs)

    def http(*args, **kwargs):
        ticks[0] += 3_000_000
        return 200, "application/json", raw, 3.0

    def paths(*args, **kwargs):
        ticks[0] += 5_000_000
        return native_paths(*args, **kwargs)

    def score(*args, **kwargs):
        ticks[0] += 7_000_000
        return native_row(*args, **kwargs)

    def sha(*args, **kwargs):
        ticks[0] += 9_000_000
        return native_sha(*args, **kwargs)

    def write(*args, **kwargs):
        ticks[0] += 11_000_000
        return native_write(*args, **kwargs)

    monkeypatch.setattr(live.time, "monotonic_ns", lambda: ticks[0])
    monkeypatch.setattr(live, "_opengrok_query", query)
    monkeypatch.setattr(live, "_http", http)
    monkeypatch.setattr(live, "_opengrok_native_paths", paths)
    monkeypatch.setattr(live, "_row", score)
    monkeypatch.setattr(live, "_sha", sha)
    monkeypatch.setattr(live, "_write", write)
    row = live._opengrok(
        {"project": "fixture", "server_image_digest": "a" * 64},
        {"task_id": "T1", "query": "symbol"},
        [],
        tmp_path,
        {},
        tmp_path / "raw.json",
    )
    assert row["elapsed_ms"] == 3.0
    assert row["completed_response"]["duration_ns"] == 10_000_000
    assert ticks[0] == 62_000_000
    live._validate_completed_row(row, [])
    for mutation in (
        lambda value: value["completed_response"].__setitem__("duration_ns", -1),
        lambda value: value["completed_response"].__setitem__("output_sha256", "0" * 64),
    ):
        changed = copy.deepcopy(row)
        mutation(changed)
        with pytest.raises(ValueError, match="completed response"):
            live._validate_completed_row(changed, [])
    with pytest.raises(ValueError, match="completed response"):
        live._validate_completed_row(row, ["forged.go"])


def test_sourcegraph_completed_clock_excludes_evidence_replay_and_raw_writes(tmp_path, monkeypatch):
    ticks = [0]
    task = {"task_id": "T1", "query": "symbol"}
    config = {"repository": "benchmark/fixture", "server_image_digest": "a" * 64}
    manifest = {
        "repository_commit": "b" * 40,
        "files": [{"path": "src/a.go", "file_sha256": "c" * 64}],
    }
    original_write = live._write

    def query(*args, **kwargs):
        ticks[0] += 2_000_000
        return "pinned query"

    def http(*args, **kwargs):
        ticks[0] += 3_000_000
        return 200, "text/event-stream", b"native", 3.0

    def normalized(*args, **kwargs):
        ticks[0] += 5_000_000
        return [], [], [], 0, 0

    def replay(*args, **kwargs):
        ticks[0] += 7_000_000
        return live._row(task, [], [], 3.0, file_paths_top_10=[])

    def write(*args, **kwargs):
        ticks[0] += 11_000_000
        return original_write(*args, **kwargs)

    monkeypatch.setattr(live.time, "monotonic_ns", lambda: ticks[0])
    monkeypatch.setattr(live.sourcegraph, "query_expression", query)
    monkeypatch.setattr(live.sourcegraph, "normalize_stream", normalized)
    monkeypatch.setattr(live, "_http", http)
    monkeypatch.setattr(live, "_sourcegraph_response", replay)
    monkeypatch.setattr(live, "_write", write)
    row = live._sourcegraph(config, task, [], manifest, tmp_path, {}, tmp_path / "raw.stream")
    assert row["completed_response"]["duration_ns"] == 10_000_000
    assert ticks[0] == 39_000_000
    live._validate_completed_row(row, [])


def index_scope_fixture(
    tmp_path,
    *,
    manifest_raw=None,
    config=None,
    projection=None,
    snapshot=None,
    view=None,
    release_digest=None,
):
    """Fixed two-file oracle, or source-bound inputs from the HTTP fixture."""
    scope = live.sourcegraph_index_scope
    root = tmp_path / "native-audit"
    root.mkdir()
    paths_root = tmp_path / "path-audit"
    paths_root.mkdir()
    release_digest = release_digest or "sha256:" + "e" * 64
    contents = {"a.go": b"func A() {}\n", "b.go": b"func B() {}\n"}
    if manifest_raw is None:
        manifest_raw = json.dumps(
            {
                "repository_commit": "f" * 40,
                "files": [
                    {"path": name, "file_sha256": live._sha(body)}
                    for name, body in contents.items()
                ],
            }
        ).encode()
    manifest = json.loads(manifest_raw)
    files = manifest["files"]
    if config is None:
        config = {
            "repository": "benchmark/fixture",
            "server_image_digest": "a" * 64,
            "backend_snapshot": {
                "root": str(tmp_path / "index"),
                "container_id": "c" * 64,
                "mount_destination": "/index",
                "container_port": "7080/tcp",
            },
        }
    if projection is None:
        projection = {
            "source_revision": manifest["repository_commit"],
            "projection_revision": "d" * 40,
        }
    if snapshot is None:
        rows = [{"path": "fixture.zoekt", "sha256": live._sha(b"fixed-index"), "bytes": 11}]
        snapshot = {
            "runtime": {
                "container_id": "c" * 64,
                "image_sha256": "a" * 64,
                "pid": 123,
                "started_at": "2026-10-04T00:00:00Z",
                "restart_count": 0,
                "mount_source": config["backend_snapshot"]["root"],
                "mount_destination": "/index",
                "container_port": "7080/tcp",
                "service_port": 18080,
            },
            "files": rows,
            "tree_sha256": live._sha(live.canonical_json(rows).encode()),
        }
    repo = config["repository"].removeprefix("benchmark/")
    runtime = snapshot["runtime"]
    native_runtime = {
        "container_id": runtime["container_id"],
        "image_id": "sha256:" + runtime["image_sha256"],
        "pid": runtime["pid"],
        "started_at": runtime["started_at"],
        "restart_count": runtime["restart_count"],
        "mounts": [
            {
                "Type": "bind",
                "Source": runtime["mount_source"],
                "Destination": runtime["mount_destination"],
                "RW": False,
            }
        ],
    }

    def write(path, value):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value, sort_keys=True))

    native_index = {"runtime": native_runtime, "files": snapshot["files"]}
    for phase in ("before", "after"):
        write(root / f"native-index-{phase}.json", native_index)
        write(paths_root / f"native-index-{phase}.json", snapshot)
    native_rows = []
    for file in files:
        body = (view / file["path"]).read_bytes() if view else contents[file["path"]]
        path = root / "native-file-bodies" / repo / file["path"]
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(body)
        native_rows.append(
            {
                "repository": repo,
                "path": file["path"],
                "file_sha256": file["file_sha256"],
                "actual_sha256": live._sha(body),
                "bytes": len(body),
                "http_status": 200,
                "matches": True,
                "seconds": 0.01,
            }
        )
    (root / "native-rows.jsonl").write_text("".join(json.dumps(row) + "\n" for row in native_rows))
    for name in ("zoekt-webserver", "native-worker.py", "probe_native_contents.py"):
        (root / name).write_bytes(name.encode())
    manifest_path = tmp_path / "manifests" / repo / "code_only.json"
    manifest_path.parent.mkdir(parents=True)
    manifest_path.write_bytes(manifest_raw)
    write(
        root / "precommit.json",
        {
            "native_runtime": native_runtime,
            "native_binary_sha256": live._sha_file(root / "zoekt-webserver"),
            "worker_sha256": live._sha_file(root / "native-worker.py"),
            "script_sha256": live._sha_file(root / "probe_native_contents.py"),
            "tasks": len(files),
            "manifest_sha256": {str(manifest_path): live._sha(manifest_raw)},
        },
    )
    write(
        root / "owned-probe-cleanup.json",
        {
            "deployed_binary_sha256": live._sha_file(root / "zoekt-webserver"),
            "only_owned_reader_stopped": True,
        },
    )
    write(
        root / "result.json",
        {
            "status": "VERIFIED",
            "qualified": False,
            "bindings_unchanged": True,
            "failures": [],
            "worker_exit": 0,
            "release_digest": release_digest,
            "rows_sha256": live._sha_file(root / "native-rows.jsonl"),
            "expected_files": len(files),
            "observed_files": len(files),
            "matched_files": len(files),
        },
    )
    revision = projection["projection_revision"]
    hits = [
        {
            "type": "path",
            "repository": config["repository"],
            "commit": revision,
            "path": row["path"],
        }
        for row in files
    ]
    stream = b"".join(
        b"event: " + kind.encode() + b"\ndata: " + json.dumps(data).encode() + b"\n\n"
        for kind, data in [
            ("matches", hits),
            ("progress", {"done": True, "matchCount": len(hits), "skipped": []}),
            ("done", {}),
        ]
    )
    (paths_root / f"{repo}.stream").write_bytes(stream)
    write(
        paths_root / "summary.json",
        {
            "qualified": False,
            "status": "path_index_observed",
            "release_digest": release_digest,
            "server_image_digest": config["server_image_digest"],
            "native_index_sha256": snapshot["tree_sha256"],
            "repositories": [
                {
                    "repository": repo,
                    "source_commit": manifest["repository_commit"],
                    "projection_commit": revision,
                    "files": len(files),
                    "native_match_count": len(files),
                    "raw_stream_sha256": live._sha(stream),
                    "query": f".* repo:^benchmark/{repo}$ rev:{revision} type:path patternType:regexp count:all",
                }
            ],
        },
    )
    receipt = {
        "schema": "external_index_scope_v1",
        "backend": "sourcegraph_zoekt",
        "scope": scope.SCOPE,
        "qualified_comparison": False,
        "release_digest": release_digest,
        "repository": repo,
        "repository_commit": manifest["repository_commit"],
        "corpus_manifest_sha256": live._sha(manifest_raw),
        "files": files,
        "native_index_inventory_sha256": live._sha_file(root / "native-index-before.json"),
        "native_runtime": native_runtime,
        "native_binary_sha256": live._sha_file(root / "zoekt-webserver"),
        "native_capture_root": str(root),
        "native_rows_sha256": live._sha_file(root / "native-rows.jsonl"),
        "path_inventory_proof": str(paths_root / "summary.json"),
        "limitations": [
            "No assertion of every posting's correctness",
            "No human qrels claim",
            "No speed qualification",
        ],
    }
    receipt_path = root / "receipt.json"
    write(receipt_path, receipt)
    return receipt_path, dict(
        manifest_raw=manifest_raw,
        release_digest=release_digest,
        config=config,
        projection=projection,
        snapshot=snapshot,
    )


def test_native_index_scope_replays_exact_paths_and_stored_bytes(tmp_path):
    receipt, args = index_scope_fixture(tmp_path)
    result = live.sourcegraph_index_scope.verify(receipt, **args)
    assert result["files"] == 2
    assert result["scope"] == "indexed_path_inventory_and_native_stored_document_bytes"
    assert result["projection_revision"] == "d" * 40
    assert result["receipt_sha256"] == "sha256:" + live._sha_file(receipt)
    assert "qualified" not in result


@pytest.mark.parametrize(
    "fault",
    [
        "body",
        "missing_body",
        "binary",
        "worker",
        "source",
        "qualified",
        "other_repository",
        "index",
        "process",
        "restart",
        "writable_mount",
        "http_boolean",
        "row_duplicate",
        "row_missing",
        "path_commit",
        "path_extra",
        "path_partial",
        "path_skipped",
        "path_count",
        "manifest_input",
    ],
)
def test_native_index_scope_refuses_independent_binding_faults(tmp_path, fault):
    receipt_path, args = index_scope_fixture(tmp_path)
    root = receipt_path.parent
    receipt = json.loads(receipt_path.read_bytes())
    paths = Path(receipt["path_inventory_proof"])
    if fault in {"body", "missing_body"}:
        p = root / "native-file-bodies/fixture/a.go"
        p.write_bytes(b"different") if fault == "body" else p.unlink()
    elif fault in {"binary", "worker"}:
        (root / ("zoekt-webserver" if fault == "binary" else "native-worker.py")).write_bytes(
            b"different"
        )
    elif fault == "source":
        receipt["repository_commit"] = "0" * 40
    elif fault == "qualified":
        receipt["qualified_comparison"] = True
    elif fault == "other_repository":
        args["config"]["repository"] = "benchmark/other"
    elif fault == "index":
        args["snapshot"]["files"][0]["sha256"] = "0" * 64
    elif fault in {"process", "restart"}:
        args["snapshot"]["runtime"]["pid" if fault == "process" else "restart_count"] += 1
    elif fault == "writable_mount":
        for phase in ("before", "after"):
            p = root / f"native-index-{phase}.json"
            value = json.loads(p.read_bytes())
            value["runtime"]["mounts"][0]["RW"] = True
            p.write_text(json.dumps(value))
        receipt["native_runtime"]["mounts"][0]["RW"] = True
        receipt["native_index_inventory_sha256"] = live._sha_file(root / "native-index-before.json")
    elif fault in {"http_boolean", "row_duplicate", "row_missing"}:
        p = root / "native-rows.jsonl"
        rows = [json.loads(line) for line in p.read_text().splitlines()]
        if fault == "http_boolean":
            rows[0]["http_status"] = True
        elif fault == "row_duplicate":
            rows[1] = copy.deepcopy(rows[0])
        else:
            rows.pop()
        p.write_text("".join(json.dumps(row) + "\n" for row in rows))
        receipt["native_rows_sha256"] = live._sha_file(p)
        result = json.loads((root / "result.json").read_bytes())
        result["rows_sha256"] = receipt["native_rows_sha256"]
        (root / "result.json").write_text(json.dumps(result))
    elif fault.startswith("path_"):
        p = paths.parent / "fixture.stream"
        events = live.sourcegraph._events(p.read_bytes())
        if fault == "path_commit":
            events[0][1][0]["commit"] = "0" * 40
        elif fault == "path_extra":
            events[0][1][0]["path"] = "other.go"
        elif fault == "path_skipped":
            events[1][1]["skipped"] = [{"reason": "limit"}]
        elif fault == "path_count":
            events[1][1]["matchCount"] += 1
        else:
            events.pop()
        p.write_bytes(
            b"".join(
                b"event: " + kind.encode() + b"\ndata: " + json.dumps(data).encode() + b"\n\n"
                for kind, data in events
            )
        )
        value = json.loads(paths.read_bytes())
        value["repositories"][0]["raw_stream_sha256"] = live._sha_file(p)
        if fault == "path_count":
            value["repositories"][0]["native_match_count"] += 1
        paths.write_text(json.dumps(value))
    else:
        (tmp_path / "manifests/fixture/code_only.json").write_bytes(b"{}")
    receipt_path.write_text(json.dumps(receipt))
    with pytest.raises((ValueError, OSError)):
        live.sourcegraph_index_scope.verify(receipt_path, **args)


def test_native_index_scope_refuses_evidence_change_during_replay(tmp_path, monkeypatch):
    receipt, args = index_scope_fixture(tmp_path)
    original = live.sourcegraph._events

    def changed_during_replay(raw):
        (receipt.parent / "native-file-bodies/fixture/a.go").write_bytes(b"changed after hash")
        return original(raw)

    monkeypatch.setattr(live.sourcegraph, "_events", changed_during_replay)
    with pytest.raises(ValueError, match="evidence changed during replay"):
        live.sourcegraph_index_scope.verify(receipt, **args)


def test_index_scope_spec_requires_backend_and_projection_binding(tmp_path):
    receipt, args = index_scope_fixture(tmp_path)
    config = {
        **args["config"],
        "base_url": "http://127.0.0.1:18080",
        "projection_git_root": str(tmp_path / "projection"),
        "indexed_scope_receipt": str(receipt),
    }
    keys = {"base_url", "repository", "server_image_digest"}
    optional = {"backend_snapshot", "projection_git_root", "indexed_scope_receipt"}
    assert live._service(config, keys, optional) == config
    for removed in ("backend_snapshot", "projection_git_root"):
        with pytest.raises(ValueError, match="requires backend and projection"):
            live._service(
                {key: value for key, value in config.items() if key != removed}, keys, optional
            )
    with pytest.raises(ValueError, match="canonical absolute"):
        live._service({**config, "indexed_scope_receipt": "relative.json"}, keys, optional)


def test_standalone_cli_bootstraps_its_source_package_from_external_cwd(tmp_path):
    env = dict(os.environ)
    env.pop("PYTHONPATH", None)
    completed = subprocess.run(
        [sys.executable, str(Path(live.__file__).resolve()), "--help"],
        cwd=tmp_path,
        env=env,
        capture_output=True,
        text=True,
        timeout=15,
    )
    assert completed.returncode == 0, completed.stderr
    assert "--spec" in completed.stdout
    assert "--verify" in completed.stdout


def test_bound_release_reuses_one_full_validation_and_refuses_changed_bytes(
    tmp_path, lexical_release_seed, monkeypatch
):
    release = tmp_path / "release"
    shutil.copytree(lexical_release_seed, release)
    original_validate = live.corpus_release.validate
    calls = []

    def validate_once(root):
        calls.append(root)
        return original_validate(root)

    monkeypatch.setattr(live.corpus_release, "validate", validate_once)
    bound = live.BoundRelease.begin(release)
    assert calls == [release]
    assert bound.recheck(release) == bound.document
    assert calls == [release]

    (release / "untracked.txt").write_text("extra")
    with pytest.raises(ValueError, match="complete file bytes changed"):
        bound.recheck(release)
    (release / "untracked.txt").unlink()
    (release / "release.json").write_bytes((release / "release.json").read_bytes() + b" ")
    with pytest.raises(ValueError, match="complete file bytes changed"):
        bound.recheck(release)


def test_bound_release_refuses_swapped_root_and_validator_identity(
    tmp_path, lexical_release_seed, monkeypatch
):
    release = tmp_path / "release"
    shutil.copytree(lexical_release_seed, release)
    bound = live.BoundRelease.begin(release)
    other = tmp_path / "other"
    shutil.copytree(release, other)
    with pytest.raises(ValueError, match="root, owner or complete file bytes changed"):
        bound.recheck(other)

    original_hash = live._sha_file
    owner = Path(live.corpus_release.__file__)
    monkeypatch.setattr(
        live,
        "_sha_file",
        lambda path: "0" * 64 if path == owner else original_hash(path),
    )
    with pytest.raises(ValueError, match="root, owner or complete file bytes changed"):
        bound.recheck(release)


def test_bound_release_refuses_mutation_during_full_validation(
    tmp_path, lexical_release_seed, monkeypatch
):
    release = tmp_path / "release"
    shutil.copytree(lexical_release_seed, release)
    original_validate = live.corpus_release.validate

    def mutate_after_validation(root):
        document = original_validate(root)
        (root / "release.json").write_bytes((root / "release.json").read_bytes() + b" ")
        return document

    monkeypatch.setattr(live.corpus_release, "validate", mutate_after_validation)
    with pytest.raises(ValueError, match="changed during full validation"):
        live.BoundRelease.begin(release)


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


@pytest.mark.parametrize("use_bound_release", [False, True])
def test_cs_fuzzy_native_request_and_separate_replay(
    tmp_path, lexical_release_seed, monkeypatch, use_bound_release
):
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
    bound_release = (
        live.BoundRelease.begin(Path(spec["corpus"]["release_path"])) if use_bound_release else None
    )
    if use_bound_release:
        monkeypatch.setattr(
            live.corpus_release,
            "validate",
            lambda _root: pytest.fail("bound cs fuzzy capture must reuse full validation"),
        )
    summary = live.capture_cs_fuzzy(spec_path, bound_release=bound_release)
    assert summary["tasks"] == 20
    assert summary["scoring_status"] == "not_scored"
    assert "completed_response_boundary" not in summary
    assert live.verify_cs_fuzzy(root, bound_release=bound_release) == summary
    capture_path = root / "capture.json"
    capture_path.write_text(json.dumps({**summary, "completed_response_boundary": "wrong"}))
    with pytest.raises(ValueError, match="cs fuzzy capture binding"):
        live.verify_cs_fuzzy(root, bound_release=bound_release)
    capture_path.write_text(json.dumps(summary))
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
        live.verify_cs_fuzzy(root, bound_release=bound_release)


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


def test_opengrok_native_inventory_uses_manifest_cardinality_not_backend_file_cap():
    paths = [f"src/file-{index:05d}.go" for index in range(4097)]
    manifest = {"files": [{"path": path} for path in paths]}
    native = json.dumps(["/fixture/" + path for path in reversed(paths)]).encode()
    live._opengrok_indexed_inventory_response(
        {"project": "fixture"}, manifest, 200, "application/json", native
    )


def test_opengrok_native_inventory_refuses_over_byte_response(monkeypatch):
    monkeypatch.setattr(live, "MAX_HTTP_BYTES", 20)
    with pytest.raises(ValueError, match="response byte limit"):
        live._opengrok_indexed_inventory_response(
            {"project": "fixture"},
            {"files": [{"path": "file.go"}]},
            200,
            "application/json",
            b'["/fixture/file.go", "extra"]',
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
    monkeypatch.setattr(live, "MAX_BACKEND_INDEX_FILES", 1)
    with pytest.raises(ValueError, match="exceeds 4096 files"):
        live._backend_tree(root)
    monkeypatch.setattr(live, "MAX_BACKEND_INDEX_FILES", 4096)
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
    assert json.loads((target / "indexed-files-after-b0000.json").read_bytes()) == [
        "/fixture/file.go",
        "/fixture/extra.go",
    ]


def test_opengrok_full_view_batches_each_have_closed_native_inventory(tmp_path, monkeypatch):
    view = tmp_path / "view"
    view.mkdir()
    files = []
    for index in range(3):
        path = f"{index}.go"
        body = f"source {index}".encode()
        (view / path).write_bytes(body)
        files.append({"path": path, "file_sha256": live._sha(body)})
    manifest = {"files": files}
    monkeypatch.setattr(live, "MAX_INDEXED_VIEW_BATCH_FILES", 2)
    inventory = json.dumps([f"/fixture/{row['path']}" for row in files]).encode()
    calls = []

    def fake_http(_config, endpoint, params, _accept):
        calls.append(endpoint)
        if endpoint.endswith("/files"):
            return 200, "application/json", inventory, 1.0
        path = params["path"].removeprefix("/fixture/")
        return 200, "application/octet-stream", (view / path).read_bytes(), 1.0

    monkeypatch.setattr(live, "_http", fake_http)
    target = tmp_path / "probe"
    assert live._opengrok_index_batches(manifest, view) == [(0, 2), (2, 3)]
    live._opengrok_indexed_view({"project": "fixture"}, manifest, view, target)
    assert calls == [
        "/api/v1/projects/fixture/files",
        "/api/v1/file/content",
        "/api/v1/file/content",
        "/api/v1/projects/fixture/files",
        "/api/v1/projects/fixture/files",
        "/api/v1/file/content",
        "/api/v1/projects/fixture/files",
    ]
    assert (target / "indexed-files-before-b0001.json").read_bytes() == inventory
    assert (target / "indexed-files-after-b0001.json").read_bytes() == inventory
    assert (target / "000002.content").read_bytes() == b"source 2"

    inventory_calls = 0

    def mutated_http(_config, endpoint, params, accept):
        nonlocal inventory_calls
        if endpoint.endswith("/files"):
            inventory_calls += 1
            if inventory_calls == 4:
                changed = json.dumps(["/fixture/0.go", "/fixture/1.go", "/fixture/extra.go"])
                return 200, "application/json", changed.encode(), 1.0
        return fake_http(_config, endpoint, params, accept)

    monkeypatch.setattr(live, "_http", mutated_http)
    with pytest.raises(ValueError, match="indexed file inventory differs from release manifest"):
        live._opengrok_indexed_view({"project": "fixture"}, manifest, view, tmp_path / "mutated")
    assert inventory_calls == 4


def test_opengrok_full_view_refuses_single_source_above_http_byte_bound(tmp_path, monkeypatch):
    view = tmp_path / "view"
    view.mkdir()
    (view / "file.go").write_bytes(b"oversized")
    monkeypatch.setattr(live, "MAX_HTTP_BYTES", 8)
    with pytest.raises(ValueError, match="source exceeds HTTP response byte limit"):
        live._opengrok_index_batches({"files": [{"path": "file.go"}]}, view)


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


@pytest.mark.parametrize(
    "query", ["", " ", "foo\nbar", "foo\tbar", "foo\x00bar", "foo\x7fbar", "foo\ud800"]
)
def test_sourcegraph_invalid_keyword_data_refuses_before_http(tmp_path, monkeypatch, query):
    def unexpected_http(*args, **kwargs):
        raise AssertionError("invalid keyword data reached HTTP submission")

    monkeypatch.setattr(live, "_http", unexpected_http)
    target = tmp_path / "invalid.stream"
    row = live._sourcegraph(
        {}, {"task_id": "invalid", "query": query}, [], {}, tmp_path, {}, target
    )
    assert row["status"] == "unsupported"
    assert row["capability_reason"] == "sourcegraph_invalid_keyword_data"
    assert row["submitted_query"] == query
    assert not target.exists()
    assert "elapsed_ms" not in row
    assert "http_status" not in row
    assert json.loads(target.with_suffix(".capability.json").read_bytes()) == row


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
    (
        "index_changes_during_queries",
        "reserved_query",
        "backend_changes_during_queries",
        "use_bound_release",
        "use_index_scope",
        "scope_changes_during_queries",
        "nl_file_query",
    ),
    [
        (False, False, False, False, False, False, False),
        (False, False, False, True, False, False, False),
        (True, False, False, False, False, False, False),
        (False, True, False, False, False, False, False),
        (False, False, True, False, False, False, False),
        (False, False, False, True, True, False, False),
        (False, False, False, False, True, True, False),
        pytest.param(False, False, False, True, False, False, True, id="nl-file-literal"),
    ],
)
def test_live_capture_makes_three_product_requests_and_retains_raw(
    tmp_path,
    lexical_release_seed,
    index_changes_during_queries,
    reserved_query,
    backend_changes_during_queries,
    use_bound_release,
    use_index_scope,
    scope_changes_during_queries,
    nl_file_query,
    monkeypatch,
):
    lexical_spec, paths = inputs(tmp_path, lexical_release_seed)
    if reserved_query or nl_file_query:
        suite = json.loads(paths["suite"].read_bytes())
        pack = json.loads(paths["query_pack"].read_bytes())
        for value in (suite, pack):
            query = "How does math/rand/v2 work?" if nl_file_query else "not"
            value["tasks"][-1]["query"] = query
            value["tasks"][-1]["query_sha256"] = hashlib.sha256(query.encode()).hexdigest()
        if nl_file_query:
            suite["routes"] = pack["routes"] = ["lexical", "semble-lexical-file"]
            for task in suite["tasks"]:
                task["query_intent"] = "semantic_intent"
                task["evaluation_contract"] = {
                    "request_mode": "natural_language_file_search",
                    "gold_unit": "distinct_file",
                    "result_unit": "distinct_file",
                }
        pack["suite_commitment_sha256"] = live._sha(live.lexical.canonical(suite))
        paths["suite"].write_bytes(live.lexical.canonical(suite))
        paths["query_pack"].write_bytes(live.lexical.canonical(pack))
    corpus = json.loads(lexical_spec.read_text())["corpus"]
    SearchHandler.commit = json.loads(paths["suite"].read_text())["repository_commit"]
    SearchHandler.view = (
        Path(corpus["release_path"]) / "views" / corpus["repository"] / corpus["view"]
    )
    binary = tmp_path / "cs"
    cs_version = "cs version 3.2.0" if nl_file_query else "cs-test"
    binary.write_text(
        f"#!{sys.executable}\nimport json, sys\nfrom pathlib import Path\n"
        "if '--version' in sys.argv:\n"
        f"    print({cs_version!r})\n"
        "else:\n"
        "    root = Path(sys.argv[sys.argv.index('--dir') + 1])\n"
        "    hits = [{'location': str(root / 'src/0.go')}] if sys.argv[-1] in ['symbol_0', '\"symbol_0\"'] else []\n"
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
        if not index_changes_during_queries and not reserved_query:
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
        if use_index_scope:
            projection_root = tmp_path / "projection"
            shutil.copytree(SearchHandler.view, projection_root)
            for argv in (
                ["git", "init", "-q"],
                ["git", "add", "-A"],
                [
                    "git",
                    "-c",
                    "user.name=Scope Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "commit",
                    "-qm",
                    "Fixed projection",
                ],
            ):
                subprocess.run(argv, cwd=projection_root, check=True, capture_output=True)
            spec["sourcegraph"]["projection_git_root"] = str(projection_root)
            release = Path(corpus["release_path"])
            manifest_raw = (
                release / "manifests" / corpus["repository"] / "code_only.json"
            ).read_bytes()
            projection = live._projection_binding(spec["sourcegraph"], json.loads(manifest_raw))
            SearchHandler.commit = projection["projection_revision"]
            receipt, _ = index_scope_fixture(
                tmp_path,
                manifest_raw=manifest_raw,
                config=spec["sourcegraph"],
                projection=projection,
                snapshot=live._backend_snapshot(spec["sourcegraph"]),
                view=SearchHandler.view,
                release_digest=json.loads((release / "release.json").read_bytes())["digest"],
            )
            spec["sourcegraph"]["indexed_scope_receipt"] = str(receipt)
            if scope_changes_during_queries:
                SearchHandler.backend_mutation_path = (
                    tmp_path / "native-audit/native-file-bodies/fixture/src/0.go"
                )
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
        elif scope_changes_during_queries:
            with pytest.raises(ValueError, match="stored bytes differ"):
                live.capture(spec_path)
        else:
            if use_bound_release:
                full_validate = live.corpus_release.validate
                validations = []

                def counted_validate(root):
                    validations.append(root)
                    return full_validate(root)

                monkeypatch.setattr(live.corpus_release, "validate", counted_validate)
                bound = live.BoundRelease.begin(Path(corpus["release_path"]))
                result = live.capture(spec_path, bound_release=bound)
                assert live.verify(Path(spec["output_root"]), bound_release=bound) == result
                assert len(validations) == 1
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
            json.loads((stage / "opengrok-view-post/indexed-files-before-b0000.json").read_bytes())[
                -1
            ]
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
    if scope_changes_during_queries:
        assert not Path(spec["output_root"]).exists()
        assert Path(spec["output_root"] + ".staging/sourcegraph-index-scope.json").exists()
        return
    assert result["indexed_universe_attested"] is False
    assert result["completed_response_boundary"] == live.COMPLETED_BOUNDARY
    assert result["opengrok_indexed_universe_attested"] is False
    if not reserved_query:
        assert set(result["backend_snapshot_sha256"]) == {"sourcegraph", "opengrok"}
    else:
        assert result["backend_snapshot_sha256"] == {}
    file_count = len(json.loads(paths["suite"].read_text())["file_universe"])
    assert (
        result["sourcegraph_index_scope"] is not None
        if use_index_scope
        else result["sourcegraph_index_scope"] is None
    )
    if use_index_scope:
        assert result["sourcegraph_index_scope"]["files"] == file_count
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
        assert sum(json.loads(row).get("file_hit_at_10", False) for row in rows) == 1
        compared = live.lexical.product_result(
            name,
            root / f"{name}_rows.jsonl",
            live.lexical._tasks(suite, pack),
            {entry["path"] for entry in suite["file_universe"]},
        )
        assert compared["completed_response_latency_ms"]["count"] == 20
        assert all(
            row["completed_response_boundary"] == "request_construction_to_normalized_response"
            and row["completed_query_latency_ms"] >= 0
            for row in compared["per_query"]
        )
    if reserved_query:
        rows = [
            json.loads(row) for row in (root / "sourcegraph_rows.jsonl").read_text().splitlines()
        ]
        assert rows[-1]["http_status"] == 200
        assert rows[-1]["request_query"].startswith('content:"not" repo:')
        assert (root / "sourcegraph" / "S19.stream").exists()
        assert rows[-1]["submitted_query"] == "not"
        scored = live.lexical.product_result(
            "sourcegraph",
            root / "sourcegraph_rows.jsonl",
            live.lexical._tasks(suite, pack),
            {row["path"] for row in suite["file_universe"]},
        )
        assert "capability_coverage" not in scored
        assert len(scored["per_query"]) == 20
        assert scored["latency_ms"]["count"] == 20
    if nl_file_query:
        rows = [
            json.loads(line) for line in (root / "opengrok_rows.jsonl").read_text().splitlines()
        ]
        assert all(row["request_mode"] == "natural_language_file_search" for row in rows)
        assert rows[-1]["submitted_query"] == "How does math/rand/v2 work?"
        assert rows[-1]["request_query"] == r"How does math\/rand\/v2 work\?"
    original_read = live._read_control_file

    def control_only(path):
        if path.name.endswith("_rows.jsonl"):
            raise AssertionError("external rows were read as a whole control document")
        return original_read(path)

    with monkeypatch.context() as patch:
        patch.setattr(live, "_read_control_file", control_only)
        assert live.verify(root) == result
    if nl_file_query:
        rows = [json.loads(line) for line in (root / "cs_rows.jsonl").read_text().splitlines()]
        assert rows[-1]["submitted_query"] == "How does math/rand/v2 work?"
        assert rows[-1]["request_query"] == '"How" "does" "math/rand/v2" "work?"'
        assert all(row["request_mode"] == "natural_language_file_search" for row in rows)
        process_path = root / "cs/S19.process.json"
        capture_path = root / "capture.json"
        original_process_bytes, original_capture = (
            process_path.read_bytes(),
            capture_path.read_bytes(),
        )
        terminal = json.loads(original_process_bytes)
        terminal["argv"][-1] = "How does math/rand/v2 work?"
        process_path.write_text(json.dumps(terminal))
        rebound = json.loads(original_capture)
        rebound["raw_capture_sha256"]["cs/S19.process.json"] = live._sha_file(process_path)
        capture_path.write_text(json.dumps(rebound))
        with pytest.raises(ValueError, match="cs terminal metadata differs"):
            live.verify(root)
        process_path.write_bytes(original_process_bytes)
        capture_path.write_bytes(original_capture)
    if not any(
        (
            index_changes_during_queries,
            reserved_query,
            backend_changes_during_queries,
            use_bound_release,
            use_index_scope,
            scope_changes_during_queries,
            nl_file_query,
        )
    ):
        native_paths = {name: path for name, path in paths.items() if not name.endswith("_rows")}
        joined = live.lexical.evaluate_external_captures(
            native_paths, {name: root for name in live.PRODUCTS}
        )
        assert set(joined["external_capture_binding"]["captures"]) == {str(root)}
        assert joined["external_capture_binding"]["captures"][str(root)]["products"] == list(
            live.PRODUCTS
        )
    assert (root / "sourcegraph" / "S00.stream").exists()
    assert (root / "opengrok" / "S00.json").exists()
    assert (root / "cs" / "S00.json").exists()
    summary_path = root / "capture.json"
    if use_index_scope:
        scope_path = root / "sourcegraph-index-scope.json"
        original_scope = scope_path.read_bytes()
        forged_scope = {**result["sourcegraph_index_scope"], "files": file_count + 1}
        scope_path.write_text(json.dumps(forged_scope))
        summary_path.write_text(json.dumps({**result, "sourcegraph_index_scope": forged_scope}))
        with pytest.raises(ValueError, match="retained Sourcegraph index scope differs"):
            live.verify(root)
        scope_path.write_bytes(original_scope)
        summary_path.write_text(json.dumps(result))
        body = tmp_path / "native-audit/native-file-bodies/fixture/src/0.go"
        original_body = body.read_bytes()
        body.write_bytes(b"changed native stored bytes")
        with pytest.raises(ValueError, match="stored bytes differ"):
            live.verify(root)
        body.write_bytes(original_body)
    if not reserved_query:
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
        forged_normalized = live.canonical_json(
            {"status": "success", "file_paths_top_10": rows[1]["gold_paths"]}
        ).encode("utf-8")
        rows[1]["completed_response"]["output_bytes"] = len(forged_normalized)
        rows[1]["completed_response"]["output_sha256"] = live._sha(forged_normalized)
        row_path.write_bytes(b"".join(json.dumps(row).encode() + b"\n" for row in rows))
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
        with pytest.raises(
            ValueError, match="external row disagrees with retained native response"
        ):
            live.lexical.product_result(product, row_path, expected, universe)
        assert all((root / name).read_bytes() == raw for name, raw in native_before.items())
        row_path.write_bytes(original)
        summary_path.write_text(json.dumps(result))
        assert live.verify(root) == result
    mutations = [
        {"schema_version": True},
        {"tasks": True},
        {"quality_qualified": True},
        {"exclusions": []},
        {"completed_response_boundary": "wrong"},
        {"python_executable_sha256": "0" * 64},
        {"python_version": "wrong-runtime"},
        {"cs_binary_sha256": "0" * 64},
        {"cs_version": "wrong-version"},
        {"server_image_digests_operator_supplied": {}},
        {"opengrok_indexed_universe_attested": True},
        {"indexed_universe_attested": True},
        {"rows_sha256": {**result["rows_sha256"], "forged-product": "0" * 64}},
    ]
    if not reserved_query:
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
    inventory_path = root / "opengrok-view-post/indexed-files-before-b0000.json"
    original_inventory = inventory_path.read_bytes()
    inventory_path.write_text(json.dumps(["/fixture/extra.go"]))
    summary_path.write_text(
        json.dumps(
            {
                **result,
                "raw_capture_sha256": {
                    **result["raw_capture_sha256"],
                    "opengrok-view-post/indexed-files-before-b0000.json": live._sha(
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


@pytest.mark.parametrize("product", live.PRODUCTS)
def test_v2_single_product_capture_replays_only_selected_native_evidence(
    tmp_path, lexical_release_seed, product
):
    lexical_spec, paths = inputs(tmp_path, lexical_release_seed)
    suite = json.loads(paths["suite"].read_bytes())
    pack = json.loads(paths["query_pack"].read_bytes())
    suite["tasks"] = suite["tasks"][:1]
    pack["tasks"] = pack["tasks"][:1]
    pack["suite_commitment_sha256"] = live._sha(live.lexical.canonical(suite))
    paths["suite"].write_bytes(live.lexical.canonical(suite))
    paths["query_pack"].write_bytes(live.lexical.canonical(pack))
    corpus = json.loads(lexical_spec.read_text())["corpus"]
    SearchHandler.commit = suite["repository_commit"]
    SearchHandler.view = (
        Path(corpus["release_path"]) / "views" / corpus["repository"] / corpus["view"]
    )
    SearchHandler.calls = []
    SearchHandler.query_seen = False
    SearchHandler.inventory_extra_after_query = False
    SearchHandler.backend_mutation_path = None
    binary = tmp_path / "cs"
    binary.write_text(
        f"#!{sys.executable}\nimport json, sys\nfrom pathlib import Path\n"
        "if '--version' in sys.argv:\n"
        "    print('cs-test')\n"
        "else:\n"
        "    root = Path(sys.argv[sys.argv.index('--dir') + 1])\n"
        "    print(json.dumps([{'location': str(root / 'src/0.go')}]))\n"
    )
    binary.chmod(0o755)
    server = ThreadingHTTPServer(("127.0.0.1", 0), SearchHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        base = f"http://127.0.0.1:{server.server_address[1]}"
        configs = {
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
        }
        spec = {
            "schema_version": 2,
            "products": [product],
            "corpus": corpus,
            "suite": str(paths["suite"]),
            "query_pack": str(paths["query_pack"]),
            product: configs[product],
            "output_root": str(tmp_path / "live"),
        }
        spec_path = tmp_path / "live-spec.json"
        spec_path.write_text(json.dumps(spec))
        summary = live.capture(spec_path)
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
    root = Path(spec["output_root"])
    assert summary["schema_version"] == 2
    assert summary["completed_response_boundary"] == live.COMPLETED_BOUNDARY
    assert summary["products"] == [product]
    assert set(summary["rows_sha256"]) == {product}
    assert live.verify(root) == summary
    retained_spec = root / "spec.json"
    retained_spec_raw = retained_spec.read_bytes()
    other = next(name for name in live.PRODUCTS if name != product)
    for changed in (
        {**spec, other: configs[other]},
        {key: value for key, value in spec.items() if key != product},
    ):
        retained_spec.write_text(json.dumps(changed))
        with pytest.raises(ValueError, match="spec keys differ"):
            live.verify(root)
    retained_spec.write_bytes(retained_spec_raw)
    for other in set(live.PRODUCTS) - {product}:
        assert not (root / other).exists()
        assert not (root / f"{other}_rows.jsonl").exists()
    expected_calls = {
        "sourcegraph": ["/.api/search/stream"],
        "opengrok": ["/api/v1/search"],
        "cs": [],
    }
    assert all(path in SearchHandler.calls for path in expected_calls[product])
    assert (not SearchHandler.calls) == (product == "cs")

    summary_path = root / "capture.json"
    summary_raw = summary_path.read_bytes()
    for selected in ([], [product, product], ["unknown"], list(live.PRODUCTS)):
        if selected == [product]:
            continue
        summary_path.write_text(json.dumps({**summary, "products": selected}))
        with pytest.raises(ValueError, match="unsupported capture metadata"):
            live.verify(root)
    summary_path.write_bytes(summary_raw)
    summary_path.write_text(json.dumps({**summary, "binding": {}}))
    with pytest.raises(ValueError, match="capture binding differs"):
        live.verify(root)
    summary_path.write_bytes(summary_raw)
    raw_name = next(iter(summary["raw_capture_sha256"]))
    native = root / raw_name
    native_raw = native.read_bytes()
    native.write_bytes(native_raw + b" ")
    with pytest.raises(ValueError, match="native bytes differ"):
        live.verify(root)
    native.write_bytes(native_raw)
    native.unlink()
    with pytest.raises(ValueError, match="raw inventory differs"):
        live.verify(root)
    native.write_bytes(native_raw)
    unexpected = root / f"{other}_rows.jsonl"
    unexpected.write_bytes(b"[]\n")
    with pytest.raises(ValueError, match="raw inventory differs"):
        live.verify(root)
    unexpected.unlink()
    row = root / f"{product}_rows.jsonl"
    row_raw = row.read_bytes()
    row.write_bytes(row_raw + b"\n")
    with pytest.raises(ValueError, match="external rows differ"):
        live.verify(root)
    row.write_bytes(row_raw)
    assert live.verify(root) == summary


@pytest.mark.parametrize(
    "products",
    [[], ["sourcegraph", "sourcegraph"], ["cs", "sourcegraph"], ["unknown"]],
)
def test_v2_spec_rejects_noncanonical_products(tmp_path, products):
    spec = {"schema_version": 2, "products": products}
    path = tmp_path / "spec.json"
    path.write_text(json.dumps(spec))
    with pytest.raises(ValueError, match="canonical subset"):
        live._spec(path)
