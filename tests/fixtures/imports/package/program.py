from . import operations as arithmetic
from .nested.labels import value


def main() -> None:
    print(__name__)
    print(arithmetic.label())
    print(value(arithmetic.twice(3)))


if __name__ == "__main__":
    main()
