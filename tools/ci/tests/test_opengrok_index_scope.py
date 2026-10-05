"""Independent live-document goldens and owned-reader replay refusal checks."""

import copy
import hashlib
import json
import struct
import zipfile

import pytest

from tools.benchmark.evidence import RawFile
from tools.benchmark.retrieval import live_lexical_external as live
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
            documents.append((
                [stored("path", native), stored("u", uid), stored("project", f"/{project}")],
                [term("u", uid)],
            ))
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
        names = sorted({field["name"] for fields, _ in documents for field in fields} | {field["field"] for _, postings in documents for field in postings})
        indexed = {field["field"] for _, postings in documents for field in postings}
        field_info = [{"name": name, "indexOptions": "DOCS" if name in indexed else "NONE", "docValuesType": "NONE", "hasVectors": False, "hasNorms": False} for name in names]
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
                "fieldInfo": field_info,
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
        "project",
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
    elif mutation == "project":
        rows[1]["storedFields"][2]["value"] = "/wrong"
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


@pytest.mark.parametrize("mutation", ["field_info_boolean", "field_info_duplicate", "field_info_index", "field_info_docvalues", "missing_uid_info", "stored_type", "stored_unknown", "unindexed_postings"])
def test_native_decoder_refuses_impossible_lucene_field_metadata(tmp_path, mutation):
    expected = {"fixture": {"src/a.rs"}}
    rows = observation(expected)
    infos = rows[0]["fieldInfo"]
    if mutation == "field_info_boolean":
        infos[0]["hasNorms"] = 0
    elif mutation == "field_info_duplicate":
        infos.append(copy.deepcopy(infos[0]))
    elif mutation == "field_info_index":
        infos[0]["indexOptions"] = True
    elif mutation == "field_info_docvalues":
        infos[0]["docValuesType"] = "unknown"
    elif mutation == "missing_uid_info":
        infos[:] = [info for info in infos if info["name"] != "u"]
    elif mutation == "stored_type":
        rows[1]["storedFields"][0]["indexOptions"] = True
    elif mutation == "stored_unknown":
        rows[1]["storedFields"][0]["unexpected"] = "extra"
    else:
        rows[1]["indexedFields"].append(term("path", "/fixture/src/a.rs"))
    path = tmp_path / "native.jsonl"
    write_rows(path, rows)
    with pytest.raises(ValueError, match="field|posting"):
        scope.verify_documents(path, expected)


def test_native_auxiliary_real_root_directory_and_parent_postings(tmp_path):
    expected = {"bat": {"src/a.rs"}}
    rows = observation(expected)
    # Independent digests observed in the deployed OpenGrok 1.14.18 index.
    rows[2]["storedFields"][0]["value"] = "/"
    rows[2]["indexedFields"] = [
        term("d", "/"),
        {
            "field": "dirpath",
            "distinctTerms": 1,
            "occurrences": 1,
            "frequenciesAvailable": False,
            "termFrequencySha256": (
                "992a0d7d116d60d629c8544701068f04e7566929f57ab106cc04bfe6548082e9"
            ),
        },
    ]
    path = tmp_path / "native.jsonl"
    write_rows(path, rows)
    assert scope.verify_documents(path, expected)["projects"]["bat"]["directory_documents"] == 1
    rows[2]["indexedFields"][1]["termFrequencySha256"] = "0" * 64
    write_rows(path, rows)
    with pytest.raises(ValueError, match="exact indexed term"):
        scope.verify_documents(path, expected)


def test_readonly_service_descriptor_denies_all_non_get_api_methods(tmp_path):
    webapps = tmp_path / "webapps"
    descriptor = webapps / "ROOT" / "WEB-INF" / "web.xml"
    descriptor.parent.mkdir(parents=True)
    etc = tmp_path / "etc"
    etc.mkdir()
    (etc / "configuration.xml").write_text("<configuration />")
    source_war = tmp_path / "source.war"
    original_war = """<web-app xmlns="https://jakarta.ee/xml/ns/jakartaee">
      <context-param><param-name>CONFIGURATION</param-name>
        <param-value>/var/opengrok/etc/configuration.xml</param-value></context-param>
    </web-app>"""
    with zipfile.ZipFile(source_war, "w") as archive:
        archive.writestr("WEB-INF/web.xml", original_war)
    config = {
        "readonly_service": {
            "webapps_root": str(webapps),
            "etc_root": str(etc),
            "source_war": str(source_war),
        }
    }
    original = """<web-app xmlns="https://jakarta.ee/xml/ns/jakartaee">
      <context-param><param-name>CONFIGURATION</param-name>
        <param-value>/opengrok/etc/configuration.xml</param-value></context-param>
      <security-constraint><web-resource-collection>
        <web-resource-name>deny API writes</web-resource-name>
        <url-pattern>/api/*</url-pattern><http-method-omission>GET</http-method-omission>
      </web-resource-collection><auth-constraint/></security-constraint>
    </web-app>"""
    descriptor.write_text(original)
    assert live._opengrok_readonly_files(config)["web_xml_sha256"] == hashlib.sha256(
        original.encode()
    ).hexdigest()
    descriptor.write_text(original.replace("<auth-constraint/>", ""))
    with pytest.raises(ValueError, match="not denied"):
        live._opengrok_readonly_files(config)
    descriptor.write_text(original.replace("http-method-omission>GET", "http-method-omission>PUT"))
    with pytest.raises(ValueError, match="not denied"):
        live._opengrok_readonly_files(config)


def test_readonly_service_config_and_write_denial_replay_are_independent(tmp_path, monkeypatch):
    config = {
        "base_url": "http://127.0.0.1:18083",
        "backend_snapshot": {"mount_destination": "/opengrok/data/index"},
    }
    replies = {
        "/api/v1/configuration/dataRoot": b"/opengrok/data",
        "/api/v1/projects/indexed": b'["bat","cli"]',
    }
    monkeypatch.setattr(
        live, "_http",
        lambda _config, endpoint, _params, _accept: (
            200, "application/json", replies[endpoint], 1.0
        ),
    )
    target = tmp_path / "config"
    expected = {"bat", "cli"}
    got = live._opengrok_service_config_probe(config, expected, target, capture=True)
    assert got == live._opengrok_service_config_probe(config, expected, target, capture=False)
    replies["/api/v1/configuration/dataRoot"] = b'"/opengrok/data"'
    with pytest.raises(ValueError, match="dataRoot"):
        live._opengrok_service_config_probe(config, expected, tmp_path / "quoted", capture=True)
    replies["/api/v1/configuration/dataRoot"] = b"/elsewhere"
    with pytest.raises(ValueError, match="dataRoot"):
        live._opengrok_service_config_probe(config, expected, tmp_path / "wrong-root", capture=True)
    denial = tmp_path / "denial"
    denial.mkdir()
    (denial / "write-denial.body").write_bytes(b"denied")
    (denial / "write-denial.transport.json").write_text(
        json.dumps({
            "endpoint": "/api/v1/configuration/dataRoot",
            "method": "PUT",
            "request_body_sha256": hashlib.sha256(b"/opengrok/data").hexdigest(),
            "status": 403,
            "elapsed_ms": 1.0,
        })
    )
    live._opengrok_write_denial_probe(config, denial, capture=False)
    transport = json.loads((denial / "write-denial.transport.json").read_text())
    transport["status"] = 204
    (denial / "write-denial.transport.json").write_text(json.dumps(transport))
    with pytest.raises(ValueError, match="not denied"):
        live._opengrok_write_denial_probe(config, denial, capture=False)


def test_readonly_service_runtime_refuses_default_writer_and_overlapping_mount(
    tmp_path, monkeypatch
):
    roots = {name: tmp_path / name for name in ("index", "webapps", "etc", "src")}
    for path in roots.values():
        path.mkdir()
    war = tmp_path / "source.war"
    war.write_bytes(b"image WAR")
    container_id = "a" * 64
    image_sha = "b" * 64
    mounts = [
        {
            "Type": "bind", "Source": str(path), "Destination": destination, "RW": False,
        }
        for destination, path in (
            ("/opengrok/data/index", roots["index"]),
            ("/usr/local/tomcat/webapps", roots["webapps"]),
            ("/opengrok/etc", roots["etc"]),
            ("/opengrok/src", roots["src"]),
        )
    ]
    inspected = {
        "Id": container_id,
        "Image": "sha256:" + image_sha,
        "Created": "2026-10-04T23:59:59Z",
        "State": {"Running": True, "Pid": 123, "StartedAt": "2026-10-05T00:00:00Z"},
        "RestartCount": 0,
        "Mounts": mounts,
        "HostConfig": {"NetworkMode": "og_ro_test", "Privileged": False, "CapAdd": None},
        "Config": {
            "Entrypoint": ["/usr/local/tomcat/bin/catalina.sh"],
            "Cmd": ["run"], "User": "1111:1111",
        },
        "Path": "/usr/local/tomcat/bin/catalina.sh",
        "Args": ["run"],
        "NetworkSettings": {
            "Networks": {"og_ro_test": {}},
            "Ports": {"8080/tcp": [{"HostPort": "18183", "HostIp": "127.0.0.1"}]},
        },
    }
    config = {
        "base_url": "http://127.0.0.1:18183",
        "server_image_digest": image_sha,
        "backend_snapshot": {
            "root": str(roots["index"]), "container_id": container_id,
            "mount_destination": "/opengrok/data/index", "container_port": "8080/tcp",
        },
        "readonly_service": {
            "webapps_root": str(roots["webapps"]), "etc_root": str(roots["etc"]),
            "source_root": str(roots["src"]), "source_war": str(war),
            "network": "og_ro_test",
        },
    }
    monkeypatch.setattr(live, "_opengrok_readonly_files", lambda _config: {
        "webapps_sha256": "c" * 64,
        "configuration_sha256": "d" * 64,
        "web_xml_sha256": "e" * 64,
    })

    def fake_process(argv, _timeout):
        if argv[1] == "inspect":
            return 0, json.dumps(inspected).encode(), b"", 0.0
        return 0, (
            hashlib.sha256(war.read_bytes()).hexdigest()
            + "  /opengrok/lib/source.war\n"
        ).encode(), b"", 0.0

    monkeypatch.setattr(live, "_process", fake_process)
    assert live._backend_runtime(config)["readonly_files"]["source_war_sha256"] == hashlib.sha256(
        war.read_bytes()
    ).hexdigest()
    inspected["Config"]["Entrypoint"] = ["/scripts/entrypoint.sh"]
    with pytest.raises(ValueError, match="only Tomcat"):
        live._backend_runtime(config)
    inspected["Config"]["Entrypoint"] = ["/usr/local/tomcat/bin/catalina.sh"]
    inspected["Mounts"].append({
        "Type": "bind", "Source": str(tmp_path / "extra"),
        "Destination": "/opengrok/data/index/bat", "RW": True,
    })
    with pytest.raises(ValueError, match="extra or overlapping mount"):
        live._backend_runtime(config)


def test_readonly_snapshot_must_be_sealed_before_container_creation(tmp_path):
    receipt_path = tmp_path / "seal.json"
    config = {
        "backend_snapshot": {"root": str(tmp_path / "index")},
        "readonly_service": {
            "webapps_root": str(tmp_path / "webapps"),
            "etc_root": str(tmp_path / "etc"),
            "source_root": str(tmp_path / "src"),
            "snapshot_receipt": str(receipt_path),
        },
    }
    snapshot = {
        "tree_sha256": "a" * 64,
        "runtime": {
            "created_at": "2026-10-05T00:00:01.000000000Z",
            "started_at": "2026-10-05T00:00:02.000000000Z",
            "readonly_files": {
                "webapps_sha256": "b" * 64,
                "configuration_sha256": "c" * 64,
                "source_war_sha256": "d" * 64,
            },
        },
    }
    receipt = {
        "schema_version": 1,
        "sealed_at_utc": "2026-10-05T00:00:00.000000000Z",
        "index_root": config["backend_snapshot"]["root"],
        "index_tree_sha256": snapshot["tree_sha256"],
        "webapps_root": config["readonly_service"]["webapps_root"],
        "webapps_sha256": "b" * 64,
        "etc_root": config["readonly_service"]["etc_root"],
        "configuration_sha256": "c" * 64,
        "source_root": config["readonly_service"]["source_root"],
        "source_war_sha256": "d" * 64,
    }
    receipt_path.write_text(json.dumps(receipt))
    live._validate_opengrok_snapshot_seal(config, snapshot)
    receipt["sealed_at_utc"] = "2026-10-05T00:00:01.000000000Z"
    receipt_path.write_text(json.dumps(receipt))
    with pytest.raises(ValueError, match="not sealed before"):
        live._validate_opengrok_snapshot_seal(config, snapshot)


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
