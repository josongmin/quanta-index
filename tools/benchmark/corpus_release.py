"""One external, content-addressed corpus release; no index/gold admission.

Views contain canonical Git blob bytes, not filtered/smudged working-tree data.
Validation restores retained bundles and re-derives the complete inventory.
"""

from __future__ import annotations

import hashlib
import os
import select
import shutil
import signal
import stat
import subprocess
import tempfile
import time
import unicodedata
from pathlib import Path

from evidence import (
    EvidenceError,
    _read_regular_file,
    _sync_dir,
    canonical_json,
    digest_bytes,
    parse_json,
)
from producer_execution import execute

ROOT = Path(__file__).resolve().parents[2]
VIEWS = ("code_only", "developer_search")
POLICY = {
    "id": "git-text-views-v1",
    "max_file_bytes": 1024 * 1024,
    "code_extensions": sorted({".rs", ".py", ".go", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"}),
    "excluded_components": sorted(
        {"vendor", "third_party", "node_modules", "target", "dist", "generated"}
    ),
    "symlinks": "exclude_without_following",
    "submodules": "exclude_without_recursing",
    "git_lfs": "exclude_pointer_without_fetching",
    "encoding": "nonempty_utf8_without_nul_or_exotic_line_breaks",
    "case_collisions": "refuse_nfc_casefold_aliases_including_parent_components",
    "generated_vendor": "exclude_declared_components_no_content_heuristic",
    "path_scope": "complete_repository_not_candidate_benchmark_root",
}
EXOTIC = frozenset("\v\f\x1c\x1d\x1e\x85\u2028\u2029")


def environment() -> dict:
    # Local Git objects only; do not forward unrelated service credentials.
    result = {
        key: value
        for key, value in os.environ.items()
        if key in {"PATH", "LANG", "LC_ALL", "TZ", "TMPDIR", "SYSTEMROOT", "WINDIR"}
    }
    result.update(
        GIT_CONFIG_GLOBAL=os.devnull,
        GIT_CONFIG_NOSYSTEM="1",
        GIT_NO_REPLACE_OBJECTS="1",
        GIT_TERMINAL_PROMPT="0",
        GIT_OPTIONAL_LOCKS="0",
        GIT_ALLOW_PROTOCOL="file",
    )
    return result


def git(root: Path, *argv: str) -> bytes:
    try:
        stdout, _stderr, _command = execute(
            ["git", "-C", str(root), *argv],
            cwd=root,
            env=environment(),
            timeout=300,
            log_dir=Path(tempfile.mkdtemp(prefix="quanta-corpus-git-")).resolve(),
        )
    except (OSError, ValueError) as exc:
        raise EvidenceError(f"corpus Git action failed: {exc}") from exc
    return stdout.read_control()


def canonical_path(value: str) -> None:
    if (
        not value
        or value.startswith("/")
        or "\\" in value
        or any(part in {"", ".", ".."} or part.casefold() == ".git" for part in value.split("/"))
    ):
        raise EvidenceError("corpus contains a noncanonical tracked path")


def regular_tree(root: Path) -> set[str]:
    if root.is_symlink() or any(parent.is_symlink() for parent in root.absolute().parents):
        raise EvidenceError("corpus release root or ancestor is a symlink")
    names = set()
    for directory, directories, files in os.walk(root, followlinks=False):
        base = Path(directory)
        if any((base / name).is_symlink() for name in directories):
            raise EvidenceError("corpus release contains a linked directory")
        for name in files:
            path = base / name
            if not stat.S_ISREG(path.lstat().st_mode):
                raise EvidenceError("corpus release contains a link or special file")
            names.add(path.relative_to(root).as_posix())
    return names


def validate_spec(spec: dict) -> list[dict]:
    keys = {
        "name",
        "language",
        "url",
        "revision",
        "benchmark_root",
        "upstream_semble_benchmark_overlap",
    }
    if (
        not isinstance(spec, dict)
        or set(spec) != {"source_revision", "repositories"}
        or not isinstance(spec["source_revision"], str)
        or not spec["source_revision"]
        or not isinstance(spec["repositories"], list)
        or not spec["repositories"]
    ):
        raise EvidenceError("corpus recipe is malformed or empty")
    seen = set()
    for entry in spec["repositories"]:
        if not isinstance(entry, dict) or set(entry) != keys:
            raise EvidenceError("corpus recipe repository has malformed keys")
        name = entry["name"]
        revision = entry["revision"]
        if (
            not isinstance(name, str)
            or not name.isascii()
            or not name.replace("-", "").isalnum()
            or name.casefold() in seen
            or not isinstance(revision, str)
            or len(revision) not in (40, 64)
            or any(c not in "0123456789abcdef" for c in revision)
            or type(entry["upstream_semble_benchmark_overlap"]) is not bool
            or any(not isinstance(entry[key], str) or not entry[key] for key in ("language", "url"))
            or not isinstance(entry["benchmark_root"], str)
        ):
            raise EvidenceError("corpus recipe identity is invalid or duplicate")
        if entry["benchmark_root"]:
            canonical_path(entry["benchmark_root"])
        seen.add(name.casefold())
    return sorted(spec["repositories"], key=lambda entry: entry["name"])


def verify_checkout(root: Path, revision: str) -> None:
    if root.is_symlink() or not root.is_dir():
        raise EvidenceError("corpus checkout must be a regular Git root")
    if (
        Path(git(root, "rev-parse", "--show-toplevel").decode().strip()).resolve() != root.resolve()
        or git(root, "rev-parse", "HEAD").decode().strip() != revision
        or git(root, "status", "--porcelain=v1", "--untracked-files=all")
    ):
        raise EvidenceError("corpus checkout root, revision or clean state differs")


class Blobs:
    """Bounded streaming Git object reader; retains at most one view-size file."""

    def __init__(self, root: Path):
        self.errors = tempfile.TemporaryFile()
        self.process = subprocess.Popen(
            ["git", "-C", str(root), "cat-file", "--batch"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self.errors,
            env=environment(),
            bufsize=0,
            start_new_session=True,
        )
        self.deadline = time.monotonic() + 300

    def __enter__(self):
        return self

    def read(self, size: int) -> bytes:
        result = bytearray()
        while len(result) < size:
            remaining = self.deadline - time.monotonic()
            if remaining <= 0 or not select.select([self.process.stdout], [], [], remaining)[0]:
                raise EvidenceError("corpus Git blob read timed out")
            data = os.read(self.process.stdout.fileno(), size - len(result))
            if not data:
                raise EvidenceError("corpus Git blob stream is truncated")
            result.extend(data)
        return bytes(result)

    def blob(self, oid: str) -> tuple[str, int, bytes | None]:
        self.process.stdin.write(oid.encode() + b"\n")
        header = bytearray()
        while len(header) < 256:
            character = self.read(1)
            if character == b"\n":
                break
            header.extend(character)
        else:
            raise EvidenceError("corpus Git blob header is oversized")
        fields = bytes(header).split()
        if len(fields) != 3 or fields[0] != oid.encode() or fields[1] != b"blob":
            raise EvidenceError("corpus Git blob identity or type differs")
        try:
            size = int(fields[2])
        except ValueError as exc:
            raise EvidenceError("corpus Git blob size is malformed") from exc
        if size < 0:
            raise EvidenceError("corpus Git blob size is negative")
        content = bytearray() if size <= POLICY["max_file_bytes"] else None
        digest = hashlib.sha256()
        object_digest = hashlib.sha1() if len(oid) == 40 else hashlib.sha256()
        object_digest.update(f"blob {size}\0".encode())
        remaining = size
        while remaining:
            data = self.read(min(65536, remaining))
            digest.update(data)
            object_digest.update(data)
            if content is not None:
                content.extend(data)
            remaining -= len(data)
        if self.read(1) != b"\n" or object_digest.hexdigest() != oid:
            raise EvidenceError("corpus Git object bytes disagree with object identity")
        return digest.hexdigest(), size, bytes(content) if content is not None else None

    def __exit__(self, kind, _value, _traceback):
        try:
            self.process.stdin.close()
            if kind is not None:
                try:
                    os.killpg(self.process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            try:
                code = self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(self.process.pid, signal.SIGKILL)
                self.process.wait(timeout=5)
                raise EvidenceError("corpus Git blob process did not terminate") from None
            if kind is None and code:
                raise EvidenceError("corpus Git blob process failed")
        finally:
            self.process.stdout.close()
            self.errors.close()


def tree_rows(root: Path, revision: str) -> list[dict]:
    rows = []
    aliases = {}
    for line in git(root, "ls-tree", "-rz", "--full-tree", revision).split(b"\0"):
        if not line:
            continue
        metadata, raw_path = line.split(b"\t", 1)
        mode, kind, oid = metadata.decode("ascii").split()
        path = raw_path.decode("utf-8", "strict")
        canonical_path(path)
        for count in range(1, len(path.split("/")) + 1):
            prefix = "/".join(path.split("/")[:count])
            alias = unicodedata.normalize("NFC", prefix).casefold()
            if alias in aliases and aliases[alias] != prefix:
                raise EvidenceError("corpus tracked paths have a case/normalization collision")
            aliases[alias] = prefix
        if (mode, kind) not in {
            ("100644", "blob"),
            ("100755", "blob"),
            ("120000", "blob"),
            ("160000", "commit"),
        }:
            raise EvidenceError("corpus has an unsupported tracked object mode")
        rows.append({"path": path, "git_mode": mode, "git_object_id": oid})
    if not rows or len({row["path"] for row in rows}) != len(rows):
        raise EvidenceError("corpus tracked inventory is empty or duplicate")
    return sorted(rows, key=lambda row: row["path"])


def exclusion(row: dict, content: bytes | None) -> str | None:
    if row["git_mode"] == "160000":
        return "submodule"
    if row["git_mode"] == "120000":
        return "symlink"
    if any(part in POLICY["excluded_components"] for part in row["path"].split("/")):
        return "generated_or_vendor_component"
    if not row["size_bytes"] or content is None:
        return "empty_or_oversize"
    if content.startswith(b"version https://git-lfs.github.com/spec/v1\n"):
        return "git_lfs_pointer"
    try:
        text = content.decode("utf-8", "strict")
    except UnicodeDecodeError:
        return "non_utf8"
    if "\0" in text or any(character in EXOTIC for character in text):
        return "binary_or_exotic_line_break"
    return None


def write(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("xb") as handle:
        handle.write(data)
        handle.flush()
        os.fsync(handle.fileno())


def freeze_repo(entry: dict, checkouts: Path, stage: Path) -> dict:
    root = checkouts / entry["name"]
    verify_checkout(root, entry["revision"])
    rows = tree_rows(root, entry["revision"])
    selected = {view: [] for view in VIEWS}
    licenses = []
    with Blobs(root) as blobs:
        for row in rows:
            content = None
            if row["git_mode"] == "160000":
                row.update(sha256=None, size_bytes=None)
            else:
                sha, size, content = blobs.blob(row["git_object_id"])
                row.update(sha256=sha, size_bytes=size)
            reason = exclusion(row, content)
            row["view_exclusions"] = {
                "developer_search": reason,
                "code_only": reason
                or (
                    None
                    if Path(row["path"]).suffix.lower() in POLICY["code_extensions"]
                    else "non_code_extension"
                ),
            }
            license_name = row["path"].lower()
            if "/" not in license_name and any(
                license_name == name or license_name.startswith(name + separator)
                for name in ("license", "licence", "copying", "unlicense")
                for separator in (".", "-", "_")
            ):
                if reason:
                    raise EvidenceError("corpus license source is not admitted regular text")
                licenses.append(
                    {"path": row["path"], "sha256": row["sha256"], "approval": "not_attested"}
                )
            if any(value is None for value in row["view_exclusions"].values()):
                blob = stage / "blobs" / row["sha256"]
                if not blob.exists():
                    write(blob, content)
                elif digest_bytes(_read_regular_file(blob)) != "sha256:" + row["sha256"]:
                    raise EvidenceError("corpus content-addressed blob differs")
                for view, excluded in row["view_exclusions"].items():
                    if excluded is None:
                        selected[view].append({"path": row["path"], "file_sha256": row["sha256"]})
                        target = stage / "views" / entry["name"] / view / row["path"]
                        write(target, content)
                        target.chmod(0o555 if row["git_mode"] == "100755" else 0o444)
    if not licenses or any(not files for files in selected.values()):
        raise EvidenceError("corpus requires license source and two nonempty views")
    views = {}
    for view, files in selected.items():
        manifest = {"repository_commit": entry["revision"], "files": files}
        relative = f"manifests/{entry['name']}/{view}.json"
        data = canonical_json(manifest).encode()
        write(stage / relative, data)
        views[view] = {
            "manifest": relative,
            "manifest_digest": digest_bytes(data),
            "file_count": len(files),
            "file_universe_digest": digest_bytes(canonical_json(files).encode()),
        }
    verify_checkout(root, entry["revision"])
    return {"recipe": entry, "tracked_inventory": rows, "license_sources": licenses, "views": views}


def build(spec: dict, checkouts: Path, stage: Path, bundles: Path | None = None) -> dict:
    entries = validate_spec(spec)
    repositories = []
    write(stage / "recipe.json", canonical_json(spec).encode())
    for entry in entries:
        repository = freeze_repo(entry, checkouts, stage)
        relative = f"bundles/{entry['name']}.bundle"
        target = stage / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        if bundles is None:
            git(checkouts / entry["name"], "bundle", "create", str(target), "HEAD")
        else:
            shutil.copyfile(bundles / f"{entry['name']}.bundle", target)
        repository["bundle"] = {"path": relative, "digest": file_digest(target)}
        repositories.append(repository)
    body = {
        "schema_version": 1,
        "kind": "external_corpus_release",
        "status": "frozen_not_admitted",
        "policy": POLICY,
        "generator_digest": digest_bytes(_read_regular_file(Path(__file__))),
        "recipe_digest": digest_bytes(canonical_json(spec).encode()),
        "repositories": repositories,
    }
    document = {**body, "digest": digest_bytes(canonical_json(body).encode())}
    write(stage / "release.json", canonical_json(document).encode())
    return document


def file_digest(path: Path) -> str:
    digest = hashlib.sha256()
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as stream:
        before = os.fstat(stream.fileno())
        if not stat.S_ISREG(before.st_mode):
            raise EvidenceError("corpus artifact must be a regular file")
        while data := stream.read(65536):
            digest.update(data)
        after = os.fstat(stream.fileno())
    stable = ("st_dev", "st_ino", "st_mode", "st_size", "st_mtime_ns", "st_ctime_ns")
    if any(getattr(before, key) != getattr(after, key) for key in stable) or (
        before.st_dev,
        before.st_ino,
    ) != (
        path.lstat().st_dev,
        path.lstat().st_ino,
    ):
        raise EvidenceError("corpus artifact changed during digest verification")
    return "sha256:" + digest.hexdigest()


def external(path: Path) -> None:
    if path.resolve().is_relative_to(ROOT):
        raise EvidenceError("corpus inputs and releases must stay outside the checkout")
    if path.is_symlink() or any(parent.is_symlink() for parent in path.absolute().parents):
        raise EvidenceError("corpus path or ancestor is a symlink")


def create(spec_path: Path, checkouts: Path, target: Path, *, source_guard=None) -> dict:
    initial_owner = digest_bytes(_read_regular_file(Path(__file__)))
    for path in (spec_path, checkouts, target):
        external(path)
    if target.exists():
        raise EvidenceError("corpus release destination already exists; never overwrite")
    if target.resolve().is_relative_to(checkouts.resolve()) or checkouts.resolve().is_relative_to(
        target.resolve()
    ):
        raise EvidenceError("corpus release overlaps its source checkouts")
    spec = parse_json(_read_regular_file(spec_path).decode())
    validate_spec(spec)
    target.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".corpus-stage-", dir=target.parent) as temporary:
        stage = Path(temporary) / "release"
        stage.mkdir()
        document = build(spec, checkouts, stage)
        validate(stage)
        # Cooperative writers serialize on the parent; the nonempty final
        # directory is never replaced by a second publisher.
        from custody import custody

        with custody(target.parent):
            if target.exists() or target.is_symlink():
                raise EvidenceError("corpus release destination was concurrently created")
            if digest_bytes(_read_regular_file(Path(__file__))) != initial_owner:
                raise EvidenceError("corpus owner changed during capture")
            if source_guard is not None:
                source_guard()
            for relative in regular_tree(stage):
                descriptor = os.open(stage / relative, os.O_RDONLY | os.O_NOFOLLOW)
                try:
                    os.fsync(descriptor)
                finally:
                    os.close(descriptor)
            for directory, _children, _files in sorted(
                os.walk(stage), key=lambda item: -len(Path(item[0]).parts)
            ):
                _sync_dir(Path(directory))
            os.rename(stage, target)
            _sync_dir(target.parent)
    return document


def validate(root: Path) -> dict:
    initial_owner = digest_bytes(_read_regular_file(Path(__file__)))
    external(root)
    actual_files = regular_tree(root)
    document = parse_json(_read_regular_file(root / "release.json").decode())
    spec = parse_json(_read_regular_file(root / "recipe.json").decode())
    entries = validate_spec(spec)
    if (
        not isinstance(document, dict)
        or set(document)
        != {
            "schema_version",
            "kind",
            "status",
            "policy",
            "generator_digest",
            "recipe_digest",
            "repositories",
            "digest",
        }
        or type(document["schema_version"]) is not int
        or document["schema_version"] != 1
        or document["kind"] != "external_corpus_release"
        or document["status"] != "frozen_not_admitted"
        or document["policy"] != POLICY
        or document["generator_digest"] != digest_bytes(_read_regular_file(Path(__file__)))
        or not isinstance(document["repositories"], list)
        or len(document["repositories"]) != len(entries)
    ):
        raise EvidenceError("corpus release identity, policy or inventory differs")
    with tempfile.TemporaryDirectory(prefix="quanta-corpus-replay-") as temporary:
        scratch = Path(temporary).resolve()
        checkouts = scratch / "checkouts"
        checkouts.mkdir()
        for entry, repository in zip(entries, document["repositories"], strict=True):
            relative = f"bundles/{entry['name']}.bundle"
            if (
                not isinstance(repository, dict)
                or repository.get("recipe") != entry
                or repository.get("bundle")
                != {"path": relative, "digest": file_digest(root / relative)}
            ):
                raise EvidenceError("corpus bundle/recipe inventory differs")
            git(
                checkouts,
                "clone",
                "--quiet",
                "--",
                str(root / relative),
                str(checkouts / entry["name"]),
            )
        expected = build(spec, checkouts, scratch / "expected", bundles=root / "bundles")
        if expected != document or regular_tree(scratch / "expected") != actual_files:
            raise EvidenceError(
                "corpus release differs from retained Git objects or complete file inventory"
            )
        for name in actual_files:
            if name.startswith("bundles/"):
                continue
            if file_digest(root / name) != file_digest(scratch / "expected" / name):
                raise EvidenceError("corpus materialized view, manifest or blob bytes differ")
            if name.startswith("views/") and stat.S_IMODE(
                (root / name).stat().st_mode
            ) != stat.S_IMODE((scratch / "expected" / name).stat().st_mode):
                raise EvidenceError("corpus materialized view executable/read-only mode differs")
    if digest_bytes(_read_regular_file(Path(__file__))) != initial_owner:
        raise EvidenceError("corpus owner changed during validation")
    return document
