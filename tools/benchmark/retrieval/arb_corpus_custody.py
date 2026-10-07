"""Selective, digest-bound restoration of official ARB corpus bytes for BCY."""

from __future__ import annotations

import hashlib
import io
import os
import shutil
import stat
import subprocess
import tarfile
import tempfile
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any, BinaryIO

from tools.benchmark import evidence

REPO = "gin-gonic/gin"
SLUG = "gin-gonic__gin"
MAX_ARCHIVE = 2 * 1024**3
MAX_MEMBER = 2 * 1024**3
MAX_TOTAL = 32 * 1024**3
MAX_MEMBERS = 1_000_000
MAX_SELECTED = 200
MAX_MANIFEST = 16 * 1024**2


class CorpusBlocked(ValueError):
    """Official corpus custody cannot be established."""


@dataclass(frozen=True)
class VerifiedCorpus:
    files: dict[tuple[str, str], Path]
    digests: dict[tuple[str, str], str]
    custody: dict[str, Any]


def _require(condition: bool, reason: str) -> None:
    if not condition:
        raise CorpusBlocked(reason)


def _owner_local_archive(info: os.stat_result) -> None:
    _require(
        stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid() and info.st_nlink == 1,
        "archive is not an owner-local single-link regular file",
    )


def _digest_file(path: Path, expected: str, copy_to: Path | None = None) -> None:
    _require(
        len(expected) == 64 and all(c in "0123456789abcdef" for c in expected),
        "invalid archive digest",
    )

    def consume(handle: BinaryIO) -> None:
        info = os.fstat(handle.fileno())
        _owner_local_archive(info)
        _require(info.st_size <= MAX_ARCHIVE, "archive size limit exceeded")
        digest = hashlib.sha256()
        target = copy_to.open("xb") if copy_to else None
        try:
            while chunk := handle.read(1024 * 1024):
                digest.update(chunk)
                if target:
                    target.write(chunk)
        finally:
            if target:
                target.close()
        _require(digest.hexdigest() == expected, f"archive digest mismatch: {path}")

    try:
        evidence._consume_regular_file(path, consume)
    except evidence.EvidenceError as exc:
        raise CorpusBlocked(f"unsafe archive: {exc}") from exc


def _member_name(raw: str) -> str:
    _require(bool(raw) and "\\" not in raw and "\x00" not in raw, "unsafe tar member name")
    _require(all(ord(char) >= 32 for char in raw), "control character in tar member name")
    name = raw.removeprefix("./")
    if name.startswith("data/"):
        name = name.removeprefix("data/")
    _require(
        not name.startswith("/") and not name.startswith("./"), "absolute/noncanonical tar member"
    )
    parts = PurePosixPath(name).parts
    _require(bool(parts) and all(part not in (".", "..", "") for part in parts), "tar traversal")
    _require(PurePosixPath(name).as_posix() == name.rstrip("/"), "noncanonical tar member")
    return name.rstrip("/")


def _safe_chunk_name(release: str, commit: str) -> str:
    _require(
        len(commit) == 40 and all(c in "0123456789abcdef" for c in commit),
        f"invalid base commit: {commit}",
    )
    return f"corpus/{release}/{SLUG}/{commit}.chunks.jsonl"


def _sidecar(b06: Path, release: str, expected: str) -> None:
    filename = f"agent_retrieval_bench_{release}.tar.zst"
    path = b06 / "data/releases" / release / (filename + ".sha256")
    try:
        raw = evidence._read_control_file(path).decode("ascii")
    except (evidence.EvidenceError, UnicodeError) as exc:
        raise CorpusBlocked(f"missing/invalid archive sidecar: {release}: {exc}") from exc
    _require(
        raw.strip() == f"{expected}  releases/{release}/{filename}",
        f"archive sidecar disagrees with policy: {release}",
    )


def _extract_selected(archive: Path, destination: Path, selected: set[str]) -> dict[str, str]:
    found: dict[str, str] = {}
    seen: set[str] = set()
    declared_bytes = 0
    count = 0
    # stdin is the verified private archive copy, so zstd cannot reopen a mutable b06 path.
    with archive.open("rb") as source:
        process = subprocess.Popen(
            ["zstd", "-dc"], stdin=source, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL
        )
        try:
            _require(process.stdout is not None, "zstd has no output pipe")
            with tarfile.open(fileobj=process.stdout, mode="r|") as stream:
                for member in stream:
                    count += 1
                    _require(count <= MAX_MEMBERS, "tar member count limit exceeded")
                    if (
                        member.name in (".", "./", "data", "data/", "./data", "./data/")
                        and member.isdir()
                    ):
                        continue
                    name = _member_name(member.name)
                    _require(name not in seen, f"duplicate tar member: {name}")
                    seen.add(name)
                    _require(member.isdir() or member.isfile(), f"unsafe tar member type: {name}")
                    _require(0 <= member.size <= MAX_MEMBER, f"tar member size limit: {name}")
                    declared_bytes += member.size
                    _require(declared_bytes <= MAX_TOTAL, "tar expanded size limit exceeded")
                    if member.isdir():
                        continue
                    if name not in selected:
                        continue
                    _require(name.startswith("corpus/"), "selected non-corpus member")
                    limit = MAX_MANIFEST if name.endswith("/corpus_manifest.jsonl") else MAX_MEMBER
                    _require(member.size <= limit, f"selected member size limit: {name}")
                    target = destination.joinpath(*PurePosixPath(name).parts)
                    target.parent.mkdir(parents=True, exist_ok=True)
                    source_member = stream.extractfile(member)
                    _require(source_member is not None, f"unreadable tar member: {name}")
                    digest = hashlib.sha256()
                    remaining = member.size
                    fd = os.open(
                        target, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600
                    )
                    with os.fdopen(fd, "wb") as output:
                        while remaining:
                            block = source_member.read(min(1024 * 1024, remaining))
                            _require(bool(block), f"truncated tar member: {name}")
                            output.write(block)
                            digest.update(block)
                            remaining -= len(block)
                    found[name] = digest.hexdigest()
            trailing = 0
            while block := process.stdout.read(1024 * 1024):
                trailing += len(block)
                _require(trailing <= 1024 * 1024, "excess tar trailing data")
            _require(process.wait() == 0, "zstd decompression failed")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            if process.stdout:
                process.stdout.close()
    _require(
        set(found) == selected, f"selected corpus members missing: {sorted(selected - set(found))}"
    )
    return found


def _validate_manifest(path: Path, release: str, commits: set[str]) -> dict[str, dict[str, Any]]:
    rows: dict[str, dict[str, Any]] = {}
    with path.open("r", encoding="utf-8") as handle:
        for line in handle:
            row = evidence.parse_json(line)
            evidence.canonical_json(row)
            _require(isinstance(row, dict), "invalid corpus manifest row")
            if row.get("repo") != REPO or row.get("base_commit") not in commits:
                continue
            commit = row["base_commit"]
            _require(commit not in rows, f"duplicate corpus manifest base commit: {commit}")
            _require(row.get("status") == "ok", f"corpus manifest status not ok: {commit}")
            declared = row.get("chunks_path")
            canonical = _safe_chunk_name(release, commit)
            _require(
                type(declared) is str
                and "\\" not in declared
                and ".." not in PurePosixPath(declared).parts
                and (declared == canonical or declared.endswith("/" + canonical)),
                f"corpus manifest path mismatch: {commit}",
            )
            rows[commit] = row
    _require(set(rows) == commits, f"corpus manifest missing selected base commits: {release}")
    return rows


def _validate_chunk_file(path: Path, manifest_row: dict[str, Any]) -> None:
    """Bind extracted JSONL row identity and counts to the official manifest."""
    counts = {"file": 0, "symbol": 0}
    total = 0
    seen_files: set[str] = set()

    def consume(handle: BinaryIO) -> None:
        nonlocal total
        while line := handle.readline(1024 * 1024 + 1):
            _require(len(line) <= 1024 * 1024, f"corpus JSONL line too large: {path}")
            total += 1
            _require(total <= 2_000_000, f"corpus JSONL row limit exceeded: {path}")
            try:
                row = evidence.parse_json(line.decode("utf-8"))
                evidence.canonical_json(row)
            except (evidence.EvidenceError, UnicodeError) as exc:
                raise CorpusBlocked(f"invalid corpus JSONL: {path}: {exc}") from exc
            _require(isinstance(row, dict), f"invalid corpus row: {path}")
            _require(
                row.get("repo") == manifest_row["repo"]
                and row.get("base_commit") == manifest_row["base_commit"],
                f"corpus row snapshot identity mismatch: {path}",
            )
            kind = row.get("kind")
            _require(kind in counts, f"unexpected corpus row kind: {kind}")
            _require(
                type(row.get("path")) is str and bool(row["path"]),
                f"corpus row path missing: {path}",
            )
            _require(type(row.get("text")) is str, f"corpus row text invalid: {path}")
            if kind == "file":
                _require(row["path"] not in seen_files, f"duplicate corpus file row: {row['path']}")
                seen_files.add(row["path"])
            counts[kind] += 1

    try:
        evidence._consume_regular_file(path, consume)
    except evidence.EvidenceError as exc:
        raise CorpusBlocked(f"unsafe extracted corpus: {path}: {exc}") from exc
    for key, actual in (
        ("chunk_count", total),
        ("file_count", counts["file"]),
        ("symbol_count", counts["symbol"]),
    ):
        _require(
            type(manifest_row.get(key)) is int and manifest_row[key] == actual,
            f"corpus manifest {key} mismatch: {path}",
        )


def restore(
    b06: Path, policy: dict[str, Any], selected_by_release: dict[str, set[str]], work_root: Path
) -> VerifiedCorpus:
    """Restore selected commit chunks into a fresh child of the OS temp directory."""
    advertised_temp = Path(tempfile.gettempdir())
    canonical_temp = advertised_temp.resolve()
    _require(
        work_root.is_absolute()
        and work_root.parent in (advertised_temp, canonical_temp)
        and work_root.parent.resolve() == canonical_temp
        and work_root.name not in (".", ".."),
        "corpus work root must be a direct child of the OS temp directory",
    )
    work_root = canonical_temp / work_root.name
    _require(
        not work_root.exists() and not work_root.is_symlink(), "corpus work root already exists"
    )
    _require(
        sum(map(len, selected_by_release.values())) <= MAX_SELECTED, "too many selected commits"
    )
    _require(
        set(selected_by_release) == set(policy["arb"]["releases"]),
        "release selection differs from pinned policy",
    )
    _require(shutil.which("zstd") is not None, "zstd executable unavailable")
    work_root.mkdir(mode=0o700)
    files: dict[tuple[str, str], Path] = {}
    digests_by_snapshot: dict[tuple[str, str], str] = {}
    receipt: dict[str, Any] = {}
    try:
        for release, commits in selected_by_release.items():
            pinned = policy["arb"]["releases"][release]
            expected_archive = pinned["tar_zst_sha256"]
            _require(
                pinned["tar_zst_sha256_recomputed"] == expected_archive,
                f"policy archive digests disagree: {release}",
            )
            _sidecar(b06, release, expected_archive)
            archive_path = (
                b06 / "data/releases" / release / f"agent_retrieval_bench_{release}.tar.zst"
            )
            private_copy = work_root / f"{release}.tar.zst"
            _digest_file(archive_path, expected_archive, private_copy)
            manifest = f"corpus/{release}/corpus_manifest.jsonl"
            selected = {manifest} | {_safe_chunk_name(release, commit) for commit in commits}
            found = _extract_selected(private_copy, work_root, selected)
            _digest_file(private_copy, expected_archive)
            private_copy.unlink()
            _require(
                found[manifest] == pinned["corpus_manifest_sha256"],
                f"corpus manifest digest mismatch: {release}",
            )
            manifest_rows = _validate_manifest(work_root / manifest, release, commits)
            for commit in commits:
                _validate_chunk_file(
                    work_root / _safe_chunk_name(release, commit), manifest_rows[commit]
                )
                key = (REPO, commit)
                digest = found[_safe_chunk_name(release, commit)]
                _require(
                    key not in digests_by_snapshot or digests_by_snapshot[key] == digest,
                    f"different official corpus bytes for shared base commit: {commit}",
                )
                files.setdefault(key, work_root / _safe_chunk_name(release, commit))
                digests_by_snapshot[key] = digest
            receipt[release] = {
                "archive_sha256": expected_archive,
                "corpus_manifest_sha256": found[manifest],
                "selected_chunks_sha256": {
                    str(commit): found[_safe_chunk_name(release, commit)]
                    for commit in sorted(commits)
                },
            }
        return VerifiedCorpus(
            files,
            digests_by_snapshot,
            {"status": "VERIFIED", "work_root": str(work_root), "releases": receipt},
        )
    except Exception:
        shutil.rmtree(work_root)
        raise


def verify_extracted(corpus: VerifiedCorpus) -> None:
    """Recheck extracted bytes after official BCY consumed them."""
    root = Path(corpus.custody["work_root"])
    for release, receipt in corpus.custody["releases"].items():
        _digest_file(
            root / f"corpus/{release}/corpus_manifest.jsonl", receipt["corpus_manifest_sha256"]
        )
        for commit, digest in receipt["selected_chunks_sha256"].items():
            _digest_file(root / _safe_chunk_name(release, commit), digest)


def _load_verified_texts(path: Path, expected: str, official_loader: Any, work_root: Path):
    """Give official load_file_texts exactly the bytes hashed from one pinned descriptor."""

    def consume(handle: BinaryIO):
        info = os.fstat(handle.fileno())
        _owner_local_archive(info)
        _require(info.st_size <= MAX_MEMBER, f"selected corpus member too large: {path}")
        digest = hashlib.sha256()
        with tempfile.TemporaryFile(mode="w+b", dir=work_root) as copy:
            while block := handle.read(1024 * 1024):
                digest.update(block)
                copy.write(block)
            _require(digest.hexdigest() == expected, f"selected corpus digest mismatch: {path}")
            copy.seek(0)

            class HashedTextSource:
                def open(self, *, encoding: str):
                    _require(encoding == "utf-8", "official corpus loader changed encoding")
                    return io.TextIOWrapper(copy, encoding=encoding)

            return official_loader(HashedTextSource())

    try:
        return evidence._consume_regular_file(path, consume)
    except evidence.EvidenceError as exc:
        raise CorpusBlocked(f"unsafe selected corpus read: {path}: {exc}") from exc


def verified_file_cache(corpus: VerifiedCorpus, official_bcy: Any):
    """Official cache interface with descriptor-bound, digest-checked lazy loads."""
    work_root = Path(corpus.custody["work_root"])

    class VerifiedCache(official_bcy.CorpusFileCache):
        def file_text(self, repo: str, base_commit: str, path: str) -> str | None:
            key = (repo, base_commit)
            chunks_path = self.manifest.get(key)
            if chunks_path is None:
                return None
            if chunks_path not in self._cache:
                self._cache[chunks_path] = _load_verified_texts(
                    chunks_path,
                    corpus.digests[key],
                    official_bcy.load_file_texts,
                    work_root,
                )
            return self._cache[chunks_path].get(path)

    return VerifiedCache(corpus.files)
