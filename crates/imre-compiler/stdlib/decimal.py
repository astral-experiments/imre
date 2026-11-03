"""Decimal construction and spelling used by serializers under the default context."""


class DecimalException(ArithmeticError):
    pass


class InvalidOperation(DecimalException):
    pass


class ConversionSyntax(InvalidOperation):
    pass


def _invalid() -> str:
    raise InvalidOperation("[<class 'decimal.ConversionSyntax'>]")


def _digits(text: str) -> str:
    digits: list[str] = []
    for char in text:
        try:
            digit = int(char)
        except ValueError:
            return _invalid()
        digits.append(str(digit))
    return "".join(digits)


def _canonical(value: str) -> str:
    text = value.strip().replace("_", "")
    sign = ""
    if text.startswith(("-", "+")):
        if text[0] == "-":
            sign = "-"
        text = text[1:]
    lowered = text.lower()
    if lowered in ("inf", "infinity"):
        return sign + "Infinity"
    if lowered.startswith(("snan", "nan")):
        if lowered.startswith("snan"):
            return sign + "sNaN" + _digits(text[4:]).lstrip("0")
        return sign + "NaN" + _digits(text[3:]).lstrip("0")
    mantissa, marker, exp_text = lowered.partition("e")
    whole, dot, fraction = mantissa.partition(".")
    if not whole and not fraction:
        return _invalid()
    digits = _digits(whole + fraction).lstrip("0") or "0"
    exp = 0
    if marker:
        neg = exp_text.startswith("-")
        if exp_text.startswith(("-", "+")):
            exp_text = exp_text[1:]
        if not exp_text:
            return _invalid()
        exp_digits = _digits(exp_text).lstrip("0")
        if len(exp_digits) > 19:
            raise InvalidOperation("[<class 'decimal.InvalidOperation'>]")
        if exp_digits:
            exp = int(exp_digits)
            if neg:
                exp = -exp
    exp -= len(fraction)
    point = len(digits) + exp
    if point - 1 > 999999999999999999 or exp < -1999999999999999997:
        raise InvalidOperation("[<class 'decimal.InvalidOperation'>]")
    if exp == 0:
        return sign + digits
    if exp < 0 and point >= -5:
        if point <= 0:
            return sign + "0." + "0" * (-point) + digits
        return sign + digits[:point] + "." + digits[point:]
    result = sign + digits[:1]
    if len(digits) > 1:
        result += "." + digits[1:]
    adjusted = point - 1
    if adjusted >= 0:
        return result + "E+" + str(adjusted)
    return result + "E" + str(adjusted)


def _parts(text: str) -> tuple[str, int]:
    """Return the signed coefficient and power of ten of a finite canonical value."""
    significand, marker, exponent = text.partition("E")
    whole, dot, fraction = significand.lstrip("-").partition(".")
    digits = (whole + fraction).lstrip("0")
    if not digits:
        return "0", 0
    coefficient = digits.rstrip("0")
    power = (int(exponent) if marker else 0) - len(fraction) + len(digits) - len(coefficient)
    if text.startswith("-"):
        coefficient = "-" + coefficient
    return coefficient, power


class Decimal:
    _text: str

    def __init__(self, value: str = "0") -> None:
        self._text = _canonical(value)

    def is_nan(self) -> bool:
        return self._text.lstrip("-").startswith(("NaN", "sNaN"))

    def as_integer_ratio(self) -> tuple[int, int]:
        """Return the exact reduced numerator and positive denominator."""
        text = self._text
        if text.lstrip("-").startswith(("NaN", "sNaN")):
            raise ValueError("cannot convert NaN to integer ratio")
        if text.endswith("Infinity"):
            raise OverflowError("cannot convert Infinity to integer ratio")

        coefficient, exponent = _parts(text)
        digits = coefficient.lstrip("-")
        # A Decimal coefficient can exceed the limit for int(string).
        first = len(digits) % 18 or 18
        numerator = int(digits[:first])
        for start in range(first, len(digits), 18):
            numerator = numerator * 10**18 + int(digits[start:start + 18])
        if coefficient.startswith("-"):
            numerator = -numerator
        scale = 10**abs(exponent)
        if exponent >= 0:
            return numerator * scale, 1

        divisor, remainder = scale, abs(numerator)
        while remainder:
            divisor, remainder = remainder, divisor % remainder
        return numerator // divisor, scale // divisor

    def __bool__(self) -> bool:
        mantissa, marker, exponent = self._text.lstrip("-").partition("E")
        return bool(mantissa.replace("0", "").replace(".", ""))

    def __str__(self) -> str:
        return self._text

    def __repr__(self) -> str:
        return "Decimal('" + self._text + "')"

    # Object-protocol methods for the boxed runtime.
    def __eq__(self, other: object):
        if not isinstance(other, Decimal):
            return NotImplemented
        left = self._text
        right = other._text
        if left.lstrip("-").startswith("sNaN") or right.lstrip("-").startswith("sNaN"):
            raise InvalidOperation([InvalidOperation])
        if left.lstrip("-").startswith("NaN") or right.lstrip("-").startswith("NaN"):
            return False
        if left.endswith("Infinity") or right.endswith("Infinity"):
            return left == right
        return _parts(left) == _parts(right)

    def __hash__(self) -> int:
        from sys import hash_info

        text = self._text
        magnitude = text.lstrip("-")
        if magnitude.startswith("sNaN"):
            raise TypeError("Cannot hash a signaling NaN value")
        if magnitude.startswith("NaN"):
            return object.__hash__(self)
        if magnitude == "Infinity":
            return -hash_info.inf if text.startswith("-") else hash_info.inf

        coefficient, exponent = _parts(text)
        digits = coefficient.lstrip("-")
        modulus = hash_info.modulus
        # Fold the coefficient in bounded chunks, even beyond the int-string limit.
        first = len(digits) % 18 or 18
        result = int(digits[:first])
        for start in range(first, len(digits), 18):
            result = (result * 10**18 + int(digits[start:start + 18])) % modulus
        result = result * pow(10, exponent, modulus) % modulus
        if coefficient.startswith("-"):
            result = -result
        return -2 if result == -1 else result
