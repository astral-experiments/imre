import decimal
from decimal import Decimal, InvalidOperation


def show(raw: str) -> None:
    try:
        value = Decimal(value=raw)
        print(str(value))
        print(repr(value))
    except InvalidOperation as error:
        print(str(error))


for raw in (
    "1.2300", "-0.00", "0002E+7", "0.000001", "0.0000001", "-0E+4",
    "0.0000000", "12.30e-4", "900e-3", ".1", "1.", "٠_١٢.٣_٠", "𝟡.𝟘",
    " +InFiNiTy ", "-inf", "naN00", "-sNaN00314", "NaN١٢٣", "1e-000",
    "1E+999999999999999999", "9.0E-1999999999999999997", "", "1.2.3",
    "nan-q", "1e+", ".", "١²", "-+", "Infinity0", "10E+999999999999999999",
):
    show(raw)
print(str(Decimal()))
print(str(Decimal(True)))
print(str(Decimal(False)))
print(str(Decimal(45)))
print(str(Decimal(value=-(1 << 200))))
big = Decimal(10 ** 5000)
digits = str(big)
print(len(digits))
print(digits[:3])
print(digits[-3:])
x = decimal.Decimal("1.00")
print(bool(Decimal("-0.00")))
print(bool(Decimal("0E+4")))
print(bool(Decimal("0.00010")))
print(bool(Decimal("sNaN42")))
print(Decimal(x) is x)
print(isinstance(x, Decimal))
print(isinstance("1.00", Decimal))
print(isinstance(1, Decimal))
