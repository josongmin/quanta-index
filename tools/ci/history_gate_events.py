"""Require every named History owner test in a Nextest terminal stream."""

from __future__ import annotations

import sys

from nextest_events import NextestEvidenceError, parse_nextest_bytes


def main() -> int:
    package, kind, *cases = sys.argv[1:]
    expected = {}
    for case in cases:
        binary, name = case.split(":", 1)
        expected[f"{package}::{binary}${name}"] = (package, binary, kind)
    if len(expected) != len(cases) or not expected:
        sys.exit("Native History gate: duplicate or empty test roster")
    try:
        result = parse_nextest_bytes(sys.stdin.buffer.read(), expected)
    except NextestEvidenceError as error:
        sys.exit(f"Native History gate: {error}")
    label = "Native History" if kind == "test" else f"Native History {package}"
    print(f"{label}: {result.passed}/{len(expected)} passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
