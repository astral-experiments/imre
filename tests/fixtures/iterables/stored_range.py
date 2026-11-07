from typing import Final

VERSIONS: Final = range(8, 16)


def collect(bounds: range) -> list[int]:
    return [value for value in bounds]


def main() -> None:
    print(collect(VERSIONS))
    left = iter(VERSIONS)
    right = iter(VERSIONS)
    print(next(left))
    print(next(left))
    print(next(right))
    print(collect(VERSIONS))
    try:
        invalid = range(1, 2, 0)
        print("constructed")
    except ValueError as error:
        print(error)


try:
    print(next(range(1, stop=3)))
except TypeError as error:
    print(error)

main()
