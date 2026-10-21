class Record:
    name: str
    count: int

    def __init__(self, name: str, /, *, count: int = 7) -> None:
        self.name = name
        self.count = count
