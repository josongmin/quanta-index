"""Compare vendor and registry SymbolRecord producers in separate processes.

This is a local differential smoke test. Artifact digests are measured from the
given paths for the run; release custody requires an external trusted manifest.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import stat
import subprocess
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
from tools.benchmark.retrieval.projection_process import (  # noqa: E402
    ProjectionProcessError,
    run_helper,
)

FIXTURES = (
    (
        "go",
        "candidate.go",
        "package p\ntype Rect struct{W float64}\nfunc Run(){_=new(1)}\n",
    ),
    (
        "syntax",
        "alias.js",
        "function local() {} export { local as const };",
    ),
    (
        "syntax",
        "star.ts",
        "export type * as HttpProxy from './basic'\nfunction sentinel() {}",
    ),
    (
        "syntax",
        "star.tsx",
        "export type * as HttpProxy from './basic'\nfunction sentinel() { return <div/> }",
    ),
)

CORPUS_PATHS = (
    "tools/ci/tests/fixtures/declaration_census/go.go",
    "tools/ci/tests/fixtures/declaration_census/javascript.js",
    "tools/ci/tests/fixtures/declaration_census/app.jsx",
    "tools/ci/tests/fixtures/declaration_census/typescript.ts",
    "tools/ci/tests/fixtures/declaration_census/component.tsx",
    "tools/ci/tests/fixtures/gold_oracle/typescript.ts",
    "benchmarks/retrieval/tests/fixtures/l5_parser/type_namespace_export.ts",
    "benchmarks/retrieval/tests/fixtures/l5_parser/generic_import_trailing_comma.ts",
    "tools/benchmark/retrieval/go_projection/main.go",
    "tools/benchmark/retrieval/go_projection/main_test.go",
    "tools/benchmark/retrieval/ts_projection.cjs",
    "tools/benchmark/retrieval/census_checkers/ts_checker.mjs",
)

NEGATIVE_PATHS = ("benchmarks/retrieval/tests/fixtures/l5_parser/malformed.ts",)
SUPPORTED_SUFFIXES = {".go", ".js", ".jsx", ".ts", ".tsx", ".mts", ".cts"}


def digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1 << 20), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def load_corpus_manifest(
    manifest: Path, expected_sha256: str, corpus_root: Path
) -> list[tuple[str, str, str, bool, int]]:
    """Read a pinned corpus with an exact file and record denominator."""
    with manifest.open("rb") as stream:
        raw = stream.read((8 << 20) + 1)
    if len(raw) > 8 << 20:
        raise ValueError("corpus manifest exceeds 8 MiB")
    if hashlib.sha256(raw).hexdigest() != expected_sha256:
        raise ValueError("corpus manifest digest differs")

    def unique_pairs(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"duplicate corpus manifest field: {key}")
            result[key] = value
        return result

    document: Any = json.loads(raw, object_pairs_hook=unique_pairs)
    if not isinstance(document, dict) or set(document) != {"schema", "files"}:
        raise ValueError("corpus manifest envelope is invalid")
    if document["schema"] != "quanta-symbol-corpus-v1":
        raise ValueError("corpus manifest schema differs")
    entries = document["files"]
    if not isinstance(entries, list) or not entries:
        raise ValueError("corpus manifest has no files")
    root = corpus_root.resolve(strict=True)
    cases = []
    seen = set()
    for entry in entries:
        if not isinstance(entry, dict) or set(entry) != {"path", "sha256", "status", "records"}:
            raise ValueError("corpus file entry is invalid")
        path = entry["path"]
        expected = entry["sha256"]
        status = entry["status"]
        count = entry["records"]
        if (
            not isinstance(path, str)
            or not path
            or path in seen
            or Path(path).is_absolute()
            or any(part in (".", "..") for part in Path(path).parts)
            or Path(path).suffix not in SUPPORTED_SUFFIXES
            or not isinstance(expected, str)
            or len(expected) != 64
            or any(character not in "0123456789abcdef" for character in expected)
            or status not in ("ok", "parse_failure")
            or type(count) is not int
            or count < 0
            or (status == "parse_failure" and count != 0)
        ):
            raise ValueError(f"invalid corpus entry: {path!r}")
        seen.add(path)
        source_path = root / path
        resolved = source_path.resolve(strict=True)
        if not resolved.is_relative_to(root):
            raise ValueError(f"corpus file escaped its root: {path}")
        try:
            descriptor = os.open(source_path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        except OSError as error:
            raise ValueError(f"corpus file cannot be opened safely: {path}") from error
        with os.fdopen(descriptor, "rb") as stream:
            if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
                raise ValueError(f"corpus path is not a regular file: {path}")
            data = stream.read((1 << 20) + 1)
        if len(data) > 1 << 20:
            raise ValueError(f"corpus file exceeds producer limit: {path}")
        if hashlib.sha256(data).hexdigest() != expected:
            raise ValueError(f"corpus file digest differs: {path}")
        source = data.decode("utf-8")
        cases.append(
            (
                "go" if path.endswith(".go") else "syntax",
                path,
                source,
                status == "parse_failure",
                count,
            )
        )
    return cases


def records(
    binary: Path,
    args: list[str],
    source: str,
    path: str,
    *,
    parse_failure: bool,
    expected_records: int | None = None,
) -> list[dict] | None:
    result = subprocess.run(
        [str(binary), *args],
        input=source.encode(),
        capture_output=True,
        timeout=90,
    )
    if parse_failure:
        stderr = result.stderr.lower()
        if result.returncode == 0 or not (b"parse failure" in stderr or b"parsefailure" in stderr):
            raise AssertionError(
                f"{path}: expected explicit parse failure from {binary}; "
                f"exit={result.returncode} stderr={result.stderr[:500]!r}"
            )
        return None
    if result.returncode != 0:
        raise AssertionError(
            f"{path}: {binary} failed; exit={result.returncode} stderr={result.stderr[:500]!r}"
        )
    decoded = json.loads(result.stdout)
    if not isinstance(decoded, list) or (expected_records is None and not decoded):
        raise ValueError(f"{path}: {binary}: expected record list")
    if expected_records is not None and len(decoded) != expected_records:
        raise AssertionError(f"{path}: {binary}: record denominator differs")
    return decoded


def candidate_batch_rows(binary: Path, args: list[str], cases: list[tuple]) -> list[dict]:
    """Keep each producer invocation within its 32-file and 16 MiB input limits."""
    rows: list[dict] = []
    for offset in range(0, len(cases), 8):
        batch = cases[offset : offset + 8]
        payload = json.dumps(
            [{"path": path, "source": source} for path, source, *_ in batch]
        ).encode()
        if len(payload) > 16 << 20:
            raise ValueError("candidate batch exceeds 16 MiB input limit")
        try:
            stdout, stderr, returncode = run_helper(
                [str(binary), *args], payload, timeout_seconds=600, max_stdout=8 << 20
            )
        except ProjectionProcessError as error:
            raise AssertionError(f"{binary}: candidate batch failed: {error.kind}") from error
        if returncode or stderr:
            raise AssertionError(f"{binary} failed; exit={returncode} stderr={stderr[:1000]!r}")
        chunk = json.loads(stdout)
        if not isinstance(chunk, list) or len(chunk) != len(batch):
            raise AssertionError(f"{binary}: candidate batch row count differs")
        rows.extend(chunk)
    return rows


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--vendor-bin", required=True, type=Path)
    parser.add_argument("--official-bin", required=True, type=Path)
    parser.add_argument("--official-batch-bin", required=True, type=Path)
    parser.add_argument("--official-go-batch-bin", type=Path)
    parser.add_argument("--go-helper", required=True, type=Path)
    parser.add_argument("--go-version", required=True)
    parser.add_argument("--node", required=True, type=Path)
    parser.add_argument("--node-version", required=True)
    parser.add_argument("--typescript-js", required=True, type=Path)
    parser.add_argument("--typescript-version", default="5.9.3")
    parser.add_argument("--script", required=True, type=Path)
    parser.add_argument("--node-library", action="append", type=Path, default=[])
    parser.add_argument("--corpus-manifest", type=Path)
    parser.add_argument("--corpus-manifest-sha256")
    parser.add_argument("--corpus-root", type=Path, default=ROOT)
    options = parser.parse_args()
    if bool(options.corpus_manifest) != bool(options.corpus_manifest_sha256):
        raise ValueError("corpus manifest and digest must be supplied together")
    if options.vendor_bin.resolve() == options.official_bin.resolve():
        raise ValueError("vendor and official binaries must be distinct")
    if len(options.node_library) > 8:
        raise ValueError("at most eight Node libraries are supported")

    go_digest = digest(options.go_helper)
    node_digest = digest(options.node)
    compiler_digest = digest(options.typescript_js)
    script_digest = digest(options.script)
    library_args = [
        argument for library in options.node_library for argument in (str(library), digest(library))
    ]
    corpus = [
        (
            "go" if path.endswith(".go") else "syntax",
            path,
            (ROOT / path).read_text(encoding="utf-8"),
            path in NEGATIVE_PATHS,
            None,
        )
        for path in (*CORPUS_PATHS, *NEGATIVE_PATHS)
    ]
    if options.corpus_manifest:
        corpus.extend(
            load_corpus_manifest(
                options.corpus_manifest,
                options.corpus_manifest_sha256,
                options.corpus_root,
            )
        )
    paths = [path for _, path, _, _, _ in corpus]
    if len(paths) != len(set(paths)):
        raise ValueError("corpus paths must be unique across built-in and supplied inputs")
    go_cases = []
    syntax_cases = []
    fixtures = [(producer, path, source, False, None) for producer, path, source in FIXTURES]
    for producer, path, source, parse_failure, expected_records in (*fixtures, *corpus):
        baseline = records(
            options.vendor_bin,
            [path],
            source,
            path,
            parse_failure=parse_failure,
            expected_records=expected_records,
        )
        if producer == "go":
            go_cases.append((path, source, baseline, parse_failure, expected_records))
        else:
            syntax_cases.append((path, source, baseline, parse_failure, expected_records))

    if options.official_go_batch_bin:
        go_rows = candidate_batch_rows(
            options.official_go_batch_bin,
            [str(options.go_helper), go_digest, options.go_version],
            go_cases,
        )
    else:
        # Retain the existing one-file CLI for callers that have not built the
        # corpus batch binary yet.
        go_rows = []
        for path, source, _baseline, parse_failure, _expected_records in go_cases:
            result = records(
                options.official_bin,
                ["go", path, str(options.go_helper), go_digest, options.go_version],
                source,
                path,
                parse_failure=parse_failure,
            )
            go_rows.append(
                {
                    "path": path,
                    "status": "parse_failure" if parse_failure else "ok",
                    "records": [] if result is None else result,
                }
            )
    for (path, _source, baseline, parse_failure, expected_records), row in zip(go_cases, go_rows):
        if not isinstance(row, dict) or row.get("path") != path:
            raise AssertionError(f"official Go batch path mismatch: {path}")
        if parse_failure:
            if row.get("status") != "parse_failure" or row.get("records") != []:
                raise AssertionError(f"{path}: expected explicit parse_failure row")
            print(f"{path}: both producers rejected parse failure", flush=True)
        elif row.get("status") != "ok" or row.get("records") != baseline:
            raise AssertionError(f"SymbolRecord mismatch for {path}")
        elif expected_records is not None and len(row["records"]) != expected_records:
            raise AssertionError(f"{path}: official Go record denominator differs")
        else:
            print(f"{path}: {len(baseline)} records equal", flush=True)

    batch_args = [
        str(options.official_batch_bin),
        str(options.node),
        node_digest,
        options.node_version,
        str(options.typescript_js),
        compiler_digest,
        script_digest,
        options.typescript_version,
        *library_args,
    ]
    rows = candidate_batch_rows(options.official_batch_bin, batch_args[1:], syntax_cases)
    for (path, _source, baseline, parse_failure, expected_records), row in zip(syntax_cases, rows):
        if not isinstance(row, dict) or row.get("path") != path:
            raise AssertionError(f"official syntax batch path mismatch: {path}")
        if parse_failure:
            if row.get("status") != "parse_failure" or row.get("records") != []:
                raise AssertionError(f"{path}: expected explicit parse_failure row")
            print(f"{path}: both producers rejected parse failure", flush=True)
        elif row.get("status") != "ok" or row.get("records") != baseline:
            raise AssertionError(f"SymbolRecord mismatch for {path}")
        elif expected_records is not None and len(row["records"]) != expected_records:
            raise AssertionError(f"{path}: official syntax record denominator differs")
        else:
            print(f"{path}: {len(baseline)} records equal", flush=True)


if __name__ == "__main__":
    main()
