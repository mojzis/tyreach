"""Right arm of the diamond — also calls shared_value."""

from __future__ import annotations

from .shared import shared_value


def via_b() -> int:
    """Right arm: fetch the shared value plus 2."""
    return shared_value() + 2
