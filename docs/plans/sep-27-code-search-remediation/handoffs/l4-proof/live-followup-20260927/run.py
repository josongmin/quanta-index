"""Capture one current-checkout L4 command without promoting drifting inputs."""

import datetime
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys


folder = pathlib.Path(__file__).resolve().parent
root = pathlib.Path(__file__).resolve().parents[6]
prior = folder.parent / "further-audit-20260927" / "snapshot.json"
manifest = json.loads(prior.read_text())
label, *command = sys.argv[1:]


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def inputs():
    return {path: sha(root / path) for path in manifest["manifest"]}


def external():
    return {path: sha(pathlib.Path(path)) for path in manifest["external_dependency_inputs"]}


def git(*args):
    return subprocess.check_output(["git", *args], cwd=root).decode().strip()


before = inputs()
external_before = external()
head = git("rev-parse", "HEAD")
dirty = git("status", "--porcelain=v1")
env = os.environ.copy()
env.update({"CARGO_BUILD_JOBS": "2", "QUANTA_INDEX_RESOURCE_WAIT_SECONDS": "1800"})
for key in ("CARGO_TARGET_DIR", "QUANTA_INDEX_PRESERVE_CARGO_TARGET_DIR", "QUANTA_INDEX_BUILD_LANE"):
    env.pop(key, None)
started = datetime.datetime.now(datetime.timezone.utc).isoformat()
log = folder / f"{label}.log"
with log.open("w") as stream:
    result = subprocess.run(command, cwd=root, env=env, stdout=stream, stderr=subprocess.STDOUT)
after = inputs()
external_after = external()
output = log.read_text()
receipt = {
    "started_utc": started,
    "ended_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "head_before": head,
    "head_after": git("rev-parse", "HEAD"),
    "dirty_before": dirty,
    "dirty_after": git("status", "--porcelain=v1"),
    "cwd": str(root),
    "command": command,
    "environment_overrides": {"CARGO_BUILD_JOBS": "2", "QUANTA_INDEX_RESOURCE_WAIT_SECONDS": "1800"},
    "rustc": subprocess.check_output(["rustc", "--version", "--verbose"]).decode(),
    "exit_code": result.returncode,
    "source_sha256": hashlib.sha256(json.dumps(before, sort_keys=True).encode()).hexdigest(),
    "inputs_before": before,
    "inputs_after": after,
    "external_inputs_before": external_before,
    "external_inputs_after": external_after,
    "inputs_unchanged": before == after and external_before == external_after,
    "test_summaries": re.findall(r"test result:.*", output),
    "log": str(log),
    "log_sha256": sha(log),
}
(folder / f"{label}.json").write_text(json.dumps(receipt, indent=2) + "\n")
print(json.dumps({key: receipt[key] for key in ("exit_code", "source_sha256", "inputs_unchanged", "test_summaries", "log_sha256")}))
sys.exit(result.returncode or (2 if not receipt["inputs_unchanged"] else 0))
