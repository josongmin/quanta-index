"""Pinned TypeScript syntax projection keeps declared source spans."""

from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
import time
from pathlib import Path

import pytest

from tools.benchmark.retrieval import declaration_census_audit, source_oracle
from tools.benchmark.retrieval import ts_projection as ts_module
from tools.benchmark.retrieval.ts_projection import SOURCE, TsProjectionError, TsProjector


@pytest.fixture(scope="module")
def projector_inputs():
    if shutil.which("node") is None or shutil.which("npm") is None:
        pytest.skip("TypeScript checker toolchain unavailable")
    node = Path(shutil.which("node")).resolve()
    library = (
        declaration_census_audit._build("typescript") / "node_modules/typescript/lib/typescript.js"
    )

    def digest(path):
        return hashlib.sha256(path.read_bytes()).hexdigest()

    node_library_dir = node.parent.parent / "lib"
    node_libraries = tuple(
        (path.resolve(), digest(path.resolve()))
        for path in sorted(node_library_dir.glob("libnode*.dylib"))
    )
    return {
        "node": node,
        "typescript_js": library,
        "node_sha256": digest(node),
        "typescript_sha256": digest(library),
        "script_sha256": digest(SOURCE),
        "node_version": subprocess.check_output([str(node), "--version"], text=True).strip(),
        "node_libraries": node_libraries,
    }


@pytest.mark.parametrize(
    ("language", "path", "raw", "expected_edits"),
    [
        ("javascript", "alias.js", b"function local() {} export { local as const };", 1),
        ("javascript", "alias.jsx", b"function local() {} export { local as return };", 1),
        (
            "typescript",
            "star.ts",
            b"export type * from './basic'\nfunction sentinel() {}",
            1,
        ),
        (
            "typescript",
            "namespace.ts",
            b"export type * as HttpProxy from './basic'\nfunction sentinel() {}",
            1,
        ),
        (
            "typescript",
            "generic.ts",
            b"runnerImport<typeof import('./basic')>(fixture('cjs.js'),)\nfunction sentinel() {}",
            1,
        ),
        (
            "typescript",
            "signature.ts",
            b"type Overloads = { <T>(): T\n<T>(): T };\nfunction sentinel() {}",
            1,
        ),
        (
            "typescript",
            "namespace.tsx",
            b"export type * as HttpProxy from './basic'\nfunction sentinel() { return <div/> }",
            1,
        ),
    ],
)
def test_candidate_census_preserves_original_declaration_bytes(
    projector_inputs, language, path, raw, expected_edits
):
    with TsProjector(**projector_inputs) as projector:
        rows, projection, extension_sha256 = source_oracle.candidate_syntax_declaration_census(
            language, path, raw, projector, timeout_seconds=5
        )
    expected_names = (
        [b"local"] if language == "javascript"
        else [b"Overloads", b"sentinel"] if path == "signature.ts"
        else [b"sentinel"]
    )
    assert [raw[start:end] for start, end, *_ in rows] == expected_names
    assert len(projection.projected) == len(raw)
    assert len(projection.edits) == expected_edits
    assert len(extension_sha256) == 64
    assert [index for index, byte in enumerate(projection.projected) if byte in (10, 13)] == [
        index for index, byte in enumerate(raw) if byte in (10, 13)
    ]


@pytest.mark.parametrize(
    ("path", "raw"),
    [
        ("broken.js", b"function const() {}"),
        ("broken.ts", b"function broken( {"),
    ],
)
def test_candidate_rejects_compiler_syntax_failure(projector_inputs, path, raw):
    with TsProjector(**projector_inputs) as projector:
        with pytest.raises(TsProjectionError) as raised:
            projector.project(path, raw, timeout_seconds=5)
    assert raised.value.kind == "parse_failure"


def test_projection_edits_keep_utf8_offsets_with_bom_and_astral_characters(projector_inputs):
    path = "unicode.js"
    raw = "\ufeff// 한😀\nfunction local() {}\nexport { local as const };\n".encode()
    alias_start = raw.index(b"const")
    with TsProjector(**projector_inputs) as projector:
        result = projector.project(path, raw, timeout_seconds=5)
    assert [(edit.start, edit.end, edit.replacement) for edit in result.edits] == [
        (alias_start, alias_start + len(b"const"), b"aaaaa")
    ]
    assert result.projected == raw.replace(b" as const", b" as aaaaa")


def test_candidate_rejects_wrong_node_digest(projector_inputs):
    wrong = {**projector_inputs, "node_sha256": "0" * 64}
    with pytest.raises(TsProjectionError) as raised:
        with TsProjector(**wrong):
            pass
    assert raised.value.kind == "binary_identity"


def test_candidate_cancellation_and_timeout(projector_inputs):
    with TsProjector(**projector_inputs) as projector:
        with pytest.raises(TsProjectionError) as cancelled:
            projector.project("a.ts", b"type A = number", timeout_seconds=5, cancelled=lambda: True)
        started = time.monotonic()
        with pytest.raises(TsProjectionError) as timed_out:
            projector.project(
                "a.ts",
                b"type A = number",
                timeout_seconds=5,
                cancelled=lambda: time.monotonic() - started >= 0.001,
            )
    assert cancelled.value.kind == "cancelled"
    assert timed_out.value.kind == "cancelled"


def test_candidate_executes_staged_node_after_original_path_swap(
    projector_inputs, tmp_path, monkeypatch
):
    monkeypatch.setenv("NODE_OPTIONS", "--invalid-quanta-node-option")
    original = tmp_path / "node"
    shutil.copyfile(projector_inputs["node"], original)
    original.chmod(0o700)
    inputs = {**projector_inputs, "node": original}
    with TsProjector(**inputs) as projector:
        original.unlink()
        original.write_text("#!/bin/sh\nexit 44\n")
        original.chmod(0o700)
        projection = projector.project("a.ts", b"type A = number", timeout_seconds=5)
    assert projection.projected == b"type A = number"


@pytest.mark.parametrize("stream", ["stdout", "stderr"])
def test_candidate_child_output_is_capped_while_reading(
    projector_inputs, tmp_path, monkeypatch, stream
):
    script = tmp_path / "flood.cjs"
    descriptor = 1 if stream == "stdout" else 2
    script.write_text(
        f"const fs = require('node:fs');\nfs.writeSync({descriptor}, Buffer.alloc(9000000));\n"
    )
    monkeypatch.setattr(ts_module, "SOURCE", script)
    inputs = {
        **projector_inputs,
        "script_sha256": hashlib.sha256(script.read_bytes()).hexdigest(),
    }
    with TsProjector(**inputs) as projector:
        with pytest.raises(TsProjectionError) as raised:
            projector.project("a.ts", b"type A = number", timeout_seconds=5)
    assert raised.value.kind == "resource_limit"


def test_candidate_deep_json_is_a_typed_producer_defect(projector_inputs, tmp_path, monkeypatch):
    script = tmp_path / "deep.cjs"
    script.write_text("process.stdout.write('['.repeat(2000) + ']'.repeat(2000));\n")
    monkeypatch.setattr(ts_module, "SOURCE", script)
    inputs = {**projector_inputs, "script_sha256": hashlib.sha256(script.read_bytes()).hexdigest()}
    with TsProjector(**inputs) as projector:
        with pytest.raises(TsProjectionError) as raised:
            projector.project("a.ts", b"type A = number", timeout_seconds=5)
    assert raised.value.kind == "producer_defect"


@pytest.mark.parametrize("status", ["[]", '"ok","status":"ok"'])
def test_candidate_ambiguous_or_non_string_status_is_a_producer_defect(
    projector_inputs, tmp_path, monkeypatch, status
):
    source = b"type A = number"
    digest = hashlib.sha256(source).hexdigest()
    response = (
        '{"schema":"quanta-ts-grammar-projection-v1","status":'
        + status
        + ',"typescript_version":"'
        + projector_inputs.get("typescript_version", "5.9.3")
        + '","node_version":"'
        + projector_inputs["node_version"]
        + '","source_sha256":"'
        + digest
        + '","edits":[]}'
    )
    script = tmp_path / "bad-status.cjs"
    script.write_text(f"process.stdout.write({response!r});\n")
    monkeypatch.setattr(ts_module, "SOURCE", script)
    inputs = {**projector_inputs, "script_sha256": hashlib.sha256(script.read_bytes()).hexdigest()}
    with TsProjector(**inputs) as projector:
        with pytest.raises(TsProjectionError) as raised:
            projector.project("a.ts", source, timeout_seconds=15)
    assert raised.value.kind == "producer_defect"


def _batch_cli(projector_inputs, files, timeout_ms=1500, typescript_js=None):
    return subprocess.run(
        [
            str(projector_inputs["node"]),
            str(SOURCE),
            "--batch",
            str(typescript_js or projector_inputs["typescript_js"]),
            str(timeout_ms),
        ],
        input=json.dumps(files),
        text=True,
        capture_output=True,
        timeout=20,
        check=False,
    )


def test_batch_projection_preserves_order_and_schema(projector_inputs):
    files = [
        {"path": "alias.js", "source": "function local() {} export { local as const };"},
        {"path": "broken.ts", "source": "function broken( {"},
        {"path": "star.ts", "source": "export type * from './basic'\nfunction sentinel() {}"},
    ]
    proc = _batch_cli(projector_inputs, files)
    assert proc.returncode == 0, proc.stderr
    rows = json.loads(proc.stdout)
    assert [row["path"] for row in rows] == [file["path"] for file in files]
    assert [row["response"]["status"] for row in rows] == [
        "ok", "parse_failure", "ok"
    ]
    assert [len(row["response"]["edits"]) for row in rows] == [1, 0, 1]
    for row, file in zip(rows, files):
        assert set(row) == {"path", "elapsed_ms", "response"}
        assert 0 <= row["elapsed_ms"] <= 1500
        assert set(row["response"]) == {
            "schema", "status", "typescript_version", "node_version", "source_sha256", "edits"
        }
        assert row["response"]["typescript_version"] == "5.9.3"
        assert row["response"]["node_version"] == projector_inputs["node_version"]
        assert row["response"]["source_sha256"] == hashlib.sha256(
            file["source"].encode()
        ).hexdigest()


@pytest.mark.parametrize(
    "files",
    [[], [{"path": "a.ts", "source": 1}], [{"path": "a.ts", "source": "x" * (1 << 20)}] * 32],
)
def test_batch_rejects_invalid_or_oversized_input(projector_inputs, files):
    proc = _batch_cli(projector_inputs, files)
    assert proc.returncode == 2
    assert proc.stdout == ""


def test_batch_deadline_restarts_worker_then_projects_next_file(projector_inputs, tmp_path):
    load_counter = tmp_path / "compiler-loads"
    compiler = tmp_path / "slow-typescript.cjs"
    compiler.write_text(
        "const fs = require('node:fs');\n"
        f"fs.appendFileSync({json.dumps(str(load_counter))}, 'x');\n"
        f"const ts = require({json.dumps(str(projector_inputs['typescript_js']))});\n"
        "module.exports = new Proxy(ts, {get(target, property) {\n"
        "  if (property === 'createSourceFile') return function(path, ...args) {\n"
        "    if (path === 'slow.ts') Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 3500);\n"
        "    return target.createSourceFile(path, ...args);\n"
        "  };\n"
        "  return Reflect.get(target, property);\n"
        "}});\n"
    )
    files = [
        {"path": "first.ts", "source": "type First = number;"},
        {"path": "slow.ts", "source": "type Slow = number;"},
        {"path": "last.ts", "source": "type Last = number;"},
    ]
    proc = _batch_cli(projector_inputs, files, typescript_js=compiler)
    assert proc.returncode == 0, proc.stderr
    rows = json.loads(proc.stdout)
    assert [row["response"]["status"] for row in rows] == ["ok", "timed_out", "ok"]
    assert rows[1]["elapsed_ms"] == 1500
    assert rows[1]["response"]["edits"] == []
    assert rows[1]["response"]["source_sha256"] == hashlib.sha256(
        files[1]["source"].encode()
    ).hexdigest()
    assert load_counter.read_text() == "xx"


def test_batch_rejects_compiler_defect_without_partial_output(projector_inputs, tmp_path):
    compiler = tmp_path / "wrong-version.cjs"
    compiler.write_text("module.exports = {version: '0.0.0'};\n")
    proc = _batch_cli(
        projector_inputs,
        [{"path": "first.ts", "source": "type First = number;"}],
        typescript_js=compiler,
    )
    assert proc.returncode == 2
    assert proc.stdout == ""
