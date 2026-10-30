"""Native request-time index of the literal and final-segment rules in a frozen map."""


class FrozenMap:
    literals: dict[str, int]
    segments: dict[str, int]

    def __init__(self) -> None:
        self.literals = {}
        self.segments = {}

    def add(self, method: str, prefix: str, capture: str, index: int) -> None:
        key = method + " " + prefix
        if capture:
            self.segments.setdefault(key, index)
        else:
            self.literals.setdefault(key, index)

    def match(self, method: str, path: str) -> tuple[int, str]:
        key = method + " " + path
        index = self.literals.get(key, -1)
        if index >= 0:
            return index, ""
        pos = len(path)
        while pos > 0 and path[pos-1] != "/":
            pos -= 1
        if pos == 0 or pos == len(path):
            return -1, ""
        index = self.segments.get(method + " " + path[:pos], -1)
        return index, path[pos:]
