"""Native call recording for fixed-layout unittest.mock users."""

from dataclasses import dataclass

FILTER_DIR: bool = True


def _format_list(parts: list[str], pretty: bool) -> str:
    entries = ", ".join(parts)
    if pretty and len(entries) + 2 > 80:
        entries = ",\n ".join(parts)
    return "[" + entries + "]"


def _clear_dict(in_dict: dict[str, str]) -> None:
    try:
        in_dict.clear()
    except AttributeError:
        for key in list(in_dict):
            del in_dict[key]


def _change_mapping(mapping: dict[str, str], values: dict[str, str], clear: bool) -> None:
    if clear:
        _clear_dict(mapping)
    try:
        mapping.update(values)
    except AttributeError:
        for key in values:
            mapping[key] = values[key]


class _DictionaryPatch:
    in_dict: dict[str, str]
    values: dict[str, str]
    clear: bool
    _original: dict[str, str] | None

    def __init__(self, mapping: dict[str, str], values: dict[str, str], clear: bool) -> None:
        self.in_dict = mapping
        self.values = values
        self.clear = clear
        self._original = None

    def __enter__(self) -> dict[str, str]:
        self._patch_dict()
        return self.in_dict

    def _patch_dict(self) -> None:
        _apply_dict_patch(self, self.values, self.in_dict, self.clear)

    def _unpatch_dict(self) -> None:
        mapping = self.in_dict
        original = self._original
        if original is None:
            _clear_dict(mapping)
            raise TypeError("'NoneType' object is not iterable")
        _change_mapping(mapping, original, True)

    def __exit__(self, *exc_info) -> bool:
        return self._exit()

    def _exit(self) -> bool:
        if self._original is not None:
            self._unpatch_dict()
        return False

    def start(self) -> dict[str, str]:
        result = self.__enter__()
        _active_patches.append(self)
        return result

    def stop(self) -> bool | None:
        for index in range(len(_active_patches)):
            if _active_patches[index] is self:
                # Keep start order for stopall, including after a manual stop.
                for tail in range(index + 1, len(_active_patches)):
                    _active_patches[tail - 1] = _active_patches[tail]
                _active_patches.pop()
                return self.__exit__(None, None, None)
        return None


def _apply_dict_patch(patcher: _DictionaryPatch, values: dict[str, str],
                      mapping: dict[str, str], clear: bool) -> None:
    try:
        original = mapping.copy()
    except AttributeError:
        original = {}
        for key in mapping:
            original[key] = mapping[key]
    patcher._original = original
    _change_mapping(mapping, values, clear)


_active_patches: list[_DictionaryPatch] = []


class _PatchFactory:
    TEST_PREFIX: str

    def __init__(self) -> None:
        self.TEST_PREFIX = "test"

    def stopall(self) -> None:
        for patcher in reversed(_active_patches):
            patcher.stop()

    def _dictionary(self, in_dict: dict[str, str], values: dict[str, str] | None, clear: bool, kwargs: dict[str, str]) -> _DictionaryPatch:
        if values is None:
            raise TypeError("'NoneType' object is not iterable")
        changes = values.copy()
        _change_mapping(changes, kwargs, False)
        return _DictionaryPatch(in_dict, changes, clear)


def _format_call_signature(name: str, args: tuple[object, ...], kwargs: dict[str, object]) -> str:
    positional = ", ".join([repr(arg) for arg in args])
    parts: list[str] = []
    for key, value in kwargs.items():
        parts.append(f"{key!s}={value!r}")
    keywords = ", ".join(parts)
    if positional and keywords:
        positional += ", "
    return name + "(" + positional + keywords + ")"


class _Ancestor:
    prefix: str
    calls: list[tuple[str, _Call]]

    def __init__(self, prefix: str, calls: list[tuple[str, _Call]]) -> None:
        self.prefix = prefix
        self.calls = calls


class _Node:
    called: bool
    call_count: int
    call_args: _Call | None
    call_args_list: list[_Call]
    side_effect: Exception | None
    _mock_name: str | None
    _children: dict[str, int]
    _result: int
    _history: list[tuple[str, _Call]]
    _ancestors: list[_Ancestor]

    def __init__(self, *, name: str | None = None, side_effect: Exception | None = None) -> None:
        self.called = False
        self.call_count = 0
        self.call_args = None
        self.call_args_list = _new_call_list()
        self.side_effect = side_effect
        self._mock_name = name
        self._children = {}
        self._result = -1
        self._history = []
        self._ancestors = []

    def _reset_state(self, reset_return: bool, reset_effect: bool) -> None:
        self.called = False
        self.call_count = 0
        self.call_args = None
        self.call_args_list = _new_call_list()
        # Children retain this private list to report subsequent ancestor calls.
        while len(self._history):
            self._history.pop()
        if reset_effect:
            self.side_effect = None
        if reset_return:
            self._result = -1

    def _reset(self, nodes: list[_Node], reset_return: bool, reset_effect: bool) -> None:
        pending: list[tuple[_Node, bool, bool]] = [(self, reset_return, reset_effect)]
        while len(pending):
            node, clear_return, clear_effect = pending.pop()
            node._reset_state(clear_return, clear_effect)
            for name in node._children:
                pending.append((nodes[node._children[name]], clear_return, clear_effect))
            if node._result != -1:
                # A retained return value resets its calls, but keeps its configuration.
                pending.append((nodes[node._result], False, False))


def _store_call(mock: Mock, args: tuple[object, ...], kwargs: dict[str, object]) -> _Call:
    mock.called = True
    received = _new_call(args, kwargs)
    mock.call_args = received
    mock.call_args_list.append(received)
    mock.call_count = len(mock.call_args_list)
    return received


def _check_child_name(name: str) -> None:
    if (name.startswith("assert") or name.startswith("assret") or name.startswith("asert")
            or name.startswith("aseert") or name.startswith("assrt")
            or name in {"called_with", "called_once", "called_once_with", "any_call", "has_calls", "not_called"}):
        raise AttributeError(f"{name!r} is not a valid assertion. Use a spec for the mock if {name!r} is meant to be an attribute.")


class Mock:
    _node: _Node
    _nodes: list[_Node]

    def __init__(self, *, name: str | None = None, side_effect: Exception | None = None) -> None:
        self._node = _Node(name=name, side_effect=side_effect)
        self._nodes = []

    def _at(self, index: int) -> Mock:
        result = Mock()
        result._node = self._nodes[index]
        result._nodes = self._nodes
        return result

    def _child(self, name: str) -> Mock:
        _check_child_name(name)
        return self._get_child(name)

    def _get_child(self, name: str) -> Mock:
        node = self._node
        if name not in node._children:
            index = len(self._nodes)
            child = _Node(name=name)
            self._attach(child, "." + name)
            self._nodes.append(child)
            node._children[name] = index
        return self._at(node._children[name])

    def _attach(self, child: _Node, suffix: str) -> None:
        child._ancestors.append(_Ancestor("call" + suffix, self._node._history))
        for ancestor in self._node._ancestors:
            child._ancestors.append(_Ancestor(ancestor.prefix + suffix, ancestor.calls))

    def _return_index(self) -> int:
        node = self._node
        if node._result == -1:
            result = _Node()
            self._attach(result, "()")
            node._result = len(self._nodes)
            self._nodes.append(result)
        return node._result

    @property
    def return_value(self) -> Mock:
        return self._at(self._return_index())

    def _record(self, args: tuple[object, ...], kwargs: dict[str, object]) -> Mock:
        received = _store_call(self, args, kwargs)
        self._node._history.append(("call", received))
        for ancestor in self._node._ancestors:
            ancestor.calls.append((ancestor.prefix, received))
        effect = self._node.side_effect
        if effect is not None:
            raise effect
        return self.return_value

    def _name(self) -> str:
        name = self._node._mock_name
        if name is None or name == "":
            return "mock"
        return name

    def _calls_repr(self) -> str:
        if len(self._node._history) == 0:
            return ""
        parts = [_format_recorded_call(value, name) for name, value in self._node._history]
        return "\nCalls: " + _format_list(parts, True) + "."

    def reset_mock(self, *, return_value: bool = False, side_effect: bool = False) -> None:
        self._node._reset(self._nodes, return_value, side_effect)

    def assert_not_called(self) -> None:
        if self._node.call_count != 0:
            raise AssertionError(f"Expected '{self._name()}' to not have been called. Called {self._node.call_count} times." + self._calls_repr())

    def assert_called(self) -> None:
        if self._node.call_count == 0:
            raise AssertionError(f"Expected '{self._name()}' to have been called.")

    def assert_called_once(self) -> None:
        if self._node.call_count != 1:
            raise AssertionError(f"Expected '{self._name()}' to have been called once. Called {self._node.call_count} times." + self._calls_repr())

    def assert_called_with(self, /, *args: object, **kwargs: object) -> None:
        expected = _new_call((*args,), kwargs)
        observed = self._node.call_args
        if observed is not None and observed == expected:
            return
        expected_text = _format_recorded_call(expected, self._name())
        actual = "not called." if observed is None else _format_recorded_call(observed, self._name())
        raise AssertionError("expected call not found.\nExpected: " + expected_text + "\n  Actual: " + actual)

    def assert_any_call(self, /, *args: object, **kwargs: object) -> None:
        expected = _new_call((*args,), kwargs)
        for actual in self._node.call_args_list:
            if expected == actual:
                return
        raise AssertionError(_format_recorded_call(expected, self._name()) + " call not found")

    def assert_called_once_with(self, /, *args: object, **kwargs: object):
        if not self.call_count == 1:
            msg = (f"Expected '{self._mock_name or 'mock'!s}' to be called once. "
                   f"Called {self.call_count!s} times.{self._calls_repr()!s}")
            raise AssertionError(msg)
        return self.assert_called_with(*args, **kwargs)
