#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11
    import tomli as tomllib  # type: ignore[no-redef]


ROOT = Path(__file__).resolve().parent.parent
CRATES_DIR = ROOT / "crates"
DUPLICATE_LINT = "clippy::multiple_crate_versions"


def local_duplicate_lint_overrides(crate_dir: Path) -> list[Path]:
    return sorted(
        path
        for path in crate_dir.rglob("*.rs")
        if DUPLICATE_LINT in path.read_text(encoding="utf-8")
    )


def main() -> int:
    bad: list[Path] = []
    duplicate_lint_overrides: list[Path] = []

    for cargo_toml in sorted(CRATES_DIR.glob("*/Cargo.toml")):
        data = tomllib.loads(cargo_toml.read_text(encoding="utf-8"))
        lints = data.get("lints")
        if not isinstance(lints, dict) or lints.get("workspace") is not True:
            bad.append(cargo_toml)
        duplicate_lint_overrides.extend(local_duplicate_lint_overrides(cargo_toml.parent))

    if bad:
        print("These crates do not inherit workspace lints:")
        for path in bad:
            print(f" - {path}")
        return 1

    if duplicate_lint_overrides:
        print(
            "Crate-local multiple_crate_versions overrides reactivate the "
            "allowlisted Cargo-graph lint; cargo-deny owns duplicate policy:"
        )
        for path in duplicate_lint_overrides:
            print(f" - {path}")
        return 1

    print(
        f"All {len(list(CRATES_DIR.glob('*/Cargo.toml')))} workspace crates inherit workspace lints."
    )
    print("No crate-local multiple_crate_versions overrides.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
