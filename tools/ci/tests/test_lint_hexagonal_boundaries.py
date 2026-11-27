from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
LINT = ROOT / "tools" / "ci" / "lint" / "lint-hexagonal-boundaries.py"


def test_hexagonal_boundary_lint_passes_on_repo() -> None:
    completed = subprocess.run(
        [sys.executable, str(LINT)],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    assert completed.returncode == 0, completed.stderr or completed.stdout


def _load_lint():
    import importlib.util

    spec = importlib.util.spec_from_file_location("lint_hexagonal_boundaries", LINT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    # The lint's dataclasses resolve their module through `sys.modules`.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture
def matrix_lint(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    lint = _load_lint()
    (tmp_path / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
    monkeypatch.setattr(lint, "ROOT", tmp_path)
    monkeypatch.setattr(lint, "CRATES", tmp_path / "crates")
    return lint


def test_native_dto_construction_protocol_does_not_admit_service_ports(
    tmp_path: Path, monkeypatch
) -> None:
    lint = _load_lint()
    crates = tmp_path / "crates"
    contract = crates / "quanta-index-contract" / "src"
    native = contract / "ipc" / "control" / "native_decode_v1.rs"
    native.parent.mkdir(parents=True)
    native.write_text(
        (ROOT / "crates/quanta-index-contract/src/ipc/control/native_decode_v1.rs").read_text()
    )
    monkeypatch.setattr(lint, "CRATES", crates)
    assert lint.check_contract_is_dto_only() == []

    native.write_text(native.read_text() + "\npub trait StoragePort {}\n")
    violations = lint.check_contract_is_dto_only()
    assert len(violations) == 1
    assert violations[0].path == native
    assert "StoragePort" in violations[0].message

    native.write_text(
        native.read_text()
        .replace("\npub trait StoragePort {}\n", "")
        .replace(
            "    fn consume_corpus_work_v1(&mut self, units: u64) -> Result<(), Self::OriginalError>;",
            "    fn consume_corpus_work_v1(&mut self, units: u64) -> Result<(), Self::OriginalError>;\n"
            "    fn storage_port(&self);",
            1,
        )
    )
    violations = lint.check_contract_is_dto_only()
    assert len(violations) == 1
    assert violations[0].path == native
    assert "NativeCorpusDecodeAdmissionV1" in violations[0].message

    other = contract / "storage.rs"
    other.write_text("pub trait NativeCorpusDecodeAdmissionV1 {}\n")
    violations = lint.check_contract_is_dto_only()
    assert {violation.path for violation in violations} == {native, other}
    assert "NativeCorpusDecodeAdmissionV1" in next(
        violation.message for violation in violations if violation.path == other
    )


def test_contract_trait_gate_ignores_doc_examples_and_string_literals(
    tmp_path: Path, monkeypatch
) -> None:
    lint = _load_lint()
    contract = tmp_path / "crates" / "quanta-index-contract" / "src"
    source = contract / "lib.rs"
    source.parent.mkdir(parents=True)
    source.write_text(
        "/// Example: `pub trait ServicePort {}`\n"
        "/* pub trait StoragePort {} */\n"
        'const EXAMPLE: &str = "pub trait TransportPort {}";\n'
    )
    monkeypatch.setattr(lint, "CRATES", tmp_path / "crates")

    assert lint.check_contract_is_dto_only() == []


def test_contract_trait_gate_detects_comment_between_pub_and_trait(
    tmp_path: Path, monkeypatch
) -> None:
    lint = _load_lint()
    contract = tmp_path / "crates" / "quanta-index-contract" / "src"
    source = contract / "lib.rs"
    source.parent.mkdir(parents=True)
    source.write_text("pub /* checked by compiler */ trait StoragePort {}\n")
    monkeypatch.setattr(lint, "CRATES", tmp_path / "crates")

    violations = lint.check_contract_is_dto_only()
    assert [(v.path, v.message) for v in violations] == [
        (
            source,
            "contract crate must not define port traits (use quanta-index-core): StoragePort",
        )
    ]


def test_domain_isolation_detects_code_without_matching_comments_or_strings(
    tmp_path: Path, monkeypatch
) -> None:
    lint = _load_lint()
    crates = tmp_path / "crates"
    domains = crates / "quanta-index-core" / "src" / "domains"
    lexical = domains / "lexical" / "service.rs"
    lexical.parent.mkdir(parents=True)
    hybrid = domains / "hybrid" / "service.rs"
    hybrid.parent.mkdir(parents=True)
    hybrid.write_text("use crate::domains::semantic::SemanticPolicy;\n")
    harmless = (
        "// crate::domains::semantic::SemanticPolicy\n"
        "/* domains::semantic::SemanticPolicy */\n"
        'const NORMAL: &str = "crate::domains::semantic::SemanticPolicy";\n'
        'const RAW: &str = r##"domains::semantic::SemanticPolicy"##;\n'
    )
    lexical.write_text(harmless + "use crate::domains::semantic::SemanticPolicy;\n")
    monkeypatch.setattr(lint, "CRATES", crates)

    violations = lint.check_domain_isolation()
    assert [(violation.path, violation.message) for violation in violations] == [
        (lexical, "domain 'lexical' must not reference domain 'semantic'")
    ]

    lexical.write_text(harmless)
    assert lint.check_domain_isolation() == []


@pytest.mark.parametrize(
    ("relative", "source"),
    [
        ("lexical/service.rs", "use crate::domains::{semantic :: SemanticPolicy};\n"),
        ("lexical/service.rs", "use super::super::semantic::SemanticPolicy;\n"),
        ("lexical/mod.rs", "use super::semantic::SemanticPolicy;\n"),
        ("lexical.rs", "use super::semantic::SemanticPolicy;\n"),
    ],
)
def test_domain_isolation_resolves_grouped_and_parent_imports(
    tmp_path: Path, monkeypatch, relative: str, source: str
) -> None:
    lint = _load_lint()
    domains = tmp_path / "crates" / "quanta-index-core" / "src" / "domains"
    lexical = domains / relative
    lexical.parent.mkdir(parents=True)
    lexical.write_text(source)
    monkeypatch.setattr(lint, "CRATES", tmp_path / "crates")

    violations = lint.check_domain_isolation()
    assert [(v.path, v.message) for v in violations] == [
        (lexical, "domain 'lexical' must not reference domain 'semantic'")
    ]


@pytest.mark.parametrize(
    ("source", "expected"),
    [
        ("mod semantic { use super::semantic::Policy; }\n", []),
        (
            "mod nested { use super::super::semantic::Policy; }\n",
            ["domain 'lexical' must not reference domain 'semantic'"],
        ),
    ],
)
def test_domain_isolation_resolves_super_inside_inline_modules(
    tmp_path: Path, monkeypatch, source: str, expected: list[str]
) -> None:
    lint = _load_lint()
    domains = tmp_path / "crates" / "quanta-index-core" / "src" / "domains"
    lexical = domains / "lexical" / "mod.rs"
    lexical.parent.mkdir(parents=True)
    lexical.write_text(source)
    monkeypatch.setattr(lint, "CRATES", tmp_path / "crates")

    violations = lint.check_domain_isolation()
    assert [(v.path, v.message) for v in violations] == [(lexical, message) for message in expected]


def test_domain_isolation_keeps_direct_scan_without_module_path(
    tmp_path: Path, monkeypatch
) -> None:
    lint = _load_lint()
    domains = tmp_path / "crates" / "quanta-index-core" / "src" / "domains"
    lexical = domains / "lexical" / "service.rs"
    lexical.parent.mkdir(parents=True)
    lexical.write_text("use crate::domains::semantic::SemanticPolicy;\n")
    monkeypatch.setattr(lint, "CRATES", tmp_path / "crates")
    blank_non_code, referenced_paths, _, enclosing_inline_modules = lint.rust_path_helpers()
    monkeypatch.setattr(
        lint,
        "rust_path_helpers",
        lambda: (
            blank_non_code,
            referenced_paths,
            lambda _root, _file: None,
            enclosing_inline_modules,
        ),
    )

    violations = lint.check_domain_isolation()
    assert [(v.path, v.message) for v in violations] == [
        (lexical, "domain 'lexical' must not reference domain 'semantic'")
    ]


@pytest.mark.parametrize("location", ["query.rs", "query/mod.rs", "domains/query.rs"])
def test_core_layout_rejects_retired_module_in_either_rust_layout(
    tmp_path: Path, monkeypatch, location: str
) -> None:
    lint = _load_lint()
    core = tmp_path / "crates" / "quanta-index-core" / "src"
    retired = core / location
    retired.parent.mkdir(parents=True)
    retired.write_text("pub struct Legacy;\n")
    monkeypatch.setattr(lint, "CRATES", tmp_path / "crates")

    violations = lint.check_legacy_core_layout()
    assert len(violations) == 1
    assert violations[0].path == (retired.parent if retired.name == "mod.rs" else retired)


def test_core_layout_keeps_the_current_generation_domain_port(tmp_path: Path, monkeypatch) -> None:
    lint = _load_lint()
    domains = tmp_path / "crates" / "quanta-index-core" / "src" / "domains"
    domains.mkdir(parents=True)
    (domains / "generation.rs").write_text("pub trait GenerationNamespaceReadPort {}\n")
    monkeypatch.setattr(lint, "CRATES", tmp_path / "crates")

    assert lint.check_legacy_core_layout() == []


def test_dependencies_are_named_as_cargo_names_them() -> None:
    """The table key (or `package`) is the name, never the path's basename.

    Deriving the name from the path, underscored, once made every internal
    dependency invisible to the allowlist check.
    """
    lint = _load_lint()
    manifest = lint.tomllib.loads(
        """
[package]
name = "quanta-index-example"

[dependencies]
quanta-index-core = { version = "0.1.0", path = "../quanta-index-core" }
alias = { package = "quanta-index-contract", version = "0.1.0", path = "../somewhere-else" }
codec = { workspace = true }

[target.'cfg(unix)'.build-dependencies]
quanta-index-storage = "0.1.0"

[target.'cfg(unix)'.dev-dependencies]
quanta-index-searchd-harness = { version = "0.1.0", path = "../quanta-index-searchd-harness" }
"""
    )
    workspace = {"codec": {"package": "quanta-index-cbor", "path": "crates/quanta-index-cbor"}}
    assert lint.dependency_packages(manifest, workspace, lint.PRODUCTION_SECTIONS) == {
        "quanta-index-core",
        "quanta-index-contract",
        "quanta-index-cbor",
        "quanta-index-storage",
    }
    assert lint.dependency_packages(manifest, workspace, lint.DEV_SECTIONS) == {
        "quanta-index-searchd-harness",
    }


def test_codec_and_unicode_are_leaf_dependencies(tmp_path: Path, matrix_lint) -> None:
    crates = tmp_path / "crates"

    def manifest(name: str, dependencies: str = "") -> Path:
        folder = crates / name
        folder.mkdir()
        path = folder / "Cargo.toml"
        path.write_text(f'[package]\nname = "{name}"\n{dependencies}', encoding="utf-8")
        return path

    crates.mkdir()
    codec = manifest("quanta-index-cbor")
    manifest("quanta-index-unicode")
    manifest("quanta-index-sqlite-fd-ledger", "\n[workspace]\n")
    manifest("quanta-index-sdk", '\n[dependencies]\nquanta-index-cbor = "0.1.0"\n')
    manifest("quanta-index-contract-base", '\n[dependencies]\nquanta-index-unicode = "0.1.0"\n')
    assert matrix_lint.check_crate_dependency_matrix() == []

    codec.write_text(
        codec.read_text()
        + "\n[target.'cfg(unix)'.dependencies]\n"
        + 'service = { package = "quanta-index-ipc", path = "../quanta-index-ipc" }\n',
        encoding="utf-8",
    )
    violations = matrix_lint.check_crate_dependency_matrix()
    assert [(violation.path, violation.message) for violation in violations] == [
        (codec, "quanta-index-cbor must not depend on quanta-index-ipc (allowed: [])")
    ]


def test_sqlite_unix_syscall_only_depends_on_fd_ledger(tmp_path: Path, monkeypatch) -> None:
    lint = _load_lint()
    crates = tmp_path / "crates"
    for name in ("quanta-index-sqlite-fd-ledger", "quanta-index-sqlite-unix-syscall"):
        manifest = crates / name / "Cargo.toml"
        manifest.parent.mkdir(parents=True)
        manifest.write_bytes((ROOT / "crates" / name / "Cargo.toml").read_bytes())
    monkeypatch.setattr(lint, "CRATES", crates)

    assert lint.check_crate_dependency_matrix() == []

    ledger = crates / "quanta-index-sqlite-fd-ledger" / "Cargo.toml"
    ledger.write_text(
        ledger.read_text()
        + '\n[dependencies]\nquanta-index-sqlite-unix-syscall = { path = "../quanta-index-sqlite-unix-syscall" }\n'
    )
    reverse = [violation.message for violation in lint.check_crate_dependency_matrix()]
    assert len(reverse) == 1
    assert (
        "quanta-index-sqlite-fd-ledger must not depend on quanta-index-sqlite-unix-syscall"
        in reverse[0]
    )

    ledger.write_bytes((ROOT / "crates/quanta-index-sqlite-fd-ledger/Cargo.toml").read_bytes())
    core = crates / "quanta-index-core" / "Cargo.toml"
    core.parent.mkdir(parents=True)
    core.write_text(
        '[package]\nname = "quanta-index-core"\n[dependencies]\n'
        'quanta-index-sqlite-unix-syscall = { path = "../quanta-index-sqlite-unix-syscall" }\n'
    )
    product = [violation.message for violation in lint.check_crate_dependency_matrix()]
    assert len(product) == 1
    assert "quanta-index-core must not depend on quanta-index-sqlite-unix-syscall" in product[0]


def test_an_unlisted_internal_dependency_is_a_violation(tmp_path: Path, matrix_lint) -> None:
    """A production edge outside the allowlist fails; a test-support crate
    is admitted as a dev-dependency only."""
    crates = tmp_path / "crates"
    core = crates / "quanta-index-core"
    core.mkdir(parents=True)
    (core / "Cargo.toml").write_text(
        """
[package]
name = "quanta-index-core"

[dependencies]
quanta-index-contract = { version = "0.1.0", path = "../quanta-index-contract" }
quanta-index-lexical = { version = "0.1.0", path = "../quanta-index-lexical" }

[dev-dependencies]
quanta-index-corpus-smoke = { version = "0.1.0", path = "../quanta-index-corpus-smoke" }
""",
        encoding="utf-8",
    )
    messages = [violation.message for violation in matrix_lint.check_crate_dependency_matrix()]
    assert messages == [
        "quanta-index-core must not depend on quanta-index-lexical "
        "(allowed: ['quanta-index-contract'])"
    ]


def test_shared_crates_respect_the_same_dependency_direction(tmp_path: Path, monkeypatch) -> None:
    lint = _load_lint()
    crates = tmp_path / "crates"
    shared = tmp_path / "shared"

    def add_manifest(directory: Path, name: str, suffix: str = "") -> Path:
        manifest = directory / name / "Cargo.toml"
        manifest.parent.mkdir(parents=True, exist_ok=True)
        manifest.write_text(f'[package]\nname = "{name}"\n{suffix}', encoding="utf-8")
        return manifest

    (tmp_path / "Cargo.toml").write_text(
        "[workspace.dependencies]\n"
        'retained = { package = "quanta-sqlite-retained-grant", path = "shared/quanta-sqlite-retained-grant" }\n',
        encoding="utf-8",
    )
    grant = add_manifest(shared, "quanta-sqlite-retained-grant")
    grant_dep = "\n[target.'cfg(unix)'.dependencies]\nretained.workspace = true\n"
    add_manifest(crates, "quanta-index-searchd-runtime", grant_dep)
    monkeypatch.setattr(lint, "ROOT", tmp_path)
    monkeypatch.setattr(lint, "CRATES", crates)

    assert lint.check_crate_dependency_matrix() == []

    core = add_manifest(crates, "quanta-index-core", grant_dep)
    with grant.open("a", encoding="utf-8") as handle:
        handle.write(
            '\n[dependencies]\nquanta-index-sdk = { path = "../../crates/quanta-index-sdk" }\n'
        )

    violations = lint.check_crate_dependency_matrix()
    assert {violation.path for violation in violations} == {core, grant}
    assert any(
        "quanta-index-core must not depend on quanta-sqlite-retained-grant" in v.message
        for v in violations
    )
    assert any(
        "quanta-sqlite-retained-grant must not depend on quanta-index-sdk" in v.message
        for v in violations
    )


def test_search_plane_real_adapter_is_test_only(tmp_path: Path, matrix_lint) -> None:
    plane = tmp_path / "crates" / "quanta-index-search-plane"
    plane.mkdir(parents=True)
    manifest = plane / "Cargo.toml"
    manifest.write_text(
        '[package]\nname = "quanta-index-search-plane"\n'
        '[dev-dependencies]\nquanta-index-lexical = { path = "../quanta-index-lexical" }\n',
        encoding="utf-8",
    )
    assert matrix_lint.check_crate_dependency_matrix() == []

    manifest.write_text(
        '[package]\nname = "quanta-index-search-plane"\n'
        '[dependencies]\nquanta-index-lexical = { path = "../quanta-index-lexical" }\n',
        encoding="utf-8",
    )
    messages = [violation.message for violation in matrix_lint.check_crate_dependency_matrix()]
    assert len(messages) == 1
    assert "must not depend on quanta-index-lexical" in messages[0]


def test_native_product_cli_and_scratch_composition_are_test_only(
    tmp_path: Path, matrix_lint
) -> None:
    runtime = tmp_path / "crates" / "quanta-index-searchd-runtime"
    runtime.mkdir(parents=True)
    manifest = runtime / "Cargo.toml"
    package = '[package]\nname = "quanta-index-searchd-runtime"\n'
    for dependency in ("quanta-index-searchctl", "quanta-index-scratch-sqlite"):
        edge = f'{dependency} = {{ path = "../{dependency}" }}\n'
        manifest.write_text(package + "[dev-dependencies]\n" + edge, encoding="utf-8")
        assert matrix_lint.check_crate_dependency_matrix() == []

        manifest.write_text(package + "[dependencies]\n" + edge, encoding="utf-8")
        messages = [violation.message for violation in matrix_lint.check_crate_dependency_matrix()]
        assert len(messages) == 1
        assert f"must not depend on {dependency}" in messages[0]


def test_native_harness_may_use_sdk_but_product_and_sdk_cannot_depend_back(
    tmp_path: Path, matrix_lint
) -> None:
    crates = tmp_path / "crates"

    def manifest(name: str, dependency: str, section: str = "dependencies") -> None:
        crate = crates / name
        crate.mkdir(parents=True, exist_ok=True)
        (crate / "Cargo.toml").write_text(
            f'[package]\nname = "{name}"\n'
            f'[{section}]\n{dependency} = {{ path = "../{dependency}" }}\n',
            encoding="utf-8",
        )

    harness = "quanta-index-searchd-harness"
    sdk = "quanta-index-sdk"
    runtime = "quanta-index-searchd-runtime"
    manifest(harness, sdk)
    manifest(runtime, harness, "dev-dependencies")
    assert matrix_lint.check_crate_dependency_matrix() == []

    manifest(runtime, harness)
    violations = matrix_lint.check_crate_dependency_matrix()
    assert len(violations) == 1
    assert violations[0].path.parent.name == runtime
    assert f"must not depend on {harness}" in violations[0].message

    manifest(runtime, harness, "dev-dependencies")
    manifest(sdk, harness)
    violations = matrix_lint.check_crate_dependency_matrix()
    assert len(violations) == 1
    assert violations[0].path.parent.name == sdk
    assert f"must not depend on {harness}" in violations[0].message


def test_native_embedding_provider_edge_is_composition_only(tmp_path: Path, matrix_lint) -> None:
    crates = tmp_path / "crates"
    names = (
        "quanta-index-searchd-runtime",
        "quanta-index-core",
        "quanta-index-kernel",
        "quanta-index-search-plane",
    )
    for name in names:
        crate = crates / name
        crate.mkdir(parents=True)
        (crate / "Cargo.toml").write_text(
            f'[package]\nname = "{name}"\n'
            '[dependencies]\nquanta-index-embed = { path = "../quanta-index-embed" }\n',
            encoding="utf-8",
        )
    violations = matrix_lint.check_crate_dependency_matrix()
    assert {violation.path.parent.name for violation in violations} == set(names[1:])
    assert all(
        "must not depend on quanta-index-embed" in violation.message for violation in violations
    )


def test_cfg_test_path_module_is_not_a_production_transport_leak(tmp_path: Path) -> None:
    lint = _load_lint()
    source = tmp_path / "owner.rs"
    tests = tmp_path / "owner_tests.rs"
    production = tmp_path / "production.rs"
    source.write_text('#[cfg(test)]\n#[path = "owner_tests.rs"]\nmod tests;\n')
    tests.write_text("fn uses_engine_segment_id() {}\n")
    production.write_text("fn leaks_segment_id() {}\n")
    assert lint.test_only_path_modules(tmp_path) == {tests.resolve()}
    assert production.resolve() not in lint.test_only_path_modules(tmp_path)


def test_transport_scope_is_relative_to_its_crate(tmp_path: Path, monkeypatch) -> None:
    lint = _load_lint()
    crates = tmp_path / "tests" / "checkout" / "crates"
    core = crates / "quanta-index-core" / "src"
    production = core / "ingest.rs"
    test_source = core / "tests" / "ingest.rs"
    test_source.parent.mkdir(parents=True)
    production.write_text("fn ingest() { let segment_id = 1; }\n")
    test_source.write_text(production.read_text())
    monkeypatch.setattr(lint, "CRATES", crates)

    assert [v.path for v in lint.check_channel_backend_isolation()] == [production]


def test_tantivy_segment_reader_call_does_not_hide_other_transport_leaks(
    tmp_path: Path, monkeypatch
) -> None:
    lint = _load_lint()
    crates = tmp_path / "crates"
    lexical = crates / "quanta-index-lexical" / "src"
    searchd = crates / "quanta-index-searchd" / "src"
    lexical.mkdir(parents=True)
    searchd.mkdir(parents=True)
    ranked_keys = lexical / "ranked_keys.rs"
    ranked_keys.write_text("fn id(reader: &SegmentReader) { reader.segment_id(); }\n")
    native = lexical / "live_statistics.rs"
    native.write_text(
        "fn id(reader: &tantivy::SegmentReader) { tantivy::SegmentReader::segment_id(reader); }\n"
    )
    other = searchd / "ingest.rs"
    other.write_text("fn ingest() { let segment_id = 1; }\n")
    monkeypatch.setattr(lint, "CRATES", crates)

    violations = lint.check_channel_backend_isolation()
    assert [(v.path, v.message) for v in violations] == [
        (other, "transport-specific token 'segment_id' leaked outside channel backend")
    ]

    ranked_keys.write_text(
        "fn id(reader: &SegmentReader) { reader.segment_id(); let segment_id = 1; }\n"
    )
    violations = lint.check_channel_backend_isolation()
    assert [v.path for v in violations] == [ranked_keys, other]

    native.write_text(
        "fn id(reader: &tantivy::SegmentReader) { "
        "tantivy::SegmentReader::segment_id(reader); "
        "let segment_id = 1; }\n"
    )
    assert {v.path for v in lint.check_channel_backend_isolation()} == {ranked_keys, native, other}

    native.write_text(
        "fn id(reader: &channel::SegmentReader) { channel::SegmentReader::segment_id(reader); }\n"
    )
    assert native in {v.path for v in lint.check_channel_backend_isolation()}

    core = crates / "quanta-index-core" / "src"
    core.mkdir(parents=True)
    core_source = core / "storage.rs"
    core_source.write_text(
        "fn id(reader: &tantivy::SegmentReader) { tantivy::SegmentReader::segment_id(reader); }\n"
    )
    assert core_source in {v.path for v in lint.check_channel_backend_isolation()}


def test_core_vendor_alias_cannot_bypass_dependency_boundary(tmp_path: Path, matrix_lint) -> None:
    core = tmp_path / "crates" / "quanta-index-core"
    core.mkdir(parents=True)
    (core / "Cargo.toml").write_text(
        """
[package]
name = "quanta-index-core"

[target.'cfg(unix)'.build-dependencies]
storage = { package = "rusqlite", version = "0.40" }
""",
        encoding="utf-8",
    )
    messages = [violation.message for violation in matrix_lint.check_crate_dependency_matrix()]
    assert messages == ["quanta-index-core must not depend on vendor crate 'rusqlite'"]
