"""Cross-check clipping, omitted bounds, negative strides, and arbitrary integers."""


def same_pair(original: object, sliced: tuple[int, int]) -> bool:
    return original is sliced


def main() -> None:
    values = (1, 2)[:]
    print(values)
    print(same_pair(values, values[:]))
    print(same_pair(values, values[-99:100:1]))
    print(values[::2])
    print(values[::-1])
    bounds: list[int | None] = [
        None,
        -1000000000000000000000000000000,
        -9,
        -1,
        0,
        1,
        4,
        9,
        1000000000000000000000000000000,
    ]
    steps: list[int | None] = [
        None,
        -1000000000000000000000000000000,
        -3,
        -1,
        1,
        2,
        1000000000000000000000000000000,
    ]
    for text in ["", "a", "abc\x00xyz", "aé🌍\x00e\u0301"]:
        numbers = [index for index in range(len(text))]
        print(text[:])
        print(text[::])
        print(text[::-1])
        print(text[:-1:-1])
        print(text[None:None:None])
        print(len(numbers[:]))
        for first in bounds:
            for last in bounds:
                for stride in steps:
                    print(text[first:last:stride])
                    selected = numbers[first:last:stride]
                    encoded = ""
                    for number in selected:
                        encoded += str(number) + ","
                    print(encoded)
        for stride in [0]:
            try:
                _text = text[::stride]
            except ValueError:
                print("zero string stride")
            try:
                _numbers = numbers[::stride]
            except ValueError:
                print("zero list stride")


main()
