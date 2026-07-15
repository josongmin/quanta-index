#!/usr/bin/env python3
"""Fail closed when executable test targets lack an authoritative CI rail.

``tools/ci/test-authority.toml`` is the machine-readable authority for Rust
integration and cargo-fuzz targets.  This guard deliberately inventories only
Cargo targets:

* ``crates/*/tests/*.rs`` -- direct children are integration-test binaries;
  nested helpers such as ``tests/common/*.rs`` are modules, not targets.
* ``crates/*/fuzz/fuzz_targets/*.rs`` -- cargo-fuzz executables declared by
  the sibling fuzz manifest.

The catalog must explicitly map every discovered target to an owner and a
declared rail.  P0/P1 invariants additionally need positive, negative,
recovery, and consumer proof targets.  The guard never infers a rail from a
file name or accepts an unregistered target: both are correctness gaps.
"""

from __future__ import annotations

import argparse
import sys
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11
    import tomli as tomllib  # type: ignore[no-redef]


ROOT = Path(__file__).resolve().parents[3]
DEFAULT_CATALOG = ROOT / "tools" / "ci" / "test-authority.toml"
SUPPORTED_TIERS = frozenset({"pr", "merge", "correctness", "nightly", "weekly"})
SUPPORTED_TARGET_KINDS = frozenset({"integration", "fuzz"})
PROOF_ROLES = (
    "positive_target",
    "negative_target",
    "recovery_target",
    "consumer_target",
)


@dataclass(frozen=True)
class Violation:
    path: Path
    message: str


def _violation(path: Path, message: str) -> Violation:
    return Violation(path=path, message=message)


def _relative_path(
    value: object, *, catalog: Path, field: str, violations: list[Violation]
) -> str | None:
    if not isinstance(value, str) or not value:
        violations.append(_violation(catalog, f"{field} must be a non-empty relative path"))
        return None
    candidate = PurePosixPath(value)
    if candidate.is_absolute() or ".." in candidate.parts:
        violations.append(
            _violation(catalog, f"{field} must stay within the repository: {value!r}")
        )
        return None
    return candidate.as_posix()


def _string(
    value: object, *, catalog: Path, context: str, violations: list[Violation]
) -> str | None:
    if isinstance(value, str) and value.strip():
        return value
    violations.append(_violation(catalog, f"{context} must be a non-empty string"))
    return None


def _load_catalog(catalog: Path, violations: list[Violation]) -> dict[str, Any] | None:
    if not catalog.is_file():
        violations.append(_violation(catalog, "test-authority catalog is missing"))
        return None
    try:
        parsed = tomllib.loads(catalog.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        violations.append(_violation(catalog, f"cannot parse TOML: {error}"))
        return None
    if not isinstance(parsed, dict):
        violations.append(_violation(catalog, "catalog root must be a TOML table"))
        return None
    return parsed


def _discover_integration_targets(root: Path) -> set[str]:
    return {
        path.relative_to(root).as_posix()
        for path in root.glob("crates/*/tests/*.rs")
        if path.is_file()
    }


def _discover_fuzz_targets(root: Path) -> set[str]:
    return {
        path.relative_to(root).as_posix()
        for path in root.glob("crates/*/fuzz/fuzz_targets/*.rs")
        if path.is_file()
    }


def _load_workflow(
    *, root: Path, catalog: Path, workflow_path: str, violations: list[Violation]
) -> dict[str, Any] | None:
    """Load a repository-owned GitHub workflow without silently accepting drift."""
    path = root / workflow_path
    if not path.is_file():
        violations.append(_violation(catalog, f"rail workflow does not exist: {workflow_path}"))
        return None
    try:
        import yaml
    except ModuleNotFoundError:
        violations.append(_violation(catalog, "PyYAML is required to validate CI rail bindings"))
        return None
    try:
        parsed = yaml.safe_load(path.read_text(encoding="utf-8"))
    except (OSError, yaml.YAMLError) as error:
        violations.append(_violation(path, f"cannot parse workflow YAML: {error}"))
        return None
    if not isinstance(parsed, dict):
        violations.append(_violation(path, "workflow root must be a YAML mapping"))
        return None
    return parsed


def _validate_rail_binding(
    *,
    root: Path,
    catalog: Path,
    rail_id: str,
    raw_rail: dict[str, Any],
    command: str,
    violations: list[Violation],
) -> None:
    workflow_path = _relative_path(
        raw_rail.get("workflow"),
        catalog=catalog,
        field=f"rail {rail_id}.workflow",
        violations=violations,
    )
    job_id = _string(
        raw_rail.get("job"),
        catalog=catalog,
        context=f"rail {rail_id}.job",
        violations=violations,
    )
    step_name = _string(
        raw_rail.get("step"),
        catalog=catalog,
        context=f"rail {rail_id}.step",
        violations=violations,
    )
    if workflow_path is None or job_id is None or step_name is None:
        return
    workflow = _load_workflow(
        root=root, catalog=catalog, workflow_path=workflow_path, violations=violations
    )
    if workflow is None:
        return
    jobs = workflow.get("jobs")
    if not isinstance(jobs, dict) or not isinstance(jobs.get(job_id), dict):
        violations.append(_violation(catalog, f"rail {rail_id} workflow job does not exist: {job_id}"))
        return
    steps = jobs[job_id].get("steps")
    if not isinstance(steps, list):
        violations.append(_violation(catalog, f"rail {rail_id} job {job_id} has no steps"))
        return
    for step in steps:
        if not isinstance(step, dict) or step.get("name") != step_name:
            continue
        run = step.get("run")
        if isinstance(run, str) and command in run:
            return
        violations.append(
            _violation(
                catalog,
                f"rail {rail_id} workflow step {step_name!r} does not execute declared command",
            )
        )
        return
    violations.append(
        _violation(catalog, f"rail {rail_id} workflow step does not exist: {step_name!r}")
    )


def _validate_rails(
    *, root: Path,
    data: dict[str, Any], catalog: Path, violations: list[Violation]
) -> dict[str, dict[str, str]]:
    raw_rails = data.get("rails")
    if not isinstance(raw_rails, dict) or not raw_rails:
        violations.append(_violation(catalog, "[rails] must declare at least one rail"))
        return {}

    rails: dict[str, dict[str, str]] = {}
    for rail_id, raw_rail in sorted(raw_rails.items()):
        if not isinstance(rail_id, str) or not rail_id or not isinstance(raw_rail, dict):
            violations.append(
                _violation(catalog, "each [rails.<id>] entry must be a non-empty table")
            )
            continue
        tier = _string(
            raw_rail.get("tier"),
            catalog=catalog,
            context=f"rail {rail_id}.tier",
            violations=violations,
        )
        command = _string(
            raw_rail.get("command"),
            catalog=catalog,
            context=f"rail {rail_id}.command",
            violations=violations,
        )
        target_kind = _string(
            raw_rail.get("target_kind"),
            catalog=catalog,
            context=f"rail {rail_id}.target_kind",
            violations=violations,
        )
        if tier is not None and tier not in SUPPORTED_TIERS:
            violations.append(
                _violation(catalog, f"rail {rail_id}.tier must be one of {sorted(SUPPORTED_TIERS)}")
            )
        if target_kind is not None and target_kind not in SUPPORTED_TARGET_KINDS:
            violations.append(
                _violation(
                    catalog,
                    f"rail {rail_id}.target_kind must be one of {sorted(SUPPORTED_TARGET_KINDS)}",
                )
            )
        if tier is not None and command is not None and target_kind is not None:
            rails[rail_id] = {"tier": tier, "command": command, "target_kind": target_kind}
            _validate_rail_binding(
                root=root,
                catalog=catalog,
                rail_id=rail_id,
                raw_rail=raw_rail,
                command=command,
                violations=violations,
            )
    return rails


def _table_array(
    data: dict[str, Any], name: str, catalog: Path, violations: list[Violation]
) -> list[dict[str, Any]]:
    raw_entries = data.get(name, [])
    if not isinstance(raw_entries, list):
        violations.append(_violation(catalog, f"{name} must be an array of tables"))
        return []
    entries: list[dict[str, Any]] = []
    for index, entry in enumerate(raw_entries):
        if not isinstance(entry, dict):
            violations.append(_violation(catalog, f"{name}[{index}] must be a table"))
            continue
        entries.append(entry)
    return entries


def _validate_targets(
    *,
    root: Path,
    catalog: Path,
    rails: dict[str, dict[str, str]],
    entries: list[dict[str, Any]],
    collection: str,
    expected_kind: str,
    discovered: set[str],
    violations: list[Violation],
) -> dict[str, dict[str, str]]:
    targets: dict[str, dict[str, str]] = {}
    paths: set[str] = set()
    for index, entry in enumerate(entries):
        prefix = f"{collection}[{index}]"
        target_id = _string(
            entry.get("id"), catalog=catalog, context=f"{prefix}.id", violations=violations
        )
        path = _relative_path(
            entry.get("path"), catalog=catalog, field=f"{prefix}.path", violations=violations
        )
        owner = _string(
            entry.get("owner"), catalog=catalog, context=f"{prefix}.owner", violations=violations
        )
        rail = _string(
            entry.get("rail"), catalog=catalog, context=f"{prefix}.rail", violations=violations
        )
        if target_id is None or path is None or owner is None or rail is None:
            continue
        if target_id in targets:
            violations.append(_violation(catalog, f"duplicate target id: {target_id}"))
            continue
        if path in paths:
            violations.append(_violation(catalog, f"duplicate catalog target path: {path}"))
            continue
        paths.add(path)
        if rail not in rails:
            violations.append(
                _violation(catalog, f"target {target_id} references unknown rail {rail}")
            )
        elif rails[rail]["target_kind"] != expected_kind:
            violations.append(
                _violation(
                    catalog,
                    f"target {target_id} has kind {expected_kind} but rail {rail} is {rails[rail]['target_kind']}",
                )
            )
        if path not in discovered:
            violations.append(_violation(catalog, f"catalog target does not exist on disk: {path}"))
        targets[target_id] = {"path": path, "owner": owner, "rail": rail, "kind": expected_kind}

    for path in sorted(discovered - paths):
        violations.append(
            _violation(root / path, f"orphan {expected_kind} test target: no catalog entry")
        )
    return targets


def _validate_fuzz_manifest_bindings(
    *,
    root: Path,
    catalog: Path,
    entries: list[dict[str, Any]],
    targets: dict[str, dict[str, str]],
    violations: list[Violation],
) -> None:
    for index, entry in enumerate(entries):
        prefix = f"fuzz_targets[{index}]"
        target_id = entry.get("id")
        if not isinstance(target_id, str) or target_id not in targets:
            continue
        manifest_path = _relative_path(
            entry.get("manifest"),
            catalog=catalog,
            field=f"{prefix}.manifest",
            violations=violations,
        )
        target_name = _string(
            entry.get("target"), catalog=catalog, context=f"{prefix}.target", violations=violations
        )
        if manifest_path is None or target_name is None:
            continue
        manifest = root / manifest_path
        if not manifest.is_file():
            violations.append(
                _violation(
                    catalog, f"fuzz target {target_id} manifest does not exist: {manifest_path}"
                )
            )
            continue
        try:
            manifest_data = tomllib.loads(manifest.read_text(encoding="utf-8"))
        except (OSError, tomllib.TOMLDecodeError) as error:
            violations.append(_violation(manifest, f"cannot parse fuzz manifest: {error}"))
            continue
        raw_bins = manifest_data.get("bin", [])
        if not isinstance(raw_bins, list):
            violations.append(_violation(manifest, "[[bin]] entries must be an array"))
            continue
        expected_path = targets[target_id]["path"]
        matched = False
        for raw_bin in raw_bins:
            if not isinstance(raw_bin, dict):
                continue
            if raw_bin.get("name") != target_name or not isinstance(raw_bin.get("path"), str):
                continue
            resolved = (manifest.parent / raw_bin["path"]).resolve()
            if resolved == (root / expected_path).resolve():
                matched = True
                break
        if not matched:
            violations.append(
                _violation(
                    catalog,
                    f"fuzz target {target_id} is not declared as [[bin]] {target_name!r} at {expected_path}",
                )
            )


def _validate_invariants(
    *,
    root: Path,
    catalog: Path,
    data: dict[str, Any],
    entries: list[dict[str, Any]],
    rails: dict[str, dict[str, str]],
    targets: dict[str, dict[str, str]],
    violations: list[Violation],
) -> None:
    universe = _table_array(data, "invariant_universe", catalog, violations)
    universe_by_id: dict[str, dict[str, str]] = {}
    for index, entry in enumerate(universe):
        prefix = f"invariant_universe[{index}]"
        invariant_id = _string(
            entry.get("id"), catalog=catalog, context=f"{prefix}.id", violations=violations
        )
        risk = _string(
            entry.get("risk"), catalog=catalog, context=f"{prefix}.risk", violations=violations
        )
        owner = _string(
            entry.get("owner"), catalog=catalog, context=f"{prefix}.owner", violations=violations
        )
        source = _relative_path(
            entry.get("source"), catalog=catalog, field=f"{prefix}.source", violations=violations
        )
        if invariant_id is None or risk is None or owner is None or source is None:
            continue
        if invariant_id in universe_by_id:
            violations.append(_violation(catalog, f"duplicate invariant universe id: {invariant_id}"))
            continue
        if risk not in {"P0", "P1", "P2", "P3"}:
            violations.append(_violation(catalog, f"{prefix}.risk must be one of P0, P1, P2, P3"))
            continue
        if not (root / source).is_file():
            violations.append(_violation(catalog, f"invariant universe source does not exist: {source}"))
        universe_by_id[invariant_id] = {"risk": risk, "owner": owner, "source": source}

    if entries and not universe_by_id:
        violations.append(_violation(catalog, "invariant_universe must declare every P0/P1 invariant"))

    ids: set[str] = set()
    for index, entry in enumerate(entries):
        prefix = f"invariants[{index}]"
        invariant_id = _string(
            entry.get("id"), catalog=catalog, context=f"{prefix}.id", violations=violations
        )
        risk = _string(
            entry.get("risk"), catalog=catalog, context=f"{prefix}.risk", violations=violations
        )
        owner = _string(
            entry.get("owner"), catalog=catalog, context=f"{prefix}.owner", violations=violations
        )
        source = _relative_path(
            entry.get("source"), catalog=catalog, field=f"{prefix}.source", violations=violations
        )
        if invariant_id is not None:
            if invariant_id in ids:
                violations.append(_violation(catalog, f"duplicate invariant id: {invariant_id}"))
            ids.add(invariant_id)
            expected = universe_by_id.get(invariant_id)
            if expected is None:
                violations.append(_violation(catalog, f"invariant {invariant_id} is absent from invariant_universe"))
            elif owner is not None and source is not None and (
                expected["owner"] != owner or expected["source"] != source or expected["risk"] != risk
            ):
                violations.append(
                    _violation(catalog, f"invariant {invariant_id} disagrees with invariant_universe")
                )
        if risk is not None and risk not in {"P0", "P1", "P2", "P3"}:
            violations.append(_violation(catalog, f"{prefix}.risk must be one of P0, P1, P2, P3"))
        if owner is not None and source is not None and not (root / source).is_file():
            violations.append(
                _violation(
                    catalog, f"invariant {invariant_id or prefix} source does not exist: {source}"
                )
            )
        if risk not in {"P0", "P1"}:
            continue
        role_targets: list[str] = []
        for role in PROOF_ROLES:
            target_id = entry.get(role)
            if not isinstance(target_id, str) or not target_id:
                violations.append(
                    _violation(
                        catalog,
                        f"invariant {invariant_id or prefix} missing required proof role {role}",
                    )
                )
            elif target_id not in targets:
                violations.append(
                    _violation(
                        catalog,
                        f"invariant {invariant_id or prefix} {role} references unknown target id {target_id}",
                    )
                )
            else:
                role_targets.append(target_id)
                if role in {"positive_target", "negative_target"} and owner is not None:
                    if targets[target_id]["owner"] != owner:
                        violations.append(
                            _violation(
                                catalog,
                                f"invariant {invariant_id or prefix} {role} must be owned by {owner}",
                            )
                        )
        if len(role_targets) != len(set(role_targets)):
            violations.append(
                _violation(
                    catalog,
                    f"invariant {invariant_id or prefix} proof roles must name distinct targets",
                )
            )
        for tier_role, expected_tier in (
            ("pr_rail", "pr"),
            ("merge_rail", "merge"),
            ("nightly_rail", "nightly"),
        ):
            rail_id = entry.get(tier_role)
            if not isinstance(rail_id, str) or rail_id not in rails:
                violations.append(
                    _violation(
                        catalog,
                        f"invariant {invariant_id or prefix} {tier_role} references unknown rail",
                    )
                )
            elif rails[rail_id]["tier"] != expected_tier:
                violations.append(
                    _violation(
                        catalog,
                        f"invariant {invariant_id or prefix} {tier_role} must reference {expected_tier} rail",
                    )
                )
    required = {
        invariant_id
        for invariant_id, entry in universe_by_id.items()
        if entry["risk"] in {"P0", "P1"}
    }
    missing = sorted(required - ids)
    for invariant_id in missing:
        violations.append(_violation(catalog, f"P0/P1 invariant missing proof row: {invariant_id}"))


def audit_catalog(root: Path = ROOT, catalog: Path = DEFAULT_CATALOG) -> list[Violation]:
    """Return all fail-closed catalog violations without writing repository state."""
    root = root.resolve()
    catalog = catalog.resolve()
    violations: list[Violation] = []
    data = _load_catalog(catalog, violations)
    if data is None:
        return violations
    if data.get("format_version") != 2:
        violations.append(_violation(catalog, "format_version must equal 2"))

    rails = _validate_rails(root=root, data=data, catalog=catalog, violations=violations)
    integration_entries = _table_array(data, "integration_targets", catalog, violations)
    fuzz_entries = _table_array(data, "fuzz_targets", catalog, violations)
    targets = _validate_targets(
        root=root,
        catalog=catalog,
        rails=rails,
        entries=integration_entries,
        collection="integration_targets",
        expected_kind="integration",
        discovered=_discover_integration_targets(root),
        violations=violations,
    )
    fuzz_targets = _validate_targets(
        root=root,
        catalog=catalog,
        rails=rails,
        entries=fuzz_entries,
        collection="fuzz_targets",
        expected_kind="fuzz",
        discovered=_discover_fuzz_targets(root),
        violations=violations,
    )
    targets.update(fuzz_targets)
    _validate_fuzz_manifest_bindings(
        root=root,
        catalog=catalog,
        entries=fuzz_entries,
        targets=fuzz_targets,
        violations=violations,
    )
    _validate_invariants(
        root=root,
        catalog=catalog,
        data=data,
        entries=_table_array(data, "invariants", catalog, violations),
        rails=rails,
        targets=targets,
        violations=violations,
    )
    return violations


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root", type=Path, default=ROOT, help="repository root (default: inferred)"
    )
    parser.add_argument("--catalog", type=Path, default=DEFAULT_CATALOG, help="catalog TOML path")
    args = parser.parse_args(argv)
    violations = audit_catalog(args.root, args.catalog)
    if violations:
        for violation in violations:
            print(f"{violation.path}: {violation.message}", file=sys.stderr)
        return 1
    print(f"test authority: OK ({args.catalog})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
