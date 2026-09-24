"""Exercise all Semgrep rules against one shared counterexample repository."""

from __future__ import annotations

import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from ruamel.yaml import YAML

ROOT = Path(__file__).resolve().parents[3]
RULES = ROOT / "tools/ci/semgrep/rules.yml"


class SemgrepPolicyTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        if shutil.which("semgrep") is None:
            raise RuntimeError("Semgrep is required for policy counterexamples")
        temporary = tempfile.TemporaryDirectory(prefix="qi-semgrep-policy-")
        cls.addClassCleanup(temporary.cleanup)
        root = Path(temporary.name)
        subprocess.run(["git", "init", "-q"], cwd=root, check=True)
        (root / ".semgrepignore").write_text(
            (ROOT / ".semgrepignore").read_text(encoding="utf-8"), encoding="utf-8"
        )
        files = {
            ".github/workflows/ci.yml": """jobs:
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
            "crates/example/src/sample.rs": """pub struct CandidateAddressV1;
fn width(value: u64) {
    if u8::try_from(value).is_ok() { narrow(); } else { wide(); }
}
fn width_chain(value: u64) {
    if value < 24 { tiny(); }
    else if u8::try_from(value).is_ok() { narrow(); }
    else if u16::try_from(value).is_ok() { medium(); }
    else if u32::try_from(value).is_ok() { large(); }
    else { wide(); }
}
fn hidden(result: Result<(), ()>) {
    if result.is_ok() { serve(); } else { serve_default(); }
}
fn inverted(result: Result<(), ()>) {
    if result.is_err() { serve_default(); } else { serve(); }
}
""",
            "crates/quanta-index-core/src/lib.rs": """fn bad(result: Result<(), ()>) {
    let _ = result.or_else(|_| Ok(()));
    let _ = result.or_else(|err| { log(err); Ok(()) });
    let _ = result.or_else(|_| { Ok(()) });
    if cfg!(debug_assertions) { fail_open(); }
}
""",
            "crates/quanta-index-core/tests/silent.rs": """fn test_only(result: Result<(), ()>) {
    let _ = result.or_else(|_| Ok(()));
}
""",
            "crates/quanta-index-core/src/tests.rs": """fn test_only(result: Result<(), ()>) {
    let _ = result.or_else(|_| Ok(()));
}
""",
            "crates/quanta-index-core/fuzz/fuzz_targets/silent.rs": """fn fuzz_target(result: Result<(), ()>) {
    let _ = result.or_else(|_| Ok(()));
}
""",
            "benchmarks/retrieval/src/silent.rs": """fn benchmark(result: Result<(), ()>) {
    let _ = result.or_else(|_| Ok(()));
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
            "crates/quanta-index-search-plane/src/bad.rs": """use std::process::Command;
use tree_sitter::Parser;
fn bad() { let _ = Command::new("git").output(); }
""",
            "crates/quanta-index-search-plane/src/direct.rs": (
                'fn bad() { let _ = std::process::Command::new("git").output(); }\n'
            ),
            "crates/quanta-index-search-plane/src/aliased.rs": (
                "use std::process::Command as HostCommand;\n"
                'fn bad() { let _ = HostCommand::new("git"); }\n'
            ),
            "crates/quanta-index-search-plane/src/qualified.rs": (
                'fn bad() { let _ = git2::Repository::open("."); }\n'
            ),
            "crates/quanta-index-search-plane/src/tests/harness.rs": (
                'use std::process::Command;\nfn test_only() { let _ = Command::new("git"); }\n'
            ),
        }
        files[".github/workflows/ci.yaml"] = files[".github/workflows/ci.yml"]
        for relative, contents in files.items():
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
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
                "--disable-version-check",
                ".",
            ],
            cwd=root,
            text=True,
            capture_output=True,
            check=False,
        )
        if result.returncode != 0:
            raise AssertionError(result.stderr)
        cls.findings = json.loads(result.stdout)["results"]

    @staticmethod
    def rule(finding: dict) -> str:
        return finding["check_id"].split(".")[-1]

    def test_every_rule_has_a_counterexample(self) -> None:
        configured = [rule["id"] for rule in YAML(typ="safe").load(RULES.read_text())["rules"]]
        self.assertEqual(len(configured), len(set(configured)))
        self.assertEqual({self.rule(item) for item in self.findings}, set(configured))

    def test_result_branch_guards_and_checked_width_exceptions(self) -> None:
        findings = {
            (self.rule(item), item["start"]["line"])
            for item in self.findings
            if item["path"] == "crates/example/src/sample.rs"
        }
        self.assertEqual(
            findings,
            {("rust-no-is-ok-as-branch", 13), ("rust-no-is-err-as-branch", 16)},
        )

    def test_workflow_guards(self) -> None:
        findings = {
            (Path(item["path"]).suffix, self.rule(item), item["start"]["line"])
            for item in self.findings
            if item["path"].startswith(".github/workflows/")
        }
        self.assertEqual(
            findings,
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

    def test_other_rust_rules_and_test_exclusions(self) -> None:
        expected_rules = {
            "rust-no-silent-or-else-ok",
            "rust-no-debug-assertions-divergence",
            "rust-no-ready-on-failed-structural-precondition",
            "rust-no-search-plane-direct-ciborium",
        }
        self.assertEqual(
            {self.rule(item) for item in self.findings if self.rule(item) in expected_rules},
            expected_rules,
        )
        self.assertEqual(
            {
                (self.rule(item), item["path"], item["start"]["line"])
                for item in self.findings
                if self.rule(item) in expected_rules
                and self.rule(item) != "rust-no-silent-or-else-ok"
            },
            {
                ("rust-no-debug-assertions-divergence", "crates/quanta-index-core/src/lib.rs", 5),
                (
                    "rust-no-ready-on-failed-structural-precondition",
                    "crates/quanta-index-searchd/src/lib.rs",
                    2,
                ),
                (
                    "rust-no-search-plane-direct-ciborium",
                    "crates/quanta-index-search-plane/src/lib.rs",
                    2,
                ),
            },
        )
        self.assertEqual(
            {
                (item["path"], item["start"]["line"])
                for item in self.findings
                if self.rule(item) == "rust-no-silent-or-else-ok"
            },
            {
                ("crates/quanta-index-core/src/lib.rs", 2),
                ("crates/quanta-index-core/src/lib.rs", 3),
                ("crates/quanta-index-core/src/lib.rs", 4),
                ("crates/quanta-index-core/fuzz/fuzz_targets/silent.rs", 2),
                ("benchmarks/retrieval/src/silent.rs", 2),
            },
        )

    def test_search_plane_authority_rules(self) -> None:
        authority_rules = {
            "search-plane-no-process-spawn",
            "search-plane-no-producer-parser-import",
        }
        findings = [item for item in self.findings if self.rule(item) in authority_rules]
        self.assertEqual({self.rule(item) for item in findings}, authority_rules)
        self.assertEqual(
            {Path(item["path"]).name for item in findings},
            {"bad.rs", "direct.rs", "aliased.rs", "qualified.rs"},
        )


if __name__ == "__main__":
    unittest.main()
