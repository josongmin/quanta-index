"""Report frozen proof integrity separately from the changing live checkout."""

import datetime
import hashlib
import json
import pathlib
import subprocess


FOLDER = pathlib.Path(__file__).resolve().parent
SNAPSHOT = json.loads((FOLDER / "snapshot.json").read_text())
ORIGIN = pathlib.Path(SNAPSHOT["origin"])
ROOT = pathlib.Path(SNAPSHOT["root"])
SELECTED = [
    "crates/quanta-index-lexical/src/searcher/snippets.rs",
    "crates/quanta-index-lexical/src/searcher/preview_types.rs",
    "crates/quanta-index-lexical/src/searcher/match_sets.rs",
    "crates/quanta-index-lexical/src/searcher/candidates.rs",
    "crates/quanta-index-lexical/tests/l4_match_anchored_preview.rs",
    "crates/quanta-index-lexical/tests/regex_cache_bounds.rs",
    "crates/quanta-index-lexical/tests/cancellation_inside_search.rs",
    "crates/quanta-index-lexical/tests/execution_budget.rs",
    "crates/quanta-index-lexical/tests/unicode_normalization_goldens.rs",
    "crates/quanta-index-lexical/tests/regex_literal_alternation.rs",
    "crates/quanta-index-lq-regex/src/executor.rs",
    "crates/quanta-index-lq-text-normalizer/src/provenance.rs",
    "crates/quanta-index-searchd-runtime/tests/l4_preview_sdk.rs",
]


def digest(path):
    try:
        return hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError:
        return None


def git(*arguments):
    return subprocess.check_output(["git", *arguments], cwd=ORIGIN, text=True).strip()


def main():
    head_before = git("rev-parse", "HEAD")
    live_paths = set(git("ls-files", "--cached", "--others", "--exclude-standard").splitlines())
    live_paths = {name for name in live_paths if not name.startswith("docs/plans/")}
    source_drift = {
        name: {"frozen": expected, "live": actual}
        for name, expected in SNAPSHOT["manifest"].items()
        if (actual := digest(ORIGIN / name)) != expected
    }
    frozen_drift = [name for name, expected in SNAPSHOT["manifest"].items()
                    if digest(ROOT / name) != expected]
    external_drift = [name for name, expected in SNAPSHOT["external_dependency_sha256"].items()
                      if digest(pathlib.Path(name)) != expected]
    selected = {name: {"frozen": SNAPSHOT["manifest"][name], "live": digest(ORIGIN / name)}
                for name in SELECTED}
    artifacts = {path.name: digest(path) for path in sorted(FOLDER.iterdir())
                 if path.is_file() and path.suffix in (".json", ".py", ".tsv")
                 and path.name != "closeout.json"}
    head_after = git("rev-parse", "HEAD")
    report = {
        "observed_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "frozen_head": SNAPSHOT["head_before"],
        "source_sha256": SNAPSHOT["source_sha256"],
        "frozen_source_changed": frozen_drift,
        "external_inputs_changed": external_drift,
        "head_before": head_before,
        "head_after": head_after,
        "dirty_state": git("status", "--short"),
        "live_source_drift": source_drift,
        "new_live_non_plan_paths": sorted(live_paths - SNAPSHOT["manifest"].keys()),
        "selected_input_sha256": selected,
        "selected_inputs_unchanged": all(row["frozen"] == row["live"] for row in selected.values()),
        "artifacts_sha256": artifacts,
        "whole_live_checkout_qualification": "NOT_RUN",
    }
    (FOLDER / "closeout.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"frozen_source_changed": frozen_drift,
                      "external_inputs_changed": external_drift,
                      "live_drift_count": len(source_drift),
                      "new_live_paths": report["new_live_non_plan_paths"],
                      "selected_inputs_unchanged": report["selected_inputs_unchanged"],
                      "head_before": head_before, "head_after": head_after}, indent=2))
    return 0 if not frozen_drift and not external_drift else 1


if __name__ == "__main__":
    raise SystemExit(main())
