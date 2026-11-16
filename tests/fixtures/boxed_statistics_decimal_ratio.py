from decimal import Decimal, Inexact, Rounded, ROUND_HALF_EVEN, ROUND_HALF_UP, localcontext
from statistics import mean


def current():
    for rounding, expected in ((ROUND_HALF_EVEN, "1.2"), (ROUND_HALF_UP, "1.3")):
        with localcontext(prec=2, rounding=rounding) as ctx:
            ctx.clear_flags()
            result = mean([Decimal("1.5"), 1])
            assert type(result) is Decimal and str(result) == expected
            assert ctx.flags[Inexact] and ctx.flags[Rounded]

    calls = []

    class ObservedDecimal(Decimal):
        def as_integer_ratio(self):
            calls.append(("ratio", str(self)))
            return super().as_integer_ratio()

        def __truediv__(self, other):
            calls.append(("divide", str(self), str(other)))
            return 42

    assert mean([ObservedDecimal("1.5"), 1]) == 42
    assert calls == [("ratio", "1.5"), ("divide", "5", "4")]

    class BrokenDecimal(Decimal):
        def as_integer_ratio(self):
            raise RuntimeError("ratio interrupted")

    for sample in ([BrokenDecimal("1"), 0.5], [0.5, BrokenDecimal("1")]):
        try:
            mean(sample)
        except RuntimeError as exc:
            assert str(exc) == "ratio interrupted"
        else:
            raise AssertionError("ratio conversion returned")
