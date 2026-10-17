"""A closed module graph with aliases, package re-exports, and shared mutable values."""

import arithmetic
import arithmetic as numbers
import package.operations
import package.nested.labels as labels
from arithmetic import add as plus
from package import adjust, nested
from package.nested import labels as words
from package.operations import append_and_read


def main() -> None:
    print(__name__)
    print(arithmetic.label())
    print(package.operations.label())
    print(labels.label())
    print(words.value(8))
    print(nested.value(9))
    print(plus(numbers.add(2, 3), adjust(4)))
    values = [1000000000000000000000000000000]
    print(append_and_read(values, values, 7))
    print(len(values))
    print(values[1])


if __name__ == "__main__":
    main()
