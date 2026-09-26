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
