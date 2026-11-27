"""Exercise the Semgrep workflow rules against one counterexample repository."""

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
      - continue-on-error: True
        run: ./scripts/cargow check --workspace
      - continue-on-error: false
        run: ./scripts/cargow check --workspace
      - continue-on-error: ${{ matrix.experimental }}
        run: ./scripts/cargow check --workspace
      - run: |
          cat <<'EOF'
          continue-on-error: true
          EOF
      - run: |
          cargo \\
            test --workspace
      - run: 'echo name: && cargo test'
      - run: cargo +nightly test --workspace
      - run: |
          cargo \\
            +1.92.0 \\
            check --workspace
      - run: ./scripts/cargow +nightly test --workspace
      - run: cargo --locked test --workspace
      - run: cargo --offline check --workspace
      - run: cargo --config net.offline=true test --workspace
      - run: cargo +nightly -Z unstable-options check --workspace
      - run: cargo -vv --color always --frozen nextest run
      - run: |
          cargo \\
            --locked \\
            test --workspace
      - run: ./scripts/cargow --locked test --workspace
      - name: cargo --locked test example
        run: echo safe
      - run: cargo --config 'net.offline = true' test --workspace
      - run: cargo --config="net.offline = true" check --workspace
      - run: ./scripts/cargow --config 'net.offline = true' test --workspace
""",
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

    def test_workflow_guards(self) -> None:
        self.assertIs(YAML(typ="safe").load("continue-on-error: True")["continue-on-error"], True)
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
                    ("workflow-use-cargow", 24),
                    ("workflow-use-cargow", 26),
                    ("workflow-use-cargow", 27),
                    ("workflow-use-cargow", 29),
                    ("workflow-use-cargow", 33),
                    ("workflow-use-cargow", 34),
                    ("workflow-use-cargow", 35),
                    ("workflow-use-cargow", 36),
                    ("workflow-use-cargow", 37),
                    ("workflow-use-cargow", 39),
                    ("workflow-use-cargow", 45),
                    ("workflow-use-cargow", 46),
                    ("workflow-no-continue-on-error", 11),
                    ("workflow-no-continue-on-error", 13),
                    ("workflow-no-continue-on-error", 15),
                    ("workflow-no-continue-on-error", 17),
                )
            },
        )


if __name__ == "__main__":
    unittest.main()
