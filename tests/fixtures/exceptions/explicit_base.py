class ParseProblem(Exception):
    def __init__(self, message: str, output: str | None = None, code: int = 2) -> None:
        Exception.__init__(self, message)
        self.message = message
        self.output = output
        self.code = code


class DirectBase(ValueError):
    def __init__(self, name: str) -> None:
        BaseException.__init__(self, "invalid: " + name)
        self.name = name


def exercise() -> None:
    error = ParseProblem("☃\nquoted '", output="stderr", code=9)
    try:
        raise error
    except ParseProblem as caught:
        print(caught is error)
        print(repr(caught.args))
        print(str(caught))
        print(repr(caught))
        print(caught.message)
        output = caught.output
        if output is not None:
            print(output)
        print(caught.code)
    try:
        raise DirectBase("flag")
    except DirectBase as caught:
        print(caught.name)
        print(str(caught))
        print(repr(caught.args))


exercise()
