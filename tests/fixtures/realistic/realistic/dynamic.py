"""Intentional dynamic dispatch via getattr — should surface as unresolved."""

from __future__ import annotations


def dispatch(target: object, method_name: str) -> object:
    """Call ``target.method_name()`` via getattr — unresolved by design."""
    return getattr(target, method_name)([])
