#!/usr/bin/env python3
"""Run a configured linter and check for changes to its selected inputs."""

from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path

from pre_commit.all_languages import languages
from pre_commit.clientlib import load_config
from pre_commit.commands.run import Classifier, _get_skips
from pre_commit.error_handler import error_handler
from pre_commit.repository import all_hooks, install_hook_envs
from pre_commit.store import Store

CONFIG = Path(".pre-commit-config.yaml")
ACTIONLINT_CONFIGS = (Path(".github/actionlint.yaml"), Path(".github/actionlint.yml"))


def read_optional(path: Path) -> bytes | None:
    if not path.is_file():
        return None
    try:
        return path.read_bytes()
    except FileNotFoundError:
        return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("hook", choices=("actionlint", "shellcheck"))
    parser.add_argument("files", nargs="+")
    args = parser.parse_args()

    config_bytes = CONFIG.read_bytes()
    config = load_config(str(CONFIG))
    store = Store()
    hooks = [
        hook
        for hook in all_hooks(config, store)
        if hook.id == args.hook and "pre-commit" in hook.stages
    ]
    if len(hooks) != 1:
        parser.error(f"expected one configured pre-commit hook named {args.hook!r}")
    (hook,) = hooks
    classifier = Classifier.from_config(args.files, config["files"], config["exclude"])
    filenames = tuple(classifier.filenames_for_hook(hook))
    skips = _get_skips(os.environ)
    if hook.id in skips or hook.alias in skips:
        print(f"{hook.name} skipped")
        return 0
    if not filenames and not hook.always_run:
        print(f"{hook.name} found no eligible lint inputs", file=sys.stderr)
        return 1

    # Pre-commit compares the whole Git diff even when given an explicit file list.
    inputs: dict[Path, bytes | None] = {CONFIG: config_bytes}
    inputs.update((path, path.read_bytes()) for path in map(Path, filenames))
    if args.hook == "actionlint":
        inputs.update((path, read_optional(path)) for path in ACTIONLINT_CONFIGS)
    install_hook_envs(hooks, store)
    os.environ["PRE_COMMIT"] = "1"
    language = languages[hook.language]
    with language.in_env(hook.prefix, hook.language_version):
        status, output = language.run_hook(
            hook.prefix,
            hook.entry,
            hook.args,
            filenames if hook.pass_filenames else (),
            is_local=hook.src == "local",
            require_serial=hook.require_serial,
            color=False,
        )
    sys.stdout.buffer.write(output)

    changed = [path for path, original in inputs.items() if read_optional(path) != original]
    for path in changed:
        print(f"Lint input changed while checking {hook.id} — {path}", file=sys.stderr)
    failed = bool(status or changed)
    print(f"{hook.name} {'failed' if failed else 'passed'}")
    return int(failed)


if __name__ == "__main__":
    with error_handler():
        raise SystemExit(main())
