"""Cross-check the Tree-sitter declaration census with an independent parser.

`source_oracle.declaration_census` defines declaration-name gold. Product
symbol producers also use Tree-sitter grammars, so a census admitted for gold
must agree, file by file, with a parser from a different implementation:
CPython `ast` (Python), `syn` (Rust), the TypeScript compiler (TypeScript and
JavaScript) and `go/ast` (Go). A language is admitted for one frozen view only
when every file of that language is censused by both sides with identical
(name start byte, name bytes) sets. Any refusal or disagreement keeps the
language unsupported for that view; it is never an empty answer set.

Checker sources and lockfiles live in `census_checkers/`; builds and caches
live outside the checkout. The audit admits a census, not labels or reviews.
"""

from __future__ import annotations

import argparse
import ast
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

try:
    from tools.benchmark.retrieval import source_oracle
except ModuleNotFoundError:  # direct script invocation
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    from tools.benchmark.retrieval import source_oracle

AUDIT_VERSION = 1
REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
CHECKERS = Path(__file__).resolve().parent / "census_checkers"
MAX_FILE_BYTES = 1024 * 1024
CHECKER_TIMEOUT_SECONDS = 600
# Python `ast` reports the `def`/`class` keyword (after decorators); the name is
# the identifier after the keyword and any whitespace or line continuations.
_PYTHON_HEAD = re.compile(rb"(?:async(?:[ \t\f]|\\\r?\n)+)?(?:def|class)(?:[ \t\f]|\\\r?\n)+")
CHECKER_FILES = {
    "rust": ("rust_checker.rs", "rust_checker.Cargo.toml", "rust_checker.Cargo.lock"),
    "go": ("go_checker.go", "go_checker.go.mod"),
    "typescript": ("ts_checker.mjs", "ts_checker.package.json", "ts_checker.package-lock.json"),
}
CHECKER_IDS = {
    "python": "cpython_ast",
    "rust": "rust_syn",
    "go": "go_ast",
    "typescript": "typescript_compiler",
    "javascript": "typescript_compiler",
}
CHECKER_ARTIFACTS = {
    "rust": ("target/release/quanta-census-syn",),
    "go": ("checker",),
    "typescript": ("checker.mjs", "node_modules/typescript/lib/typescript.js"),
}


class CensusAuditError(ValueError):
    """The independent checker could not be built or run."""


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _cache_root() -> Path:
    root = Path(
        os.environ.get("QUANTA_CENSUS_CACHE", Path.home() / ".cache/quanta-census-checkers")
    ).resolve()
    if root == REPOSITORY_ROOT or REPOSITORY_ROOT in root.parents:
        raise CensusAuditError("census checker cache must stay outside the checkout")
    return root


def _source_digest(kind: str) -> str:
    digest = hashlib.sha256()
    for name in CHECKER_FILES[kind]:
        digest.update(name.encode() + b"\0" + (CHECKERS / name).read_bytes() + b"\0")
    return digest.hexdigest()


def _run(argv: list[str], cwd: Path, stdin: bytes = b"") -> bytes:
    try:
        completed = subprocess.run(
            argv,
            cwd=cwd,
            input=stdin,
            capture_output=True,
            timeout=CHECKER_TIMEOUT_SECONDS,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise CensusAuditError(f"census checker command failed: {argv[0]}: {exc}") from exc
    if completed.returncode:
        tail = completed.stderr.decode("utf-8", "replace")[-2000:]
        raise CensusAuditError(f"census checker command failed: {argv[0]}: {tail}")
    return completed.stdout


def _artifact_digests(kind: str, root: Path) -> dict[str, str]:
    digests = {}
    for name in CHECKER_ARTIFACTS[kind]:
        path = root / name
        if path.is_symlink() or not path.is_file():
            raise CensusAuditError(f"census checker artifact is absent or linked: {name}")
        digests[name] = _sha(path.read_bytes())
    return digests


def _cache_manifest(kind: str, root: Path, source_digest: str) -> dict[str, Any]:
    return {
        "source_digest": source_digest,
        "artifact_sha256": _artifact_digests(kind, root),
    }


def _require_ready_cache(kind: str, root: Path, source_digest: str) -> None:
    marker = root / "ready.json"
    if root.is_symlink() or marker.is_symlink() or not marker.is_file():
        raise CensusAuditError("census checker cache has no trusted ready marker")
    try:
        recorded = marker.read_text()
    except OSError as exc:
        raise CensusAuditError("census checker cache marker is malformed") from exc
    expected = json.dumps(_cache_manifest(kind, root, source_digest), sort_keys=True) + "\n"
    if recorded != expected:
        raise CensusAuditError("census checker cache source or artifact differs")


def _build(kind: str) -> Path:
    """Build one checker from committed sources and lockfile into a keyed cache."""
    source_digest = _source_digest(kind)
    root = _cache_root() / f"{kind}-{source_digest}"
    if root.exists() or root.is_symlink():
        _require_ready_cache(kind, root, source_digest)
        return root
    root.parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix=f".{kind}-", dir=root.parent))
    try:
        if kind == "rust":
            (stage / "src").mkdir()
            shutil.copyfile(CHECKERS / "rust_checker.rs", stage / "src/main.rs")
            shutil.copyfile(CHECKERS / "rust_checker.Cargo.toml", stage / "Cargo.toml")
            shutil.copyfile(CHECKERS / "rust_checker.Cargo.lock", stage / "Cargo.lock")
            _run(["cargo", "build", "--release", "--locked", "--quiet"], stage)
        elif kind == "go":
            shutil.copyfile(CHECKERS / "go_checker.go", stage / "main.go")
            shutil.copyfile(CHECKERS / "go_checker.go.mod", stage / "go.mod")
            env_go = ["env", "GOTOOLCHAIN=local", "GOFLAGS=-mod=mod", "CGO_ENABLED=0"]
            _run([*env_go, "go", "build", "-trimpath", "-o", "checker", "."], stage)
        else:
            shutil.copyfile(CHECKERS / "ts_checker.mjs", stage / "checker.mjs")
            shutil.copyfile(CHECKERS / "ts_checker.package.json", stage / "package.json")
            shutil.copyfile(CHECKERS / "ts_checker.package-lock.json", stage / "package-lock.json")
            _run(["npm", "ci", "--ignore-scripts", "--no-audit", "--no-fund", "--silent"], stage)
        (stage / "ready.json").write_text(
            json.dumps(_cache_manifest(kind, stage, source_digest), sort_keys=True) + "\n"
        )
        try:
            stage.rename(root)
        except OSError:
            _require_ready_cache(kind, root, source_digest)
    finally:
        if stage.exists():
            shutil.rmtree(stage, ignore_errors=True)
    return root


def checker_identity(language: str) -> dict[str, Any]:
    checker = CHECKER_IDS[language]
    if language == "python":
        return {
            "id": checker,
            "version": sys.version.split()[0],
            "source_digest": _sha(Path(__file__).read_bytes()),
        }
    kind = "typescript" if language in ("typescript", "javascript") else language
    root = _build(kind)
    if kind == "rust":
        lock = (CHECKERS / "rust_checker.Cargo.lock").read_text()
        syn = re.search(r'name = "syn"\nversion = "([^"]+)"', lock)
        version = (
            "syn "
            + (syn.group(1) if syn else "unknown")
            + "; "
            + _run(["rustc", "--version"], root).decode().strip()
        )
    elif kind == "go":
        version = _run(["go", "version"], root).decode().strip()
    else:
        package = json.loads((root / "node_modules/typescript/package.json").read_text())
        version = (
            "typescript "
            + package["version"]
            + "; node "
            + _run(["node", "--version"], root).decode().strip()
        )
    return {
        "id": checker,
        "version": version,
        "source_digest": _source_digest(kind),
        "artifact_sha256": _artifact_digests(kind, root),
    }


def _python_ast(raw: bytes) -> set[tuple[int, bytes]]:
    try:
        tree = ast.parse(raw)
    except (SyntaxError, ValueError) as exc:
        raise CensusAuditError(f"parse: {exc.__class__.__name__}: {exc}") from exc
    starts, offset = [], 0
    for line in raw.splitlines(keepends=True):
        starts.append(offset)
        offset += len(line)
    rows = set()
    for node in ast.walk(tree):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            head = _PYTHON_HEAD.match(raw, starts[node.lineno - 1] + node.col_offset)
            name = node.name.encode("utf-8")
            if head is None or raw[head.end() : head.end() + len(name)] != name:
                raise CensusAuditError(
                    f"python declaration name is not at its keyword: {node.name}"
                )
            rows.add((head.end(), name))
    return rows


def _rust_offsets(raw: bytes, rows: list[list[Any]]) -> set[tuple[int, bytes]]:
    starts, offset = [], 0
    for line in raw.split(b"\n"):
        starts.append(offset)
        offset += len(line) + 1
    text_lines = raw.decode("utf-8").split("\n")
    found = set()
    for name, _kind, line, column in rows:
        start = starts[line - 1] + len(text_lines[line - 1][:column].encode("utf-8"))
        found.add((start, name.encode("utf-8")))
    return found


def _external(language: str, paths: list[Path]) -> dict[Path, Any]:
    kind = "typescript" if language in ("typescript", "javascript") else language
    root = _build(kind)
    argv = {
        "rust": [str(root / "target/release/quanta-census-syn")],
        "go": [str(root / "checker")],
        "typescript": ["node", str(root / "checker.mjs")],
    }[kind]
    stdin = "".join(f"{path}\n" for path in paths).encode("utf-8")
    results = {}
    for line in _run(argv, root, stdin).decode("utf-8").splitlines():
        row = json.loads(line)
        results[Path(row["path"])] = row
    if set(results) != set(paths):
        raise CensusAuditError("census checker output inventory differs from its input")
    return results


def file_set_sha256(language: str, raws: dict[str, bytes]) -> str:
    """Identity of the exact bytes of one language's files, for binding callers."""
    selected = sorted(path for path in raws if source_oracle.declaration_language(path) == language)
    return _sha(json.dumps([[path, _sha(raws[path])] for path in selected]).encode("utf-8"))


def audit_files(language: str, view: Path, paths: list[str]) -> dict[str, Any]:
    """Compare both censuses for every listed file of one language in one view."""
    if language not in CHECKER_IDS:
        raise CensusAuditError("unsupported census audit language")
    selected = sorted(
        path for path in paths if source_oracle.declaration_language(path) == language
    )
    raws = {}
    for path in selected:
        absolute = view / path
        if absolute.is_symlink() or not absolute.is_file():
            raise CensusAuditError(f"census audit source is not a regular file: {path}")
        raw = absolute.read_bytes()
        if len(raw) > MAX_FILE_BYTES:
            raise CensusAuditError(f"census audit source exceeds file limit: {path}")
        raws[path] = raw
    checker_before = checker_identity(language)
    external = (
        _external(language, [view / path for path in selected])
        if language != "python" and selected
        else {}
    )
    refused, disagreements, admitted_paths, declarations = [], [], [], 0
    for path in selected:
        raw = raws[path]
        try:
            tree_sitter = {
                (start, raw[start:end])
                for start, end, *_definition in source_oracle.declaration_census(
                    language, path, raw
                )
            }
        except source_oracle.SourceOracleError as exc:
            tree_sitter = exc
        try:
            if language == "python":
                independent = _python_ast(raw)
            else:
                row = external[view / path]
                if "error" in row:
                    raise CensusAuditError(row["error"])
                if language == "rust":
                    independent = _rust_offsets(raw, row["declarations"])
                else:
                    independent = {
                        (offset, name.encode("utf-8"))
                        for name, _kind, offset in row["declarations"]
                    }
        except CensusAuditError as exc:
            independent = exc
        failures = [
            (side, str(value))
            for side, value in (("tree_sitter", tree_sitter), ("independent", independent))
            if isinstance(value, Exception)
        ]
        if failures:
            refused.append({"path": path, "refusals": [list(row) for row in failures]})
            continue
        if tree_sitter != independent:
            disagreements.append(
                {
                    "path": path,
                    "only_tree_sitter": sorted(
                        [start, name.decode("utf-8", "replace")]
                        for start, name in tree_sitter - independent
                    ),
                    "only_independent": sorted(
                        [start, name.decode("utf-8", "replace")]
                        for start, name in independent - tree_sitter
                    ),
                }
            )
            continue
        admitted_paths.append(path)
        declarations += len(tree_sitter)
    checker_after = checker_identity(language)
    if checker_after != checker_before:
        raise CensusAuditError("census checker changed during audit")
    return {
        "schema_version": 1,
        "kind": "declaration_census_audit",
        "audit_version": AUDIT_VERSION,
        "language": language,
        "census": source_oracle.DECLARATION_CENSUS[language],
        "comparison": "name_start_byte_and_name_bytes_set_equality",
        "checker": checker_before,
        "files": len(selected),
        "file_set_sha256": file_set_sha256(language, raws),
        "agreeing_files": len(selected) - len(refused) - len(disagreements),
        "admitted_paths": admitted_paths,
        "excluded_paths": sorted(
            [row["path"] for row in refused] + [row["path"] for row in disagreements]
        ),
        "agreeing_declarations": declarations,
        "refused": refused,
        "disagreements": disagreements,
        "status": "admitted" if selected and not refused and not disagreements else "unsupported",
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--release", required=True, type=Path)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--view", default="code_only")
    parser.add_argument("--language", required=True, choices=sorted(CHECKER_IDS))
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(REPOSITORY_ROOT / "tools/benchmark"))
    import corpus_release

    try:
        output = args.output.resolve()
        if output.exists() or output == REPOSITORY_ROOT or REPOSITORY_ROOT in output.parents:
            raise CensusAuditError("audit output must be new and outside the checkout")
        document = corpus_release.validate(args.release.resolve())
        repository = next(
            row for row in document["repositories"] if row["recipe"]["name"] == args.repository
        )
        metadata = repository["views"][args.view]
        manifest = json.loads((args.release / metadata["manifest"]).read_bytes())
        view = args.release.resolve() / "views" / args.repository / args.view
        result = audit_files(args.language, view, [row["path"] for row in manifest["files"]])
        result.update(
            release_digest=document["digest"],
            repository=args.repository,
            repository_commit=repository["recipe"]["revision"],
            view=args.view,
            manifest_digest=metadata["manifest_digest"],
        )
    except (OSError, ValueError, StopIteration, KeyError) as exc:
        parser.exit(2, f"ERROR: {exc}\n")
    with output.open("x", encoding="utf-8") as stream:
        json.dump(result, stream, indent=2, sort_keys=True)
        stream.write("\n")
    print(json.dumps({key: result[key] for key in ("language", "files", "status")}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
