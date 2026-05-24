#!/usr/bin/env python3
"""Enforce quanta-index domain + hexagonal crate boundaries."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import re
import sys

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover
    import tomli as tomllib  # type: ignore[no-redef]


ROOT = Path(__file__).resolve().parents[3]
CRATES = ROOT / "crates"

DOMAINS = ("bundle_ingest", "generation", "materialization", "query")

LEGACY_CORE_MODULES = (
    "artifact_objects",
    "bundle_ingest",
    "generation_registry",
    "query_serving",
    "ports.rs",
    "services",
)

FORBIDDEN_VENDOR_DEPS = frozenset({"rusqlite", "tantivy", "lancedb", "lance"})

_ADAPTER_CRATE_DEPS = frozenset({"quanta-index-contract", "quanta-index-core"})

ALLOWED_CRATE_DEPS: dict[str, frozenset[str]] = {
    "quanta-index-contract": frozenset(),
    "quanta-index-core": frozenset({"quanta-index-contract"}),
    "quanta-index-control": frozenset({"quanta-index-contract", "quanta-index-core"}),
    "quanta-index-lexical": _ADAPTER_CRATE_DEPS,
    "quanta-index-semantic": _ADAPTER_CRATE_DEPS,
    "quanta-index-ipc": _ADAPTER_CRATE_DEPS,
    "quanta-index-searchd": frozenset(
        {
            "quanta-index-contract",
            "quanta-index-core",
            "quanta-index-control",
            "quanta-index-lexical",
            "quanta-index-semantic",
            "quanta-index-ipc",
        }
    ),
}

_ADAPTER_CRATES = frozenset(
    {
        "quanta-index-lexical",
        "quanta-index-semantic",
        "quanta-index-ipc",
    }
)

DOMAIN_USE_RE = re.compile(
    r"\b(?:crate::domains::|domains::)(?P<target>bundle_ingest|generation|materialization|query)\b"
)


@dataclass(frozen=True)
class Violation:
    path: Path
    message: str


def crate_name(cargo_toml: Path) -> str:
    data = tomllib.loads(cargo_toml.read_text(encoding="utf-8"))
    name = data.get("package", {}).get("name")
    if not isinstance(name, str):
        raise ValueError(f"missing package.name in {cargo_toml}")
    return name


def path_dependencies(cargo_toml: Path) -> set[str]:
    data = tomllib.loads(cargo_toml.read_text(encoding="utf-8"))
    deps: set[str] = set()
    for section in ("dependencies", "dev-dependencies", "build-dependencies"):
        block = data.get(section, {})
        if not isinstance(block, dict):
            continue
        for value in block.values():
            if not isinstance(value, dict):
                continue
            dep_name = value.get("package") or value.get("name")
            path = value.get("path")
            if path is not None and isinstance(dep_name, str):
                deps.add(dep_name)
            elif path is not None:
                # path-only dep: infer crate folder name from path tail
                deps.add(Path(str(path)).name.replace("-", "_"))
    return deps


def check_crate_dependency_matrix() -> list[Violation]:
    violations: list[Violation] = []
    for cargo_toml in sorted(CRATES.glob("*/Cargo.toml")):
        name = crate_name(cargo_toml)
        allowed = ALLOWED_CRATE_DEPS.get(name)
        if allowed is None:
            violations.append(
                Violation(cargo_toml, f"unknown workspace crate {name!r} (update ALLOWED_CRATE_DEPS)")
            )
            continue

        for dep in sorted(path_dependencies(cargo_toml)):
            if dep.startswith("quanta-index-") and dep not in allowed:
                violations.append(
                    Violation(
                        cargo_toml,
                        f"{name} must not depend on {dep} (allowed: {sorted(allowed)})",
                    )
                )

        all_deps = tomllib.loads(cargo_toml.read_text(encoding="utf-8")).get("dependencies", {})
        if name == "quanta-index-core" and isinstance(all_deps, dict):
            for dep_key in all_deps:
                base = dep_key.split("/")[0]
                if base in FORBIDDEN_VENDOR_DEPS:
                    violations.append(
                        Violation(
                            cargo_toml,
                            f"quanta-index-core must not depend on vendor crate {dep_key!r}",
                        )
                    )

        if name == "quanta-index-contract":
            if path_dependencies(cargo_toml):
                violations.append(
                    Violation(cargo_toml, "quanta-index-contract must not depend on workspace crates")
                )

        if name in _ADAPTER_CRATES:
            peer_adapters = path_dependencies(cargo_toml) & _ADAPTER_CRATES
            if peer_adapters:
                violations.append(
                    Violation(
                        cargo_toml,
                        f"{name} must not depend on other adapter crates: {sorted(peer_adapters)}",
                    )
                )

    return violations


def check_legacy_core_layout() -> list[Violation]:
    violations: list[Violation] = []
    core_src = CRATES / "quanta-index-core" / "src"
    for legacy in LEGACY_CORE_MODULES:
        path = core_src / legacy
        if path.exists():
            violations.append(
                Violation(path, f"legacy module {legacy!r} must migrate under domains/")
            )
    return violations


def owning_domain(rel_path: Path) -> str | None:
    parts = rel_path.parts
    if "domains" not in parts:
        return None
    idx = parts.index("domains")
    if idx + 1 >= len(parts):
        return None
    domain = parts[idx + 1]
    return domain if domain in DOMAINS else None


def check_domain_isolation() -> list[Violation]:
    violations: list[Violation] = []
    core_domains = CRATES / "quanta-index-core" / "src" / "domains"
    if not core_domains.is_dir():
        violations.append(Violation(core_domains, "missing domains/ directory in quanta-index-core"))
        return violations

    for rust_file in sorted(core_domains.rglob("*.rs")):
        owner = owning_domain(rust_file.relative_to(core_domains))
        if owner is None:
            continue
        text = rust_file.read_text(encoding="utf-8")
        for match in DOMAIN_USE_RE.finditer(text):
            target = match.group("target")
            if target != owner:
                violations.append(
                    Violation(
                        rust_file,
                        f"domain {owner!r} must not reference domain {target!r}",
                    )
                )
    return violations


def check_contract_is_dto_only() -> list[Violation]:
    violations: list[Violation] = []
    contract_src = CRATES / "quanta-index-contract" / "src"
    for rust_file in sorted(contract_src.rglob("*.rs")):
        text = rust_file.read_text(encoding="utf-8")
        if re.search(r"\bpub\s+trait\b", text):
            violations.append(
                Violation(rust_file, "contract crate must not define port traits (use quanta-index-core)")
            )
    return violations


def check_control_is_sqlite_only() -> list[Violation]:
    violations: list[Violation] = []
    control_toml = CRATES / "quanta-index-control" / "Cargo.toml"
    if not control_toml.is_file():
        return violations
    deps = tomllib.loads(control_toml.read_text(encoding="utf-8")).get("dependencies", {})
    if isinstance(deps, dict):
        for dep in deps:
            base = dep.split("/")[0]
            if base in FORBIDDEN_VENDOR_DEPS and base != "rusqlite":
                violations.append(
                    Violation(control_toml, f"quanta-index-control must not depend on {dep!r}")
                )
    return violations


def main() -> int:
    checks = (
        check_crate_dependency_matrix,
        check_legacy_core_layout,
        check_domain_isolation,
        check_contract_is_dto_only,
        check_control_is_sqlite_only,
    )

    violations: list[Violation] = []
    for check in checks:
        violations.extend(check())

    if violations:
        print("Hexagonal boundary check failed:", file=sys.stderr)
        for item in violations:
            rel = item.path.relative_to(ROOT) if item.path.is_absolute() else item.path
            print(f"  - {rel}: {item.message}", file=sys.stderr)
        return 1

    print("Hexagonal boundary check passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
