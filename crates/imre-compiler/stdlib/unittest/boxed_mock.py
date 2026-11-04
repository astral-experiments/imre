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
    return issubclass(type(value), NonCallableMock)


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


class NonCallableMock(_FixedMock):
    _mock_sealed = False
    _mock_methods = None
    _mock_strict = False
    _spec_class = None
    _spec_asyncs = ()
    _mock_auto_magics = ()

    def __new__(cls, spec=None, *, spec_set=None, **kwargs):
        # Give each mock its own class for special-method lookup.
        return object.__new__(type(cls.__name__, (cls,), {}))

    def __init__(self, spec=None, *, spec_set=None, name=None, side_effect=None, return_value=_unset_return,
                 parent=None, _new_parent=None, _new_name="", **attrs):
        methods = spec_set if spec_set is not None else spec
        if _is_instance_mock(methods):
            raise InvalidSpecError(f"Cannot spec a Mock object. [object={methods!r}]")
        original_spec = methods
        spec_class = None
        if issubclass(type(methods), type):
            spec_class = methods
            methods = dir(methods)
        elif methods is not None and type(methods) not in (list, tuple):
            spec_class = type(methods)
            methods = dir(methods)
        if spec_class is not None:
            object.__setattr__(self, "_spec_asyncs", _spec_async_methods(original_spec, methods))
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
        self._mock_parent = parent
        self._mock_children = {}
        if _new_parent is None:
            _new_parent = parent
        if _new_parent is not None:
            self._node._owner = _new_parent
            self._node._suffix = _new_name if _new_name == "()" else "." + _new_name
        self.mock_calls = _CallList()
        self.method_calls = _CallList()
        self.side_effect = side_effect
        if return_value is not _unset_return:
            self.return_value = return_value
        if attrs:
            for key in ("wraps", "unsafe"):
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

    def attach_mock(self, mock, attribute):
        """Reparent a mock so its calls are recorded under the given attribute."""
        if _is_instance_mock(mock):
            node = mock._node
            node._mock_name = ""
            mock._mock_parent = None
            if hasattr(node, "_owner"):
                del node._owner
        else:
            # Writable objects can also be attached using the standard mock attributes.
            mock._mock_parent = mock._mock_new_parent = None
            mock._mock_name = ""
            mock._mock_new_name = None
        setattr(self, attribute, mock)

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
                value._mock_parent = self
            self._mock_children[name] = value
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
        if name in ("called", "call_count", "call_args", "call_args_list"):
            setattr(self._node, name, value)
            return
        if name not in ("_node", "_nodes", "_mock_sealed", "_mock_parent", "_mock_children",
                        "return_value", "side_effect", "mock_calls", "method_calls"):
            if self._mock_strict and name not in self._mock_methods and name not in self.__dict__:
                raise AttributeError(f"Mock object has no attribute {name!r}")
            if name == "__class__":
                object.__setattr__(self, "_spec_class", value)
                return
            adopted = self._adopt(value, "." + name)
            if self._mock_sealed and not adopted and not hasattr(self, name):
                raise AttributeError("Cannot set " + self._mock_path() + "." + name)
            if isinstance(value, PropertyMock):
                # Installing the mock must not call the property's setter.
                self.__dict__[name] = value
                return
        object.__setattr__(self, name, value)

    def __delattr__(self, name):
        if name in _mock_magics and name in type(self).__dict__:
            delattr(type(self), name)
            self._mock_children.pop(name, None)
            if name in self.__dict__:
                object.__delattr__(self, name)
            return
        object.__delattr__(self, name)

    def _get_child_mock(self, /, **kwargs):
        # Each instance has a private subclass for its own protocol slots.
        factory = type(self).__mro__[1]
        name = kwargs.get("_new_name")
        if name in self._spec_asyncs:
            factory = AsyncMock
        elif issubclass(factory, AsyncMock):
            is_sync = name in _mock_magics or name in (self._mock_methods or ())
            factory = MagicMock if is_sync else AsyncMock
        elif not issubclass(factory, CallableMixin):
            factory = MagicMock if issubclass(factory, NonCallableMagicMock) else Mock
        return factory(**kwargs)

    def _get_child(self, name, *, magic=False):
        child = self._mock_children.get(name)
        if child is None:
            kwargs = {} if magic else {"parent": self}
            child = self._get_child_mock(name=name, _new_parent=self, _new_name=name, **kwargs)
            self._mock_children[name] = child
        return child

    def __getattr__(self, name):
        if name in ("called", "call_count", "call_args", "call_args_list", "_mock_name"):
            return getattr(self._node, name)
        methods = self._mock_methods
        if methods is not None and name not in methods:
            raise AttributeError(f"Mock object has no attribute {name!r}")
        if name.startswith("_") and (methods is None or name in _mock_magics):
            raise AttributeError(name)
        members = getattr(self._node, "_autospec_members", {})
        if name in members:
            value, target = members[name]
            child = _make_autospec(value, self._mock_strict, False, target=target)
            if isinstance(value, (FunctionType, MethodType)):
                # A spec method has a return value even if first read after seal().
                child.return_value = MagicMock()
            self._adopt(child, "." + name)
            object.__setattr__(self, name, child)
            if self._mock_sealed:
                seal(child)
            return child
        if self._mock_sealed and name not in self._mock_children:
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
            if issubclass(type(current), AsyncMock):
                current._reset_await_state()
            current.mock_calls = _CallList()
            current.method_calls = _CallList()
            if node._result != -1:
                result = current.return_value
                if _is_instance_mock(result):
                    pending.append((result, False, False))
            for child in current._mock_children.values():
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
        sig = getattr(mock._node, "_spec_signature", None)
        if name:
            sig = None
            for part in name.replace("()", "").split("."):
                child = mock._mock_children.get(part)
                if child is None:
                    break
                mock = child
                sig = getattr(mock._node, "_spec_signature", None)
        if sig is None:
            return recorded
        try:
            bound = sig.bind(*args, **kwargs)
        except TypeError as error:
            return error.with_traceback(None)
        return call(name, bound.args, bound.kwargs)

    def _assert_called_with(self, args, kwargs):
        self._assert_last_with(args, kwargs, self._node.call_args, "call")

    def _assert_last_with(self, args, kwargs, observed, action):
        expected = _new_call(args, kwargs)
        cause = None
        if observed is not None:
            match = self._call_matcher(expected)
            if self._call_matcher(observed) == match:
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
        if self.parent._mock_sealed and self.name not in self.parent._mock_children:
            raise AttributeError(self.parent._mock_path() + "." + self.name)
        child = self.parent._get_child(self.name, magic=True)
        setattr(self.parent, self.name, child)
        if self.name == "__exit__":
            child.return_value = False
        elif self.name == "__hash__":
            child.return_value = object.__hash__(self.parent)
        elif self.name == "__int__":
            child.return_value = 1
        elif self.name == "__iter__":
            def iterate():
                if child._node._result == -1:
                    return iter(())
                return iter(child.return_value)
            child.side_effect = iterate
        return child


class CallableMixin:
    def __call__(self, /, *args, **kwargs):
        node = self._node
        signature = getattr(node, "_call_signature", None)
        if signature is not None:
            signature.bind(*args, **kwargs)
        node.called = True
        node.call_args = _new_call(args, kwargs)
        node.call_args_list.append(node.call_args)
        node.call_count = len(node.call_args_list)
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
        return self.return_value


class Mock(CallableMixin, NonCallableMock):
    pass


class MagicMock(Mock):
    _mock_auto_magics = ("__enter__", "__exit__", "__getitem__", "__setitem__", "__hash__", "__int__", "__iter__", "__next__")


class AsyncMock(Mock):
    """Record calls immediately, then execute their effects when awaited."""

    _mock_auto_magics = MagicMock._mock_auto_magics

    def __init__(self, /, *args, **kwargs):
        from inspect import markcoroutinefunction

        super().__init__(*args, **kwargs)
        # The marker belongs to the instance, including when spec_set restricts writes.
        object.__setattr__(self, "_is_coroutine_marker", None)
        markcoroutinefunction(self)
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


class NonCallableMagicMock(NonCallableMock):
    _mock_auto_magics = MagicMock._mock_auto_magics


def _autospec_target(spec, name, value):
    from functools import partial
    from inspect import getattr_static
    from types import FunctionType

    # Only ordinary functions read from a class still need a receiver.
    if isinstance(value, FunctionType) and isinstance(spec, type):
        if isinstance(getattr_static(spec, name, value), FunctionType):
            return partial(value, None)
    return value


def _autospec_signature(target):
    from inspect import signature

    try:
        return signature(target)
    except ValueError:
        # Some builtin callables do not expose a signature.
        return None


def _make_autospec(spec, strict, as_instance, name=None, target=None, *, eat_self=False):
    from dataclasses import fields, is_dataclass
    from functools import partial
    from inspect import iscoroutinefunction, isdatadescriptor

    if _is_instance_mock(spec):
        raise InvalidSpecError("Cannot autospec a Mock object")
    if type(spec) in (list, tuple):
        spec = type(spec)
    if isdatadescriptor(spec):
        # A property viewed through its class has no known result type.
        return MagicMock(name=name)
    is_class = isinstance(spec, type)
    cls = MagicMock
    if is_class:
        if not as_instance:
            target = partial(spec.__init__, None)
        else:
            if not _instance_callable(spec):
                cls = NonCallableMagicMock
            target = getattr(spec, "__call__", _unset_return)
            if cls is NonCallableMagicMock and callable(target):
                # The metaclass constructor does not model an instance call.
                target = _unset_return
    elif isinstance(spec, classmethod):
        target = partial(spec.__func__, None)
    elif isinstance(spec, staticmethod):
        target = spec.__func__
    elif target is None and isinstance(spec, (FunctionType, MethodType)):
        target = spec
    elif target is None:
        # Inspect the call wrapper itself, as CPython does. Absence means no
        # signature; an explicitly noncallable wrapper still raises TypeError.
        target = getattr(spec, "__call__", _unset_return)
        if target is _unset_return and not callable(spec):
            cls = NonCallableMagicMock
    if target is not _unset_return and iscoroutinefunction(target):
        raise NotImplementedError("autospeccing async callables is not supported")
    mock = cls(spec_set=spec, name=name) if strict else cls(spec=spec, name=name)
    if spec is not None:
        # Capture the original attributes now; build their mocks on first access.
        members = {}
        for entry in mock._mock_methods:
            if entry.startswith("__") and entry.endswith("__"):
                continue
            try:
                value = getattr(spec, entry)
            except AttributeError:
                continue
            target_method = None
            if isinstance(value, FunctionType):
                target_method = _autospec_target(spec, entry, value)
            members[entry] = (value, target_method)
        if is_class and as_instance and is_dataclass(spec):
            # Fields without defaults need not appear in dir(spec). The declared
            # type supplies the instance attribute's spec even when it has a default.
            for field in fields(spec):
                mock._mock_methods.append(field.name)
                if "__%s__" % field.name[2:-2] != field.name:
                    members[field.name] = (field.type, None)
        mock._node._autospec_members = members
    if target is not _unset_return:
        # Calls supply the receiver. Assertions on an explicit instance spec
        # use the raw signature, while a factory's return mock eats self too.
        if eat_self:
            target = partial(target, None)
        sig = _autospec_signature(target)
        call_sig = _autospec_signature(partial(target, None)) if is_class and as_instance and not eat_self else sig
        mock._node._spec_signature = sig
        if call_sig is not None:
            mock._node._call_signature = call_sig
            type(mock).__signature__ = call_sig
    if is_class and not as_instance:
        mock.return_value = _make_autospec(spec, strict, True, eat_self=True)
    return mock


def create_autospec(spec, spec_set=False, instance=False, *, unsafe=False, **kwargs):
    """Create a mock of a class or instance, checking calls against the source signatures."""
    from types import BuiltinFunctionType

    if _is_instance_mock(spec):
        raise InvalidSpecError("Cannot autospec a Mock object")
    if isinstance(spec, (FunctionType, MethodType)):
        # Python functions and methods need a delegating function exposing .mock.
        # A bound builtin with an explicitly absent signature returns the mock itself.
        if not (
            isinstance(spec, MethodType)
            and isinstance(spec.__func__, BuiltinFunctionType)
            and getattr(spec.__func__, "__text_signature__", False) is None
        ):
            raise NotImplementedError("create_autospec does not yet support standalone routines")
    if not unsafe:
        for typo in ("autospect", "auto_spec", "set_spec"):
            if typo in kwargs:
                raise RuntimeError(f"{typo!r} might be a typo; use unsafe=True if this is intended")
    for option in ("wraps", "parent"):
        if option in kwargs:
            raise NotImplementedError(f"create_autospec does not support {option!r}")
    mock = _make_autospec(spec, spec_set, instance, kwargs.pop("name", None))
    if kwargs:
        mock.configure_mock(**kwargs)
    return mock


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
        names = set(current._mock_children)
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
