"""Native call recording for fixed-layout unittest.mock users."""

from dataclasses import dataclass, field

FILTER_DIR: bool = True


def _format_list(parts: list[str], pretty: bool) -> str:
    entries = ", ".join(parts)
    if pretty and len(entries) + 2 > 80:
        entries = ",\n ".join(parts)
    return "[" + entries + "]"


def _key(text: str, kind: str) -> str:
    if kind == "bool":
        return "int:1" if text == "True" else "int:0"
    return kind + ":" + text


def _change_mapping(target: dict[str, str], values: dict[str, str] | None, clear: bool) -> None:
    if clear:
        target.clear()
    if values is None:
        raise TypeError("'NoneType' object is not iterable")
    for key in values:
        target[key] = values[key]


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

    def _enter(self) -> dict[str, str]:
        self._original = self.in_dict.copy()
        _change_mapping(self.in_dict, self.values, self.clear)
        return self.in_dict

    def _change(self, values: dict[str, str] | None, clear: bool) -> None:
        _change_mapping(self.in_dict, values, clear)

    def _exit(self) -> bool:
        if self._original is not None:
            self._change(self._original, True)
        return False

    def start(self) -> dict[str, str]:
        result = self._enter()
        _active_patches.append(self)
        return result

    def stop(self) -> bool | None:
        for index in range(len(_active_patches)):
            if _active_patches[index] is self:
                # Keep start order for stopall, including after a manual stop.
                for tail in range(index + 1, len(_active_patches)):
                    _active_patches[tail - 1] = _active_patches[tail]
                _active_patches.pop()
                return self._exit()
        return None


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


@dataclass(repr=False)
class _Arg:
    text: str = field(compare=False)
    key: str

    def __init__(self, text: str, kind: str) -> None:
        self.text = text
        self.key = _key(text, kind)

    def __repr__(self) -> str:
        return self.text


def _format_call_signature(name: str, args: list[_Arg], kwargs: dict[str, _Arg]) -> str:
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
    calls: list[str]

    def __init__(self, prefix: str, calls: list[str]) -> None:
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
    _history: list[str]
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

    def _store_call(self, received: _Call) -> None:
        self.call_args = received
        self.call_args_list.append(received)
        self.call_count = len(self.call_args_list)

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

    def _record(self, args: list[_Arg], kwargs: dict[str, _Arg]) -> Mock:
        received = _new_call(args, kwargs)
        self._node.called = True
        self._node._store_call(received)
        self._node._history.append(_format_recorded_call(received, "call"))
        for ancestor in self._node._ancestors:
            ancestor.calls.append(_format_recorded_call(received, ancestor.prefix))
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
        return "\nCalls: " + _format_list(self._node._history, True) + "."

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

    def _assert_called_with(self, args: list[_Arg], kwargs: dict[str, _Arg]) -> None:
        expected = _new_call(args, kwargs)
        observed = self._node.call_args
        if observed is not None and observed == expected:
            return
        actual = "not called." if observed is None else _format_recorded_call(observed, self._name())
        raise AssertionError("expected call not found.\nExpected: " + _format_recorded_call(expected, self._name()) + "\n  Actual: " + actual)

    def _assert_any_call(self, args: list[_Arg], kwargs: dict[str, _Arg]) -> None:
        expected = _new_call(args, kwargs)
        for actual in self._node.call_args_list:
            if expected == actual:
                return
        raise AssertionError(_format_recorded_call(expected, self._name()) + " call not found")

    def _assert_called_once_with(self, args: list[_Arg], kwargs: dict[str, _Arg]) -> None:
        if self._node.call_count != 1:
            raise AssertionError(f"Expected '{self._name()}' to be called once. Called {self._node.call_count} times." + self._calls_repr())
        self._assert_called_with(args, kwargs)
