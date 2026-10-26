"""Exercise context-local bindings with wheel-defined objects in the native object runtime."""

from contextvars import Context, ContextVar, Token, copy_context


class Request:
    pass


def current():
    var = ContextVar("request", default=None)
    req = Request()
    outer = var.set(req)
    saved = copy_context()
    inner = var.set(None)
    print(var.name, isinstance(var, ContextVar), isinstance(inner, Token))
    print(outer.var is var, outer.old_value is Token.MISSING, inner.old_value is req)
    var.reset(inner)
    var.reset(outer)
    print(var.get() is None, var.get(req) is req, saved.run(var.get) is req)
    with var.set(req):
        print(var.get() is req)
    try:
        var.reset(inner)
    except RuntimeError:
        print("used token")
    token = var.set(req)
    try:
        Context().run(var.reset, token)
    except ValueError:
        print("different context")
    var.reset(token)
    missing = ContextVar("absent")
    try:
        missing.get()
    except LookupError as error:
        print(error.args[0] is missing, "<ContextVar name='absent'" in str(error))
    try:
        saved[missing]
    except KeyError as error:
        print(error.args[0] is missing)
    print(
        var in saved, len(saved), list(saved.keys()) == [var],
        list(saved.values())[0] is req,
    )

    class Payload:
        def __init__(self, tag):
            self.tag = tag

    # Force collection with values reachable through a copied context, the token
    # history and the active bindings.
    kept = ContextVar("kept", default=Payload("default"))
    token = kept.set(Payload("old"))
    kept.set(Payload("in-copy"))
    copy = copy_context()
    older = kept.set(Payload("old-only"))
    newer = kept.set(Payload("current"))
    for i in range(2500):
        _ = [Payload(i), {}]
    print(copy.run(kept.get).tag, newer.old_value.tag, kept.get().tag)
    kept.reset(older)
    kept.reset(token)
    print(kept.get().tag)
