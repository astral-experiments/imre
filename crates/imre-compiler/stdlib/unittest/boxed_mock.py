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
    def __init__(self, *, name=None, side_effect=None, return_value=_unset_return):
        super().__init__(name=name)
        self.side_effect = side_effect
        if return_value is not _unset_return:
            self.return_value = return_value

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
        child = super().return_value
        return getattr(child._node, "_configured_return", child)

    @return_value.setter
    def return_value(self, value):
        if value is DEFAULT:
            self._node._result = -1
            return
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
