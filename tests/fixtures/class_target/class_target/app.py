class MyCls:
    """A minimal leaf class used as a call target."""

    def __init__(self, name: str) -> None:
        self.name = name


def main() -> MyCls:
    return MyCls("hello")
