"""Structural checks for the separate, gold-blind file intent diagnostic."""

from __future__ import annotations

import hashlib
import io
import json
import tempfile
import unittest
from contextlib import redirect_stdout
from difflib import unified_diff
from pathlib import Path

from tools.benchmark.retrieval.arb_adapter import adapt
from tools.benchmark.retrieval.intent_file_diagnostic import (
    DiagnosticRefusal,
    main,
    rank_bound_sdk_record,
    resolve_edit2ripple_go_field_owner,
    resolve_edit2ripple_import_destinations,
    run_opt_in_candidate_lanes,
)

GO_CONTEXT = b'package gin\ntype Context struct {\n engine *Engine\n}\nfunc (c *Context) ClientIP() string {\n return ""\n}\n'
GO_ENGINE = b"package gin\ntype Engine struct {\n AppEngine bool\n}\n"


def edit_diff(before: bytes, after: bytes) -> str:
    return "".join(
        list(unified_diff(before.decode().splitlines(True), after.decode().splitlines(True)))[2:]
    )


def bound_inputs(
    lexical: list[tuple[str, str]], semantic: list[tuple[str, str]], *, task_type: str = "code2test"
) -> tuple[dict, dict, dict]:
    sample = {
        "id": "fixed",
        "task_type": task_type,
        "query": {"anchor_file": "src/anchor.rs"}
        if task_type == "edit2ripple"
        else {"pr_title": "fix"},
    }
    text = "fixed input"
    digest = hashlib.sha256(text.encode()).hexdigest()
    pack = {"task_id": "ARB-fixed", "query": text, "query_sha256": digest}

    def route(rows: list[tuple[str, str]]) -> dict:
        return {
            "candidates": [
                {"candidate_id": candidate_id, "source_repo_id": "repo", "path": path}
                for candidate_id, path in rows
            ],
            "window": {"returned": len(rows)},
        }

    record = {
        "tasks": [
            {
                "task_id": "ARB-fixed",
                "query_identity": {"original_query_sha256": digest, "semantic_text_sha256": digest},
                "lexical": route(lexical),
                "semantic": route(semantic),
            }
        ]
    }
    return record, pack, sample


def bound_import_inputs(
    root: Path, anchor: str, diff: str, sources: dict[str, bytes]
) -> tuple[dict, dict, dict, dict, str]:
    sample = {
        "id": "import-fixture",
        "task_type": "edit2ripple",
        "query": {"anchor_file": anchor, "anchor_diff": diff, "intent": "find related edits"},
    }
    adapted = adapt(sample)
    digest = adapted["adapted_text_sha256"]
    entry = {
        "task_id": "ARB-import-fixture",
        "query": adapted["adapted_text"],
        "query_sha256": digest,
    }
    files = []
    for path, raw in sources.items():
        target = root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(raw)
        files.append({"path": path, "file_sha256": hashlib.sha256(raw).hexdigest()})
    manifest = {"repository_commit": "a" * 40, "files": files}
    pack = {
        "repository_commit": "a" * 40,
        "file_universe": [dict(row) for row in files],
        "tasks": [entry],
    }
    empty = {"candidates": [], "window": {"returned": 0}}
    record = {
        "generation": {"revision_id": "a" * 40},
        "tasks": [
            {
                "task_id": entry["task_id"],
                "query_identity": {"original_query_sha256": digest, "semantic_text_sha256": digest},
                "lexical": empty,
                "semantic": empty,
            }
        ],
    }
    original = hashlib.sha256(
        json.dumps(sample["query"], ensure_ascii=False, sort_keys=True).encode()
    ).hexdigest()
    return record, pack, sample, manifest, original


class IntentFileDiagnosticTests(unittest.TestCase):
    def test_code2test_merges_different_chunks_and_prioritizes_tests(self) -> None:
        record, pack, sample = bound_inputs(
            [("a", "src/handler.rs"), ("b", "tests/handler.rs"), ("c", "docs/guide.md")],
            [("d", "src/handler.rs"), ("e", "tests/handler.rs")],
        )
        ranked = rank_bound_sdk_record(record, pack, sample)
        self.assertEqual(
            [row["path"] for row in ranked["results"]],
            ["tests/handler.rs", "src/handler.rs", "docs/guide.md"],
        )
        self.assertEqual(ranked["results"][0]["lexical"]["candidate_id"], "b")
        self.assertEqual(ranked["results"][0]["semantic"]["candidate_id"], "e")
        self.assertEqual(ranked["file_universe_size"], 3)

    def test_edit2ripple_demotes_declared_anchor(self) -> None:
        record, pack, sample = bound_inputs(
            [("a", "src/anchor.rs"), ("b", "src/neighbor.rs")],
            [("a", "src/anchor.rs")],
            task_type="edit2ripple",
        )
        ranked = rank_bound_sdk_record(record, pack, sample)
        self.assertEqual(
            [row["path"] for row in ranked["results"]], ["src/neighbor.rs", "src/anchor.rs"]
        )

    def test_refuses_cross_query_or_cross_file_identity(self) -> None:
        record, pack, sample = bound_inputs([("a", "src/a.rs")], [("b", "src/b.rs")])
        record["tasks"][0]["query_identity"]["semantic_text_sha256"] = "0" * 64
        with self.assertRaisesRegex(DiagnosticRefusal, "query identity mismatch"):
            rank_bound_sdk_record(record, pack, sample)
        record, pack, sample = bound_inputs([("a", "src/a.rs")], [("a", "src/b.rs")])
        with self.assertRaisesRegex(DiagnosticRefusal, "cross-route candidate ID collision"):
            rank_bound_sdk_record(record, pack, sample)

    def test_refuses_gold_input_and_unbound_window(self) -> None:
        record, pack, sample = bound_inputs([("a", "tests/a.rs")], [])
        sample["gold"] = {"files": ["tests/a.rs"]}
        with self.assertRaisesRegex(DiagnosticRefusal, "blind-projected"):
            rank_bound_sdk_record(record, pack, sample)
        del sample["gold"]
        record["tasks"][0]["lexical"]["window"]["returned"] = 2
        with self.assertRaisesRegex(DiagnosticRefusal, "invalid SDK window"):
            rank_bound_sdk_record(record, pack, sample)

    def test_rust_added_import_resolves_exact_pinned_module(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            anchor = "clap_builder/src/output/help_template.rs"
            target = "clap_builder/src/util/mod.rs"
            record, pack, sample, manifest, original = bound_import_inputs(
                root,
                anchor,
                "@@ -0,0 +1 @@\n+use crate::util::Escape;\n",
                {anchor: b"pub fn help() {}\n", target: b"pub mod escape;\n"},
            )
            lane = resolve_edit2ripple_import_destinations(
                record, pack, sample, manifest, root, original
            )
            self.assertEqual(lane["status"], "resolved")
            self.assertEqual([row["path"] for row in lane["candidates"]], [target])
            self.assertEqual(lane["candidates"][0]["added_import"], "util::Escape")
            # A second exact module interpretation must not pick a destination.
            other = "clap_builder/src/util/Escape.rs"
            (root / other).write_bytes(b"pub struct Escape;\n")
            entry = {
                "path": other,
                "file_sha256": hashlib.sha256((root / other).read_bytes()).hexdigest(),
            }
            manifest["files"].append(entry)
            pack["file_universe"].append(entry)
            with self.assertRaisesRegex(DiagnosticRefusal, "ambiguous Rust import destination"):
                resolve_edit2ripple_import_destinations(
                    record, pack, sample, manifest, root, original
                )

    def test_go_added_import_resolves_only_unique_module_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            anchor = "handler.go"
            target = "internal/cache/cache.go"
            record, pack, sample, manifest, original = bound_import_inputs(
                root,
                anchor,
                '@@ -2,2 +2,3 @@\n import (\n+    "example.com/project/internal/cache"\n )\n',
                {
                    "go.mod": b"module example.com/project\n",
                    anchor: b"package main\nimport (\n)\n",
                    target: b"package cache\n",
                },
            )
            lane = resolve_edit2ripple_import_destinations(
                record, pack, sample, manifest, root, original
            )
            self.assertEqual([row["path"] for row in lane["candidates"]], [target])
            second = "internal/cache/other.go"
            (root / second).write_bytes(b"package cache\n")
            entry = {
                "path": second,
                "file_sha256": hashlib.sha256((root / second).read_bytes()).hexdigest(),
            }
            manifest["files"].append(entry)
            pack["file_universe"].append(entry)
            with self.assertRaisesRegex(DiagnosticRefusal, "ambiguous Go package destination"):
                resolve_edit2ripple_import_destinations(
                    record, pack, sample, manifest, root, original
                )

    def test_gin_style_diff_has_no_import_destination(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            record, pack, sample, manifest, original = bound_import_inputs(
                root,
                "context.go",
                edit_diff(
                    GO_CONTEXT, GO_CONTEXT.replace(b' return ""', b' // no import\n return ""')
                ),
                {"context.go": GO_CONTEXT, "gin.go": GO_ENGINE},
            )
            lane = resolve_edit2ripple_import_destinations(
                record, pack, sample, manifest, root, original
            )
            self.assertEqual(lane["status"], "no_added_import")
            self.assertEqual(lane["candidates"], [])
            (root / "context.go").write_bytes(b"tampered\n")
            with self.assertRaisesRegex(DiagnosticRefusal, "source digest mismatch"):
                resolve_edit2ripple_import_destinations(
                    record, pack, sample, manifest, root, original
                )

    def test_go_raw_string_does_not_create_import_candidate(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            before = b"package main\nvar message = `\nregular text\n`\n"
            after = before.replace(b"regular text", b'import "example.com/project/internal/cache"')
            record, pack, sample, manifest, original = bound_import_inputs(
                root,
                "handler.go",
                edit_diff(before, after),
                {
                    "go.mod": b"module example.com/project\n",
                    "handler.go": before,
                    "internal/cache/cache.go": b"package cache\n",
                },
            )
            lane = resolve_edit2ripple_import_destinations(
                record, pack, sample, manifest, root, original
            )
            self.assertEqual(lane["status"], "no_added_import")

    def test_rust_raw_string_and_macro_do_not_create_import_candidates(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            before = b'const TEXT: &str = r#"\n"#;\nmacro_rules! m { () => {\n}; }\n'
            after = before.replace(b'"#;', b'use crate::util::Escape;\n"#;').replace(
                b"}; }", b"use crate::util::Escape;\n}; }"
            )
            record, pack, sample, manifest, original = bound_import_inputs(
                root,
                "src/anchor.rs",
                edit_diff(before, after),
                {"src/anchor.rs": before, "src/util.rs": b"pub struct Escape;\n"},
            )
            lane = resolve_edit2ripple_import_destinations(
                record, pack, sample, manifest, root, original
            )
            self.assertEqual(lane["status"], "no_added_import")

    def test_go_comments_and_shadowed_receivers_do_not_create_field_candidates(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            after = GO_CONTEXT.replace(
                b' return ""',
                b' // c.engine.AppEngine\n {\n  c := &Context{}\n  _ = c.engine.AppEngine\n }\n return ""',
            )
            record, pack, sample, manifest, original = bound_import_inputs(
                root,
                "context.go",
                edit_diff(GO_CONTEXT, after),
                {"context.go": GO_CONTEXT, "gin.go": GO_ENGINE},
            )
            lane = resolve_edit2ripple_go_field_owner(
                record, pack, sample, manifest, root, original
            )
            self.assertEqual(lane["status"], "no_exact_added_selector")
            self.assertEqual(lane["candidates"], [])

    def test_go_added_selector_resolves_existing_field_owner_only(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            record, pack, sample, manifest, original = bound_import_inputs(
                root,
                "context.go",
                edit_diff(
                    GO_CONTEXT,
                    GO_CONTEXT.replace(
                        b' return ""',
                        b' switch {\n case c.engine.AppEngine:\n case c.engine.CloudflareProxy:\n }\n return ""',
                    ),
                ),
                {
                    "context.go": GO_CONTEXT,
                    "gin.go": GO_ENGINE,
                },
            )
            lane = resolve_edit2ripple_go_field_owner(
                record, pack, sample, manifest, root, original
            )
            self.assertEqual(lane["status"], "partial")
            self.assertEqual([row["path"] for row in lane["candidates"]], ["gin.go"])
            self.assertEqual(lane["candidates"][0]["declaration"], "Engine.AppEngine")
            self.assertEqual(lane["unresolved_selectors"], ["c.engine.CloudflareProxy"])
            other = "duplicate.go"
            (root / other).write_bytes(b"package gin\ntype Engine struct {\n AppEngine bool\n}\n")
            entry = {
                "path": other,
                "file_sha256": hashlib.sha256((root / other).read_bytes()).hexdigest(),
            }
            manifest["files"].append(entry)
            pack["file_universe"].append(entry)
            with self.assertRaisesRegex(DiagnosticRefusal, "ambiguous Go field owner"):
                resolve_edit2ripple_go_field_owner(record, pack, sample, manifest, root, original)

    def test_opt_in_cli_requires_frozen_blind_inputs(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "source"
            source.mkdir()
            record, pack, sample, manifest, original = bound_import_inputs(
                source,
                "context.go",
                edit_diff(
                    GO_CONTEXT,
                    GO_CONTEXT.replace(b' return ""', b' _ = c.engine.AppEngine\n return ""'),
                ),
                {
                    "context.go": GO_CONTEXT,
                    "gin.go": GO_ENGINE,
                },
            )
            pack["schema_version"] = 3

            def write(name: str, payload: dict) -> tuple[Path, str]:
                path = root / name
                raw = (json.dumps(payload, ensure_ascii=False, sort_keys=True) + "\n").encode()
                path.write_bytes(raw)
                return path, hashlib.sha256(raw).hexdigest()

            record_path, record_sha = write("record.json", record)
            pack_path, pack_sha = write("pack.json", pack)
            sample_path, sample_sha = write("public-sample.json", sample)
            manifest_path, manifest_sha = write("manifest.json", manifest)
            binding = {
                "schema_version": 1,
                "task_id": "ARB-import-fixture",
                "original_query_sha256": original,
                "record_sha256": record_sha,
                "pack_sha256": pack_sha,
                "public_sample_sha256": sample_sha,
                "manifest_sha256": manifest_sha,
                "source_root": str(source),
                "repository_commit": "a" * 40,
            }
            binding_path, _ = write("binding.json", binding)
            output = io.StringIO()
            with redirect_stdout(output):
                code = main(
                    [
                        "--record",
                        str(record_path),
                        "--pack",
                        str(pack_path),
                        "--public-sample",
                        str(sample_path),
                        "--manifest",
                        str(manifest_path),
                        "--source-root",
                        str(source),
                        "--binding",
                        str(binding_path),
                        "--lane",
                        "both",
                    ]
                )
            self.assertEqual(code, 0)
            result = json.loads(output.getvalue())
            self.assertEqual(result["lanes"]["imports"]["status"], "no_added_import")
            self.assertEqual(result["lanes"]["go_field_owner"]["candidates"][0]["path"], "gin.go")
            binding["original_query_sha256"] = "0" * 64
            write("binding.json", binding)
            with self.assertRaisesRegex(DiagnosticRefusal, "original query digest mismatch"):
                run_opt_in_candidate_lanes(
                    record_path, pack_path, sample_path, manifest_path, source, binding_path, "both"
                )
            binding["original_query_sha256"] = original
            sample["gold"] = {"files": ["gin.go"]}
            _, binding["public_sample_sha256"] = write("public-sample.json", sample)
            write("binding.json", binding)
            with self.assertRaisesRegex(DiagnosticRefusal, "blind-projected"):
                run_opt_in_candidate_lanes(
                    record_path, pack_path, sample_path, manifest_path, source, binding_path, "both"
                )


if __name__ == "__main__":
    unittest.main()
