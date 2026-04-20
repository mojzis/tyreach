"""Left arm of the diamond — calls shared_value."""

from __future__ import annotations

from .b import via_b
from .shared import shared_value


def via_a() -> int:
    """Left arm: fetch the shared value plus 1."""
    return shared_value() + 1


def combined() -> int:
    """Combine both arms so both reach ``shared_value``."""
    return via_a() + via_b()
