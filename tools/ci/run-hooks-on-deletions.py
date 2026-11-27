#!/usr/bin/env python3
"""Run local repository-wide hooks when deletion removes their selected inputs."""

from __future__ import annotations

import argparse
import os
import shlex
import subprocess

from pre_commit import git
from pre_commit.clientlib import load_config
from pre_commit.commands.run import Classifier, _get_skips, filter_by_include_exclude
from pre_commit.error_handler import error_handler
from pre_commit.repository import all_hooks
from pre_commit.store import Store
from pre_commit.util import CalledProcessError, cmd_output


def deleted_files(from_ref: str | None, to_ref: str | None) -> list[str]:
    command = (
        "git",
        "diff",
        "--name-only",
        "--diff-filter=D",
        "--no-renames",
        "--no-ext-diff",
        "-z",
    )
    if from_ref and to_ref:
        try:
            _, output, _ = cmd_output(*command, f"{from_ref}...{to_ref}", "--")
        except CalledProcessError:
            _, output, _ = cmd_output(*command, f"{from_ref}..{to_ref}", "--")
    else:
        _, output, _ = cmd_output(*command, "--cached", "--")
    return git.zsplit(output)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", choices=("pre-commit", "pre-push"))
    args = parser.parse_args()

    from_ref, to_ref = os.getenv("PRE_COMMIT_FROM_REF"), os.getenv("PRE_COMMIT_TO_REF")
    if bool(from_ref) != bool(to_ref):
        parser.error("pre-commit requires both ref endpoints")
    if args.stage == "pre-push" and not from_ref:
        # Initial pushes already run against all files, with no older tree to diff.
        return 0
    deleted = deleted_files(from_ref, to_ref)
    if not deleted:
        return 0

    config = load_config(".pre-commit-config.yaml")
    deleted = tuple(filter_by_include_exclude(deleted, config["files"], config["exclude"]))
    existing = (
        git.get_changed_files(from_ref, to_ref) if from_ref and to_ref else git.get_staged_files()
    )
    classifier = Classifier.from_config(existing, config["files"], config["exclude"])
    skips = _get_skips(os.environ)
    failed = False
    for hook in all_hooks(config, Store()):
        if (
            hook.src != "local"
            or hook.language not in {"system", "unsupported"}
            or hook.pass_filenames
            or args.stage not in hook.stages
        ):
            continue
        if hook.id in skips or hook.alias in skips or hook.always_run:
            continue
        # Deleted contents are unavailable to identify's type classifier.
        if tuple(hook.types) != ("file",) or hook.types_or or hook.exclude_types:
            continue
        if not any(filter_by_include_exclude(deleted, hook.files, hook.exclude)):
            continue
        if any(classifier.filenames_for_hook(hook)):
            continue
        print(f"Running {hook.id} for deleted inputs", flush=True)
        result = subprocess.run([*shlex.split(hook.entry), *hook.args], check=False)
        if result.returncode != 0:
            failed = True
            if config["fail_fast"] or hook.fail_fast:
                break
    return int(failed)


if __name__ == "__main__":
    with error_handler():
        raise SystemExit(main())
