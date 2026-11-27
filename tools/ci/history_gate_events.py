"""Require every named History owner test in a Nextest terminal stream."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from nextest_events import NextestEvidenceError, parse_nextest_bytes, parse_nextest_inventory


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inventory", type=Path)
    parser.add_argument("package")
    parser.add_argument("kind", choices=("test", "lib"))
    parser.add_argument("cases", nargs="+")
    args = parser.parse_args()
    expected = {}
    for case in args.cases:
        binary, name = case.split(":", 1)
        expected[f"{args.package}::{binary}${name}"] = (args.package, binary, args.kind)
    if len(expected) != len(args.cases):
        sys.exit("Native History gate: duplicate or empty test roster")
    try:
        inventory = parse_nextest_inventory(args.inventory) if args.inventory else None
        if inventory is not None and dict(inventory) != expected:
            raise NextestEvidenceError("nextest collection differs from the required History tests")
        result = parse_nextest_bytes(sys.stdin.buffer.read(), inventory or expected)
    except NextestEvidenceError as error:
        sys.exit(f"Native History gate: {error}")
    label = "Native History" if args.kind == "test" else f"Native History {args.package}"
    print(f"{label}: {result.passed}/{len(expected)} passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
