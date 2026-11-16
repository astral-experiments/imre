from textwrap import dedent

saved = ValueError("before split")


def argument() -> str | None:
    print("argument")
    raise saved


try:
    dedent(argument())
except ValueError as caught:
    print(caught is saved)
