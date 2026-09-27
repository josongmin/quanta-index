"""Exercise the L4 receipt gate against isolated missing and forged inputs."""

import hashlib
import json
import pathlib
import shutil
import subprocess
import sys
import tempfile


FOLDER = pathlib.Path(__file__).resolve().parent
LABELS = ("format", "owner", "sdk", "owner-lint", "sdk-lint", "adversarial", "regex-properties")


def run_case(name, mutate=None):
    with tempfile.TemporaryDirectory(prefix="l4-receipt-negative-") as temp:
        root = pathlib.Path(temp)
        for filename in ("snapshot.json", "verify_receipts.py"):
            shutil.copy2(FOLDER / filename, root / filename)
        for label in LABELS:
            for suffix in ("json", "log"):
                shutil.copy2(FOLDER / f"{label}.{suffix}", root / f"{label}.{suffix}")
        if mutate:
            mutate(root)
        result = subprocess.run(
            [sys.executable, str(root / "verify_receipts.py")],
            cwd=root,
            text=True,
            capture_output=True,
            check=False,
        )
        report = json.loads((root / "validation.json").read_text())
        return {
            "case": name,
            "exit_code": result.returncode,
            "status": report["status"],
            "errors": report["errors"],
            "check_errors": {item["label"]: item["errors"] for item in report["checks"] if item["errors"]},
        }


def change_json(root, filename, change):
    path = root / filename
    value = json.loads(path.read_text())
    change(value)
    path.write_text(json.dumps(value, indent=2) + "\n")


def zero_test_selection(root):
    log_path = root / "owner.log"
    log_path.write_text("test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n")
    def change(receipt):
        receipt["log_sha256"] = hashlib.sha256(log_path.read_bytes()).hexdigest()
        receipt["test_summaries"] = [log_path.read_text().strip()]
    change_json(root, "owner.json", change)


def main():
    cases = [
        run_case("unaltered"),
        run_case("missing_receipt", lambda root: (root / "sdk.json").unlink()),
        run_case("wrong_source", lambda root: change_json(
            root, "sdk.json", lambda receipt: receipt.__setitem__("source_sha256", "0" * 64)
        )),
        run_case("raw_log_tamper", lambda root: (root / "sdk.log").write_text("tampered\n")),
        run_case("zero_selected", zero_test_selection),
        run_case("wrong_command", lambda root: change_json(
            root, "owner.json", lambda receipt: receipt.__setitem__("command", ["true"])
        )),
        run_case("changed_binary", lambda root: change_json(
            root, "sdk.json", lambda receipt: receipt["binaries"][0].__setitem__("sha256", "0" * 64)
        )),
        run_case("wrong_external", lambda root: change_json(
            root, "sdk.json", lambda receipt: receipt.__setitem__("external_inputs_after", {})
        )),
    ]
    valid = cases[0]["exit_code"] == 0 and cases[0]["status"] == "VERIFIED"
    rejected = all(case["exit_code"] != 0 and case["status"] == "BLOCKED" for case in cases[1:])
    report = {"status": "VERIFIED" if valid and rejected else "FAILED", "cases": cases}
    (FOLDER / "validator-negative.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"status": report["status"], "cases": [(case["case"], case["status"]) for case in cases]}))
    return 0 if valid and rejected else 1


if __name__ == "__main__":
    sys.exit(main())
