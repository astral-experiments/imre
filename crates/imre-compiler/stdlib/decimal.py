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
    if type(text) is str and text.isascii() and text.isdigit():
        return text
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


def _coefficient_int(digits: str) -> int:
    # A Decimal coefficient can exceed the digit limit on int(string).
    first = len(digits) % 18 or 18
    value = int(digits[:first])
    for offset in range(first, len(digits), 18):
        value = value * 10**18 + int(digits[offset:offset + 18])
    return value


def _product_parts(magnitude: str) -> tuple[str, int]:
    mantissa, marker, exponent = magnitude.partition("E")
    whole, dot, fraction = mantissa.partition(".")
    return (whole + fraction).lstrip("0") or "0", (int(exponent) if marker else 0) - len(fraction)


def _display(text: str) -> str:
    return text


def _parts(text: str) -> tuple[str, int]:
    """Return the signed coefficient and power of ten of a finite canonical value."""
    digits, power = _product_parts(text.lstrip("-"))
    if digits == "0":
        return "0", 0
    coefficient = digits.rstrip("0")
    power += len(digits) - len(coefficient)
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
        numerator = _coefficient_int(digits)
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
        return _display(self._text)

    def __repr__(self) -> str:
        return "Decimal('" + _display(self._text) + "')"

    # Object-protocol methods for the boxed runtime.
    def __float__(self, /) -> float:
        if not issubclass(type(self), Decimal):
            raise TypeError(
                f"descriptor '__float__' requires a 'decimal.Decimal' object "
                f"but received a '{type(self).__name__}'"
            )
        text = self._text
        magnitude = text.lstrip("-")
        if magnitude.startswith("sNaN"):
            raise ValueError("cannot convert signaling NaN to float")
        if magnitude.startswith("NaN"):
            text = "-nan" if text.startswith("-") else "nan"
        return float(text)

    def __eq__(self, other: object):
        if isinstance(other, Decimal):
            right = other._text
        elif issubclass(type(other), int):
            right = Decimal(other)._text
        else:
            return NotImplemented
        left = self._text
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

    def __mul__(self, other, /):
        return _decimal_product(self, other, False)

    def __rmul__(self, other, /):
        return _decimal_product(self, other, True)


def _round_product_up(rounding, sign, last, first, sticky):
    # Called only if at least one discarded digit is nonzero.
    if rounding == ROUND_DOWN:
        return False
    if rounding == ROUND_UP:
        return True
    if rounding == ROUND_CEILING:
        return not sign
    if rounding == ROUND_FLOOR:
        return bool(sign)
    if rounding == ROUND_05UP:
        return last in "05"
    if first != "5" or sticky:
        return first >= "5"
    return rounding == ROUND_HALF_UP or (rounding == ROUND_HALF_EVEN and last in "13579")


def _product_overflow(context, sign):
    context._signal(Overflow, Inexact, Rounded)
    rounding = context.rounding
    if (rounding in (ROUND_DOWN, ROUND_05UP)
            or (rounding == ROUND_CEILING and sign)
            or (rounding == ROUND_FLOOR and not sign)):
        return Decimal(sign + "9" * context.prec + "E" + str(context.Etop()))
    return Decimal(sign + "Infinity")


def _finish_product(sign, digits, exponent, context):
    tiny = context.Etiny()
    top = context.Etop()
    if digits == "0":
        bounded = max(tiny, min(exponent, top if context.clamp else context.Emax))
        if bounded != exponent:
            context._signal(Clamped)
        return Decimal(sign + "0E" + str(bounded))

    adjusted = exponent + len(digits) - 1
    if adjusted > context.Emax:
        return _product_overflow(context, sign)
    subnormal = adjusted < context.Emin
    signals = [Subnormal] if subnormal else []

    # Round once, at the stricter of the precision and subnormal limits.
    drop = max(len(digits) - context.prec, tiny - exponent)
    if drop > 0:
        keep = len(digits) - drop
        tail = digits[max(keep, 0):]
        digits = digits[:max(keep, 0)] or "0"
        exponent += drop
        signals.append(Rounded)
        if tail.strip("0"):
            signals.append(Inexact)
            if subnormal:
                signals.append(Underflow)
            first = tail[0] if keep >= 0 else "0"
            if _round_product_up(context.rounding, sign, digits[-1], first, bool(tail[1:].strip("0"))):
                digits = Decimal(_coefficient_int(digits) + 1)._text
                if len(digits) > context.prec:
                    digits = digits[:-1]
                    exponent += 1
                if exponent + len(digits) - 1 > context.Emax:
                    return _product_overflow(context, sign)
            if digits == "0":
                signals.append(Clamped)

    if context.clamp and exponent > top:
        digits += "0" * (exponent - top)
        exponent = top
        signals.append(Clamped)
    if signals:
        context._signal(*signals)
    return Decimal(sign + digits + "E" + str(exponent))


def _decimal_product(self, other, reflected):
    if not issubclass(type(self), Decimal):
        method = "__rmul__" if reflected else "__mul__"
        raise TypeError(f"descriptor '{method}' requires a 'decimal.Decimal' object but received a '{type(self).__name__}'")
    if not issubclass(type(other), (Decimal, int)):
        return NotImplemented
    left = Decimal(self)._text
    right = Decimal(other)._text
    if reflected:
        left, right = right, left
    context = getcontext()

    # A signaling NaN takes precedence over a quiet NaN; preserve the selected payload.
    for prefix in ("sNaN", "NaN"):
        for text in (left, right):
            magnitude = text.lstrip("-")
            if magnitude.startswith(prefix):
                if prefix == "sNaN":
                    context._signal(InvalidOperation)
                size = context.prec - context.clamp
                payload = magnitude[len(prefix):]
                return Decimal(("-" if text.startswith("-") else "") + "NaN" + (payload[-size:] if size else ""))

    sign = "-" if left.startswith("-") != right.startswith("-") else ""
    left, right = left.lstrip("-"), right.lstrip("-")
    if left == "Infinity" or right == "Infinity":
        peer = right if left == "Infinity" else left
        if not peer.partition("E")[0].strip("0."):
            context._signal(InvalidOperation)
            return Decimal("NaN")
        return Decimal(sign + "Infinity")
    a, ea = _product_parts(left)
    b, eb = _product_parts(right)
    digits = Decimal(_coefficient_int(a) * _coefficient_int(b))._text
    return _finish_product(sign, digits, ea + eb, context)
