from __future__ import annotations

import subprocess
import sys
from pathlib import Path

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


def test_path_dependencies_are_named_as_cargo_names_them(tmp_path: Path) -> None:
    """The table key (or `package`) is the name, never the path's basename.

    Deriving the name from the path, underscored, once made every internal
    dependency invisible to the allowlist check.
    """
    lint = _load_lint()
    cargo_toml = tmp_path / "Cargo.toml"
    cargo_toml.write_text(
        """
[package]
name = "quanta-index-example"

[dependencies]
quanta-index-core = { version = "0.1.0", path = "../quanta-index-core" }
alias = { package = "quanta-index-contract", version = "0.1.0", path = "../somewhere-else" }
serde = { workspace = true }

[dev-dependencies]
quanta-index-searchd-harness = { version = "0.1.0", path = "../quanta-index-searchd-harness" }
""",
        encoding="utf-8",
    )
    assert lint.path_dependencies(cargo_toml, lint.PRODUCTION_SECTIONS) == {
        "quanta-index-core",
        "quanta-index-contract",
    }
    assert lint.path_dependencies(cargo_toml, lint.DEV_SECTIONS) == {
        "quanta-index-searchd-harness",
    }


def test_an_unlisted_internal_dependency_is_a_violation(tmp_path: Path, monkeypatch) -> None:
    """A production edge outside the allowlist fails; a test-support crate
    is admitted as a dev-dependency only."""
    lint = _load_lint()
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
    monkeypatch.setattr(lint, "CRATES", crates)
    messages = [violation.message for violation in lint.check_crate_dependency_matrix()]
    assert messages == [
        "quanta-index-core must not depend on quanta-index-lexical "
        "(allowed: ['quanta-index-contract'])"
    ]


def test_search_plane_real_adapter_is_test_only(tmp_path: Path, monkeypatch) -> None:
    lint = _load_lint()
    plane = tmp_path / "crates" / "quanta-index-search-plane"
    plane.mkdir(parents=True)
    manifest = plane / "Cargo.toml"
    manifest.write_text(
        '[package]\nname = "quanta-index-search-plane"\n'
        '[dev-dependencies]\nquanta-index-lexical = { path = "../quanta-index-lexical" }\n',
        encoding="utf-8",
    )
    monkeypatch.setattr(lint, "CRATES", tmp_path / "crates")
    assert lint.check_crate_dependency_matrix() == []

    manifest.write_text(
        '[package]\nname = "quanta-index-search-plane"\n'
        '[dependencies]\nquanta-index-lexical = { path = "../quanta-index-lexical" }\n',
        encoding="utf-8",
    )
    messages = [violation.message for violation in lint.check_crate_dependency_matrix()]
    assert len(messages) == 1
    assert "must not depend on quanta-index-lexical" in messages[0]


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


def test_core_vendor_alias_cannot_bypass_dependency_boundary(tmp_path: Path, monkeypatch) -> None:
    lint = _load_lint()
    core = tmp_path / "crates" / "quanta-index-core"
    core.mkdir(parents=True)
    (core / "Cargo.toml").write_text(
        """
[package]
name = "quanta-index-core"

[dependencies]
storage = { package = "rusqlite", version = "0.40" }
""",
        encoding="utf-8",
    )
    monkeypatch.setattr(lint, "CRATES", tmp_path / "crates")
    messages = [violation.message for violation in lint.check_crate_dependency_matrix()]
    assert messages == ["quanta-index-core must not depend on vendor crate 'rusqlite'"]
