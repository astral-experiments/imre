class CountError(Exception):
    def __init__(self, value: int) -> None:
        super().__init__("count")
        self._count: int = value

    @property
    def count(self) -> int:
        print("get", self._count)
        return self._count

    @count.setter
    def count(self, value: int) -> None:
        print("set", value)
        self._count = value


class Child(CountError):
    pass


def set_count(error: CountError) -> None:
    error.count = 3


def update(error: Child) -> None:
    set_count(error)
    error.count += 2


error = Child(1)
update(error)
print(error.count)
print(error.args)
