from collections.abc import Callable

# From CPython 3.13's Lib/_pyrepl/types.py.
type Callback = Callable[[], object]
type Completer = Callable[[str, int], str | None]
type CharBuffer = list[str]

later = str


def current():
    callback = Callback.__value__
    assert callback == Callable[[], object] and Callback.__value__ is callback
    completer = Completer.__value__
    assert completer == Callable[[str, int], str | None] and Completer.__value__ is completer
    chars = CharBuffer.__value__
    assert chars == list[str] and CharBuffer.__value__ is chars
    calls = []

    def materialize(element):
        calls.append(element)
        return list[element]

    def make_alias():
        type Row = materialize(later)
        assert calls == []
        try:
            Row.__value__
        except NameError as exc:
            assert exc.name == "later"
        else:
            raise AssertionError("resolved an unbound local from the module")
        later = bytes
        return Row

    row = make_alias()
    assert calls == []
    value = row.__value__
    assert value == list[bytes] and row.__value__ is value
    assert calls == [bytes]
    print("checked lazy aliases")
