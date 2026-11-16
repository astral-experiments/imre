import decimal
import _pydecimal
from decimal import Decimal


class AppContext(decimal.Context):
    def __eq__(self, other):
        raise AssertionError("setcontext compared contexts by value")


def current():
    left = Decimal("1.20")
    total = left + Decimal("2.30")
    assert type(total) is Decimal
    assert str(total) == "3.50"
    assert total == Decimal("3.5") and total > left
    assert Decimal(value=left) is left

    assert format(left, "f") == "1.20"
    assert format(Decimal("-0E+8"), "f") == "-0"
    assert format(Decimal("-0.000"), "f") == "-0.000"
    assert format(Decimal("1.23E+4"), "f") == "12300"
    assert format(Decimal("2.70E-8"), "f") == "0.0000000270"

    with decimal.localcontext(prec=2) as ctx:
        assert Decimal(1) / Decimal(3) == Decimal("0.33")
        assert ctx.flags[decimal.Inexact]
        ctx.traps[decimal.InvalidOperation] = False
        assert Decimal("invalid").is_nan()
        assert ctx.flags[decimal.InvalidOperation]

    try:
        left + 0.5
    except TypeError:
        pass
    else:
        raise AssertionError("Decimal mixed with a float")

    with decimal.localcontext():
        ctx = AppContext(prec=3)
        decimal.setcontext(ctx)
        try:
            decimal.setcontext(_pydecimal.Context())
        except TypeError:
            pass
        else:
            raise AssertionError("setcontext accepted a foreign context")
        assert decimal.getcontext() is ctx
        assert Decimal(1) / Decimal(3) == Decimal("0.333")

    cents = Decimal("0.01")
    foreign = _pydecimal.Context()
    try:
        Decimal("19.985").quantize(cents, context=foreign)
    except TypeError:
        pass
    else:
        raise AssertionError("quantize accepted a foreign context")
    assert not any(foreign.flags.values())

    for post in (lambda: left.quantize(cents, "HALF-EVEN"),
                 lambda: Decimal(17).to_integral_exact("HALF-EVEN"),
                 lambda: Decimal(17).to_integral_value("HALF-EVEN")):
        try:
            post()
        except TypeError:
            pass
        else:
            raise AssertionError("accepted an invalid rounding mode on an exact amount")

    print("boxed decimal arithmetic and contexts")
