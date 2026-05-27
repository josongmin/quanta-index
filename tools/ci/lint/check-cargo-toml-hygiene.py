#!/usr/bin/env python3
"""Structural manifest discipline for workspace crates.

Every crate listed in `[workspace.members]` must obey:

  1. `[package]` inherits `version`, `edition`, `license`, `publish` from the
     workspace (either dotted or table form).
  2. `[lints]` inherits from workspace. (Cross-checked here, primary enforcement
     remains `scripts/check_workspace_lints.py`.)
  3. Every external dependency (anything not named `quanta-index-*`) MUST use
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

DEP_SECTIONS: tuple[str, ...] = (
    "dependencies",
    "dev-dependencies",
    "build-dependencies",
)


@dataclass(frozen=True)
class Violation:
    path: Path
    message: str


def workspace_members() -> list[Path]:
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


def check_dependencies(cargo_toml: Path, data: dict) -> list[Violation]:
    violations: list[Violation] = []
    for section in DEP_SECTIONS:
        block = data.get(section)
        if not isinstance(block, dict):
            continue
        for name, value in block.items():
            internal = name.startswith("quanta-index-")
            violations.extend(check_single_dep(cargo_toml, section, name, value, internal))
    return violations


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
    has_inline_version = "version" in value and not has_workspace_true

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
                if not str(resolved).startswith(str(CRATES_DIR.resolve())):
                    violations.append(
                        Violation(
                            cargo_toml,
                            f"[{section}] dep {name!r} path escapes crates/ (resolved: {resolved})",
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
                    f"[{section}] external dep {name!r} declares a path. "
                    "External deps must come from the registry via workspace.dependencies.",
                )
            )
        if has_inline_version:
            violations.append(
                Violation(
                    cargo_toml,
                    f"[{section}] external dep {name!r} pins an inline version "
                    "alongside `workspace = true`. Move the pin to workspace.dependencies.",
                )
            )

    return violations


def audit_crate(cargo_toml: Path) -> list[Violation]:
    if not cargo_toml.is_file():
        return [Violation(cargo_toml, "manifest missing for workspace member")]
    data = tomllib.loads(cargo_toml.read_text(encoding="utf-8"))
    return (
        check_package_inherits(cargo_toml, data)
        + check_lints_inherits(cargo_toml, data)
        + check_dependencies(cargo_toml, data)
    )


def main() -> int:
    violations: list[Violation] = []
    for cargo_toml in workspace_members():
        violations.extend(audit_crate(cargo_toml))

    if violations:
        print("Cargo.toml hygiene check failed:", file=sys.stderr)
        for v in violations:
            rel = v.path.relative_to(ROOT) if v.path.is_absolute() else v.path
            print(f"  - {rel}: {v.message}", file=sys.stderr)
        return 1

    print(f"All {len(workspace_members())} workspace crates pass Cargo.toml hygiene.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
