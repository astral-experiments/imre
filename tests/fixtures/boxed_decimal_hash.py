import sys
from decimal import Decimal, InvalidOperation


class MisleadingInteger(int):
    @property
    def numerator(self):
        raise AssertionError("Decimal compared an int through its numerator")


class QuietLooking(Decimal):
    def is_snan(self):
        return False


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

    limit = sys.get_int_max_str_digits()
    try:
        sys.set_int_max_str_digits(640)
        for n in (10**640, -(10**640), MisleadingInteger(-21)):
            value = Decimal(n)
            assert {value: "hit"}[n] == {n: "hit"}[value] == "hit"
            assert len({value: 1, n: 2}) == 1
    finally:
        sys.set_int_max_str_digits(limit)

    quiet = QuietLooking("NaN")
    assert hash(quiet) == object.__hash__(quiet)
    try:
        hash(QuietLooking("-sNaN31"))
    except TypeError:
        pass
    else:
        raise AssertionError("is_snan override hid a signaling NaN")

    assert Decimal("-١٢.٥") == Decimal("-12.5")
    assert str(Decimal("sNaN١٢")) == "sNaN12"
    try:
        Decimal("ſNaN")
    except InvalidOperation:
        pass
    else:
        raise AssertionError("accepted a non-ASCII marker")
