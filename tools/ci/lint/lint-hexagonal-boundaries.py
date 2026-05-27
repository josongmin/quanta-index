#!/usr/bin/env python3
"""Enforce quanta-index domain + hexagonal crate boundaries."""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover
    import tomli as tomllib  # type: ignore[no-redef]


ROOT = Path(__file__).resolve().parents[3]
CRATES = ROOT / "crates"

DOMAINS = ("lexical", "semantic", "hybrid")

LEGACY_CORE_MODULES = (
    "artifact_objects",
    "bundle_ingest",
    "generation",
    "generation_registry",
    "materialization",
    "query",
    "query_serving",
    "ports.rs",
    "services",
)

FORBIDDEN_VENDOR_DEPS = frozenset({"rusqlite", "tantivy", "lancedb", "lance"})

_ADAPTER_CRATE_DEPS = frozenset({"quanta-index-contract", "quanta-index-core"})

ALLOWED_CRATE_DEPS: dict[str, frozenset[str]] = {
    "quanta-index-contract-base": frozenset(),
    "quanta-index-contract": frozenset({"quanta-index-lq-norm"}),
    "quanta-index-core": frozenset({"quanta-index-contract"}),
    "quanta-index-lexical": _ADAPTER_CRATE_DEPS,
    "quanta-index-semantic": _ADAPTER_CRATE_DEPS,
    "quanta-index-ipc": _ADAPTER_CRATE_DEPS,
    "quanta-index-search-plane": frozenset(
        {
            "quanta-index-contract",
            "quanta-index-core",
            "quanta-index-lq-bridge",
            "quanta-index-lq-norm",
        }
    ),
    # PRE-NORM lexical-query normalizer. Stand-alone until PRE-CONTRACT-EXT
    # publishes the canonical `LqQuery` carrier in the contract crate, at
    # which point this crate will start depending on quanta-index-contract.
    "quanta-index-lq-norm": frozenset({"quanta-index-contract"}),
    # LQ trigram index (in-progress). Peer of the lq-* family.
    "quanta-index-lq-trigram": frozenset({"quanta-index-contract"}),
    # LQ positional index (in-progress). Peer of the lq-* family.
    "quanta-index-lq-positions": frozenset({"quanta-index-contract"}),
    # LQ regex matcher (in-progress). Peer of the lq-* family.
    "quanta-index-lq-regex": frozenset({"quanta-index-contract"}),
    # LQ ranker (in-progress). Peer of the lq-* family.
    # LQ runtime (in-progress). Peer of the lq-* family.
    # LQ structural index (in-progress). Peer of the lq-* family.
    "quanta-index-lq-structural": frozenset({"quanta-index-contract"}),
    # LQ history index (in-progress). Peer of the lq-* family.
    # RepoMap projection / query store (in-progress). Consumed by searchd.
    "quanta-index-repomap": frozenset({"quanta-index-contract", "quanta-index-core"}),
    # Corpus parser/runner smoke helpers. No production deps; tests in
    # other crates consume it via dev-dependencies only.
    "quanta-index-corpus-smoke": frozenset(),
    "quanta-index-searchd": frozenset(
        {
            "quanta-index-contract",
            "quanta-index-core",
            "quanta-index-lexical",
            "quanta-index-semantic",
            "quanta-index-ipc",
            "quanta-index-repomap",
            "quanta-index-search-plane",
        }
    ),
    "quanta-index-searchd-runtime": frozenset(
        {
            "quanta-index-contract",
            "quanta-index-core",
            "quanta-index-ipc",
            "quanta-index-lexical",
            "quanta-index-lq-bridge",
            "quanta-index-repomap",
            "quanta-index-search-plane",
            "quanta-index-searchd",
            "quanta-index-semantic",
        }
    ),
    "quanta-index-sdk": frozenset(
        {
            "quanta-index-contract",
            "quanta-index-ipc",
        }
    ),
    "quanta-index-searchctl": frozenset({"quanta-index-contract", "quanta-index-ipc"}),
}

_ADAPTER_CRATES = frozenset(
    {
        "quanta-index-lexical",
        "quanta-index-semantic",
        "quanta-index-ipc",
    }
)

_TRANSPORT_LEAK_TOKENS = (
    re.compile(r"\bwal_mmap\b"),
    re.compile(r"\bsegment_id\b"),
)

DOMAIN_USE_RE = re.compile(r"\b(?:crate::domains::|domains::)(?P<target>lexical|semantic|hybrid)\b")


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
                deps.add(Path(str(path)).name.replace("-", "_"))
    return deps


def check_crate_dependency_matrix() -> list[Violation]:
    violations: list[Violation] = []
    for cargo_toml in sorted(CRATES.glob("*/Cargo.toml")):
        name = crate_name(cargo_toml)
        allowed = ALLOWED_CRATE_DEPS.get(name)
        if allowed is None:
            # Fallback for in-progress `quanta-index-lq-*` family crates: peers
            # that depend only on the contract crate. New additions land
            # without forcing a lint script edit; an explicit allow-list entry
            # is still preferred but no longer mandatory for scaffolding.
            if name.startswith("quanta-index-lq-"):
                allowed = frozenset({"quanta-index-contract"})
            else:
                violations.append(
                    Violation(
                        cargo_toml,
                        f"unknown workspace crate {name!r} (update ALLOWED_CRATE_DEPS)",
                    )
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
    if not core_src.is_dir():
        return violations
    for legacy in LEGACY_CORE_MODULES:
        path = core_src / legacy
        if path.exists():
            violations.append(
                Violation(path, f"legacy module {legacy!r} must not exist in core/src/")
            )
        legacy_domain = core_src / "domains" / legacy
        if legacy_domain.exists() and legacy not in DOMAINS:
            violations.append(
                Violation(
                    legacy_domain,
                    f"legacy domain {legacy!r} removed; new domains are {DOMAINS}",
                )
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
        violations.append(
            Violation(core_domains, "missing domains/ directory in quanta-index-core")
        )
        return violations

    for rust_file in sorted(core_domains.rglob("*.rs")):
        owner = owning_domain(rust_file.relative_to(core_domains))
        if owner is None:
            continue
        text = rust_file.read_text(encoding="utf-8")
        for match in DOMAIN_USE_RE.finditer(text):
            target = match.group("target")
            if target == owner:
                continue
            if owner == "hybrid" and target in ("lexical", "semantic"):
                continue
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
    if not contract_src.is_dir():
        return violations
    for rust_file in sorted(contract_src.rglob("*.rs")):
        text = rust_file.read_text(encoding="utf-8")
        if re.search(r"\bpub\s+trait\b", text):
            violations.append(
                Violation(
                    rust_file, "contract crate must not define port traits (use quanta-index-core)"
                )
            )
    return violations


def check_channel_backend_isolation() -> list[Violation]:
    violations: list[Violation] = []
    channel_src = CRATES / "quanta-index-channel" / "src"
    if channel_src.is_dir():
        api_dir = channel_src / "api"
        backend_dir = channel_src / "backends"
        if api_dir.is_dir():
            for rust_file in sorted(api_dir.rglob("*.rs")):
                text = rust_file.read_text(encoding="utf-8")
                if re.search(r"\bbackends::\w", text):
                    violations.append(
                        Violation(
                            rust_file,
                            "channel::api must not reference channel::backends",
                        )
                    )

        if backend_dir.is_dir():
            backends = [p for p in backend_dir.iterdir() if p.is_dir()]
            for backend in backends:
                other_names = {b.name for b in backends if b != backend}
                if not other_names:
                    continue
                for rust_file in sorted(backend.rglob("*.rs")):
                    text = rust_file.read_text(encoding="utf-8")
                    for other in other_names:
                        if re.search(rf"\bbackends::{re.escape(other)}\b", text):
                            violations.append(
                                Violation(
                                    rust_file,
                                    f"channel backend {backend.name!r} must not reference {other!r}",
                                )
                            )

    leak_scopes = [
        CRATES / "quanta-index-core",
        CRATES / "quanta-index-lexical",
        CRATES / "quanta-index-semantic",
        CRATES / "quanta-index-ipc",
        CRATES / "quanta-index-searchd",
    ]
    for scope in leak_scopes:
        if not scope.is_dir():
            continue
        for rust_file in sorted(scope.rglob("*.rs")):
            if "tests" in rust_file.parts:
                continue
            text = rust_file.read_text(encoding="utf-8")
            for token in _TRANSPORT_LEAK_TOKENS:
                m = token.search(text)
                if m is not None:
                    violations.append(
                        Violation(
                            rust_file,
                            f"transport-specific token {m.group(0)!r} leaked outside channel backend",
                        )
                    )
                    break
    return violations


def main() -> int:
    checks = (
        check_crate_dependency_matrix,
        check_legacy_core_layout,
        check_domain_isolation,
        check_contract_is_dto_only,
        check_channel_backend_isolation,
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
