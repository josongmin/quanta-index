"""Pinned TypeScript syntax projection keeps declared source spans."""

from __future__ import annotations

import hashlib
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
def test_candidate_census_matches_vendor_and_original_bytes(
    projector_inputs, language, path, raw, expected_edits
):
    baseline = source_oracle.declaration_census(language, path, raw)
    with TsProjector(**projector_inputs) as projector:
        rows, projection, extension_sha256 = source_oracle.candidate_syntax_declaration_census(
            language, path, raw, projector, timeout_seconds=5
        )
    assert rows == baseline
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


def test_candidate_executes_staged_node_after_original_path_swap(projector_inputs, tmp_path):
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
