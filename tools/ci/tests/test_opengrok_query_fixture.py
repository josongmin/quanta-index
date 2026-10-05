"""Independent fixed patch and compiler-output oracles for the optional fixture."""

from __future__ import annotations

import hashlib
import sys

import pytest

from tools.benchmark.retrieval import opengrok_query_fixture as fixture


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def test_embedded_source_patch_and_javac_class_goldens_are_fixed() -> None:
    assert _sha(fixture.PATCH) == "8e6b4cdb7d11fad1fe864eb69f4a5647fa04482f77059b703fba1be2e013ca98"
    assert (
        fixture.ORIGINAL_SOURCE_SHA256
        == "e7880011219a11e8b2e133c8c8b6cc678b9d8b39cc4fbb7e98836c4631c55654"
    )
    assert (
        fixture.PATCHED_SOURCE_SHA256
        == "21b0cb48cd58553a2d4657b1434ee5fb5228dc630f30de98ccaddd813b560f27"
    )
    assert (
        fixture.COMPILER_IMAGE_ID
        == "sha256:1b79b7700154fec76b32816c560b1d67f30e115868fc8caf5c123207ae6074e7"
    )
    assert fixture.CLASS_SHA256 == {
        "org/opengrok/web/api/v1/controller/SearchController$SearchEngineWrapper.class": "e477af140507338e72c22eb0753720a1f63c89aa7811a7fbea2fdb67259c1335",
        "org/opengrok/web/api/v1/controller/SearchController$SearchHit.class": "f8945465238c3ed984f51edb987339ab85ed615ee0e7f375ac600c4780f16c65",
        "org/opengrok/web/api/v1/controller/SearchController$SearchResult.class": "8db685df4821fba6f3fb0cc218b78ca7fcc638fb5c72fc4845f1f42aee80ac82",
        "org/opengrok/web/api/v1/controller/SearchController.class": "c492587703599e7203d17872326fe8d0af694d2547b801dbe7195a4036b9c372",
    }


def test_materialize_applies_checked_patch_and_preserves_exact_bytes(tmp_path, monkeypatch) -> None:
    original = b"first\nsecond\nthird\n"
    patched = b"first\nSECOND\nthird\n"
    patch = (
        b"--- a/SearchController.java\n"
        b"+++ b/SearchController.java\n"
        b"@@ -1,3 +1,3 @@\n"
        b" first\n"
        b"-second\n"
        b"+SECOND\n"
        b" third\n"
    )
    monkeypatch.setattr(fixture, "PATCH", patch)
    monkeypatch.setattr(fixture, "ORIGINAL_SOURCE_SHA256", _sha(original))
    monkeypatch.setattr(fixture, "SOURCE_PATCH_SHA256", _sha(patch))
    monkeypatch.setattr(fixture, "PATCHED_SOURCE_SHA256", _sha(patched))
    upstream = tmp_path / "SearchController.java"
    upstream.write_bytes(original)
    output = tmp_path / "fixture"
    assert fixture.materialize(upstream, output) == output
    assert (output / "original/SearchController.java").read_bytes() == original
    assert (output / "patched/SearchController.java").read_bytes() == patched
    assert (output / "SearchController.patch").read_bytes() == patch


def test_wrong_upstream_original_refused_before_creating_output(tmp_path) -> None:
    upstream = tmp_path / "SearchController.java"
    upstream.write_bytes(b"wrong OpenGrok source\n")
    output = tmp_path / "fixture"
    with pytest.raises(ValueError, match="upstream.*SHA differs"):
        fixture.materialize(upstream, output)
    assert not output.exists()


def test_class_oracle_rejects_extra_missing_and_mutated_files(tmp_path, monkeypatch) -> None:
    payloads = {name: name.encode() for name in fixture.CLASS_SHA256}
    monkeypatch.setattr(
        fixture, "CLASS_SHA256", {name: _sha(data) for name, data in payloads.items()}
    )
    classes = tmp_path / "classes"
    for name, data in payloads.items():
        target = classes / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    assert fixture.verify_class_outputs(classes) == fixture.CLASS_SHA256
    target = classes / next(iter(payloads))
    target.write_bytes(b"wrong class")
    with pytest.raises(ValueError, match="class SHA differs"):
        fixture.verify_class_outputs(classes)
    target.write_bytes(payloads[next(iter(payloads))])
    extra = classes / "Unrelated.class"
    extra.write_bytes(b"extra")
    with pytest.raises(ValueError, match="class inventory differs"):
        fixture.verify_class_outputs(classes)
    extra.unlink()
    target.unlink()
    with pytest.raises(ValueError, match="class inventory differs"):
        fixture.verify_class_outputs(classes)


def test_cli_build_failure_leaves_final_absent_and_removes_staging(tmp_path, monkeypatch) -> None:
    upstream = tmp_path / "upstream.java"
    upstream.write_bytes(b"fixture source")
    output = tmp_path / "fixture"

    def materialize(_original, candidate):
        candidate.mkdir()
        (candidate / "source").write_bytes(b"materialized")
        return candidate

    def failed_build(candidate, _web_inf):
        assert candidate.exists() and not output.exists()
        raise ValueError("compiler failed")

    monkeypatch.setattr(fixture, "materialize", materialize)
    monkeypatch.setattr(fixture, "build_classes", failed_build)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "fixture",
            "--original",
            str(upstream),
            "--output",
            str(output),
            "--build-web-inf",
            str(tmp_path),
        ],
    )
    with pytest.raises(ValueError, match="compiler failed"):
        fixture.main()
    assert not output.exists()
    assert set(tmp_path.iterdir()) == {upstream}


def test_cli_build_refuses_existing_output_without_touching_it(tmp_path, monkeypatch) -> None:
    output = tmp_path / "fixture"
    output.mkdir()
    marker = output / "keep"
    marker.write_bytes(b"existing")
    monkeypatch.setattr(fixture, "materialize", lambda *_args: pytest.fail("overwrote output"))
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "fixture",
            "--original",
            str(tmp_path / "upstream.java"),
            "--output",
            str(output),
            "--build-web-inf",
            str(tmp_path),
        ],
    )
    with pytest.raises(ValueError, match="fresh external path"):
        fixture.main()
    assert marker.read_bytes() == b"existing"
    assert set(tmp_path.iterdir()) == {output}


def test_cli_build_publishes_only_after_compiled_classes_pass(tmp_path, monkeypatch) -> None:
    output = tmp_path / "fixture"

    def materialize(_original, candidate):
        candidate.mkdir()
        return candidate

    def accepted_build(candidate, _web_inf):
        assert candidate.exists() and not output.exists()
        (candidate / "classes").mkdir()
        return {}

    monkeypatch.setattr(fixture, "materialize", materialize)
    monkeypatch.setattr(fixture, "build_classes", accepted_build)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "fixture",
            "--original",
            str(tmp_path / "upstream.java"),
            "--output",
            str(output),
            "--build-web-inf",
            str(tmp_path),
        ],
    )
    assert fixture.main() == 0
    assert (output / "classes").is_dir()
    assert set(tmp_path.iterdir()) == {output}
