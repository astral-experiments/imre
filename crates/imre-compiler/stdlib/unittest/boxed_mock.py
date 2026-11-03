from types import MethodType
from unittest.util import safe_repr


class _ANY:
    def __eq__(self, other):
        return True

    def __ne__(self, other):
        return False

    def __repr__(self):
        return "<ANY>"


ANY = _ANY()


class _CallList(list):
    def __contains__(self, value):
        if not isinstance(value, list):
            return any(item is value or item == value for item in self)
        size = len(value)
        return any(self[i:i + size] == value for i in range(len(self) - size + 1))

    def __repr__(self):
        return _format_list([repr(value) for value in self], True)


def _call_parts(value, size, name=""):
    args, kwargs = (), {}
    if size == 3:
        name, args, kwargs = value
    elif size == 2:
        first, second = value
        if not isinstance(first, str):
            args, kwargs = first, second
        elif isinstance(second, tuple):
            name, args = first, second
        else:
            name, kwargs = first, second
    elif size == 1:
        first, = value
        if isinstance(first, str):
            name = first
        elif isinstance(first, tuple):
            args = first
        else:
            kwargs = first
    return name, args, kwargs


# The boxed tuple type does not expose its class dictionary. These are the
# members in CPython 3.14's tuple.__dict__ that _Call hides on explicit lookup.
_tuple_call_names = {
    "__new__", "__repr__", "__hash__", "__lt__", "__le__", "__eq__", "__ne__",
    "__gt__", "__ge__", "__iter__", "__len__", "__getitem__", "__add__", "__mul__",
    "__rmul__", "__contains__", "__getnewargs__", "index", "count",
    "__class_getitem__", "__doc__",
}


def _format_recorded_call(value, name):
    parts = [repr(arg) for arg in value.args]
    parts.extend(key + "=" + repr(arg) for key, arg in value.kwargs.items())
    return name + "(" + ", ".join(parts) + ")"


class _Call(tuple):
    def __new__(cls, value=(), name="", parent=None, two=False, from_kall=True):
        name, args, kwargs = _call_parts(value, len(value), name)
        return tuple.__new__(cls, (args, kwargs) if two else (name, args, kwargs))

    def __init__(self, value=(), name=None, parent=None, two=False, from_kall=True):
        self._mock_name = name
        self._mock_parent = parent
        self._mock_from_kall = from_kall

    @property
    def args(self):
        return self[-2]

    @property
    def kwargs(self):
        return self[-1]

    def __eq__(self, other):
        try:
            size = len(other)
        except TypeError:
            return NotImplemented
        if (self._mock_parent and getattr(other, "_mock_parent", None)
                and self._mock_parent != other._mock_parent):
            return False
        if size > 3:
            return False
        name, args, kwargs = _call_parts(other, size)
        if len(self) == 3 and self[0] and self[0] != name:
            return False
        # Let the expected arguments' comparison hooks run first.
        return (args, kwargs) == (self.args, self.kwargs)

    def __ne__(self, other):
        equal = _Call.__eq__(self, other)
        if equal is NotImplemented:
            return NotImplemented
        return not equal

    def __call__(self, /, *args, **kwargs):
        name = self._mock_name
        if name is None:
            return _Call(("", args, kwargs), name="()")
        return _Call((name, args, kwargs), name=name + "()", parent=self)

    def __getattr__(self, attr):
        name = self._mock_name
        if name is None:
            return _Call(name=attr, from_kall=False)
        return _Call(name=name + "." + attr, parent=self, from_kall=False)

    def __getattribute__(self, attr):
        # A tuple method such as count can name a call in the same way as any other attribute.
        if attr in _tuple_call_names:
            raise AttributeError(attr)
        return object.__getattribute__(self, attr)

    def __repr__(self):
        if not self._mock_from_kall:
            name = self._mock_name or "call"
            return "call" + name if name.startswith("()") else name
        name = self[0] if len(self) == 3 else ""
        if not name:
            return _format_recorded_call(self, "call")
        prefix = "call" if name.startswith("()") else "call."
        return _format_recorded_call(self, prefix + name)

    def call_list(self):
        calls = []
        current = self
        while current is not None:
            if current._mock_from_kall:
                calls.append(current)
            current = current._mock_parent
        return _CallList(reversed(calls))


call = _Call(from_kall=False)


def _new_call(args, kwargs):
    return _Call((args, kwargs), two=True)


def _new_call_list():
    return _CallList()


_FixedMock = Mock
_unset_return = object()


class InvalidSpecError(Exception):
    """Indicates that an invalid value was used as a mock spec."""


def _is_instance_mock(value):
    # A spec or proxy can report a different __class__.
    return issubclass(type(value), Mock)


def _is_exception(value):
    return isinstance(value, BaseException) or isinstance(value, type) and issubclass(value, BaseException)


def _callable(value):
    if isinstance(value, type):
        return True
    if isinstance(value, (staticmethod, classmethod, MethodType)):
        return _callable(value.__func__)
    # Some boxed native callables lack a visible __call__ wrapper. An explicit
    # __call__ of None still makes an iterable the source of successive effects.
    return getattr(value, "__call__", callable(value) or None) is not None


class Mock(_FixedMock):
    _mock_sealed = False
    _mock_methods = None
    _mock_strict = False
    _spec_class = None
    _mock_auto_magics = ()

    def __new__(cls, spec=None, *, spec_set=None, **kwargs):
        # Give each mock its own class for special-method lookup.
        return object.__new__(type(cls.__name__, (cls,), {}))

    def __init__(self, spec=None, *, spec_set=None, name=None, side_effect=None, return_value=_unset_return, **attrs):
        methods = spec_set if spec_set is not None else spec
        if _is_instance_mock(methods):
            raise InvalidSpecError(f"Cannot spec a Mock object. [object={methods!r}]")
        spec_class = None
        if issubclass(type(methods), type):
            spec_class = methods
            methods = dir(methods)
        elif methods is not None and type(methods) not in (list, tuple):
            raise NotImplementedError("mock specifications from objects are not supported")
        object.__setattr__(self, "_spec_class", spec_class)
        owner = type(self)
        for magic in owner._mock_auto_magics:
            if methods is None or magic in methods:
                if magic not in owner.__dict__:
                    setattr(owner, magic, _MagicSlot(magic, self))
            elif magic in owner.__dict__:
                delattr(self, magic)
        object.__setattr__(self, "_mock_methods", methods)
        object.__setattr__(self, "_mock_strict", spec_set is not None)
        super().__init__(name=name)
        self.mock_calls = _CallList()
        self.method_calls = _CallList()
        self.side_effect = side_effect
        if return_value is not _unset_return:
            self.return_value = return_value
        if attrs:
            for key in ("wraps", "parent", "unsafe"):
                if key in attrs:
                    raise TypeError(f"Mock.__init__() got an unexpected keyword argument '{key}'")
            self.configure_mock(**attrs)

    @property
    def __class__(self):
        if self._spec_class is None:
            return type(self)
        return self._spec_class

    def configure_mock(self, /, **attrs):
        for path in sorted(attrs, key=lambda name: name.count(".")):
            target = self
            parts = path.split(".")
            for part in parts[:-1]:
                target = getattr(target, part)
            setattr(target, parts[-1], attrs[path])

    def _attach(self, child, suffix):
        child._owner = self
        child._suffix = suffix

    def _adopt(self, value, suffix, *, magic=False):
        if not _is_instance_mock(value) or value._node._mock_name or hasattr(value._node, "_owner"):
            return False
        ancestor = self
        while ancestor is not None:
            if ancestor is value:
                return False
            ancestor = getattr(ancestor._node, "_owner", None)
        self._attach(value._node, suffix)
        if suffix != "()":
            name = suffix[1:]
            if not magic:
                value._node._mock_name = name
            if not hasattr(self._node, "_assigned_children"):
                self._node._assigned_children = {}
            self._node._assigned_children[name] = value
        return True

    def _mock_path(self):
        node = self._node
        suffix = ""
        while hasattr(node, "_owner"):
            suffix = node._suffix + suffix
            node = node._owner._node
        return (node._mock_name or "mock") + suffix

    def __setattr__(self, name, value):
        if name in _mock_magics:
            if self._mock_methods is not None and name not in self._mock_methods:
                raise AttributeError(f"Mock object has no attribute {name!r}")
            if _is_instance_mock(value):
                if self._adopt(value, "." + name, magic=True):
                    value._node._magic = True
                if not hasattr(self._node, "_assigned_children"):
                    self._node._assigned_children = {}
                self._node._assigned_children[name] = value
                member = stored = value
            else:
                def member(instance, /, *args, **kwargs):
                    return value(instance, *args, **kwargs)
                def stored(*args, **kwargs):
                    return value(self, *args, **kwargs)
            setattr(type(self), name, member)
            object.__setattr__(self, name, stored)
            return
        if name in ("called", "call_count", "call_args", "call_args_list"):
            setattr(self._node, name, value)
            return
        if name not in ("_node", "_nodes", "_mock_sealed", "return_value", "side_effect", "mock_calls", "method_calls"):
            if self._mock_strict and name not in self._mock_methods and name not in self.__dict__:
                raise AttributeError(f"Mock object has no attribute {name!r}")
            if name == "__class__":
                object.__setattr__(self, "_spec_class", value)
                return
            adopted = self._adopt(value, "." + name)
            if self._mock_sealed and not adopted and not hasattr(self, name):
                raise AttributeError("Cannot set " + self._mock_path() + "." + name)
        object.__setattr__(self, name, value)

    def __delattr__(self, name):
        if name in _mock_magics and name in type(self).__dict__:
            delattr(type(self), name)
            self._node._children.pop(name, None)
            getattr(self._node, "_assigned_children", {}).pop(name, None)
            if name in self.__dict__:
                object.__delattr__(self, name)
            return
        object.__delattr__(self, name)

    def _at(self, index):
        node = self._nodes[index]
        if not hasattr(node, "_proxy"):
            # Each instance has a private subclass for its own protocol slots.
            factory = type(self).__mro__[1]
            proxy = factory()
            proxy._node = node
            proxy._nodes = self._nodes
            node._proxy = proxy
        return node._proxy

    def __getattr__(self, name):
        if name in ("called", "call_count", "call_args", "call_args_list", "_mock_name"):
            return getattr(self._node, name)
        methods = self._mock_methods
        if methods is not None and name not in methods:
            raise AttributeError(f"Mock object has no attribute {name!r}")
        if name.startswith("_") and (methods is None or name in _mock_magics):
            raise AttributeError(name)
        if self._mock_sealed and name not in self._node._children:
            if methods is None:
                _check_child_name(name)
            raise AttributeError(self._mock_path() + "." + name)
        if methods is not None:
            return self._get_child(name)
        return self._child(name)

    @property
    def side_effect(self):
        return self._node.side_effect

    @side_effect.setter
    def side_effect(self, effect):
        if effect is not None and not _is_exception(effect) and not _callable(effect):
            try:
                effect = iter(effect)
            except TypeError:
                # Like CPython, keep a non-iterable value until the mock is called.
                pass
        self._node.side_effect = effect

    @property
    def return_value(self):
        if self._mock_sealed and self._node._result == -1:
            raise AttributeError(self._mock_path() + ".return_value")
        child = super().return_value
        return getattr(child._node, "_configured_return", child)

    @return_value.setter
    def return_value(self, value):
        if value is DEFAULT:
            self._node._result = -1
            return
        self._adopt(value, "()")
        child = super().return_value
        child._node._configured_return = value

    def __call__(self, /, *args, **kwargs):
        node = self._node
        node.called = True
        node.call_args = _new_call(args, kwargs)
        node.call_args_list.append(node.call_args)
        node.call_count = len(node.call_args_list)
        self.mock_calls.append(_Call(("", args, kwargs)))

        current = node
        path = ""
        separator = ""
        method_suffix = ""
        while hasattr(current, "_owner"):
            suffix = current._suffix
            name = suffix.removeprefix(".")
            # mock_calls skips empty ancestor names and joins a leading "()"
            # without a dot, even when it was reached as a literal attribute.
            if name or current is node:
                path = name + separator + path
                separator = "" if name == "()" else "."
            parent = current._owner
            if suffix == "()" or getattr(current, "_magic", False):
                method_suffix = None
            if method_suffix is not None:
                # Only real return-value edges stop method_calls propagation.
                method_suffix = suffix + method_suffix
                parent.method_calls.append(_Call((method_suffix[1:], args, kwargs)))
            parent.mock_calls.append(_Call((path, args, kwargs)))
            current = parent._node

        effect = node.side_effect
        if effect is not None:
            if _is_exception(effect):
                raise effect
            if _callable(effect):
                result = effect(*args, **kwargs)
            else:
                result = next(effect)
                if _is_exception(result):
                    raise result
            if result is not DEFAULT:
                return result
        return self.return_value

    def reset_mock(self, visited=None, *, return_value=False, side_effect=False):
        if visited is None:
            visited = []
        pending = [(self, return_value, side_effect)]
        while pending:
            current, clear_return, clear_effect = pending.pop()
            if id(current) in visited:
                continue
            visited.append(id(current))
            node = current._node
            # A generated magic method retains its protocol return value on reset.
            if clear_return and issubclass(type(current), MagicMock) and node._mock_name:
                name = node._mock_name
                if "__%s__" % name[2:-2] == name:
                    clear_return = False
            node._reset_state(clear_return, clear_effect)
            current.mock_calls = _CallList()
            current.method_calls = _CallList()
            if node._result != -1:
                result = current.return_value
                if _is_instance_mock(result):
                    pending.append((result, False, False))
            children = {name: current._at(index) for name, index in node._children.items()}
            children.update(getattr(node, "_assigned_children", {}))
            for child in children.values():
                pending.append((child, clear_return, clear_effect))

    def _calls_repr(self):
        if not self.mock_calls:
            return ""
        return f"\nCalls: {safe_repr(self.mock_calls)}."

    def assert_called_with(self, /, *args, **kwargs):
        self._assert_called_with(args, kwargs)

    def assert_called_once_with(self, /, *args, **kwargs):
        self._assert_called_once_with(args, kwargs)

    def assert_has_calls(self, calls, any_order=False):
        # Iterate without consulting user length hints; they may record calls.
        expected = [item for item in calls]
        cause = next((item for item in expected if isinstance(item, Exception)), None)
        actual = _CallList(item for item in self.mock_calls)
        if not any_order:
            if expected not in actual:
                problem = "Calls not found."
                if cause is not None:
                    errors = [item if isinstance(item, Exception) else None for item in expected]
                    problem = f"Error processing expected calls.\nErrors: {errors}"
                raise AssertionError(f"{problem}\nExpected: {_CallList(calls)}\n  Actual: {safe_repr(self.mock_calls)}") from cause
            return

        remaining = list(actual)
        missing = []
        for call in expected:
            try:
                remaining.remove(call)
            except ValueError:
                missing.append(call)
        if missing:
            raise AssertionError(f"{self._name()!r} does not contain all of {tuple(missing)!r} in its call list, found {remaining!r} instead") from cause

    def assert_any_call(self, /, *args, **kwargs):
        expected = _new_call(args, kwargs)
        # Equality hooks may record more calls. Check only the history at entry.
        for actual in [item for item in self._node.call_args_list]:
            assert len(expected) == len(actual)
            if all([left == right for left, right in zip(expected, actual)]):
                return
        raise AssertionError(_format_recorded_call(expected, self._name()) + " call not found")


# CPython's protocol names must not fall through to ordinary attribute children.
_mock_magics = set("__%s__" % name for name in (
    "abs add aenter aexit aiter and anext bool ceil complex contains delete delitem dir divmod "
    "enter eq exit float floor floordiv format fspath ge get getformat getinitargs getitem "
    "getnewargs getnewargs_ex getstate gt hash iadd iand ifloordiv ilshift imatmul imod imul "
    "index int invert ior ipow irshift isub iter itruediv ixor le len lshift lt matmul missing "
    "mod mul ne neg next or pos pow radd rand rdivmod reduce reduce_ex repr reversed rfloordiv "
    "rlshift rmatmul rmod rmul ror round rpow rrshift rshift rsub rtruediv rxor set setitem "
    "setstate sizeof str sub subclasses truediv trunc xor"
).split())


class _MagicSlot:
    def __init__(self, name, parent):
        self.name = name
        self.parent = parent

    def __get__(self, instance, owner):
        if self.parent._mock_sealed and self.name not in self.parent._node._children:
            raise AttributeError(self.parent._mock_path() + "." + self.name)
        child = self.parent._get_child(self.name)
        child._node._magic = True
        setattr(self.parent, self.name, child)
        if self.name == "__exit__":
            child.return_value = False
        elif self.name == "__iter__":
            def iterate():
                if child._node._result == -1:
                    return iter(())
                return iter(child.return_value)
            child.side_effect = iterate
        return child


class MagicMock(Mock):
    _mock_auto_magics = ("__enter__", "__exit__", "__getitem__", "__iter__", "__next__")


def seal(mock):
    """Prevent new children from being created throughout this mock's owned tree."""
    pending = [mock]
    seen = set()
    while pending:
        current = pending.pop()
        if id(current) in seen:
            continue
        seen.add(id(current))
        current._mock_sealed = True
        names = set(current._node._children)
        names.update(current.__dict__)
        if current._node._result != -1:
            names.add("return_value")
        for name in names:
            value = getattr(current, name)
            if _is_instance_mock(value) and getattr(value._node, "_owner", None) is current:
                pending.append(value)


# CPython 3.14.5 Lib/unittest/mock.py
file_spec = None
open_spec = None


def _to_stream(read_data):
    import io
    if isinstance(read_data, bytes):
        return io.BytesIO(read_data)
    else:
        return io.StringIO(read_data)


def mock_open(mock=None, read_data=''):
    """
    A helper function to create a mock to replace the use of `open`. It works
    for `open` called directly or used as a context manager.

    The `mock` argument is the mock object to configure. If `None` (the
    default) then a `MagicMock` will be created for you, with the API limited
    to methods or attributes available on standard file handles.

    `read_data` is a string for the `read`, `readline` and `readlines` of the
    file handle to return.  This is an empty string by default.
    """
    _read_data = _to_stream(read_data)
    _state = [_read_data, None]

    def _readlines_side_effect(*args, **kwargs):
        if handle.readlines.return_value is not None:
            return handle.readlines.return_value
        return _state[0].readlines(*args, **kwargs)

    def _read_side_effect(*args, **kwargs):
        if handle.read.return_value is not None:
            return handle.read.return_value
        return _state[0].read(*args, **kwargs)

    def _readline_side_effect(*args, **kwargs):
        yield from _iter_side_effect()
        while True:
            yield _state[0].readline(*args, **kwargs)

    def _iter_side_effect():
        if handle.readline.return_value is not None:
            while True:
                yield handle.readline.return_value
        for line in _state[0]:
            yield line

    def _next_side_effect():
        if handle.readline.return_value is not None:
            return handle.readline.return_value
        return next(_state[0])

    def _exit_side_effect(exctype, excinst, exctb):
        handle.close()

    global file_spec
    if file_spec is None:
        import _io
        file_spec = list(set(dir(_io.TextIOWrapper)).union(set(dir(_io.BytesIO))))

    global open_spec
    if open_spec is None:
        import _io
        open_spec = list(set(dir(_io.open)))
    if mock is None:
        mock = MagicMock(name='open', spec=open_spec)

    handle = MagicMock(spec=file_spec)
    handle.__enter__.return_value = handle

    handle.write.return_value = None
    handle.read.return_value = None
    handle.readline.return_value = None
    handle.readlines.return_value = None

    handle.read.side_effect = _read_side_effect
    _state[1] = _readline_side_effect()
    handle.readline.side_effect = _state[1]
    handle.readlines.side_effect = _readlines_side_effect
    handle.__iter__.side_effect = _iter_side_effect
    handle.__next__.side_effect = _next_side_effect
    handle.__exit__.side_effect = _exit_side_effect

    def reset_data(*args, **kwargs):
        _state[0] = _to_stream(read_data)
        if handle.readline.side_effect == _state[1]:
            # Only reset the side effect if the user hasn't overridden it.
            _state[1] = _readline_side_effect()
            handle.readline.side_effect = _state[1]
        return DEFAULT

    mock.side_effect = reset_data
    mock.return_value = handle
    return mock
