"""Overflow-safe finite JSON scalar admission; never coerces bool or null."""

from __future__ import annotations

import math
import sys


def is_finite_json_number(value: object) -> bool:
    """Accept only exact int/float scalars representable as finite binary64.

    Compare arbitrary-size integers before math/float conversion. Python's
    math.isfinite converts integers to float and raises on JSON integers such
    as 10**400, which would bypass a caller's typed evidence refusal.
    """
    return (
        type(value) in (int, float)
        and -sys.float_info.max <= value <= sys.float_info.max
        and math.isfinite(value)
    )
