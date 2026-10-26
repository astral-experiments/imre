from collections.abc import Iterator


class Numbers:
    def items(self, value: int) -> Iterator[int]:
        print("items body")
        yield from [value]

    def values(self) -> Iterator[int]:
        print("values body")
        yield from [1, 2]

    @staticmethod
    def glob(value: int) -> Iterator[int]:
        print("glob body")
        yield from [value]


def argument() -> int:
    print("argument")
    return 4


numbers = Numbers()
empty: list[int] = []
for pair in zip(empty, numbers.items(argument())):
    print(pair)
for value in Numbers.items(numbers, argument()):
    print(value)
print(sum(numbers.values(), start=argument()))
print(list(Numbers.glob(argument())))
