import decimal
from decimal import Decimal


def current():
    left = Decimal("1.20")
    total = left + Decimal("2.30")
    assert type(total) is Decimal
    assert str(total) == "3.50"
    assert total == Decimal("3.5") and total > left
    assert Decimal(value=left) is left

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

    print("boxed decimal arithmetic and contexts")
