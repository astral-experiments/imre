from decimal import Decimal


def current():
    for raw in ("12.30", "-12.30", "-0E-9", "Infinity", "-Infinity", "1E+900", "-1E-900",
                "1E+999999999999999999", "9" * 4500):
        print(len(raw), hash(Decimal(raw)))

    values = {Decimal("1.00"): "one", Decimal("-0.00"): "zero"}
    print(values[Decimal("1")], values[Decimal("0E+5")])
    for raw in ("NaN", "-NaN0031"):
        value = Decimal(raw)
        print(raw, hash(value) == object.__hash__(value), {value: 42}[value])
    for raw in ("sNaN", "-sNaN0031"):
        try:
            hash(Decimal(raw))
        except TypeError as error:
            print(raw, str(error))
        else:
            raise AssertionError("a signaling NaN was hashable")
