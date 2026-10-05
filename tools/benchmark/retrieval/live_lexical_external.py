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
import math
import os
import re
import selectors
import signal
import stat
import subprocess
import sys
import tarfile
import time
import urllib.error
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET
import zipfile
from contextlib import ExitStack
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

SOURCE_ROOT = Path(__file__).resolve().parents[3]
if str(SOURCE_ROOT) not in sys.path:
    sys.path.insert(0, str(SOURCE_ROOT))
BENCH_ROOT = SOURCE_ROOT / "tools" / "benchmark"
if str(BENCH_ROOT) not in sys.path:
    sys.path.insert(0, str(BENCH_ROOT))

import corpus_binding  # noqa: E402
import corpus_release  # noqa: E402
from evidence import RawFile, _read_control_file, canonical_json, file_digest  # noqa: E402

from tools.benchmark.retrieval import lexical_file_comparison as lexical  # noqa: E402
from tools.benchmark.retrieval import (  # noqa: E402
    opengrok_index_scope,
    opengrok_query_witness,
    query_plan,
    sourcegraph,
    sourcegraph_index_scope,
)

MAX_HTTP_BYTES = 16 * 1024 * 1024
MAX_PROCESS_BYTES = 16 * 1024 * 1024
MAX_BACKEND_INDEX_FILES = 4096
MAX_INDEX_BYTES = 512 * 1024 * 1024
MAX_INDEXED_VIEW_SECONDS = 900
MAX_INDEXED_VIEW_BATCH_FILES = 1024
MAX_INDEXED_VIEW_BATCH_BYTES = 512 * 1024 * 1024
HTTP_TIMEOUT = 50
MAX_SOURCEGRAPH_REQUEST_TARGET_BYTES = 8 * 1024
CS_FUZZY_CAPABILITY = "cs_fuzzy_osa1_file"
CS_VERIFIED_VERSION = "cs version 3.2.0"
PRODUCTS = ("sourcegraph", "opengrok", "cs")
COMPLETED_BOUNDARY = "request_construction_to_normalized_response"
COMPLETED_CLOCK = "same_process_monotonic_ns"


@dataclass(frozen=True)
class BoundRelease:
    """Reuse one full Git replay while rehashing every release file per cell."""

    root: Path
    document: dict
    files: dict[str, str]
    owner_sha256: str

    @classmethod
    def begin(cls, root: Path) -> BoundRelease:
        root = root.resolve(strict=True)
        files = {name: _sha_file(root / name) for name in sorted(corpus_release.regular_tree(root))}
        owner_sha256 = _sha_file(Path(corpus_release.__file__))
        document = corpus_release.validate(root)
        if files != {
            name: _sha_file(root / name) for name in sorted(corpus_release.regular_tree(root))
        } or owner_sha256 != _sha_file(Path(corpus_release.__file__)):
            raise ValueError("batch-bound release or validator changed during full validation")
        return cls(root, document, files, owner_sha256)

    def recheck(self, root: Path) -> dict:
        if (
            root.resolve(strict=True) != self.root
            or _sha_file(Path(corpus_release.__file__)) != self.owner_sha256
            or {name: _sha_file(root / name) for name in sorted(corpus_release.regular_tree(root))}
            != self.files
        ):
            raise ValueError("batch-bound release root, owner or complete file bytes changed")
        return self.document


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


def _sha_file(path: Path) -> str:
    return file_digest(path)[0].removeprefix("sha256:")


def _source_hashes() -> dict[str, str]:
    sources = {
        "producer": Path(__file__),
        "sourcegraph_adapter": Path(sourcegraph.__file__),
        "sourcegraph_index_scope": Path(sourcegraph_index_scope.__file__),
        "opengrok_index_scope": Path(opengrok_index_scope.__file__),
        "opengrok_native_reader": opengrok_index_scope.READER,
        "opengrok_query_witness": Path(opengrok_query_witness.__file__),
        "lexical_scorer": Path(lexical.__file__),
        "corpus_binding": Path(corpus_binding.__file__),
        "corpus_release": Path(corpus_release.__file__),
        "query_planner": Path(query_plan.__file__),
    }
    return {name: _sha_file(path) for name, path in sources.items()}


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


def _service(value: object, keys: set[str], optional: set[str] = frozenset()) -> dict:
    if (
        not isinstance(value, dict)
        or not keys <= set(value)
        or set(value) - keys - optional - {"token_file"}
    ):
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
    if "projection_git_root" in value:
        projection = value["projection_git_root"]
        if (
            not isinstance(projection, str)
            or not Path(projection).is_absolute()
            or ".." in Path(projection).parts
        ):
            raise ValueError("projection Git root must be canonical absolute")
    if "indexed_scope_receipt" in value:
        sourcegraph_index_scope._absolute(value["indexed_scope_receipt"])
        if not {"backend_snapshot", "projection_git_root"} <= set(value):
            raise ValueError("indexed scope receipt requires backend and projection binding")
    if "backend_snapshot" in value:
        snapshot = value["backend_snapshot"]
        if not isinstance(snapshot, dict) or set(snapshot) != {
            "root",
            "container_id",
            "mount_destination",
            "container_port",
        }:
            raise ValueError("backend snapshot spec keys differ")
        root = snapshot["root"]
        mount = snapshot["mount_destination"]
        container_id = snapshot["container_id"]
        port = snapshot["container_port"]
        if (
            not isinstance(root, str)
            or not Path(root).is_absolute()
            or ".." in Path(root).parts
            or not isinstance(mount, str)
            or not Path(mount).is_absolute()
            or ".." in Path(mount).parts
            or not isinstance(container_id, str)
            or re.fullmatch(r"[0-9a-f]{64}", container_id) is None
            or not isinstance(port, str)
            or re.fullmatch(r"[1-9][0-9]{0,4}/tcp", port) is None
            or int(port.split("/")[0]) > 65535
        ):
            raise ValueError("backend snapshot requires canonical paths, container ID and TCP port")
        if urllib.parse.urlsplit(result["base_url"]).hostname not in {
            "localhost",
            "127.0.0.1",
            "::1",
        }:
            raise ValueError("backend snapshot requires a loopback service URL")
    return result


def _spec(path: Path) -> dict:
    value = _json(_read_control_file(path))
    common = {"schema_version", "corpus", "suite", "query_pack", "output_root"}
    version = value.get("schema_version")
    if type(version) is not int or version not in (1, 2):
        raise ValueError("live external spec requires schema version 1 or 2")
    if version == 1:
        products = PRODUCTS
        expected = common | set(PRODUCTS)
    else:
        selected = value.get("products")
        if (
            not isinstance(selected, list)
            or not selected
            or any(type(name) is not str for name in selected)
            or selected != [name for name in PRODUCTS if name in selected]
        ):
            raise ValueError("live external products must be a nonempty canonical subset")
        products = tuple(selected)
        expected = common | {"products"} | set(products)
    if set(value) != expected:
        raise ValueError("live external spec keys differ from selected products")
    corpus_binding._selection(value["corpus"])
    if "sourcegraph" in products:
        value["sourcegraph"] = _service(
            value["sourcegraph"],
            {"base_url", "repository", "server_image_digest"},
            {"backend_snapshot", "projection_git_root", "indexed_scope_receipt"},
        )
    if "opengrok" in products:
        value["opengrok"] = _service(
            value["opengrok"],
            {"base_url", "project", "server_image_digest"},
            {"indexed_view_probe", "backend_snapshot", "native_index_reader", "readonly_service",
             "query_reader_witness"},
        )
        if value["opengrok"].get("indexed_view_probe") not in (None, "full"):
            raise ValueError("OpenGrok indexed view probe must be full or absent")
        if "native_index_reader" in value["opengrok"]:
            config = value["opengrok"]
            if "backend_snapshot" not in config or config.get("indexed_view_probe") != "full":
                raise ValueError(
                    "OpenGrok native reader requires readonly backend and full view probe"
                )
            opengrok_index_scope.reader_identity(config["native_index_reader"])
        if "readonly_service" in value["opengrok"]:
            config = value["opengrok"]
            if "native_index_reader" not in config or "token_file" not in config:
                raise ValueError("OpenGrok readonly service requires native reader and API token")
            service = config["readonly_service"]
            if type(service) is not dict or set(service) != {
                "webapps_root",
                "etc_root",
                "source_root",
                "source_war",
                "network",
                "snapshot_receipt",
            }:
                raise ValueError("OpenGrok readonly service spec keys differ")
            for name in ("webapps_root", "etc_root", "source_root"):
                if type(service[name]) is not str:
                    raise ValueError("OpenGrok readonly service host mount differs")
                path = Path(service[name])
                if not path.is_absolute() or path.resolve(strict=True) != path or not path.is_dir():
                    raise ValueError("OpenGrok readonly service host mount differs")
            if type(service["source_war"]) is not str:
                raise ValueError("OpenGrok readonly service source WAR path differs")
            war = Path(service["source_war"])
            if not war.is_absolute() or war.resolve(strict=True) != war or not war.is_file():
                raise ValueError("OpenGrok readonly service source WAR differs")
            if type(service["snapshot_receipt"]) is not str:
                raise ValueError("OpenGrok readonly snapshot receipt path differs")
            receipt = Path(service["snapshot_receipt"])
            if (
                not receipt.is_absolute()
                or receipt.resolve(strict=True) != receipt
                or not receipt.is_file()
            ):
                raise ValueError("OpenGrok readonly snapshot receipt differs")
            if (
                type(service["network"]) is not str
                or re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,127}", service["network"]) is None
            ):
                raise ValueError("OpenGrok readonly service network differs")
        if "query_reader_witness" in value["opengrok"]:
            config = value["opengrok"]
            fixture = config["query_reader_witness"]
            if (
                "readonly_service" not in config
                or type(fixture) is not dict
                or set(fixture)
                != {"contract", "original_source", "patched_source", "source_patch",
                    "instrumented_war"}
                or fixture["contract"] != opengrok_query_witness.CONTRACT
                or config["server_image_digest"]
                != opengrok_query_witness.COMPILER_IMAGE_SHA256
            ):
                raise ValueError("OpenGrok query reader fixture spec differs")
            for field, expected_sha in (
                ("original_source", opengrok_query_witness.ORIGINAL_SOURCE_SHA256),
                ("patched_source", opengrok_query_witness.PATCHED_SOURCE_SHA256),
                ("source_patch", opengrok_query_witness.SOURCE_PATCH_SHA256),
            ):
                if type(fixture[field]) is not str:
                    raise ValueError("OpenGrok query reader fixture source differs")
                path = Path(fixture[field])
                if (
                    not path.is_absolute()
                    or path.resolve(strict=True) != path
                    or not path.is_file()
                    or _sha_file(path) != expected_sha
                ):
                    raise ValueError("OpenGrok query reader fixture source differs")
            if type(fixture["instrumented_war"]) is not str:
                raise ValueError("OpenGrok instrumented WAR differs")
            war = Path(fixture["instrumented_war"])
            if (
                not war.is_absolute()
                or war.resolve(strict=True) != war
                or not war.is_file()
            ):
                raise ValueError("OpenGrok instrumented WAR differs")
    if "cs" in products and (not isinstance(value["cs"], dict) or set(value["cs"]) != {"binary"}):
        raise ValueError("cs spec requires only binary")
    for key in ("suite", "query_pack", "output_root"):
        item = value[key]
        if not isinstance(item, str) or not Path(item).is_absolute() or ".." in Path(item).parts:
            raise ValueError(f"{key} path must be canonical absolute")
    if "cs" in products:
        binary = value["cs"]["binary"]
        if (
            not isinstance(binary, str)
            or not Path(binary).is_absolute()
            or ".." in Path(binary).parts
        ):
            raise ValueError("cs binary path must be canonical absolute")
    return value


def _selected_products(spec: dict) -> tuple[str, ...]:
    return PRODUCTS if spec["schema_version"] == 1 else tuple(spec["products"])


def _projection_binding(config: dict, manifest: dict) -> dict | None:
    """Bind an exact-file Git projection to the original source manifest."""
    if "projection_git_root" not in config:
        return None
    root = Path(config["projection_git_root"]).resolve(strict=True)
    if not root.is_dir():
        raise ValueError("projection Git root is not a directory")
    try:
        top = subprocess.check_output(
            ["git", "-C", str(root), "rev-parse", "--show-toplevel"], timeout=30, text=True
        ).strip()
        revision = subprocess.check_output(
            ["git", "-C", str(root), "rev-parse", "HEAD"], timeout=30, text=True
        ).strip()
        dirty = subprocess.check_output(
            ["git", "-C", str(root), "status", "--porcelain"], timeout=30
        )
        tracked = subprocess.check_output(
            ["git", "-C", str(root), "ls-files", "-z"], timeout=30
        ).split(b"\0")[:-1]
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as exc:
        raise ValueError("projection Git repository inspection failed") from exc
    if top != str(root) or dirty or re.fullmatch(r"[0-9a-f]{40}", revision) is None:
        raise ValueError("projection Git root, commit or clean state differs")
    expected = {row["path"]: row["file_sha256"] for row in manifest["files"]}
    if len(expected) != len(manifest["files"]):
        raise ValueError("projection source manifest has duplicate paths")
    try:
        paths = [raw.decode("utf-8", "strict") for raw in tracked]
    except UnicodeDecodeError as exc:
        raise ValueError("projection Git path is not UTF-8") from exc
    if len(paths) != len(set(paths)) or set(paths) != set(expected):
        raise ValueError("projection tracked paths differ from the source manifest")
    for path in paths:
        file = root / path
        if (
            not lexical._canonical_result_path(path)
            or not file.is_file()
            or file.is_symlink()
            or _sha_file(file) != expected[path]
        ):
            raise ValueError(f"projection file differs from source manifest: {path}")
    # `git status` can hide skip-worktree or assume-unchanged content. Sourcegraph
    # indexes the requested commit, so compare that commit's blobs directly.
    process = subprocess.Popen(
        ["git", "-C", str(root), "archive", "--format=tar", "HEAD"],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    committed: set[str] = set()
    try:
        assert process.stdout is not None
        with tarfile.open(fileobj=process.stdout, mode="r|") as archive:
            for member in archive:
                if member.isdir():
                    continue
                if not member.isfile() or member.name not in expected or member.name in committed:
                    raise ValueError("projection commit tree has an unexpected file")
                if member.size > MAX_INDEX_BYTES:
                    raise ValueError("projection commit blob exceeds file bound")
                stream = archive.extractfile(member)
                if stream is None:
                    raise ValueError("projection commit blob cannot be read")
                digest = hashlib.sha256()
                with stream:
                    while block := stream.read(1024 * 1024):
                        digest.update(block)
                if digest.hexdigest() != expected[member.name]:
                    raise ValueError(f"projection commit blob differs: {member.name}")
                committed.add(member.name)
        if process.wait(timeout=30) != 0 or committed != set(expected):
            raise ValueError("projection commit tree differs from source manifest")
    finally:
        if process.stdout is not None:
            process.stdout.close()
        if process.poll() is None:
            process.kill()
            process.wait()
    return {
        "source_revision": manifest["repository_commit"],
        "projection_revision": revision,
        "files_sha256": _sha(canonical_json(manifest["files"]).encode()),
        "file_count": len(paths),
    }


def _sourcegraph_revision(config: dict, manifest: dict) -> str:
    if "projection_git_root" in config:
        revision = config.get("projection_revision")
        if not isinstance(revision, str):
            raise ValueError("Sourcegraph projection was not preflighted")
        return revision
    return manifest["repository_commit"]


def _auth(config: dict, scheme: str) -> dict[str, str]:
    headers = {"Accept": "application/json"}
    if "token_file" in config:
        token = _read_control_file(Path(config["token_file"])).decode("utf-8").strip()
        if not token or "\n" in token or "\r" in token:
            raise ValueError("invalid service token")
        headers["Authorization"] = scheme + " " + token
    return headers


def _service_opener(config: dict):
    """Reach the inspected read-only container directly, without ambient proxy routing."""
    handlers = [urllib.request.ProxyHandler({})] if "readonly_service" in config else []
    return urllib.request.build_opener(*handlers, _NoRedirect)


def _http(
    config: dict, endpoint: str, params: dict, accept: str, scheme: str = "Bearer"
) -> tuple[int, str, bytes, float]:
    url = config["base_url"] + endpoint + "?" + urllib.parse.urlencode(params)
    headers = _auth(config, scheme)
    headers["Accept"] = accept
    request = urllib.request.Request(url, headers=headers, method="GET")
    start = time.monotonic_ns()
    try:
        with _service_opener(config).open(
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


def _opengrok_witness_http(
    config: dict, endpoint: str, params: dict, nonce: str
) -> tuple[int, str, bytes, float, list[str] | None]:
    """Capture duplicate-aware raw witness headers from this search response."""
    url = config["base_url"] + endpoint + "?" + urllib.parse.urlencode(params)
    headers = _auth(config, "Bearer")
    headers["Accept"] = "application/json"
    headers[opengrok_query_witness.REQUEST_HEADER] = nonce
    request = urllib.request.Request(url, headers=headers, method="GET")
    start = time.monotonic_ns()
    try:
        with _service_opener(config).open(
            request, timeout=HTTP_TIMEOUT
        ) as response:
            status = response.status
            content_type = response.headers.get_content_type()
            witness = response.headers.get_all(opengrok_query_witness.HEADER)
            raw = response.read(MAX_HTTP_BYTES + 1)
    except urllib.error.HTTPError as error:
        status = error.code
        content_type = error.headers.get_content_type()
        witness = error.headers.get_all(opengrok_query_witness.HEADER)
        raw = error.read(MAX_HTTP_BYTES + 1)
    elapsed = (time.monotonic_ns() - start) / 1_000_000
    if len(raw) > MAX_HTTP_BYTES:
        raise ValueError("HTTP response exceeds 16 MiB limit")
    return status, content_type, raw, elapsed, witness


def _paths(paths: list[str], admitted: dict[str, str], view: Path) -> list[str]:
    if len(paths) > 10 or len(paths) != len(set(paths)):
        raise ValueError("duplicate or excessive result paths")
    for path in paths:
        if not lexical._canonical_result_path(path) or path not in admitted:
            raise ValueError(f"result outside corpus view: {path!r}")
        if _sha_file(view / path) != admitted[path]:
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


def _completed_native(paths: list[str], start_ns: int) -> dict:
    """Close after native result normalization; hash and benchmark work follow."""
    required = canonical_json({"status": "success", "file_paths_top_10": paths}).encode("utf-8")
    end_ns = time.monotonic_ns()
    if type(start_ns) is not int or type(end_ns) is not int or end_ns < start_ns:
        raise ValueError("completed response clock moved backwards")
    return {
        "boundary": COMPLETED_BOUNDARY,
        "clock": COMPLETED_CLOCK,
        "duration_ns": end_ns - start_ns,
        "output_bytes": len(required),
        "output_sha256": _sha(required),
    }


def _validate_completed_row(row: dict, paths: list[str]) -> None:
    timing = row.get("completed_response")
    if not isinstance(timing, dict) or set(timing) != {
        "boundary",
        "clock",
        "duration_ns",
        "output_bytes",
        "output_sha256",
    }:
        raise ValueError("completed response metadata is absent or malformed")
    encoded = canonical_json({"status": "success", "file_paths_top_10": paths}).encode("utf-8")
    if (
        type(row.get("elapsed_ms")) not in (int, float)
        or not math.isfinite(row["elapsed_ms"])
        or row["elapsed_ms"] < 0
        or timing["boundary"] != COMPLETED_BOUNDARY
        or timing["clock"] != COMPLETED_CLOCK
        or type(timing["duration_ns"]) is not int
        or timing["duration_ns"] < 0
        or type(timing["output_bytes"]) is not int
        or timing["output_bytes"] != len(encoded)
        or timing["output_sha256"] != _sha(encoded)
    ):
        raise ValueError("completed response clock or normalized output differs")


def _sourcegraph(
    config: dict,
    task: dict,
    gold: list[str],
    manifest: dict,
    view: Path,
    admitted: dict[str, str],
    target: Path,
) -> dict:
    capability = lexical.sourcegraph_capability(task["query"])
    if capability["status"] == "unsupported":
        row = _unsupported_sourcegraph(task, gold)
        _write(
            target.with_suffix(".capability.json"), json.dumps(row, sort_keys=True).encode() + b"\n"
        )
        return row
    start_ns = time.monotonic_ns()
    extensions = sourcegraph.file_extensions([row["path"] for row in manifest["files"]])
    query = sourcegraph.query_expression(
        task["query"],
        config["repository"],
        _sourcegraph_revision(config, manifest),
        file_extensions_filter=extensions,
    )
    status, content_type, raw, elapsed = _http(
        config, "/.api/search/stream", {"q": query, "v": "V3"}, "text/event-stream", "token"
    )
    if status != 200 or content_type != "text/event-stream":
        raise ValueError(f"Sourcegraph returned HTTP {status} / {content_type}")
    _, _, native_files, _, _ = sourcegraph.normalize_stream(
        {
            "repository": config["repository"],
            "revision": _sourcegraph_revision(config, manifest),
            "file_filter_extensions": extensions,
        },
        raw,
        admitted,
        3 if "projection_git_root" in config else 2,
        evidence_hashes=False,
    )
    native_paths = [row["path"] for row in native_files[:10]]
    completed = _completed_native(native_paths, start_ns)
    row = _sourcegraph_response(
        config, task, gold, manifest, view, admitted, status, content_type, raw, elapsed
    )
    row["completed_response"] = completed
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
    return row


def _unsupported_sourcegraph(task: dict, gold: list[str]) -> dict:
    capability = lexical.sourcegraph_capability(task["query"])
    if capability["status"] != "unsupported":
        raise ValueError("supported Sourcegraph query cannot be relabeled unsupported")
    return {
        "lane": "symbol_only",
        "task_id": task["task_id"],
        "submitted_query": task["query"],
        "gold_paths": gold,
        "status": "unsupported",
        "capability_reason": capability["reason"],
    }


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
    extensions = sourcegraph.file_extensions([row["path"] for row in manifest["files"]])
    query = sourcegraph.query_expression(
        task["query"],
        config["repository"],
        _sourcegraph_revision(config, manifest),
        file_extensions_filter=extensions,
    )
    request = {
        "capture_version": 3 if "projection_git_root" in config else 2,
        "api_version": "V3",
        "endpoint": "/.api/search/stream",
        "query": task["query"],
        "query_sha256": task["query_sha256"],
        "file_filter_extensions": extensions,
        "request_query": query,
        "repository": config["repository"],
        "revision": _sourcegraph_revision(config, manifest),
        "response_sha256": _sha(raw),
        "http_status": status,
        "content_type": content_type,
        "server_image_digest": config["server_image_digest"],
    }
    if "projection_git_root" in config:
        request["source_revision"] = manifest["repository_commit"]
    binding = {
        "proof_version": 1,
        "method": "input_manifest_postfiltered",
        "repository": config["repository"],
        "revision": _sourcegraph_revision(config, manifest),
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
        out_of_manifest_match_count=result["out_of_manifest_match_count"],
        response_sha256=_sha(raw),
        server_image_digest=config["server_image_digest"],
    )


def _preflight_sourcegraph_request_targets(
    config: dict, tasks: list[dict], manifest: dict
) -> tuple[list[str], int]:
    extensions = sourcegraph.file_extensions([row["path"] for row in manifest["files"]])
    max_target_bytes = 0
    for task in tasks:
        if lexical.sourcegraph_capability(task["query"])["status"] == "unsupported":
            continue
        query = sourcegraph.query_expression(
            task["query"],
            config["repository"],
            _sourcegraph_revision(config, manifest),
            file_extensions_filter=extensions,
        )
        target = "/.api/search/stream?" + urllib.parse.urlencode({"q": query, "v": "V3"})
        target_bytes = len(target.encode("ascii"))
        if target_bytes > MAX_SOURCEGRAPH_REQUEST_TARGET_BYTES:
            raise ValueError("Sourcegraph request target exceeds the 8 KiB preflight limit")
        max_target_bytes = max(max_target_bytes, target_bytes)
    return extensions, max_target_bytes


def _opengrok_query(query: str, *, literal_query: bool = False) -> str:
    """Keep declared natural-language input out of Lucene query syntax."""
    if not literal_query:
        return query
    escaped = re.sub(r'([+\-!(){}\[\]^"~*?:\\|&/])', r"\\\1", query)
    return re.sub(r"(?<!\S)(?:AND|OR|NOT)(?!\S)", lambda m: '"' + m[0] + '"', escaped)


def _opengrok(
    config: dict,
    task: dict,
    gold: list[str],
    view: Path,
    admitted: dict[str, str],
    target: Path,
    *,
    literal_query: bool = False,
    native_commit: dict | None = None,
    output_root: str | None = None,
) -> dict:
    start_ns = time.monotonic_ns()
    params = {
        "full": _opengrok_query(task["query"], literal_query=literal_query),
        "projects": config["project"],
        "maxresults": 10,
        "start": 0,
        "sort": "relevancy",
    }
    query_witness = "query_reader_witness" in config
    nonce = None
    witness_headers = None
    if query_witness:
        if native_commit is None or output_root is None:
            raise ValueError("OpenGrok query reader requires native commit and capture root")
        nonce = opengrok_query_witness.request_nonce(output_root, config, task)
        status, content_type, raw, elapsed, witness_headers = _opengrok_witness_http(
            config, "/api/v1/search", params, nonce
        )
        opengrok_query_witness.verify_header(
            witness_headers,
            nonce=nonce,
            project=config["project"],
            native_commits={config["project"]: native_commit},
        )
    else:
        status, content_type, raw, elapsed = _http(
            config, "/api/v1/search", params, "application/json"
        )
    paths = _opengrok_native_paths(config, status, content_type, raw)
    completed = _completed_native(paths, start_ns)
    row = _opengrok_response(
        config,
        task,
        gold,
        view,
        admitted,
        status,
        content_type,
        raw,
        elapsed,
        literal_query=literal_query,
    )
    row["completed_response"] = completed
    _write(target, raw)
    _write(
        target.with_suffix(".transport.json"),
        json.dumps(
            {
                "status": status,
                "content_type": content_type,
                "elapsed_ms": elapsed,
                **(
                    {"request_nonce": nonce, "reader_witness_headers": witness_headers}
                    if query_witness
                    else {}
                ),
            },
            sort_keys=True,
        ).encode()
        + b"\n",
    )
    return row


def _opengrok_native_paths(config: dict, status: int, content_type: str, raw: bytes) -> list[str]:
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
        # Benchmark admission requires a source line and location even though
        # OpenAPI does not mark SearchHit fields required. Captured OpenGrok
        # responses also use null for an absent optional tag.
        for hit in hits:
            if (
                not isinstance(hit, dict)
                or not {"line", "lineNumber"} <= set(hit)
                or set(hit) - {"line", "lineNumber", "tag"}
                or not isinstance(hit["line"], str)
                or not isinstance(hit["lineNumber"], str)
                or re.fullmatch(r"[1-9][0-9]*", hit["lineNumber"]) is None
                or ("tag" in hit and hit["tag"] is not None and not isinstance(hit["tag"], str))
            ):
                raise ValueError("OpenGrok response has a malformed SearchHit")
        paths.append(absolute[len(prefix) :])
    if len(paths) != len(set(paths)) or any(
        not lexical._canonical_result_path(path) for path in paths
    ):
        raise ValueError("OpenGrok native paths are duplicate or noncanonical")
    return paths


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
    *,
    literal_query: bool = False,
) -> dict:
    paths = _paths(_opengrok_native_paths(config, status, content_type, raw), admitted, view)
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
        **(
            {
                "request_query": _opengrok_query(task["query"], literal_query=True),
                "request_mode": query_plan.NATURAL_LANGUAGE_FILE_SEARCH,
            }
            if literal_query
            else {}
        ),
    )


def _opengrok_indexed_view_response(
    row: dict, view: Path, status: int, content_type: str, raw: bytes
) -> None:
    # The bracketing /projects/{project}/files responses enumerate Lucene UIDs.
    # The octet endpoint binds each indexed path to served source bytes without
    # relying on the text endpoint's separate getDocument(path) query.
    # Neither check proves that Lucene content terms match the current bytes.
    if status != 200 or content_type != "application/octet-stream":
        raise ValueError(
            f"OpenGrok indexed source {row['path']} is unavailable or not octet data: "
            f"HTTP {status} / {content_type}"
        )
    if (
        _sha(raw) != row["file_sha256"]
        or _sha(_read_control_file(view / row["path"])) != row["file_sha256"]
    ):
        raise ValueError(f"OpenGrok indexed source {row['path']} bytes differ from release")


def _opengrok_indexed_inventory_response(
    config: dict, manifest: dict, status: int, content_type: str, raw: bytes
) -> None:
    if status != 200 or content_type != "application/json":
        raise ValueError(
            f"OpenGrok indexed file inventory is unavailable: HTTP {status} / {content_type}"
        )
    if len(raw) > MAX_HTTP_BYTES:
        raise ValueError("OpenGrok indexed file inventory exceeds HTTP response byte limit")
    native = json.loads(
        raw.decode("utf-8"), object_pairs_hook=_object, parse_constant=_reject_constant
    )
    expected = [row["path"] for row in manifest["files"]]
    if not expected or len(expected) != len(set(expected)):
        raise ValueError("OpenGrok release manifest path inventory is invalid")
    if not isinstance(native, list):
        raise ValueError("OpenGrok indexed file inventory is not a path list")
    prefix = "/" + config["project"] + "/"
    paths = []
    for path in native:
        if not isinstance(path, str) or not path.startswith(prefix):
            raise ValueError("OpenGrok indexed file inventory has an invalid project path")
        relative = path[len(prefix) :]
        if not lexical._canonical_result_path(relative):
            raise ValueError("OpenGrok indexed file inventory has a noncanonical path")
        paths.append(relative)
    if len(paths) != len(set(paths)):
        raise ValueError("OpenGrok indexed file inventory has duplicate paths")
    if len(paths) != len(expected) or set(paths) != set(expected):
        raise ValueError("OpenGrok indexed file inventory differs from release manifest")


def _opengrok_indexed_inventory(
    config: dict, manifest: dict, target: Path, phase: str, batch: int
) -> None:
    endpoint = "/api/v1/projects/" + urllib.parse.quote(config["project"], safe="") + "/files"
    status, content_type, raw, elapsed = _http(config, endpoint, {}, "application/json")
    name = f"indexed-files-{phase}-b{batch:04d}"
    _write(target / f"{name}.json", raw)
    _write(
        target / f"{name}.transport.json",
        json.dumps(
            {
                "endpoint": endpoint,
                "status": status,
                "content_type": content_type,
                "elapsed_ms": elapsed,
            },
            sort_keys=True,
        ).encode()
        + b"\n",
    )
    _opengrok_indexed_inventory_response(config, manifest, status, content_type, raw)


def _opengrok_service_config_probe(
    config: dict, projects: set[str], target: Path, *, capture: bool
) -> dict:
    """Bind the HTTP configuration to the only read-only index mount.

    This observes configuration, not an in-process SearcherManager generation.
    """
    mount = Path(config["backend_snapshot"]["mount_destination"])
    if mount.name != "index" or not mount.is_absolute():
        raise ValueError("OpenGrok service index mount must end in /index")
    result = {}
    for name, endpoint in (
        ("data-root", "/api/v1/configuration/dataRoot"),
        ("indexed-projects", "/api/v1/projects/indexed"),
    ):
        if capture:
            status, content_type, raw, elapsed = _http(config, endpoint, {}, "application/json")
            transport = {
                "endpoint": endpoint,
                "status": status,
                "content_type": content_type,
                "elapsed_ms": elapsed,
            }
            _write(target / f"{name}.json", raw)
            _write(
                target / f"{name}.transport.json",
                canonical_json(transport).encode() + b"\n",
            )
        else:
            raw = _read_control_file(target / f"{name}.json")
            transport = _json(_read_control_file(target / f"{name}.transport.json"))
        if (
            set(transport) != {"endpoint", "status", "content_type", "elapsed_ms"}
            or transport["endpoint"] != endpoint
            or type(transport["status"]) is not int
            or transport["status"] != 200
            or transport["content_type"] != "application/json"
            or type(transport["elapsed_ms"]) not in (int, float)
            or not math.isfinite(transport["elapsed_ms"])
            or transport["elapsed_ms"] < 0
        ):
            raise ValueError("OpenGrok HTTP service configuration transport differs")
        if name == "data-root":
            # Jersey writes String configuration fields as raw text even with JSON media type.
            native = raw.decode("utf-8", "strict")
            if native != str(mount.parent):
                raise ValueError("OpenGrok service dataRoot differs from readonly index mount")
        else:
            native = json.loads(
                raw.decode("utf-8"), object_pairs_hook=_object, parse_constant=_reject_constant
            )
            if (
                type(native) is not list
                or any(type(project) is not str for project in native)
                or len(native) != len(set(native))
                or set(native) != projects
            ):
                raise ValueError("OpenGrok HTTP indexed projects differ from native index")
        result[name] = native
    return result


def _opengrok_write_denial_probe(config: dict, target: Path, *, capture: bool) -> None:
    """Demand an authenticated 403 for a harmless same-value config PUT."""
    endpoint = "/api/v1/configuration/dataRoot"
    body = str(Path(config["backend_snapshot"]["mount_destination"]).parent).encode()
    if capture:
        headers = _auth(config, "Bearer")
        headers["Content-Type"] = "text/plain"
        request = urllib.request.Request(
            config["base_url"] + endpoint,
            data=body,
            headers=headers,
            method="PUT",
        )
        started = time.monotonic_ns()
        try:
            with _service_opener(config).open(
                request, timeout=HTTP_TIMEOUT
            ) as response:
                status = response.status
                raw = response.read(MAX_HTTP_BYTES + 1)
        except urllib.error.HTTPError as error:
            status = error.code
            raw = error.read(MAX_HTTP_BYTES + 1)
        if len(raw) > MAX_HTTP_BYTES:
            raise ValueError("OpenGrok write denial response exceeds bound")
        transport = {
            "endpoint": endpoint,
            "method": "PUT",
            "request_body_sha256": _sha(body),
            "status": status,
            "elapsed_ms": (time.monotonic_ns() - started) / 1_000_000,
        }
        _write(target / "write-denial.body", raw)
        _write(target / "write-denial.transport.json", canonical_json(transport).encode() + b"\n")
    else:
        raw = _read_control_file(target / "write-denial.body")
        transport = _json(_read_control_file(target / "write-denial.transport.json"))
    if (
        set(transport) != {"endpoint", "method", "request_body_sha256", "status", "elapsed_ms"}
        or transport["endpoint"] != endpoint
        or transport["method"] != "PUT"
        or transport["request_body_sha256"] != _sha(body)
        or type(transport["status"]) is not int
        or transport["status"] != 403
        or type(transport["elapsed_ms"]) not in (int, float)
        or not math.isfinite(transport["elapsed_ms"])
        or transport["elapsed_ms"] < 0
        or len(raw) > MAX_HTTP_BYTES
    ):
        raise ValueError("OpenGrok authenticated configuration write was not denied")


def _opengrok_index_batches(manifest: dict, view: Path) -> list[tuple[int, int]]:
    """Partition the frozen source universe by bounded file count and bytes."""
    files = manifest["files"]
    if not isinstance(files, list) or not files:
        raise ValueError("OpenGrok indexed view requires a nonempty manifest")
    batches: list[tuple[int, int]] = []
    start = 0
    accumulated = 0
    for index, row in enumerate(files):
        size = (view / row["path"]).stat().st_size
        if size > MAX_HTTP_BYTES:
            raise ValueError("OpenGrok indexed source exceeds HTTP response byte limit")
        if index > start and (
            index - start >= MAX_INDEXED_VIEW_BATCH_FILES
            or accumulated + size > MAX_INDEXED_VIEW_BATCH_BYTES
        ):
            batches.append((start, index))
            start = index
            accumulated = 0
        accumulated += size
    batches.append((start, len(files)))
    return batches


def _opengrok_indexed_view(config: dict, manifest: dict, view: Path, target: Path) -> None:
    files = manifest["files"]
    for batch, (start, end) in enumerate(_opengrok_index_batches(manifest, view)):
        deadline = time.monotonic() + MAX_INDEXED_VIEW_SECONDS
        _opengrok_indexed_inventory(config, manifest, target, "before", batch)
        if time.monotonic() >= deadline:
            raise ValueError("OpenGrok full indexed view probe timed out")
        for index in range(start, end):
            row = files[index]
            if time.monotonic() >= deadline:
                raise ValueError("OpenGrok full indexed view probe timed out")
            path = "/" + config["project"] + "/" + row["path"]
            status, content_type, raw, elapsed = _http(
                config, "/api/v1/file/content", {"path": path}, "application/octet-stream"
            )
            name = f"{index:06d}"
            _write(target / f"{name}.content", raw)
            _write(
                target / f"{name}.transport.json",
                json.dumps(
                    {
                        "path": path,
                        "status": status,
                        "content_type": content_type,
                        "elapsed_ms": elapsed,
                    },
                    sort_keys=True,
                ).encode()
                + b"\n",
            )
            _opengrok_indexed_view_response(row, view, status, content_type, raw)
            if time.monotonic() >= deadline:
                raise ValueError("OpenGrok full indexed view probe timed out")
        _opengrok_indexed_inventory(config, manifest, target, "after", batch)
        if time.monotonic() >= deadline:
            raise ValueError("OpenGrok full indexed view probe timed out")


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
                    if sum(map(len, buffers.values())) > MAX_PROCESS_BYTES:
                        raise ValueError("cs process output exceeds 16 MiB limit")
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise ValueError("cs process timed out")
        process.wait(timeout=remaining)
    except BaseException as error:
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


def _cs_literal_query(query: str) -> str:
    """Encode whitespace terms as data, preserving cs's native default AND.

    cs 3.2.0's phrase lexer does not support quote escaping. A term containing
    a quote therefore uses a literal RE2 pattern with all metacharacters and
    the regex delimiter escaped; backslashes inside ordinary phrases are data.
    """
    if not isinstance(query, str) or not query.split() or "\x00" in query:
        raise ValueError("cs natural-language query must be nonempty data without NUL")
    terms = []
    for term in query.split():
        if '"' in term:
            pattern = "".join("\\" + char if char in r"\.+*?()|[]{}^$/" else char for char in term)
            terms.append(f"/{pattern}/")
        else:
            terms.append(f'"{term}"')
    return " ".join(terms)


def _cs_argv(binary: Path, query: str, view: Path, *, literal_query: bool = False) -> list[str]:
    return [
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
        _cs_literal_query(query) if literal_query else query,
    ]


def _cs(
    binary: Path,
    task: dict,
    gold: list[str],
    view: Path,
    admitted: dict[str, str],
    target: Path,
    *,
    literal_query: bool = False,
) -> dict:
    start_ns = time.monotonic_ns()
    argv = _cs_argv(binary, task["query"], view, literal_query=literal_query)
    code, stdout, stderr, elapsed = _process(argv, 60)
    paths = _cs_native_paths(view, code, stdout, stderr)
    completed = _completed_native(paths, start_ns)
    row = _cs_response(
        task, gold, view, admitted, code, stdout, stderr, elapsed, literal_query=literal_query
    )
    row["completed_response"] = completed
    _write(target, stdout)
    _write(target.with_suffix(".stderr"), stderr)
    _write(
        target.with_suffix(".process.json"),
        json.dumps(
            {
                "argv": argv,
                "exit_code": code,
                "elapsed_ms": elapsed,
            },
            sort_keys=True,
        ).encode()
        + b"\n",
    )
    return row


def _cs_fuzzy_query(query: str) -> str:
    """cs 3.2.0's explicit one-edit term syntax, separate from bare search."""
    if not isinstance(query, str):
        raise ValueError("cs fuzzy query must be a bare ASCII identifier")
    try:
        query_plan.plan_lexical_request("code_search_typo_file", query)
    except query_plan.QueryPlanError as error:
        raise ValueError("cs fuzzy query must be a bare ASCII identifier") from error
    return query + "~1"


def _cs_fuzzy_argv(binary: Path, query: str, view: Path) -> list[str]:
    return _cs_argv(binary, _cs_fuzzy_query(query), view)


def _cs_fuzzy_response(
    task: dict,
    gold: list[str],
    view: Path,
    admitted: dict[str, str],
    argv: list[str],
    code: int,
    stdout: bytes,
    stderr: bytes,
    elapsed: float,
) -> dict:
    if not argv or argv != _cs_fuzzy_argv(Path(argv[0]), task["query"], view):
        raise ValueError("cs fuzzy native argv differs from the declared request")
    decoded = _cs_response(task, gold, view, admitted, code, stdout, stderr, elapsed)
    return {
        "capability": CS_FUZZY_CAPABILITY,
        "request_mode": "explicit_osa1_typo",
        "task_id": task["task_id"],
        "submitted_query": task["query"],
        "native_query": argv[-1],
        "status": "success",
        "paths": decoded["paths"],
        "elapsed_ms": elapsed,
        "exit_code": code,
        "stdout_sha256": _sha(stdout),
        "stderr_sha256": _sha(stderr),
    }


def _cs_fuzzy(
    binary: Path, task: dict, gold: list[str], view: Path, admitted: dict[str, str], target: Path
) -> dict:
    argv = _cs_fuzzy_argv(binary, task["query"], view)
    code, stdout, stderr, elapsed = _process(argv, 60)
    _write(target, stdout)
    _write(target.with_suffix(".stderr"), stderr)
    _write(
        target.with_suffix(".process.json"),
        json.dumps(
            {"argv": argv, "exit_code": code, "elapsed_ms": elapsed}, sort_keys=True
        ).encode()
        + b"\n",
    )
    return _cs_fuzzy_response(task, gold, view, admitted, argv, code, stdout, stderr, elapsed)


def _cs_fuzzy_spec(path: Path) -> dict:
    spec = _json(_read_control_file(path))
    if (
        set(spec)
        != {"schema_version", "capability", "corpus", "suite", "query_pack", "cs", "output_root"}
        or type(spec["schema_version"]) is not int
        or spec["schema_version"] != 1
        or spec["capability"] != CS_FUZZY_CAPABILITY
        or not isinstance(spec["cs"], dict)
        or set(spec["cs"]) != {"binary"}
    ):
        raise ValueError("cs fuzzy capability requires a closed schema v1 spec")
    corpus_binding._selection(spec["corpus"])
    for key in ("suite", "query_pack", "output_root"):
        value = spec[key]
        if (
            not isinstance(value, str)
            or not Path(value).is_absolute()
            or ".." in Path(value).parts
            or Path(value).as_posix() != value
        ):
            raise ValueError(f"cs fuzzy {key} must be a canonical absolute path")
    binary = spec["cs"]["binary"]
    if (
        not isinstance(binary, str)
        or not Path(binary).is_absolute()
        or ".." in Path(binary).parts
        or Path(binary).as_posix() != binary
    ):
        raise ValueError("cs fuzzy binary must be a canonical absolute path")
    return spec


def _cs_fuzzy_inputs(
    spec: dict, *, bound_release: BoundRelease | None = None
) -> tuple[dict, bytes, bytes, bytes, dict, dict, dict, Path]:
    release = Path(spec["corpus"]["release_path"])
    document = (
        corpus_release.validate(release)
        if bound_release is None
        else bound_release.recheck(release)
    )
    repository = next(
        (
            row
            for row in document["repositories"]
            if row["recipe"]["name"] == spec["corpus"]["repository"]
        ),
        None,
    )
    if repository is None:
        raise ValueError("cs fuzzy selected repository is absent from release")
    view_name = spec["corpus"]["view"]
    manifest_raw = _read_control_file(release / repository["views"][view_name]["manifest"])
    suite_raw = _read_control_file(Path(spec["suite"]))
    pack_raw = _read_control_file(Path(spec["query_pack"]))
    binding = corpus_binding._bind(document, manifest_raw, spec["corpus"], suite_raw, pack_raw)
    suite, pack, manifest = _json(suite_raw), _json(pack_raw), _json(manifest_raw)
    admitted = lexical._file_universe(suite, pack)
    tasks = lexical._tasks(
        suite, pack, file_policy="code_search_typo_file", allow_single_lexical=True
    )
    if any(
        not isinstance(task.get("evaluation_contract"), dict)
        or task["evaluation_contract"].get("request_mode") != "explicit_osa1_typo"
        or task["evaluation_contract"].get("result_unit") != "distinct_file"
        for task in suite["tasks"]
    ):
        raise ValueError("cs fuzzy suite requires explicit OSA1 file-search contract")
    if any(re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*", task_id) is None for task_id in tasks):
        raise ValueError("cs fuzzy task IDs must be safe filename components")
    if {row["path"] for row in manifest["files"]} != admitted:
        raise ValueError("cs fuzzy file universe differs from selected release")
    for task in pack["tasks"]:
        _cs_fuzzy_query(task["query"])
    view = release / "views" / spec["corpus"]["repository"] / view_name
    return (
        binding,
        suite_raw,
        pack_raw,
        manifest_raw,
        pack,
        tasks,
        {row["path"]: row["file_sha256"] for row in manifest["files"]},
        view,
    )


def capture_cs_fuzzy(spec_path: Path, *, bound_release: BoundRelease | None = None) -> dict:
    """Capture cs ~1 separately; never submit these rows to default product_result."""
    spec = _cs_fuzzy_spec(spec_path)
    root = Path(spec["output_root"])
    stage = root.with_name(root.name + ".staging")
    checkout = Path(__file__).resolve().parents[3]
    release = Path(spec["corpus"]["release_path"])
    input_paths = (
        spec_path,
        root,
        release,
        Path(spec["suite"]),
        Path(spec["query_pack"]),
        Path(spec["cs"]["binary"]),
    )
    if any(path.resolve().is_relative_to(checkout) for path in input_paths):
        raise ValueError("cs fuzzy inputs and output must stay outside the source checkout")
    if (
        root.exists()
        or root.is_symlink()
        or stage.exists()
        or stage.is_symlink()
        or root.resolve().is_relative_to(release.resolve())
        or release.resolve().is_relative_to(root.resolve())
    ):
        raise ValueError("cs fuzzy output must be fresh and disjoint from release")
    binding, suite_raw, pack_raw, manifest_raw, pack, tasks, admitted, view = _cs_fuzzy_inputs(
        spec, bound_release=bound_release
    )
    binary = Path(spec["cs"]["binary"]).resolve(strict=True)
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise ValueError("cs fuzzy binary must be executable")
    code, version_raw, stderr, _ = _process([str(binary), "--version"], 10)
    version = version_raw.decode().strip()
    if code != 0 or stderr or version != CS_VERIFIED_VERSION:
        raise ValueError("cs fuzzy capability is verified only for cs 3.2.0")
    binary_sha = _sha_file(binary)
    sources = _source_hashes()
    stage.mkdir(parents=True)
    for name, raw in (
        ("spec.json", _read_control_file(spec_path)),
        ("suite.json", suite_raw),
        ("query-pack.json", pack_raw),
        ("manifest.json", manifest_raw),
        ("binding.json", canonical_json(binding).encode() + b"\n"),
    ):
        _write(stage / name, raw)
    with (stage / "cs_fuzzy_rows.jsonl").open("xb") as stream:
        for task in pack["tasks"]:
            task_id = task["task_id"]
            row = _cs_fuzzy(
                binary, task, tasks[task_id][1], view, admitted, stage / "cs" / f"{task_id}.json"
            )
            stream.write(canonical_json(row).encode() + b"\n")
    if (
        _read_control_file(Path(spec["suite"])) != suite_raw
        or _read_control_file(Path(spec["query_pack"])) != pack_raw
        or _sha_file(binary) != binary_sha
        or _source_hashes() != sources
        or (
            corpus_release.validate(release)
            if bound_release is None
            else bound_release.recheck(release)
        )["digest"]
        != binding["release_digest"]
    ):
        raise ValueError("cs fuzzy input, binary or source changed during capture")
    summary = {
        "schema_version": 1,
        "capability": CS_FUZZY_CAPABILITY,
        "request_mode": "explicit_osa1_typo",
        "status": "diagnostic_unqualified",
        "scoring_status": "not_scored",
        "tasks": len(tasks),
        "binding": binding,
        "cs_version": version,
        "cs_binary_sha256": binary_sha,
        "producer_sources_sha256": sources,
        "rows_sha256": _sha_file(stage / "cs_fuzzy_rows.jsonl"),
        "raw_capture_sha256": {
            path.relative_to(stage).as_posix(): _sha_file(path)
            for path in sorted((stage / "cs").iterdir())
        },
        "indexed_universe_attested": False,
        "exclusions": [
            "independent_gold",
            "qualified_speed",
            "backend_indexed_universe_attestation",
        ],
    }
    _write(stage / "capture.json", canonical_json(summary).encode() + b"\n")
    if root.exists():
        raise ValueError("cs fuzzy output appeared during capture")
    stage.rename(root)
    return summary


def verify_cs_fuzzy(root: Path, *, bound_release: BoundRelease | None = None) -> dict:
    """Re-derive the explicit native argv and every row from captured process bytes."""
    inventory = corpus_release.regular_tree(root)
    spec = _cs_fuzzy_spec(root / "spec.json")
    if Path(spec["output_root"]) != root:
        raise ValueError("cs fuzzy output root differs from frozen spec")
    summary = _json(_read_control_file(root / "capture.json"))
    binding, suite_raw, pack_raw, manifest_raw, pack, tasks, admitted, view = _cs_fuzzy_inputs(
        spec, bound_release=bound_release
    )
    binary = Path(spec["cs"]["binary"]).resolve(strict=True)
    code, version_raw, stderr, _ = _process([str(binary), "--version"], 10)
    version = version_raw.decode().strip()
    if code != 0 or stderr or version != CS_VERIFIED_VERSION:
        raise ValueError("cs fuzzy capability/version changed")
    fixed = {
        "spec.json",
        "suite.json",
        "query-pack.json",
        "manifest.json",
        "binding.json",
        "capture.json",
        "cs_fuzzy_rows.jsonl",
    }
    raw_paths = {
        f"cs/{task['task_id']}.{suffix}"
        for task in pack["tasks"]
        for suffix in ("json", "stderr", "process.json")
    }
    if set(inventory) != fixed | raw_paths:
        raise ValueError("cs fuzzy capture file inventory differs")
    if (
        set(summary)
        != {
            "schema_version",
            "capability",
            "request_mode",
            "status",
            "scoring_status",
            "tasks",
            "binding",
            "cs_version",
            "cs_binary_sha256",
            "producer_sources_sha256",
            "rows_sha256",
            "raw_capture_sha256",
            "indexed_universe_attested",
            "exclusions",
        }
        or summary["schema_version"] != 1
        or summary["capability"] != CS_FUZZY_CAPABILITY
        or summary["request_mode"] != "explicit_osa1_typo"
        or summary["status"] != "diagnostic_unqualified"
        or summary["scoring_status"] != "not_scored"
        or summary["indexed_universe_attested"] is not False
        or summary["tasks"] != len(tasks)
        or summary["binding"] != binding
        or summary["cs_version"] != version
        or summary["cs_binary_sha256"] != _sha_file(binary)
        or summary["producer_sources_sha256"] != _source_hashes()
        or summary["rows_sha256"] != _sha_file(root / "cs_fuzzy_rows.jsonl")
        or summary["raw_capture_sha256"]
        != {name: _sha_file(root / name) for name in sorted(raw_paths)}
        or summary["exclusions"]
        != ["independent_gold", "qualified_speed", "backend_indexed_universe_attestation"]
        or _read_control_file(root / "suite.json") != suite_raw
        or _read_control_file(root / "query-pack.json") != pack_raw
        or _read_control_file(root / "manifest.json") != manifest_raw
        or _json(_read_control_file(root / "binding.json")) != binding
    ):
        raise ValueError("cs fuzzy capture binding or native bytes differ")

    def replay(task: dict, row: dict) -> None:
        task_id = task["task_id"]
        terminal = _json(_read_control_file(root / "cs" / f"{task_id}.process.json"))
        argv = _cs_fuzzy_argv(binary, task["query"], view)
        if (
            set(terminal) != {"argv", "exit_code", "elapsed_ms"}
            or terminal["argv"] != argv
            or type(terminal["exit_code"]) is not int
            or type(terminal["elapsed_ms"]) not in (int, float)
            or not math.isfinite(terminal["elapsed_ms"])
            or terminal["elapsed_ms"] < 0
        ):
            raise ValueError("cs fuzzy native argv or process metadata differs")
        derived = _cs_fuzzy_response(
            task,
            tasks[task_id][1],
            view,
            admitted,
            argv,
            terminal["exit_code"],
            _read_control_file(root / "cs" / f"{task_id}.json"),
            _read_control_file(root / "cs" / f"{task_id}.stderr"),
            terminal["elapsed_ms"],
        )
        if row != derived:
            raise ValueError("cs fuzzy row differs from native response")

    _replay_rows(root / "cs_fuzzy_rows.jsonl", pack["tasks"], replay)
    if bound_release is not None:
        bound_release.recheck(Path(spec["corpus"]["release_path"]))
    return summary


def _cs_native_paths(view: Path, code: int, stdout: bytes, stderr: bytes) -> list[str]:
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
    if len(paths) != len(set(paths)) or any(
        not lexical._canonical_result_path(path) for path in paths
    ):
        raise ValueError("cs native paths are duplicate or noncanonical")
    return paths


def _cs_response(
    task: dict,
    gold: list[str],
    view: Path,
    admitted: dict[str, str],
    code: int,
    stdout: bytes,
    stderr: bytes,
    elapsed: float,
    *,
    literal_query: bool = False,
) -> dict:
    paths = _paths(_cs_native_paths(view, code, stdout, stderr), admitted, view)
    return _row(
        task,
        gold,
        paths,
        elapsed,
        exit_code=code,
        paths=paths,
        stdout_sha256=_sha(stdout),
        stderr_sha256=_sha(stderr),
        **(
            {
                "request_query": _cs_literal_query(task["query"]),
                "request_mode": query_plan.NATURAL_LANGUAGE_FILE_SEARCH,
            }
            if literal_query
            else {}
        ),
    )


def _opengrok_readonly_files(config: dict) -> dict:
    service = config["readonly_service"]
    fixture = config.get("query_reader_witness")
    allowed_classes = opengrok_query_witness.CLASS_FILES if fixture is not None else frozenset()
    if fixture is not None and (
        set(opengrok_query_witness.PINNED_CLASS_SHA256) != allowed_classes
        or any(
            type(digest) is not str or re.fullmatch(r"[0-9a-f]{64}", digest) is None
            for digest in opengrok_query_witness.PINNED_CLASS_SHA256.values()
        )
    ):
        raise ValueError("OpenGrok instrumented classes lack independent pinned build digests")
    webapps = Path(service["webapps_root"])
    etc = Path(service["etc_root"])
    if {path.name for path in webapps.iterdir()} != {"ROOT"}:
        raise ValueError("OpenGrok readonly webapps must contain only exploded ROOT")
    xml_path = webapps / "ROOT" / "WEB-INF" / "web.xml"
    config_path = etc / "configuration.xml"
    for path in (xml_path, config_path):
        if path.is_symlink() or not path.is_file() or path.resolve(strict=True) != path:
            raise ValueError("OpenGrok readonly web or configuration file differs")
    raw = _read_control_file(xml_path)
    if b"<!DOCTYPE" in raw or b"<!ENTITY" in raw:
        raise ValueError("OpenGrok readonly web descriptor has external markup")
    root = ET.fromstring(raw)
    namespace = "{https://jakarta.ee/xml/ns/jakartaee}"
    if root.tag != namespace + "web-app":
        raise ValueError("OpenGrok readonly web descriptor namespace differs")
    contexts = root.findall(namespace + "context-param")
    configs = [
        item.findtext(namespace + "param-value")
        for item in contexts
        if item.findtext(namespace + "param-name") == "CONFIGURATION"
    ]
    if configs != ["/opengrok/etc/configuration.xml"]:
        raise ValueError("OpenGrok readonly web configuration path differs")
    denies = []
    for item in root.findall(namespace + "security-constraint"):
        collections = item.findall(namespace + "web-resource-collection")
        if any(
            collection.findtext(namespace + "url-pattern") == "/api/*"
            and [node.text for node in collection.findall(namespace + "http-method-omission")]
            == ["GET"]
            and not collection.findall(namespace + "http-method")
            for collection in collections
        ):
            constraint = item.find(namespace + "auth-constraint")
            denies.append(constraint is not None and len(constraint) == 0)
    if denies != [True]:
        raise ValueError("OpenGrok non-GET API methods are not denied by web.xml")
    denial = next(
        item
        for item in root.findall(namespace + "security-constraint")
        if any(
            collection.findtext(namespace + "url-pattern") == "/api/*"
            and [node.text for node in collection.findall(namespace + "http-method-omission")]
            == ["GET"]
            for collection in item.findall(namespace + "web-resource-collection")
        )
    )
    root.remove(denial)
    with zipfile.ZipFile(service["source_war"]) as archive:
        expected_files = set()
        base_hashes = {}
        total = 0
        for entry in archive.infolist():
            if entry.is_dir():
                continue
            name = entry.filename
            if (
                name in expected_files
                or not lexical._canonical_result_path(name)
                or stat.S_ISLNK(entry.external_attr >> 16)
            ):
                raise ValueError("OpenGrok source WAR has unsafe or duplicate member")
            expected_files.add(name)
            total += entry.file_size
            if len(expected_files) > 4096 or total > MAX_INDEX_BYTES:
                raise ValueError("OpenGrok source WAR exceeds webapp bound")
            base_hashes[name] = _sha(archive.read(entry))
            if name != "WEB-INF/web.xml" and name not in allowed_classes:
                target = webapps / "ROOT" / name
                if not target.is_file() or base_hashes[name] != _sha_file(target):
                    raise ValueError("OpenGrok readonly webapp differs from image WAR")
        if "WEB-INF/web.xml" not in expected_files:
            raise ValueError("OpenGrok source WAR has no web descriptor")
        original = ET.fromstring(archive.read("WEB-INF/web.xml"))
    if fixture is not None:
        if not allowed_classes <= expected_files:
            raise ValueError("OpenGrok instrumented controller classes are absent from image WAR")
        with zipfile.ZipFile(fixture["instrumented_war"]) as archive:
            observed = set()
            total = 0
            for entry in archive.infolist():
                if entry.is_dir():
                    continue
                name = entry.filename
                if (
                    name in observed
                    or not lexical._canonical_result_path(name)
                    or stat.S_ISLNK(entry.external_attr >> 16)
                ):
                    raise ValueError("OpenGrok instrumented WAR has unsafe or duplicate member")
                observed.add(name)
                total += entry.file_size
                if total > MAX_INDEX_BYTES or len(observed) > 4096:
                    raise ValueError("OpenGrok instrumented WAR exceeds webapp bound")
                changed_sha = _sha(archive.read(entry))
                target = webapps / "ROOT" / name
                if not target.is_file() or changed_sha != _sha_file(target):
                    raise ValueError("OpenGrok instrumented WAR differs from mounted webapp")
                if name not in allowed_classes | {"WEB-INF/web.xml"} and changed_sha != base_hashes.get(name):
                    raise ValueError("OpenGrok instrumented WAR changes unrelated class or resource")
                if name in allowed_classes and (
                    changed_sha == base_hashes.get(name)
                    or changed_sha != opengrok_query_witness.PINNED_CLASS_SHA256[name]
                ):
                    raise ValueError("OpenGrok instrumented controller class digest differs")
            if observed != expected_files:
                raise ValueError("OpenGrok instrumented WAR file inventory differs")
    if original.tag != root.tag:
        raise ValueError("OpenGrok web descriptor differs from image WAR")
    original_configs = [
        item.find(namespace + "param-value")
        for item in original.findall(namespace + "context-param")
        if item.findtext(namespace + "param-name") == "CONFIGURATION"
    ]
    if (
        len(original_configs) != 1
        or original_configs[0].text != "/var/opengrok/etc/configuration.xml"
    ):
        raise ValueError("OpenGrok source WAR configuration default differs")
    original_configs[0].text = "/opengrok/etc/configuration.xml"

    def shape(element):
        return (
            element.tag,
            tuple(sorted(element.attrib.items())),
            (element.text or "").strip(),
            tuple(shape(child) for child in element),
        )

    if shape(original) != shape(root):
        raise ValueError("OpenGrok readonly web descriptor changes unrelated behavior")
    actual_files = {
        path.relative_to(webapps / "ROOT").as_posix()
        for path in (webapps / "ROOT").rglob("*")
        if path.is_file()
    }
    if actual_files != expected_files:
        raise ValueError("OpenGrok readonly webapp file inventory differs from source WAR")
    _, webapps_sha = _backend_tree(webapps)
    return {
        "webapps_sha256": webapps_sha,
        "configuration_sha256": _sha(_read_control_file(config_path)),
        "web_xml_sha256": _sha(raw),
        **(
            {
                "instrumented_war_sha256": _sha_file(Path(fixture["instrumented_war"])),
                "patched_source_sha256": _sha_file(Path(fixture["patched_source"])),
                "source_patch_sha256": _sha_file(Path(fixture["source_patch"])),
                "original_source_sha256": _sha_file(Path(fixture["original_source"])),
            }
            if fixture is not None
            else {}
        ),
    }


def _opengrok_query_reader_scope(config: dict, native_scope: dict, queries: int) -> dict:
    project = config["project"]
    commits = native_scope["reader_commits"]
    if project not in commits or type(queries) is not int or queries <= 0:
        raise ValueError("OpenGrok selected query project is absent from native commits")
    return {
        "scope": "instrumented_search_requests_selected_project_only",
        "selected_project": project,
        "queries": queries,
        "reader_commit": commits[project],
        "instrumented_war_sha256": _sha_file(
            Path(config["query_reader_witness"]["instrumented_war"])
        ),
        "all_project_readers_attested": False,
        "timing_scope": "instrumented_service_only",
        "attested": True,
    }


def _backend_runtime(config: dict) -> dict:
    """Bind a loopback endpoint to one running container and its read-only index mount."""
    backend = config["backend_snapshot"]
    code, stdout, stderr, _ = _process(
        [
            "docker",
            "inspect",
            "--type",
            "container",
            "--format",
            "{{json .}}",
            backend["container_id"],
        ],
        10,
    )
    if code != 0 or stderr:
        raise ValueError("backend container inspect failed")
    native = _json(stdout)
    state = native.get("State")
    network = native.get("NetworkSettings")
    mounts = native.get("Mounts")
    root = Path(backend["root"])
    if not root.is_dir() or root.resolve(strict=True) != root:
        raise ValueError("backend index root must be an existing canonical directory")
    if (
        native.get("Id") != backend["container_id"]
        or native.get("Image") != "sha256:" + config["server_image_digest"]
        or not isinstance(state, dict)
        or state.get("Running") is not True
        or type(state.get("Pid")) is not int
        or state["Pid"] <= 0
        or not isinstance(state.get("StartedAt"), str)
        or not state["StartedAt"]
        or type(native.get("RestartCount")) is not int
        or not isinstance(mounts, list)
        or not isinstance(network, dict)
        or not isinstance(network.get("Ports"), dict)
    ):
        raise ValueError("backend container identity or running process differs")
    selected_mounts = [
        mount
        for mount in mounts
        if isinstance(mount, dict) and mount.get("Destination") == backend["mount_destination"]
    ]
    if (
        len(selected_mounts) != 1
        or selected_mounts[0].get("Type") != "bind"
        or selected_mounts[0].get("Source") != backend["root"]
        or selected_mounts[0].get("RW") is not False
    ):
        raise ValueError("backend index mount differs or is writable")
    readonly_files = None
    if "readonly_service" in config:
        service = config["readonly_service"]
        host = native.get("HostConfig")
        container = native.get("Config")
        attached = network.get("Networks")
        required_mounts = {
            "/usr/local/tomcat/webapps": service["webapps_root"],
            "/opengrok/etc": service["etc_root"],
            "/opengrok/src": service["source_root"],
        }
        protected = set(required_mounts) | {backend["mount_destination"]}
        if (
            not isinstance(host, dict)
            or not isinstance(container, dict)
            or not isinstance(attached, dict)
            or host.get("NetworkMode") != service["network"]
            or host.get("Privileged") is not False
            or host.get("CapAdd") not in (None, [])
            or set(attached) != {service["network"]}
            or native.get("Path") != "/usr/local/tomcat/bin/catalina.sh"
            or native.get("Args") != ["run"]
            or container.get("Entrypoint") != ["/usr/local/tomcat/bin/catalina.sh"]
            or container.get("Cmd") != ["run"]
            or container.get("User") != "1111:1111"
            or native.get("RestartCount") != 0
            or not isinstance(native.get("Created"), str)
            or not native["Created"]
        ):
            raise ValueError("OpenGrok readonly service must start only Tomcat")
        for destination, source in required_mounts.items():
            selected = [
                mount
                for mount in mounts
                if isinstance(mount, dict) and mount.get("Destination") == destination
            ]
            if (
                len(selected) != 1
                or selected[0].get("Type") != "bind"
                or selected[0].get("Source") != source
                or selected[0].get("RW") is not False
            ):
                raise ValueError("OpenGrok readonly service mount differs")
        temporary = {
            "/tmp",
            "/usr/local/tomcat/temp",
            "/usr/local/tomcat/work",
            "/usr/local/tomcat/logs",
        }
        for mount in mounts:
            destination = mount.get("Destination") if isinstance(mount, dict) else None
            if not isinstance(destination, str):
                raise ValueError("OpenGrok readonly service has malformed mount")
            if destination in protected:
                continue
            if destination in temporary and mount.get("Type") == "tmpfs":
                continue
            raise ValueError("OpenGrok readonly service has extra or overlapping mount")
        if set(network["Ports"]) != {backend["container_port"]}:
            raise ValueError("OpenGrok readonly service exposes another container port")
        readonly_files = _opengrok_readonly_files(config)
        code, stdout, stderr, _ = _process(
            ["docker", "exec", backend["container_id"], "sha256sum", "/opengrok/lib/source.war"],
            30,
        )
        expected_war_sha = _sha_file(Path(service["source_war"]))
        if (
            code != 0
            or stderr
            or stdout.decode("ascii", "strict").strip()
            != expected_war_sha + "  /opengrok/lib/source.war"
        ):
            raise ValueError("OpenGrok readonly webapp source WAR differs from running image")
        readonly_files["source_war_sha256"] = expected_war_sha
    parsed = urllib.parse.urlsplit(config["base_url"])
    service_port = parsed.port or (443 if parsed.scheme == "https" else 80)
    bindings = network["Ports"].get(backend["container_port"])
    allowed_host_ips = {
        "127.0.0.1": {"127.0.0.1", "0.0.0.0"},
        "::1": {"::1", "::"},
        "localhost": {"127.0.0.1", "0.0.0.0", "::1", "::"},
    }[parsed.hostname]
    if not isinstance(bindings, list) or not any(
        isinstance(binding, dict)
        and binding.get("HostPort") == str(service_port)
        and binding.get("HostIp") in allowed_host_ips
        for binding in bindings
    ):
        raise ValueError("backend container port does not bind the service URL")
    if "readonly_service" in config and (
        len(bindings) != 1 or bindings[0].get("HostIp") not in {"127.0.0.1", "::1"}
    ):
        raise ValueError("OpenGrok readonly service port must bind only loopback")
    return {
        "container_id": backend["container_id"],
        "image_sha256": config["server_image_digest"],
        "pid": state["Pid"],
        "started_at": state["StartedAt"],
        "restart_count": native["RestartCount"],
        "mount_source": backend["root"],
        "mount_destination": backend["mount_destination"],
        "container_port": backend["container_port"],
        "service_port": service_port,
        **(
            {"readonly_files": readonly_files, "created_at": native["Created"]}
            if readonly_files is not None
            else {}
        ),
    }


def _backend_index_paths(root: Path) -> set[str]:
    """Inventory index files while rejecting active Zoekt staging artifacts."""
    if root.is_symlink() or any(parent.is_symlink() for parent in root.absolute().parents):
        raise ValueError("backend index contains a link or special file")

    def onerror(error: OSError) -> None:
        raise ValueError("backend index inventory is unreadable") from error

    paths: set[str] = set()
    for directory, directories, files in os.walk(root, followlinks=False, onerror=onerror):
        base = Path(directory)
        for name in tuple(directories):
            path = base / name
            if path.is_symlink():
                raise ValueError("backend index contains a link or special file")
            if base == root and name in {".indexserver.tmp", ".trash"}:
                if any(path.iterdir()):
                    raise ValueError("backend transient index directory is nonempty")
                directories.remove(name)
        for name in files:
            path = base / name
            mode = path.lstat().st_mode
            if base == root and name == "indexserver.sock" and stat.S_ISSOCK(mode):
                continue
            if not stat.S_ISREG(mode):
                raise ValueError("backend index contains a link or special file")
            paths.add(path.relative_to(root).as_posix())
    return paths


def _backend_tree(root: Path) -> tuple[list[dict], str]:
    paths = _backend_index_paths(root)
    if not paths or len(paths) > MAX_BACKEND_INDEX_FILES:
        raise ValueError("backend index file inventory is empty or exceeds 4096 files")
    rows = []
    total = 0
    for name in sorted(paths):
        if not lexical._canonical_result_path(name):
            raise ValueError("backend index contains a noncanonical path")
        path = root / name
        before = path.stat()
        if not stat.S_ISREG(before.st_mode):
            raise ValueError("backend index contains a nonregular file")
        total += before.st_size
        if total > MAX_INDEX_BYTES:
            raise ValueError("backend index exceeds 512 MiB")
        digest, size = file_digest(path)
        after = path.stat()
        if size != before.st_size or (
            before.st_dev,
            before.st_ino,
            before.st_mtime_ns,
            before.st_size,
        ) != (after.st_dev, after.st_ino, after.st_mtime_ns, after.st_size):
            raise ValueError("backend index file changed during hashing")
        rows.append({"path": name, "sha256": digest.removeprefix("sha256:"), "bytes": size})
    if _backend_index_paths(root) != paths:
        raise ValueError("backend index inventory changed during hashing")
    return rows, _sha(canonical_json(rows).encode())


def _backend_snapshot(config: dict) -> dict:
    first = _backend_runtime(config)
    rows, digest = _backend_tree(Path(config["backend_snapshot"]["root"]))
    if _backend_runtime(config) != first:
        raise ValueError("backend process or mount changed during index hashing")
    return {"runtime": first, "files": rows, "tree_sha256": digest}


def _docker_utc_time(value: object) -> tuple[int, int]:
    if type(value) is not str:
        raise ValueError("OpenGrok snapshot time is not UTC text")
    match = re.fullmatch(r"(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(?:\.(\d{1,9}))?Z", value)
    if match is None:
        raise ValueError("OpenGrok snapshot time differs from Docker UTC format")
    base = datetime.strptime(match[1], "%Y-%m-%dT%H:%M:%S").replace(tzinfo=timezone.utc)
    return int(base.timestamp()), int((match[2] or "").ljust(9, "0"))


def _validate_opengrok_snapshot_seal(config: dict, snapshot: dict) -> None:
    """Check declared seal ordering; the caller's timestamp is not an independent attestation."""
    service = config["readonly_service"]
    fixture = config.get("query_reader_witness")
    receipt = _json(_read_control_file(Path(service["snapshot_receipt"])))
    witness_receipt = (
        {
            "query_witness_contract": opengrok_query_witness.CONTRACT,
            **{
                name: snapshot["runtime"]["readonly_files"][name]
                for name in (
                    "instrumented_war_sha256", "patched_source_sha256",
                    "source_patch_sha256", "original_source_sha256",
                )
            },
        }
        if fixture is not None
        else {}
    )
    if (
        set(receipt)
        != ({
            "schema_version",
            "sealed_at_utc",
            "index_root",
            "index_tree_sha256",
            "webapps_root",
            "webapps_sha256",
            "etc_root",
            "configuration_sha256",
            "source_root",
            "source_war_sha256",
        } | set(witness_receipt))
        or type(receipt["schema_version"]) is not int
        or receipt["schema_version"] != 1
        or receipt["index_root"] != config["backend_snapshot"]["root"]
        or receipt["index_tree_sha256"] != snapshot["tree_sha256"]
        or receipt["webapps_root"] != service["webapps_root"]
        or receipt["webapps_sha256"] != snapshot["runtime"]["readonly_files"]["webapps_sha256"]
        or receipt["etc_root"] != service["etc_root"]
        or receipt["configuration_sha256"]
        != snapshot["runtime"]["readonly_files"]["configuration_sha256"]
        or receipt["source_root"] != service["source_root"]
        or receipt["source_war_sha256"]
        != snapshot["runtime"]["readonly_files"]["source_war_sha256"]
        or any(receipt[name] != value for name, value in witness_receipt.items())
        or not (
            _docker_utc_time(receipt["sealed_at_utc"])
            < _docker_utc_time(snapshot["runtime"]["created_at"])
            <= _docker_utc_time(snapshot["runtime"]["started_at"])
        )
    ):
        raise ValueError("OpenGrok readonly snapshot was not sealed before server creation")


def _validate_backend_snapshot(config: dict, snapshot: dict) -> None:
    backend = config["backend_snapshot"]
    if not isinstance(snapshot, dict) or set(snapshot) != {"runtime", "files", "tree_sha256"}:
        raise ValueError("backend snapshot shape differs")
    runtime = snapshot["runtime"]
    if (
        not isinstance(runtime, dict)
        or set(runtime)
        != {
            "container_id",
            "image_sha256",
            "pid",
            "started_at",
            "restart_count",
            "mount_source",
            "mount_destination",
            "container_port",
            "service_port",
        }
        | ({"readonly_files", "created_at"} if "readonly_service" in config else set())
        or runtime["container_id"] != backend["container_id"]
        or runtime["image_sha256"] != config["server_image_digest"]
        or runtime["mount_source"] != backend["root"]
        or runtime["mount_destination"] != backend["mount_destination"]
        or runtime["container_port"] != backend["container_port"]
        or type(runtime["pid"]) is not int
        or runtime["pid"] <= 0
        or type(runtime["restart_count"]) is not int
        or runtime["restart_count"] < 0
        or not isinstance(runtime["started_at"], str)
        or not runtime["started_at"]
        or type(runtime["service_port"]) is not int
        or runtime["service_port"]
        != (
            urllib.parse.urlsplit(config["base_url"]).port
            or (443 if config["base_url"].startswith("https:") else 80)
        )
    ):
        raise ValueError("backend snapshot runtime identity differs")
    if "readonly_service" in config:
        files = runtime["readonly_files"]
        expected_files = _opengrok_readonly_files(config)
        if (
            type(runtime["created_at"]) is not str
            or not runtime["created_at"]
            or runtime["restart_count"] != 0
            or type(files) is not dict
            or set(files)
            != set(expected_files) | {"source_war_sha256"}
            or any(
                type(value) is not str or re.fullmatch(r"[0-9a-f]{64}", value) is None
                for value in files.values()
            )
            or files
            != {
                **expected_files,
                "source_war_sha256": _sha_file(Path(config["readonly_service"]["source_war"])),
            }
        ):
            raise ValueError("OpenGrok readonly web/config files changed")
    rows = snapshot["files"]
    if not isinstance(rows, list) or not 0 < len(rows) <= MAX_BACKEND_INDEX_FILES:
        raise ValueError("backend snapshot file inventory differs")
    total = 0
    previous = ""
    for row in rows:
        if (
            not isinstance(row, dict)
            or set(row) != {"path", "sha256", "bytes"}
            or not isinstance(row["path"], str)
            or not lexical._canonical_result_path(row["path"])
            or row["path"] <= previous
            or not isinstance(row["sha256"], str)
            or re.fullmatch(r"[0-9a-f]{64}", row["sha256"]) is None
            or type(row["bytes"]) is not int
            or row["bytes"] < 0
        ):
            raise ValueError("backend snapshot file entry differs")
        previous = row["path"]
        total += row["bytes"]
    if total > MAX_INDEX_BYTES or snapshot["tree_sha256"] != _sha(canonical_json(rows).encode()):
        raise ValueError("backend snapshot tree digest differs")
    if "readonly_service" in config:
        _validate_opengrok_snapshot_seal(config, snapshot)


def capture(spec_path: Path, *, bound_release: BoundRelease | None = None) -> dict:
    spec = _spec(spec_path)
    products = _selected_products(spec)
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
        *((Path(spec["cs"]["binary"]),) if "cs" in products else ()),
        *(
            (Path(spec["sourcegraph"]["indexed_scope_receipt"]),)
            if "sourcegraph" in products and "indexed_scope_receipt" in spec["sourcegraph"]
            else ()
        ),
        *(
            (Path(spec["sourcegraph"]["projection_git_root"]),)
            if "sourcegraph" in products and "projection_git_root" in spec["sourcegraph"]
            else ()
        ),
        *(
            Path(spec[name]["backend_snapshot"]["root"])
            for name in products
            if name in ("sourcegraph", "opengrok")
            if "backend_snapshot" in spec[name]
        ),
        *(
            tuple(
                Path(spec["opengrok"]["readonly_service"][name])
                for name in (
                    "webapps_root",
                    "etc_root",
                    "source_root",
                    "source_war",
                    "snapshot_receipt",
                )
            )
            if "opengrok" in products and "readonly_service" in spec["opengrok"]
            else ()
        ),
    )
    if any(path.resolve().is_relative_to(checkout) for path in input_paths):
        raise ValueError("live capture inputs and output must stay outside the source checkout")
    if "opengrok" in products and "readonly_service" in spec["opengrok"]:
        service = spec["opengrok"]["readonly_service"]
        for name in ("webapps_root", "etc_root", "source_root", "source_war", "snapshot_receipt"):
            path = Path(service[name]).resolve(strict=True)
            if root.resolve().is_relative_to(path) or path.is_relative_to(root.resolve()):
                raise ValueError("OpenGrok readonly service inputs must be disjoint from output")
    for name in products:
        if name == "cs":
            continue
        if "backend_snapshot" in spec[name]:
            backend_root = Path(spec[name]["backend_snapshot"]["root"]).resolve(strict=True)
            if (
                backend_root.is_relative_to(release.resolve())
                or backend_root.is_relative_to(root.resolve())
                or release.resolve().is_relative_to(backend_root)
                or root.resolve().is_relative_to(backend_root)
            ):
                raise ValueError("backend index root must be disjoint from release and output")
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
    literal_file_query = (
        suite["tasks"][0].get("evaluation_contract", {}).get("request_mode")
        == query_plan.NATURAL_LANGUAGE_FILE_SEARCH
    )
    if any(re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*", task_id) is None for task_id in tasks):
        raise ValueError("task IDs must be safe filename components")
    document = (
        corpus_release.validate(release)
        if bound_release is None
        else bound_release.recheck(release)
    )
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
    projection = (
        _projection_binding(spec["sourcegraph"], manifest) if "sourcegraph" in products else None
    )
    if projection is not None:
        projection_root = Path(spec["sourcegraph"]["projection_git_root"]).resolve(strict=True)
        if (
            projection_root.is_relative_to(release.resolve())
            or projection_root.is_relative_to(root.resolve())
            or release.resolve().is_relative_to(projection_root)
            or root.resolve().is_relative_to(projection_root)
        ):
            raise ValueError("Sourcegraph projection root must be disjoint")
        spec["sourcegraph"]["projection_revision"] = projection["projection_revision"]
    sourcegraph_max_request_target_bytes = None
    if "sourcegraph" in products:
        _, sourcegraph_max_request_target_bytes = _preflight_sourcegraph_request_targets(
            spec["sourcegraph"], pack["tasks"], manifest
        )
    binary = None
    binary_sha = None
    version = None
    if "cs" in products:
        binary = Path(spec["cs"]["binary"]).resolve(strict=True)
        if not binary.is_file() or not os.access(binary, os.X_OK):
            raise ValueError("cs binary must be an executable regular file")
        code, version, stderr, _ = _process([str(binary), "--version"], 10)
        if code != 0 or stderr or not version.strip():
            raise ValueError("cs version command failed")
        binary_sha = _sha_file(binary)
        if literal_file_query and version.decode().strip() != CS_VERIFIED_VERSION:
            raise ValueError("cs natural-language data encoding is verified only for cs 3.2.0")
        if literal_file_query:
            for task in pack["tasks"]:
                _cs_literal_query(task["query"])
    source_hashes = _source_hashes()
    stage.mkdir(parents=True)
    _write(stage / "spec.json", _read_control_file(spec_path))
    _write(stage / "binding.json", json.dumps(binding, sort_keys=True).encode() + b"\n")
    _write(stage / "suite.json", suite_raw)
    _write(stage / "query-pack.json", pack_raw)
    _write(stage / "manifest.json", manifest_raw)
    if projection is not None:
        _write(stage / "sourcegraph-projection.json", canonical_json(projection).encode() + b"\n")
    backend_names = tuple(
        name
        for name in products
        if name in ("sourcegraph", "opengrok") and "backend_snapshot" in spec[name]
    )
    backend_before = {}
    for name in backend_names:
        snapshot = _backend_snapshot(spec[name])
        _validate_backend_snapshot(spec[name], snapshot)
        backend_before[name] = snapshot
        _write(stage / "backend" / f"{name}-before.json", canonical_json(snapshot).encode() + b"\n")
    native_scope = None
    native_projects = None
    if "opengrok" in products and "native_index_reader" in spec["opengrok"]:
        native_projects = opengrok_index_scope.expected_projects(release, document, view_name)
        native_scope = opengrok_index_scope.collect(
            spec["opengrok"]["native_index_reader"],
            Path(spec["opengrok"]["backend_snapshot"]["root"]),
            native_projects,
            stage / "opengrok-native-before",
        )
    readonly_service = "opengrok" in products and "readonly_service" in spec["opengrok"]
    service_config_before = None
    if readonly_service:
        service_config_before = _opengrok_service_config_probe(
            spec["opengrok"], set(native_projects), stage / "opengrok-service-before", capture=True
        )
        _opengrok_write_denial_probe(
            spec["opengrok"], stage / "opengrok-service-before", capture=True
        )
    index_scope = None
    if "sourcegraph" in products and "indexed_scope_receipt" in spec["sourcegraph"]:
        index_scope = sourcegraph_index_scope.verify(
            Path(spec["sourcegraph"]["indexed_scope_receipt"]),
            manifest_raw=manifest_raw,
            release_digest=document["digest"],
            config=spec["sourcegraph"],
            projection=projection,
            snapshot=backend_before["sourcegraph"],
            require_owned_service=True,
        )
        _write(stage / "sourcegraph-index-scope.json", canonical_json(index_scope).encode() + b"\n")
    probe_indexed_view = (
        "opengrok" in products and spec["opengrok"].get("indexed_view_probe") == "full"
    )
    if probe_indexed_view:
        _opengrok_indexed_view(spec["opengrok"], manifest, view, stage / "opengrok-view")
    capture_product = {
        "sourcegraph": lambda task, gold: _sourcegraph(
            spec["sourcegraph"],
            task,
            gold,
            manifest,
            view,
            files,
            stage / "sourcegraph" / f"{task['task_id']}.stream",
        ),
        "opengrok": lambda task, gold: _opengrok(
            spec["opengrok"],
            task,
            gold,
            view,
            files,
            stage / "opengrok" / f"{task['task_id']}.json",
            literal_query=literal_file_query,
            **(
                {
                    "native_commit": native_scope["reader_commits"][spec["opengrok"]["project"]],
                    "output_root": str(root),
                }
                if "query_reader_witness" in spec["opengrok"]
                else {}
            ),
        ),
        "cs": lambda task, gold: _cs(
            binary,
            task,
            gold,
            view,
            files,
            stage / "cs" / f"{task['task_id']}.json",
            literal_query=literal_file_query,
        ),
    }
    with ExitStack() as stack:
        streams = {
            name: stack.enter_context((stage / f"{name}_rows.jsonl").open("xb"))
            for name in products
        }
        for task in pack["tasks"]:
            task_id = task["task_id"]
            gold = tasks[task_id][1]
            for name in products:
                row = capture_product[name](task, gold)
                streams[name].write(json.dumps(row, sort_keys=True).encode() + b"\n")
    if probe_indexed_view:
        # Two fixed, independently bounded full probes bracket every search.
        _opengrok_indexed_view(spec["opengrok"], manifest, view, stage / "opengrok-view-post")
    if native_scope is not None:
        native_after = opengrok_index_scope.collect(
            spec["opengrok"]["native_index_reader"],
            Path(spec["opengrok"]["backend_snapshot"]["root"]),
            native_projects,
            stage / "opengrok-native-after",
        )
        if canonical_json(native_scope) != canonical_json(native_after):
            raise ValueError("OpenGrok native index or reader changed during queries")
    if readonly_service:
        service_config_after = _opengrok_service_config_probe(
            spec["opengrok"], set(native_projects), stage / "opengrok-service-after", capture=True
        )
        if canonical_json(service_config_before) != canonical_json(service_config_after):
            raise ValueError("OpenGrok service configuration changed during queries")
    for name in backend_names:
        snapshot = _backend_snapshot(spec[name])
        _validate_backend_snapshot(spec[name], snapshot)
        _write(stage / "backend" / f"{name}-after.json", canonical_json(snapshot).encode() + b"\n")
        if canonical_json(snapshot) != canonical_json(backend_before[name]):
            raise ValueError(f"{name} backend process, mount or index changed during capture")
    if (
        index_scope is not None
        and sourcegraph_index_scope.verify(
            Path(spec["sourcegraph"]["indexed_scope_receipt"]),
            manifest_raw=manifest_raw,
            release_digest=document["digest"],
            config=spec["sourcegraph"],
            projection=projection,
            snapshot=backend_before["sourcegraph"],
            require_owned_service=True,
        )
        != index_scope
    ):
        raise ValueError("Sourcegraph index scope evidence changed during queries")
    for name in products:
        destination = stage / f"{name}_rows.jsonl"
        # The capture is still in staging; complete native replay runs after
        # capture.json and the final raw inventory are written.
        lexical.product_result(name, destination, tasks, admitted, _native_replay=True)
    if (
        (
            corpus_release.validate(release)
            if bound_release is None
            else bound_release.recheck(release)
        )
        != document
        or _read_control_file(spec_path) != _read_control_file(stage / "spec.json")
        or _read_control_file(Path(spec["suite"])) != suite_raw
        or _read_control_file(Path(spec["query_pack"])) != pack_raw
        or (binary is not None and _sha_file(binary) != binary_sha)
        or _source_hashes() != source_hashes
        or (
            "sourcegraph" in products
            and _projection_binding(spec["sourcegraph"], manifest) != projection
        )
    ):
        raise ValueError(
            "release, live spec, suite, pack, cs binary or producer source changed during capture"
        )
    summary = {
        "schema_version": spec["schema_version"],
        **({"products": list(products)} if spec["schema_version"] == 2 else {}),
        "status": "diagnostic_unqualified",
        "completed_response_boundary": COMPLETED_BOUNDARY,
        "release_digest": document["digest"],
        "binding": binding,
        "tasks": len(tasks),
        "indexed_universe_attested": False,
        "sourcegraph_index_scope": index_scope,
        **({"opengrok_index_scope": native_scope} if native_scope is not None else {}),
        **(
            {"opengrok_query_reader_scope": _opengrok_query_reader_scope(
                spec["opengrok"], native_scope, len(pack["tasks"])
            )}
            if "opengrok" in products and "query_reader_witness" in spec["opengrok"]
            else {}
        ),
        # Disk, configuration and API probes do not observe the loaded reader.
        "opengrok_indexed_universe_attested": False,
        **(
            {
                "opengrok_service_configuration": service_config_before,
                "opengrok_service_loaded_reader_attested": False,
                "opengrok_snapshot_receipt_sha256": _sha_file(
                    Path(spec["opengrok"]["readonly_service"]["snapshot_receipt"])
                ),
            }
            if readonly_service
            else {}
        ),
        "opengrok_indexed_view_probe": (
            "exact_indexed_inventory_and_served_bytes_bracketing_queries"
            if probe_indexed_view
            else "not_requested"
        ),
        "opengrok_indexed_view_files": len(manifest["files"]) if probe_indexed_view else 0,
        "backend_snapshot_sha256": {
            name: _sha_file(stage / "backend" / f"{name}-before.json") for name in backend_names
        },
        "producer_sources_sha256": source_hashes,
        "python_executable_sha256": _sha_file(Path(sys.executable).resolve()),
        "python_version": sys.version.split()[0],
        "server_image_digests_operator_supplied": {
            name: spec[name]["server_image_digest"] for name in products if name != "cs"
        },
        "sourcegraph_max_request_target_bytes": sourcegraph_max_request_target_bytes,
        "cs_binary_sha256": binary_sha,
        "cs_version": version.decode().strip() if version is not None else None,
        "rows_sha256": {name: _sha_file(stage / f"{name}_rows.jsonl") for name in products},
        "raw_capture_sha256": {
            path.relative_to(stage).as_posix(): _sha_file(path)
            for name in (
                *products,
                *(("opengrok-view", "opengrok-view-post") if probe_indexed_view else ()),
                *(("backend",) if backend_names else ()),
                *(
                    ("opengrok-native-before", "opengrok-native-after")
                    if native_scope is not None
                    else ()
                ),
                *(
                    ("opengrok-service-before", "opengrok-service-after")
                    if readonly_service
                    else ()
                ),
            )
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


def _replay_rows(path: Path, tasks: list[dict], replay) -> None:
    """Check row order and completeness with bounded, fully hashed lines."""

    def consume(lines):
        for task, line in zip(tasks, lines, strict=True):
            row = _json(line)
            if row.get("task_id") != task["task_id"]:
                raise ValueError("external capture row order differs")
            replay(task, row)

    RawFile.capture(path).consume_lines(consume)


def verify(root: Path, *, bound_release: BoundRelease | None = None) -> dict:
    """Re-derive every external row from retained native bytes; no live searches."""
    inventory = corpus_release.regular_tree(root)
    spec = _spec(root / "spec.json")
    products = _selected_products(spec)
    summary = _json(_read_control_file(root / "capture.json"))
    fields = {
        "schema_version",
        "status",
        "completed_response_boundary",
        "release_digest",
        "binding",
        "tasks",
        "indexed_universe_attested",
        "sourcegraph_index_scope",
        "opengrok_indexed_universe_attested",
        "opengrok_indexed_view_probe",
        "opengrok_indexed_view_files",
        "backend_snapshot_sha256",
        "producer_sources_sha256",
        "python_executable_sha256",
        "python_version",
        "server_image_digests_operator_supplied",
        "sourcegraph_max_request_target_bytes",
        "cs_binary_sha256",
        "cs_version",
        "rows_sha256",
        "raw_capture_sha256",
        "exclusions",
    }
    if spec["schema_version"] == 2:
        fields.add("products")
    native_reader = spec["opengrok"].get("native_index_reader") if "opengrok" in products else None
    readonly_service = "opengrok" in products and "readonly_service" in spec["opengrok"]
    query_reader_witness = "opengrok" in products and "query_reader_witness" in spec["opengrok"]
    if native_reader is not None:
        fields.add("opengrok_index_scope")
    if query_reader_witness:
        fields.add("opengrok_query_reader_scope")
    if readonly_service:
        fields.update(
            {
                "opengrok_service_configuration",
                "opengrok_service_loaded_reader_attested",
                "opengrok_snapshot_receipt_sha256",
            }
        )
    if (
        set(summary) != fields
        or type(summary.get("schema_version")) is not int
        or summary["schema_version"] != spec["schema_version"]
        or (spec["schema_version"] == 2 and summary.get("products") != list(products))
        or summary.get("status") != "diagnostic_unqualified"
        or summary.get("completed_response_boundary") != COMPLETED_BOUNDARY
        or summary.get("indexed_universe_attested") is not False
        or summary.get("opengrok_indexed_universe_attested") is not False
        or (
            readonly_service and summary.get("opengrok_service_loaded_reader_attested") is not False
        )
        or (
            readonly_service
            and summary.get("opengrok_snapshot_receipt_sha256")
            != _sha_file(Path(spec["opengrok"]["readonly_service"]["snapshot_receipt"]))
        )
        or not isinstance(summary.get("backend_snapshot_sha256"), dict)
        or set(summary["backend_snapshot_sha256"])
        != {name for name in products if name != "cs" and "backend_snapshot" in spec[name]}
        or summary.get("opengrok_indexed_view_probe")
        != (
            "exact_indexed_inventory_and_served_bytes_bracketing_queries"
            if "opengrok" in products and spec["opengrok"].get("indexed_view_probe") == "full"
            else "not_requested"
        )
        or type(summary.get("tasks")) is not int
        or summary["tasks"] <= 0
        or summary.get("producer_sources_sha256") != _source_hashes()
        or summary.get("python_executable_sha256") != _sha_file(Path(sys.executable).resolve())
        or summary.get("python_version") != sys.version.split()[0]
        or summary.get("server_image_digests_operator_supplied")
        != {name: spec[name]["server_image_digest"] for name in products if name != "cs"}
        or summary.get("cs_binary_sha256")
        != (
            _sha_file(Path(spec["cs"]["binary"]).resolve(strict=True)) if "cs" in products else None
        )
        or not isinstance(summary.get("rows_sha256"), dict)
        or set(summary["rows_sha256"]) != set(products)
        or summary.get("exclusions")
        != ["backend_indexed_universe_attestation", "independent_gold", "qualified_speed"]
    ):
        raise ValueError("unsupported capture metadata, claim or runtime identity")
    if "cs" in products:
        code, version, stderr, _ = _process([spec["cs"]["binary"], "--version"], 10)
        if code != 0 or stderr or summary.get("cs_version") != version.decode().strip():
            raise ValueError("unsupported capture metadata: cs version identity differs")
    elif summary.get("cs_version") is not None:
        raise ValueError("unselected cs version identity is present")
    suite_raw = _read_control_file(root / "suite.json")
    pack_raw = _read_control_file(root / "query-pack.json")
    suite, pack = _json(suite_raw), _json(pack_raw)
    admitted = lexical._file_universe(suite, pack)
    tasks = lexical._tasks(suite, pack)
    literal_file_query = (
        suite["tasks"][0].get("evaluation_contract", {}).get("request_mode")
        == query_plan.NATURAL_LANGUAGE_FILE_SEARCH
    )
    if "cs" in products and literal_file_query and summary["cs_version"] != CS_VERIFIED_VERSION:
        raise ValueError("cs natural-language data encoding is verified only for cs 3.2.0")
    if any(re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*", task_id) is None for task_id in tasks):
        raise ValueError("task IDs must be safe filename components")
    release = Path(spec["corpus"]["release_path"])
    document = (
        corpus_release.validate(release)
        if bound_release is None
        else bound_release.recheck(release)
    )
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
    manifest = _json(manifest_raw)
    projection = (
        _projection_binding(spec["sourcegraph"], manifest) if "sourcegraph" in products else None
    )
    if projection is not None:
        spec["sourcegraph"]["projection_revision"] = projection["projection_revision"]
        if canonical_json(
            _json(_read_control_file(root / "sourcegraph-projection.json"))
        ) != canonical_json(projection):
            raise ValueError("Sourcegraph projection differs from retained capture")
    sourcegraph_max_request_target_bytes = None
    if "sourcegraph" in products:
        _, sourcegraph_max_request_target_bytes = _preflight_sourcegraph_request_targets(
            spec["sourcegraph"], pack["tasks"], manifest
        )
    if (
        canonical_json(binding) != canonical_json(_json(_read_control_file(root / "binding.json")))
        or canonical_json(binding) != canonical_json(summary.get("binding"))
        or summary.get("tasks") != len(tasks)
        or summary.get("release_digest") != document["digest"]
        or canonical_json(summary.get("sourcegraph_max_request_target_bytes"))
        != canonical_json(sourcegraph_max_request_target_bytes)
    ):
        raise ValueError("external capture binding differs")
    files = {row["path"]: row["file_sha256"] for row in manifest["files"]}
    view = release / "views" / spec["corpus"]["repository"] / view_name
    probe_indexed_view = (
        "opengrok" in products and spec["opengrok"].get("indexed_view_probe") == "full"
    )
    indexed_batches = _opengrok_index_batches(manifest, view) if probe_indexed_view else []
    if type(summary.get("opengrok_indexed_view_files")) is not int or summary[
        "opengrok_indexed_view_files"
    ] != (len(manifest["files"]) if probe_indexed_view else 0):
        raise ValueError("external indexed view probe count differs")
    expected_raw = set()
    if readonly_service:
        expected_raw.update(
            f"{directory}/{name}"
            for directory in ("opengrok-service-before", "opengrok-service-after")
            for name in (
                "data-root.json",
                "data-root.transport.json",
                "indexed-projects.json",
                "indexed-projects.transport.json",
            )
        )
        expected_raw.update(
            {
                "opengrok-service-before/write-denial.body",
                "opengrok-service-before/write-denial.transport.json",
            }
        )
    if native_reader is not None:
        expected_raw.update(
            f"{directory}/{name}"
            for directory in ("opengrok-native-before", "opengrok-native-after")
            for name in ("stdout", "stderr", "execution.json")
        )
    for task in pack["tasks"]:
        task_id = task["task_id"]
        if "sourcegraph" in products:
            expected_raw.update(
                {f"sourcegraph/{task_id}.stream", f"sourcegraph/{task_id}.transport.json"}
            )
        if "opengrok" in products:
            expected_raw.update({f"opengrok/{task_id}.json", f"opengrok/{task_id}.transport.json"})
        if "cs" in products:
            expected_raw.update(
                {f"cs/{task_id}.json", f"cs/{task_id}.stderr", f"cs/{task_id}.process.json"}
            )
        if (
            "sourcegraph" in products
            and lexical.sourcegraph_capability(task["query"])["status"] == "unsupported"
        ):
            expected_raw.difference_update(
                {f"sourcegraph/{task_id}.stream", f"sourcegraph/{task_id}.transport.json"}
            )
            expected_raw.add(f"sourcegraph/{task_id}.capability.json")
    if probe_indexed_view:
        for probe in ("opengrok-view", "opengrok-view-post"):
            for batch in range(len(indexed_batches)):
                for phase in ("before", "after"):
                    name = f"{probe}/indexed-files-{phase}-b{batch:04d}"
                    expected_raw.update({f"{name}.json", f"{name}.transport.json"})
            for index in range(len(manifest["files"])):
                name = f"{probe}/{index:06d}"
                expected_raw.update({f"{name}.content", f"{name}.transport.json"})
    for name in summary["backend_snapshot_sha256"]:
        expected_raw.update({f"backend/{name}-before.json", f"backend/{name}-after.json"})
    fixed = {
        "spec.json",
        "capture.json",
        "binding.json",
        "suite.json",
        "query-pack.json",
        "manifest.json",
        *(("sourcegraph-projection.json",) if projection is not None else ()),
        *(
            ("sourcegraph-index-scope.json",)
            if "sourcegraph" in products and "indexed_scope_receipt" in spec["sourcegraph"]
            else ()
        ),
        *(f"{name}_rows.jsonl" for name in products),
    }
    if (
        set(inventory) != fixed | expected_raw
        or set(summary.get("raw_capture_sha256", {})) != expected_raw
    ):
        raise ValueError("external capture raw inventory differs")
    for name in expected_raw:
        if _sha_file(root / name) != summary["raw_capture_sha256"][name]:
            raise ValueError("external native bytes differ from capture")
    for name, digest in summary["backend_snapshot_sha256"].items():
        before_path = root / "backend" / f"{name}-before.json"
        after_path = root / "backend" / f"{name}-after.json"
        before = _json(_read_control_file(before_path))
        after = _json(_read_control_file(after_path))
        _validate_backend_snapshot(spec[name], before)
        _validate_backend_snapshot(spec[name], after)
        if digest != _sha_file(before_path) or canonical_json(before) != canonical_json(after):
            raise ValueError("backend snapshot changed during capture or replay")
    index_scope = None
    if "sourcegraph" in products and "indexed_scope_receipt" in spec["sourcegraph"]:
        index_scope = sourcegraph_index_scope.verify(
            Path(spec["sourcegraph"]["indexed_scope_receipt"]),
            manifest_raw=manifest_raw,
            release_digest=document["digest"],
            config=spec["sourcegraph"],
            projection=projection,
            snapshot=_json(_read_control_file(root / "backend/sourcegraph-before.json")),
            require_owned_service=True,
        )
        if canonical_json(
            _json(_read_control_file(root / "sourcegraph-index-scope.json"))
        ) != canonical_json(index_scope):
            raise ValueError("retained Sourcegraph index scope differs from evidence replay")
    if canonical_json(summary["sourcegraph_index_scope"]) != canonical_json(index_scope):
        raise ValueError("Sourcegraph index scope claim differs from evidence replay")
    if native_reader is not None:
        native_projects = opengrok_index_scope.expected_projects(release, document, view_name)
        native_scopes = [
            opengrok_index_scope.replay(
                native_reader,
                Path(spec["opengrok"]["backend_snapshot"]["root"]),
                native_projects,
                root / directory,
                execution_target=root.with_name(root.name + ".staging") / directory,
            )
            for directory in ("opengrok-native-before", "opengrok-native-after")
        ]
        if any(
            canonical_json(scope) != canonical_json(summary["opengrok_index_scope"])
            for scope in native_scopes
        ):
            raise ValueError("OpenGrok native scope differs across queries or replay")
        native_scope = native_scopes[0]
    if query_reader_witness and not opengrok_query_witness.canonical_typed_equal(
        summary["opengrok_query_reader_scope"],
        _opengrok_query_reader_scope(spec["opengrok"], native_scope, len(pack["tasks"])),
    ):
        raise ValueError("OpenGrok query reader scope differs from native replay")
    if readonly_service:
        service_scopes = [
            _opengrok_service_config_probe(
                spec["opengrok"], set(native_projects), root / directory, capture=False
            )
            for directory in ("opengrok-service-before", "opengrok-service-after")
        ]
        if any(
            canonical_json(scope) != canonical_json(summary["opengrok_service_configuration"])
            for scope in service_scopes
        ):
            raise ValueError("OpenGrok readonly service config differs across queries or replay")
        _opengrok_write_denial_probe(
            spec["opengrok"], root / "opengrok-service-before", capture=False
        )
    if probe_indexed_view:
        endpoint = (
            "/api/v1/projects/"
            + urllib.parse.quote(spec["opengrok"]["project"], safe="")
            + "/files"
        )
        for probe in ("opengrok-view", "opengrok-view-post"):
            for batch, (start, end) in enumerate(indexed_batches):
                for phase in ("before", "after"):
                    name = f"{probe}/indexed-files-{phase}-b{batch:04d}"
                    transport = _json(_read_control_file(root / f"{name}.transport.json"))
                    if (
                        set(transport) != {"endpoint", "status", "content_type", "elapsed_ms"}
                        or transport["endpoint"] != endpoint
                        or type(transport["status"]) is not int
                        or type(transport["elapsed_ms"]) not in (int, float)
                        or not math.isfinite(transport["elapsed_ms"])
                        or transport["elapsed_ms"] < 0
                    ):
                        raise ValueError(
                            "OpenGrok indexed file inventory transport metadata differs"
                        )
                    _opengrok_indexed_inventory_response(
                        spec["opengrok"],
                        manifest,
                        transport["status"],
                        transport["content_type"],
                        _read_control_file(root / f"{name}.json"),
                    )
                for index in range(start, end):
                    row = manifest["files"][index]
                    name = f"{probe}/{index:06d}"
                    transport = _json(_read_control_file(root / f"{name}.transport.json"))
                    if (
                        set(transport) != {"path", "status", "content_type", "elapsed_ms"}
                        or transport["path"]
                        != "/" + spec["opengrok"]["project"] + "/" + row["path"]
                        or type(transport["status"]) is not int
                        or type(transport["elapsed_ms"]) not in (int, float)
                        or not math.isfinite(transport["elapsed_ms"])
                        or transport["elapsed_ms"] < 0
                    ):
                        raise ValueError("OpenGrok indexed view transport metadata differs")
                    _opengrok_indexed_view_response(
                        row,
                        view,
                        transport["status"],
                        transport["content_type"],
                        _read_control_file(root / f"{name}.content"),
                    )
    for name in products:
        row_path = root / f"{name}_rows.jsonl"
        if _sha_file(row_path) != summary.get("rows_sha256", {}).get(name):
            raise ValueError("external rows differ from capture")
        # The replay validates the complete native capture below. This nested
        # shape/quality check must not recursively request replay or expose a
        # completed clock as verified before native normalization is checked.
        lexical.product_result(name, row_path, tasks, admitted, _native_replay=True)

        def replay_row(task, row, name=name):
            task_id, gold = task["task_id"], tasks[task["task_id"]][1]
            if (
                name == "sourcegraph"
                and lexical.sourcegraph_capability(task["query"])["status"] == "unsupported"
            ):
                derived = _unsupported_sourcegraph(task, gold)
                if (
                    _json(_read_control_file(root / name / f"{task_id}.capability.json")) != derived
                    or row != derived
                ):
                    raise ValueError("Sourcegraph capability row differs from frozen query support")
                return
            if name == "cs":
                terminal = _json(_read_control_file(root / name / f"{task_id}.process.json"))
                if (
                    set(terminal) != {"argv", "exit_code", "elapsed_ms"}
                    or type(terminal["exit_code"]) is not int
                    or terminal["argv"]
                    != _cs_argv(
                        Path(spec["cs"]["binary"]).resolve(strict=True),
                        task["query"],
                        view,
                        literal_query=literal_file_query,
                    )
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
                    literal_query=literal_file_query,
                )
            else:
                terminal = _json(_read_control_file(root / name / f"{task_id}.transport.json"))
                required_transport = {"status", "content_type", "elapsed_ms"}
                if name == "opengrok" and query_reader_witness:
                    required_transport |= {"request_nonce", "reader_witness_headers"}
                if (
                    set(terminal) != required_transport
                    or type(terminal["status"]) is not int
                ):
                    raise ValueError("HTTP terminal metadata differs")
                if name == "opengrok" and query_reader_witness:
                    nonce = opengrok_query_witness.request_nonce(
                        spec["output_root"], spec["opengrok"], task
                    )
                    if terminal["request_nonce"] != nonce:
                        raise ValueError("OpenGrok query reader request nonce differs")
                    opengrok_query_witness.verify_header(
                        terminal["reader_witness_headers"],
                        nonce=nonce,
                        project=spec["opengrok"]["project"],
                        native_commits={
                            spec["opengrok"]["project"]: native_scope["reader_commits"][
                                spec["opengrok"]["project"]
                            ]
                        },
                    )
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
                        literal_query=literal_file_query,
                    )
            derived["completed_response"] = row.get("completed_response")
            if canonical_json(row) != canonical_json(derived):
                raise ValueError("external row disagrees with retained native response")
            _validate_completed_row(
                row,
                derived["paths"] if name == "cs" else derived["file_paths_top_10"],
            )

        _replay_rows(row_path, pack["tasks"], replay_row)
    if bound_release is not None:
        bound_release.recheck(release)
    return summary


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--spec", type=Path)
    mode.add_argument("--verify", type=Path)
    mode.add_argument("--cs-fuzzy-spec", type=Path)
    mode.add_argument("--verify-cs-fuzzy", type=Path)
    args = parser.parse_args()
    try:
        if args.cs_fuzzy_spec:
            result = capture_cs_fuzzy(args.cs_fuzzy_spec)
        elif args.verify_cs_fuzzy:
            result = verify_cs_fuzzy(args.verify_cs_fuzzy)
        elif args.verify:
            result = verify(args.verify)
        else:
            result = capture(args.spec)
        print(json.dumps(result, sort_keys=True, indent=2))
    except (ValueError, OSError, subprocess.SubprocessError, corpus_release.EvidenceError) as error:
        parser.error(str(error))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
