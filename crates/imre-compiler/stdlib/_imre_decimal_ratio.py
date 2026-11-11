# Copyright (c) 2004 Python Software Foundation.
# All rights reserved.
"""Finite coefficient operations from CPython 3.14.5 _pydecimal. See licenses/CPython.txt."""


def _coefficient_as_int(digits: str, modulus: int | None = None) -> int:
    # A Decimal coefficient can exceed the limit for int(string). For hashing,
    # reduce each block so no intermediate grows with the coefficient length.
    first = len(digits) % 18 or 18
    value = int(digits[:first])
    for start in range(first, len(digits), 18):
        value = value * 10**18 + int(digits[start:start + 18])
        if modulus is not None:
            value %= modulus
    return value


def _unsigned_ratio(n: int, exponent: int) -> tuple[int, int]:
    if exponent >= 0:
        return n * 10**abs(exponent), 1
    # Remove common factors of 2 and 5 from the numerator and denominator.
    d5 = -exponent
    while d5 > 0 and n % 5 == 0:
        n //= 5
        d5 -= 1

    # (n & -n).bit_length() - 1 counts trailing zeros in binary
    # representation of n (provided n is nonzero).
    d2 = -exponent
    shift2 = min((n & -n).bit_length() - 1, d2)
    if shift2:
        n >>= shift2
        d2 -= shift2

    d = 5**abs(d5) << d2
    return n, d
