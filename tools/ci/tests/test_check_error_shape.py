"""Tests for tools/ci/lint/check-error-shape.py."""

from __future__ import annotations

import importlib.util
import sys
import textwrap
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools" / "ci" / "lint" / "check-error-shape.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("check_error_shape", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["check_error_shape"] = module
    spec.loader.exec_module(module)
    return module


MODULE = _load_module()


def test_checkout_under_tests_directory_still_checks_library_source(tmp_path, monkeypatch):
    root = tmp_path / "tests" / "checkout"
    src = root / "crates" / "demo" / "src"
    src.mkdir(parents=True)
    manifest = root / "Cargo.toml"
    manifest.write_text('[workspace]\nmembers = ["crates/demo"]\n')
    (src / "lib.rs").write_text("pub struct MissingError;\n")
    (src / "tests").mkdir()
    (src / "tests" / "helpers.rs").write_text("pub struct IgnoredError;\n")
    monkeypatch.setattr(MODULE, "ROOT", root)
    monkeypatch.setattr(MODULE, "WORKSPACE_TOML", manifest)

    assert MODULE.crate_source_files() == [src / "lib.rs"]
    assert MODULE.main() == 1


def test_linked_library_source_cannot_be_silently_skipped(tmp_path, monkeypatch):
    src = tmp_path / "crates" / "demo" / "src"
    src.mkdir(parents=True)
    manifest = tmp_path / "Cargo.toml"
    manifest.write_text('[workspace]\nmembers = ["crates/demo"]\n')
    (src / "lib.rs").write_text("mod linked;\n")
    linked = tmp_path / "external"
    linked.mkdir()
    (linked / "mod.rs").write_text("pub struct MissingError;\n")
    (src / "linked").symlink_to(linked, target_is_directory=True)
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)
    monkeypatch.setattr(MODULE, "WORKSPACE_TOML", manifest)

    assert MODULE.main() == 2


def write(tmp_path: Path, content: str) -> Path:
    p = tmp_path / "err.rs"
    p.write_text(textwrap.dedent(content), encoding="utf-8")
    return p


def test_well_formed_error_enum_passes(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #[derive(Debug, Error)]
        pub enum FooError {
            #[error("a")]
            A,
            #[error("b")]
            B(String),
        }
        """,
    )
    assert MODULE.audit_file(p) == []


def test_qualified_thiserror_derive_recognized(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #[derive(Debug, thiserror::Error)]
        pub enum BarError {
            #[error("a")]
            A,
        }
        """,
    )
    assert MODULE.audit_file(p) == []


def test_missing_derive_is_flagged(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #[derive(Debug)]
        pub enum NoDeriveError {
            A,
        }
        """,
    )
    findings = MODULE.audit_file(p)
    messages = [v.message for v in findings]
    assert any("coverage cannot be established" in m for m in messages)


def test_versioned_error_name_is_not_a_gate_escape(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #[derive(Debug)]
        pub enum VersionedValidationErrorV1 {
            Invalid,
        }
        """,
    )
    findings = MODULE.audit_file(p)
    assert len(findings) == 1
    assert "VersionedValidationErrorV1" in findings[0].message


def test_variant_missing_error_attr_is_flagged(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #[derive(Debug, Error)]
        pub enum HoleError {
            #[error("ok")]
            Ok,
            BareVariant,
        }
        """,
    )
    findings = MODULE.audit_file(p)
    assert len(findings) == 1
    assert "BareVariant" in findings[0].message


def test_struct_error_needs_derive(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #[derive(Debug, Error)]
        #[error("plain")]
        pub struct PlainError {
            message: String,
        }
        """,
    )
    assert MODULE.audit_file(p) == []


def test_non_error_enums_are_ignored(tmp_path: Path):
    p = write(
        tmp_path,
        """
        pub enum Mood {
            Happy,
            Sad,
        }
        """,
    )
    assert MODULE.audit_file(p) == []


def test_manual_error_impl_satisfies_requirement(tmp_path: Path):
    """Repo convention: manual impl Error + impl Display passes (no thiserror derive)."""
    p = write(
        tmp_path,
        """
        #[derive(Debug)]
        pub enum HandRolledError {
            One,
            Two(String),
        }

        impl core::fmt::Display for HandRolledError {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                match self {
                    Self::One => f.write_str("one"),
                    Self::Two(s) => write!(f, "two: {s}"),
                }
            }
        }

        impl std::error::Error for HandRolledError {}
        """,
    )
    assert MODULE.audit_file(p) == []


def test_generic_manual_error_impl_satisfies_requirement(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #[derive(Debug)]
        pub enum CheckpointError<E> {
            Index,
            Checkpoint(E),
        }

        impl<E: core::fmt::Display> core::fmt::Display for CheckpointError<E> {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("checkpoint")
            }
        }

        impl<E: core::error::Error> core::error::Error for CheckpointError<E> {}
        """,
    )
    assert MODULE.audit_file(p) == []


@pytest.mark.parametrize(
    "error_gate, display_gate",
    [("#[cfg(test)]", "#[cfg(test)]"), ("#[cfg(test)]", "")],
)
def test_test_only_manual_impls_do_not_prove_public_error_contract(
    tmp_path: Path, error_gate: str, display_gate: str
):
    p = write(
        tmp_path,
        f"""
        pub struct ConditionalError;
        {error_gate}
        impl std::error::Error for ConditionalError {{}}
        {display_gate}
        impl std::fmt::Display for ConditionalError {{
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{
                f.write_str("conditional")
            }}
        }}
        """,
    )
    assert any("ConditionalError" in item.message for item in MODULE.audit_file(p))


def test_matching_cfg_manual_impls_satisfy_conditional_error_contract(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #[cfg(feature = "extra")]
        // The declaration and its impls share the same gate.
        pub struct ConditionalError;
        #[cfg(feature="extra")]
        /* The comment must not detach cfg from this impl. */
        impl std::error::Error for ConditionalError {}
        #[cfg(feature = "extra")]
        impl std::fmt::Display for ConditionalError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("conditional")
            }
        }
        """,
    )
    assert MODULE.audit_file(p) == []


def test_feature_only_impl_does_not_prove_unconditional_error_contract(tmp_path: Path):
    p = write(
        tmp_path,
        """
        pub struct ConditionalError;
        #[cfg(feature = "extra")]
        impl std::error::Error for ConditionalError {}
        impl std::fmt::Display for ConditionalError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("conditional")
            }
        }
        """,
    )
    assert any("ConditionalError" in item.message for item in MODULE.audit_file(p))


def test_std_only_impl_reports_unproven_cfg_coverage(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #[derive(Debug)]
        pub struct StdOnlyError;
        #[cfg(feature = "std")]
        impl std::error::Error for StdOnlyError {}
        impl core::fmt::Display for StdOnlyError {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("std only")
            }
        }
        """,
    )
    findings = MODULE.audit_file(p)
    assert len(findings) == 1
    assert "StdOnlyError" in findings[0].message
    assert "coverage cannot be established" in findings[0].message


def test_direct_std_and_core_impls_cover_complementary_feature_gates(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #[derive(Debug)]
        pub struct SplitError;
        #[cfg(feature = "std")]
        impl std::error::Error for SplitError {}
        #[cfg(not(feature="std"))]
        impl core::error::Error for SplitError {}
        impl core::fmt::Display for SplitError {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("split")
            }
        }
        """,
    )
    assert MODULE.audit_file(p) == []


def test_cfg_guard_comparison_preserves_raw_string_contents(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #[cfg(feature = r#"one two"#)]
        pub struct RawError;
        #[cfg(feature = r#"onetwo"#)]
        impl std::error::Error for RawError {}
        #[cfg(feature = r#"one two"#)]
        impl std::fmt::Display for RawError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("raw")
            }
        }
        """,
    )
    assert any("RawError" in item.message for item in MODULE.audit_file(p))


def test_generic_default_manual_error_impl_still_passes(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #[derive(Debug)]
        pub struct DefaultedError<E = std::io::Error>(E);
        impl<E: std::fmt::Display> std::fmt::Display for DefaultedError<E> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
        impl<E: std::error::Error + 'static> std::error::Error for DefaultedError<E> {}
        """,
    )
    assert MODULE.audit_file(p) == []


@pytest.mark.parametrize(
    "target",
    ["SpecializedError<u8>", "SpecializedError<T, T>"],
)
def test_specialized_generic_impl_does_not_prove_whole_public_type(tmp_path: Path, target: str):
    declaration = "#[derive(Debug)]\npub struct SpecializedError<T, U = u8>(T, U);"
    type_parameters = "" if target.endswith("<u8>") else "<T>"
    p = write(
        tmp_path,
        f"""
        {declaration}
        impl{type_parameters} std::error::Error for {target} {{}}
        impl{type_parameters} std::fmt::Display for {target} {{
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{
                f.write_str("specialized")
            }}
        }}
        """,
    )
    assert any("SpecializedError" in item.message for item in MODULE.audit_file(p))


def test_cfg_and_derive_attributes_remain_attached_across_comments(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #[derive(Debug, Error)]
        // Explanation of the public error.
        pub enum CommentedError {
            #[error("ok")]
            // Explanation of the variant.
            Ok,
        }
        #[cfg(feature = "extra")]
        /* Explanation of the manual impl. */
        impl std::error::Error for CommentedError {}
        #[derive(Debug)]
        pub struct CommentedManualError;
        // This impl is available in production.
        impl std::error::Error for CommentedManualError {}
        /* This impl is available in production. */
        impl std::fmt::Display for CommentedManualError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("manual")
            }
        }
        """,
    )
    assert MODULE.audit_file(p) == []


def test_generic_error_impl_without_display_still_fails(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #[derive(Debug)]
        pub enum CheckpointError<E> { Checkpoint(E) }
        impl<E: core::error::Error> core::error::Error for CheckpointError<E> {}
        """,
    )
    assert len(MODULE.audit_file(p)) == 1


def test_manual_impl_skips_variant_attr_check(tmp_path: Path):
    """Manual impl Display covers the Display surface — per-variant #[error] not required."""
    p = write(
        tmp_path,
        """
        #[derive(Debug)]
        pub enum NoVariantAttrError {
            BareA,
            BareB,
        }

        impl core::fmt::Display for NoVariantAttrError {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("x")
            }
        }

        impl std::error::Error for NoVariantAttrError {}
        """,
    )
    assert MODULE.audit_file(p) == []


def test_missing_display_impl_still_fails(tmp_path: Path):
    """Manual impl Error WITHOUT impl Display does NOT count — both are required."""
    p = write(
        tmp_path,
        """
        #[derive(Debug)]
        pub enum HalfManualError {
            A,
        }

        impl std::error::Error for HalfManualError {}
        """,
    )
    findings = MODULE.audit_file(p)
    assert len(findings) == 1
    assert "coverage cannot be established" in findings[0].message


def test_commented_out_impls_do_not_satisfy_error_contract(tmp_path: Path):
    p = write(
        tmp_path,
        """
        pub enum ProbeError { A }
        // impl std::error::Error for ProbeError {}
        // impl std::fmt::Display for ProbeError {}
        """,
    )
    assert len(MODULE.audit_file(p)) == 1


def test_derive_attached_to_previous_item_does_not_transfer(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #[derive(Debug, Error)]
        pub enum PreviousError {
            #[error("x")]
            X,
        }
        pub enum ProbeError { A }
        """,
    )
    findings = MODULE.audit_file(p)
    assert len(findings) == 1
    assert "ProbeError" in findings[0].message


def test_impl_for_different_type_does_not_satisfy_error_contract(tmp_path: Path):
    p = write(
        tmp_path,
        """
        pub enum ProbeError { A }
        pub enum OtherError { B }
        impl std::error::Error for OtherError {}
        impl std::fmt::Display for OtherError {}
        """,
    )
    assert any("ProbeError" in item.message for item in MODULE.audit_file(p))


def test_wire_dto_allowlist_recognised():
    """SearchPlaneIpcError is a wire DTO and must be in the allowlist."""
    assert "SearchPlaneIpcError" in MODULE.WIRE_DTO_ERRORS


def test_scan_scope_keeps_first_party_members_and_excludes_vendor_members(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    manifest = tmp_path / "Cargo.toml"
    manifest.write_text(
        '[workspace]\nmembers = ["crates/local", "shared/owned", '
        '"benchmarks/owned", "vendor/external"]\n',
        encoding="utf-8",
    )
    first_party = tmp_path / "crates/local/src/lib.rs"
    shared = tmp_path / "shared/owned/src/lib.rs"
    benchmark = tmp_path / "benchmarks/owned/src/lib.rs"
    vendor = tmp_path / "vendor/external/src/lib.rs"
    for path in (first_party, shared, benchmark, vendor):
        path.parent.mkdir(parents=True)
        path.write_text("pub struct MissingError;\n", encoding="utf-8")
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)
    monkeypatch.setattr(MODULE, "WORKSPACE_TOML", manifest)

    scanned = MODULE.crate_source_files()
    assert first_party in scanned
    assert shared in scanned
    assert benchmark in scanned
    assert vendor not in scanned
    assert all(MODULE.audit_file(path) for path in scanned)


def test_repo_audit_passes():
    """Existing *Error types must already satisfy the shape rules."""
    violations: list = []
    for path in MODULE.crate_source_files():
        violations.extend(MODULE.audit_file(path))
    assert violations == [], "tree has error-shape violations:\n" + "\n".join(
        f"{v.path}:{v.line}: {v.message}" for v in violations
    )
