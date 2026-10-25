"""Native call recording for fixed-layout unittest.mock users."""


def _key(text: str, kind: str) -> str:
    if kind == "bool":
        return "int:1" if text == "True" else "int:0"
    return kind + ":" + text


class _Arg:
    text: str
    key: str

    def __init__(self, text: str, kind: str) -> None:
        self.text = text
        self.key = _key(text, kind)


class _Call:
    _args: list[_Arg]
    _kwargs: dict[str, _Arg]

    def __init__(self, args: list[_Arg], kwargs: dict[str, _Arg]) -> None:
        self._args = args
        self._kwargs = kwargs

    def _format(self, name: str) -> str:
        parts: list[str] = []
        for arg in self._args:
            parts.append(arg.text)
        for key in self._kwargs:
            parts.append(key + "=" + self._kwargs[key].text)
        return name + "(" + ", ".join(parts) + ")"

    def _matches(self, other: _Call) -> bool:
        if len(self._args) != len(other._args) or len(self._kwargs) != len(other._kwargs):
            return False
        for i in range(len(self._args)):
            if self._args[i].key != other._args[i].key:
                return False
        for key in self._kwargs:
            if key not in other._kwargs or self._kwargs[key].key != other._kwargs[key].key:
                return False
        return True


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
        self.call_args_list = []
        self.side_effect = side_effect
        self._mock_name = name
        self._children = {}
        self._result = -1
        self._history = []
        self._ancestors = []


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
        if (name.startswith("assert") or name.startswith("assret") or name.startswith("asert")
                or name.startswith("aseert") or name.startswith("assrt")
                or name in {"called_with", "called_once", "called_once_with", "any_call", "has_calls", "not_called"}):
            raise AttributeError(f"{name!r} is not a valid assertion. Use a spec for the mock if {name!r} is meant to be an attribute.")
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

    @property
    def return_value(self) -> Mock:
        node = self._node
        if node._result == -1:
            result = _Node()
            self._attach(result, "()")
            node._result = len(self._nodes)
            self._nodes.append(result)
        return self._at(node._result)

    def _record(self, args: list[_Arg], kwargs: dict[str, _Arg]) -> Mock:
        received = _Call(args, kwargs)
        self._node.called = True
        self._node.call_count += 1
        self._node.call_args = received
        self._node.call_args_list.append(received)
        self._node._history.append(received._format("call"))
        for ancestor in self._node._ancestors:
            ancestor.calls.append(received._format(ancestor.prefix))
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
        entries = ", ".join(self._node._history)
        if len(entries) + 2 > 80:
            entries = ",\n ".join(self._node._history)
        return "\nCalls: [" + entries + "]."

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
        expected = _Call(args, kwargs)
        observed = self._node.call_args
        if observed is not None and expected._matches(observed):
            return
        actual = "not called." if observed is None else observed._format(self._name())
        raise AssertionError("expected call not found.\nExpected: " + expected._format(self._name()) + "\n  Actual: " + actual)

    def _assert_called_once_with(self, args: list[_Arg], kwargs: dict[str, _Arg]) -> None:
        if self._node.call_count != 1:
            raise AssertionError(f"Expected '{self._name()}' to be called once. Called {self._node.call_count} times." + self._calls_repr())
        self._assert_called_with(args, kwargs)
