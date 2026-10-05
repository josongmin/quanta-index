"""Independent live-document goldens and owned-reader replay refusal checks."""

import copy
import hashlib
import json
import struct

import pytest

from tools.benchmark.evidence import RawFile
from tools.benchmark.retrieval import opengrok_index_scope as scope


def stored(name, value, kind="string"):
    result = {
        "name": name,
        "stored": True,
        "indexOptions": "NONE",
        "docValuesType": "NONE",
        "valueKind": kind,
    }
    if kind == "string":
        result["value"] = value
    elif kind == "number":
        result.update(valueDecimal=value, numberClass="java.lang.Integer")
    else:
        result["valueBase64"] = value
    return result


def term(name, value):
    data = value.encode()
    digest = hashlib.sha256(struct.pack(">I", len(data)) + data + struct.pack(">I", 1)).hexdigest()
    return {
        "field": name,
        "distinctTerms": 1,
        "occurrences": 1,
        "termFrequencySha256": digest,
        "frequenciesAvailable": False,
    }


def observation(expected):
    rows = []
    total = 0
    for project, paths in sorted(expected.items()):
        documents = []
        for path in sorted(paths):
            native = f"/{project}/{path}"
            uid = native.replace("/", "\0") + "\0" + "20261003170210646"
            documents.append(([stored("path", native), stored("u", uid)], [term("u", uid)]))
        documents.append(
            (
                [
                    stored("d", f"/{project}"),
                    stored("loc", "7", "number"),
                    stored("numl", "8", "number"),
                ],
                [
                    term("d", f"/{project}"),
                    term("dirpath", "kigpprkqvgihuljvtovtkuglmsilmotnsiujvvqo"),
                ],
            )
        )
        # Fixed product UID and version; serialized settings remain opaque.
        settings_uid = "uthuslvotkgltggqqjmurqojpjpjjkutkujktnkk"
        documents.append(
            (
                [
                    stored("objver", "3", "number"),
                    stored("objser", "rO0ABQ==", "binary"),
                ],
                [term("objuid", settings_uid)],
            )
        )
        # One deleted document slot is absent from the live stream.
        rows.append(
            {
                "kind": "segment",
                "repository": project,
                "segmentName": "_0",
                "leafOrdinal": 0,
                "docBase": 0,
                "maxDoc": len(documents) + 1,
                "numDocs": len(documents),
                "fieldInfo": [],
            }
        )
        for local, (fields, postings) in enumerate(documents, 1):
            rows.append(
                {
                    "kind": "document",
                    "repository": project,
                    "segmentName": "_0",
                    "leafOrdinal": 0,
                    "docLocal": local,
                    "docGlobal": local,
                    "storedFields": fields,
                    "selectedStoredFields": {
                        name: {
                            "present": any(field["name"] == name for field in fields),
                            "values": [field for field in fields if field["name"] == name],
                        }
                        for name in ("path", "u", "type", "t", "project", "date")
                    },
                    "pathStringPresent": any(field["name"] == "path" for field in fields),
                    "indexedFields": postings,
                }
            )
        rows.append(
            {
                "kind": "repository_summary",
                "repository": project,
                "liveDocs": len(documents),
                "pathPresent": len(paths),
                "pathMissing": 2,
                "pathFieldMissing": 2,
            }
        )
        total += len(documents)
    rows.append({"kind": "terminal", "repositories": sorted(expected), "liveDocs": total})
    return rows


def write_rows(path, rows):
    path.write_bytes(b"".join(json.dumps(row).encode() + b"\n" for row in rows))


def fake_execution(expected, *, mutate=None):
    """Simulate Java output; retains the real execute() sidecar wire shape."""

    def run(argv, *, cwd, env, timeout, log_dir):
        assert timeout == 1800
        assert argv[1:3] == ["-Xmx512m", "--class-path"]
        assert not {"JAVA_TOOL_OPTIONS", "JDK_JAVA_OPTIONS", "_JAVA_OPTIONS", "CLASSPATH"} & set(
            env
        )
        rows = observation(expected)
        if mutate is not None:
            mutate(log_dir, rows)
        write_rows(log_dir / "stdout", rows)
        (log_dir / "stderr").write_bytes(b"")
        request = {"argv": argv, "cwd": str(cwd), "timeout_seconds": timeout}
        raw = [RawFile.capture(log_dir / name) for name in ("stdout", "stderr")]
        record = {
            "status": "completed",
            "request": request,
            "command": {**request, "status": "completed", "exit_code": 0, "wall_ms": 1},
            "error_type": None,
            "output_errors": [],
            "raw": [
                {"path": str(ref.path), "sha256": ref.sha256, "bytes": ref.size} for ref in raw
            ],
        }
        (log_dir / "execution.json").write_text(json.dumps(record))

    return run


def reader_config(tmp_path):
    java = tmp_path / "java"
    java.write_text("fixture executable")
    java.chmod(0o755)
    jar = tmp_path / "lucene.jar"
    jar.write_bytes(b"fixture jar")
    return {"java": str(java), "classpath": [str(jar)]}


def test_native_live_documents_accept_deletion_gap_and_multiple_projects(tmp_path):
    expected = {"fixture": {"src/a.rs"}, "other": {"b.go"}}
    path = tmp_path / "native.jsonl"
    write_rows(path, observation(expected))
    result = scope.verify_documents(path, expected)
    assert result["live_documents"] == 6
    assert result["projects"] == {
        name: {"source_files": 1, "directory_documents": 1, "settings_documents": 1}
        for name in expected
    }
    assert result["service_loaded_reader_attested"] is False
    assert result["qualified"] is False


@pytest.mark.parametrize(
    "mutation",
    [
        "missing",
        "duplicate",
        "extra",
        "uid",
        "postings",
        "unknown_aux",
        "settings_version",
        "settings_uid",
        "empty_settings",
        "directory",
        "bool",
        "float",
        "segment",
        "terminal",
        "trailing",
        "selected",
    ],
)
def test_native_live_documents_refuse_independent_mutants(tmp_path, mutation):
    expected = {"fixture": {"src/a.rs"}}
    rows = observation(expected)
    if mutation == "missing":
        del rows[1]
    elif mutation == "duplicate":
        rows.insert(2, copy.deepcopy(rows[1]))
    elif mutation == "extra":
        rows = observation({"fixture": {"src/a.rs", "extra.rs"}})
    elif mutation == "uid":
        rows[1]["storedFields"][1]["value"] = "wrong"
    elif mutation == "postings":
        rows[1]["indexedFields"][0]["termFrequencySha256"] = "0" * 64
    elif mutation == "unknown_aux":
        rows[2]["storedFields"][0]["name"] = "unknown"
    elif mutation == "settings_version":
        rows[3]["storedFields"][0]["valueDecimal"] = "4"
    elif mutation == "settings_uid":
        rows[3]["indexedFields"][0] = term("objuid", "unexpected")
    elif mutation == "empty_settings":
        rows[3]["storedFields"][1]["valueBase64"] = ""
    elif mutation == "directory":
        rows[2]["storedFields"][0]["value"] = "/fixture/../escape"
    elif mutation == "bool":
        rows[1]["docGlobal"] = True
    elif mutation == "float":
        rows[0]["numDocs"] = 3.0
    elif mutation == "segment":
        rows[1]["docLocal"] = 4
    elif mutation == "terminal":
        rows.pop()
    elif mutation == "trailing":
        rows.append(copy.deepcopy(rows[-1]))
    else:
        rows[1]["selectedStoredFields"]["type"]["present"] = True
    path = tmp_path / "native.jsonl"
    write_rows(path, rows)
    with pytest.raises(ValueError):
        scope.verify_documents(path, expected)


def test_native_auxiliary_shape_uses_deployed_directory_parent_and_index_only_uid(tmp_path):
    expected = {"bat": {"src/a.rs"}}
    rows = observation(expected)
    directory = "/bat/src/syntax_mapping"
    rows[2]["storedFields"][0]["value"] = directory
    # Fixed digest observed in deployed OpenGrok 1.14.18, not computed by the verifier.
    rows[2]["indexedFields"] = [
        term("d", directory),
        {
            "field": "dirpath",
            "distinctTerms": 1,
            "occurrences": 1,
            "frequenciesAvailable": False,
            "termFrequencySha256": "2105e2df99f53bff307dec7631992aaa360535165cac242644de76967ea8c854",
        },
    ]
    path = tmp_path / "native.jsonl"
    write_rows(path, rows)
    assert scope.verify_documents(path, expected)["projects"]["bat"]["directory_documents"] == 1
    rows[2]["indexedFields"][1]["termFrequencySha256"] = "0" * 64
    write_rows(path, rows)
    with pytest.raises(ValueError, match="exact indexed term"):
        scope.verify_documents(path, expected)


def test_native_reader_collect_replay_rejects_failed_command_and_output_drift(
    tmp_path, monkeypatch
):
    config = reader_config(tmp_path)
    index = tmp_path / "index"
    index.mkdir()
    expected = {"fixture": {"a.rs"}}
    monkeypatch.setenv("JAVA_TOOL_OPTIONS", "untrusted options")
    monkeypatch.setattr(scope, "execute", fake_execution(expected))
    target = tmp_path / "logs"
    result = scope.collect(config, index, expected, target)
    renamed = tmp_path / "renamed"
    target.rename(renamed)
    assert scope.replay(config, index, expected, renamed, execution_target=target) == result
    record_path = renamed / "execution.json"
    original = record_path.read_text()
    record = json.loads(original)
    for mutation in (False, 1, 0.0):
        record["command"]["exit_code"] = mutation
        record_path.write_text(json.dumps(record))
        with pytest.raises(ValueError, match="command"):
            scope.replay(config, index, expected, renamed, execution_target=target)
    record_path.write_text(original)
    (renamed / "stdout").write_bytes(b"altered")
    with pytest.raises(ValueError, match="custody"):
        scope.replay(config, index, expected, renamed, execution_target=target)


def test_native_reader_refuses_identity_drift_during_collection(tmp_path, monkeypatch):
    config = reader_config(tmp_path)
    index = tmp_path / "index"
    index.mkdir()
    expected = {"fixture": {"a.rs"}}
    original = fake_execution(expected)

    def run(*args, **kwargs):
        original(*args, **kwargs)
        (tmp_path / "lucene.jar").write_bytes(b"changed jar")

    monkeypatch.setattr(scope, "execute", run)
    with pytest.raises(ValueError, match="changed during collection"):
        scope.collect(config, index, expected, tmp_path / "logs")
