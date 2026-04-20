"""Shared helper at the bottom of the diamond."""

from __future__ import annotations


def shared_value() -> int:
    """Return the diamond's shared base value."""
    return _base_value() + _offset()


def _base_value() -> int:
    """The literal base number."""
    return 40


def _offset() -> int:
    """The literal offset added to the base."""
    return 2
