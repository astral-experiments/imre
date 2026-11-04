from decimal import Decimal, Inexact, ROUND_HALF_UP, Rounded, localcontext


def main():
    amount = Decimal("1.250")
    print(amount * 4, 4 * amount, False * Decimal("-1.00"))
    print(Decimal("1.00") * (2**53 + 1))

    with localcontext(prec=3) as ctx:
        print(Decimal("1.245") * 1, ctx.flags[Inexact], ctx.flags[Rounded])
        ctx.clear_flags()
        ctx.traps[Inexact] = True
        try:
            print(3 * Decimal("0.4125"))
        except Inexact:
            print("trapped", ctx.flags[Inexact], ctx.flags[Rounded])

    print(amount.__mul__(0.5) is NotImplemented)

    class ReflectedAmount(Decimal):
        def __rmul__(self, other):
            return marker

    marker = object()
    print((2 * ReflectedAmount("3")) is marker)

    class RoundingName(str):
        def __eq__(self, other):
            raise AssertionError("unexpected equality callback")

    class PretendMode:
        def __eq__(self, other):
            calls.append(other)
            return True

    calls = []
    with localcontext(prec=3, rounding=RoundingName("ROUND_HALF_UP")) as ctx:
        print(ctx.rounding is ROUND_HALF_UP, Decimal("1.245") * 1)
        try:
            ctx.rounding = PretendMode()
        except TypeError:
            print("rejected", ctx.rounding is ROUND_HALF_UP, len(calls))


main()
