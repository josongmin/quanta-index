"""Tests for tools/ci/lint/check-digest-fallibility.py."""

from __future__ import annotations

import importlib.util
import sys
import textwrap
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools" / "ci" / "lint" / "check-digest-fallibility.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("check_digest_fallibility", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["check_digest_fallibility"] = module
    spec.loader.exec_module(module)
    return module


MODULE = _load_module()


def write(tmp_path: Path, content: str, name: str = "fn.rs") -> Path:
    p = tmp_path / name
    p.write_text(textwrap.dedent(content), encoding="utf-8")
    return p


def test_bare_digest_return_is_flagged(tmp_path: Path):
    p = write(
        tmp_path,
        """
        pub fn weights_hash() -> [u8; 32] {
            [0u8; 32]
        }
        """,
    )
    _, violations = MODULE.audit_file(p)
    assert len(violations) == 1
    assert "[u8; 32]" in violations[0].message
    assert "Result<[u8; 32], _>" in violations[0].message
    assert "infallible by construction" in violations[0].message


def test_result_wrapped_passes(tmp_path: Path):
    p = write(
        tmp_path,
        """
        pub fn weights_hash() -> Result<[u8; 32], MyErr> {
            Ok([0u8; 32])
        }
        """,
    )
    sites, violations = MODULE.audit_file(p)
    assert violations == []
    assert len(sites) == 1  # still counted as a digest-return site


def test_result_wrapped_with_path_qualified_err_passes(tmp_path: Path):
    p = write(
        tmp_path,
        """
        pub fn weights_hash() -> Result<[u8; 32], crate::errors::RankerError> {
            Ok([0u8; 32])
        }
        """,
    )
    _, violations = MODULE.audit_file(p)
    assert violations == []


def test_infallible_doc_escape_hatch_passes(tmp_path: Path):
    p = write(
        tmp_path,
        """
        /// Returns the all-zero digest.
        ///
        /// infallible by construction — no codec step can fail.
        #[must_use]
        pub fn zero_digest() -> [u8; 32] {
            [0u8; 32]
        }
        """,
    )
    _, violations = MODULE.audit_file(p)
    assert violations == []


def test_infallible_doc_is_case_insensitive(tmp_path: Path):
    p = write(
        tmp_path,
        """
        /// Returns the all-zero digest.
        /// Infallible By Construction.
        pub fn zero_digest() -> [u8; 16] {
            [0u8; 16]
        }
        """,
    )
    _, violations = MODULE.audit_file(p)
    assert violations == []


def test_non_digest_size_is_not_flagged(tmp_path: Path):
    """N=17 is not one of {16, 20, 32, 48, 64} → not a digest, not flagged."""
    p = write(
        tmp_path,
        """
        pub fn weird() -> [u8; 17] {
            [0u8; 17]
        }
        """,
    )
    sites, violations = MODULE.audit_file(p)
    assert sites == []
    assert violations == []


def test_all_five_digest_sizes_are_flagged(tmp_path: Path):
    for n in (16, 20, 32, 48, 64):
        p = write(
            tmp_path,
            f"""
            pub fn h() -> [u8; {n}] {{
                [0u8; {n}]
            }}
            """,
            name=f"d{n}.rs",
        )
        _, violations = MODULE.audit_file(p)
        assert len(violations) == 1, f"size {n} should be flagged"
        assert f"[u8; {n}]" in violations[0].message


def test_literal_prefilter_keeps_whitespace_tolerant_digest(tmp_path: Path):
    p = write(
        tmp_path,
        """
        pub fn h() -> [ u8 ; 32 ] {
            [0u8; 32]
        }
        """,
    )
    sites, violations = MODULE.audit_file(p)
    assert len(sites) == 1
    assert len(violations) == 1


def test_multiline_signature_handled(tmp_path: Path):
    p = write(
        tmp_path,
        """
        pub fn
            weights_hash(
                weights: &RankerWeights,
            )
            -> [u8; 32]
        {
            [0u8; 32]
        }
        """,
    )
    _, violations = MODULE.audit_file(p)
    assert len(violations) == 1


def test_multiline_signature_with_result_passes(tmp_path: Path):
    p = write(
        tmp_path,
        """
        pub fn weights_hash(
            weights: &RankerWeights,
        ) -> Result<
            [u8; 32],
            RankerError,
        > {
            Ok([0u8; 32])
        }
        """,
    )
    _, violations = MODULE.audit_file(p)
    assert violations == []


def test_pub_crate_is_treated_as_pub(tmp_path: Path):
    p = write(
        tmp_path,
        """
        pub(crate) fn h() -> [u8; 32] {
            [0u8; 32]
        }
        """,
    )
    _, violations = MODULE.audit_file(p)
    assert len(violations) == 1


def test_pub_super_is_treated_as_pub(tmp_path: Path):
    p = write(
        tmp_path,
        """
        pub(super) fn h() -> [u8; 20] {
            [0u8; 20]
        }
        """,
    )
    _, violations = MODULE.audit_file(p)
    assert len(violations) == 1


def test_private_fn_is_not_flagged(tmp_path: Path):
    """Private (non-`pub`) functions are out of scope — caller surface is local."""
    p = write(
        tmp_path,
        """
        fn h() -> [u8; 32] {
            [0u8; 32]
        }
        """,
    )
    sites, violations = MODULE.audit_file(p)
    assert sites == []
    assert violations == []


def test_test_fn_excluded(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #[test]
        pub fn returns_digest() -> [u8; 32] {
            [0u8; 32]
        }
        """,
    )
    _, violations = MODULE.audit_file(p)
    assert violations == []


def test_cfg_test_fn_excluded(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #[cfg(test)]
        pub fn returns_digest() -> [u8; 32] {
            [0u8; 32]
        }
        """,
    )
    _, violations = MODULE.audit_file(p)
    assert violations == []


def test_mod_tests_block_excluded(tmp_path: Path):
    p = write(
        tmp_path,
        """
        pub fn real_one() -> Result<[u8; 32], MyErr> {
            Ok([0u8; 32])
        }

        #[cfg(test)]
        mod tests {
            pub fn helper() -> [u8; 32] {
                [0u8; 32]
            }
        }
        """,
    )
    _, violations = MODULE.audit_file(p)
    assert violations == []


def test_inline_mod_tests_without_cfg_excluded(tmp_path: Path):
    """`mod tests` (the conventional name) is excluded even without `#[cfg(test)]`."""
    p = write(
        tmp_path,
        """
        mod tests {
            pub fn helper() -> [u8; 32] {
                [0u8; 32]
            }
        }
        """,
    )
    _, violations = MODULE.audit_file(p)
    assert violations == []


def test_reference_return_is_not_flagged(tmp_path: Path):
    """`&[u8; 32]` borrows out an existing buffer — cannot fail."""
    p = write(
        tmp_path,
        """
        pub fn as_bytes(&self) -> &[u8; 32] {
            &self.0
        }
        """,
    )
    sites, violations = MODULE.audit_file(p)
    assert sites == []
    assert violations == []


def test_generic_container_with_digest_not_flagged(tmp_path: Path):
    """`&BTreeMap<[u8; 32], _>` — digest is a type arg, not the return shape."""
    p = write(
        tmp_path,
        """
        pub fn applied_packets(&self) -> &BTreeMap<[u8; 32], TraceRecord> {
            &self.applied_packets
        }
        """,
    )
    sites, violations = MODULE.audit_file(p)
    assert sites == []
    assert violations == []


def test_const_named_digest_not_flagged(tmp_path: Path):
    """`[u8; DIGEST_LEN]` — const-named size, out of scope (false negative ok)."""
    p = write(
        tmp_path,
        """
        pub fn h() -> [u8; DIGEST_LEN] {
            [0u8; DIGEST_LEN]
        }
        """,
    )
    sites, violations = MODULE.audit_file(p)
    assert sites == []
    assert violations == []


def test_doc_block_must_contain_literal_phrase(tmp_path: Path):
    """A doc comment that doesn't mention the magic phrase is not enough."""
    p = write(
        tmp_path,
        """
        /// This function cannot fail.
        pub fn h() -> [u8; 32] {
            [0u8; 32]
        }
        """,
    )
    _, violations = MODULE.audit_file(p)
    assert len(violations) == 1


def test_attribute_between_doc_and_fn_does_not_break_scan(tmp_path: Path):
    p = write(
        tmp_path,
        """
        /// infallible by construction
        #[must_use]
        #[inline]
        pub fn h() -> [u8; 32] {
            [0u8; 32]
        }
        """,
    )
    _, violations = MODULE.audit_file(p)
    assert violations == []


def test_block_comment_does_not_confuse_parser(tmp_path: Path):
    p = write(
        tmp_path,
        """
        /* multi
           line
           pub fn fake() -> [u8; 32] */
        pub fn real() -> Result<[u8; 32], E> {
            Ok([0u8; 32])
        }
        """,
    )
    _, violations = MODULE.audit_file(p)
    assert violations == []


def test_where_clause_does_not_hide_digest_return(tmp_path: Path):
    p = write(tmp_path, "pub fn h<T>(t: T) -> [u8; 32] where T: Clone { [0; 32] }")
    _, violations = MODULE.audit_file(p)
    assert len(violations) == 1


def test_result_generic_is_an_explicit_failure_channel(tmp_path: Path):
    p = write(tmp_path, "pub fn h() -> Result<[u8; 32], MyErr> { todo!() }")
    sites, violations = MODULE.audit_file(p)
    assert len(sites) == 1
    assert violations == []


def test_comment_delimiters_inside_strings_do_not_hide_following_function(tmp_path: Path):
    p = write(
        tmp_path,
        'const MARKER: &str = "/*";\npub fn bad() -> [u8; 32] { [0; 32] }\n',
    )
    sites, violations = MODULE.audit_file(p)
    assert len(sites) == 1
    assert len(violations) == 1


def test_repo_audit_surfaces_real_violations_only():
    """Walking the workspace must not crash and must report digest-return sites.

    We don't assert zero violations — the lint catches a real legacy anti-pattern
    in `quanta-index-lq-history/src/write_packet.rs::hash` that the codebase
    has not yet fixed. The assertion is that ALL surfaced violations match the
    expected bare-digest shape, not a parser artifact.
    """
    all_sites: list = []
    all_violations: list = []
    for path in MODULE.crate_source_files():
        sites, violations = MODULE.audit_file(path)
        all_sites.extend(sites)
        all_violations.extend(violations)
    for v in all_violations:
        assert "must return Result<[u8;" in v.message
