from types import FunctionType, MethodType
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
    return _format_call_signature(name, *value)


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

    __ne__ = object.__ne__

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
        if len(self) == 2:
            name = "call"
            args, kwargs = self
        else:
            name, args, kwargs = self
            if not name:
                name = "call"
            elif name.startswith("()"):
                name = "call%s" % name
            else:
                name = "call.%s" % name
        return _format_call_signature(name, args, kwargs)

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
    return issubclass(type(value), NonCallableMock)


def _extract_mock(value):
    # Autospecced functions expose their backing mock via .mock.
    if isinstance(value, (FunctionType, MethodType)):
        return getattr(value, "mock", value)
    return value


def _async_object_type(value):
    try:
        return type(value)
    except NotImplementedError:
        # Some native module functions have no exposed Python type.
        return None


def _is_async_obj(value):
    from inspect import isawaitable, iscoroutinefunction

    cls = _async_object_type(value)
    if cls is not None and issubclass(cls, NonCallableMock) and not isinstance(value, AsyncMock):
        return False
    # CPython probes before fetching. A descriptor can change between the reads.
    if hasattr(value, "__func__"):
        value = value.__func__
        cls = _async_object_type(value)
    return iscoroutinefunction(value) or (cls is not None and isawaitable(value))


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


def _instance_callable(spec):
    if isinstance(spec, type):
        return any(base.__dict__.get("__call__") is not None for base in spec.__mro__)
    return getattr(spec, "__call__", None) is not None


def _spec_async_methods(spec, names):
    from inspect import getattr_static, iscoroutinefunction, unwrap

    result = set()
    for name in names:
        member = getattr_static(spec, name, None)
        if isinstance(member, (classmethod, staticmethod)):
            member = member.__func__
        try:
            member = unwrap(member)
        except ValueError:
            # A wrapper cycle can still carry its own coroutine marker.
            pass
        if iscoroutinefunction(member):
            result.add(name)
    return result


def _call_state_property(name):
    return property(lambda self: getattr(self._node, name),
                    lambda self, value: setattr(self._node, name, value))


class NonCallableMock(_FixedMock):
    called = _call_state_property("called")
    call_count = _call_state_property("call_count")
    call_args = _call_state_property("call_args")
    call_args_list = _call_state_property("call_args_list")
    _mock_sealed = False
    _mock_methods = None
    _mock_strict = False
    _spec_class = None
    _spec_asyncs = ()
    _mock_auto_magics = ()

    def __new__(cls, spec=None, *, spec_set=None, **kwargs):
        # Give each mock its own class for special-method lookup.
        bases = (cls,)
        if not issubclass(cls, AsyncMockMixin):
            spec_arg = spec_set or spec
            if spec_arg is not None and _is_async_obj(spec_arg):
                bases = (AsyncMockMixin, cls)
        return object.__new__(type(cls.__name__, bases, {}))

    def __init__(self, spec=None, *, spec_set=None, name=None, side_effect=None, return_value=_unset_return,
                 wraps=None, parent=None, _new_parent=None, _new_name="", _spec_as_instance=False,
                 _eat_self=None, unsafe=False, **attrs):
        if _eat_self is None:
            _eat_self = parent is not None
        self._mock_add_spec(spec_set if spec_set is not None else spec, spec_set is not None,
                            _spec_as_instance, _eat_self)
        self._mock_set_magics()
        object.__setattr__(self, "_mock_wraps", wraps)
        object.__setattr__(self, "_mock_unsafe", unsafe)
        super().__init__(name=name)
        self._mock_parent = parent
        self._mock_children = {}
        if _new_parent is None:
            _new_parent = parent
        if _new_parent is not None:
            self._node._owner = _new_parent
            self._node._suffix = _new_name if _new_name == "()" else "." + _new_name
        self.__dict__["_mock_mock_calls"] = _CallList()
        self.method_calls = _CallList()
        self.side_effect = side_effect
        if return_value is not _unset_return:
            self.return_value = return_value
        if attrs:
            self.configure_mock(**attrs)

    def _mock_add_spec(self, spec, strict, as_instance=False, eat_self=False):
        if _is_instance_mock(spec):
            raise InvalidSpecError(f"Cannot spec a Mock object. [object={spec!r}]")
        methods = spec
        spec_class = signature = None
        asyncs = ()
        if spec is not None and type(spec) not in (list, tuple):
            spec_class = spec if issubclass(type(spec), type) else type(spec)
            signature = _mock_signature(self, spec, as_instance, eat_self)
            methods = dir(spec)
            asyncs = _spec_async_methods(spec, methods)
        object.__setattr__(self, "_spec_signature", signature)
        object.__setattr__(self, "_spec_asyncs", asyncs)
        object.__setattr__(self, "_spec_class", spec_class)
        object.__setattr__(self, "_mock_methods", methods)
        object.__setattr__(self, "_mock_strict", strict)

    def mock_add_spec(self, spec, spec_set=False):
        self._mock_add_spec(spec, spec_set)

    def _mock_set_magics(self):
        owner = type(self)
        magics = owner._mock_auto_magics
        if not magics:
            return
        present = set(owner.__dict__)
        allowed = magics
        if self._mock_methods is not None:
            allowed = magics.intersection(self._mock_methods)
            for name in (magics - allowed) & present:
                delattr(self, name)
        for name in allowed - present:
            setattr(owner, name, _MagicSlot(name, self))

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

    def attach_mock(self, mock, attribute):
        """Reparent a mock so its calls are recorded under the given attribute."""
        inner = _extract_mock(mock)
        if _is_instance_mock(inner):
            node = inner._node
            node._mock_name = ""
            inner._mock_parent = None
            if hasattr(node, "_owner"):
                del node._owner
        else:
            # Writable objects can also be attached using the standard mock attributes.
            inner._mock_parent = inner._mock_new_parent = None
            inner._mock_name = ""
            inner._mock_new_name = None
        setattr(self, attribute, mock)

    def _attach(self, child, suffix):
        child._owner = self
        child._suffix = suffix

    def _adopt(self, value, suffix, *, magic=False):
        value = _extract_mock(value)
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
                value._mock_parent = self
            self._mock_children[name] = value
        return True

    def _extract_mock_name(self):
        node = self._node
        suffix = ""
        while hasattr(node, "_owner"):
            suffix = node._suffix + suffix
            node = node._owner._node
        return (node._mock_name or "mock") + suffix

    _mock_path = _extract_mock_name

    def __repr__(self):
        name = self._extract_mock_name()
        name_part = f" name={name!r}" if name not in ("mock", "mock.") else ""
        spec_part = ""
        if self._spec_class is not None:
            kind = "spec_set" if self._mock_strict else "spec"
            spec_part = f" {kind}={self._spec_class.__name__!r}"
        return f"<{type(self).__name__}{name_part}{spec_part} id='{id(self)}'>"

    def __dir__(self):
        if not FILTER_DIR:
            return object.__dir__(self)
        extras = self._mock_methods or []
        from_type = dir(type(self))
        from_dict = list(self.__dict__)
        children = [name for name, value in self._mock_children.items() if value is not _deleted]
        from_type = [name for name in from_type if not name.startswith("_")]
        from_dict = [name for name in from_dict
                     if not name.startswith("_") or "__%s__" % name[2:-2] == name]
        return sorted(set(extras + from_type + from_dict + children))

    def __setattr__(self, name, value):
        if name in _unsupported_magics:
            raise AttributeError(f"Attempting to set unsupported magic method {name!r}.")
        if name in _mock_magics:
            if self._mock_methods is not None and name not in self._mock_methods:
                raise AttributeError(f"Mock object has no attribute {name!r}")
            if _is_instance_mock(value):
                self._adopt(value, "." + name, magic=True)
                self._mock_children[name] = value
                member = stored = value
            else:
                def member(instance, /, *args, **kwargs):
                    return value(instance, *args, **kwargs)
                def stored(*args, **kwargs):
                    return value(self, *args, **kwargs)
            setattr(type(self), name, member)
            object.__setattr__(self, name, stored)
            return
        if name not in ("_node", "_nodes", "_mock_sealed", "_mock_parent", "_mock_children",
                        "called", "call_count", "call_args", "call_args_list",
                        "return_value", "side_effect", "mock_calls", "method_calls"):
            if self._mock_strict and name not in self._mock_methods and name not in self.__dict__:
                raise AttributeError(f"Mock object has no attribute {name!r}")
            if name == "__class__":
                object.__setattr__(self, "_spec_class", value)
                return
            adopted = self._adopt(value, "." + name)
            if self._mock_sealed and not adopted and not hasattr(self, name):
                raise AttributeError("Cannot set " + self._extract_mock_name() + "." + name)
            if isinstance(value, PropertyMock):
                # Installing the mock must not call the property's setter.
                self.__dict__[name] = value
                return
        object.__setattr__(self, name, value)

    def __delattr__(self, name):
        if name in _mock_magics and name in type(self).__dict__:
            delattr(type(self), name)
            if name not in self.__dict__:
                return
        if name in self.__dict__:
            object.__delattr__(self, name)
        elif self._mock_children.get(name) is _deleted:
            raise AttributeError(name)
        self._mock_children[name] = _deleted

    def _get_child_mock(self, /, **kwargs):
        # Each instance has a private subclass for its own protocol slots.
        factory = type(self).__mro__[1]
        name = kwargs.get("_new_name")
        if name in self._spec_asyncs or (name in _async_method_magics and issubclass(factory, MagicMock)):
            factory = AsyncMock
        elif issubclass(type(self), AsyncMockMixin):
            is_sync = name in _all_sync_magics or name in (self._mock_methods or ())
            factory = MagicMock if is_sync else AsyncMock
        elif not issubclass(factory, CallableMixin):
            factory = MagicMock if issubclass(factory, NonCallableMagicMock) else Mock
        return factory(**kwargs)

    def _get_child(self, name, *, magic=False):
        child = self._mock_children.get(name)
        if child is None or child is _deleted:
            kwargs = {} if magic else {"parent": self}
            if not magic and self._mock_wraps is not None:
                kwargs["wraps"] = getattr(self._mock_wraps, name)
            child = self._get_child_mock(name=name, _new_parent=self, _new_name=name, **kwargs)
            self._mock_children[name] = child
        return child

    def __getattr__(self, name):
        if name == "_mock_name":
            return self._node._mock_name
        methods = self._mock_methods
        if methods is not None and name not in methods:
            raise AttributeError(f"Mock object has no attribute {name!r}")
        if name.startswith("_") and (methods is None or name in _mock_magics):
            raise AttributeError(name)
        if methods is None and not self._mock_unsafe:
            _check_child_name(name)
        child = self._mock_children.get(name)
        if child is _deleted:
            raise AttributeError(name)
        members = getattr(self._node, "_autospec_members", {})
        if name in members:
            if child is None:
                value, eat_self = members[name]
                try:
                    child = _make_autospec(value, self._mock_strict, False, eat_self=eat_self)
                except InvalidSpecError:
                    target_name = self._node._mock_name or self
                    raise InvalidSpecError(
                        f"Cannot autospec attr {name!r} from target {target_name!r} "
                        f"as it has already been mocked out. [target={self!r}, attr={value!r}]"
                    )
                self._adopt(child, "." + name)
            object.__setattr__(self, name, child)
            if self._mock_sealed:
                seal(child)
            return child
        if self._mock_sealed and name not in self._mock_children:
            raise AttributeError(self._extract_mock_name() + "." + name)
        return self._get_child(name)

    @property
    def mock_calls(self):
        return self._mock_mock_calls

    @mock_calls.setter
    def mock_calls(self, calls):
        self.__dict__["_mock_mock_calls"] = calls

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
        if self._node._result == -1:
            if self._mock_wraps is not None:
                return DEFAULT
            if self._mock_sealed:
                raise AttributeError(self._extract_mock_name() + ".return_value")
        index = self._return_index()
        node = self._nodes[index]
        if hasattr(node, "_configured_return"):
            return node._configured_return
        value = self._get_child_mock(_new_parent=self, _new_name="()")
        self.return_value = value
        return value

    @return_value.setter
    def return_value(self, value):
        if value is DEFAULT:
            self._node._result = -1
            return
        self._adopt(value, "()")
        self._nodes[self._return_index()]._configured_return = value

    def _reset_mock_state(self, return_value, side_effect):
        self._node._reset_state(return_value, side_effect)
        if issubclass(type(self), AsyncMockMixin):
            self._reset_await_state()
        self.mock_calls = _CallList()
        self.method_calls = _CallList()

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
            current._reset_mock_state(clear_return, clear_effect)
            if node._result != -1:
                if isinstance(node, _FunctionMockState):
                    # The wrapper may since have been given a different return value.
                    result = current._nodes[node._result]._configured_return
                else:
                    result = current.return_value
                if _is_instance_mock(result):
                    pending.append((result, False, False))
            for child in current._mock_children.values():
                if child is not _deleted:
                    pending.append((child, clear_return, clear_effect))

    def _calls_repr(self):
        if not self.mock_calls:
            return ""
        return f"\nCalls: {safe_repr(self.mock_calls)}."

    def _call_matcher(self, recorded):
        # Normalize arguments for comparisons only. Keep the original spelling
        # for call_args, call_args_list and assertion failure messages.
        if not isinstance(recorded, tuple) or len(recorded) not in (2, 3):
            return recorded
        name, args, kwargs = _call_parts(recorded, len(recorded))
        mock = self
        sig = mock._spec_signature
        if name:
            sig = None
            for part in name.replace("()", "").split("."):
                child = mock._mock_children.get(part)
                if child is None:
                    break
                mock = child
                sig = mock._spec_signature
        if sig is None:
            return recorded
        try:
            bound = sig.bind(*args, **kwargs)
        except TypeError as error:
            return error.with_traceback(None)
        return call(name, bound.args, bound.kwargs)

    def _format_mock_call_signature(self, args, kwargs):
        return _format_recorded_call(_new_call(args, kwargs), str(self._mock_name or "mock"))

    def _assert_called_with(self, args, kwargs):
        self._assert_last_with(args, kwargs, self._node.call_args, "call")

    def _assert_last_with(self, args, kwargs, observed, action):
        expected = _new_call(args, kwargs)
        cause = None
        if observed is not None:
            match = self._call_matcher(expected)
            if not (self._call_matcher(observed) != match):
                return
            if isinstance(match, Exception):
                cause = match
        name = self._name()
        expected_text = _format_recorded_call(expected, name)
        if action == "await" and observed is None:
            raise AssertionError(f"Expected await: {expected_text}\nNot awaited")
        latest = self._node.call_args if action == "await" else observed
        actual = "not called." if latest is None else _format_recorded_call(latest, name)
        raise AssertionError(f"expected {action} not found.\nExpected: {expected_text}\n  Actual: {actual}") from cause

    def assert_called_with(self, /, *args, **kwargs):
        self._assert_called_with(args, kwargs)

    def assert_called_once_with(self, /, *args, **kwargs):
        self._assert_called_once_with(args, kwargs)

    def assert_has_calls(self, calls, any_order=False):
        # Iterate without consulting user length hints; they may record calls.
        expected = [self._call_matcher(item) for item in calls]
        cause = next((item for item in expected if isinstance(item, Exception)), None)
        actual = _CallList(self._call_matcher(item) for item in self.mock_calls)
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
        self._assert_any(args, kwargs, self._node.call_args_list, "call")

    def _assert_any(self, args, kwargs, history, action):
        expected = _new_call(args, kwargs)
        match = self._call_matcher(expected)
        cause = match if isinstance(match, Exception) else None
        # Equality hooks may record more calls. Check only the history at entry.
        if cause is None:
            for actual in [self._call_matcher(item) for item in history]:
                if all([left == right for left, right in zip(match, actual)]):
                    return
        raise AssertionError(f"{_format_recorded_call(expected, self._name())} {action} not found") from cause


# Binary operations have reflected and in-place forms. divmod has no in-place form.
_numeric_magics = "add sub mul matmul truediv floordiv mod lshift rshift and xor or pow".split()
_magics = {"__%s__" % name for name in (
    "lt le gt ge eq ne getitem setitem delitem len contains iter hash str sizeof enter exit "
    "divmod rdivmod neg pos abs invert complex int float index round trunc floor ceil bool next fspath aiter"
).split()}
_magics.update("__%s%s__" % (prefix, name) for name in _numeric_magics for prefix in ("", "r", "i"))
_async_method_magics = {"__aenter__", "__aexit__", "__anext__"}
_all_sync_magics = _magics | {"__%s__" % name for name in (
    "get set delete reversed missing reduce reduce_ex getinitargs getnewargs getstate setstate "
    "getformat repr dir subclasses format getnewargs_ex"
).split()}
_mock_magics = _all_sync_magics | _async_method_magics
_unsupported_magics = {"__getattr__", "__setattr__", "__init__", "__new__", "__prepare__",
                       "__instancecheck__", "__subclasscheck__", "__del__"}

_magic_return_values = {
    "__lt__": NotImplemented, "__gt__": NotImplemented, "__le__": NotImplemented, "__ge__": NotImplemented,
    "__int__": 1, "__index__": 1, "__float__": 1.0, "__complex__": 1j, "__bool__": True,
    "__len__": 0, "__contains__": False, "__exit__": False, "__aexit__": False,
}


class _AsyncIterator:
    def __init__(self, iterator):
        self.iterator = iterator

    async def __anext__(self):
        try:
            return next(self.iterator)
        except StopIteration:
            pass
        raise StopAsyncIteration


class _MagicSlot:
    def __init__(self, name, parent):
        self.name = name
        self.parent = parent

    def __get__(self, instance, owner):
        if self.parent._mock_sealed and self.parent._mock_children.get(self.name, _deleted) is _deleted:
            raise AttributeError(self.parent._extract_mock_name() + "." + self.name)
        child = self.parent._get_child(self.name, magic=True)
        setattr(self.parent, self.name, child)
        name = self.name
        parent = self.parent
        if name in _magic_return_values:
            child.return_value = _magic_return_values[name]
        elif name == "__hash__":
            child.return_value = object.__hash__(parent)
        elif name == "__str__":
            child.return_value = object.__str__(parent)
        elif name == "__sizeof__":
            child.return_value = object.__sizeof__(parent)
        elif name == "__fspath__":
            child.return_value = f"{type(parent).__name__}/{parent._extract_mock_name()}/{id(parent)}"
        elif name in ("__eq__", "__ne__"):
            def compare(other):
                if child._node._result != -1:
                    return child.return_value if name == "__eq__" else DEFAULT
                if parent is other:
                    return name == "__eq__"
                return NotImplemented
            child.side_effect = compare
        elif name in ("__iter__", "__aiter__"):
            def iterate():
                iterator = iter(()) if child._node._result == -1 else iter(child.return_value)
                return iterator if name == "__iter__" else _AsyncIterator(iterator)
            child.side_effect = iterate
        return child


class CallableMixin:
    def __call__(self, /, *args, **kwargs):
        signature = getattr(self._node, "_call_signature", None)
        if signature is not None:
            signature.bind(*args, **kwargs)
        self._increment_mock_call(*args, **kwargs)
        return self._mock_call(*args, **kwargs)

    def _increment_mock_call(self, /, *args, **kwargs):
        node = self._node
        node.called = True
        node._store_call(_new_call(args, kwargs))
        self.mock_calls.append(_Call(("", args, kwargs)))

        current = node
        path = ""
        separator = ""
        # An explicit parent records the method name separately from the mock_calls path.
        method_name = node._mock_name
        record_methods = self._mock_parent is not None
        while hasattr(current, "_owner"):
            suffix = current._suffix
            name = suffix.removeprefix(".")
            # mock_calls skips empty ancestor names and joins a leading "()"
            # without a dot, even when it was reached as a literal attribute.
            if name or current is node:
                path = name + separator + path
                separator = "" if name == "()" else "."
            parent = current._owner
            if record_methods:
                parent.method_calls.append(_Call((method_name, args, kwargs)))
                record_methods = parent._mock_parent is not None
                if record_methods:
                    method_name = parent._mock_name + "." + method_name
            parent.mock_calls.append(_Call((path, args, kwargs)))
            current = parent._node

    def _mock_call(self, /, *args, **kwargs):
        return self._execute_mock_call(*args, **kwargs)

    def _execute_mock_call(self, /, *args, **kwargs):
        effect = self.side_effect
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
        if self._node._result == -1 and self._mock_wraps is not None:
            if isinstance(self._node, _FunctionMockState) and self._node._function.return_value is not DEFAULT:
                return self.return_value
            return self._mock_wraps(*args, **kwargs)
        return self.return_value


class Mock(CallableMixin, NonCallableMock):
    pass


class MagicMock(Mock):
    _mock_auto_magics = _magics | _async_method_magics

    def mock_add_spec(self, spec, spec_set=False):
        self._mock_add_spec(spec, spec_set)
        self._mock_set_magics()


class AsyncMockMixin:
    def __init__(self, /, *args, **kwargs):
        from inspect import markcoroutinefunction

        super().__init__(*args, **kwargs)
        # Marking self would follow a descriptor spec's mocked __func__.
        marked = markcoroutinefunction(AsyncMockMixin._execute_mock_call)
        object.__setattr__(self, "_is_coroutine_marker", marked._is_coroutine_marker)
        self._reset_await_state()

    def _reset_await_state(self):
        object.__setattr__(self, "await_count", 0)
        object.__setattr__(self, "await_args", None)
        object.__setattr__(self, "await_args_list", _CallList())

    async def _execute_mock_call(self, /, *args, **kwargs):
        from inspect import iscoroutinefunction

        recorded = _new_call(args, kwargs)
        self.await_count += 1
        self.await_args = recorded
        self.await_args_list.append(recorded)
        effect = self.side_effect
        if effect is not None:
            if _is_exception(effect):
                raise effect
            if not _callable(effect):
                try:
                    result = next(effect)
                except StopIteration:
                    raise StopAsyncIteration
                if _is_exception(result):
                    raise result
            elif iscoroutinefunction(effect):
                result = await effect(*args, **kwargs)
            else:
                result = effect(*args, **kwargs)
            if result is not DEFAULT:
                return result
        if self._node._result == -1 and self._mock_wraps is not None:
            if iscoroutinefunction(self._mock_wraps):
                return await self._mock_wraps(*args, **kwargs)
            return self._mock_wraps(*args, **kwargs)
        return self.return_value

    def assert_not_awaited(self):
        if self.await_count != 0:
            raise AssertionError(f"Expected {self._name()} to not have been awaited. Awaited {self.await_count} times.")

    def assert_awaited(self):
        if self.await_count == 0:
            raise AssertionError(f"Expected {self._name()} to have been awaited.")

    def assert_awaited_once(self):
        if self.await_count != 1:
            raise AssertionError(f"Expected {self._name()} to have been awaited once. Awaited {self.await_count} times.")

    def assert_awaited_with(self, /, *args, **kwargs):
        self._assert_last_with(args, kwargs, self.await_args, "await")

    def assert_awaited_once_with(self, /, *args, **kwargs):
        self.assert_awaited_once()
        self.assert_awaited_with(*args, **kwargs)

    def assert_any_await(self, /, *args, **kwargs):
        self._assert_any(args, kwargs, self.await_args_list, "await")

    def assert_has_awaits(self, calls, any_order=False):
        expected = [self._call_matcher(item) for item in calls]
        cause = next((item for item in expected if isinstance(item, Exception)), None)
        actual = _CallList(self._call_matcher(item) for item in self.await_args_list)
        if not any_order:
            if expected not in actual:
                problem = "Awaits not found."
                if cause is not None:
                    errors = [item if isinstance(item, Exception) else None for item in expected]
                    problem = f"Error processing expected awaits.\nErrors: {errors}"
                raise AssertionError(f"{problem}\nExpected: {_CallList(calls)}\nActual: {self.await_args_list}") from cause
            return
        missing = []
        for item in expected:
            try:
                actual.remove(item)
            except ValueError:
                missing.append(item)
        if missing:
            raise AssertionError(f"{tuple(missing)!r} not all found in await list") from cause


class AsyncMock(AsyncMockMixin, Mock):
    """Record calls immediately, then execute their effects when awaited."""

    _mock_auto_magics = MagicMock._mock_auto_magics


class NonCallableMagicMock(NonCallableMock):
    _mock_auto_magics = MagicMock._mock_auto_magics
    mock_add_spec = MagicMock.mock_add_spec


def _spec_init(spec):
    from inspect import getattr_static, isdatadescriptor

    # The boxed set types expose a permissive initializer to static inspection,
    # but can lack its ordinary attribute, including on a Python subclass.
    if (issubclass(type(spec), type) and issubclass(spec, (set, frozenset))
            and getattr_static(spec, "__init__", None) is object.__init__):
        meta = type(spec)
        if (getattr_static(meta, "__getattribute__") is getattr_static(type, "__getattribute__")
                and getattr_static(meta, "__getattr__", _unset_return) is _unset_return
                and not isdatadescriptor(getattr_static(meta, "__init__", None))):
            return getattr(spec, "__init__", object.__init__)
    # User-defined initializers and metaclass hooks participate in normal lookup.
    return spec.__init__


def _mock_signature(mock, spec, as_instance, eat_self):
    from annotationlib import Format
    from functools import partial
    from inspect import signature

    if isinstance(spec, type) and not as_instance:
        target = _spec_init(spec)
        eat_self = True
    elif isinstance(spec, (classmethod, staticmethod)):
        target = spec.__func__
        eat_self = eat_self or isinstance(spec, classmethod)
    elif isinstance(spec, (FunctionType, MethodType)):
        target = spec
    else:
        target = getattr(spec, "__call__", _unset_return)
        if target is _unset_return:
            return None
        if as_instance and isinstance(spec, type) and not callable(mock) and callable(target):
            # A metaclass constructor cannot describe calls to its instances.
            return None
    if eat_self:
        target = partial(target, None)
    try:
        return signature(target, annotation_format=Format.FORWARDREF)
    except ValueError:
        # Some builtin callables do not expose a signature.
        return None


def _autospec_mock(cls, spec, strict, as_instance, eat_self, name=None, wraps=None):
    spec_arg = {"spec_set": spec} if strict else {"spec": spec}
    mock = cls(name=name, wraps=wraps, _spec_as_instance=as_instance, _eat_self=eat_self, **spec_arg)
    # A descriptor can return a different callable for each lookup. The spec
    # constructor resolved the matcher; resolve the invocation guard separately
    # before reading child attributes, as create_autospec does.
    call_sig = _mock_signature(mock, spec, as_instance, isinstance(spec, type) or eat_self)
    if call_sig is not None:
        mock._node._call_signature = call_sig
        type(mock).__signature__ = call_sig
    return mock


def _make_autospec(spec, strict, as_instance, name=None, *, eat_self=False, wraps=None):
    from dataclasses import fields, is_dataclass
    from inspect import getattr_static, iscoroutinefunction, isdatadescriptor

    if _is_instance_mock(spec):
        raise InvalidSpecError(f"Cannot autospec a Mock object. [object={spec!r}]")
    if type(spec) in (list, tuple):
        spec = type(spec)
    is_class = isinstance(spec, type)
    is_async = getattr(spec, "__code__", None) and iscoroutinefunction(spec)
    entries = dir(spec) if spec is not None else ()
    instance_fields = fields(spec) if is_class and as_instance and is_dataclass(spec) else ()
    if isdatadescriptor(spec):
        # A property viewed through its class has no known result type.
        return MagicMock(name=name)
    cls = MagicMock
    if is_async:
        if as_instance:
            raise RuntimeError("Instance can not be True when create_autospec is mocking an async function")
        cls = AsyncMock
    elif not _callable(spec) or (is_class and as_instance and not _instance_callable(spec)):
        cls = NonCallableMagicMock
    mock = _autospec_mock(cls, spec, strict, as_instance, eat_self, name, wraps)
    if is_class and not as_instance:
        mock.return_value = _make_autospec(spec, strict, True, eat_self=True, wraps=wraps)
    if spec is not None:
        # Capture each attribute once. Methods have signatures even before
        # attribute access, including on the class mock of a returned instance.
        members = {}
        for entry in entries:
            if entry.startswith("__") and entry.endswith("__"):
                continue
            try:
                value = getattr(spec, entry)
            except AttributeError:
                continue
            # Match the presence check on the wrapped object, but retain the
            # callable already obtained from the spec, including from a descriptor.
            child_wraps = value if wraps and hasattr(wraps, entry) and not strict else None
            # Only ordinary functions read from a class still need a receiver.
            skip_receiver = (is_class and isinstance(value, FunctionType)
                             and isinstance(getattr_static(spec, entry, value), FunctionType))
            members[entry] = (value, skip_receiver)
            if isinstance(value, (FunctionType, MethodType)):
                child_cls = AsyncMock if iscoroutinefunction(value) else MagicMock
                child = _autospec_mock(child_cls, value, strict, False, skip_receiver, wraps=child_wraps)
                child.return_value = child._get_child_mock()
                mock._adopt(child, "." + entry)
        # Fields without defaults need not appear in dir(spec). Their declared
        # types supply the instance attributes' specs.
        for field in instance_fields:
            mock._mock_methods.append(field.name)
            if "__%s__" % field.name[2:-2] != field.name:
                members[field.name] = (field.type, False)
        mock._node._autospec_members = members
    return mock


class _FunctionMockState(_Node):
    """Share the backing mock's call history with the function returned to the caller."""

    _fields = ("called", "call_count", "call_args", "call_args_list")

    def __init__(self, function, node):
        self._function = function
        for name, value in vars(node).items():
            setattr(self, name, value)

    def __getattr__(self, name):
        if name in self._fields:
            return getattr(self._function, name)
        raise AttributeError(name)

    def __setattr__(self, name, value):
        if name in self._fields:
            setattr(self._function, name, value)
        else:
            object.__setattr__(self, name, value)

    @property
    def side_effect(self):
        effect = self._function.side_effect
        if effect is not None and not _is_exception(effect) and not _callable(effect):
            if not hasattr(effect, "__next__"):
                effect = iter(effect)
                self._function.side_effect = effect
        return effect

    @side_effect.setter
    def side_effect(self, effect):
        self._function.side_effect = effect

    def _reset_state(self, reset_return, reset_effect):
        # Resetting the backing mock leaves the function's configured effect intact.
        super()._reset_state(reset_return, False)


def _autospec_function(spec, mock, sig, return_value):

    is_async = issubclass(type(mock), AsyncMock)
    if is_async:
        async def funcopy(*args, **kwargs):
            sig.bind(*args, **kwargs)
            return await mock(*args, **kwargs)
    else:
        def funcopy(*args, **kwargs):
            sig.bind(*args, **kwargs)
            return mock(*args, **kwargs)

    name = spec.__name__
    funcopy.__name__ = funcopy.__qualname__ = name if name.isidentifier() else "funcopy"
    funcopy.__signature__ = sig
    funcopy.mock = mock
    funcopy._mock_children = mock._mock_children
    if return_value is not DEFAULT:
        # Constructor-supplied mocks keep their own call histories.
        mock._nodes[mock._return_index()]._configured_return = return_value
    funcopy.return_value = mock.return_value
    funcopy.method_calls = _CallList()
    mock._node = _FunctionMockState(funcopy, mock._node)
    # Only the wrapper enforces the signature. Direct backing calls still use
    # the spec signature to normalize positional and keyword call assertions.
    mock._node._call_signature = None

    def get_return(backing):
        if funcopy.return_value is DEFAULT and backing._mock_wraps is None:
            funcopy.return_value = backing._get_child_mock(_new_parent=backing, _new_name="()")
        return funcopy.return_value

    # Each mock already has a private type for its protocol slots.
    owner = type(mock)
    owner.return_value = property(get_return, lambda backing, value: setattr(funcopy, "return_value", value))
    histories = ("mock_calls",)
    assertions = ("assert_called", "assert_called_once", "assert_not_called", "assert_called_with",
                  "assert_called_once_with", "assert_any_call", "assert_has_calls")
    if is_async:
        histories += ("await_count", "await_args", "await_args_list")
        assertions += ("assert_awaited", "assert_awaited_once", "assert_awaited_with", "assert_awaited_once_with",
                       "assert_any_await", "assert_has_awaits", "assert_not_awaited")
    for field in histories:
        setattr(funcopy, field, getattr(mock, field))
        setattr(owner, field, property(lambda backing, key=field: getattr(funcopy, key),
                                       lambda backing, value, key=field: setattr(funcopy, key, value)))
    for assertion in assertions:
        setattr(funcopy, assertion, getattr(mock, assertion))

    def reset_mock():
        funcopy.method_calls = _CallList()
        mock.reset_mock()
        result = funcopy.return_value
        if _is_instance_mock(result) and result is not mock:
            result.reset_mock()

    funcopy.reset_mock = reset_mock
    for entry, (value, _) in mock._node._autospec_members.items():
        if isinstance(value, (FunctionType, MethodType)):
            setattr(funcopy, entry, getattr(mock, entry))
    return funcopy


def _check_spec_arg_typos(kwargs):
    for typo in ("autospect", "auto_spec", "set_spec"):
        if typo in kwargs:
            raise RuntimeError(f"{typo!r} might be a typo; use unsafe=True if this is intended")


def create_autospec(spec, spec_set=False, instance=False, *, unsafe=False, **kwargs):
    """Create a mock that checks calls against the source signatures."""
    from types import BuiltinFunctionType

    if _is_instance_mock(spec):
        raise InvalidSpecError(f"Cannot autospec a Mock object. [object={spec!r}]")
    if isinstance(spec, MethodType):
        function = spec.__func__
        wrap_function = isinstance(function, FunctionType)
        # A bound builtin with an explicitly absent signature returns the mock itself.
        if not wrap_function and not (
            isinstance(function, BuiltinFunctionType) and getattr(function, "__text_signature__", False) is None
        ):
            raise NotImplementedError("create_autospec does not yet support standalone routines")
    else:
        wrap_function = isinstance(spec, FunctionType)
    if not unsafe:
        _check_spec_arg_typos(kwargs)
    wraps = kwargs.pop("wraps", None)
    if "parent" in kwargs:
        raise NotImplementedError("create_autospec does not support 'parent'")
    mock = _make_autospec(spec, spec_set, instance, kwargs.pop("name", None), wraps=wraps)
    result = mock
    if wrap_function:
        sig = getattr(mock._node, "_call_signature", None)
        if sig is not None:
            result = _autospec_function(spec, mock, sig, kwargs.pop("return_value", DEFAULT))
    if kwargs:
        mock.configure_mock(**kwargs)
    return result


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
        names = {name for name, child in current._mock_children.items() if child is not _deleted}
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


class PropertyMock(Mock):
    """Record property reads as calls without arguments and writes as calls with the value."""

    def _get_child_mock(self, /, **kwargs):
        return MagicMock(**kwargs)

    def __get__(self, obj, obj_type=None):
        return self()

    def __set__(self, obj, val):
        self(val)
