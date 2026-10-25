"""Compare call history, reset propagation, and retained aliases with pinned CPython."""

from unittest.mock import Mock


def calls() -> None:
    probe = Mock(name="which")
    child = probe.which
    child("dontcare")
    child.assert_called_once_with("dontcare")
    child.reset_mock()
    child.assert_not_called()
    probe.which("other", version=False)
    probe.which("last")
    child.assert_any_call("other", version=0)
    child.assert_any_call("last")
    try:
        child.assert_any_call("dontcare")
    except AssertionError as e:
        print(str(e))
    try:
        child.assert_called_with("other", version=False)
    except AssertionError as e:
        print(str(e))
    print(repr(child.call_args))
    saved = child.call_args_list
    print(repr(saved), repr(probe.call_args))
    result = child.return_value
    result.run(9)
    probe.reset_mock()
    print(len(saved), len(child.call_args_list), child.call_args is None)
    print(child is probe.which, result is child.return_value)
    result.run.assert_not_called()
    child("new")
    child.assert_called_once_with("new")
    probe("root")
    try:
        probe.assert_not_called()
    except AssertionError as e:
        print(str(e))


def configs() -> None:
    root = Mock()
    child = root.child
    child.side_effect = ValueError("child")
    result = root.return_value
    result.side_effect = TypeError("result")
    try:
        child()
    except ValueError as e:
        print(str(e))
    try:
        root()()
    except TypeError as e:
        print(str(e))
    root.reset_mock(side_effect=True)
    child.assert_not_called()
    result.assert_not_called()
    child(42)
    child.assert_called_once_with(42)
    try:
        result()
    except TypeError as e:
        print(str(e))
    root.reset_mock(return_value=True)
    print(result is root.return_value, child is root.child, result.call_count)
    root.return_value.assert_not_called()
    root.return_value(5)
    root.return_value.assert_called_once_with(5)


calls()
configs()
