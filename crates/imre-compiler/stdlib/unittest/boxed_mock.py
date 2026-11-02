from types import MethodType


class _ANY:
    def __eq__(self, other):
        return True

    def __ne__(self, other):
        return False

    def __repr__(self):
        return "<ANY>"


ANY = _ANY()


# Boxed calls keep their actual arguments. The fixed-layout frontend supplies
# preformatted scalar arguments to the same assertion and reset machinery.
class _Call(tuple):
    def __new__(cls, args, kwargs):
        return tuple.__new__(cls, (args, kwargs))

    @property
    def args(self):
        return self[0]

    @property
    def kwargs(self):
        return self[1]

    def __eq__(self, other):
        try:
            size = len(other)
        except TypeError:
            return NotImplemented

        # Recorded call_args has no name and accepts the abbreviated call tuples.
        args, kwargs = (), {}
        if size == 1:
            value, = other
            if isinstance(value, tuple):
                args = value
            elif not isinstance(value, str):
                kwargs = value
        elif size == 2:
            first, second = other
            if not isinstance(first, str):
                args, kwargs = first, second
            elif isinstance(second, tuple):
                args = second
            else:
                kwargs = second
        elif size == 3:
            _, args, kwargs = other
        elif size != 0:
            return False
        # Let the expected arguments' comparison hooks run first.
        return (args, kwargs) == (self.args, self.kwargs)

    def __ne__(self, other):
        equal = self.__eq__(other)
        if equal is NotImplemented:
            return NotImplemented
        return not equal

    def _format(self, name):
        parts = [repr(value) for value in self.args]
        parts.extend(key + "=" + repr(value) for key, value in self.kwargs.items())
        return name + "(" + ", ".join(parts) + ")"

    def __repr__(self):
        return self._format("call")


_FixedMock = Mock
_unset_return = object()


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

    def __init__(self, *, name=None, side_effect=None, return_value=_unset_return, **attrs):
        super().__init__(name=name)
        self.side_effect = side_effect
        if return_value is not _unset_return:
            self.return_value = return_value
        if attrs:
            for key in ("spec", "spec_set", "wraps", "parent", "unsafe"):
                if key in attrs:
                    raise TypeError(f"Mock.__init__() got an unexpected keyword argument '{key}'")
            self.configure_mock(**attrs)

    def configure_mock(self, /, **attrs):
        for path in sorted(attrs, key=lambda name: name.count(".")):
            target = self
            parts = path.split(".")
            for part in parts[:-1]:
                target = getattr(target, part)
            setattr(target, parts[-1], attrs[path])

    def _attach(self, child, suffix):
        super()._attach(child, suffix)
        child._owner = self
        child._suffix = suffix

    def _adopt(self, value, suffix):
        if not isinstance(value, Mock) or value._node._mock_name or hasattr(value._node, "_owner"):
            return False
        ancestor = self
        while ancestor is not None:
            if ancestor is value:
                return False
            ancestor = getattr(ancestor._node, "_owner", None)
        self._attach(value._node, suffix)
        if suffix != "()":
            name = suffix[1:]
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
        if name not in ("_node", "_nodes", "_mock_sealed", "return_value", "side_effect"):
            adopted = self._adopt(value, "." + name)
            if self._mock_sealed and not adopted and not hasattr(self, name):
                raise AttributeError("Cannot set " + self._mock_path() + "." + name)
        object.__setattr__(self, name, value)

    def _at(self, index):
        node = self._nodes[index]
        if not hasattr(node, "_proxy"):
            proxy = type(self)()
            proxy._node = node
            proxy._nodes = self._nodes
            node._proxy = proxy
        return node._proxy

    def __getattr__(self, name):
        if name in ("called", "call_count", "call_args", "call_args_list", "_mock_name"):
            return getattr(self._node, name)
        if name.startswith("_"):
            raise AttributeError(name)
        if self._mock_sealed and name not in self._node._children:
            _check_child_name(name)
            raise AttributeError(self._mock_path() + "." + name)
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
        call = _Call(args, kwargs)
        node = self._node
        node.called = True
        node.call_count += 1
        node.call_args = call
        node.call_args_list.append(call)
        node._history.append(("call", call))
        for ancestor in node._ancestors:
            ancestor.calls.append((ancestor.prefix, call))
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
            node._reset_state(clear_return, clear_effect)
            if node._result != -1:
                result = current.return_value
                if isinstance(result, Mock):
                    pending.append((result, False, False))
            children = {name: current._at(index) for name, index in node._children.items()}
            children.update(getattr(node, "_assigned_children", {}))
            for child in children.values():
                pending.append((child, clear_return, clear_effect))

    def _calls_repr(self):
        if not self._node._history:
            return ""
        calls = [call._format(name) for name, call in self._node._history]
        return "\nCalls: " + _format_list(calls, True) + "."

    def assert_called_with(self, /, *args, **kwargs):
        self._assert_called_with(args, kwargs)

    def assert_called_once_with(self, /, *args, **kwargs):
        self._assert_called_once_with(args, kwargs)

    def assert_any_call(self, /, *args, **kwargs):
        expected = _Call(args, kwargs)
        # Equality hooks may record more calls. Check only the history at entry.
        for actual in list(self._node.call_args_list):
            assert len(expected) == len(actual)
            if all([left == right for left, right in zip(expected, actual)]):
                return
        raise AssertionError(expected._format(self._name()) + " call not found")


class MagicMock(Mock):
    pass


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
            if isinstance(value, Mock) and getattr(value._node, "_owner", None) is current:
                pending.append(value)
