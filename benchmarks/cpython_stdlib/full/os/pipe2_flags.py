"""Linux pipe2 argument handling and the flags on both real descriptors."""
import os
import posix


def show(pair: tuple[int, int]) -> None:
    reader, writer = pair
    try:
        print(os.get_inheritable(reader))
        print(os.get_inheritable(writer))
        print(os.get_blocking(reader))
        print(os.get_blocking(writer))
    finally:
        os.close(reader)
        os.close(writer)


class Indexed:
    def __index__(self) -> int:
        print("index")
        return os.O_NONBLOCK


class BadIndex:
    def __index__(self) -> str:
        print("bad index")
        return "bad"


class FailingIndex:
    def __index__(self) -> int:
        raise ValueError("from index")


def evaluated(n: int) -> int:
    print("evaluated")
    print(n)
    return n


show(os.pipe())
show(os.pipe2(0))
show(posix.pipe2(False))
show(os.pipe2(os.O_NONBLOCK))
show(posix.pipe2(os.O_CLOEXEC))
show(os.pipe2(os.O_CLOEXEC | os.O_NONBLOCK))
show(posix.pipe2(Indexed()))
show(os.pipe2(*(0,)))
show(posix.pipe2(*(Indexed(),)))
for flags in (-1, 1, 2147483647, 2147483648, -2147483649, 1 << 100):
    try:
        show(os.pipe2(flags))
    except OverflowError as err:
        print("OverflowError")
        print(str(err))
    except OSError as err:
        print("OSError")
        print(str(err))
try:
    show(posix.pipe2("DEADBEEF"))
except TypeError as err:
    print("TypeError")
    print(str(err))
try:
    show(posix.pipe2(BadIndex()))
except TypeError as err:
    print("TypeError")
    print(str(err))
try:
    show(posix.pipe2(FailingIndex()))
except ValueError as err:
    print("ValueError")
    print(str(err))
try:
    show(os.pipe2())
except TypeError as err:
    print("TypeError")
    print(str(err))
try:
    show(os.pipe2(evaluated(1), evaluated(2)))
except TypeError as err:
    print("TypeError")
    print(str(err))
try:
    show(os.pipe2(flags=evaluated(3)))
except TypeError as err:
    print("TypeError")
    print(str(err))
try:
    show(posix.pipe2(*(evaluated(4), evaluated(5))))
except TypeError as err:
    print("TypeError")
    print(str(err))
noargs = ()
try:
    show(os.pipe2(*noargs))
except TypeError as err:
    print("TypeError")
    print(str(err))
try:
    os.getuid(*(evaluated(6),))
except TypeError as err:
    print("TypeError")
    print(str(err))
