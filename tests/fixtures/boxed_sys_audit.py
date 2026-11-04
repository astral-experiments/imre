def current():
    import gc
    import sys
    from sys import addaudithook, audit
    from types import BuiltinFunctionType

    for fn, leaf in [(audit, "audit"), (addaudithook, "addaudithook")]:
        assert fn is sys.__dict__[leaf]
        assert type(fn) is BuiltinFunctionType
        assert fn.__name__ == fn.__qualname__ == leaf
        assert fn.__module__ == "sys" and fn.__self__ is sys
        assert repr(fn) == "<built-in function " + leaf + ">"

    class Event(str):
        def __str__(self):
            raise AssertionError("audit must use the original Unicode data")

    for value, expected in [(None, TypeError), (b"event", TypeError),
                            ("x\0y", ValueError), (chr(0xd800), UnicodeEncodeError)]:
        try:
            audit(value)
        except expected:
            pass
        else:
            raise AssertionError("invalid event accepted")

    seen = []
    received = []
    gate = []
    class Stop(BaseException):
        pass

    def late(event, args):
        if event.startswith("sample."):
            seen.append(("late", event))
            received.append((event, args))

    class First:
        def __call__(self, event, args):
            if event == "sys.addaudithook":
                seen.append(("register", args))
                if gate:
                    raise gate[0]
            if event.startswith("sample."):
                seen.append(("first", event))
                received.append((event, args))
                if event == "sample.outer":
                    addaudithook(hook=late)
                    audit("sample.inner")

    addaudithook(First())
    gc.collect()
    for error in (RuntimeError("deny"), ValueError("deny"), Stop("deny")):
        gate.append(error)
        try:
            result = addaudithook(late)
        except Stop as caught:
            assert caught is error
        else:
            assert isinstance(error, Exception) and result is None
        gate.clear()
    assert seen == [("register", ())] * 3
    seen.clear()

    data = []
    outer = Event("sample.outer")
    assert audit(outer, data, outer) is None
    assert seen == [("first", outer), ("register", ()), ("first", "sample.inner"),
                    ("late", "sample.inner"), ("late", outer)]
    first_name, first_args = received[0]
    last_name, last_args = received[-1]
    assert type(first_name) is str and first_name is last_name and first_name is not outer
    assert type(first_args) is tuple and first_args is last_args
    assert first_args[0] is data and first_args[1] is outer

    class Veto(Exception):
        pass
    veto = Veto("stop here")

    def reject(event, args):
        if event == "subprocess.Popen":
            assert args == ("/bin/true", ["/bin/true"], None, None)
            raise veto
        if event == "sample.deny":
            raise veto
    def unreachable(event, args):
        if event == "sample.deny":
            raise AssertionError("dispatch must stop at the first failing hook")
    addaudithook(reject)
    addaudithook(unreachable)
    try:
        audit("sample.deny")
    except Veto as error:
        assert error is veto
    else:
        raise AssertionError("audit swallowed a hook exception")

    import subprocess
    try:
        subprocess.run(("/bin/true",), capture_output=True)
    except Veto as error:
        assert error is veto
    else:
        raise AssertionError("Popen ignored a launch veto")

    assert addaudithook(42) is None
    try:
        audit("sample.noncallable")
    except TypeError as error:
        assert str(error) == "'int' object is not callable"
    else:
        raise AssertionError("noncallable hook was ignored")
    print("explicit audit events and registration checked")
