"""Allow bans inspect Rust metadata, including conditional and formatted sites."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "tools/ci/lint/check-rust-allow-attributes.py"
sys.path.insert(0, str(SCRIPT.parent))
spec = importlib.util.spec_from_file_location("check_rust_allow_attributes", SCRIPT)
assert spec and spec.loader
MODULE = importlib.util.module_from_spec(spec)
spec.loader.exec_module(MODULE)


@pytest.mark.parametrize(
    "attribute",
    [
        "#[allow(dead_code)]",
        "#![ allow ( dead_code ) ]",
        "#[allow\n(dead_code)]",
        "#[ cfg_attr ( all(), cfg_attr(all(), allow(dead_code))) ]",
        "#[cfg_attr(all(), allow(dead_code), /* trailing */)]",
    ],
)
def test_allow_sites_cannot_hide_in_formatting_or_cfg_attr(attribute: str) -> None:
    assert MODULE.allow_sites(attribute + "\nfn f() {}") == [1]


def test_comments_strings_and_cfg_predicate_are_not_allow_attributes() -> None:
    assert (
        MODULE.allow_sites("""// #[allow(dead_code)]
        #[doc = "allow(dead_code)"]
        #[cfg_attr(allow(dead_code), derive(Debug))]
        struct S;
    """)
        == []
    )


def test_invalid_attribute_is_a_scan_error() -> None:
    with pytest.raises(ValueError, match="invalid Rust attribute"):
        MODULE.allow_sites("#[allow(\nfn f() {}")


def test_cli_refuses_benchmark_allow_and_missing_roots(tmp_path: Path) -> None:
    assert MODULE.main(["--root", str(tmp_path)]) == 2
    (tmp_path / "crates").mkdir()
    source = tmp_path / "benchmarks" / "fixture" / "src" / "lib.rs"
    source.parent.mkdir(parents=True)
    source.write_text("#[ cfg_attr(all(), allow(dead_code)) ]\nfn f() {}")
    assert MODULE.main(["--root", str(tmp_path)]) == 1
    source.write_text('#[expect(dead_code, reason = "fixture")]\nfn f() {}')
    assert MODULE.main(["--root", str(tmp_path)]) == 0


def test_shell_front_door_propagates_scanner_failure(tmp_path: Path) -> None:
    result = subprocess.run(
        ["bash", str(ROOT / "scripts/check-rust-allow-attributes.sh"), "--root", str(tmp_path)],
        check=False,
        capture_output=True,
        text=True,
        env={"PATH": str(Path(sys.executable).parent) + ":/usr/bin:/bin"},
    )
    assert result.returncode == 2
    assert "Rust source roots are missing" in result.stderr


@pytest.mark.parametrize(
    "source",
    [
        "macro_rules! m { () => { #[allow(dead_code)] fn f() {} }; }",
        "m! { #![cfg_attr(all(), cfg_attr(all(), allow(dead_code)))] }",
        "#[r#allow(dead_code)] fn f() {}",
    ],
)
def test_raw_and_macro_allow_attributes_are_banned(source: str) -> None:
    assert MODULE.allow_sites(source) == [1]


def test_macro_strings_and_unrelated_metadata_do_not_trigger_allow_ban() -> None:
    assert MODULE.allow_sites('m! { r#"#[allow(dead_code)]"# #[doc = "allow"] }') == []


def test_directory_traversal_failure_cannot_certify_clean_inventory(tmp_path, monkeypatch):
    import rust_attribute_policy as policy

    roots = (tmp_path / "crates", tmp_path / "benchmarks")
    for root in roots:
        root.mkdir()

    def unreadable_walk(root, *, onerror):
        onerror(PermissionError("fixture Rust directory cannot be read"))
        return iter(())

    monkeypatch.setattr(policy.os, "walk", unreadable_walk)
    assert MODULE.main(["--root", str(tmp_path)]) == 2


def test_symlinked_source_directory_cannot_be_silently_omitted(tmp_path):
    roots = (tmp_path / "crates", tmp_path / "benchmarks")
    for root in roots:
        root.mkdir()
    outside = tmp_path / "external"
    outside.mkdir()
    (outside / "lib.rs").write_text("#[allow(dead_code)] fn f() {}")
    (roots[0] / "linked-crate").symlink_to(outside, target_is_directory=True)
    assert MODULE.main(["--root", str(tmp_path)]) == 2


@pytest.mark.parametrize("area", ["src", "tests", "fuzz/fuzz_targets", "benches", "examples"])
def test_source_module_named_target_is_inventoried_and_guarded(tmp_path, area):
    from rust_attribute_policy import rust_source_files

    (tmp_path / "benchmarks").mkdir()
    source = tmp_path / "crates" / "demo" / area / "target" / "mod.rs"
    source.parent.mkdir(parents=True)
    source.write_text("#[allow(dead_code)] fn f() {}")
    assert rust_source_files((tmp_path / "crates", tmp_path / "benchmarks")) == [source]
    assert MODULE.main(["--root", str(tmp_path)]) == 1


def test_canonical_package_and_fuzz_build_outputs_are_excluded(tmp_path):
    from rust_attribute_policy import rust_source_files

    (tmp_path / "benchmarks").mkdir()
    for location in ["target", "fuzz/target"]:
        output = tmp_path / "crates" / "demo" / location / "generated.rs"
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text("#[allow(dead_code)] fn f() {}")
    assert rust_source_files((tmp_path / "crates", tmp_path / "benchmarks")) == []
    assert MODULE.main(["--root", str(tmp_path)]) == 0
