"""Learn the optional layout of a None default without weakening annotated inputs."""

def identity(value=None):
    return value


def forward(value=None):
    return identity(value)


class Example:
    def __init__(self) -> None:
        pass

    def show(self, actual: str, expected=None) -> None:
        if expected:
            print(actual == expected)
        else:
            print("absent")


def maybe(flag: bool) -> str | None:
    if flag:
        return "ready"
    return None


def main() -> None:
    print(forward() is None)
    value = forward("word")
    if value is not None:
        print(value)
    print(forward(None) is None)
    e = Example()
    e.show("ready", maybe(True))
    e.show("ready", maybe(False))
    e.show("different", "ready")
    e.show("ready")


main()
