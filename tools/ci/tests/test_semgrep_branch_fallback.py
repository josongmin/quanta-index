"""Keep Result-branch guards distinct from checked conversion predicates."""

from __future__ import annotations

import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

RULES = Path(__file__).resolve().parents[1] / "semgrep" / "rules.yml"


@unittest.skipUnless(shutil.which("semgrep"), "Semgrep is not installed")
class BranchFallbackRulesTest(unittest.TestCase):
    def test_fallbacks_fail_but_checked_width_and_versioned_contracts_do_not(self) -> None:
        with tempfile.TemporaryDirectory(prefix="qi-branch-fallback-") as directory:
            root = Path(directory)
            source = root / "crates" / "example" / "src" / "sample.rs"
            source.parent.mkdir(parents=True)
            source.write_text(
                """pub struct CandidateAddressV1;
fn width(value: u64) {
    if u8::try_from(value).is_ok() { narrow(); } else { wide(); }
}
fn hidden(result: Result<(), ()>) {
    if result.is_ok() { serve(); } else { serve_default(); }
}
fn inverted(result: Result<(), ()>) {
    if result.is_err() { serve_default(); } else { serve(); }
}
""",
                encoding="utf-8",
            )
            result = subprocess.run(
                [
                    "semgrep",
                    "scan",
                    "--config",
                    str(RULES),
                    "--json",
                    "--no-git-ignore",
                    "--metrics",
                    "off",
                    str(source),
                ],
                cwd=root,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            rules = {
                finding["check_id"].split(".")[-1]
                for finding in json.loads(result.stdout)["results"]
            }
            self.assertEqual(
                rules,
                {"rust-no-is-ok-as-branch", "rust-no-is-err-as-branch"},
            )

    def test_workflow_guards_detect_direct_cargo_and_continue_on_error(self) -> None:
        with tempfile.TemporaryDirectory(prefix="qi-workflow-guards-") as directory:
            root = Path(directory)
            workflow = root / ".github" / "workflows" / "ci.yml"
            workflow.parent.mkdir(parents=True)
            workflow.write_text(
                """jobs:
  build:
    steps:
      - run: cargo test --workspace
      - run: cargo machete --with-metadata
      - run: ./scripts/cargow test --workspace
      - name: cargo test (nightly)
        run: |
          source scripts/quanta-index-env.sh
          cargo test -Z build-std
      - continue-on-error: true
        run: ./scripts/cargow check --workspace
""",
                encoding="utf-8",
            )
            workflow.with_suffix(".yaml").write_text(
                workflow.read_text(encoding="utf-8"), encoding="utf-8"
            )
            result = subprocess.run(
                [
                    "semgrep",
                    "scan",
                    "--config",
                    str(RULES),
                    "--json",
                    "--no-git-ignore",
                    "--metrics",
                    "off",
                    str(workflow),
                    str(workflow.with_suffix(".yaml")),
                ],
                cwd=root,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            findings = [
                (Path(item["path"]).suffix, item["check_id"].split(".")[-1], item["start"]["line"])
                for item in json.loads(result.stdout)["results"]
            ]
            self.assertEqual(
                set(findings),
                {
                    (suffix, rule, line)
                    for suffix in (".yml", ".yaml")
                    for rule, line in (
                        ("workflow-use-cargow", 4),
                        ("workflow-use-cargow", 10),
                        ("workflow-no-continue-on-error", 11),
                    )
                },
            )

    def test_remaining_rust_rules_have_reachable_counterexamples(self) -> None:
        with tempfile.TemporaryDirectory(prefix="qi-rust-guards-") as directory:
            root = Path(directory)
            subprocess.run(["git", "init", "-q"], cwd=root, check=True)
            files = {
                "crates/quanta-index-core/src/lib.rs": """fn bad(result: Result<(), ()>) {
    let _ = result.or_else(|_| Ok(()));
    if cfg!(debug_assertions) { fail_open(); }
}
""",
                "crates/quanta-index-searchd/src/lib.rs": """fn bad() -> StructuralReadiness {
    let Ok(value) = prerequisite() else { return StructuralReadiness::Ready; };
    let _ = value;
    StructuralReadiness::Ready
}
""",
                "crates/quanta-index-search-plane/src/lib.rs": """fn bad() {
    let _ = ciborium::from_reader(input());
}
""",
            }
            for relative, contents in files.items():
                path = root / relative
                path.parent.mkdir(parents=True)
                path.write_text(contents, encoding="utf-8")
            result = subprocess.run(
                [
                    "semgrep",
                    "scan",
                    "--config",
                    str(RULES),
                    "--json",
                    "--no-git-ignore",
                    "--metrics",
                    "off",
                    "crates",
                ],
                cwd=root,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            rules = {
                item["check_id"].split(".")[-1] for item in json.loads(result.stdout)["results"]
            }
            self.assertEqual(
                rules,
                {
                    "rust-no-silent-or-else-ok",
                    "rust-no-debug-assertions-divergence",
                    "rust-no-ready-on-failed-structural-precondition",
                    "rust-no-search-plane-direct-ciborium",
                },
            )


if __name__ == "__main__":
    unittest.main()
