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
