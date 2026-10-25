class ParseProblem(Exception):
    def __init__(self, message: str, stdout: str | None = None, stderr: str | None = None, code: int = 2) -> None:
        Exception.__init__(self, message, stdout, stderr)
        self.message = message
        self.stdout = stdout
        self.stderr = stderr
        self.code = code


class DirectBase(ValueError):
    def __init__(self, name: str) -> None:
        BaseException.__init__(self, "invalid: " + name)
        self.name = name


class NoArguments(Exception):
    def __init__(self) -> None:
        super().__init__()


class InnerArguments(Exception):
    def __init__(self, flag: bool | None, text: str) -> None:
        super().__init__(flag, (text, 17))


def exercise() -> None:
    message = "☃\nquoted '"
    error = ParseProblem(message, stdout=None, stderr="", code=9)
    try:
        raise error
    except ParseProblem as caught:
        print(caught is error)
        print(repr(caught.args))
        print(str(caught))
        print(repr(caught))
        print(caught.message)
        print(len(caught.args))
        print(caught.args == (message, None, ""))
        print(repr(caught.args[1]))
        print(repr(caught.args[-1]))
        print(caught.code)
    try:
        raise DirectBase("flag")
    except DirectBase as caught:
        print(caught.name)
        print(str(caught))
        print(repr(caught.args))
    print(repr(NoArguments().args))
    print(str(NoArguments()))
    for flag in [False, True]:
        details = InnerArguments(flag, "name")
        print(repr(details.args))
        print(repr(details.args[0]))
    missing = InnerArguments(None, "name")
    print(repr(missing.args))
    print(repr(missing.args[0]))


exercise()
