from operator import attrgetter


class Request:
    def __init__(self, value):
        self.value = value

    def get_json(self):
        return self.value


class Context:
    @property
    def denied(self):
        raise AttributeError("request unavailable")

    @property
    def replace(self):
        self.request = None
        for i in range(2200):
            _ = [i, {}]
        return 42


def current():
    ctx = Context()
    ctx.request = Request(3)
    get_json = attrgetter("request.get_json")
    print(isinstance(get_json, attrgetter), callable(get_json), get_json(ctx)())
    ctx.request = Request(7)
    print(get_json(ctx)(), attrgetter("request.value", "request.value")(ctx))
    saved, replaced = attrgetter("request", "replace")(ctx)
    print(saved.get_json(), replaced)
    print(repr(attrgetter("request.value")))
    print(repr(attrgetter("request", "replace")))

    # The same getter preserves an undotted argument but rejoins dotted paths.
    name = "".join(("request", "_to_copy"))
    getter = attrgetter(name, "request.value", ".")
    constructor, args = getter.__reduce__()
    _, again = attrgetter.__reduce__(getter)
    assert constructor is attrgetter and args == (name, "request.value", ".")
    assert args[0] is again[0] and args[1] is not again[1] and args[2] is not again[2]
    ctx.request = Request(11)
    ctx.request_to_copy = 7
    setattr(ctx, "", ctx)
    assert constructor(*args)(ctx) == (7, 11, ctx)

    for call in (
        lambda: attrgetter(),
        lambda: attrgetter("request", 2),
        lambda: attrgetter(attr="request"),
        lambda: get_json(),
        lambda: get_json(ctx, ctx),
        lambda: get_json(obj=ctx),
        lambda: attrgetter("denied.value")(ctx),
    ):
        try:
            call()
        except (TypeError, AttributeError) as exc:
            print(str(exc))


if __name__ == "__main__":
    current()
