"""Exercise the search-plane producer-authority Semgrep rules on counterexamples."""

from __future__ import annotations

import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

RULES = Path(__file__).resolve().parents[1] / "semgrep" / "rules.yml"
RULE_IDS = {
    "search-plane-no-process-spawn",
    "search-plane-no-producer-parser-import",
}


@unittest.skipUnless(shutil.which("semgrep"), "Semgrep is not installed")
class SearchPlaneAuthorityRulesTest(unittest.TestCase):
    def test_process_and_parser_import_are_rejected_without_test_false_positive(self) -> None:
        with tempfile.TemporaryDirectory(prefix="qi-source-authority-") as directory:
            root = Path(directory)
            subprocess.run(["git", "init", "-q"], cwd=root, check=True)
            source_root = root / "crates/quanta-index-search-plane/src"
            source_root.mkdir(parents=True)
            (source_root / "bad.rs").write_text(
                """use std::process::Command;
use tree_sitter::Parser;
fn bad() { let _ = Command::new("git").output(); }
""",
                encoding="utf-8",
            )
            (source_root / "direct.rs").write_text(
                'fn bad() { let _ = std::process::Command::new("git").output(); }\n',
                encoding="utf-8",
            )
            (source_root / "aliased.rs").write_text(
                'use std::process::Command as HostCommand;\nfn bad() { let _ = HostCommand::new("git"); }\n',
                encoding="utf-8",
            )
            (source_root / "qualified.rs").write_text(
                'fn bad() { let _ = git2::Repository::open("."); }\n',
                encoding="utf-8",
            )
            tests_root = source_root / "tests"
            tests_root.mkdir()
            (tests_root / "harness.rs").write_text(
                'use std::process::Command;\nfn test_only() { let _ = Command::new("git"); }\n',
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
                    "--disable-version-check",
                    "crates/quanta-index-search-plane/src",
                ],
                cwd=root,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            findings = [
                item
                for item in json.loads(result.stdout)["results"]
                if item["check_id"].split(".")[-1] in RULE_IDS
            ]
            self.assertEqual({item["check_id"].split(".")[-1] for item in findings}, RULE_IDS)
            self.assertEqual(
                {Path(item["path"]).name for item in findings},
                {"bad.rs", "direct.rs", "aliased.rs", "qualified.rs"},
            )


if __name__ == "__main__":
    unittest.main()
