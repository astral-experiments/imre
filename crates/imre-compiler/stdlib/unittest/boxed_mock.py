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

    def _format(self, name):
        parts = [repr(value) for value in self.args]
        parts.extend(key + "=" + repr(value) for key, value in self.kwargs.items())
        return name + "(" + ", ".join(parts) + ")"

    def __repr__(self):
        return self._format("call")


_FixedMock = Mock
_unset_return = object()


class Mock(_FixedMock):
    def __init__(self, *, name=None, side_effect=None, return_value=_unset_return):
        super().__init__(name=name, side_effect=side_effect)
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
        self._node.side_effect = effect

    @property
    def return_value(self):
        child = super().return_value
        return getattr(child._node, "_configured_return", child)

    @return_value.setter
    def return_value(self, value):
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
            if isinstance(effect, BaseException) or isinstance(effect, type) and issubclass(effect, BaseException):
                raise effect
            return effect(*args, **kwargs)
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
        self._assert_any_call(args, kwargs)


class MagicMock(Mock):
    pass
