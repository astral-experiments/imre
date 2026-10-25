def surround(text: str, *, before: str = "[", after: str = "]") -> str:
    print("target")
    return before + text + after


def forward(text: str, **options) -> str:
    print("wrapper")
    result = surround(text, **options)
    print("return")
    return result


class Reporter:
    def render(self, value: str, **options) -> str:
        return surround(value, **options)


def observe(label: str) -> str:
    print(label)
    return label


def fail() -> str:
    print("fail")
    raise ValueError("no prefix")


print(forward("plain"))
print(forward(observe("body"), after=observe("end"), before=observe("start")))
print(Reporter().render("instance", after="!"))
try:
    print(forward(observe("arg"), after=observe("first"), before=fail()))
except ValueError as error:
    print(str(error))
