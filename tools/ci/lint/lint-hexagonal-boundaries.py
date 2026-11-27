#!/usr/bin/env python3
"""Enforce quanta-index domain + hexagonal crate boundaries."""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path
from runpy import run_path

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
    "ports",
    "services",
)

FORBIDDEN_VENDOR_DEPS = frozenset({"rusqlite", "tantivy", "lancedb", "lance"})

# Borrowed construction protocols belong with the DTO visitor they govern:
# moving this callback contract to core would create a contract -> core cycle.
# This admits no storage, transport or service port, even in the same file.
NATIVE_CORPUS_DECODE_ADMISSION_V1 = """
pub trait NativeCorpusDecodeAdmissionV1:
    quanta_index_contract_base::NativeIdentityDecodeAdmissionV1<Error = NativeCorpusDecodeRefusalV1>
    + quanta_index_contract_base::NativeActivationTokenDecodeAdmissionV1<
        ControlError = Self::OriginalError,
        Error = NativeCorpusDecodeRefusalV1,
    >
{
    fn consume_corpus_work_v1(&mut self, units: u64) -> Result<(), Self::OriginalError>;
    fn refuse_corpus_work_arithmetic_v1(&mut self) -> Self::OriginalError;
    fn admit_corpus_string_birth_v1(
        &mut self,
        bytes: usize,
        birth: &mut dyn FnMut() -> bool,
    ) -> Result<bool, Self::OriginalError>;
    fn corpus_invalid_data_v1(
        &self,
        cause: NativeCorpusDataFailureV1,
        field: Option<&'static str>,
    ) -> NativeCorpusDecodeRefusalV1;
}
"""
CONTRACT_CONSTRUCTION_TRAITS = {
    Path("ipc/control/native_decode_v1.rs"): {
        "NativeCorpusDecodeAdmissionV1": NATIVE_CORPUS_DECODE_ADMISSION_V1
    },
}


def approved_construction_trait(source: str, start: int, expected: str) -> bool:
    opening = source.find("{", start)
    if opening < 0:
        return False
    depth = 0
    for offset in range(opening, len(source)):
        if source[offset] == "{":
            depth += 1
        elif source[offset] == "}":
            depth -= 1
            if depth == 0:
                actual = source[start : offset + 1]
                return re.sub(r"\s+", "", actual) == re.sub(r"\s+", "", expected)
    return False


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
    # Native process tests exercise the real CLI against the daemon; this
    # client dependency is test-only and forbidden in runtime production.
    # Physical scratch tests open the actual adapter under the Runtime IO
    # binding. Production remains on the Core scratch port and Storage owner.
    # XFS pressure tests read the privileged verifier as an independent
    # quota witness. Production reaches it through Searchd's quota admission.
    "quanta-index-searchd-runtime": frozenset(
        {"quanta-index-searchctl", "quanta-index-scratch-sqlite", "quanta-state-root-quota"}
    ),
    "quanta-index-search-plane": frozenset({"quanta-index-lexical"}),
    "quanta-index-kernel": frozenset(
        {
            "quanta-index-catalog",
            "quanta-index-storage",
            "quanta-index-lexical",
            "quanta-index-repomap",
            "quanta-index-semantic",
        }
    ),
}

SHARED_CRATE_DEPS: dict[str, frozenset[str]] = {
    "quanta-explicit-fd-process": frozenset(),
    "quanta-installed-artifact-trust": frozenset({"quanta-explicit-fd-process"}),
    "quanta-sqlite-retained-grant": frozenset(),
    "quanta-state-root-quota": frozenset(),
}

ALLOWED_CRATE_DEPS: dict[str, frozenset[str]] = {
    **SHARED_CRATE_DEPS,
    "quanta-index-cbor": frozenset(),
    "quanta-index-unicode": frozenset(),
    # Standalone under crates/ until the SQLite integration is qualified.
    "quanta-index-sqlite-fd-ledger": frozenset(),
    # The standalone SQLite adapter shares only the FD ledger and retained grant.
    "quanta-index-sqlite-unix-syscall": frozenset(
        {"quanta-index-sqlite-fd-ledger", "quanta-sqlite-retained-grant"}
    ),
    "quanta-index-contract-base": frozenset({"quanta-index-unicode"}),
    # The contract re-exports the base DTOs it is split from.
    "quanta-index-contract": frozenset(
        {"quanta-index-contract-base", "quanta-index-lq-norm", "quanta-index-lq-bridge"}
    ),
    "quanta-index-lq-bridge": frozenset({"quanta-index-lq-norm"}),
    "quanta-index-core": frozenset({"quanta-index-contract"}),
    "quanta-index-kernel": _ADAPTER_CRATE_DEPS | frozenset({"quanta-index-scratch-sqlite"}),
    "quanta-index-storage": _ADAPTER_CRATE_DEPS | frozenset({"quanta-index-scratch-sqlite"}),
    # The lexical adapter also uses the lq-* index primitives and the shared
    # text normalizer (one Unicode contract for index, sidecars and query).
    "quanta-index-lexical": _ADAPTER_CRATE_DEPS
    | frozenset(
        {
            "quanta-index-scratch-sqlite",
            "quanta-index-lq-positions",
            "quanta-index-lq-regex",
            "quanta-index-lq-text-normalizer",
            "quanta-index-lq-trigram",
        }
    ),
    # Shared disposable SQLite membership implements retention ports; no
    # vendor handle or semantic authority crosses the Core boundary.
    "quanta-index-semantic": _ADAPTER_CRATE_DEPS | frozenset({"quanta-index-scratch-sqlite"}),
    # Embedding adapter: implements core outbound ports from contract DTOs.
    "quanta-index-embed": _ADAPTER_CRATE_DEPS,
    "quanta-index-ipc": _ADAPTER_CRATE_DEPS
    | frozenset({"quanta-index-cbor", "quanta-explicit-fd-process"}),
    # The search plane uses the transport crate for exactly its codec: the
    # CBOR payload encoding the authority snapshots persist in and the
    # canonical batch digest idempotency verifies (the wire encoding is the
    # transport's, and no vendor codec token leaves it). lq-obs is the
    # metric primitive library the plane's scrape is built from.
    "quanta-index-search-plane": frozenset(
        {
            "quanta-index-cbor",
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
    # Lexical normalization and identity decoding share the Unicode implementation.
    "quanta-index-lq-text-normalizer": frozenset({"quanta-index-unicode"}),
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
            "quanta-index-scratch-sqlite",
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
    # The Catalog owns the SQLite connection and its Linux protected FD reader.
    # The search plane reaches this adapter only through core ports.
    "quanta-index-catalog": frozenset(
        {
            "quanta-index-contract",
            "quanta-index-core",
            "quanta-index-ipc",
            "quanta-index-sqlite-unix-syscall",
        }
    ),
    # Shared SQLite scratch admission utility. Concrete adapters and the
    # kernel's existing spool use it; it depends inward only on core budget
    # and allocation contracts, never on a composition root or another adapter.
    # Retention membership uses canonical generation and error DTOs.
    "quanta-index-scratch-sqlite": _ADAPTER_CRATE_DEPS,
    # The composition root: the one crate that names concrete adapters,
    # the embedding provider and the structural matcher included. Structural
    # repo/file filters compile once per request through the shared regex
    # executor so their resource refusals match lexical and search-plane paths.
    "quanta-index-searchd": frozenset(
        {
            "quanta-state-root-quota",
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
    # Native product/provider construction and physical shutdown belong to this
    # composition root. Core, Kernel and search-plane still cannot name embed.
    "quanta-index-searchd-runtime": frozenset(
        {
            "quanta-explicit-fd-process",
            "quanta-installed-artifact-trust",
            "quanta-sqlite-retained-grant",
            "quanta-index-kernel",
            "quanta-index-storage",
            "quanta-index-catalog",
            "quanta-index-contract",
            "quanta-index-core",
            "quanta-index-embed",
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
    # product/composition crates must never depend back on this crate. Its
    # SDK edge uses the public Native UDS client to exercise the configured
    # daemon; the SDK remains independent of the harness and composition roots.
    # The lexical adapter is a direct dependency so the scale rail can time the sealed
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
            "quanta-index-sdk",
            "quanta-index-semantic",
        }
    ),
    "quanta-index-sdk": frozenset(
        {
            "quanta-index-cbor",
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


PRODUCTION_SECTIONS = ("dependencies", "build-dependencies")
DEV_SECTIONS = ("dev-dependencies",)


def dependency_packages(
    manifest: dict, workspace_dependencies: dict, sections: tuple[str, ...]
) -> set[str]:
    """Read Cargo package names, including aliases, inheritance and target tables."""
    deps: set[str] = set()
    for table in (manifest, *manifest.get("target", {}).values()):
        for section in sections:
            for key, spec in table.get(section, {}).items():
                if isinstance(spec, dict) and spec.get("workspace") is True:
                    spec = workspace_dependencies[key]
                package = spec.get("package", key) if isinstance(spec, dict) else key
                if not isinstance(package, str):
                    raise ValueError(f"non-string package name for {key!r}")
                deps.add(package)
    return deps


def check_crate_dependency_matrix() -> list[Violation]:
    violations: list[Violation] = []
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]
    manifests = (*CRATES.glob("*/Cargo.toml"), *(ROOT / "shared").glob("*/Cargo.toml"))
    for cargo_toml in sorted(manifests):
        manifest = tomllib.loads(cargo_toml.read_text(encoding="utf-8"))
        name = manifest.get("package", {}).get("name")
        if not isinstance(name, str):
            violations.append(Violation(cargo_toml, "missing package.name"))
            continue
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
                        f"unknown crate {name!r} (update ALLOWED_CRATE_DEPS)",
                    )
                )
                continue

        workspace_dependencies = manifest.get("workspace", workspace).get("dependencies", {})
        try:
            production = dependency_packages(manifest, workspace_dependencies, PRODUCTION_SECTIONS)
            development = dependency_packages(manifest, workspace_dependencies, DEV_SECTIONS)
        except (KeyError, ValueError) as error:
            violations.append(Violation(cargo_toml, f"invalid dependency declaration: {error}"))
            continue
        for dep in sorted(production):
            if (dep.startswith("quanta-index-") or dep in SHARED_CRATE_DEPS) and dep not in allowed:
                violations.append(
                    Violation(
                        cargo_toml,
                        f"{name} must not depend on {dep} (allowed: {sorted(allowed)})",
                    )
                )
        dev_allowed = allowed | TEST_SUPPORT_CRATES | DEV_INTEGRATION_DEPS.get(name, frozenset())
        for dep in sorted(development):
            if (
                (dep.startswith("quanta-index-") or dep in SHARED_CRATE_DEPS)
                and dep not in dev_allowed
                and dep != name
            ):
                violations.append(
                    Violation(
                        cargo_toml,
                        f"{name} must not dev-depend on {dep} (allowed: {sorted(dev_allowed)})",
                    )
                )

        if name == "quanta-index-core":
            for package in sorted(production & FORBIDDEN_VENDOR_DEPS):
                violations.append(
                    Violation(
                        cargo_toml,
                        f"quanta-index-core must not depend on vendor crate {package!r}",
                    )
                )

        if name in _ADAPTER_CRATES:
            peer_adapters = (production | development) & _ADAPTER_CRATES
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
        for name in (legacy, f"{legacy}.rs"):
            path = core_src / name
            if path.exists():
                violations.append(
                    Violation(path, f"legacy module {legacy!r} must not exist in core/src/")
                )
            # The current generation port is a Core domain file. The retired
            # generation service directory still cannot be restored here.
            if name == "generation.rs":
                continue
            legacy_domain = core_src / "domains" / name
            if legacy_domain.exists():
                violations.append(
                    Violation(
                        legacy_domain,
                        f"legacy domain {legacy!r} removed; new domains are {DOMAINS}",
                    )
                )
    return violations


def owning_domain(rel_path: Path) -> str | None:
    parts = rel_path.with_suffix("").parts
    if "domains" not in parts:
        return None
    idx = parts.index("domains")
    if idx + 1 >= len(parts):
        return None
    domain = parts[idx + 1]
    return domain if domain in DOMAINS else None


@lru_cache(maxsize=1)
def rust_path_helpers():
    # The module-cycle gate already masks Rust comments and normal/raw string
    # literals and expands grouped use paths for the standalone hook.
    helpers = run_path(str(Path(__file__).with_name("check-module-cycles.py")))
    return (
        helpers["blank_non_code"],
        helpers["referenced_paths_with_offsets"],
        helpers["module_path_of"],
        helpers["enclosing_inline_modules"],
    )


def check_domain_isolation() -> list[Violation]:
    violations: list[Violation] = []
    core_domains = CRATES / "quanta-index-core" / "src" / "domains"
    if not core_domains.is_dir():
        violations.append(
            Violation(core_domains, "missing domains/ directory in quanta-index-core")
        )
        return violations

    blank_non_code, referenced_paths_with_offsets, module_path_of, enclosing_inline_modules = (
        rust_path_helpers()
    )
    for rust_file in sorted(core_domains.rglob("*.rs")):
        owner = owning_domain(rust_file.relative_to(core_domains.parent))
        if owner is None:
            continue
        text = blank_non_code(rust_file.read_text(encoding="utf-8"))
        module_path = module_path_of(core_domains.parent, rust_file)
        module_parts = module_path.split("::") if module_path else []
        targets = {match.group("target") for match in DOMAIN_USE_RE.finditer(text)}
        paths = referenced_paths_with_offsets(text) if module_path is not None else ()
        for offset, path in paths:
            path = [part.strip() for part in path]
            if path[0] == "crate":
                resolved = path[1:]
            elif path[0] == "self":
                resolved = module_parts + path[1:]
            else:
                scoped_parts = module_parts + list(enclosing_inline_modules(text, offset))
                parents = 0
                while parents < len(path) and path[parents] == "super":
                    parents += 1
                if parents > len(scoped_parts):
                    continue
                resolved = scoped_parts[: len(scoped_parts) - parents] + path[parents:]
            if len(resolved) >= 2 and resolved[0] == "domains":
                targets.add(resolved[1])
        for target in sorted(targets & set(DOMAINS)):
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
    blank_non_code = rust_path_helpers()[0]
    for rust_file in sorted(contract_src.rglob("*.rs")):
        text = rust_file.read_text(encoding="utf-8")
        code = blank_non_code(text)
        allowed = CONTRACT_CONSTRUCTION_TRAITS.get(rust_file.relative_to(contract_src), {})
        forbidden = {
            match.group("name")
            for match in re.finditer(r"\bpub\s+trait\s+(?P<name>\w+)", code)
            if match.group("name") not in allowed
            or not approved_construction_trait(text, match.start(), allowed[match.group("name")])
        }
        if forbidden:
            violations.append(
                Violation(
                    rust_file,
                    "contract crate must not define port traits (use quanta-index-core): "
                    + ", ".join(sorted(forbidden)),
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
            if (
                "tests" in rust_file.relative_to(scope).parts
                or rust_file.resolve() in test_only_sources
            ):
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
