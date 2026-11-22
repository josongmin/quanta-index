#!/usr/bin/env python3
"""Require an explicit disposition for every Legacy lexical integration test."""

from __future__ import annotations

import argparse
import hashlib
import os
import re
import subprocess
import sys
from functools import cache
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python 3.10 compatibility
    import tomli as tomllib  # type: ignore[no-redef]


ROOT = Path(__file__).resolve().parents[3]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
from tools.ci.retirement_nextest import (  # noqa: E402
    RUST_PROOF_SOURCE,
    check_migration_observations,
    retained_executable_digests,
)
from tools.ci.rust_test_mount import committed_path_resolver, retirement_event_matcher  # noqa: E402

MANIFEST = ROOT / "tools/ci/inventory/lexical-test-disposition.toml"
LEXICAL_CARGO_MANIFEST = ROOT / "crates/quanta-index-lexical/Cargo.toml"
TEST_ATTRIBUTE = re.compile(r"^\s*#\[test\]\s*$")
TEST_FUNCTION = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)"
)
ALLOWED_DISPOSITIONS = {"retain", "migrate", "retire"}
ALLOWED_PROOF = {"VERIFIED", "FAILED", "BLOCKED", "NOT_RUN", "NOT_APPLICABLE"}

# Removal of the adapter and its Cargo feature only proves the old execution
# path is gone. A body waiver applies to reviewed engine-only test identities.
ENGINE_ONLY_RETIREMENTS = frozenset({
    (
        "crates/quanta-index-lexical/tests/sealed_manifest.rs",
        "point_inventory_matches_full_inventory_for_digest_and_family_symlink",
    ),
})

# Sorted, newline-joined IDs from d8a490565, the last committed pre-removal sources.
# Pin identity outside the editable roster so removing a target and its mapping
# together still fails, including in shallow and source-only checkouts.
FROZEN_TARGETS = {
    f"crates/quanta-index-lexical/tests/{target}.rs": (digest, feature)
    for target, digest, feature in (
        (
            "sealed_manifest",
            "8d3101fbe7d48579887d8964cc235e968c4944e5d50ff1a0320dc1a7567e1457",
            "legacy-tantivy",
        ),
        (
            "text_authority_shards",
            "0bb097bf75e68d78fe4be89f6e858d2ca05ef75231c5a787a0aefdea1c0cfcfb",
            "legacy-tantivy",
        ),
        (
            "tantivy_smoke",
            "29b243b782c3fcdac4c123018f8d8d9d9c4a85fd2f2fd3a1f40cddab8afdf4a1",
            "legacy-tantivy",
        ),
        (
            "sealed_commitment_cost",
            "1d109cf205e9778ef1f9acd2c53e03d3f69fa93e5a24dd4b5c6b700f69a00d80",
            None,
        ),
    )
}


def source_tests(path: Path, source: str) -> list[str]:
    lines = source.splitlines()
    tests: list[str] = []
    for index, line in enumerate(lines):
        if not TEST_ATTRIBUTE.match(line):
            continue
        for candidate in lines[index + 1 : index + 20]:
            match = TEST_FUNCTION.match(candidate)
            if match:
                tests.append(match.group(1))
                break
        else:
            raise ValueError(f"{path}: #[test] at line {index + 1} has no function")
    return tests


def git_text(*args: str) -> str:
    return subprocess.check_output(
        ["git", "--no-replace-objects", *args], cwd=ROOT
    ).decode("utf-8")


def replacement_test_ids(candidates: object) -> set[tuple[str, str]]:
    if not isinstance(candidates, list) or not candidates:
        raise ValueError("VERIFIED disposition requires explicit replacement tests")
    selected: set[tuple[str, str]] = set()
    for candidate in candidates:
        if not isinstance(candidate, dict) or set(candidate) != {"path", "tests"}:
            raise ValueError("replacement candidate requires a Rust path and test IDs")
        path = candidate["path"]
        if not isinstance(path, str) or not RUST_PROOF_SOURCE.fullmatch(path):
            raise ValueError(f"invalid replacement candidate path: {path!r}")
        tests = candidate["tests"]
        if not isinstance(tests, list) or not tests:
            raise ValueError(f"{path}: replacement candidate has no test IDs")
        for test in tests:
            if not isinstance(test, str) or not test.isascii() or not test.isidentifier():
                raise ValueError(f"{path}: invalid replacement test ID {test!r}")
            if (path, test) in selected:
                raise ValueError(f"duplicate replacement test ID: {path}::{test}")
            selected.add((path, test))
    return selected


def main(
    *,
    revision: str | None = None,
    inventory_only: bool = False,
    evidence_dir: Path | None = None,
) -> int:
    if revision is not None and inventory_only:
        print("committed retirement checks require migration dispositions", file=sys.stderr)
        return 1
    errors: list[str] = []
    committed_paths: set[str] | None = None
    pending_groups: list[str] = []
    executable_digest = retained_executable_digests()

    def read(path: Path) -> str:
        if committed_paths is None:
            return path.read_bytes().decode("utf-8")
        relative = path.relative_to(ROOT).as_posix()
        if relative not in committed_paths:
            raise FileNotFoundError(relative)
        return git_text("show", f"{revision}:{relative}")

    @cache
    def candidate_test_names(path: Path) -> frozenset[str]:
        return frozenset(source_tests(path, read(path)))

    resolve_path = None if revision is None else committed_path_resolver(
        lambda _root, *args: git_text(*args).encode("utf-8"), ROOT, revision,
    )
    event_matches = retirement_event_matcher(
        ROOT, lambda relative: read(ROOT / relative), resolve_path=resolve_path,
    )

    try:
        if revision is not None:
            if re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", revision) is None:
                raise ValueError("release revision must be a full Git commit ID")
            committed_paths = set(git_text("ls-tree", "-rz", "--name-only", revision).split("\0"))
        document = tomllib.loads(read(MANIFEST))
        cargo_document = tomllib.loads(read(LEXICAL_CARGO_MANIFEST))
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"lexical disposition input: {error}", file=sys.stderr)
        return 1

    # A removed [features] table means the crate currently exposes no named
    # features. Keep evaluating the disposition manifest so the failure names
    # every Legacy target whose migration proof is still incomplete.
    cargo_features = cargo_document.get("features", {})
    if not isinstance(cargo_features, dict):
        print(f"{LEXICAL_CARGO_MANIFEST}: expected a [features] table", file=sys.stderr)
        return 1

    targets = document.get("targets")
    if document.get("version") != 1 or not isinstance(targets, list) or not targets:
        print(f"{MANIFEST}: expected version=1 and a nonempty [[targets]] array", file=sys.stderr)
        return 1

    seen_paths: set[str] = set()
    total = 0
    for target in targets:
        if not isinstance(target, dict):
            errors.append("target entry is not a table")
            continue
        relative = target.get("path")
        if not isinstance(relative, str) or relative in seen_paths:
            errors.append(f"invalid or duplicate target path: {relative!r}")
            continue
        seen_paths.add(relative)
        path = ROOT / relative
        frozen = FROZEN_TARGETS.get(relative)
        required_feature = target.get("required_feature")
        if required_feature is not None and (
            not isinstance(required_feature, str) or not required_feature.strip()
        ):
            errors.append(f"{relative}: required_feature must be a nonempty string")
            required_feature = None
        if frozen is not None and required_feature != frozen[1]:
            errors.append(f"{relative}: expected frozen required_feature {frozen[1]!r}")
        actual: list[str] | None
        source_removed = False
        try:
            actual = source_tests(path, read(path))
        except FileNotFoundError:
            actual = None
            source_removed = True
            if frozen is None:
                errors.append(
                    f"{relative}: cannot inventory missing source without a frozen target"
                )
        except (OSError, ValueError, subprocess.CalledProcessError) as error:
            errors.append(f"{relative}: cannot inventory source test IDs: {error}")
            actual = None

        feature_removed = required_feature is not None and required_feature not in cargo_features
        mapped: dict[str, str] = {}
        pending = 0
        dispositions = target.get("dispositions")
        if not isinstance(dispositions, list) or not dispositions:
            errors.append(f"{relative}: no disposition groups")
            continue
        for group in dispositions:
            if not isinstance(group, dict):
                errors.append(f"{relative}: disposition group is not a table")
                continue
            disposition = group.get("disposition")
            proof = group.get("proof")
            replacement = group.get("replacement")
            tests = group.get("tests")
            if not isinstance(disposition, str) or disposition not in ALLOWED_DISPOSITIONS:
                errors.append(f"{relative}: invalid disposition {disposition!r}")
            if not isinstance(proof, str) or proof not in ALLOWED_PROOF:
                errors.append(f"{relative}: invalid proof {proof!r}")
            elif disposition == "migrate" and proof == "NOT_APPLICABLE":
                errors.append(f"{relative}: migration requires a body outcome")
            if not isinstance(replacement, str) or not replacement.strip():
                errors.append(f"{relative}: disposition lacks a replacement/retention rail")
            retired_engine = (
                disposition == "retire"
                and proof == "NOT_APPLICABLE"
                and frozen is not None
                and source_removed
                and feature_removed
            )
            if disposition == "retire" and proof != "VERIFIED" and not retired_engine:
                errors.append(
                    f"{relative}: retirement requires VERIFIED proof or a removed frozen engine target"
                )
            if not isinstance(tests, list) or not tests:
                errors.append(f"{relative}: disposition has no test IDs")
                continue
            remaining = group.get("remaining_assertions", [])
            if not isinstance(remaining, list) or any(
                not isinstance(item, str) or not item.strip() for item in remaining
            ):
                errors.append(f"{relative}: remaining assertions must be nonempty descriptions")
                proof = "BLOCKED"
                remaining = []
            elif remaining and proof == "VERIFIED":
                errors.append(f"{relative}: verified disposition still has missing assertions")
                proof = "BLOCKED"
            if disposition in ("migrate", "retire") and ("candidates" in group or proof == "VERIFIED"):
                try:
                    expected = replacement_test_ids(group.get("candidates"))
                    for candidate_path, case in sorted(expected):
                        if case not in candidate_test_names(ROOT / candidate_path):
                            raise ValueError(f"replacement test {case!r} is absent from {candidate_path}")
                    if proof == "VERIFIED":
                        observed = check_migration_observations(
                            group.get("observations"), read,
                            root=ROOT, source_tests=source_tests,
                            inventory_only=inventory_only, evidence_dir=evidence_dir,
                            release_revision=revision,
                            event_matches=event_matches,
                            executable_digest=executable_digest,
                        )
                        if observed != expected:
                            raise ValueError(
                                "terminal observations differ from declared replacement tests: "
                                f"missing={sorted(expected - observed)} extra={sorted(observed - expected)}"
                            )
                except (OSError, ValueError, subprocess.CalledProcessError) as error:
                    errors.append(f"{relative}: {error}")
                    proof = "BLOCKED"
            if not retired_engine and (
                disposition not in ("migrate", "retire") or proof != "VERIFIED"
            ):
                pending += len(tests)
                if source_removed or feature_removed:
                    names = ", ".join(str(test) for test in tests)
                    detail = "".join(f"\n    Missing assertion: {assertion}" for assertion in remaining)
                    pending_groups.append(f"{relative} [{proof}] {names}\n  {replacement}{detail}")
            for test in tests:
                if not isinstance(test, str) or not test.isascii() or not test.isidentifier():
                    errors.append(f"{relative}: invalid test ID {test!r}")
                    continue
                if retired_engine and (relative, test) not in ENGINE_ONLY_RETIREMENTS:
                    errors.append(f"{relative}::{test}: engine-only retirement is not recorded")
                if test in mapped:
                    errors.append(
                        f"{relative}: {test} is mapped twice ({mapped[test]}, {disposition})"
                    )
                else:
                    mapped[test] = str(disposition)

        if actual is not None:
            actual_set = set(actual)
            mapped_set = set(mapped)
            for missing in sorted(actual_set - mapped_set):
                errors.append(f"{relative}: unmapped test ID {missing}")
            for stale in sorted(mapped_set - actual_set):
                errors.append(f"{relative}: stale/deleted test ID remains mapped: {stale}")
            if len(actual) != len(actual_set):
                errors.append(f"{relative}: duplicate source test ID")
        if frozen is not None:
            digest = hashlib.sha256("\n".join(sorted(mapped)).encode("utf-8")).hexdigest()
            if digest != frozen[0]:
                errors.append(f"{relative}: mapped test IDs differ from the frozen census")
        total += len(mapped)

        if pending and not inventory_only and (source_removed or feature_removed):
            reason = (
                "source was removed"
                if source_removed
                else f"Cargo feature {required_feature!r} was removed"
            )
            errors.append(
                f"{relative}: {reason} with {pending} test IDs still lacking VERIFIED migrate/retire proof"
            )

    for missing in sorted(FROZEN_TARGETS.keys() - seen_paths):
        errors.append(f"{missing}: missing frozen target")
    if errors:
        if not inventory_only:
            errors.extend(pending_groups)
        print("\n".join(errors), file=sys.stderr)
        return 1
    scope = "inventory only" if inventory_only else "disposition"
    print(f"lexical test {scope}: {total} IDs across {len(targets)} targets")
    return 0


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--revision", help="read the release candidate's committed disposition and test files"
    )
    parser.add_argument(
        "--inventory-only", action="store_true", help="validate the development roster during migration"
    )
    args = parser.parse_args()
    evidence_dir = os.environ.get("QUANTA_RETIREMENT_NEXTEST_EVIDENCE_DIR")
    raise SystemExit(main(
        revision=args.revision, inventory_only=args.inventory_only,
        evidence_dir=Path(evidence_dir) if evidence_dir else None,
    ))
