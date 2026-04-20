"""CLI entry point for the realistic fixture.

Instantiates Service, calls a regular method and a decorated one, and touches
the diamond-inheritance module so the BFS reaches it.
"""

from .core import ExtendedService, Service
from .decorators import cached
from .diamond.a import combined
from .dynamic import dispatch
from .partial_usage import multiply_by_two


@cached
def greet(name: str) -> str:
    """Return a greeting (decorated with @cached)."""
    return f"hello, {name}"


def _banner(text: str) -> str:
    """Local helper so main reaches another internal function directly."""
    return f"=== {text} ==="


def main() -> int:
    """Kick off a short workflow that exercises several call patterns."""
    _banner("start")
    svc = Service(name="alpha")
    svc.process(["a", "b"])
    ext = ExtendedService(name="beta")
    ext.process(["c"])
    greet(svc.name)
    combined()
    dispatch(svc, "process")
    multiply_by_two(21)
    return 0
