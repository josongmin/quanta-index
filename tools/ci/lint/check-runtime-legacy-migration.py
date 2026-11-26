#!/usr/bin/env python3
"""Keep retired Runtime bodies until body-bearing current oracles are verified."""

from __future__ import annotations

import argparse
import hashlib
import os
import re
import subprocess
import sys
from collections.abc import Callable
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python 3.10 compatibility
    import tomli as tomllib  # type: ignore[no-redef]


ROOT = Path(__file__).resolve().parents[3]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
from tools.ci.retirement_nextest import (  # noqa: E402
    check_migration_observations,
    retained_executable_digests,
)
from tools.ci.rust_test_mount import committed_path_resolver, retirement_event_matcher  # noqa: E402

MANIFEST = ROOT / "tools/ci/inventory/runtime-legacy-migration.toml"
CURRENT_PERF_PATH = Path("crates/quanta-index-searchd-runtime/tests/legacy_perf_chaos_routes.rs")
CURRENT_PERF = ROOT / CURRENT_PERF_PATH
FROZEN_COMMIT = "df090e56a72f45bcd676686c9c1d8437f54d83db"
FROZEN_PERF_ID_SHA256 = "d4acfc79a61a53eaeb8e132ec95462ba93da274a3ac84ff1ac3c004c25678a7f"
FROZEN_ORACLE_ID_SHA256 = "d7e4258ea165be104d1c6d78cfb4c03184a346ec167a48249e25f2a26759f500"
FROZEN_SOURCE_SHA256 = {
    "crates/quanta-index-searchd-runtime/tests/e2e_perf_chaos.rs": (
        "df665bd12b3d92b17028f852402035ae6bc6f36e2aa6ea763fd7212e96a70c1e"
    ),
    "crates/quanta-index-searchd-runtime/tests/e2e_snapshot_registry.rs": (
        "60d04b76b506ee03b67a27fade66a1c6bef3e11fa9f1c7221485c590afe0917b"
    ),
    "crates/quanta-index-searchd-runtime/tests/e2e_process_envelope.rs": (
        "20dfc9d0325e4f1d4c014bf91050461b38d443acf19964583929182ff00f4125"
    ),
    "crates/quanta-index-searchd-runtime/tests/e2e_semantic_budget_interruption.rs": (
        "74fc2be60d66226cebc6b12c92c1b342c4e177665f1c6465c3f91cb22f9452b8"
    ),
    "crates/quanta-index-searchd-runtime/tests/e2e_socket_access.rs": (
        "0953685f45f0a55d88dada5c50a84406f9c9b839beef31b703057fd5d114a28f"
    ),
}
TEST_ATTRIBUTE = re.compile(r"^\s*#\[test\]\s*$")
TEST_FUNCTION = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)"
)
ALLOWED_PROOF = {"VERIFIED", "FAILED", "BLOCKED", "NOT_RUN"}
REQUIRED_PUBLIC_ROUTES = frozenset(
    {
        "hybrid_count_cap_surfaces_truthful_early_stop_reason",
        "hybrid_large_tied_result_set_keeps_order_stable",
        "oversized_raw_substring_query_fails_parse_and_does_not_poison_next_query",
        "raw_substring_trigram_plan_limit_is_typed_and_does_not_poison_next_query",
        "regex_false_positive_candidate_is_rejected_by_exact_verify",
        "regex_timeout_is_typed_and_does_not_poison_next_query",
        "regex_typed_rejection_does_not_poison_next_query",
        "structural_mixed_lexical_and_not_executes_and_does_not_poison_next_query",
        "structural_mixed_lexical_or_executes_and_does_not_poison_next_query",
        "structural_typed_hole_kind_rejects_typed_and_does_not_poison_next_query",
        "trigram_plan_limit_query_fails_typed_and_does_not_poison_next_query",
    }
)


def git_text(*arguments: str) -> str:
    return subprocess.check_output(
        ["git", "--no-replace-objects", *arguments],
        cwd=ROOT,
        encoding="utf-8",
    )


def source_tests(path: str, source: str) -> list[str]:
    lines = source.splitlines()
    tests: list[str] = []
    for index, line in enumerate(lines):
        if not TEST_ATTRIBUTE.match(line):
            continue
        attribute_depth = 0
        for offset, candidate in enumerate(lines[index + 1 : index + 40], start=index + 2):
            stripped = candidate.strip()
            if attribute_depth:
                attribute_depth += candidate.count("[") - candidate.count("]")
                if attribute_depth < 0:
                    raise ValueError(f"{path}: malformed test attribute at line {offset}")
                continue
            if not stripped or stripped.startswith("//"):
                continue
            if stripped.startswith("#["):
                attribute_depth = candidate.count("[") - candidate.count("]")
                if attribute_depth < 0:
                    raise ValueError(f"{path}: malformed test attribute at line {offset}")
                continue
            match = TEST_FUNCTION.match(candidate)
            if match:
                tests.append(match.group(1))
                break
            raise ValueError(
                f"{path}: #[test] at line {index + 1} is not attached to a function; "
                f"next item is line {offset}"
            )
        else:
            raise ValueError(f"{path}: #[test] at line {index + 1} has no function")
    return tests


def id_digest(tests: set[str] | list[str]) -> str:
    return hashlib.sha256("\n".join(sorted(tests)).encode("utf-8")).hexdigest()


def source_digest(source: str) -> str:
    return hashlib.sha256(source.encode("utf-8")).hexdigest()


def current_source(path: str, revision: str | None) -> str:
    candidate = Path(path)
    if candidate.is_absolute() or ".." in candidate.parts or candidate.suffix != ".rs":
        raise ValueError(f"candidate path must be a repository-relative Rust source: {path!r}")
    if revision is None:
        selected = CURRENT_PERF if candidate == CURRENT_PERF_PATH else ROOT / candidate
        return selected.read_bytes().decode("utf-8")
    return git_text("show", f"{revision}:{candidate.as_posix()}")


def validate_candidates(
    candidates: object,
    *,
    revision: str | None,
    context: str,
    errors: list[str],
) -> set[tuple[str, str]]:
    selected: set[tuple[str, str]] = set()
    if not isinstance(candidates, list) or not candidates:
        errors.append(f"{context}: no body-bearing current candidates")
        return selected
    for candidate in candidates:
        if not isinstance(candidate, dict):
            errors.append(f"{context}: candidate is not a table")
            continue
        path = candidate.get("path")
        tests = candidate.get("tests")
        if not isinstance(path, str):
            errors.append(f"{context}: candidate path is not a string")
            continue
        if not isinstance(tests, list) or not tests:
            errors.append(f"{context}: {path} has no candidate tests")
            continue
        try:
            available = set(source_tests(path, current_source(path, revision)))
        except (OSError, ValueError, subprocess.CalledProcessError) as error:
            errors.append(f"{context}: cannot read current candidate {path}: {error}")
            continue
        for test in tests:
            if not isinstance(test, str) or not test.isidentifier() or not test.isascii():
                errors.append(f"{context}: invalid candidate test {test!r} in {path}")
            elif test not in available:
                errors.append(f"{context}: {path} has no body-bearing test {test}")
            else:
                selected.add((path, test))
    return selected


def runtime_event_matcher(revision: str | None) -> Callable[[Path, str, str], bool]:
    def read(relative: str) -> str:
        if relative.endswith(".rs"):
            return current_source(relative, revision)
        if revision is None:
            return (ROOT / relative).read_text(encoding="utf-8")
        return git_text("show", f"{revision}:{relative}")

    resolve_path = (
        None
        if revision is None
        else committed_path_resolver(
            lambda _root, *args: git_text(*args).encode("utf-8"),
            ROOT,
            revision,
        )
    )
    return retirement_event_matcher(ROOT, read, resolve_path=resolve_path)


def main(
    *,
    revision: str | None = None,
    require_execution: bool = False,
    evidence_dir: Path | None = None,
) -> int:
    errors: list[str] = []
    require_execution = require_execution or revision is not None
    revision_candidate = revision or "HEAD"
    event_matches = runtime_event_matcher(revision)
    executable_digest = retained_executable_digests()

    def validate_execution(
        entry: dict,
        candidates: set[tuple[str, str]],
        context: str,
    ) -> None:
        if entry.get("proof") != "VERIFIED":
            return
        try:
            observed = check_migration_observations(
                entry.get("observations"),
                lambda path: current_source(path.relative_to(ROOT).as_posix(), revision),
                root=ROOT,
                source_tests=lambda path, raw: source_tests(str(path), raw),
                inventory_only=not require_execution,
                evidence_dir=evidence_dir,
                release_revision=revision,
                event_matches=event_matches,
                executable_digest=executable_digest,
            )
            if observed != candidates:
                raise ValueError(
                    f"terminal observations differ from candidate cases: "
                    f"missing={sorted(candidates - observed)} extra={sorted(observed - candidates)}"
                )
        except (OSError, ValueError, subprocess.CalledProcessError) as error:
            errors.append(f"{context}: {error}")

    try:
        if revision is not None and re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", revision) is None:
            raise ValueError("release revision must be a full Git commit ID")
        manifest_source = (
            MANIFEST.read_text(encoding="utf-8")
            if revision is None
            else git_text("show", f"{revision}:{MANIFEST.relative_to(ROOT).as_posix()}")
        )
        document = tomllib.loads(manifest_source)
        ancestor = subprocess.run(
            ["git", "merge-base", "--is-ancestor", FROZEN_COMMIT, revision_candidate],
            cwd=ROOT,
            check=False,
        )
        if ancestor.returncode != 0:
            errors.append(f"{revision_candidate}: frozen Runtime source is not an ancestor")
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"runtime legacy migration input: {error}", file=sys.stderr)
        return 1

    if document.get("version") != 1 or document.get("source_commit") != FROZEN_COMMIT:
        errors.append("manifest must bind version 1 and the frozen Runtime source commit")

    targets = document.get("targets")
    if not isinstance(targets, list) or len(targets) != 1:
        errors.append("manifest must contain the one frozen perf target")
        targets = []
    mapped_perf: set[str] = set()
    for target in targets:
        path = target.get("path") if isinstance(target, dict) else None
        if path != "crates/quanta-index-searchd-runtime/tests/e2e_perf_chaos.rs":
            errors.append(f"unexpected frozen perf target: {path!r}")
            continue
        expected_sha = FROZEN_SOURCE_SHA256[path]
        if target.get("source_sha256") != expected_sha:
            errors.append(f"{path}: editable source hash differs from the frozen hash")
        try:
            frozen_source = git_text("show", f"{FROZEN_COMMIT}:{path}")
            frozen_tests = source_tests(path, frozen_source)
            if source_digest(frozen_source) != expected_sha:
                errors.append(f"{path}: frozen source bytes changed")
        except (ValueError, subprocess.CalledProcessError) as error:
            errors.append(f"{path}: cannot read frozen source: {error}")
            frozen_tests = []
        dispositions = target.get("dispositions")
        if not isinstance(dispositions, list) or not dispositions:
            errors.append(f"{path}: no dispositions")
            continue
        for group in dispositions:
            if not isinstance(group, dict):
                errors.append(f"{path}: disposition is not a table")
                continue
            if group.get("disposition") != "migrate":
                errors.append(f"{path}: Runtime source cannot be retired before exact proof")
            proof = group.get("proof")
            if proof not in ALLOWED_PROOF:
                errors.append(f"{path}: invalid proof state {proof!r}")
            if not isinstance(group.get("replacement"), str) or not group["replacement"].strip():
                errors.append(f"{path}: migration group has no replacement contract")
            tests = group.get("tests")
            if not isinstance(tests, list) or not tests:
                errors.append(f"{path}: migration group has no test IDs")
                continue
            for test in tests:
                if not isinstance(test, str) or not test.isidentifier() or not test.isascii():
                    errors.append(f"{path}: invalid test ID {test!r}")
                elif test in mapped_perf:
                    errors.append(f"{path}: duplicate test ID {test}")
                else:
                    mapped_perf.add(test)
        if set(frozen_tests) != mapped_perf or id_digest(mapped_perf) != FROZEN_PERF_ID_SHA256:
            errors.append(f"{path}: mapped IDs differ from the frozen 43-test census")

    try:
        public_source = (
            CURRENT_PERF.read_text(encoding="utf-8")
            if revision is None
            else current_source(CURRENT_PERF_PATH.as_posix(), revision)
        )
        current_public_tests = set(source_tests(str(CURRENT_PERF), public_source))
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        errors.append(f"{CURRENT_PERF}: cannot read current public routes: {error}")
        current_public_tests = set()
    for missing in sorted(REQUIRED_PUBLIC_ROUTES - current_public_tests):
        errors.append(f"{CURRENT_PERF}: required public-route ID missing: {missing}")

    current_proofs = document.get("current_proofs")
    if not isinstance(current_proofs, list) or not current_proofs:
        errors.append("manifest must map the frozen perf census to current body-bearing tests")
        current_proofs = []
    mapped_current: set[str] = set()
    for index, proof_group in enumerate(current_proofs, start=1):
        context = f"current proof group {index}"
        if not isinstance(proof_group, dict):
            errors.append(f"{context}: proof is not a table")
            continue
        proof = proof_group.get("proof")
        if proof not in ALLOWED_PROOF:
            errors.append(f"{context}: invalid proof state {proof!r}")
        elif require_execution and proof != "VERIFIED":
            errors.append(f"{context}: retirement requires VERIFIED execution, got {proof}")
        source_ids = proof_group.get("source_tests")
        if not isinstance(source_ids, list) or not source_ids:
            errors.append(f"{context}: no frozen source tests")
        else:
            for test in source_ids:
                if not isinstance(test, str) or not test.isidentifier() or not test.isascii():
                    errors.append(f"{context}: invalid frozen source test {test!r}")
                elif test not in mapped_perf:
                    errors.append(f"{context}: unknown frozen source test {test}")
                elif test in mapped_current:
                    errors.append(f"{context}: duplicate frozen source mapping {test}")
                else:
                    mapped_current.add(test)
        candidates = validate_candidates(
            proof_group.get("candidates"),
            revision=revision,
            context=context,
            errors=errors,
        )
        validate_execution(proof_group, candidates, context)
    if mapped_current != mapped_perf:
        errors.append(
            "current proof mapping differs from the frozen 43-test census: "
            f"missing={sorted(mapped_perf - mapped_current)} "
            f"extra={sorted(mapped_current - mapped_perf)}"
        )

    exact = document.get("exact_oracles")
    if not isinstance(exact, list) or len(exact) != 4:
        errors.append("manifest must contain all four exact internal oracles")
        exact = []
    oracle_ids: set[str] = set()
    oracle_paths: set[str] = set()
    for entry in exact:
        if not isinstance(entry, dict):
            errors.append("exact oracle is not a table")
            continue
        path = entry.get("path")
        test = entry.get("test")
        if path not in FROZEN_SOURCE_SHA256 or path.endswith("e2e_perf_chaos.rs"):
            errors.append(f"unexpected exact-oracle source: {path!r}")
            continue
        if path in oracle_paths:
            errors.append(f"duplicate exact-oracle source: {path}")
        oracle_paths.add(path)
        expected_sha = FROZEN_SOURCE_SHA256[path]
        if entry.get("source_sha256") != expected_sha:
            errors.append(f"{path}: editable source hash differs from the frozen hash")
        if not isinstance(test, str) or not test.isidentifier() or test in oracle_ids:
            errors.append(f"{path}: invalid or duplicate exact test {test!r}")
            continue
        oracle_ids.add(test)
        proof = entry.get("proof")
        if proof not in ALLOWED_PROOF:
            errors.append(f"{path}: invalid proof state {proof!r}")
        elif require_execution and proof != "VERIFIED":
            errors.append(f"{path}:{test}: retirement requires VERIFIED execution, got {proof}")
        gap = entry.get("gap")
        if proof != "VERIFIED" and (not isinstance(gap, str) or not gap.strip()):
            errors.append(f"{path}: pending migration has no exact-oracle gap")
        candidates = validate_candidates(
            entry.get("candidates"),
            revision=revision,
            context=f"{path}:{test}",
            errors=errors,
        )
        validate_execution(entry, candidates, f"{path}:{test}")
        try:
            frozen_source = git_text("show", f"{FROZEN_COMMIT}:{path}")
            if source_digest(frozen_source) != expected_sha:
                errors.append(f"{path}: frozen source bytes changed")
            if test not in source_tests(path, frozen_source):
                errors.append(f"{path}: frozen source has no test {test}")
        except (ValueError, subprocess.CalledProcessError) as error:
            errors.append(f"{path}: cannot read frozen source: {error}")
    expected_oracle_paths = set(FROZEN_SOURCE_SHA256) - {
        "crates/quanta-index-searchd-runtime/tests/e2e_perf_chaos.rs"
    }
    if expected_oracle_paths != oracle_paths:
        errors.append("exact-oracle source set differs from the frozen four-file census")
    if id_digest(oracle_ids) != FROZEN_ORACLE_ID_SHA256:
        errors.append("exact-oracle IDs differ from the frozen four-test census")

    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(
        "runtime legacy migration: 43 perf IDs map to current bodies and "
        "4 exact internal oracles are preserved"
    )
    return 0


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--revision", help="read the release candidate's committed roster and current source"
    )
    parser.add_argument(
        "--require-execution", action="store_true", help="reject pending migration execution"
    )
    args = parser.parse_args()
    evidence_dir = os.environ.get("QUANTA_RETIREMENT_NEXTEST_EVIDENCE_DIR")
    raise SystemExit(
        main(
            revision=args.revision,
            require_execution=args.require_execution,
            evidence_dir=Path(evidence_dir) if evidence_dir else None,
        )
    )
