"""Exercise context-local bindings with wheel-defined objects in the native object runtime."""

import _contextvars
import contextvars
from contextvars import Context, ContextVar, Token, copy_context
from operator import length_hint
from types import ClassMethodDescriptorType, GenericAlias


class Request:
    pass


def current():
    assert _contextvars.__name__ == "_contextvars"
    assert contextvars.__name__ == "contextvars"
    assert _contextvars is not contextvars
    assert _contextvars.Context is Context
    assert _contextvars.ContextVar is ContextVar
    assert _contextvars.Token is Token
    assert _contextvars.copy_context is copy_context

    for cls in (ContextVar, Token):
        alias = cls[Request]
        raw = vars(cls)["__class_getitem__"]
        assert type(raw) is ClassMethodDescriptorType
        assert type(alias) is GenericAlias
        assert alias.__origin__ is cls and alias.__args__ == (Request,)
        assert cls.__class_getitem__.__self__ is cls
        assert raw(cls, Request) == alias == raw.__get__(None, cls)(Request)
        try:
            class InvalidBase(alias):
                pass
        except TypeError as error:
            assert "not an acceptable base type" in str(error)
        else:
            raise AssertionError("subclassed a final context type")
    var = _contextvars.ContextVar[Request]("request", default=None)
    assert type(var) is ContextVar and type(Context()) is Context
    assert type(ContextVar) is type and ContextVar.__module__ == "_contextvars"
    assert ContextVar.__qualname__ == "ContextVar"
    assert copy_context.__self__ is _contextvars and copy_context.__module__ == "_contextvars"
    req = Request()
    outer = var.set(req)
    assert type(outer) is Token
    saved = copy_context()
    try:
        saved.run(saved.run)
    except TypeError as error:
        assert str(error) == "run() missing 1 required positional argument"
    else:
        raise AssertionError("entered a context without a callback")
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

    class BadRepr:
        def __repr__(self):
            raise LookupError("token repr")

    default = BadRepr()
    key = ContextVar("request", default=default)
    token = key.set(req)
    for invalid in (token, default):
        try:
            var.reset(invalid)
        except LookupError as error:
            assert error.args == ("token repr",)
        else:
            raise AssertionError("reset skipped __repr__")
    key.reset(token)

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

    left = Context()
    right = Context()
    assert left == right
    left.run(var.set, [1])
    right.run(var.set, [1])
    assert left == right and left == left.copy()
    right.run(var.set, [2])
    assert left != right
    left.run(var.set, left)
    right.run(var.set, right)
    assert left == left.copy()
    try:
        left == right
    except RecursionError as error:
        assert isinstance(error, RuntimeError)
    else:
        raise AssertionError("recursive comparison")

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

    another = ContextVar("request")
    saved.run(another.set, None)
    mapping = dict(saved)
    assert list(mapping) == list(saved) and mapping[var] is req and mapping[another] is None
    mapping[missing] = "keep"
    mapping[var] = None
    mapping.update(saved)
    assert len(mapping) == 3 and mapping[var] is req and mapping[missing] == "keep"


    empty = Context()
    for view in (empty.keys(), empty.items(), empty.values()):
        assert not view and len(view) == 0 and length_hint(view) == 0
    empty.run(var.set, 1)
    views = (empty.keys(), empty.items(), empty.values())
    empty.run(another.set, 2)
    for view in views:
        assert view and len(view) == 1
        assert len(list(view)) == 1 and list(view) == []
        assert view and len(view) == 1 and length_hint(view) == 1
