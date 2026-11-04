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
    if not isinstance(spec, type):
        return getattr(spec, "__call__", None) is not None
    from inspect import getattr_static

    for base in spec.__mro__:
        try:
            namespace = base.__dict__
        except AttributeError:
            if type(base) is not type:
                raise
            # Native bases can expose descriptors without a Python namespace.
            # The metaclass's __call__ describes calling the class itself.
            call = getattr_static(base, "__call__", None)
            if getattr(call, "__objclass__", None) is base:
                return True
        else:
            if namespace.get("__call__") is not None:
                return True
    return False


class NonCallableMock(_FixedMock):
    _mock_sealed = False
    _mock_methods = None
    _mock_strict = False
    _spec_class = None
    _mock_auto_magics = ()

    def __new__(cls, spec=None, *, spec_set=None, **kwargs):
        # Give each mock its own class for special-method lookup.
        return object.__new__(type(cls.__name__, (cls,), {}))

    def __init__(self, spec=None, *, spec_set=None, name=None, side_effect=None, return_value=_unset_return,
                 _new_parent=None, _new_name="", **attrs):
        methods = spec_set if spec_set is not None else spec
        if _is_instance_mock(methods):
            raise InvalidSpecError(f"Cannot spec a Mock object. [object={methods!r}]")
        spec_class = None
        if issubclass(type(methods), type):
            spec_class = methods
            methods = dir(methods)
        elif methods is not None and type(methods) not in (list, tuple):
            spec_class = type(methods)
            methods = dir(methods)
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
        if _new_parent is not None:
            suffix = _new_name if _new_name == "()" else "." + _new_name
            _new_parent._attach(self._node, suffix)
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

    def attach_mock(self, mock, attribute):
        """Reparent a mock so its calls are recorded under the given attribute."""
        if _is_instance_mock(mock):
            node = mock._node
            node._mock_name = ""
            node._magic = False
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
            if isinstance(value, PropertyMock):
                # Installing the mock must not call the property's setter.
                self.__dict__[name] = value
                return
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

    def _get_child_mock(self, /, **kwargs):
        # Each instance has a private subclass for its own protocol slots.
        factory = type(self).__mro__[1]
        if not issubclass(factory, CallableMixin):
            factory = MagicMock if issubclass(factory, NonCallableMagicMock) else Mock
        return factory(**kwargs)

    def _at(self, index):
        node = self._nodes[index]
        if not hasattr(node, "_proxy"):
            proxy = self._get_child_mock()
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
        members = getattr(self._node, "_autospec_members", {})
        if name in members:
            value, target = members[name]
            child = _make_autospec(value, self._mock_strict, False, target=target)
            self._adopt(child, "." + name)
            object.__setattr__(self, name, child)
            if self._mock_sealed:
                seal(child)
            return child
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
        index = self._return_index()
        node = self._nodes[index]
        if hasattr(node, "_configured_return"):
            return node._configured_return
        return self._at(index)

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
                node = mock._node
                child = getattr(node, "_assigned_children", {}).get(part)
                if child is None:
                    if part not in node._children:
                        break
                    child = mock._at(node._children[part])
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
        expected = _new_call(args, kwargs)
        observed = self._node.call_args
        cause = None
        if observed is not None:
            match = self._call_matcher(expected)
            if self._call_matcher(observed) == match:
                return
            if isinstance(match, Exception):
                cause = match
        name = self._name()
        actual = "not called." if observed is None else _format_recorded_call(observed, name)
        raise AssertionError("expected call not found.\nExpected: " +
                             _format_recorded_call(expected, name) + "\n  Actual: " + actual) from cause

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
        expected = _new_call(args, kwargs)
        match = self._call_matcher(expected)
        cause = match if isinstance(match, Exception) else None
        # Equality hooks may record more calls. Check only the history at entry.
        if cause is None:
            for actual in [self._call_matcher(item) for item in self._node.call_args_list]:
                if all([left == right for left, right in zip(match, actual)]):
                    return
        raise AssertionError(_format_recorded_call(expected, self._name()) + " call not found") from cause


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
        signature = getattr(node, "_spec_signature", None)
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


class Mock(CallableMixin, NonCallableMock):
    pass


class MagicMock(Mock):
    _mock_auto_magics = ("__enter__", "__exit__", "__getitem__", "__setitem__", "__hash__", "__int__", "__iter__", "__next__")


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


def _make_autospec(spec, strict, as_instance, name=None, target=None):
    from dataclasses import fields, is_dataclass
    from functools import partial
    from inspect import iscoroutinefunction, isdatadescriptor, signature
    from types import FunctionType

    if _is_instance_mock(spec):
        raise InvalidSpecError("Cannot autospec a Mock object")
    if type(spec) in (list, tuple):
        spec = type(spec)
    if isdatadescriptor(spec):
        # A property viewed through its class has no known result type.
        return MagicMock(name=name)
    is_class = isinstance(spec, type)
    if is_class:
        if not as_instance:
            target = partial(spec.__init__, None)
        elif _instance_callable(spec):
            target = _autospec_target(spec, "__call__", spec.__call__)
    elif isinstance(spec, classmethod):
        target = partial(spec.__func__, None)
    elif isinstance(spec, staticmethod):
        target = spec.__func__
    elif target is None and _callable(spec):
        target = spec
    cls = MagicMock if target is not None else NonCallableMagicMock
    if target is not None and target is spec and not isinstance(spec, (FunctionType, MethodType)):
        # Inspect the call wrapper, as in CPython. Some native routines have
        # no visible wrapper and can still be mocked without checking arguments.
        target = getattr(spec, "__call__", None)
    if target is not None and iscoroutinefunction(target):
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
    if target is not None:
        try:
            sig = signature(target)
        except ValueError:
            # Some builtin callables do not expose a signature.
            pass
        else:
            mock._node._spec_signature = sig
            type(mock).__signature__ = sig
    if is_class and not as_instance:
        mock.return_value = _make_autospec(spec, strict, True)
    elif isinstance(spec, (FunctionType, MethodType)):
        # A spec method has a return value even if first read after seal().
        mock.return_value = MagicMock()
    return mock


def create_autospec(spec, spec_set=False, instance=False, *, unsafe=False, **kwargs):
    """Create a mock of a class or instance, checking calls against the source signatures."""
    from inspect import isroutine
    from types import BuiltinFunctionType, MethodDescriptorType

    if _is_instance_mock(spec):
        raise InvalidSpecError("Cannot autospec a Mock object")
    if isroutine(spec) and not isinstance(spec, (classmethod, staticmethod, BuiltinFunctionType, MethodDescriptorType)):
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


class PropertyMock(Mock):
    """Record property reads as calls without arguments and writes as calls with the value."""

    def _get_child_mock(self, /, **kwargs):
        return MagicMock(**kwargs)

    def __get__(self, obj, obj_type=None):
        return self()

    def __set__(self, obj, val):
        self(val)
