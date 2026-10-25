"""Compare optional scalars, nested exception args and lazy conversion through Template."""
from string import Template


class Detail(Exception):
    def __init__(self, flag: bool | None, text: str | None) -> None:
        super().__init__(flag, (text, flag))


def render(value: object) -> str:
    return Template("shown: $x, $x").substitute(x=value)


def observe(flag: bool | None, text: str | None) -> None:
    err = Detail(flag, text)
    print(repr(err.args))
    print(str(err))
    print(repr(err))
    print(err.args == (flag, (text, flag)))
    print(err.args[0] == flag)
    print(err.args[0] is None)
    print(isinstance(err.args[0], bool))
    print(bool(err.args[0]))
    print(render(err.args))
    print(render(err.args[0]))
    print(render(err.args[1]))
    print(render(flag))
    print(render(text))
    print(repr(ImportError(flag, (text, flag)).args))


observe(False, "")
observe(True, "é")
observe(None, None)
huge = 10**4500
print(Template("no interpolation").substitute(x=huge))
try:
    print(Template("$x").substitute(x=huge))
except ValueError as exc:
    print(str(exc))
