"""Bounded OpenGrok disk-index observation and replay of every live document.

The reader observes a frozen Lucene index, not the server's loaded reader.
Neither collector nor verifier promotes this to service/quality qualification.
Serialized analysis settings remain opaque; no object deserialization is used.
"""

from __future__ import annotations

import base64
import hashlib
import json
import os
import re
import struct
from pathlib import Path

from tools.benchmark.evidence import RawFile, canonical_json, file_digest
from tools.benchmark.producer_execution import execute

READER = Path(__file__).with_name("native") / "FullLiveDocuments.java"
MAX_BYTES = 256 * 1024 * 1024
MAX_LINE = 4 * 1024 * 1024
MAX_DOCUMENTS = 4 * 1024 * 1024
SETTINGS_UID = "uthuslvotkgltggqqjmurqojpjpjjkutkujktnkk"
SELECTED = {"path", "u", "type", "t", "project", "date"}


def _require(ok: bool, reason: str) -> None:
    if not ok:
        raise ValueError(reason)


def _json(raw: bytes) -> dict:
    def pairs(items):
        result = {}
        for key, value in items:
            _require(key not in result, "duplicate native JSON key")
            result[key] = value
        return result

    def constant(_value):
        raise ValueError("nonfinite native JSON number")

    value = json.loads(raw, object_pairs_hook=pairs, parse_constant=constant)
    _require(type(value) is dict, "native row must be an object")
    return value


def reader_identity(config: dict) -> dict:
    _require(
        type(config) is dict and set(config) == {"java", "classpath"}, "native reader keys differ"
    )
    jars = config["classpath"]
    _require(type(jars) is list and 0 < len(jars) <= 128, "native reader classpath bound differs")
    _require(
        all(type(p) is str for p in jars) and len(set(jars)) == len(jars),
        "native reader jars repeat",
    )
    result = {}
    for name, paths in (("java", [config["java"]]), ("classpath", jars)):
        rows = []
        for text in paths:
            _require(type(text) is str, "native reader path must be a string")
            path = Path(text)
            _require(
                path.is_absolute() and path.resolve(strict=True) == path and path.is_file(),
                "native reader requires canonical regular files",
            )
            _require(name != "java" or os.access(path, os.X_OK), "native Java must be executable")
            _require(
                name != "classpath" or path.suffix == ".jar", "native classpath must contain jars"
            )
            digest, size = file_digest(path)
            rows.append({"path": text, "sha256": digest, "bytes": size})
        result[name] = rows[0] if name == "java" else rows
    result["source_sha256"] = file_digest(READER)[0]
    return result


def expected_projects(release: Path, document: dict, view: str) -> dict[str, set[str]]:
    projects = {}
    for repo in document["repositories"]:
        name = repo["recipe"]["name"]
        _require(
            name not in projects and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*", name) is not None,
            "native project identity differs",
        )
        _require(view in repo["views"], "native index release view missing")
        manifest = _json((release / repo["views"][view]["manifest"]).read_bytes())
        paths = [row["path"] for row in manifest["files"]]
        _require(len(set(paths)) == len(paths), "native expected paths repeat")
        projects[name] = set(paths)
    _require(bool(projects), "native project universe is empty")
    return projects


def _integer(value: object) -> bool:
    return type(value) is int and 0 <= value <= MAX_DOCUMENTS


def _text(fields: dict, name: str) -> str:
    values = fields.get(name, [])
    _require(
        len(values) == 1 and values[0]["valueKind"] == "string",
        "native string field missing or repeated",
    )
    return values[0]["value"]


def _one_term(indexed: dict, field: str, value: str) -> None:
    raw = value.encode("utf-8")
    expected = hashlib.sha256(struct.pack(">I", len(raw)) + raw + struct.pack(">I", 1)).hexdigest()
    term = indexed.get(field)
    _require(
        type(term) is dict
        and term["distinctTerms"] == 1
        and term["occurrences"] == 1
        and term["frequenciesAvailable"] is False
        and term["termFrequencySha256"] == expected,
        "native stored value and exact indexed term differ",
    )


def verify_documents(path: Path, expected: dict[str, set[str]]) -> dict:
    """Replay counts, identities, UID postings and every source/auxiliary role."""
    _require(
        bool(expected)
        and all(type(name) is str and type(paths) is set for name, paths in expected.items()),
        "expected native projects differ",
    )
    raw = RawFile.capture(path)
    _require(raw.size <= MAX_BYTES, "native document stream exceeds bound")
    segments = {}
    seen = {name: set() for name in expected}
    sources = {name: set() for name in expected}
    directories = {name: set() for name in expected}
    settings = {name: 0 for name in expected}
    closed = set()
    terminal = False
    total = 0

    def consume(lines):
        nonlocal terminal, total
        for line in lines:
            row = _json(line)
            kind = row.get("kind")
            _require(not terminal, "native rows follow terminal")
            if kind == "terminal":
                _require(
                    set(row) == {"kind", "repositories", "liveDocs"}
                    and row["repositories"] == sorted(expected)
                    and closed == set(expected)
                    and _integer(row["liveDocs"])
                    and row["liveDocs"] == total,
                    "native terminal is partial or differs",
                )
                terminal = True
                continue
            name = row.get("repository")
            _require(
                type(name) is str and name in expected and name not in closed,
                "unknown or already completed native project",
            )
            if kind == "segment":
                _require(
                    set(row)
                    == {
                        "kind",
                        "repository",
                        "segmentName",
                        "leafOrdinal",
                        "docBase",
                        "maxDoc",
                        "numDocs",
                        "fieldInfo",
                    },
                    "native segment keys differ",
                )
                _require(
                    type(row["segmentName"]) is str
                    and bool(row["segmentName"])
                    and all(
                        _integer(row[k]) for k in ("leafOrdinal", "docBase", "maxDoc", "numDocs")
                    )
                    and row["numDocs"] <= row["maxDoc"],
                    "native segment counts differ",
                )
                key = (name, row["segmentName"])
                ordinal = sorted((v["leafOrdinal"] for k, v in segments.items() if k[0] == name))
                base = sum(v["maxDoc"] for k, v in segments.items() if k[0] == name)
                _require(
                    key not in segments
                    and row["leafOrdinal"] == len(ordinal)
                    and row["docBase"] == base
                    and type(row["fieldInfo"]) is list,
                    "native segment identity overlaps or repeats",
                )
                segments[key] = {**row, "seen": set()}
                continue
            if kind == "repository_summary":
                _require(
                    set(row)
                    == {
                        "kind",
                        "repository",
                        "liveDocs",
                        "pathPresent",
                        "pathMissing",
                        "pathFieldMissing",
                    }
                    and all(
                        _integer(row[k])
                        for k in ("liveDocs", "pathPresent", "pathMissing", "pathFieldMissing")
                    ),
                    "native summary shape differs",
                )
                count = len(seen[name])
                _require(
                    sources[name] == expected[name]
                    and settings[name] == 1
                    and row["liveDocs"] == count
                    and row["pathPresent"] == len(sources[name])
                    and row["pathMissing"] == row["pathFieldMissing"] == count - len(sources[name]),
                    "native sources or repository counts differ",
                )
                _require(
                    all(
                        len(v["seen"]) == v["numDocs"] for k, v in segments.items() if k[0] == name
                    ),
                    "native segment live documents are incomplete",
                )
                closed.add(name)
                continue
            _require(
                kind == "document"
                and set(row)
                == {
                    "kind",
                    "repository",
                    "segmentName",
                    "leafOrdinal",
                    "docLocal",
                    "docGlobal",
                    "storedFields",
                    "selectedStoredFields",
                    "pathStringPresent",
                    "indexedFields",
                },
                "unknown native document shape",
            )
            key = (name, row["segmentName"])
            _require(
                key in segments
                and all(_integer(row[k]) for k in ("leafOrdinal", "docLocal", "docGlobal")),
                "native document identity differs",
            )
            segment = segments[key]
            local, global_id = row["docLocal"], row["docGlobal"]
            _require(
                local < segment["maxDoc"]
                and global_id == segment["docBase"] + local
                and row["leafOrdinal"] == segment["leafOrdinal"]
                and global_id not in seen[name],
                "native document is repeated or outside segment",
            )
            segment["seen"].add(local)
            seen[name].add(global_id)
            total += 1
            _require(total <= MAX_DOCUMENTS, "native document count exceeds bound")
            _require(
                type(row["storedFields"]) is list and type(row["indexedFields"]) is list,
                "native field arrays differ",
            )
            fields, indexed = {}, {}
            for field in row["storedFields"]:
                _require(
                    type(field) is dict
                    and {"name", "stored", "indexOptions", "docValuesType", "valueKind"}
                    <= set(field)
                    and type(field["name"]) is str
                    and field["stored"] is True,
                    "native stored field differs",
                )
                kind = field["valueKind"]
                _require(kind in ("string", "number", "binary"), "unknown native stored value type")
                if kind == "string":
                    _require(type(field.get("value")) is str, "native string is malformed")
                elif kind == "number":
                    _require(
                        type(field.get("valueDecimal")) is str
                        and re.fullmatch(r"[0-9]+", field["valueDecimal"]) is not None
                        and field.get("numberClass") in ("java.lang.Integer", "java.lang.Long"),
                        "native numeric value differs",
                    )
                else:
                    _require(type(field.get("valueBase64")) is str, "native binary value differs")
                    base64.b64decode(field["valueBase64"], validate=True)
                fields.setdefault(field["name"], []).append(field)
            _require(
                type(row["selectedStoredFields"]) is dict
                and set(row["selectedStoredFields"]) == SELECTED,
                "native selected field keys differ",
            )
            for field in SELECTED:
                _require(
                    canonical_json(row["selectedStoredFields"][field])
                    == canonical_json(
                        {"present": bool(fields.get(field)), "values": fields.get(field, [])}
                    ),
                    "native selected fields differ from full stored values",
                )
            for field in row["indexedFields"]:
                _require(
                    type(field) is dict
                    and set(field)
                    == {
                        "field",
                        "distinctTerms",
                        "occurrences",
                        "termFrequencySha256",
                        "frequenciesAvailable",
                    }
                    and type(field["field"]) is str
                    and field["field"] not in indexed
                    and _integer(field["distinctTerms"])
                    and _integer(field["occurrences"])
                    and type(field["frequenciesAvailable"]) is bool
                    and type(field["termFrequencySha256"]) is str
                    and re.fullmatch(r"[0-9a-f]{64}", field["termFrequencySha256"]) is not None,
                    "native posting summary differs",
                )
                indexed[field["field"]] = field
            _require(
                type(row["pathStringPresent"]) is bool
                and row["pathStringPresent"] == ("path" in fields),
                "native path presence differs",
            )
            if "path" in fields:
                native_path, uid = _text(fields, "path"), _text(fields, "u")
                prefix = "/" + name + "/"
                _require(native_path.startswith(prefix), "native source project differs")
                _require(
                    _text(fields, "project") == "/" + name,
                    "native stored source project differs",
                )
                source = native_path[len(prefix) :]
                _require(
                    source in expected[name] and source not in sources[name],
                    "native source is missing, extra or duplicated",
                )
                pieces = uid.rsplit("\x00", 1)
                _require(
                    len(pieces) == 2
                    and pieces[0].replace("\x00", "/") == native_path
                    and re.fullmatch(r"[0-9]{17}", pieces[1]) is not None,
                    "native UID does not identify source path",
                )
                _one_term(indexed, "u", uid)
                sources[name].add(source)
            elif set(fields) == {"d", "loc", "numl"}:
                directory = _text(fields, "d")
                _require(
                    (
                        directory == "/"
                        or directory == "/" + name
                        or directory.startswith("/" + name + "/")
                    )
                    and directory not in directories[name]
                    and set(indexed) == {"d", "dirpath"},
                    "native directory role differs",
                )
                _require(
                    directory == "/"
                    or all(
                        piece and piece not in {".", ".."}
                        for piece in directory.split("/")[1:]
                    ),
                    "native directory path is noncanonical",
                )
                _require(
                    all(
                        len(fields[k]) == 1 and fields[k][0]["valueKind"] == "number"
                        for k in ("loc", "numl")
                    ),
                    "native directory counters differ",
                )
                _one_term(indexed, "d", directory)
                # NumLinesLOCAccessor indexes the parent, normalized by QueryBuilder.
                parent = directory.rsplit("/", 1)[0]
                normalized_path = "" if directory == "/" else parent.rstrip("/") + "/"
                digest = hashlib.sha1(
                    normalized_path.encode("utf-8"), usedforsecurity=False
                ).digest()
                normalized = "".join(
                    chr(103 + (byte >> 4)) + chr(103 + (byte & 15)) for byte in digest
                )
                _one_term(indexed, "dirpath", normalized)
                directories[name].add(directory)
            elif set(fields) == {"objver", "objser"}:
                _require(
                    len(fields["objver"]) == 1
                    and fields["objver"][0]["valueKind"] == "number"
                    and fields["objver"][0]["valueDecimal"] == "3"
                    and len(fields["objser"]) == 1
                    and fields["objser"][0]["valueKind"] == "binary"
                    and set(indexed) == {"objuid"},
                    "native settings role differs",
                )
                _require(
                    bool(fields["objser"][0]["valueBase64"]), "native settings payload is empty"
                )
                _one_term(indexed, "objuid", SETTINGS_UID)
                settings[name] += 1
                _require(settings[name] == 1, "native settings document repeats")
            else:
                raise ValueError("unknown native pathless document role")
        _require(terminal, "native document stream has no complete terminal")

    raw.consume_lines(consume, max_line_bytes=MAX_LINE)
    return {
        "scope": "readonly_disk_live_documents_and_uid_postings",
        "raw_sha256": raw.sha256,
        "live_documents": total,
        "projects": {
            name: {
                "source_files": len(sources[name]),
                "directory_documents": len(directories[name]),
                "settings_documents": settings[name],
            }
            for name in sorted(expected)
        },
        "service_loaded_reader_attested": False,
        "qualified": False,
    }


def _argv(config: dict, index: Path, expected: dict[str, set[str]]) -> list[str]:
    _require(
        index.is_absolute() and index.resolve(strict=True) == index and index.is_dir(),
        "native index root must be canonical",
    )
    return [
        config["java"],
        "-Xmx512m",
        "--class-path",
        os.pathsep.join(config["classpath"]),
        str(READER),
        str(index),
        ",".join(sorted(expected)),
    ]


def replay(
    config: dict,
    index: Path,
    expected: dict[str, set[str]],
    target: Path,
    *,
    execution_target: Path | None = None,
) -> dict:
    """Validate owned execution and stream without executing Java or deserializing objects."""
    identity = reader_identity(config)
    argv = _argv(config, index, expected)
    record = _json(RawFile.capture(target / "execution.json").read_control())
    request = {"argv": argv, "cwd": str(READER.parent), "timeout_seconds": 1800}
    _require(
        set(record) == {"status", "command", "request", "error_type", "output_errors", "raw"}
        and record["status"] == "completed"
        and canonical_json(record["request"]) == canonical_json(request)
        and record["error_type"] is None
        and record["output_errors"] == [],
        "native execution is incomplete or differs",
    )
    command = record["command"]
    _require(
        type(command) is dict
        and set(command) == set(request) | {"status", "exit_code", "wall_ms"}
        and command["status"] == "completed"
        and type(command["exit_code"]) is int
        and command["exit_code"] == 0
        and type(command["wall_ms"]) is int
        and command["wall_ms"] >= 0
        and canonical_json({key: command[key] for key in request}) == canonical_json(request),
        "native execution command failed or differs",
    )
    logs = execution_target or target
    refs = []
    for name in ("stdout", "stderr"):
        raw = RawFile.capture(target / name)
        refs.append({"path": str(logs / name), "sha256": raw.sha256, "bytes": raw.size})
    _require(
        canonical_json(record["raw"]) == canonical_json(refs),
        "native execution output custody differs",
    )
    result = verify_documents(target / "stdout", expected)
    _require(
        canonical_json(reader_identity(config)) == canonical_json(identity),
        "native reader changed during replay",
    )
    return {**result, "reader": identity}


def collect(config: dict, index: Path, expected: dict[str, set[str]], target: Path) -> dict:
    identity = reader_identity(config)
    target.mkdir(parents=True)
    env = {
        key: value
        for key, value in os.environ.items()
        if key not in {"JAVA_TOOL_OPTIONS", "JDK_JAVA_OPTIONS", "_JAVA_OPTIONS", "CLASSPATH"}
    }
    execute(
        _argv(config, index, expected), cwd=READER.parent, env=env, timeout=1800, log_dir=target
    )
    result = replay(config, index, expected, target)
    _require(
        canonical_json(reader_identity(config)) == canonical_json(identity),
        "native reader changed during collection",
    )
    return result
