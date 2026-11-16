from unittest.mock import ThreadingMock, call

mock = ThreadingMock(name="report", timeout=0)
try:
    mock.wait_until_called()
except AssertionError as error:
    assert str(error) == "report was not called before timeout(0)."
else:
    raise AssertionError("an uncalled mock signaled")

mock([1], status="ready")
mock.wait_until_called(timeout=0)
mock.wait_until_any_call_with([1], status="ready")
assert mock.call_args_list == [call([1], status="ready")]

child = mock.child
child(7)
child.wait_until_any_call_with(7)
mock.reset_mock()
try:
    child.wait_until_any_call_with(7)
except AssertionError as error:
    assert str(error) == "child(7) call not found"
else:
    raise AssertionError("reset retained a child's signal")

failure = ValueError("failed")
mock.side_effect = failure
try:
    mock("again")
except ValueError as error:
    assert error is failure
else:
    raise AssertionError("side effect did not raise")
mock.assert_called_once_with("again")
try:
    mock.wait_until_any_call_with("again")
except AssertionError as error:
    assert str(error) == "report('again') call not found"
else:
    raise AssertionError("failed call signaled")

for name, label in ((0, "mock"), (42, "42")):
    named = ThreadingMock(name=name, timeout=0)
    try:
        named.wait_until_any_call_with("missing")
    except AssertionError as error:
        assert str(error) == f"{label}('missing') call not found"
    else:
        raise AssertionError("an unmatched call signaled")
