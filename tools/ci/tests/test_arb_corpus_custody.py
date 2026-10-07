"""Independent synthetic archive fixtures for official ARB BCY custody."""

from __future__ import annotations

import hashlib
import io
import json
import os
import shutil
import stat
import subprocess
import tarfile
import tempfile
import uuid
from pathlib import Path
from types import SimpleNamespace

import pytest

from tools.benchmark.retrieval import arb_corpus_custody as custody
from tools.benchmark.retrieval import arb_official_score as scorer

RELEASE = "v2_trace2code"
COMMIT = "a" * 40
CHUNK_NAME = f"corpus/{RELEASE}/gin-gonic__gin/{COMMIT}.chunks.jsonl"


def _sha(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def _bundle(
    tmp_path: Path,
    extra: list[tuple[str, bytes, bytes | None]] = (),
    chunk_override: bytes | None = None,
    release: str = RELEASE,
):
    if shutil.which("zstd") is None:
        pytest.skip("zstd unavailable: synthetic archive fixture cannot be built")
    root = tmp_path / "b06"
    release_dir = root / "data/releases" / release
    release_dir.mkdir(parents=True)
    manifest_name = f"corpus/{release}/corpus_manifest.jsonl"
    chunk_name = f"corpus/{release}/gin-gonic__gin/{COMMIT}.chunks.jsonl"
    row = {
        "repo": "gin-gonic/gin",
        "base_commit": COMMIT,
        "status": "ok",
        "chunks_path": f"/official/data/{chunk_name}",
        "chunk_count": 2,
        "file_count": 2,
        "symbol_count": 0,
    }
    manifest = (json.dumps(row) + "\n").encode()
    chunks = (
        json.dumps(
            {
                "kind": "file",
                "path": "gold.go",
                "text": "found",
                "repo": "gin-gonic/gin",
                "base_commit": COMMIT,
            }
        )
        + "\n"
        + json.dumps(
            {
                "kind": "file",
                "path": "noise.go",
                "text": "noise",
                "repo": "gin-gonic/gin",
                "base_commit": COMMIT,
            }
        )
        + "\n"
    ).encode()
    if chunk_override is not None:
        chunks = chunk_override
    tar_path = tmp_path / "source.tar"
    with tarfile.open(tar_path, "w") as tar:
        for name, payload, kind in [
            (manifest_name, manifest, None),
            (chunk_name, chunks, None),
            *extra,
        ]:
            info = tarfile.TarInfo(name)
            info.size = len(payload)
            if kind is not None:
                info.type = kind
                info.linkname = "/private/tmp/escape"
            tar.addfile(info, io.BytesIO(payload) if kind is None else None)
    archive_name = f"agent_retrieval_bench_{release}.tar.zst"
    archive = release_dir / archive_name
    with archive.open("wb") as handle:
        subprocess.run(["zstd", "-q", "-c", str(tar_path)], stdout=handle, check=True)
    digest = _sha(archive.read_bytes())
    (release_dir / (archive_name + ".sha256")).write_text(
        f"{digest}  releases/{release}/{archive_name}\n", encoding="ascii"
    )
    policy = {
        "arb": {
            "releases": {
                release: {
                    "tar_zst_sha256": digest,
                    "tar_zst_sha256_recomputed": digest,
                    "corpus_manifest_sha256": _sha(manifest),
                }
            }
        }
    }
    return root, policy, archive


def _work_root() -> Path:
    return Path(tempfile.gettempdir()).resolve() / f"arb-corpus-test-{uuid.uuid4().hex}"


def test_selected_official_bytes_and_fixed_bcy_golden(tmp_path):
    b06, policy, _ = _bundle(tmp_path)
    work = _work_root()
    try:
        result = custody.restore(b06, policy, {RELEASE: {COMMIT}}, work)
        assert result.custody["status"] == "VERIFIED"
        assert result.custody["releases"][RELEASE]["selected_chunks_sha256"][COMMIT] == _sha(
            (work / CHUNK_NAME).read_bytes()
        )
        custody.verify_extracted(result)
        source = Path(
            os.environ.get(
                "ARB_B06_ROOT",
                "/Users/songmin/Documents/code-new/qi-s30-bench-trust-20260930-0d21914e/b06",
            )
        )
        if not (source / "policy.json").is_file():
            pytest.skip("pinned official BCY implementation unavailable")
        _, _, bcy = scorer.pinned_arb(source)
        cache = custody.verified_file_cache(result, bcy)
        detail = {
            "sample_id": "fixed",
            "task_type": "trace2code",
            "repo": "gin-gonic/gin",
            "base_commit": COMMIT,
            "gold_files": ["gold.go"],
            "top_files": ["noise.go", "gold.go"],
        }
        official = bcy.evaluate_run(
            "fixed", "test", work / "detail.jsonl", [detail], cache, (1, 100), (1,)
        )
        assert official["samples"] == 1
        assert official["overall"]["BCY@1"] == 0.0
        assert official["overall"]["BCY@100"] == 1.0
    finally:
        shutil.rmtree(work, ignore_errors=True)


@pytest.mark.parametrize(
    "name,kind,reason",
    [
        ("../../escape", None, "tar traversal"),
        ("/private/tmp/escape", None, "absolute"),
        ("corpus/v2_trace2code/link", tarfile.SYMTYPE, "unsafe tar member type"),
        ("corpus/v2_trace2code/hard", tarfile.LNKTYPE, "unsafe tar member type"),
        (CHUNK_NAME, None, "duplicate tar member"),
    ],
)
def test_rejects_unsafe_tar_members(tmp_path, name, kind, reason):
    b06, policy, _ = _bundle(tmp_path, [(name, b"x", kind)])
    work = _work_root()
    with pytest.raises(custody.CorpusBlocked, match=reason):
        custody.restore(b06, policy, {RELEASE: {COMMIT}}, work)
    assert not work.exists()


def test_rejects_tampered_archive_and_missing_archive(tmp_path):
    b06, policy, archive = _bundle(tmp_path)
    archive.write_bytes(archive.read_bytes() + b"tamper")
    work = _work_root()
    with pytest.raises(custody.CorpusBlocked, match="archive digest mismatch"):
        custody.restore(b06, policy, {RELEASE: {COMMIT}}, work)
    assert not work.exists()
    archive.unlink()
    with pytest.raises(custody.CorpusBlocked, match="unsafe archive"):
        custody.restore(b06, policy, {RELEASE: {COMMIT}}, work)
    assert not work.exists()


def test_rejects_manifest_digest_and_size_limit(tmp_path, monkeypatch):
    b06, policy, _ = _bundle(tmp_path)
    policy["arb"]["releases"][RELEASE]["corpus_manifest_sha256"] = "0" * 64
    work = _work_root()
    with pytest.raises(custody.CorpusBlocked, match="manifest digest mismatch"):
        custody.restore(b06, policy, {RELEASE: {COMMIT}}, work)
    assert not work.exists()
    _, policy, _ = _bundle(tmp_path / "second")
    b06 = tmp_path / "second/b06"
    monkeypatch.setattr(custody, "MAX_MEMBER", 4)
    with pytest.raises(custody.CorpusBlocked, match="size limit"):
        custody.restore(b06, policy, {RELEASE: {COMMIT}}, work)
    assert not work.exists()


def test_rejects_foreign_owner_and_existing_root(tmp_path):
    fake = SimpleNamespace(st_mode=stat.S_IFREG | 0o600, st_uid=os.getuid() + 1, st_nlink=1)
    with pytest.raises(custody.CorpusBlocked, match="owner-local"):
        custody._owner_local_archive(fake)
    b06, policy, _ = _bundle(tmp_path)
    work = _work_root()
    work.mkdir()
    try:
        with pytest.raises(custody.CorpusBlocked, match="already exists"):
            custody.restore(b06, policy, {RELEASE: {COMMIT}}, work)
    finally:
        work.rmdir()


def test_extracted_chunk_tamper_blocks_custody(tmp_path):
    b06, policy, _ = _bundle(tmp_path)
    work = _work_root()
    try:
        result = custody.restore(b06, policy, {RELEASE: {COMMIT}}, work)
        (work / CHUNK_NAME).write_text("tampered\n", encoding="utf-8")
        with pytest.raises(custody.CorpusBlocked, match="digest mismatch"):
            custody.verify_extracted(result)
    finally:
        shutil.rmtree(work, ignore_errors=True)


def test_rejects_archive_pinned_to_wrong_snapshot_rows(tmp_path):
    wrong = (
        json.dumps(
            {
                "kind": "file",
                "path": "gold.go",
                "text": "found",
                "repo": "gin-gonic/gin",
                "base_commit": "b" * 40,
            }
        )
        + "\n"
    ).encode()
    b06, policy, _ = _bundle(tmp_path, chunk_override=wrong)
    work = _work_root()
    with pytest.raises(custody.CorpusBlocked, match="snapshot identity mismatch"):
        custody.restore(b06, policy, {RELEASE: {COMMIT}}, work)
    assert not work.exists()


def test_rejects_ambiguous_duplicate_file_text(tmp_path):
    row = {
        "kind": "file",
        "path": "gold.go",
        "text": "found",
        "repo": "gin-gonic/gin",
        "base_commit": COMMIT,
    }
    duplicate = (json.dumps(row) + "\n" + json.dumps({**row, "text": "different"}) + "\n").encode()
    b06, policy, _ = _bundle(tmp_path, chunk_override=duplicate)
    work = _work_root()
    with pytest.raises(custody.CorpusBlocked, match="duplicate corpus file row"):
        custody.restore(b06, policy, {RELEASE: {COMMIT}}, work)
    assert not work.exists()


def test_rejects_shared_commit_with_different_official_corpus_bytes(tmp_path):
    b06, policy, _ = _bundle(tmp_path / "first")
    other_release = "v2_code2test"
    changed_rows = [
        {
            "kind": "file",
            "path": "gold.go",
            "text": "different",
            "repo": "gin-gonic/gin",
            "base_commit": COMMIT,
        },
        {
            "kind": "file",
            "path": "noise.go",
            "text": "noise",
            "repo": "gin-gonic/gin",
            "base_commit": COMMIT,
        },
    ]
    changed = ("\n".join(json.dumps(row) for row in changed_rows) + "\n").encode()
    other_b06, other_policy, _ = _bundle(
        tmp_path / "second", chunk_override=changed, release=other_release
    )
    shutil.copytree(
        other_b06 / "data/releases" / other_release,
        b06 / "data/releases" / other_release,
    )
    policy["arb"]["releases"].update(other_policy["arb"]["releases"])
    work = _work_root()
    with pytest.raises(custody.CorpusBlocked, match="different official corpus bytes"):
        custody.restore(b06, policy, {RELEASE: {COMMIT}, other_release: {COMMIT}}, work)
    assert not work.exists()


def test_os_temp_root_and_alias_are_canonicalized(tmp_path, monkeypatch):
    b06, policy, _ = _bundle(tmp_path)
    advertised = Path(tempfile.gettempdir())
    canonical = advertised.resolve()
    for parent in {advertised, canonical}:
        work = parent / f"arb-corpus-test-{uuid.uuid4().hex}"
        canonical_work = canonical / work.name
        try:
            result = custody.restore(b06, policy, {RELEASE: {COMMIT}}, work)
            assert result.custody["work_root"] == str(canonical_work)
            custody.verify_extracted(result)
        finally:
            shutil.rmtree(canonical_work, ignore_errors=True)
    with pytest.raises(custody.CorpusBlocked, match="OS temp directory"):
        custody.restore(b06, policy, {RELEASE: {COMMIT}}, canonical / "nested" / "fresh")

    # A Linux-style temporary directory chosen by the runtime is accepted.
    monkeypatch.setattr(tempfile, "gettempdir", lambda: str(tmp_path))
    simulated = tmp_path / f"arb-corpus-test-{uuid.uuid4().hex}"
    try:
        result = custody.restore(b06, policy, {RELEASE: {COMMIT}}, simulated)
        assert result.custody["work_root"] == str(simulated)
    finally:
        shutil.rmtree(simulated, ignore_errors=True)


def test_missing_zstd_is_blocked_without_creating_output(monkeypatch):
    monkeypatch.setattr(custody.shutil, "which", lambda _: None)
    work = _work_root()
    policy = {"arb": {"releases": {RELEASE: {}}}}
    with pytest.raises(custody.CorpusBlocked, match="zstd executable unavailable"):
        custody.restore(Path("/absent"), policy, {RELEASE: set()}, work)
    assert not work.exists()


def test_verified_cache_rejects_mutated_bytes_before_official_read(tmp_path):
    b06, policy, _ = _bundle(tmp_path)
    work = _work_root()
    try:
        result = custody.restore(b06, policy, {RELEASE: {COMMIT}}, work)
        source = Path(
            os.environ.get(
                "ARB_B06_ROOT",
                "/Users/songmin/Documents/code-new/qi-s30-bench-trust-20260930-0d21914e/b06",
            )
        )
        if not (source / "policy.json").is_file():
            pytest.skip("pinned official BCY implementation unavailable")
        _, _, bcy = scorer.pinned_arb(source)
        (work / CHUNK_NAME).write_text("tampered\n", encoding="utf-8")
        cache = custody.verified_file_cache(result, bcy)
        with pytest.raises(custody.CorpusBlocked, match="selected corpus digest mismatch"):
            cache.file_text("gin-gonic/gin", COMMIT, "gold.go")
    finally:
        shutil.rmtree(work, ignore_errors=True)


def test_verified_cache_detects_source_change_during_official_decode(tmp_path):
    b06, policy, _ = _bundle(tmp_path)
    work = _work_root()
    try:
        result = custody.restore(b06, policy, {RELEASE: {COMMIT}}, work)
        selected = work / CHUNK_NAME
        original = selected.read_bytes()

        class Cache:
            def __init__(self, manifest):
                self.manifest = manifest
                self._cache = {}

        def loader(source):
            with source.open(encoding="utf-8") as handle:
                text = handle.read()
            selected.write_bytes(b"changed")
            selected.write_bytes(original)
            return {"gold.go": text}

        fake_official = SimpleNamespace(CorpusFileCache=Cache, load_file_texts=loader)
        cache = custody.verified_file_cache(result, fake_official)
        with pytest.raises(custody.CorpusBlocked, match="unsafe selected corpus read"):
            cache.file_text("gin-gonic/gin", COMMIT, "gold.go")
    finally:
        shutil.rmtree(work, ignore_errors=True)
