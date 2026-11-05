import math
import struct
from math import fsum


def current():
    assert fsum is math.fsum
    total = fsum(iter((1e16, 1.25, -1e16)))
    assert type(total) is float and total == 1.25
    assert fsum([2.0**53, 1.0, 2.0**-100]) == 2.0**53 + 2.0
    assert math.copysign(1.0, fsum([-0.0])) == 1.0
    assert fsum([]) == 0.0
    assert fsum([math.inf, 1.0]) == math.inf
    assert math.isnan(fsum([math.nan, math.inf]))

    # Keep NaN sign, payload, and signaling status observable across runtimes.
    payloads = struct.unpack(">dd", bytes.fromhex("7ff8000000004321fff0000000000001"))
    for values in ([payloads[1]], payloads, tuple(reversed(payloads))):
        print(struct.pack(">d", fsum(values)).hex())

    interrupted = map(float, (math.inf, -math.inf, "stop"))
    for values, kind, message in (
        ([math.inf, -math.inf], ValueError, "-inf + inf in fsum"),
        (interrupted, ValueError, "could not convert string to float: 'stop'"),
        ([math.inf, -math.inf, "bad"], TypeError, "must be real number, not str"),
        ([1e308, 1e308, "bad"], OverflowError, "intermediate overflow in fsum"),
    ):
        try:
            fsum(values)
        except kind as exc:
            assert str(exc) == message
        else:
            raise AssertionError(f"expected {kind.__name__}")
    print(total)


if __name__ == "__main__":
    current()
