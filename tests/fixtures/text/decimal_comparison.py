from decimal import Decimal, InvalidOperation


def compare(left: str, right: str, equal: bool) -> None:
    a = Decimal(left)
    b = Decimal(right)
    try:
        if equal:
            print(left, "==", right, a == b)
        else:
            print(left, "!=", right, a != b)
    except InvalidOperation as error:
        print(left, right, equal, "InvalidOperation", str(error))


for text in (
    "19.20", "-19.20", "0", "-0.0", "Infinity", "-Infinity",
    "NaN", "-NaN007", "+NaN42", "sNaN", "-sNaN007", "+sNaN42",
):
    value = Decimal(text)
    print(text, str(value), value.is_nan())

for left, right in (
    ("1.20", "12E-1"), ("1.20", "-1.2"), ("-0.00", "0E+9"),
    ("9007199254740992", "9007199254740993"), ("1E+900", "10E+899"),
    ("inf", "+Infinity"), ("Infinity", "-Infinity"), ("-Infinity", "-1E+900"),
    ("NaN31", "NaN31"), ("-NaN", "1.5"), ("+sNaN42", "NaN42"), ("2", "-sNaN7"),
):
    compare(left, right, True)
    compare(left, right, False)

value = Decimal("1")
print(value == "1", "1" != value)
print(value == 1, True == value, False != Decimal("-0.00"))
one = 1
print(value == one is value, value == one is not value)
large = 9007199254740993
print(large == Decimal("9007199254740993.0"), Decimal("9007199254740992") != large)
print(Decimal("1e999999999") == 1, 0 != Decimal("1e-999999999"))
