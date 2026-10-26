from operator import attrgetter


class Request:
    def __init__(self, value: int):
        self.value = value


def read(key: str) -> int:
    get_name = attrgetter(key)
    return get_name(Request(7))
