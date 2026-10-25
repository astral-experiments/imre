def render(fmt: str, pair: tuple[int, str]) -> None:
    try:
        print(repr(fmt % pair))
    except TypeError as exc:
        print("TypeError: " + str(exc))
    except ValueError as exc:
        print("ValueError: " + str(exc))
    except OverflowError as exc:
        print("OverflowError: " + str(exc))


def format_expr() -> str:
    print("format")
    return "%q"


def operand(n: int) -> int:
    print(n)
    if n == 3:
        raise LookupError("argument failure")
    return n


for pattern in ["%d: %r", "%#x/%a", "%*s", "%.*r", "%c:%s", "%s", "%%", "%s-%s-%s",
                "π%界", "%(item)s", "%s %.", "%r%%", "%g %s", "%d %i"]:
    for number, word in [(-3, "π"), (0, ""), (65, "x'\n"), (2**100, "word")]:
        pair = number, word
        render(pattern, pair)

for fmt in ["", "plain %%", "%s", "%.*s", "%(x)s", "%"]:
    try:
        print(repr(fmt % ()))
    except TypeError as exc:
        print("TypeError: " + str(exc))
    except ValueError as exc:
        print("ValueError: " + str(exc))

for width, precision in [(6, 2), (-7, 1), (2, -1), (0, 3)]:
    print(repr("%*.*s" % (width, precision, "αβγ")))


# A malformed format is inspected only after every operand has been evaluated.
for last in [2, 3]:
    try:
        print(format_expr() % (operand(1), operand(last)))
    except ValueError as exc:
        print("ValueError: " + str(exc))
    except LookupError as exc:
        print("LookupError: " + str(exc))
