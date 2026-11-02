"""CPython 3.14 instruction numbers and stack effects exposed by _opcode."""
import _opcode
import sys
from _opcode import stack_effect


class Index:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        return self.value


class BrokenIndex:
    def __index__(self):
        raise LookupError("index callback")


def check_error(kind, callback, *args, **kwargs):
    try:
        callback(*args, **kwargs)
    except kind:
        return
    raise AssertionError("expected conversion or argument error")


def check():
    assert _opcode.stack_effect is stack_effect
    assert stack_effect.__doc__ == "Compute the stack effect of the opcode."
    assert _opcode.has_free.__doc__ == (
        "Return True if the opcode accesses a free variable, False otherwise.\n\n"
        "Note that 'free' in this context refers to names in the current scope\n"
        "that are referenced by inner scopes or names in outer scopes that are\n"
        "referenced from this scope. It does not include references to global\n"
        "or builtin scopes."
    )
    assert stack_effect(31) == -1  # POP_TOP
    assert stack_effect(49, Index(3)) == -2  # BUILD_SLICE
    assert stack_effect(Index(118), 515) == 5  # UNPACK_EX
    assert stack_effect(92, 0) == 1 and stack_effect(92, 1) == 2  # LOAD_GLOBAL
    for code, effect in [(263, 2), (264, 1), (265, 1)]:
        assert stack_effect(code) == stack_effect(code, jump=None) == effect
        assert stack_effect(code, jump=False) == 0
        assert stack_effect(code, jump=True) == effect
        assert _opcode.has_exc(opcode=code) is True
    assert _opcode.is_valid(130) is True  # the specialized BINARY_OP_ADD_INT
    assert _opcode.has_arg(Index(92)) is True
    assert _opcode.has_arg(1000) is False
    check_error(ValueError, stack_effect, 130)
    check_error(ValueError, stack_effect, 30000)
    check_error(ValueError, stack_effect, 49, -1)
    check_error(ValueError, stack_effect, 31, jump=1)
    check_error(TypeError, stack_effect, opcode=31)
    check_error(TypeError, stack_effect, 31, 0, True)
    check_error(TypeError, stack_effect, 3.5)
    check_error(OverflowError, stack_effect, 1 << 100)
    check_error(OverflowError, stack_effect, 31, 1 << 100)
    check_error(LookupError, stack_effect, BrokenIndex())
    check_error(LookupError, stack_effect, 31, BrokenIndex(), jump=0)
    check_error(TypeError, _opcode.get_nb_ops, 1)
    nb = _opcode.get_nb_ops()
    assert (len(nb), nb[0], nb[-1]) == (27, ("NB_ADD", "+"), ("NB_SUBSCR", "[]"))
    nb[0] = None
    assert _opcode.get_nb_ops()[0] == ("NB_ADD", "+")
    for reader, size, first in [
        (_opcode.get_intrinsic1_descs, 12, "INTRINSIC_1_INVALID"),
        (_opcode.get_intrinsic2_descs, 6, "INTRINSIC_2_INVALID"),
        (_opcode.get_special_method_names, 4, "__enter__"),
    ]:
        values = reader()
        assert len(values) == size and values[0] == first
        assert values is not reader()
        values[0] = "changed"
        assert reader()[0] == first


def check_executor():
    check_error(TypeError, _opcode.get_executor, None, 0)
    check_error(OverflowError, _opcode.get_executor, None, 1 << 100)
    try:
        _opcode.get_executor(code=check_executor.__code__, offset=0)
    except RuntimeError as exc:
        assert str(exc) == "Executors are not available in this build"
    except ValueError as exc:
        # A tier-2-enabled interpreter has no executor at this byte offset.
        assert str(exc) == "no executor at given byte offset"
    else:
        raise AssertionError("unexpected executor at function entry")
    stats = _opcode.get_specialization_stats()
    assert stats is None or isinstance(stats, dict)


def check_fresh_module():
    old = _opcode
    saved = old.stack_effect
    del sys.modules["_opcode"]
    try:
        import _opcode as fresh
        assert fresh is not old and fresh.stack_effect is not saved
        assert saved.__self__ is old and fresh.stack_effect.__self__ is fresh
        sys.modules["_opcode"] = None
        assert saved.__self__ is old and fresh.stack_effect.__self__ is fresh
        assert saved(31) == fresh.stack_effect(31) == -1
    finally:
        sys.modules["_opcode"] = old


check()
check_executor()
check_fresh_module()
print("opcode operations OK")
