from relar.json import decode, encode


def computed_value(value: int) -> int:
    print("constructor")
    return value + 1


class Computed:
    value: int
    label: str

    def __init__(self, value: int) -> None:
        self.value = computed_value(value)
        self.label = "computed"


def ascii_option(log: list[int], values: list[str]) -> bool:
    log.append(1)
    values[0] = "changed: é"
    return False


def indent_option(log: list[int]) -> int:
    log.append(2)
    return 2


def remove_record(values: list[Computed]) -> int:
    values.pop()
    return 2


def main() -> None:
    text = '\x00\x08\x0c\n\r\t"\\/\x1f\x7fé𐀀\U0010ffff'
    print(encode(text))
    print(encode(text, ensure_ascii=False))
    print(encode(None))
    print(encode(True))
    print(encode(False))
    print(encode(Computed(40), indent=2))
    sole_owner = [Computed(4)]
    print(encode(sole_owner[0], indent=remove_record(sole_owner)))
    print(len(sole_owner))
    values = ["original"]
    log: list[int] = []
    print(encode(values, ensure_ascii=ascii_option(log, values), indent=indent_option(log)))
    for event in log:
        print(event)
    nested: dict[str, list[int]] = {"second": [2, 3], "first": []}
    for indent in [-2, 0, 1, 2]:
        print(encode(nested, indent=indent))
    print(encode([0.0, -0.0, 0.1, 1e-5, 1e-6, 1e16, 900719925474099.25, 5e-324]))
    for invalid in ["nan", "inf", "-inf"]:
        try:
            encode([float(invalid)])
        except ValueError:
            print("nonfinite")
    try:
        encode(nested, indent=1000000000000000000000000000000)
    except OverflowError:
        print("indent overflow")
    number = 1
    for _ in range(5000):
        number = number * 10
    encoded = encode([number, -number], indent=2)
    restored = decode(encoded, list[int])
    print(len(encoded))
    print(restored[0] == number)
    print(restored[1] == -number)


main()
