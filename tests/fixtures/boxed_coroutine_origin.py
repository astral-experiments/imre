"""CPython 3.14.5 oracle for snapshots taken when a coroutine object is created."""
import gc
import sys
import types
import weakref

class Pause:
    def __await__(self):
        return (yield "paused")

async def target():
    return await Pause()

def leaf():
    return (
        target()
    )

def outer():
    return (
        leaf()
    )

class Producer:
    async def produce(self):
        return 1

def dotted(producer):
    return (
        producer
        .produce()
    )

class Payload: pass

def with_payload():
    local = Payload()
    return target(), weakref.ref(local)

def expect(kind, message, fn):
    try:
        fn()
    except kind as err:
        assert str(err) == message, (str(err), message)
    else:
        raise AssertionError("invalid operation accepted")
    assert sys.get_coroutine_origin_tracking_depth() == 2

def current():
    get = sys.get_coroutine_origin_tracking_depth
    set_depth = sys.set_coroutine_origin_tracking_depth
    from sys import get_coroutine_origin_tracking_depth as imported_get
    from sys import set_coroutine_origin_tracking_depth as imported_set
    assert imported_get is get and imported_set is set_depth
    for function, name, signature in [
        (get, "get_coroutine_origin_tracking_depth", "($module, /)"),
        (set_depth, "set_coroutine_origin_tracking_depth", "($module, /, depth)"),
    ]:
        assert type(function) is types.BuiltinFunctionType
        assert function.__self__ is sys and function.__module__ == "sys"
        assert function.__name__ == function.__qualname__ == name
        assert function.__text_signature__ == signature
        assert repr(function) == f"<built-in function {name}>"
    old = get()
    try:
        assert set_depth(0) is None
        disabled = outer()
        assert disabled.cr_origin is None
        assert set_depth(depth=True) is None and get() == 1
        one = outer()
        file = leaf.__code__.co_filename
        expected_one = ((file, leaf.__code__.co_firstlineno + 2, "leaf"),)
        assert one.cr_origin == expected_one

        class Index:
            def __index__(self): return 2
        assert set_depth(Index()) is None and get() == 2
        two = outer()
        expected_two = expected_one + ((file, outer.__code__.co_firstlineno + 2, "outer"),)
        stored = two.cr_origin
        assert type(stored) is tuple and all(type(row) is tuple for row in stored)
        assert stored == expected_two and two.cr_origin is stored, (stored, expected_two)
        descriptor = types.CoroutineType.cr_origin
        assert isinstance(descriptor, types.MemberDescriptorType)
        assert descriptor.__get__(two) is stored
        assert vars(type(two))["cr_origin"] is descriptor and "cr_origin" in dir(two)
        method = dotted(Producer())
        assert method.cr_origin[0] == (file, dotted.__code__.co_firstlineno + 3, "dotted")
        method.close()
        scope = {}
        source = "async def make(): pass\rdef create():\r    return make()\rresult=create()"
        exec(compile(source, "carriage.py", "exec"), scope)
        result = scope["result"]
        assert result.cr_origin == (("carriage.py", 3, "create"), ("carriage.py", 4, "<module>"))
        result.close()
        assert disabled.cr_origin is None and one.cr_origin == expected_one
        expect(ValueError, "depth must be >= 0", lambda: set_depth(-1))
        expect(OverflowError, "Python int too large to convert to C int", lambda: set_depth(1 << 48))
        expect(TypeError, "'float' object cannot be interpreted as an integer", lambda: set_depth(2.0))
        expect(TypeError, "set_coroutine_origin_tracking_depth() missing required argument 'depth' (pos 1)", lambda: set_depth())
        expect(TypeError, "set_coroutine_origin_tracking_depth() takes at most 1 argument (2 given)", lambda: set_depth(1,depth=1))
        expect(TypeError, "set_coroutine_origin_tracking_depth() missing required argument 'depth' (pos 1)", lambda: set_depth(unrelated=1))
        expect(TypeError, "sys.get_coroutine_origin_tracking_depth() takes no arguments (1 given)", lambda: get(1))
        expect(TypeError, "sys.get_coroutine_origin_tracking_depth() takes no keyword arguments", lambda: get(depth=1))
        expect(AttributeError, "readonly attribute", lambda: setattr(two,"cr_origin",None))
        expect(AttributeError, "readonly attribute", lambda: delattr(two,"cr_origin"))
        assert two.send(None) == "paused" and two.cr_origin is stored
        set_depth(0)
        try:
            two.send(37)
        except StopIteration as e:
            assert e.value == 37
        else:
            raise AssertionError("coroutine did not complete")
        assert two.cr_origin is stored
        one.close()
        disabled.close()
        assert one.cr_origin == expected_one and disabled.cr_origin is None
        set_depth(1 << 20)
        deep = outer()
        assert deep.cr_origin[:2] == expected_two
        assert 3 <= len(deep.cr_origin) < 1000 and deep.cr_origin[2][2] == "current"
        deep.close()
        owned, ref = with_payload()
        assert owned.cr_origin[0] == (file, with_payload.__code__.co_firstlineno + 2, "with_payload")
        gc.collect()
        assert ref() is None, "origin retained caller locals"
        owned.close()
        print("creation frames", [(row[2], row[1]) for row in stored])
        print("zero, sticky snapshots, suspension, close, deep, weak locals, ten errors: PASS")
    finally:
        set_depth(old)
    assert get() == old

if __name__ == "__main__":
    current()
