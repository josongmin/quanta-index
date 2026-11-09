"""Verify source-only, exhaustive Go trace review before SDK score admission.

Reviewers see the frozen query, failure excerpt, and source files. The frozen
plan commits their completed forms before SDK capture. This validator checks
structure and source binding; reviewer identity remains self-reported.
"""

from __future__ import annotations

import hashlib
from pathlib import Path, PurePosixPath

from tools.benchmark.evidence import _read_control_file, parse_json


class ReviewError(ValueError):
    """A Go trace review receipt cannot support complete-corpus scoring."""


def _require(condition: bool, reason: str) -> None:
    if not condition:
        raise ReviewError(reason)


def _sha(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def _exact(value: object, keys: set[str], where: str) -> dict:
    _require(isinstance(value, dict) and set(value) == keys, f"{where} fields differ")
    return value


def _safe_path(value: object) -> str:
    _require(isinstance(value, str) and bool(value), "Go trace review source path is empty")
    path = PurePosixPath(value)
    _require(
        not path.is_absolute()
        and path.as_posix() == value
        and all(part not in {"", ".", ".."} for part in path.parts)
        and "\\" not in value
        and value.endswith(".go"),
        f"Go trace review source path is unsafe: {value}",
    )
    return value


def _source_bytes(case_dir: Path, manifest: dict) -> dict[str, str]:
    files = manifest.get("files")
    _require(isinstance(files, list) and bool(files), "Go trace review manifest is empty")
    expected: dict[str, str] = {}
    for row in files:
        row = _exact(row, {"path", "file_sha256"}, "Go trace review manifest row")
        path = _safe_path(row["path"])
        digest = row["file_sha256"]
        _require(
            isinstance(digest, str) and len(digest) == 64
            and all(ch in "0123456789abcdef" for ch in digest),
            "Go trace review manifest digest is invalid",
        )
        _require(path not in expected, "Go trace review manifest repeats a path")
        expected[path] = digest
    _require(list(expected) == sorted(expected), "Go trace review manifest is not sorted")
    corpus = case_dir / "corpus"
    _require(corpus.is_dir() and not corpus.is_symlink(), "Go trace review corpus is absent")
    observed: set[str] = set()
    total_bytes = 0
    for entry in corpus.rglob("*"):
        _require(not entry.is_symlink(), "Go trace review corpus contains a symlink")
        if entry.is_dir():
            continue
        _require(entry.is_file(), "Go trace review corpus contains a non-file")
        path = _safe_path(entry.relative_to(corpus).as_posix())
        _require(path in expected and path not in observed, "Go trace review corpus path drift")
        raw = _read_control_file(entry, max_bytes=1024 * 1024)
        total_bytes += len(raw)
        _require(total_bytes <= 512 * 1024 * 1024, "Go trace review corpus exceeds source limit")
        _require(_sha(raw) == expected[path], "Go trace review source byte drift")
        observed.add(path)
    _require(observed == set(expected), "Go trace review corpus does not cover manifest")
    return expected


def validate(
    case_dir: Path,
    case: dict,
    judgment: dict,
    denominator: dict,
    manifest: dict,
    pack: dict,
) -> dict[Path, str]:
    """Return immutable input digests after complete, source-bound adjudication."""
    receipts = _exact(
        case.get("review_receipts"),
        {"review_1_sha256", "review_2_sha256", "adjudication_sha256"},
        "Go trace review receipt commitment",
    )
    _require(
        judgment.get("review_receipts") == receipts,
        "Go trace judgment/review commitment drift",
    )
    review_location = judgment.get("review_directory")
    _require(
        isinstance(review_location, str) and Path(review_location).is_absolute(),
        "Go trace review directory is not absolute",
    )
    review_dir = Path(review_location)
    case_root = case_dir.resolve()
    review_root = review_dir.resolve()
    _require(
        review_dir.is_dir() and not review_dir.is_symlink()
        and not review_root.is_relative_to(case_root)
        and not case_root.is_relative_to(review_root),
        "Go trace review directory overlaps SDK case",
    )
    source = _source_bytes(case_dir, manifest)
    production = {path for path in source if not path.endswith("_test.go")}
    _require(bool(production), "Go trace review has no production files")
    tasks = pack.get("tasks")
    _require(isinstance(tasks, list) and len(tasks) == 1, "Go trace review pack task count differs")
    task = tasks[0]
    _require(
        isinstance(task, dict)
        and task.get("task_id") == judgment.get("task_id")
        and isinstance(task.get("query"), str)
        and isinstance(task.get("semantic_input"), dict),
        "Go trace review task identity differs",
    )
    _require(
        isinstance(denominator.get("criterion"), str)
        and denominator["criterion"].strip()
        and isinstance(denominator.get("scope"), str)
        and denominator["scope"].strip(),
        "Go trace review denominator criterion/scope missing",
    )
    context = {
        "schema_version": 1,
        "criterion": denominator["criterion"],
        "scope": denominator["scope"],
        "kind": "go_trace_source_only_review_v1",
        "task_id": task["task_id"],
        "query": task["query"],
        "semantic_input": task["semantic_input"],
        "source_commit": case.get("commit"),
        "source_manifest_sha256": case.get("manifest_sha256"),
        "pack_sha256": case.get("pack_sha256"),
    }
    tracked: dict[Path, str] = {
        case_dir / "corpus" / path: digest for path, digest in source.items()
    }
    forms = []
    for filename, digest_key in (
        ("review-1.json", "review_1_sha256"),
        ("review-2.json", "review_2_sha256"),
        ("adjudication.json", "adjudication_sha256"),
    ):
        path = review_dir / filename
        raw = _read_control_file(path)
        _require(_sha(raw) == receipts[digest_key], f"Go trace {filename} commitment drift")
        form = parse_json(raw.decode("utf-8"))
        required = set(context) | {"reviewer_id", "files"}
        if filename == "adjudication.json":
            required |= {"review_sha256", "resolution"}
        form = _exact(form, required, f"Go trace {filename}")
        _require(
            all(form[key] == value for key, value in context.items()),
            f"Go trace {filename} query/source context drift",
        )
        reviewer_id = form["reviewer_id"]
        _require(
            isinstance(reviewer_id, str) and reviewer_id.strip(),
            f"Go trace {filename} reviewer identity missing",
        )
        rows = form["files"]
        _require(isinstance(rows, list), f"Go trace {filename} files malformed")
        graded = {}
        for row in rows:
            row = _exact(
                row,
                {"path", "file_sha256", "direct_production", "rationale"},
                f"Go trace {filename} file row",
            )
            name = _safe_path(row["path"])
            _require(
                name in production and name not in graded
                and row["file_sha256"] == source[name]
                and type(row["direct_production"]) is bool
                and isinstance(row["rationale"], str)
                and (not row["direct_production"] or bool(row["rationale"].strip())),
                f"Go trace {filename} file judgment invalid: {name}",
            )
            graded[name] = row
        _require(set(graded) == production, f"Go trace {filename} production coverage incomplete")
        forms.append((form, graded))
        tracked[path] = _sha(raw)
    ids = [form["reviewer_id"] for form, _ in forms]
    _require(len(set(ids)) == 3, "Go trace reviewers and adjudicator are not distinct")
    adjudication = forms[2][0]
    _require(
        adjudication["review_sha256"] == [
            tracked[review_dir / "review-1.json"], tracked[review_dir / "review-2.json"]
        ]
        and isinstance(adjudication["resolution"], str)
        and adjudication["resolution"].strip(),
        "Go trace adjudication does not bind both reviews",
    )
    adjudicated = forms[2][1]
    for path in production:
        first = forms[0][1][path]["direct_production"]
        second = forms[1][1][path]["direct_production"]
        _require(
            first != second or adjudicated[path]["direct_production"] == first,
            f"Go trace adjudication changed an undisputed judgment: {path}",
        )
    gold = judgment.get("direct_production_gold")
    _require(isinstance(gold, list) and bool(gold), "Go trace adjudication has no direct gold")
    _require(
        all(
            isinstance(row, dict)
            and set(row) == {"path", "evidence"}
            and isinstance(row["evidence"], str)
            and row["evidence"].strip()
            for row in gold
        ),
        "Go trace direct gold evidence is malformed",
    )
    gold_by_path = {row["path"]: row["evidence"] for row in gold}
    _require(len(gold_by_path) == len(gold), "Go trace direct gold repeats a path")
    positive = {
        path: row["rationale"]
        for path, row in adjudicated.items()
        if row["direct_production"]
    }
    _require(gold_by_path == positive, "Go trace direct gold differs from adjudication")
    _require(
        judgment.get("recall_denominator_exhaustive") is True,
        "Go trace judgment does not claim complete denominator",
    )
    return tracked


def validate_module_frame(
    case_dir: Path,
    case: dict,
    plan: dict,
    pack: dict,
    manifest: dict,
    context: dict,
    trace: dict,
) -> dict[Path, str]:
    """Check Rust-verified frame fields against frozen input, bytes and SDK."""
    inputs_path = plan.get("source_owner_inputs_path")
    _require(
        isinstance(inputs_path, str)
        and Path(inputs_path).is_absolute()
        and not Path(inputs_path).is_symlink(),
        "Go trace module source-owner input path is missing",
    )
    source_input_path = Path(inputs_path)
    raw = _read_control_file(source_input_path)
    digest = _sha(raw)
    _require(digest == plan.get("input_sha256"), "Go trace module source-owner input drift")
    inputs = parse_json(raw.decode("utf-8"))
    task = pack["tasks"][0]
    matches = [
        item for item in inputs
        if isinstance(item, dict) and item.get("task_id") == task["task_id"]
    ] if isinstance(inputs, list) else []
    _require(len(matches) == 1, "Go trace module source-owner input is not unique")
    source_input = matches[0]
    provenance = source_input.get("provenance")
    excerpt = task["semantic_input"]["failure_excerpt"]
    _require(
        source_input.get("repo_id") == case.get("repo_id")
        and source_input.get("commit") == case.get("commit")
        and source_input.get("failure_excerpt") == excerpt
        and isinstance(provenance, dict)
        and provenance.get("kind") == "original_upstream_issue_failure_transcript"
        and provenance.get("frame_stratum") == "in_repo_module_cache_panic_frame"
        and provenance.get("failure_excerpt_sha256") == _sha(excerpt.encode())
        and provenance.get("module_tag_commit") == case.get("commit"),
        "Go trace module source-owner provenance differs",
    )
    module_path = provenance.get("module_path")
    module_version = provenance.get("module_version")
    _require(
        isinstance(module_path, str) and isinstance(module_version, str),
        "Go trace module identity missing",
    )
    files = {row["path"]: row["file_sha256"] for row in manifest["files"]}
    frame_ref = _exact(
        case.get("module_frame"),
        {"path", "line", "symbol", "file_sha256"},
        "Go trace frozen module frame",
    )
    frame_path = _safe_path(frame_ref["path"])
    frame_line = frame_ref["line"]
    _require(
        frame_path in files
        and not frame_path.endswith("_test.go")
        and frame_ref["file_sha256"] == files[frame_path]
        and type(frame_line) is int
        and frame_line >= 1
        and isinstance(frame_ref["symbol"], str)
        and frame_ref["symbol"].strip()
        and case.get("module_path") == module_path
        and case.get("module_version") == module_version,
        "Go trace frozen module frame reference differs",
    )
    source_file = case_dir / "corpus" / frame_path
    source_raw = _read_control_file(source_file)
    _require(
        _sha(source_raw) == files[frame_path]
        and frame_line <= len(source_raw.splitlines()),
        "Go trace module frame source coordinate differs",
    )
    _require(
        context.get("mode") == "frozen_module_panic_v1"
        and context.get("failure_source_repo_id") == case.get("repo_id")
        and context.get("module_path") == module_path
        and context.get("module_version") == module_version
        and context.get("module_frame") == frame_ref
        and trace.get("frame_class") == "exact",
        "Go trace SDK module frame receipt differs",
    )
    candidates = trace.get("candidates")
    _require(isinstance(candidates, list), "Go trace module candidates missing")
    for candidate in candidates:
        _require(
            isinstance(candidate, dict)
            and type(candidate.get("source_frame")) is bool
            and candidate["source_frame"] == (candidate.get("path") == frame_path),
            "Go trace promoted module frame path differs",
        )
    return {source_input_path: digest}
