"""A toy @cached decorator. Heavily decorated wrappers are known to confuse
goto-definition — this file is intentionally thin so we can observe how ty
handles the indirection.
"""

from __future__ import annotations

from functools import wraps


def cached(fn):
    """Wrap ``fn`` in a tiny memo cache keyed by positional args."""
    memo: dict[tuple, object] = {}

    @wraps(fn)
    def wrapper(*args):
        key = args
        if key not in memo:
            memo[key] = fn(*args)
        return memo[key]

    return wrapper
