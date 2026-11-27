"""Corpus authority for a separate-process grammar comparison."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pytest

from tools.ci.compare_tree_sitter_symbol_records import candidate_batch_rows, load_corpus_manifest


def write_manifest(tmp_path, files):
    manifest = tmp_path / "manifest.json"
    raw = json.dumps({"schema": "quanta-symbol-corpus-v1", "files": files}).encode()
    manifest.write_bytes(raw)
    return manifest, hashlib.sha256(raw).hexdigest()


def entry(path, source, *, status="ok", records=1):
    return {
        "path": path,
        "sha256": hashlib.sha256(source).hexdigest(),
        "status": status,
        "records": records,
    }


def test_manifest_binds_exact_file_bytes_and_record_denominator(tmp_path):
    source = b"package p\nfunc A() {}\n"
    (tmp_path / "a.go").write_bytes(source)
    manifest, checksum = write_manifest(tmp_path, [entry("a.go", source, records=1)])
    assert load_corpus_manifest(manifest, checksum, tmp_path) == [
        ("go", "a.go", source.decode(), False, 1)
    ]
    with pytest.raises(ValueError, match="manifest digest"):
        load_corpus_manifest(manifest, "0" * 64, tmp_path)
    (tmp_path / "a.go").write_bytes(b"package p\nfunc B() {}\n")
    with pytest.raises(ValueError, match="file digest"):
        load_corpus_manifest(manifest, checksum, tmp_path)


def test_manifest_rejects_duplicate_traversal_and_symlink(tmp_path):
    source = b"function A() {}"
    (tmp_path / "a.js").write_bytes(source)
    duplicate, checksum = write_manifest(tmp_path, [entry("a.js", source), entry("a.js", source)])
    with pytest.raises(ValueError, match="invalid corpus entry"):
        load_corpus_manifest(duplicate, checksum, tmp_path)
    traversal, checksum = write_manifest(tmp_path, [entry("../a.js", source)])
    with pytest.raises(ValueError, match="invalid corpus entry"):
        load_corpus_manifest(traversal, checksum, tmp_path)
    (tmp_path / "alias.js").symlink_to(tmp_path / "a.js")
    symlink, checksum = write_manifest(tmp_path, [entry("alias.js", source)])
    with pytest.raises(ValueError, match="cannot be opened safely"):
        load_corpus_manifest(symlink, checksum, tmp_path)


def test_manifest_rejects_duplicate_json_key(tmp_path):
    raw = b'{"schema":"quanta-symbol-corpus-v1","schema":"quanta-symbol-corpus-v1","files":[]}'
    manifest = tmp_path / "manifest.json"
    manifest.write_bytes(raw)
    with pytest.raises(ValueError, match="duplicate corpus manifest field"):
        load_corpus_manifest(manifest, hashlib.sha256(raw).hexdigest(), tmp_path)


def test_negative_case_requires_zero_expected_records(tmp_path):
    source = b"function broken("
    (tmp_path / "bad.ts").write_bytes(source)
    bad, checksum = write_manifest(
        tmp_path, [entry("bad.ts", source, status="parse_failure", records=1)]
    )
    with pytest.raises(ValueError, match="invalid corpus entry"):
        load_corpus_manifest(bad, checksum, tmp_path)
    valid, checksum = write_manifest(
        tmp_path, [entry("bad.ts", source, status="parse_failure", records=0)]
    )
    assert load_corpus_manifest(valid, checksum, tmp_path) == [
        ("syntax", "bad.ts", source.decode(), True, 0)
    ]


def test_candidate_corpus_is_split_into_bounded_batches(monkeypatch):
    sizes = []

    def fake_run(_command, source, *, timeout_seconds, max_stdout):
        assert timeout_seconds == 600 and max_stdout == 8 << 20
        paths = [row["path"] for row in json.loads(source)]
        sizes.append(len(paths))
        rows = [{"path": path, "status": "ok", "records": []} for path in paths]
        return json.dumps(rows).encode(), b"", 0

    monkeypatch.setattr("tools.ci.compare_tree_sitter_symbol_records.run_helper", fake_run)
    cases = [(f"file-{index}.go", "package p", None, False, 0) for index in range(17)]
    rows = candidate_batch_rows(Path("official-go"), [], cases)
    assert sizes == [8, 8, 1]
    assert [row["path"] for row in rows] == [case[0] for case in cases]
