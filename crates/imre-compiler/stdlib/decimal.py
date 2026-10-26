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
    whole, dot, fraction = mantissa.partition(".")
    if not whole and not fraction:
        return _invalid()
    digits = _digits(whole + fraction).lstrip("0") or "0"
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


class Decimal:
    _text: str

    def __init__(self, value: str = "0") -> None:
        self._text = _canonical(value)

    def __bool__(self) -> bool:
        mantissa, marker, exponent = self._text.lstrip("-").partition("E")
        return bool(mantissa.replace("0", "").replace(".", ""))

    def __str__(self) -> str:
        return self._text

    def __repr__(self) -> str:
        return "Decimal('" + self._text + "')"
