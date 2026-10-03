"""Validate the current producer census and its source-bound commitments."""

from __future__ import annotations

import hashlib
import os
import re
import stat
import sys
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # Python 3.10 uses the declared tomli dependency.
    import tomli as tomllib

ROOT = Path(__file__).resolve().parents[3]
try:
    from tools.benchmark.evidence import _consume_regular_file, parse_json
except ModuleNotFoundError:  # direct retrieval script invocation
    sys.path.insert(0, str(ROOT))
    from tools.benchmark.evidence import _consume_regular_file, parse_json

PRODUCER = "source-bound-symbols-v2"
LANGUAGES = {
    "rs": "rust",
    "go": "go",
    "py": "python",
    "js": "javascript",
    "mjs": "javascript",
    "cjs": "javascript",
    "jsx": "javascript",
    "ts": "typescript",
    "mts": "typescript",
    "cts": "typescript",
    "tsx": "typescript_tsx",
}
LIMITS = (
    "max_file_bytes",
    "max_symbols_per_file",
    "max_symbols_total",
    "max_diagnostics_per_file",
    "max_diagnostics_total",
)
TIMEOUTS = ("timeout_per_file_ns", "timeout_total_ns")
ROW_KEYS = {"path", "source_sha256", "language", "coverage", "failure"}
REPORT_ROW_KEYS = ROW_KEYS | {
    "failure_detail",
    "failure_detail_truncated",
    "diagnostics",
    "diagnostics_total",
    "diagnostics_truncated",
    "diagnostics_complete",
}


def exact(value: object, keys: set[str], where: str) -> dict:
    if not isinstance(value, dict) or set(value) != keys:
        raise ValueError(f"{where} must hold exactly {sorted(keys)}")
    return value


def is_sha(value: object) -> bool:
    return isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value) is not None


def count(value: object) -> bool:
    return type(value) is int and 0 <= value < 2**64


def regular_bytes(path: Path, *, max_bytes: int | None = None) -> bytes:
    def consume(handle):
        if max_bytes is None:
            return handle.read()
        if os.fstat(handle.fileno()).st_size > max_bytes:
            raise ValueError("preflight artifact exceeds evidence budget")
        data = handle.read(max_bytes + 1)
        if len(data) > max_bytes:
            raise ValueError("preflight artifact exceeds evidence budget")
        return data

    # Keep descriptor identity and ancestor custody through the entire read.
    # A pathname stat followed by read_bytes can consume a replacement or grow
    # past the checked size. The shared reader also refuses symlink ancestors.
    return _consume_regular_file(path, consume)


def grammar_identity(root: Path = ROOT) -> str:
    versions = {
        "tree-sitter": "0.25.10",
        "tree-sitter-rust": "0.24.2",
        "tree-sitter-go": "0.25.0",
        "tree-sitter-javascript": "0.25.0",
        "tree-sitter-python": "0.25.0",
        "tree-sitter-typescript": "0.23.2",
    }
    lock = tomllib.loads(regular_bytes(root / "Cargo.lock").decode())
    for name, version in versions.items():
        rows = [p for p in lock["package"] if p["name"] == name]
        if len(rows) != 1 or rows[0]["version"] != version:
            raise ValueError(f"grammar lock identity differs: {name}")
        if name in {"tree-sitter-go", "tree-sitter-typescript"} and (
            "source" in rows[0] or "checksum" in rows[0]
        ):
            raise ValueError(f"{name} grammar must resolve to the source-bound path dependency")
    manifest = tomllib.loads(regular_bytes(root / "Cargo.toml").decode())
    for tag, version in [("typescript", "0.23.2"), ("go", "0.25.0")]:
        if manifest["workspace"]["dependencies"]["tree-sitter-" + tag] != {
            "version": "=" + version,
            "path": "vendor/tree-sitter-" + tag,
        }:
            raise ValueError(f"{tag} grammar path dependency differs")
    return (
        "tree-sitter@0.25.10;rust@0.24.2;"
        "go@0.25.0+quanta-go-compatibility-1;vendored-source-sha256="
        + _vendor_digest(root / "vendor/tree-sitter-go", "go")
        + ";javascript@0.25.0;python@0.25.0;"
        "typescript@0.23.2+quanta-typescript-compatibility-2;vendored-source-sha256="
        + _vendor_digest(root / "vendor/tree-sitter-typescript", "typescript")
    )


def _vendor_digest(vendor: Path, tag: str) -> str:
    files = []
    for path in vendor.rglob("*"):
        mode = path.lstat().st_mode
        if stat.S_ISDIR(mode):
            continue
        if not stat.S_ISREG(mode):
            raise ValueError(f"non-regular vendored grammar input: {path}")
        files.append((path.relative_to(vendor).as_posix(), path))
    if not files or vendor.is_symlink():
        raise ValueError("vendored grammar source is empty or symlinked")
    digest = hashlib.sha256(f"quanta-index:vendored-{tag}-inputs:v1\0".encode())
    for name, path in sorted(files):
        name_bytes, data = name.encode(), regular_bytes(path)
        digest.update(len(name_bytes).to_bytes(8, "little"))
        digest.update(name_bytes)
        digest.update(len(data).to_bytes(8, "little"))
        digest.update(data)
    return digest.hexdigest()


def policy_digest(policy: dict, root: Path = ROOT) -> str:
    exact(policy, set(LIMITS + TIMEOUTS), "symbol extraction policy")
    if any(not count(policy[key]) for key in LIMITS):
        raise ValueError("invalid symbol extraction limit")
    for key in TIMEOUTS:
        value = policy[key]
        if (
            not isinstance(value, str)
            or re.fullmatch(r"0|[1-9][0-9]{0,38}", value) is None
            or int(value) >= 2**128
        ):
            raise ValueError("invalid symbol extraction deadline")
    digest = hashlib.sha256()
    parts = [
        b"quanta-index:symbol-preflight:v1",
        PRODUCER.encode(),
        grammar_identity(root).encode(),
    ]
    parts += [
        regular_bytes(root / path)
        for path in (
            "Cargo.lock",
            "benchmarks/retrieval/src/symbols.rs",
            "benchmarks/retrieval/src/symbols/preflight.rs",
            "benchmarks/retrieval/src/symbols/definition_query.rs",
            "benchmarks/retrieval/build.rs",
        )
    ]
    for data in parts:
        digest.update(str(len(data)).encode() + b"\0" + data)
    for key in LIMITS:
        digest.update(str(policy[key]).encode() + b"\0")
    for key in TIMEOUTS:
        digest.update(int(policy[key]).to_bytes(16, "little"))
    return digest.hexdigest()


def validate_row(row: dict) -> tuple[str, int | None]:
    path = row["path"]
    if (
        not isinstance(path, str)
        or not path
        or path.startswith("/")
        or "\\" in path
        or len(path.encode("utf-8")) > 4096
        or re.match(r"[A-Za-z]:", path)
        or any(ord(c) < 32 or 0x7F <= ord(c) <= 0x9F for c in path)
        or any(p in ("", ".", "..") for p in path.split("/"))
    ):
        raise ValueError("symbol coverage path is invalid")
    if not is_sha(row["source_sha256"]):
        raise ValueError("symbol coverage source hash is invalid")
    language = LANGUAGES.get(path.rsplit(".", 1)[-1]) if "." in path else None
    if row["language"] != language:
        raise ValueError("symbol coverage grammar mismatch")
    coverage = row["coverage"]
    if not isinstance(coverage, dict):
        raise ValueError("symbol coverage state must be explicit")
    state = coverage.get("state")
    if state == "complete":
        exact(coverage, {"state", "symbol_count"}, "complete coverage")
        if language is None or not count(coverage["symbol_count"]) or row["failure"] is not None:
            raise ValueError("invalid complete symbol coverage")
        return state, coverage["symbol_count"]
    exact(coverage, {"state"}, "incomplete coverage")
    expected_failure = {"unsupported": "unsupported_language", "parse_failed": "syntax_error"}
    if (
        not isinstance(state, str)
        or state not in expected_failure
        or row["failure"] != expected_failure[state]
    ):
        raise ValueError("fatal or unknown symbol coverage cannot be accepted")
    if (state == "unsupported") != (language is None):
        raise ValueError("incomplete symbol coverage grammar mismatch")
    return state, None


def validate_metrics(metrics: dict, expected_grammar: str) -> None:
    if (
        metrics["symbol_producer_identity"] != PRODUCER
        or metrics["symbol_grammars"] != expected_grammar
        or not count(metrics["symbol_count"])
        or not count(metrics["file_count"])
    ):
        raise ValueError("invalid symbol producer evidence")
    policy = metrics["symbol_coverage_policy"]
    if policy not in ("require-complete", "allow-incomplete"):
        raise ValueError("unknown symbol coverage policy")
    for key in ("symbol_preflight_sha256", "symbol_producer_policy_sha256"):
        if not is_sha(metrics[key]):
            raise ValueError(f"invalid {key}")
    ref = metrics["symbol_preflight_out"]
    if (
        not isinstance(ref, str)
        or not ref
        or ref in (".", "..")
        or "/" in ref
        or "\\" in ref
        or "\0" in ref
    ):
        raise ValueError("preflight reference must name a sibling regular artifact")
    coverage = metrics["symbol_coverage"]
    if not isinstance(coverage, list) or len(coverage) != metrics["file_count"]:
        raise ValueError("symbol coverage does not enumerate every file")
    paths, total, incomplete, unsupported = [], 0, 0, []
    for row in coverage:
        exact(row, ROW_KEYS | {"definition_count"}, "symbol coverage row")
        state, definitions = validate_row(row)
        if row["definition_count"] != definitions or (
            definitions is not None and not count(row["definition_count"])
        ):
            raise ValueError("symbol coverage count is inconsistent or incomplete")
        paths.append(row["path"])
        total += definitions or 0
        incomplete += state != "complete"
        if state == "unsupported":
            unsupported.append({k: row[k] for k in ROW_KEYS})
    if paths != sorted(set(paths)) or total != metrics["symbol_count"]:
        raise ValueError("symbol coverage is duplicate, reordered, or incomplete")
    for key in (
        "symbol_incomplete_files",
        "symbol_unsupported_files",
        "symbol_only_scopes",
        "empty_scopes",
    ):
        if not count(metrics[key]) or metrics[key] > metrics["file_count"]:
            raise ValueError(f"{key} is outside the admitted file count")
    if metrics["symbol_only_scopes"] + metrics["empty_scopes"] > metrics["file_count"]:
        raise ValueError("zero-chunk scope counts overlap")
    details = metrics["symbol_unsupported_details"]
    if not isinstance(details, list) or len(details) != metrics["symbol_unsupported_files"]:
        raise ValueError("unsupported symbol details differ from count")
    for row in details:
        exact(row, REPORT_ROW_KEYS, "unsupported symbol report")
        validate_report_row(row)
    if [{k: row[k] for k in ROW_KEYS} for row in details] != unsupported:
        raise ValueError("unsupported symbol details differ from coverage")
    if metrics["symbol_incomplete_files"] != incomplete or (
        policy == "require-complete" and incomplete
    ):
        raise ValueError("incomplete symbol coverage contradicts the admission policy")


def validate_report_row(row: dict) -> None:
    exact(row, REPORT_ROW_KEYS, "symbol file report")
    state, _ = validate_row(row)
    if (
        not count(row["diagnostics_total"])
        or type(row["diagnostics_truncated"]) is not bool
        or row["diagnostics_complete"] is not True
        or type(row["failure_detail_truncated"]) is not bool
    ):
        raise ValueError("invalid or incomplete diagnostic census")
    detail = row["failure_detail"]
    if detail is not None and (not isinstance(detail, str) or len(detail) > 512):
        raise ValueError("invalid bounded failure detail")
    if row["failure_detail_truncated"] and (not isinstance(detail, str) or len(detail) != 512):
        raise ValueError("invalid failure detail truncation")
    diagnostics = row["diagnostics"]
    if (
        not isinstance(diagnostics, list)
        or len(diagnostics) > row["diagnostics_total"]
        or row["diagnostics_truncated"] != (len(diagnostics) < row["diagnostics_total"])
    ):
        raise ValueError("invalid diagnostic truncation/count")
    keys = []
    for diagnostic in diagnostics:
        exact(diagnostic, {"kind", "byte_start", "byte_end"}, "symbol diagnostic")
        start, end, kind = diagnostic["byte_start"], diagnostic["byte_end"], diagnostic["kind"]
        if state == "parse_failed":
            if (
                not count(start)
                or not count(end)
                or start > end
                or kind not in ("syntax_error", "missing_syntax")
            ):
                raise ValueError("invalid source diagnostic range")
            keys.append((start, end, kind))
        elif (
            state != "unsupported"
            or start is not None
            or end is not None
            or kind != "unsupported_language"
        ):
            raise ValueError("diagnostic differs from coverage state")
    if keys != sorted(keys):
        raise ValueError("diagnostics are reordered")
    if state == "complete" and (
        row["diagnostics_total"] != 0 or detail is not None or row["failure_detail_truncated"]
    ):
        raise ValueError("complete coverage carries failure diagnostics")
    if state != "complete" and row["diagnostics_total"] == 0:
        raise ValueError("failed coverage omits diagnostic census")
    if state == "unsupported" and row["diagnostics_total"] != 1:
        raise ValueError("unsupported coverage must have exactly one diagnostic")


def verify_requested_timeout(policy: dict, expected_total_ms: int) -> None:
    if type(expected_total_ms) is not int or not 0 < expected_total_ms < 2**64:
        raise ValueError("requested symbol timeout must be a positive u64")
    if policy.get("timeout_total_ns") != str(expected_total_ms * 1_000_000):
        raise ValueError("symbol total timeout differs from requested budget")


def verify_artifact(
    metrics: dict,
    phase_path: Path,
    corpus: dict,
    root: Path = ROOT,
    *,
    expected_timeout_total_ms: int | None = None,
) -> Path:
    validate_metrics(metrics, grammar_identity(root))
    artifact = phase_path.parent / metrics["symbol_preflight_out"]
    raw = regular_bytes(artifact, max_bytes=64 * 1024 * 1024)
    if hashlib.sha256(raw).hexdigest() != metrics["symbol_preflight_sha256"]:
        raise ValueError("preflight artifact digest mismatch")
    value = parse_json(raw)
    exact(
        value,
        {"symbol_coverage_policy", "repository_commit", "file_universe_sha256", "preflight"},
        "preflight envelope",
    )
    expected = sorted((row["path"], row["file_sha256"]) for row in corpus["files"])
    if len({path for path, _ in expected}) != len(expected):
        raise ValueError("duplicate admitted source")
    universe = hashlib.sha256(
        b"".join(path.encode() + b"\0" + sha.encode() + b"\0" for path, sha in expected)
    ).hexdigest()
    if (
        value["repository_commit"] != corpus["repository_commit"]
        or value["file_universe_sha256"] != universe
        or value["symbol_coverage_policy"] != metrics["symbol_coverage_policy"]
    ):
        raise ValueError("preflight corpus/profile binding mismatch")
    report = exact(
        value["preflight"],
        {
            "schema",
            "producer_identity",
            "grammar_identity",
            "lockfile_sha256",
            "producer_policy_sha256",
            "policy",
            "files",
            "admitted_files",
            "incomplete_files",
        },
        "preflight report",
    )
    if (
        report["schema"] != "symbol-preflight-v1"
        or report["producer_identity"] != PRODUCER
        or report["grammar_identity"] != metrics["symbol_grammars"]
        or report["lockfile_sha256"]
        != hashlib.sha256(regular_bytes(root / "Cargo.lock")).hexdigest()
        or report["producer_policy_sha256"] != metrics["symbol_producer_policy_sha256"]
        or report["producer_policy_sha256"] != policy_digest(report["policy"], root)
    ):
        raise ValueError("preflight source/grammar/policy commitment mismatch")
    if expected_timeout_total_ms is not None:
        verify_requested_timeout(report["policy"], expected_timeout_total_ms)
    files = report["files"]
    if (
        not isinstance(files, list)
        or not count(report["admitted_files"])
        or not count(report["incomplete_files"])
    ):
        raise ValueError("invalid preflight census counts")
    projected, unsupported, retained = [], [], 0
    for row in files:
        validate_report_row(row)
        state, definitions = validate_row(row)
        projected.append({**{k: row[k] for k in ROW_KEYS}, "definition_count": definitions})
        if state == "unsupported":
            unsupported.append(row)
        expected_retained = min(
            row["diagnostics_total"],
            report["policy"]["max_diagnostics_per_file"],
            report["policy"]["max_diagnostics_total"] - retained,
        )
        if len(row["diagnostics"]) != expected_retained:
            raise ValueError("diagnostic retention differs from the declared file/global budgets")
        retained += len(row["diagnostics"])
        if any(
            d["byte_end"] is not None and d["byte_end"] > report["policy"]["max_file_bytes"]
            for d in row["diagnostics"]
        ):
            raise ValueError("diagnostic exceeds admitted source budget")
        if definitions is not None and definitions > report["policy"]["max_symbols_per_file"]:
            raise ValueError("per-file symbol budget exceeded")
    if (
        retained > report["policy"]["max_diagnostics_total"]
        or metrics["symbol_count"] > report["policy"]["max_symbols_total"]
        or projected != metrics["symbol_coverage"]
        or unsupported != metrics["symbol_unsupported_details"]
        or report["admitted_files"] != metrics["file_count"]
        or report["incomplete_files"] != metrics["symbol_incomplete_files"]
        or [(r["path"], r["source_sha256"]) for r in files] != expected
    ):
        raise ValueError("preflight census differs from metrics or admitted corpus")
    return artifact
