"""Core service class with inheritance.

ExtendedService subclasses Service and calls super().__init__() — a common
pattern we want the walker to traverse.
"""

from __future__ import annotations


def _normalize(item: str) -> str:
    """Lower-case the item; a plain helper reached through ``process``."""
    return item.strip().lower()


def _validate(item: str) -> bool:
    """Return True when the item is non-empty after normalization."""
    normalized = _normalize(item)
    return bool(normalized)


class Service:
    """A minimal service with a public and private helper."""

    def __init__(self, name: str) -> None:
        self.name = name
        self._count = 0

    def process(self, items: list[str]) -> int:
        """Process items and return how many were processed."""
        for item in items:
            if _validate(item):
                self._private_helper(item)
        return self._count

    def _private_helper(self, item: str) -> None:
        self._count += 1
        _ = _normalize(item)


class ExtendedService(Service):
    """Subclass to exercise super() traversal."""

    def __init__(self, name: str) -> None:
        super().__init__(name)
        self.extended = True

    def process(self, items: list[str]) -> int:
        """Delegate to the base implementation."""
        return super().process(items)
