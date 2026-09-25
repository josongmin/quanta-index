"""Focused contract tests for candidate corpus-set freezing."""

import importlib.util
import subprocess
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("corpus_set.py")
SPEC = importlib.util.spec_from_file_location("corpus_set", MODULE_PATH)
assert SPEC and SPEC.loader
corpus_set = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(corpus_set)


class CorpusSetTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.checkouts = Path(self.temp.name)
        self.repo = self.checkouts / "fixture"
        self.repo.mkdir()
        subprocess.run(["git", "init", "-q", str(self.repo)], check=True)
        subprocess.run(["git", "-C", str(self.repo), "config", "user.name", "Fixture"], check=True)
        subprocess.run(
            ["git", "-C", str(self.repo), "config", "user.email", "fixture@example.invalid"],
            check=True,
        )
        (self.repo / "src").mkdir()
        (self.repo / "src" / "main.rs").write_text("fn main() {}\n")
        (self.repo / "src" / "large.rs").write_bytes(b"x" * (corpus_set.MAX_FILE_BYTES + 1))
        (self.repo / "src" / "bad.rs").write_text("bad\u2028line")
        (self.repo / "README.md").write_text("not code\n")
        (self.repo / "LICENSE").write_text("fixture license source\n")
        subprocess.run(["git", "-C", str(self.repo), "add", "."], check=True)
        subprocess.run(["git", "-C", str(self.repo), "commit", "-qm", "fixture"], check=True)
        self.commit = corpus_set.git(self.repo, "rev-parse", "HEAD").decode().strip()
        self.entry = {
            "name": "fixture",
            "language": "rust",
            "url": "https://example.invalid/fixture.git",
            "revision": self.commit,
            "benchmark_root": "src",
            "upstream_semble_benchmark_overlap": False,
        }

    def test_freezes_exact_code_universe_with_exclusions(self):
        manifests, summary = corpus_set.freeze_set(
            {"source_revision": "fixture", "repositories": [self.entry]}, self.checkouts
        )
        self.assertEqual([f["path"] for f in manifests["fixture"]["files"]], ["src/main.rs"])
        self.assertEqual(summary["file_count"], 1)
        self.assertEqual(
            summary["repositories"][0]["excluded_tracked_file_counts"],
            {
                "binary_or_exotic_line_break": 1,
                "empty_or_oversize": 1,
                "outside_benchmark_root": 2,
            },
        )
        self.assertEqual(
            summary["repositories"][0]["license_source_files_not_approval"][0]["path"], "LICENSE"
        )
        self.assertEqual(len(summary["repositories"][0]["excluded_tracked_files"]), 4)
        again, again_summary = corpus_set.freeze_set(
            {"source_revision": "fixture", "repositories": [self.entry]}, self.checkouts
        )
        self.assertEqual(corpus_set.canonical_json(manifests), corpus_set.canonical_json(again))
        self.assertEqual(
            corpus_set.canonical_json(summary), corpus_set.canonical_json(again_summary)
        )

    def test_rejects_dirty_wrong_revision_and_duplicate(self):
        spec = {"source_revision": "fixture", "repositories": [self.entry]}
        with self.assertRaisesRegex(ValueError, "duplicate"):
            corpus_set.freeze_set(
                {**spec, "repositories": [self.entry, self.entry]}, self.checkouts
            )
        with self.assertRaisesRegex(ValueError, "revision mismatch"):
            corpus_set.freeze_set(
                {**spec, "repositories": [{**self.entry, "revision": "0" * 40}]}, self.checkouts
            )
        (self.repo / "src" / "main.rs").write_text("dirty\n")
        with self.assertRaisesRegex(ValueError, "dirty checkout"):
            corpus_set.freeze_set(spec, self.checkouts)

    def test_rejects_unknown_entry_and_unsafe_root(self):
        with self.assertRaisesRegex(ValueError, "invalid repository entry"):
            corpus_set.freeze_set(
                {"source_revision": "fixture", "repositories": [{**self.entry, "other": 1}]},
                self.checkouts,
            )
        with self.assertRaisesRegex(ValueError, "invalid benchmark root"):
            corpus_set.freeze_set(
                {
                    "source_revision": "fixture",
                    "repositories": [{**self.entry, "benchmark_root": "../src"}],
                },
                self.checkouts,
            )
        with self.assertRaisesRegex(ValueError, "invalid benchmark root"):
            corpus_set.freeze_set(
                {
                    "source_revision": "fixture",
                    "repositories": [{**self.entry, "benchmark_root": "/src"}],
                },
                self.checkouts,
            )

    def test_refuses_corpus_inputs_or_outputs_inside_checkout(self):
        with self.assertRaisesRegex(ValueError, "outside"):
            corpus_set.require_external_path(
                MODULE_PATH.parent / "candidate.json", "corpus-set spec"
            )
        corpus_set.require_external_path(self.checkouts, "corpus checkouts")


if __name__ == "__main__":
    unittest.main()
