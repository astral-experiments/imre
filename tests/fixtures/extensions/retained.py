import retainer


class Observer:
    items: list[int]

    def __init__(self, items: list[int]) -> None:
        self.items = items

    def changed(self, value: int) -> int:
        assert retainer.first() == value
        self.items.append(value + 1)
        return self.items[0] + self.items[-1]


def start(items: list[int]) -> None:
    retainer.remember(items, Observer(items).changed)


def failing(value: int) -> int:
    raise ValueError("callback failed")


def releasing(value: int) -> int:
    retainer.release()
    return value + 1


def main() -> None:
    assert retainer.buffer_bytes(b"a\x00\xff", 0) == b"a\x00\xff"
    assert retainer.buffer_bytes(b"", 1) == b""
    assert len(retainer.buffer_bytes(b"abcd", 1)) == 4
    assert retainer.buffer_bytes(b"", 4) == b"\x00\xff"
    try:
        retainer.buffer_bytes(b"", 2)
    except SystemError as error:
        assert str(error) == "Negative size passed to PyBytes_FromStringAndSize"
    else:
        raise AssertionError("expected a negative size error")
    try:
        retainer.buffer_bytes(b"", 3)
    except OverflowError as error:
        assert str(error) == "byte string is too large"
    else:
        raise AssertionError("expected a size overflow")

    items = [1]
    start(items)
    assert retainer.same(items) == 1
    assert retainer.same([1]) == 0
    items[0] = 10
    assert retainer.first() == 10
    assert retainer.replace_and_call(7) == 15
    print(f"{items[0]} {items[1]}")
    large = 100000000000000000000000000000000
    retainer.pin_args(large)
    assert retainer.replace_and_call(large) == 2 * large + 1
    assert retainer.pinned_value() == large

    retainer.remember(items, failing)
    try:
        retainer.replace_and_call(11)
    except ValueError as error:
        print(str(error))
    assert items[0] == 11

    retainer.remember(items, releasing)
    assert retainer.replace_and_call(19) == 20
    try:
        retainer.first()
    except RuntimeError as error:
        print(str(error))
    print(items[0])

    # Only C retains this list and bound receiver after start returns.
    start([2])
    assert retainer.replace_and_call(23) == 47
    # The module free hook releases these last references at shutdown.


if __name__ == "__main__":
    main()
