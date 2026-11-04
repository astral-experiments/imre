"""Checked integer-pair representation of a Fraction."""


class Fraction:
    _numerator: int
    _denominator: int

    def __init__(self, numerator: int, denominator: int) -> None:
        self._numerator = numerator
        self._denominator = denominator

    @property
    def numerator(self) -> int:
        return self._numerator

    @property
    def denominator(self) -> int:
        return self._denominator

    def as_integer_ratio(self) -> tuple[int, int]:
        return self._numerator, self._denominator

    def is_integer(self) -> bool:
        return self._denominator == 1

    def __bool__(self) -> bool:
        return bool(self._numerator)

    def __str__(self) -> str:
        if self._denominator == 1:
            return str(self._numerator)
        return str(self._numerator) + "/" + str(self._denominator)

    def __repr__(self) -> str:
        return "Fraction(" + str(self._numerator) + ", " + str(self._denominator) + ")"

    def limit_denominator(self, max_denominator: int = 1000000) -> 'Fraction':
        if max_denominator < 1:
            raise ValueError("max_denominator should be at least 1")
        n = self._numerator
        d = self._denominator
        if d <= max_denominator:
            return Fraction(n, d)
        whole = n // d
        remainder = n - whole * d
        # Every non-integer with denominator at most M lies at least 1/M
        # from the closest integer. At equality the smaller denominator wins;
        # an exact midpoint between integers goes to the lower one.
        if remainder:
            complement = d - remainder
            if remainder <= complement:
                if 2 * remainder * max_denominator <= d:
                    return Fraction(whole, 1)
            elif 2 * complement * max_denominator <= d:
                return Fraction(whole + 1, 1)
        p0, q0, p1, q1 = 1, 0, whole, 1
        n, d = d, remainder
        while True:
            a, remainder = divmod(n, d)
            q2 = q0 + a * q1
            if q2 > max_denominator:
                break
            p0, q0, p1, q1 = p1, q1, p0 + a * p1, q2
            n, d = d, remainder
        k = (max_denominator - q0) // q1
        q2 = q0 + k * q1
        if 2 * d * q2 <= self._denominator:
            return Fraction(p1, q1)
        return Fraction(p0 + k * p1, q2)


def _normalized(n: int, d: int) -> Fraction:
    if d == 0:
        raise ZeroDivisionError("Fraction(" + str(n) + ", 0)")
    a, b = n, d
    while b:
        a, b = b, a % b
    return Fraction(n // a, d // a)


def _digits(text: str) -> bool:
    if text.isascii() and text.isdigit():
        return True
    previous_digit = False
    for char in text:
        if char == "_":
            if not previous_digit:
                return False
            previous_digit = False
        else:
            try:
                int(char)
            except ValueError:
                return False
            previous_digit = True
    return previous_digit


def _from_text(value: str) -> Fraction:
    text = value.strip()
    negative = text.startswith("-")
    if negative or text.startswith("+"):
        text = text[1:]
    if "/" in text:
        left, _, right = text.partition("/")
        left = left.rstrip()
        right = right.lstrip()
        if not _digits(left) or not _digits(right):
            raise ValueError("Invalid literal for Fraction: " + repr(value))
        n = int(left)
        return _normalized(-n if negative else n, int(right))

    mantissa, e, exponent = text.lower().partition("e")
    whole, _, decimal = mantissa.partition(".")
    if (not whole and not decimal) or (whole and not _digits(whole)) or (decimal and not _digits(decimal)):
        raise ValueError("Invalid literal for Fraction: " + repr(value))
    if e:
        magnitude = exponent
        if magnitude.startswith(("+", "-")):
            magnitude = magnitude[1:]
        if not _digits(magnitude):
            raise ValueError("Invalid literal for Fraction: " + repr(value))
    n = int(whole or "0")
    d = 1
    if decimal:
        decimal = decimal.replace("_", "")
        d = 10 ** len(decimal)
        n = n * d + int(decimal)
    if e:
        power = int(exponent)
        scale = 10 ** abs(power)
        if power >= 0:
            n *= scale
        else:
            d *= scale
    return _normalized(-n if negative else n, d)


def _copy(value: Fraction) -> Fraction:
    return Fraction(value.numerator, value.denominator)
