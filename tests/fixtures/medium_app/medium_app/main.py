from medium_app.a import run_a
from medium_app.b import run_b


def main() -> str:
    x = run_a()
    y = run_b()
    return x + y


def recur() -> None:
    recur()
