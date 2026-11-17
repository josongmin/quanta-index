"""Regression tests for the immutable process-source inventory gate."""

import json
import shutil
import tempfile
import unittest
from pathlib import Path

from audit_source import PACKAGE_ROOT, audit_package


class SourceAuditTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name) / "package"
        self.root.mkdir()
        shutil.copytree(PACKAGE_ROOT / "src", self.root / "src")
        for name in ("Cargo.toml", "audited-source.json"):
            shutil.copyfile(PACKAGE_ROOT / name, self.root / name)

    def test_reviewed_snapshot_passes(self) -> None:
        self.assertEqual(audit_package(self.root), [])

    def test_tampered_source_fails(self) -> None:
        with (self.root / "src/tests.rs").open("a") as output:
            output.write("\n// changed\n")
        self.assertIn("changed audited Rust source: src/tests.rs", audit_package(self.root))

    def test_relocation_inventory_cannot_be_rewritten_by_reviewed_changes(self) -> None:
        path = self.root / "audited-source.json"
        baseline = json.loads(path.read_text())
        baseline["files"]["src/tests.rs"] = baseline["reviewed_changes"]["src/tests.rs"]
        path.write_text(json.dumps(baseline))
        self.assertEqual(audit_package(self.root), ["relocation source inventory changed"])

    def test_reviewed_changes_reject_noncanonical_paths(self) -> None:
        path = self.root / "audited-source.json"
        baseline = json.loads(path.read_text())
        baseline["reviewed_changes"]["src//tests.rs"] = baseline["reviewed_changes"]["src/tests.rs"]
        path.write_text(json.dumps(baseline))
        self.assertEqual(audit_package(self.root), ["invalid audited source path: src//tests.rs"])

    def test_added_source_fails(self) -> None:
        (self.root / "src/extra.rs").write_text("fn extra() {}\n")
        self.assertIn("unexpected Rust source: src/extra.rs", audit_package(self.root))

    def test_missing_source_fails(self) -> None:
        (self.root / "src/tests.rs").unlink()
        self.assertIn("missing audited Rust source: src/tests.rs", audit_package(self.root))

    def test_symlink_source_fails(self) -> None:
        source = self.root / "src/tests.rs"
        source.unlink()
        source.symlink_to(PACKAGE_ROOT / "src/tests.rs")
        self.assertTrue(
            any("nonregular source file" in failure for failure in audit_package(self.root))
        )

    def test_changed_manifest_fails(self) -> None:
        with (self.root / "Cargo.toml").open("a") as output:
            output.write("\n# changed\n")
        self.assertIn("changed audited Cargo.toml", audit_package(self.root))


if __name__ == "__main__":
    unittest.main()
