"""Require Go and TypeScript projection bodies in the pinned CI Python rail."""

from __future__ import annotations

import sys
import xml.etree.ElementTree as ET
from pathlib import Path

EXPECTED_MODULES = ("test_go_projection", "test_ts_projection")


def validate_junit(report: bytes) -> dict[str, int]:
    try:
        root = ET.fromstring(report)
    except ET.ParseError as error:
        raise ValueError("projection CI JUnit XML is malformed") from error
    counts = {module: 0 for module in EXPECTED_MODULES}
    for case in root.iter("testcase"):
        classname = case.get("classname", "")
        for module in EXPECTED_MODULES:
            if classname.split(".")[-1] == module:
                if case.find("skipped") is not None:
                    raise ValueError(f"{module} was skipped in pinned CI")
                if case.find("failure") is not None or case.find("error") is not None:
                    raise ValueError(f"{module} failed in pinned CI")
                counts[module] += 1
    missing = [module for module, count in counts.items() if count == 0]
    if missing:
        raise ValueError(f"projection CI bodies missing: {', '.join(missing)}")
    return counts


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: verify_parser_projection_ci.py JUNIT_XML")
    counts = validate_junit(Path(sys.argv[1]).read_bytes())
    print(
        "pinned projection CI bodies: "
        + ", ".join(f"{name}={count}" for name, count in counts.items())
    )


if __name__ == "__main__":
    main()
