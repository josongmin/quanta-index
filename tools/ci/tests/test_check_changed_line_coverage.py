"""Contract tests for the fail-closed changed-line coverage gate."""

from __future__ import annotations

import importlib.util
import os
import shlex
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools" / "ci" / "lint" / "check-changed-line-coverage.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("check_changed_line_coverage", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["check_changed_line_coverage"] = module
    spec.loader.exec_module(module)
    return module


def _write_lcov(path: Path, source: Path, hits: dict[int, int]) -> Path:
    lines = [f"SF:{source}"]
    lines.extend(f"DA:{line},{count}" for line, count in sorted(hits.items()))
    lines.append("end_of_record")
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return path


def test_relative_lcov_source_is_resolved_from_repo_root(tmp_path: Path, monkeypatch):
    module = _load_module()
    root = tmp_path / "repo"
    source = root / "crates/demo/src/lib.rs"
    source.parent.mkdir(parents=True)
    source.write_text("pub fn demo() {}\n", encoding="utf-8")
    lcov = root / "coverage.lcov"
    lcov.write_text("SF:crates/demo/src/lib.rs\nDA:1,1\nend_of_record\n", encoding="utf-8")
    outside = tmp_path / "outside"
    outside.mkdir()
    monkeypatch.chdir(outside)

    result = module.audit_changed_coverage(root, lcov, {("crates/demo/src/lib.rs", 1)}, 90.0)

    assert result.percent == 100.0
    assert result.violations == []


def test_external_lcov_sources_remain_excluded(tmp_path: Path):
    module = _load_module()
    root = tmp_path / "repo"
    root.mkdir()
    external = tmp_path / "outside.rs"
    external.write_text("pub fn outside() {}\n", encoding="utf-8")
    lcov = root / "coverage.lcov"
    for source in (str(external), "../outside.rs"):
        lcov.write_text(f"SF:{source}\nDA:1,1\nend_of_record\n", encoding="utf-8")
        assert module.parse_lcov(root, lcov) == {}


def test_changed_executable_lines_meeting_threshold_are_green(tmp_path: Path):
    module = _load_module()
    source = tmp_path / "crates" / "demo" / "src" / "lib.rs"
    source.parent.mkdir(parents=True)
    source.write_text("pub fn demo() {}\n", encoding="utf-8")
    lcov = _write_lcov(tmp_path / "coverage.lcov", source, {1: 3, 2: 1})

    result = module.audit_changed_coverage(
        tmp_path,
        lcov,
        {
            (source.relative_to(tmp_path).as_posix(), 1),
            (source.relative_to(tmp_path).as_posix(), 2),
        },
        90.0,
    )

    assert result.violations == []
    assert result.covered_lines == 2
    assert result.total_lines == 2


def test_uncovered_changed_line_fails_closed(tmp_path: Path):
    module = _load_module()
    source = tmp_path / "crates" / "demo" / "src" / "lib.rs"
    source.parent.mkdir(parents=True)
    source.write_text("pub fn demo() {}\n", encoding="utf-8")
    lcov = _write_lcov(tmp_path / "coverage.lcov", source, {1: 1, 2: 0})

    result = module.audit_changed_coverage(
        tmp_path,
        lcov,
        {
            (source.relative_to(tmp_path).as_posix(), 1),
            (source.relative_to(tmp_path).as_posix(), 2),
        },
        90.0,
    )

    assert result.covered_lines == 1
    assert result.total_lines == 2
    assert any("uncovered changed line" in item for item in result.violations)
    assert any("below required 90.00%" in item for item in result.violations)


def test_threshold_allows_known_zero_hits_but_requires_evidence(tmp_path: Path):
    module = _load_module()
    source = tmp_path / "crates" / "demo" / "src" / "lib.rs"
    hits = {line: int(line != 10) for line in range(1, 11)}
    lcov = _write_lcov(tmp_path / "coverage.lcov", source, hits)
    changed = {(source.relative_to(tmp_path).as_posix(), line) for line in hits}

    result = module.audit_changed_coverage(tmp_path, lcov, changed, 90.0)
    assert result.percent == 90.0
    assert result.violations == []

    del hits[10]
    _write_lcov(lcov, source, hits)
    missing = module.audit_changed_coverage(tmp_path, lcov, changed, 90.0)
    assert missing.percent == 90.0
    assert any("changed line missing from LCOV" in violation for violation in missing.violations)


def test_changed_production_line_missing_from_lcov_fails_closed(tmp_path: Path):
    module = _load_module()
    source = tmp_path / "crates" / "demo" / "src" / "lib.rs"
    source.parent.mkdir(parents=True)
    source.write_text("pub fn demo() {}\n", encoding="utf-8")
    lcov = _write_lcov(tmp_path / "coverage.lcov", source, {1: 1})

    result = module.audit_changed_coverage(
        tmp_path, lcov, {(source.relative_to(tmp_path).as_posix(), 2)}, 90.0
    )

    assert any("missing from LCOV" in item for item in result.violations)


def test_no_changed_production_lines_is_explicitly_not_a_coverage_claim(tmp_path: Path):
    module = _load_module()
    lcov = tmp_path / "coverage.lcov"
    lcov.write_text("", encoding="utf-8")

    result = module.audit_changed_coverage(tmp_path, lcov, set(), 90.0)

    assert result.violations == []
    assert result.total_lines == 0


def test_duplicate_lcov_line_and_partial_record_are_refused(tmp_path: Path):
    import pytest

    module = _load_module()
    source = tmp_path / "crates" / "demo" / "src" / "lib.rs"
    source.parent.mkdir(parents=True)
    source.write_text("pub fn demo() {}\n", encoding="utf-8")
    lcov = tmp_path / "coverage.lcov"
    lcov.write_text(f"SF:{source}\nDA:1,0\nDA:1,1\nend_of_record\n", encoding="utf-8")
    with pytest.raises(ValueError, match="duplicate LCOV DA"):
        module.parse_lcov(tmp_path, lcov)
    lcov.write_text(f"SF:{source}\nDA:1,1\n", encoding="utf-8")
    with pytest.raises(ValueError, match="lacks end_of_record"):
        module.parse_lcov(tmp_path, lcov)


@pytest.mark.parametrize(
    "filename, mark_binary",
    [
        ("lib.rs", False),
        ("한글.rs", False),
        ("tab\tfile.rs", False),
        ('quote"file.rs', False),
        ("space file.rs", False),
        ("lib.rs", True),
    ],
)
def test_actual_git_diff_preserves_every_changed_production_path(
    tmp_path: Path, filename: str, mark_binary: bool
):
    module = _load_module()

    def git(*args):
        return subprocess.run(
            ["git", *args], cwd=tmp_path, check=True, capture_output=True, text=True
        ).stdout.strip()

    git("init", "-q")
    git("config", "user.name", "Coverage oracle")
    git("config", "user.email", "coverage@example.invalid")
    git("config", "diff.noprefix", "true")
    path = tmp_path / "crates/demo/src" / filename
    path.parent.mkdir(parents=True)
    path.write_text("pub fn demo() {}\n")
    if mark_binary:
        (tmp_path / ".gitattributes").write_text("*.rs -diff\n", encoding="utf-8")
    git("add", ".")
    git("commit", "-qm", "base")
    base = git("rev-parse", "HEAD")
    path.write_text('pub fn demo() { panic!("new uncovered behavior"); }\n')
    git("add", ".")
    git("commit", "-qm", "candidate")
    changed = module.changed_production_lines(tmp_path, base)
    assert changed == {(path.relative_to(tmp_path).as_posix(), 1)}
    lcov = _write_lcov(tmp_path / "coverage.lcov", path, {1: 0})
    verdict = module.audit_changed_coverage(tmp_path, lcov, changed, 90.0)
    assert verdict.total_lines == 1
    assert verdict.violations


def test_comment_only_commit_has_no_executable_changed_lines(tmp_path: Path):
    module = _load_module()

    def git(*args):
        return subprocess.run(
            ["git", *args], cwd=tmp_path, check=True, capture_output=True, text=True
        ).stdout.strip()

    git("init", "-q")
    git("config", "user.name", "Coverage oracle")
    git("config", "user.email", "coverage@example.invalid")
    source = tmp_path / "crates/demo/src/lib.rs"
    source.parent.mkdir(parents=True)
    source.write_text("pub fn demo() -> u32 { 1 }\n")
    git("add", ".")
    git("commit", "-qm", "base")
    base = git("rev-parse", "HEAD")
    source.write_text("/// New docs.\n\n// New comment.\npub fn demo() -> u32 { 1 }\n")
    git("add", ".")
    git("commit", "-qm", "comment only")
    source.write_text("pub fn dirty_uncommitted() -> u32 { 2 }\n")

    assert module.changed_production_lines(tmp_path, base) == set()


def test_patch_body_cannot_replace_the_destination_path(tmp_path: Path):
    module = _load_module()

    def git(*args):
        return subprocess.run(
            ["git", *args], cwd=tmp_path, check=True, capture_output=True, text=True
        ).stdout.strip()

    git("init", "-q")
    git("config", "user.name", "Coverage oracle")
    git("config", "user.email", "coverage@example.invalid")
    relative = "crates/demo/src/lib.rs"
    path = tmp_path / relative
    path.parent.mkdir(parents=True)
    path.write_text('pub const DATA: &str = r#"\n"#;\n\npub fn changed() -> u32 { 1 }\n')
    git("add", ".")
    git("commit", "-qm", "base")
    base = git("rev-parse", "HEAD")
    path.write_text(
        'pub const DATA: &str = r#"\n++ b/elsewhere.rs\n"#;\n\npub fn changed() -> u32 { 2 }\n'
    )
    git("commit", "-qam", "candidate")

    assert module.changed_production_lines(tmp_path, base) == {(relative, 2), (relative, 5)}


def test_rust_syntax_excludes_only_proven_test_scope_and_trivia():
    module = _load_module()
    source = """/// New docs.
use std::fmt;
pub struct Data { field: u32 }
#[cfg(test)]
mod tests { fn hidden() { panic!() } }
#[cfg(all(test, feature = "x"))]
fn test_only() { panic!() }
#[cfg(any(test, all(test, feature = "x")))]
fn also_test_only() { panic!() }
#[cfg(any(test, feature = "x"))]
fn mixed() { panic!() }
#[cfg(not(test))]
fn non_test() { panic!() }
pub fn live() -> u32 { 1 } // Inline comment.
"""
    lines = source.splitlines()
    selected = module._eligible_changed_rust_lines(
        source.encode(),
        set(range(1, len(lines) + 1)),
        "crates/demo/src/lib.rs",
        set(range(1, len(lines) + 1)),
    )
    assert {lines[number - 1] for number in selected} == {
        '#[cfg(any(test, feature = "x"))]',
        "fn mixed() { panic!() }",
        "#[cfg(not(test))]",
        "fn non_test() { panic!() }",
        "pub fn live() -> u32 { 1 } // Inline comment.",
    }


def test_malformed_rust_is_not_classified_as_trivia():
    module = _load_module()
    with pytest.raises(ValueError, match="cannot classify changed Rust source"):
        module._eligible_changed_rust_lines(b"pub fn broken( {", {1}, "crates/demo/src/lib.rs", {1})


def test_punctuation_inside_multiline_literal_is_still_eligible():
    module = _load_module()
    source = b'pub const DATA: &str = r#"\n{}\n"#;\n'
    assert module._eligible_changed_rust_lines(source, {2}, "crates/demo/src/lib.rs", {2}) == {2}


def test_new_test_scope_is_excluded_but_new_guard_on_existing_code_is_not():
    module = _load_module()
    source = b"#[cfg(test)]\npub fn former_production() { panic!() }\n"
    path = "crates/demo/src/lib.rs"
    assert module._eligible_changed_rust_lines(source, {1}, path, {1}) == {1}
    assert module._eligible_changed_rust_lines(source, {1, 2}, path, {1, 2}) == set()
    assert module._eligible_changed_rust_lines(source, {2}, path, {2}) == set()



@pytest.mark.parametrize(
    "mutation", [None, "before-build", "hidden-edit", "during-build", "head-moved"]
)
def test_coverage_recipe_binds_lcov_to_committed_source(tmp_path, mutation):
    just = shutil.which("just")
    if just is None:
        pytest.skip("Just is needed to exercise the changed coverage recipe")

    checkout = tmp_path / "checkout"
    checkout.mkdir()

    def git(*args):
        return subprocess.run(
            ["git", *args], cwd=checkout, check=True, capture_output=True, text=True
        ).stdout.strip()

    git("init", "-q")
    git("config", "user.name", "Coverage fixture")
    git("config", "user.email", "coverage@example.invalid")
    source = checkout / "crates/demo/src/lib.rs"
    source.parent.mkdir(parents=True)
    source.write_text("pub fn demo() -> u32 { 1 }\n")
    checker = checkout / "tools/ci/lint" / SCRIPT_PATH.name
    checker.parent.mkdir(parents=True)
    shutil.copyfile(SCRIPT_PATH, checker)
    git("add", ".")
    git("commit", "-qm", "base")
    base = git("rev-parse", "HEAD")
    source.write_text("pub fn demo() -> u32 { 2 }\n")
    git("commit", "-qam", "candidate")

    if mutation in ("before-build", "hidden-edit"):
        source.write_text("pub fn demo() -> u32 { 3 }\n")
    if mutation == "hidden-edit":
        git("update-index", "--assume-unchanged", str(source.relative_to(checkout)))
        assert git("status", "--porcelain") == ""
    build_marker = tmp_path / "built"
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    # Replay an LLVM-shaped artifact with one hit on the same filename and
    # line for both candidate bodies. Only the recipe knows which bytes ran.
    cargo = bin_dir / "coverage-producer"
    cargo.write_text(
        "#!/bin/sh\n"
        "set -eu\n"
        'printf "%s\\n" built > "$BUILD_MARKER"\n'
        'for report do :; done\n'
        'printf "SF:%s/crates/demo/src/lib.rs\\nDA:1,1\\nend_of_record\\n" "$PWD" > "$report"\n'
        'if [ "$MUTATION" = during-build ] || [ "$MUTATION" = head-moved ]; then\n'
        '    printf "%s\\n" "pub fn demo() -> u32 { 3 }" > crates/demo/src/lib.rs\n'
        "fi\n"
        'if [ "$MUTATION" = head-moved ]; then git commit -qam "another candidate"; fi\n'
    )
    uv = bin_dir / "uv"
    uv.write_text(
        "#!/bin/sh\n"
        "set -eu\n"
        "shift 5\n"
        f'exec {shlex.quote(sys.executable)} "$@"\n'
    )
    for tool in (cargo, uv):
        tool.chmod(0o755)
    result = subprocess.run(
        [
            just, "--justfile", str(REPO_ROOT / "Justfile"),
            "--working-directory", str(checkout),
            "--set", "cargo", str(cargo), "rust-coverage-changed", base,
        ],
        env={
            **os.environ,
            "PATH": f"{bin_dir}{os.pathsep}{os.environ['PATH']}",
            "BUILD_MARKER": str(build_marker),
            "MUTATION": mutation or "",
        },
        check=False, capture_output=True, text=True,
    )
    if mutation is None:
        assert result.returncode == 0, result.stderr
        assert "1/1 (100.00%)" in result.stdout
    else:
        assert result.returncode == 2, result.stdout + result.stderr
        assert "coverage requires unchanged committed source" in result.stderr
    assert build_marker.is_file() == (mutation not in ("before-build", "hidden-edit"))
