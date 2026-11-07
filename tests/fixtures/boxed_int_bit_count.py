import sys


def check():
    class Number(int):
        def bit_count(self):
            return 42

        def __int__(self):
            raise AssertionError("unexpected conversion")

    assert sys.maxsize.bit_count() == sys.maxsize.bit_length()
    assert [n.bit_count() for n in (0, True, False, 7, -7, 1 << 400)] == [0, 1, 0, 3, 3, 1]
    assert Number(-9).bit_count() == 42
    assert int.bit_count(Number(-9)) == 2
    assert bool.bit_count is int.bit_count
    for call in (
        lambda: int.bit_count(),
        lambda: int.bit_count("seven"),
        lambda: (7).bit_count(1),
        lambda: (7).bit_count(value=1),
    ):
        try:
            call()
        except TypeError as error:
            print(str(error))
        else:
            raise AssertionError("invalid bit_count call accepted")


check()
