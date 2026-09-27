#!/usr/bin/env python3
"""Capture fresh Sourcegraph, OpenGrok, and cs rows for the lexical diagnostic.

This is an exploratory producer. A corpus release proves input bytes, not that
an external service indexed every file. The output retains native responses and
labels server image identities as operator supplied.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import selectors
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

BENCH_ROOT = Path(__file__).resolve().parents[1]
if str(BENCH_ROOT) not in sys.path:
    sys.path.insert(0, str(BENCH_ROOT))

import corpus_binding  # noqa: E402
import corpus_release  # noqa: E402
from evidence import _read_control_file, canonical_json  # noqa: E402

from tools.benchmark.retrieval import lexical_file_comparison as lexical  # noqa: E402
from tools.benchmark.retrieval import sourcegraph  # noqa: E402

MAX_HTTP_BYTES = 16 * 1024 * 1024
MAX_PROCESS_BYTES = 16 * 1024 * 1024
HTTP_TIMEOUT = 50


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, new_url):
        return None


def _object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _reject_constant(value: str):
    raise ValueError(f"non-finite JSON constant: {value}")


def _json(data: bytes) -> dict:
    value = json.loads(
        data.decode("utf-8"), object_pairs_hook=_object, parse_constant=_reject_constant
    )
    if not isinstance(value, dict):
        raise ValueError("expected a JSON object")
    return value


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _source_hashes() -> dict[str, str]:
    sources = {
        "producer": Path(__file__),
        "sourcegraph_adapter": Path(sourcegraph.__file__),
        "lexical_scorer": Path(lexical.__file__),
        "corpus_binding": Path(corpus_binding.__file__),
        "corpus_release": Path(corpus_release.__file__),
    }
    return {name: _sha(path.read_bytes()) for name, path in sources.items()}


def _write(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("xb") as stream:
        stream.write(data)


def _url(value: object) -> str:
    if not isinstance(value, str):
        raise ValueError("service base URL must be a string")
    parsed = urllib.parse.urlsplit(value)
    if (
        parsed.scheme not in {"http", "https"}
        or not parsed.hostname
        or parsed.username
        or parsed.password
        or parsed.query
        or parsed.fragment
        or parsed.path not in ("", "/")
    ):
        raise ValueError("service base URL must be an HTTP(S) origin without credentials")
    if parsed.scheme == "http" and parsed.hostname not in {"localhost", "127.0.0.1", "::1"}:
        raise ValueError("non-loopback services require HTTPS")
    return value.rstrip("/")


def _service(value: object, keys: set[str]) -> dict:
    if not isinstance(value, dict) or not keys <= set(value) or set(value) - keys - {"token_file"}:
        raise ValueError("service spec keys differ")
    result = dict(value, base_url=_url(value["base_url"]))
    for key in keys - {"base_url", "server_image_digest"}:
        if not isinstance(value[key], str) or not value[key]:
            raise ValueError(f"service {key} must be a nonempty string")
    digest = value["server_image_digest"]
    if (
        not isinstance(digest, str)
        or len(digest) != 64
        or any(c not in "0123456789abcdef" for c in digest)
    ):
        raise ValueError("server image digest must be lowercase sha256")
    if "token_file" in value:
        if not isinstance(value["token_file"], str):
            raise ValueError("token file path must be a string")
        token = Path(value["token_file"])
        if not token.is_absolute() or ".." in token.parts:
            raise ValueError("token file path must be canonical absolute")
    return result


def _spec(path: Path) -> dict:
    value = _json(_read_control_file(path))
    if (
        set(value)
        != {
            "schema_version",
            "corpus",
            "suite",
            "query_pack",
            "sourcegraph",
            "opengrok",
            "cs",
            "output_root",
        }
        or type(value["schema_version"]) is not int
        or value["schema_version"] != 1
    ):
        raise ValueError("live external spec requires the closed schema version 1")
    corpus_binding._selection(value["corpus"])
    value["sourcegraph"] = _service(
        value["sourcegraph"], {"base_url", "repository", "server_image_digest"}
    )
    value["opengrok"] = _service(value["opengrok"], {"base_url", "project", "server_image_digest"})
    if not isinstance(value["cs"], dict) or set(value["cs"]) != {"binary"}:
        raise ValueError("cs spec requires only binary")
    for key in ("suite", "query_pack", "output_root"):
        item = value[key]
        if not isinstance(item, str) or not Path(item).is_absolute() or ".." in Path(item).parts:
            raise ValueError(f"{key} path must be canonical absolute")
    binary = value["cs"]["binary"]
    if not isinstance(binary, str) or not Path(binary).is_absolute() or ".." in Path(binary).parts:
        raise ValueError("cs binary path must be canonical absolute")
    return value


def _auth(config: dict, scheme: str) -> dict[str, str]:
    headers = {"Accept": "application/json"}
    if "token_file" in config:
        token = _read_control_file(Path(config["token_file"])).decode("utf-8").strip()
        if not token or "\n" in token or "\r" in token:
            raise ValueError("invalid service token")
        headers["Authorization"] = scheme + " " + token
    return headers


def _http(
    config: dict, endpoint: str, params: dict, accept: str, scheme: str = "Bearer"
) -> tuple[int, str, bytes, float]:
    url = config["base_url"] + endpoint + "?" + urllib.parse.urlencode(params)
    headers = _auth(config, scheme)
    headers["Accept"] = accept
    request = urllib.request.Request(url, headers=headers)
    start = time.monotonic_ns()
    try:
        with urllib.request.build_opener(_NoRedirect).open(
            request, timeout=HTTP_TIMEOUT
        ) as response:
            status = response.status
            content_type = response.headers.get_content_type()
            raw = response.read(MAX_HTTP_BYTES + 1)
    except urllib.error.HTTPError as error:
        status = error.code
        content_type = error.headers.get_content_type()
        raw = error.read(MAX_HTTP_BYTES + 1)
    elapsed = (time.monotonic_ns() - start) / 1_000_000
    if len(raw) > MAX_HTTP_BYTES:
        raise ValueError("HTTP response exceeds 16 MiB limit")
    return status, content_type, raw, elapsed


def _paths(paths: list[str], admitted: dict[str, str], view: Path) -> list[str]:
    if len(paths) > 10 or len(paths) != len(set(paths)):
        raise ValueError("duplicate or excessive result paths")
    for path in paths:
        if not lexical._canonical_result_path(path) or path not in admitted:
            raise ValueError(f"result outside corpus view: {path!r}")
        if _sha(_read_control_file(view / path)) != admitted[path]:
            raise ValueError(f"result file changed after release validation: {path}")
    return paths


def _row(task: dict, gold: list[str], result_paths: list[str], elapsed: float, **terminal) -> dict:
    return {
        "lane": "symbol_only",
        "task_id": task["task_id"],
        "submitted_query": task["query"],
        "gold_paths": gold,
        "file_hit_at_10": bool(set(result_paths) & set(gold)),
        "elapsed_ms": elapsed,
        **terminal,
    }


def _sourcegraph(
    config: dict,
    task: dict,
    gold: list[str],
    manifest: dict,
    view: Path,
    admitted: dict[str, str],
    target: Path,
) -> dict:
    query = sourcegraph.query_expression(
        task["query"],
        config["repository"],
        manifest["repository_commit"],
        [row["path"] for row in manifest["files"]],
    )
    status, content_type, raw, elapsed = _http(
        config, "/.api/search/stream", {"q": query, "v": "V3"}, "text/event-stream", "token"
    )
    _write(target, raw)
    _write(
        target.with_suffix(".transport.json"),
        json.dumps(
            {
                "status": status,
                "content_type": content_type,
                "elapsed_ms": elapsed,
            },
            sort_keys=True,
        ).encode()
        + b"\n",
    )
    return _sourcegraph_response(
        config, task, gold, manifest, view, admitted, status, content_type, raw, elapsed
    )


def _sourcegraph_response(
    config: dict,
    task: dict,
    gold: list[str],
    manifest: dict,
    view: Path,
    admitted: dict[str, str],
    status: int,
    content_type: str,
    raw: bytes,
    elapsed: float,
) -> dict:
    query = sourcegraph.query_expression(
        task["query"],
        config["repository"],
        manifest["repository_commit"],
        [row["path"] for row in manifest["files"]],
    )
    request = {
        "capture_version": 1,
        "api_version": "V3",
        "endpoint": "/.api/search/stream",
        "query": task["query"],
        "query_sha256": task["query_sha256"],
        "request_query": query,
        "repository": config["repository"],
        "revision": manifest["repository_commit"],
        "response_sha256": _sha(raw),
        "http_status": status,
        "content_type": content_type,
        "server_image_digest": config["server_image_digest"],
    }
    binding = {
        "proof_version": 1,
        "method": "input_manifest_only",
        "repository": config["repository"],
        "revision": manifest["repository_commit"],
        "files": manifest["files"],
    }
    result = sourcegraph.validate_capture(request, raw, manifest, binding)
    paths = _paths([hit["path"] for hit in result["file_order"][:10]], admitted, view)
    return _row(
        task,
        gold,
        paths,
        elapsed,
        http_status=status,
        error=None,
        file_paths_top_10=paths,
        request_query=query,
        response_sha256=_sha(raw),
        server_image_digest=config["server_image_digest"],
    )


def _opengrok(
    config: dict, task: dict, gold: list[str], view: Path, admitted: dict[str, str], target: Path
) -> dict:
    params = {
        "full": task["query"],
        "projects": config["project"],
        "maxresults": 10,
        "start": 0,
        "sort": "relevancy",
    }
    status, content_type, raw, elapsed = _http(config, "/api/v1/search", params, "application/json")
    _write(target, raw)
    _write(
        target.with_suffix(".transport.json"),
        json.dumps(
            {
                "status": status,
                "content_type": content_type,
                "elapsed_ms": elapsed,
            },
            sort_keys=True,
        ).encode()
        + b"\n",
    )
    return _opengrok_response(
        config, task, gold, view, admitted, status, content_type, raw, elapsed
    )


def _opengrok_response(
    config: dict,
    task: dict,
    gold: list[str],
    view: Path,
    admitted: dict[str, str],
    status: int,
    content_type: str,
    raw: bytes,
    elapsed: float,
) -> dict:
    if status != 200 or content_type != "application/json":
        raise ValueError(f"OpenGrok returned HTTP {status} / {content_type}")
    body = _json(raw)
    if set(body) != {"time", "resultCount", "results", "startDocument", "endDocument"}:
        raise ValueError("OpenGrok response envelope differs")
    count = body["resultCount"]
    if (
        type(count) is not int
        or count < 0
        or type(body["time"]) is not int
        or body["time"] < 0
        or type(body["startDocument"]) is not int
        or type(body["endDocument"]) is not int
        or body["startDocument"] != 0
        or body["endDocument"] != (min(count, 10) - 1 if count else 0)
        or not isinstance(body["results"], dict)
        or len(body["results"]) != min(count, 10)
    ):
        raise ValueError("OpenGrok response is partial or malformed")
    paths = []
    for absolute, hits in body["results"].items():
        prefix = "/" + config["project"] + "/"
        if (
            not isinstance(absolute, str)
            or not absolute.startswith(prefix)
            or not isinstance(hits, list)
            or not hits
        ):
            raise ValueError("OpenGrok response has an invalid project path or empty hit")
        paths.append(absolute[len(prefix) :])
    paths = _paths(paths, admitted, view)
    return _row(
        task,
        gold,
        paths,
        elapsed,
        http_status=status,
        error=None,
        field="full",
        file_paths_top_10=paths,
        response_sha256=_sha(raw),
        server_image_digest=config["server_image_digest"],
    )


def _process(argv: list[str], timeout: int) -> tuple[int, bytes, bytes, float]:
    start = time.monotonic_ns()
    deadline = time.monotonic() + timeout
    process = subprocess.Popen(
        argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True
    )
    buffers = {process.stdout: bytearray(), process.stderr: bytearray()}
    try:
        with selectors.DefaultSelector() as selector:
            for stream in buffers:
                selector.register(stream, selectors.EVENT_READ)
            while selector.get_map():
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise ValueError("cs process timed out")
                for key, _ in selector.select(remaining):
                    chunk = os.read(key.fileobj.fileno(), 65536)
                    if not chunk:
                        selector.unregister(key.fileobj)
                        continue
                    buffers[key.fileobj].extend(chunk)
                    if len(buffers[key.fileobj]) > MAX_PROCESS_BYTES:
                        raise ValueError("cs process output exceeds 16 MiB limit")
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise ValueError("cs process timed out")
        process.wait(timeout=remaining)
    except (ValueError, subprocess.TimeoutExpired) as error:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait()
        if isinstance(error, subprocess.TimeoutExpired):
            raise ValueError("cs process timed out") from error
        raise
    finally:
        for stream in buffers:
            stream.close()
    elapsed = (time.monotonic_ns() - start) / 1_000_000
    return (
        process.returncode,
        bytes(buffers[process.stdout]),
        bytes(buffers[process.stderr]),
        elapsed,
    )


def _cs(
    binary: Path, task: dict, gold: list[str], view: Path, admitted: dict[str, str], target: Path
) -> dict:
    argv = [
        str(binary),
        "--format",
        "json",
        "--result-limit",
        "10",
        "--dir",
        str(view),
        "--hidden",
        "--no-gitignore",
        "--no-ignore",
        "--min",
        "--max-read-size-bytes",
        "10000000",
        task["query"],
    ]
    code, stdout, stderr, elapsed = _process(argv, 60)
    _write(target, stdout)
    _write(target.with_suffix(".stderr"), stderr)
    _write(
        target.with_suffix(".process.json"),
        json.dumps(
            {
                "exit_code": code,
                "elapsed_ms": elapsed,
            },
            sort_keys=True,
        ).encode()
        + b"\n",
    )
    return _cs_response(task, gold, view, admitted, code, stdout, stderr, elapsed)


def _cs_response(
    task: dict,
    gold: list[str],
    view: Path,
    admitted: dict[str, str],
    code: int,
    stdout: bytes,
    stderr: bytes,
    elapsed: float,
) -> dict:
    if code != 0 or stderr:
        raise ValueError(f"cs failed with exit {code} or nonempty stderr")
    native = json.loads(
        stdout.decode("utf-8"), object_pairs_hook=_object, parse_constant=_reject_constant
    )
    if native is None:  # cs v3.2.0 emits JSON null for an empty successful search.
        native = []
    if not isinstance(native, list) or len(native) > 10:
        raise ValueError("cs result shape differs")
    paths = []
    for hit in native:
        if not isinstance(hit, dict) or not isinstance(hit.get("location"), str):
            raise ValueError("cs hit lacks an absolute location")
        location = Path(hit["location"]).resolve(strict=True)
        paths.append(location.relative_to(view).as_posix())
    paths = _paths(paths, admitted, view)
    return _row(
        task,
        gold,
        paths,
        elapsed,
        exit_code=code,
        paths=paths,
        stdout_sha256=_sha(stdout),
        stderr_sha256=_sha(stderr),
    )


def capture(spec_path: Path) -> dict:
    spec = _spec(spec_path)
    root = Path(spec["output_root"])
    stage = root.with_name(root.name + ".staging")
    release = Path(spec["corpus"]["release_path"])
    checkout = Path(__file__).resolve().parents[3]
    input_paths = (
        spec_path,
        root,
        release,
        Path(spec["suite"]),
        Path(spec["query_pack"]),
        Path(spec["cs"]["binary"]),
    )
    if any(path.resolve().is_relative_to(checkout) for path in input_paths):
        raise ValueError("live capture inputs and output must stay outside the source checkout")
    if (
        root.exists()
        or root.is_symlink()
        or stage.exists()
        or stage.is_symlink()
        or root.resolve().is_relative_to(release.resolve())
        or release.resolve().is_relative_to(root.resolve())
    ):
        raise ValueError("live output must be fresh and disjoint from the corpus release")
    suite_raw = _read_control_file(Path(spec["suite"]))
    pack_raw = _read_control_file(Path(spec["query_pack"]))
    suite, pack = _json(suite_raw), _json(pack_raw)
    admitted = lexical._file_universe(suite, pack)
    tasks = lexical._tasks(suite, pack)
    if any(re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*", task_id) is None for task_id in tasks):
        raise ValueError("task IDs must be safe filename components")
    document = corpus_release.validate(release)
    repository = next(
        (
            row
            for row in document["repositories"]
            if row["recipe"]["name"] == spec["corpus"]["repository"]
        ),
        None,
    )
    if repository is None:
        raise ValueError("selected repository is absent from the release")
    view_name = spec["corpus"]["view"]
    view = release / "views" / spec["corpus"]["repository"] / view_name
    manifest_raw = _read_control_file(release / repository["views"][view_name]["manifest"])
    binding = corpus_binding._bind(document, manifest_raw, spec["corpus"], suite_raw, pack_raw)
    manifest = _json(manifest_raw)
    files = {row["path"]: row["file_sha256"] for row in manifest["files"]}
    if set(files) != admitted:
        raise ValueError("live capture file universe differs from selected release")
    binary = Path(spec["cs"]["binary"]).resolve(strict=True)
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise ValueError("cs binary must be an executable regular file")
    code, version, stderr, _ = _process([str(binary), "--version"], 10)
    if code != 0 or stderr or not version.strip():
        raise ValueError("cs version command failed")
    binary_sha = _sha(binary.read_bytes())
    source_hashes = _source_hashes()
    stage.mkdir(parents=True)
    _write(stage / "spec.json", _read_control_file(spec_path))
    _write(stage / "binding.json", json.dumps(binding, sort_keys=True).encode() + b"\n")
    _write(stage / "suite.json", suite_raw)
    _write(stage / "query-pack.json", pack_raw)
    _write(stage / "manifest.json", manifest_raw)
    rows = {name: [] for name in ("sourcegraph", "opengrok", "cs")}
    for task in pack["tasks"]:
        task_id = task["task_id"]
        gold = tasks[task_id][1]
        rows["sourcegraph"].append(
            _sourcegraph(
                spec["sourcegraph"],
                task,
                gold,
                manifest,
                view,
                files,
                stage / "sourcegraph" / f"{task_id}.stream",
            )
        )
        rows["opengrok"].append(
            _opengrok(
                spec["opengrok"], task, gold, view, files, stage / "opengrok" / f"{task_id}.json"
            )
        )
        rows["cs"].append(_cs(binary, task, gold, view, files, stage / "cs" / f"{task_id}.json"))
    for name, data in rows.items():
        destination = stage / f"{name}_rows.jsonl"
        _write(
            destination, b"".join(json.dumps(row, sort_keys=True).encode() + b"\n" for row in data)
        )
        lexical.product_result(name, destination, tasks, admitted)
    if (
        corpus_release.validate(release) != document
        or _read_control_file(spec_path) != _read_control_file(stage / "spec.json")
        or _read_control_file(Path(spec["suite"])) != suite_raw
        or _read_control_file(Path(spec["query_pack"])) != pack_raw
        or _sha(binary.read_bytes()) != binary_sha
        or _source_hashes() != source_hashes
    ):
        raise ValueError(
            "release, live spec, suite, pack, cs binary or producer source changed during capture"
        )
    summary = {
        "schema_version": 1,
        "status": "diagnostic_unqualified",
        "release_digest": document["digest"],
        "binding": binding,
        "tasks": len(tasks),
        "indexed_universe_attested": False,
        "producer_sources_sha256": source_hashes,
        "python_executable_sha256": _sha(Path(sys.executable).resolve().read_bytes()),
        "python_version": sys.version.split()[0],
        "server_image_digests_operator_supplied": {
            name: spec[name]["server_image_digest"] for name in ("sourcegraph", "opengrok")
        },
        "cs_binary_sha256": binary_sha,
        "cs_version": version.decode().strip(),
        "rows_sha256": {name: _sha((stage / f"{name}_rows.jsonl").read_bytes()) for name in rows},
        "raw_capture_sha256": {
            path.relative_to(stage).as_posix(): _sha(path.read_bytes())
            for name in rows
            for path in sorted((stage / name).iterdir())
        },
        "exclusions": [
            "backend_indexed_universe_attestation",
            "independent_gold",
            "qualified_speed",
        ],
    }
    _write(stage / "capture.json", json.dumps(summary, sort_keys=True, indent=2).encode() + b"\n")
    if root.exists():
        raise ValueError("live output appeared during capture")
    stage.rename(root)
    return summary


def verify(root: Path) -> dict:
    """Re-derive every external row from retained native bytes; no live searches."""
    inventory = corpus_release.regular_tree(root)
    spec = _spec(root / "spec.json")
    summary = _json(_read_control_file(root / "capture.json"))
    fields = {
        "schema_version",
        "status",
        "release_digest",
        "binding",
        "tasks",
        "indexed_universe_attested",
        "producer_sources_sha256",
        "python_executable_sha256",
        "python_version",
        "server_image_digests_operator_supplied",
        "cs_binary_sha256",
        "cs_version",
        "rows_sha256",
        "raw_capture_sha256",
        "exclusions",
    }
    if (
        set(summary) != fields
        or type(summary.get("schema_version")) is not int
        or summary["schema_version"] != 1
        or summary.get("status") != "diagnostic_unqualified"
        or summary.get("indexed_universe_attested") is not False
        or type(summary.get("tasks")) is not int
        or summary["tasks"] <= 0
        or summary.get("producer_sources_sha256") != _source_hashes()
        or summary.get("python_executable_sha256")
        != _sha(Path(sys.executable).resolve().read_bytes())
        or summary.get("python_version") != sys.version.split()[0]
        or summary.get("server_image_digests_operator_supplied")
        != {name: spec[name]["server_image_digest"] for name in ("sourcegraph", "opengrok")}
        or summary.get("cs_binary_sha256")
        != _sha(Path(spec["cs"]["binary"]).resolve(strict=True).read_bytes())
        or not isinstance(summary.get("rows_sha256"), dict)
        or set(summary["rows_sha256"]) != set(lexical.PRODUCTS)
        or summary.get("exclusions")
        != ["backend_indexed_universe_attestation", "independent_gold", "qualified_speed"]
    ):
        raise ValueError("unsupported capture metadata, claim or runtime identity")
    code, version, stderr, _ = _process([spec["cs"]["binary"], "--version"], 10)
    if code != 0 or stderr or summary.get("cs_version") != version.decode().strip():
        raise ValueError("unsupported capture metadata: cs version identity differs")
    suite_raw = _read_control_file(root / "suite.json")
    pack_raw = _read_control_file(root / "query-pack.json")
    suite, pack = _json(suite_raw), _json(pack_raw)
    admitted = lexical._file_universe(suite, pack)
    tasks = lexical._tasks(suite, pack)
    if any(re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*", task_id) is None for task_id in tasks):
        raise ValueError("task IDs must be safe filename components")
    release = Path(spec["corpus"]["release_path"])
    document = corpus_release.validate(release)
    repository = next(
        row
        for row in document["repositories"]
        if row["recipe"]["name"] == spec["corpus"]["repository"]
    )
    manifest_raw = _read_control_file(root / "manifest.json")
    view_name = spec["corpus"]["view"]
    if manifest_raw != _read_control_file(release / repository["views"][view_name]["manifest"]):
        raise ValueError("retained manifest differs from release")
    binding = corpus_binding._bind(document, manifest_raw, spec["corpus"], suite_raw, pack_raw)
    if (
        canonical_json(binding) != canonical_json(_json(_read_control_file(root / "binding.json")))
        or canonical_json(binding) != canonical_json(summary.get("binding"))
        or summary.get("tasks") != len(tasks)
        or summary.get("release_digest") != document["digest"]
    ):
        raise ValueError("external capture binding differs")
    manifest = _json(manifest_raw)
    files = {row["path"]: row["file_sha256"] for row in manifest["files"]}
    view = release / "views" / spec["corpus"]["repository"] / view_name
    expected_raw = set()
    for task in pack["tasks"]:
        task_id = task["task_id"]
        expected_raw.update(
            {
                f"sourcegraph/{task_id}.stream",
                f"sourcegraph/{task_id}.transport.json",
                f"opengrok/{task_id}.json",
                f"opengrok/{task_id}.transport.json",
                f"cs/{task_id}.json",
                f"cs/{task_id}.stderr",
                f"cs/{task_id}.process.json",
            }
        )
    fixed = {
        "spec.json",
        "capture.json",
        "binding.json",
        "suite.json",
        "query-pack.json",
        "manifest.json",
        *(f"{name}_rows.jsonl" for name in lexical.PRODUCTS),
    }
    if (
        set(inventory) != fixed | expected_raw
        or set(summary.get("raw_capture_sha256", {})) != expected_raw
    ):
        raise ValueError("external capture raw inventory differs")
    for name in expected_raw:
        if _sha(_read_control_file(root / name)) != summary["raw_capture_sha256"][name]:
            raise ValueError("external native bytes differ from capture")
    for name in lexical.PRODUCTS:
        row_path = root / f"{name}_rows.jsonl"
        if _sha(_read_control_file(row_path)) != summary.get("rows_sha256", {}).get(name):
            raise ValueError("external rows differ from capture")
        lexical.product_result(name, row_path, tasks, admitted)
        rows = [_json(line) for line in _read_control_file(row_path).splitlines()]
        if [row["task_id"] for row in rows] != [task["task_id"] for task in pack["tasks"]]:
            raise ValueError("external capture row order differs")
        for task, row in zip(pack["tasks"], rows, strict=True):
            task_id, gold = task["task_id"], tasks[task["task_id"]][1]
            if name == "cs":
                terminal = _json(_read_control_file(root / name / f"{task_id}.process.json"))
                if (
                    set(terminal) != {"exit_code", "elapsed_ms"}
                    or type(terminal["exit_code"]) is not int
                ):
                    raise ValueError("cs terminal metadata differs")
                derived = _cs_response(
                    task,
                    gold,
                    view,
                    files,
                    terminal["exit_code"],
                    _read_control_file(root / name / f"{task_id}.json"),
                    _read_control_file(root / name / f"{task_id}.stderr"),
                    terminal["elapsed_ms"],
                )
            else:
                terminal = _json(_read_control_file(root / name / f"{task_id}.transport.json"))
                if (
                    set(terminal) != {"status", "content_type", "elapsed_ms"}
                    or type(terminal["status"]) is not int
                ):
                    raise ValueError("HTTP terminal metadata differs")
                raw = _read_control_file(
                    root / name / f"{task_id}.{'stream' if name == 'sourcegraph' else 'json'}"
                )
                if name == "sourcegraph":
                    derived = _sourcegraph_response(
                        spec[name],
                        task,
                        gold,
                        manifest,
                        view,
                        files,
                        terminal["status"],
                        terminal["content_type"],
                        raw,
                        terminal["elapsed_ms"],
                    )
                else:
                    derived = _opengrok_response(
                        spec[name],
                        task,
                        gold,
                        view,
                        files,
                        terminal["status"],
                        terminal["content_type"],
                        raw,
                        terminal["elapsed_ms"],
                    )
            if canonical_json(row) != canonical_json(derived):
                raise ValueError("external row disagrees with retained native response")
    return summary


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--spec", type=Path)
    mode.add_argument("--verify", type=Path)
    args = parser.parse_args()
    try:
        print(
            json.dumps(
                verify(args.verify) if args.verify else capture(args.spec), sort_keys=True, indent=2
            )
        )
    except (ValueError, OSError, subprocess.SubprocessError, corpus_release.EvidenceError) as error:
        parser.error(str(error))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
