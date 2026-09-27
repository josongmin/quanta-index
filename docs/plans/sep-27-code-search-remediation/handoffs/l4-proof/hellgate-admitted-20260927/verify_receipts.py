"""Reject stale, partial, or zero-selection L4 hell-gate receipts."""

import hashlib
import json
import pathlib
import re
import sys


FOLDER = pathlib.Path(__file__).resolve().parent
SNAPSHOT = json.loads((FOLDER / "snapshot.json").read_text())
EXPECTED = {
    "format": (["./scripts/cargow", "fmt", "--all", "--", "--check"], "fmt", []),
    "owner": (
        ["./scripts/cargow", "test", "-p", "quanta-index-lexical", "-p",
         "quanta-index-lq-regex", "-p", "quanta-index-lq-text-normalizer",
         "--lib", "--test", "l4_match_anchored_preview", "--locked"],
        "test",
        [
            "every_positive_match_in_the_emitted_window_has_a_typed_span",
            "l4_unobserved_capture_removal_preserves_reference_ranges",
            "indexed_and_manual_preserve_overlapping_raw_witnesses",
        ],
    ),
    "sdk": (["./scripts/cargow", "test", "-p", "quanta-index-searchd-runtime",
             "--test", "l4_preview_sdk", "--locked"], "test",
            ["l4_sdk_preview_uses_matcher_ranges_and_original_source_bytes",
             "l4_sdk_preview_survives_daemon_process_restart"]),
    "owner-lint": (["./scripts/cargow", "clippy", "-p", "quanta-index-lexical",
                    "-p", "quanta-index-lq-regex", "--lib", "--test",
                    "l4_match_anchored_preview", "--locked", "--no-deps"], "clippy", []),
    "sdk-lint": (["./scripts/cargow", "clippy", "-p", "quanta-index-searchd-runtime",
                  "--test", "l4_preview_sdk", "--locked", "--no-deps"], "clippy", []),
    "adversarial": (
        ["./scripts/cargow", "test", "-p", "quanta-index-lexical", "--test",
         "regex_cache_bounds", "--test", "cancellation_inside_search", "--test",
         "execution_budget", "--test", "unicode_normalization_goldens", "--test",
         "regex_literal_alternation", "--locked"],
        "test",
        ["a_cancelled_budget_is_refused_before_cold_or_warm_regex_execution",
         "resident_bytes_never_exceed_the_policy_under_many_distinct_regexes",
         "golden_table_holds_on_both_routes"],
    ),
    "regex-properties": (
        ["env", "PROPTEST_RNG_SEED=20260927", "./scripts/cargow", "test", "-p", "quanta-index-lq-regex", "--test",
         "golden_regex", "--test", "property_ast_walk", "--test",
         "property_dialect_idempotent", "--locked"],
        "test",
        ["safe_patterns_accepted", "injected_possessive_is_caught",
         "parse_implies_dialect_ok"],
    ),
}
SUMMARY = re.compile(
    r"^test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; "
    r"\d+ measured; (\d+) filtered out;.*$",
    re.MULTILINE,
)
TEST_LINE = re.compile(r"^test \S+ \.\.\. ok$", re.MULTILINE)
SELECTED = re.compile(r"^running (\d+) tests?$", re.MULTILINE)


def digest(path):
    try:
        return hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError:
        return None


def validate(label, command, kind, expected_names):
    errors = []
    receipt_path = FOLDER / f"{label}.json"
    log_path = FOLDER / f"{label}.log"
    try:
        receipt = json.loads(receipt_path.read_text())
        log = log_path.read_text()
    except (OSError, ValueError) as error:
        return {"label": label, "command_exit_code": None, "command_executed": False,
                "errors": [f"missing or malformed receipt/log: {error}"]}
    if not isinstance(receipt, dict):
        return {"label": label, "command_exit_code": None, "command_executed": False,
                "errors": ["receipt is not an object"]}
    if receipt.get("status") != "VERIFIED" or receipt.get("exit_code") != 0:
        errors.append("command did not finish verified with exit 0")
    if receipt.get("inputs_unchanged") is not True or receipt.get("matches_frozen_snapshot") is not True:
        errors.append("runner did not bind unchanged frozen inputs")
    if receipt.get("command_executed") is not True:
        errors.append("command did not execute")
    if receipt.get("command") != command:
        errors.append("wrong command or target selection")
    if receipt.get("cwd") != SNAPSHOT["root"]:
        errors.append("wrong source root")
    if receipt.get("inputs_before") != SNAPSHOT["manifest"]:
        errors.append("pre-run source differs from frozen manifest")
    if receipt.get("inputs_after") != SNAPSHOT["manifest"]:
        errors.append("post-run source differs from frozen manifest")
    for field in ("external_inputs_before", "external_inputs_after"):
        if receipt.get(field) != SNAPSHOT["external_dependency_sha256"]:
            errors.append(f"{field} differs from frozen external inputs")
    if receipt.get("source_sha256") != SNAPSHOT["source_sha256"]:
        errors.append("wrong source digest")
    if receipt.get("log_sha256") != digest(log_path):
        errors.append("log digest mismatch")
    recorded_summaries = receipt.get("test_summaries")
    if not isinstance(recorded_summaries, list) or recorded_summaries != re.findall(r"test result:.*", log):
        errors.append("summary differs from raw log")
    summaries = SUMMARY.findall(log)
    executed = len(TEST_LINE.findall(log))
    passed = sum(int(row[0]) for row in summaries)
    if kind == "test":
        if not summaries or not isinstance(recorded_summaries, list) or len(summaries) != len(recorded_summaries):
            errors.append("missing or malformed test summary")
        if passed == 0 or executed != passed:
            errors.append("zero or mismatched selected/executed tests")
        selected = [int(count) for count in SELECTED.findall(log)]
        if selected != [int(row[0]) for row in summaries]:
            errors.append("selected suite counts differ from terminal pass counts")
        if any(tuple(row[1:]) != ("0", "0", "0") for row in summaries):
            errors.append("failed, ignored, or filtered tests")
        for name in expected_names:
            if not any(line.endswith(f"::{name} ... ok") or line == f"test {name} ... ok"
                       for line in TEST_LINE.findall(log)):
                errors.append(f"required regression did not execute: {name}")
    elif summaries or executed:
        errors.append("unexpected test output from non-test command")
    binaries = receipt.get("binaries")
    if not isinstance(binaries, list):
        errors.append("binary list missing or malformed")
        binaries = []
    for binary in binaries:
        if not isinstance(binary, dict) or not isinstance(binary.get("path"), str):
            errors.append("binary identity malformed")
        elif digest(pathlib.Path(binary["path"])) != binary.get("sha256"):
            errors.append(f"executed binary changed or missing: {binary['path']}")
    if kind == "test" and not binaries:
        errors.append("test binary identity missing")
    daemon_binary = None
    if label == "sdk":
        if log.count("searchd: umask set") != 2:
            errors.append("real daemon did not start twice")
        if binaries and isinstance(binaries[0], dict) and isinstance(binaries[0].get("path"), str):
            daemon_path = pathlib.Path(binaries[0]["path"]).parent.parent / "quanta-index-searchd"
            daemon_binary = {"path": str(daemon_path), "sha256": digest(daemon_path)}
            if daemon_binary["sha256"] is None:
                errors.append("real daemon binary missing")
    return {
        "label": label,
        "command_exit_code": receipt.get("exit_code"),
        "command_executed": receipt.get("command_executed"),
        "receipt_sha256": digest(receipt_path),
        "log_sha256": digest(log_path),
        "selected_executed_passed": passed if kind == "test" else None,
        "binary_sha256": binaries,
        "daemon_binary": daemon_binary,
        "errors": errors,
    }


def main():
    errors = []
    if SNAPSHOT.get("copy_matches_live_before_after") is not True:
        errors.append("source copy was not stable")
    manifest_digest = hashlib.sha256(
        json.dumps(SNAPSHOT["manifest"], sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    if manifest_digest != SNAPSHOT["source_sha256"]:
        errors.append("frozen source digest does not match its manifest")
    root = pathlib.Path(SNAPSHOT["root"])
    for name, expected in SNAPSHOT["manifest"].items():
        if digest(root / name) != expected:
            errors.append(f"frozen source changed: {name}")
    for name, expected in SNAPSHOT["external_dependency_sha256"].items():
        if digest(pathlib.Path(name)) != expected:
            errors.append(f"external input changed: {name}")
    checks = [validate(label, *expected) for label, expected in EXPECTED.items()]
    if any(check["errors"] for check in checks):
        errors.append("one or more command receipts failed validation")
    executed_failure = any(
        check["command_executed"] is True
        and check["command_exit_code"] not in (None, 0)
        for check in checks
    )
    report = {
        "status": "VERIFIED" if not errors else "FAILED" if executed_failure else "BLOCKED",
        "source_sha256": SNAPSHOT["source_sha256"],
        "validator_sha256": digest(pathlib.Path(__file__)),
        "checks": checks,
        "errors": errors,
    }
    (FOLDER / "validation.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"status": report["status"], "errors": errors,
                      "selected_counts": {row["label"]: row["selected_executed_passed"]
                                          for row in checks}}, indent=2))
    return 0 if not errors else 1


if __name__ == "__main__":
    sys.exit(main())
