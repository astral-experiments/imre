import keyword
from keyword import iskeyword, issoftkeyword
from keys import Nested


class MissingHash:
    __hash__ = None


class BrokenHash:
    def __hash__(self):
        raise ValueError("hash called")


assert keyword.iskeyword is iskeyword
assert keyword.issoftkeyword is issoftkeyword
assert iskeyword is not issoftkeyword
assert iskeyword.__self__ == frozenset(keyword.kwlist)
assert issoftkeyword.__self__ == frozenset(keyword.softkwlist)
assert type(iskeyword).__name__ == "builtin_function_or_method"
assert iskeyword.__name__ == "__contains__"
assert iskeyword.__qualname__ == "frozenset.__contains__"
assert iskeyword.__module__ is None
assert not hasattr(iskeyword, "__func__")

keyword.kwlist.append("ordinary")
keyword.softkwlist = []
assert iskeyword("class") and not iskeyword("ordinary")
assert issoftkeyword("match") and not issoftkeyword(None)
assert not iskeyword(set()) and not issoftkeyword("λ")
for test in (iskeyword, issoftkeyword):
    for value in ([], {}, bytearray(b"abc"), ([],), MissingHash(), Nested.MissingHash(), BrokenHash()):
        try:
            test(value)
        except (TypeError, ValueError) as error:
            print(type(error).__name__, str(error))
        else:
            raise AssertionError(value)
    for args, kwargs in [([], {}), (["a", "b"], {}), ([], {"key": "class"})]:
        try:
            test(*args, **kwargs)
        except TypeError as error:
            print(str(error))
        else:
            raise AssertionError(args)
