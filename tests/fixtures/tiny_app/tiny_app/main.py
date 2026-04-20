import os

from tiny_app.lib import foo


def main() -> str:
    value = foo()
    return os.environ.get("X", value)
