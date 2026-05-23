#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import sys

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11
    import tomli as tomllib  # type: ignore[no-redef]


ROOT = Path(__file__).resolve().parent.parent
CRATES_DIR = ROOT / "crates"


def main() -> int:
    bad: list[Path] = []

    for cargo_toml in sorted(CRATES_DIR.glob("*/Cargo.toml")):
        data = tomllib.loads(cargo_toml.read_text(encoding="utf-8"))
        lints = data.get("lints")
        if not isinstance(lints, dict) or lints.get("workspace") is not True:
            bad.append(cargo_toml)

    if bad:
        print("These crates do not inherit workspace lints:")
        for path in bad:
            print(f" - {path}")
        return 1

    print(f"All {len(list(CRATES_DIR.glob('*/Cargo.toml')))} workspace crates inherit workspace lints.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
