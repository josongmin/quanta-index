#!/usr/bin/env python3
"""Structural manifest discipline for first-party workspace crates.

Every crate listed in `[workspace.members]` is checked. First-party crates
must obey the full inheritance and dependency policy:

  1. First-party `[package]` fields inherit `version`, `edition`, `license`,
     `publish` from the workspace (either dotted or table form). The shared
     signed-artifact crate retains its own exact published package metadata.
  2. `[lints]` inherits from workspace. (Cross-checked here, primary enforcement
     remains `scripts/check_workspace_lints.py`.)
  3. Every first-party external dependency (anything not named `quanta-index-*`)
     MUST use
     the workspace form — `{ workspace = true [, features = [...]] }` — so the
     version, default-features posture, and audit surface are pinned in the
     root manifest. Inline-version deps (`foo = "1.0"`) are banned because they
     bypass cargo-deny's duplicate/banned-crate checks at the workspace level.
  4. Every workspace-internal dependency (`quanta-index-*`) MUST use the
     `{ version = "...", path = "../quanta-index-..." }` form. No bare path,
     no workspace-true on internal deps.
  5. No `git = "..."` source.
  6. No `path = "..."` deps to anything outside `crates/`.

Failure modes this catches in practice:
  - copy-pasted manifest ad-hoc adds (`serde = "1.0"`) that fork the version
    line across crates
  - silent `git` deps that bypass `deny.toml` registry policy
  - mid-refactor crates that lose `version.workspace = true` (skew across PRs)
"""

from __future__ import annotations

import sys
from dataclasses import dataclass
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11
    import tomli as tomllib  # type: ignore[no-redef]


ROOT = Path(__file__).resolve().parents[3]
WORKSPACE_TOML = ROOT / "Cargo.toml"
CRATES_DIR = ROOT / "crates"

REQUIRED_PACKAGE_INHERITS: tuple[str, ...] = (
    "version",
    "edition",
    "license",
    "publish",
)

# This shared package is a workspace member with its original edition/license.
PACKAGE_METADATA_EXCEPTIONS: dict[str, dict[str, object]] = {
    "shared/quanta-installed-artifact-trust/Cargo.toml": {
        "name": "quanta-installed-artifact-trust",
        "version": "0.1.0",
        "edition": "2021",
        "license": "MIT OR Apache-2.0",
        "publish": False,
    },
}

DEP_SECTIONS: tuple[str, ...] = (
    "dependencies",
    "dev-dependencies",
    "build-dependencies",
)


@dataclass(frozen=True)
class Violation:
    path: Path
    message: str


def workspace_members(data: dict | None = None) -> list[Path]:
    if data is None:
        data = tomllib.loads(WORKSPACE_TOML.read_text(encoding="utf-8"))
    members = data.get("workspace", {}).get("members", [])
    return [ROOT / member / "Cargo.toml" for member in members]


def is_workspace_inherited(value: object) -> bool:
    """Accept both `key.workspace = true` (dotted) and `key = { workspace = true }` (table)."""
    if isinstance(value, dict):
        return value.get("workspace") is True
    return value is True


def check_package_inherits(cargo_toml: Path, data: dict) -> list[Violation]:
    violations: list[Violation] = []
    package = data.get("package", {})
    for field in REQUIRED_PACKAGE_INHERITS:
        raw = package.get(field)
        if not is_workspace_inherited(raw):
            violations.append(
                Violation(
                    cargo_toml,
                    f"[package].{field} must inherit from workspace "
                    f"(use `{field}.workspace = true`)",
                )
            )
    return violations


def check_package_metadata(cargo_toml: Path, data: dict) -> list[Violation]:
    try:
        relative = cargo_toml.relative_to(ROOT).as_posix()
    except ValueError:
        return check_package_inherits(cargo_toml, data)
    expected = PACKAGE_METADATA_EXCEPTIONS.get(relative)
    if expected is None:
        return check_package_inherits(cargo_toml, data)
    package = data.get("package", {})
    return [
        Violation(cargo_toml, f"[package].{field} must remain {value!r}")
        for field, value in expected.items()
        if package.get(field) != value
    ]


def check_lints_inherits(cargo_toml: Path, data: dict) -> list[Violation]:
    lints = data.get("lints")
    if not isinstance(lints, dict) or lints.get("workspace") is not True:
        return [
            Violation(
                cargo_toml,
                "[lints] must contain `workspace = true` to inherit deny rails",
            )
        ]
    return []


def dependency_sections(data: dict) -> list[tuple[str, dict]]:
    """Every dependency table in a manifest, top-level and target-specific.

    `[target.'cfg(...)'.dependencies]` tables carry the same policy as the
    top-level ones; a platform-gated dep must still come from
    `[workspace.dependencies]`.
    """
    sections: list[tuple[str, dict]] = []
    for section in DEP_SECTIONS:
        block = data.get(section)
        if isinstance(block, dict):
            sections.append((section, block))
    targets = data.get("target")
    if isinstance(targets, dict):
        for target, tables in targets.items():
            if not isinstance(tables, dict):
                continue
            for section in DEP_SECTIONS:
                block = tables.get(section)
                if isinstance(block, dict):
                    sections.append((f"target.{target}.{section}", block))
    return sections


def check_dependencies(cargo_toml: Path, data: dict) -> list[Violation]:
    violations: list[Violation] = []
    for section, block in dependency_sections(data):
        for name, value in block.items():
            internal = name.startswith("quanta-index-")
            violations.extend(check_single_dep(cargo_toml, section, name, value, internal))
    return violations


def check_root_git_sources(data: dict) -> list[Violation]:
    """Root source declarations must obey the same no-git rule as members."""
    sections = dependency_sections(data)
    workspace_deps = data.get("workspace", {}).get("dependencies", {})
    if isinstance(workspace_deps, dict):
        sections.append(("workspace.dependencies", workspace_deps))
    patches = data.get("patch", {})
    if isinstance(patches, dict):
        for source, block in patches.items():
            if isinstance(block, dict):
                sections.append((f"patch.{source}", block))
    replacements = data.get("replace", {})
    if isinstance(replacements, dict):
        sections.append(("replace", replacements))
    return [
        Violation(WORKSPACE_TOML, f"[{section}] dep {name!r} uses git source")
        for section, block in sections
        for name, value in block.items()
        if isinstance(value, dict) and "git" in value
    ]


def check_single_dep(
    cargo_toml: Path,
    section: str,
    name: str,
    value: object,
    internal: bool,
) -> list[Violation]:
    violations: list[Violation] = []

    # bare-string version: `foo = "1.0"`
    if isinstance(value, str):
        if internal:
            violations.append(
                Violation(
                    cargo_toml,
                    f"[{section}] internal dep {name!r} must use "
                    f'`{{ version = "...", path = "../{name}" }}` form, not a bare string',
                )
            )
        else:
            violations.append(
                Violation(
                    cargo_toml,
                    f"[{section}] external dep {name!r} must use "
                    f"`{{ workspace = true }}` form, not a bare version string. "
                    "Add it to [workspace.dependencies] first.",
                )
            )
        return violations

    if not isinstance(value, dict):
        violations.append(
            Violation(
                cargo_toml,
                f"[{section}] dep {name!r} has unexpected shape: {type(value).__name__}",
            )
        )
        return violations

    if "git" in value:
        violations.append(
            Violation(
                cargo_toml,
                f"[{section}] dep {name!r} uses git source (banned by deny.toml policy)",
            )
        )

    has_workspace_true = value.get("workspace") is True
    has_path = "path" in value

    if internal:
        if has_workspace_true:
            violations.append(
                Violation(
                    cargo_toml,
                    f"[{section}] internal dep {name!r} must use path form, "
                    "not `workspace = true`. Workspace.dependencies is reserved for "
                    "external crates.",
                )
            )
        if "package" in value and value["package"] != name:
            violations.append(
                Violation(
                    cargo_toml,
                    f"[{section}] internal dep {name!r} uses package rename {value['package']!r}",
                )
            )
        if "version" not in value:
            violations.append(
                Violation(
                    cargo_toml,
                    f"[{section}] internal dep {name!r} must specify `version` with `path`",
                )
            )
        if not has_path:
            violations.append(
                Violation(
                    cargo_toml,
                    f'[{section}] internal dep {name!r} must specify `path = "../{name}"`',
                )
            )
        else:
            path_value = value["path"]
            if isinstance(path_value, str):
                resolved = (cargo_toml.parent / path_value).resolve()
                try:
                    resolved.relative_to(CRATES_DIR.resolve())
                except ValueError:
                    violations.append(
                        Violation(
                            cargo_toml,
                            f"[{section}] dep {name!r} path escapes crates/ (resolved: {resolved})",
                        )
                    )
                else:
                    expected = (CRATES_DIR / name).resolve()
                    if resolved != expected:
                        violations.append(
                            Violation(
                                cargo_toml,
                                f"[{section}] internal dep {name!r} path must target {expected}",
                            )
                        )
    else:
        if not has_workspace_true:
            violations.append(
                Violation(
                    cargo_toml,
                    f"[{section}] external dep {name!r} must use "
                    f"`{{ workspace = true }}` form. Add to [workspace.dependencies] "
                    "in the root manifest, then reference it via workspace = true.",
                )
            )
        if has_path:
            violations.append(
                Violation(
                    cargo_toml,
                    f"[{section}] external dep {name!r} declares a path in the member manifest. "
                    "Declare its source in workspace.dependencies.",
                )
            )
        if "version" in value:
            violations.append(
                Violation(
                    cargo_toml,
                    f"[{section}] external dep {name!r} declares an inline version. "
                    "Pin its version in workspace.dependencies.",
                )
            )

    return violations


def audit_crate(cargo_toml: Path) -> list[Violation]:
    if not cargo_toml.is_file():
        return [Violation(cargo_toml, "manifest missing for workspace member")]
    data = tomllib.loads(cargo_toml.read_text(encoding="utf-8"))
    package_and_lints = check_package_metadata(cargo_toml, data) + check_lints_inherits(
        cargo_toml, data
    )
    return package_and_lints + check_dependencies(cargo_toml, data)


def main() -> int:
    root_data = tomllib.loads(WORKSPACE_TOML.read_text(encoding="utf-8"))
    members = workspace_members(root_data)
    violations = check_root_git_sources(root_data)
    for cargo_toml in members:
        violations.extend(audit_crate(cargo_toml))

    if violations:
        print("Cargo.toml hygiene check failed:", file=sys.stderr)
        for v in violations:
            rel = v.path.relative_to(ROOT) if v.path.is_absolute() else v.path
            print(f"  - {rel}: {v.message}", file=sys.stderr)
        return 1

    print(f"All {len(members)} workspace crates pass Cargo.toml hygiene.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
