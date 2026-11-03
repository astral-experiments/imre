from decimal import Decimal


for text in ("-12.500", "3.5e-2", "5e3", "-0e999999", "1." + "0" * 40 + "1"):
    print(Decimal(text).as_integer_ratio())

for special in ("Infinity", "-Infinity"):
    try:
        Decimal(special).as_integer_ratio()
    except OverflowError as exc:
        assert str(exc) == "cannot convert Infinity to integer ratio"
    else:
        raise AssertionError("converted an infinite Decimal")

for special in ("NaN23", "-sNaN42"):
    try:
        Decimal(special).as_integer_ratio()
    except ValueError as exc:
        assert str(exc) == "cannot convert NaN to integer ratio"
    else:
        raise AssertionError("converted a Decimal NaN")

# Converting to a ratio does not pass the entire coefficient through int(str).
assert Decimal("-" + "9" * 4500).as_integer_ratio() == (1 - 10**4500, 1)
