from operator import attrgetter


class Request:
    value: int

    def __init__(self, value: int):
        self.value = value

    @property
    def doubled(self) -> int:
        return self.value * 2

    def answer(self) -> int:
        return self.value + 1


class Context:
    request: Request

    def __init__(self, request: Request):
        self.request = request


ctx = Context(Request(7))
print(attrgetter("request.value")(ctx))
print(attrgetter("request.doubled")(ctx))
print(attrgetter("request.answer")(ctx)())
