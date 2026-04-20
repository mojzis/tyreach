"""functools.partial bindings — the wrapped callable is expected to be
unresolved by ty in v1.
"""

from functools import partial


def _multiply(a: int, b: int) -> int:
    return a * b


multiply_by_two = partial(_multiply, 2)
