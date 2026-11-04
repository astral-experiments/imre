from decimal import Decimal


def convert(value: Decimal) -> float:
    method = value.__float__
    return method()


print(convert(Decimal("-2.75")))
