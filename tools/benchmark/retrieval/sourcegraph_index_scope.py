"""Replay a bounded Zoekt path/content audit against a query capture's index.

This proves stored document bytes and the complete selected repository path
set. It does not prove every posting, relevance labels, or fair performance.
"""

from __future__ import annotations

import math
import re
from pathlib import Path

from tools.benchmark.evidence import RawFile, _read_control_file, canonical_json, parse_json
from tools.benchmark.retrieval import sourcegraph

SCOPE = "indexed_path_inventory_and_native_stored_document_bytes"
MAX_STREAM_BYTES = 16 * 1024 * 1024


def _absolute(value: object) -> Path:
    if not isinstance(value, str):
        raise ValueError("index scope path must be a canonical absolute string")
    path = Path(value)
    if not path.is_absolute() or str(path) != value or ".." in path.parts:
        raise ValueError("index scope path must be canonical absolute")
    return path


def verify(
    receipt_path: Path,
    *,
    manifest_raw: bytes,
    release_digest: str,
    config: dict,
    projection: dict,
    snapshot: dict,
) -> dict:
    try:
        return _verify(
            receipt_path,
            manifest_raw=manifest_raw,
            release_digest=release_digest,
            config=config,
            projection=projection,
            snapshot=snapshot,
        )
    except (KeyError, TypeError, UnicodeDecodeError) as exc:
        raise ValueError("malformed index scope evidence") from exc


def _verify(
    receipt_path: Path,
    *,
    manifest_raw: bytes,
    release_digest: str,
    config: dict,
    projection: dict,
    snapshot: dict,
) -> dict:
    """Re-derive one repository scope; never trust a receipt's success flag."""
    commitments: dict[str, RawFile] = {}

    def raw(path: Path) -> RawFile:
        item = RawFile.capture(path)
        commitments[str(item.path)] = item
        return item

    def read(path: Path, limit: int | None = None) -> bytes:
        item = raw(path)
        value = _read_control_file(path, max_bytes=limit)
        if sourcegraph.sha256(value) != item.sha256.removeprefix("sha256:"):
            raise ValueError("index scope bytes changed during read")
        return value

    def document(path: Path) -> dict:
        value = parse_json(read(path).decode("utf-8"))
        if not isinstance(value, dict):
            raise ValueError("index scope document must be an object")
        return value

    receipt = document(receipt_path)
    if set(receipt) != {
        "schema",
        "backend",
        "scope",
        "qualified_comparison",
        "release_digest",
        "repository",
        "repository_commit",
        "corpus_manifest_sha256",
        "files",
        "native_index_inventory_sha256",
        "native_runtime",
        "native_binary_sha256",
        "native_capture_root",
        "native_rows_sha256",
        "path_inventory_proof",
        "limitations",
    }:
        raise ValueError("index scope receipt keys differ")
    manifest = parse_json(manifest_raw.decode("utf-8"))
    files = sourcegraph._files(manifest["files"], "index scope manifest")
    repo = receipt["repository"]
    if (
        receipt["schema"] != "external_index_scope_v1"
        or receipt["backend"] != "sourcegraph_zoekt"
        or receipt["scope"] != SCOPE
        or receipt["qualified_comparison"] is not False
        or receipt["release_digest"] != release_digest
        or receipt["corpus_manifest_sha256"] != sourcegraph.sha256(manifest_raw)
        or receipt["repository_commit"] != manifest["repository_commit"]
        or receipt["files"] != files
        or not isinstance(repo, str)
        or re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*", repo) is None
        or config["repository"] != "benchmark/" + repo
        or projection is None
        or projection["source_revision"] != manifest["repository_commit"]
        or "backend_snapshot" not in config
        or receipt["limitations"]
        != [
            "No assertion of every posting's correctness",
            "No human qrels claim",
            "No speed qualification",
        ]
    ):
        raise ValueError("index scope receipt differs from selected source or contract")
    root = _absolute(receipt["native_capture_root"])
    before = document(root / "native-index-before.json")
    after = document(root / "native-index-after.json")
    if (
        before != after
        or set(before) != {"runtime", "files"}
        or commitments[str(root / "native-index-before.json")].sha256
        != "sha256:" + receipt["native_index_inventory_sha256"]
        or before["files"] != snapshot["files"]
        or before["runtime"] != receipt["native_runtime"]
    ):
        raise ValueError("native content audit index differs from query capture")
    native = before["runtime"]
    runtime = snapshot["runtime"]
    mounts = native.get("mounts")
    if (
        native.get("container_id") != runtime["container_id"]
        or native.get("image_id") != "sha256:" + runtime["image_sha256"]
        or type(native.get("pid")) is not int
        or native["pid"] != runtime["pid"]
        or native.get("started_at") != runtime["started_at"]
        or type(native.get("restart_count")) is not int
        or native["restart_count"] != runtime["restart_count"]
        or not isinstance(mounts, list)
        or len(mounts) != 1
        or not isinstance(mounts[0], dict)
        or mounts[0].get("Type") != "bind"
        or mounts[0].get("RW") is not False
        or mounts[0].get("Source") != runtime["mount_source"]
        or mounts[0].get("Destination") != runtime["mount_destination"]
    ):
        raise ValueError("native content audit runtime differs from query capture")
    precommit = document(root / "precommit.json")
    result = document(root / "result.json")
    cleanup = document(root / "owned-probe-cleanup.json")
    if (
        precommit["native_runtime"] != native
        or precommit["native_binary_sha256"] != receipt["native_binary_sha256"]
        or raw(root / "zoekt-webserver").sha256 != "sha256:" + receipt["native_binary_sha256"]
        or cleanup["deployed_binary_sha256"] != receipt["native_binary_sha256"]
        or cleanup["only_owned_reader_stopped"] is not True
        or raw(root / "native-worker.py").sha256 != "sha256:" + precommit["worker_sha256"]
        or raw(root / "probe_native_contents.py").sha256 != "sha256:" + precommit["script_sha256"]
        or result["status"] != "VERIFIED"
        or result["qualified"] is not False
        or result["bindings_unchanged"] is not True
        or result["failures"] != []
        or type(result["worker_exit"]) is not int
        or result["worker_exit"] != 0
        or result["release_digest"] != release_digest
        or result["rows_sha256"] != receipt["native_rows_sha256"]
    ):
        raise ValueError("native content audit custody differs")
    rows = raw(root / "native-rows.jsonl")
    if rows.sha256 != "sha256:" + receipt["native_rows_sha256"]:
        raise ValueError("native content audit row digest differs")
    expected = {row["path"]: row["file_sha256"] for row in files}
    seen: set[tuple[str, str]] = set()
    selected: set[str] = set()

    def consume(lines):
        for line in lines:
            row = parse_json(line.decode("utf-8"))
            if not isinstance(row, dict) or set(row) != {
                "repository",
                "path",
                "file_sha256",
                "actual_sha256",
                "http_status",
                "bytes",
                "matches",
                "seconds",
            }:
                raise ValueError("native content audit row shape differs")
            sourcegraph._path(row["path"])
            if not isinstance(row["repository"], str):
                raise ValueError("native content audit repository differs")
            key = (row["repository"], row["path"])
            if key in seen:
                raise ValueError("native content audit duplicate row")
            seen.add(key)
            if (
                type(row["http_status"]) is not int
                or row["http_status"] != 200
                or row["matches"] is not True
                or type(row["bytes"]) is not int
                or row["bytes"] < 0
                or type(row["seconds"]) not in (int, float)
                or not math.isfinite(row["seconds"])
                or row["seconds"] < 0
            ):
                raise ValueError("native content audit unsuccessful row")
            if row["repository"] == repo:
                if expected.get(row["path"]) != row["file_sha256"]:
                    raise ValueError("native content audit path or manifest digest differs")
                body = raw(root / "native-file-bodies" / repo / row["path"])
                if (
                    body.sha256 != "sha256:" + row["actual_sha256"]
                    or row["actual_sha256"] != expected[row["path"]]
                    or body.size != row["bytes"]
                ):
                    raise ValueError("native content audit stored bytes differ")
                selected.add(row["path"])

    rows.consume_lines(consume)
    if (
        selected != set(expected)
        or type(precommit["tasks"]) is not int
        or len(seen) != precommit["tasks"]
        or any(
            type(result[k]) is not int or result[k] != len(seen)
            for k in ("expected_files", "observed_files", "matched_files")
        )
    ):
        raise ValueError("native content audit incomplete path coverage")
    manifest_inputs = [
        path for path in precommit["manifest_sha256"] if _absolute(path).parent.name == repo
    ]
    if len(manifest_inputs) != 1:
        raise ValueError("native content audit manifest input is absent or duplicated")
    manifest_path = _absolute(manifest_inputs[0])
    if read(manifest_path) != manifest_raw or precommit["manifest_sha256"][
        str(manifest_path)
    ] != sourcegraph.sha256(manifest_raw):
        raise ValueError("native content audit manifest input differs")
    paths_path = _absolute(receipt["path_inventory_proof"])
    paths = document(paths_path)
    path_before = document(paths_path.parent / "native-index-before.json")
    path_after = document(paths_path.parent / "native-index-after.json")
    if (
        paths["qualified"] is not False
        or paths["status"] != "path_index_observed"
        or paths["release_digest"] != release_digest
        or paths["server_image_digest"] != config["server_image_digest"]
        or path_before != snapshot
        or path_after != snapshot
        or paths["native_index_sha256"] != snapshot["tree_sha256"]
    ):
        raise ValueError("indexed path audit differs from query capture")
    selected_rows = [row for row in paths["repositories"] if row["repository"] == repo]
    if len(selected_rows) != 1:
        raise ValueError("indexed path audit repository is absent or duplicated")
    path_row = selected_rows[0]
    revision = projection["projection_revision"]
    stream = read(paths_path.parent / f"{repo}.stream", MAX_STREAM_BYTES)
    if (
        path_row["raw_stream_sha256"] != sourcegraph.sha256(stream)
        or path_row["source_commit"] != manifest["repository_commit"]
        or path_row["projection_commit"] != revision
        or type(path_row["files"]) is not int
        or path_row["files"] != len(files)
        or path_row["query"]
        != f".* repo:^benchmark/{repo}$ rev:{revision} type:path patternType:regexp count:all"
    ):
        raise ValueError("indexed path audit request identity differs")
    events = sourcegraph._events(stream)
    found: list[str] = []
    progress = None
    for index, (kind, data) in enumerate(events):
        if kind == "done":
            if index != len(events) - 1 or data != {}:
                raise ValueError("indexed path audit invalid terminal event")
        elif kind == "alert":
            raise ValueError("indexed path audit stream alert")
        elif kind == "progress":
            if (
                not isinstance(data, dict)
                or data.get("skipped")
                or type(data.get("done")) is not bool
                or type(data.get("matchCount")) is not int
                or data["matchCount"] < len(found)
                or (
                    progress is not None
                    and (progress["done"] or data["matchCount"] < progress["matchCount"])
                )
            ):
                raise ValueError("indexed path audit incomplete progress")
            progress = data
        elif kind == "matches":
            if (
                not isinstance(data, list)
                or not data
                or (progress is not None and progress["done"])
            ):
                raise ValueError("indexed path audit invalid matches")
            for hit in data:
                if (
                    not isinstance(hit, dict)
                    or hit.get("type") != "path"
                    or hit.get("repository") != config["repository"]
                    or hit.get("commit") != revision
                ):
                    raise ValueError("indexed path audit hit identity differs")
                found.append(sourcegraph._path(hit.get("path")))
    if (
        events[-1][0] != "done"
        or progress is None
        or progress["done"] is not True
        or progress["matchCount"] != path_row["native_match_count"]
        or progress["matchCount"] < len(found)
        or len(found) != len(set(found))
        or set(found) != set(expected)
    ):
        raise ValueError("indexed path audit incomplete or foreign inventory")
    for item in commitments.values():
        if RawFile.capture(item.path) != item:
            raise ValueError("index scope evidence changed during replay")
    return {
        "scope": SCOPE,
        "files": len(files),
        "receipt_sha256": commitments[str(receipt_path.absolute())].sha256,
        "evidence_sha256": "sha256:"
        + sourcegraph.sha256(
            canonical_json(
                {
                    path: {"sha256": item.sha256, "bytes": item.size}
                    for path, item in sorted(commitments.items())
                }
            ).encode()
        ),
        "backend_tree_sha256": snapshot["tree_sha256"],
        "projection_revision": revision,
        "native_binary_sha256": receipt["native_binary_sha256"],
    }
