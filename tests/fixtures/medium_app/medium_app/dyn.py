def caller(obj: object, name: str) -> object:
    return getattr(obj, name)()
