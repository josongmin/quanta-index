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

# Crates a `[dev-dependencies]` table may name on top of the crate's own
# allowlist: a test drives a daemon or a corpus through them the way a
# client does. A `[dependencies]` table still needs its own allowlist entry
# to name one (the CLI's on the SDK, the benchmark driver's on the harness).
TEST_SUPPORT_CRATES = frozenset(
    {
        "quanta-index-corpus-smoke",
        "quanta-index-sdk",
        "quanta-index-searchd-harness",
    }
)

# The search-plane's test-only opener contract is exercised against the real
# lexical adapter. This edge is forbidden in production dependencies and is
# admitted only for that composition test.
DEV_INTEGRATION_DEPS = {
    "quanta-index-search-plane": frozenset({"quanta-index-lexical"}),
}

ALLOWED_CRATE_DEPS: dict[str, frozenset[str]] = {
    "quanta-index-contract-base": frozenset(),
    # The contract re-exports the base DTOs it is split from.
    "quanta-index-contract": frozenset({"quanta-index-contract-base", "quanta-index-lq-norm"}),
    "quanta-index-core": frozenset({"quanta-index-contract"}),
    # The lexical adapter also uses the lq-* index primitives and the shared
    # text normalizer (one Unicode contract for index, sidecars and query).
    "quanta-index-lexical": _ADAPTER_CRATE_DEPS
    | frozenset(
        {
            "quanta-index-lq-positions",
            "quanta-index-lq-regex",
            "quanta-index-lq-text-normalizer",
            "quanta-index-lq-trigram",
        }
    ),
    "quanta-index-semantic": _ADAPTER_CRATE_DEPS,
    # Embedding adapter: implements core outbound ports from contract DTOs.
    "quanta-index-embed": _ADAPTER_CRATE_DEPS,
    "quanta-index-ipc": _ADAPTER_CRATE_DEPS,
    # The search plane uses the transport crate for exactly its codec: the
    # CBOR payload encoding the authority snapshots persist in and the
    # canonical batch digest idempotency verifies (the wire encoding is the
    # transport's, and no vendor codec token leaves it). lq-obs is the
    # metric primitive library the plane's scrape is built from.
    "quanta-index-search-plane": frozenset(
        {
            "quanta-index-contract",
            "quanta-index-core",
            "quanta-index-ipc",
            "quanta-index-lq-bridge",
            "quanta-index-lq-norm",
            "quanta-index-lq-obs",
            "quanta-index-lq-regex",
            "quanta-index-lq-text-normalizer",
        }
    ),
    # PRE-NORM lexical-query normalizer. Stand-alone until PRE-CONTRACT-EXT
    # publishes the canonical `LqQuery` carrier in the contract crate, at
    # which point this crate will start depending on quanta-index-contract.
    # The DSL maps its `case:` option onto the text normalizer's mode.
    "quanta-index-lq-norm": frozenset({"quanta-index-contract", "quanta-index-lq-text-normalizer"}),
    # The one Unicode text normalization contract (QI-BB-011): a leaf crate
    # shared by the DSL, the lexical adapter and the search plane.
    "quanta-index-lq-text-normalizer": frozenset(),
    # LQ trigram index (in-progress). Peer of the lq-* family.
    "quanta-index-lq-trigram": frozenset({"quanta-index-contract"}),
    # LQ positional index (in-progress). Peer of the lq-* family.
    "quanta-index-lq-positions": frozenset({"quanta-index-contract"}),
    # LQ regex matcher: verifies over the trigram family's doc resolver.
    "quanta-index-lq-regex": frozenset({"quanta-index-contract", "quanta-index-lq-trigram"}),
    # LQ ranker (in-progress). Peer of the lq-* family.
    # LQ runtime (in-progress). Peer of the lq-* family.
    # LQ structural index (in-progress). Peer of the lq-* family.
    # Structural matching runs its regex predicates through the lq regex.
    "quanta-index-lq-structural": frozenset({"quanta-index-contract", "quanta-index-lq-regex"}),
    # LQ history index (in-progress). Peer of the lq-* family.
    # RepoMap projection / query store (in-progress). Consumed by searchd.
    # P02A: the compiler and query engine reuse the one shared Unicode
    # tokenizer (QI-BB-011 leaf crate); there is no route-local tokenizer.
    "quanta-index-repomap": frozenset(
        {
            "quanta-index-contract",
            "quanta-index-core",
            "quanta-index-lq-text-normalizer",
            # P03 (SEP-21 S21-01B/S21-02): the store's candidate/activation/
            # quarantine visibility authority is the durable catalog; the
            # composition root shares its one `SqliteCatalog` connection
            # with the store, exactly as it shares the idempotency and
            # auxiliary ports.
            "quanta-index-catalog",
        }
    ),
    # Corpus parser/runner smoke helpers. No production deps; tests in
    # other crates consume it via dev-dependencies only.
    "quanta-index-corpus-smoke": frozenset(),
    # Durable catalog adapter (W2, G0-C: SQLite via rusqlite). Owns the storage
    # engine; the search plane reaches it only through core ports.
    "quanta-index-catalog": frozenset(
        {"quanta-index-contract", "quanta-index-core", "quanta-index-ipc"}
    ),
    # Non-production benchmark driver. It drives the lexical adapter
    # directly (in-process, no daemon) and writes the one benchmark artifact
    # envelope the harness owns (`BenchArtifactV1`, QI-BB-010), so it may
    # depend on the harness crate for that model; it must not orchestrate
    # the runtime itself.
    "quanta-index-scan-experiment": frozenset(
        {
            "quanta-index-contract",
            "quanta-index-core",
            # Direct batch producers must stamp the one IPC-owned digest.
            "quanta-index-ipc",
            "quanta-index-lexical",
            "quanta-index-searchd-harness",
        }
    ),
    # The composition root: the one crate that names concrete adapters,
    # the embedding provider and the structural matcher included. Structural
    # repo/file filters compile once per request through the shared regex
    # executor so their resource refusals match lexical and search-plane paths.
    "quanta-index-searchd": frozenset(
        {
            "quanta-index-contract",
            "quanta-index-core",
            "quanta-index-embed",
            "quanta-index-lexical",
            "quanta-index-lq-regex",
            "quanta-index-lq-structural",
            "quanta-index-semantic",
            "quanta-index-ipc",
            "quanta-index-repomap",
            "quanta-index-search-plane",
        }
    ),
    "quanta-index-searchd-runtime": frozenset(
        {
            "quanta-index-catalog",
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
    # Black-box runtime/quality harness. Its dependency fan-in is intentional:
    # production crates must never depend back on this crate. The lexical
    # adapter is a direct dependency so the scale rail can time the sealed
    # generation's adapter-only open/plan/execute beside the daemon's; the
    # semantic adapter so the ANN rail can seal and query a generation
    # through the production build, seal and open path (QI-BB-027).
    "quanta-index-searchd-harness": frozenset(
        {
            "quanta-index-contract",
            "quanta-index-core",
            "quanta-index-embed",
            "quanta-index-ipc",
            "quanta-index-lexical",
            "quanta-index-search-plane",
            "quanta-index-searchd",
            "quanta-index-searchd-runtime",
            "quanta-index-semantic",
        }
    ),
    "quanta-index-sdk": frozenset(
        {
            "quanta-index-contract",
            "quanta-index-ipc",
        }
    ),
    # The operator CLI is a client of the SDK.
    "quanta-index-searchctl": frozenset(
        {"quanta-index-contract", "quanta-index-ipc", "quanta-index-sdk"}
    ),
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

_TEST_PATH_MODULE = re.compile(r'#\[cfg\(test\)\]\s*#\[path\s*=\s*"([^"\n]+)"\]\s*mod\s+\w+\s*;')


def test_only_path_modules(scope: Path) -> set[Path]:
    """Identify out-of-line Rust modules compiled only under cfg(test)."""
    targets: set[Path] = set()
    for source in scope.rglob("*.rs"):
        text = source.read_text(encoding="utf-8")
        for match in _TEST_PATH_MODULE.finditer(text):
            target = (source.parent / match.group(1)).resolve()
            if target.is_file() and target.is_relative_to(scope.resolve()):
                targets.add(target)
    return targets


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


PRODUCTION_SECTIONS = ("dependencies", "build-dependencies")
DEV_SECTIONS = ("dev-dependencies",)


def path_dependencies(
    cargo_toml: Path,
    sections: tuple[str, ...] = PRODUCTION_SECTIONS + DEV_SECTIONS,
) -> set[str]:
    """The package names of every path dependency in `sections`, as Cargo
    names them.

    A dependency's package is its table key unless `package = "..."`
    renames it; the path's directory name is never the authority. (Deriving
    the name from the path, underscored, once made every internal
    dependency invisible to the `quanta-index-` allowlist check.)
    """
    data = tomllib.loads(cargo_toml.read_text(encoding="utf-8"))
    deps: set[str] = set()
    for section in sections:
        block = data.get(section, {})
        if not isinstance(block, dict):
            continue
        for key, value in block.items():
            if not isinstance(value, dict) or value.get("path") is None:
                continue
            package = value.get("package", key)
            if not isinstance(package, str):
                raise ValueError(f"non-string package name for {key!r} in {cargo_toml}")
            deps.add(package)
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

        for dep in sorted(path_dependencies(cargo_toml, PRODUCTION_SECTIONS)):
            if dep.startswith("quanta-index-") and dep not in allowed:
                violations.append(
                    Violation(
                        cargo_toml,
                        f"{name} must not depend on {dep} (allowed: {sorted(allowed)})",
                    )
                )
        dev_allowed = allowed | TEST_SUPPORT_CRATES | DEV_INTEGRATION_DEPS.get(name, frozenset())
        for dep in sorted(path_dependencies(cargo_toml, DEV_SECTIONS)):
            if dep.startswith("quanta-index-") and dep not in dev_allowed and dep != name:
                violations.append(
                    Violation(
                        cargo_toml,
                        f"{name} must not dev-depend on {dep} (allowed: {sorted(dev_allowed)})",
                    )
                )

        all_deps = tomllib.loads(cargo_toml.read_text(encoding="utf-8")).get("dependencies", {})
        if name == "quanta-index-core" and isinstance(all_deps, dict):
            for dep_key, dep_value in all_deps.items():
                package = (
                    dep_value.get("package", dep_key) if isinstance(dep_value, dict) else dep_key
                )
                base = package.split("/")[0]
                if base in FORBIDDEN_VENDOR_DEPS:
                    violations.append(
                        Violation(
                            cargo_toml,
                            f"quanta-index-core must not depend on vendor crate {package!r}",
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
        test_only_sources = test_only_path_modules(scope)
        for rust_file in sorted(scope.rglob("*.rs")):
            if "tests" in rust_file.parts or rust_file.resolve() in test_only_sources:
                continue
            text = rust_file.read_text(encoding="utf-8")
            # The lexical adapter binds Tantivy segment files at seal/open.
            # This concrete storage API is unrelated to the channel backend's
            # transport identifier; keep every other segment_id use checked.
            if scope.name == "quanta-index-lexical" and rust_file.name == "ranked_keys.rs":
                text = re.sub(r"\breader\.segment_id\s*\(\s*\)", "", text)
            if scope.name == "quanta-index-lexical":
                # The qualified native accessor identifies the concrete
                # storage API without granting transport fields or arbitrary
                # receiver methods an exemption.
                text = re.sub(r"\btantivy::SegmentReader::segment_id\s*\(", "", text)
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
