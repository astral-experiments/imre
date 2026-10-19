from library import TABLE as shared


def read(key: str) -> int:
    return shared[key]
