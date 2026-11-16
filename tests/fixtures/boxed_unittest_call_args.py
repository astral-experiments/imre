from unittest.mock import Mock, _Call


class Mismatch:
    def __eq__(self, other):
        return False


class Matches:
    def __eq__(self, other):
        return True


class BrokenComparison:
    def __eq__(self, other):
        raise RuntimeError("keyword compared")


class AppendMatch:
    def __init__(self, mock):
        self.mock = mock

    def __eq__(self, value):
        if value == "later":
            return True
        self.mock("later")
        return False


m = Mock()
m()
assert m.call_args.args == ()
assert m.call_args == ()
m(1)
assert m.call_args == ((1,),)
assert m.call_args != ((2,),)
m(flag=True)
assert ({"flag": True},) == m.call_args
assert ("child", {"flag": False}) != m.call_args
assert m.call_args != 7
assert m.call_args == Matches()
m(Mismatch(), flag=1)
assert m.call_args == ("child", (Matches(),), {"flag": 1})
m.assert_called_with(Matches(), flag=1)
m.assert_any_call(Matches(), flag=1)
try:
    m.assert_called_with(Matches(), flag=2)
except AssertionError:
    pass
else:
    raise AssertionError("mismatched keyword matched")

m = Mock()
m(0, flag=1)
try:
    m.assert_any_call(1, flag=BrokenComparison())
except RuntimeError as error:
    assert str(error) == "keyword compared"
else:
    raise AssertionError("keyword comparison was skipped")

m = Mock()
m("earlier")
try:
    m.assert_any_call(AppendMatch(m))
except AssertionError:
    pass
else:
    raise AssertionError("call made during comparison matched")
assert m.call_count == 2
m.assert_any_call("later")
m.call_args_list.append((("manual",), {"flag": 1}))
m.assert_any_call("manual", flag=1)


class RejectRecord(Mock):
    @Mock.call_args.setter
    def call_args(self, value):
        assert self.called and self.call_count == 0
        raise ValueError("record rejected")


reject = RejectRecord()
try:
    reject(1)
except ValueError as error:
    assert str(error) == "record rejected"
else:
    raise AssertionError("call_args setter was skipped")
assert reject.call_args is None and reject.call_args_list == [] and reject.mock_calls == []


class YieldingCall(_Call):
    def __iter__(self):
        yield from ("child", (1,), {})


assert YieldingCall(()) == ("child", (1,), {})


class Name(str):
    def __ne__(self, other):
        if self is expected_name:
            raise name_error
        return False


name_error = LookupError("expected name compared first")
expected_name = Name("child")
expected = _Call((expected_name, (), {}))
recorded = _Call((Name("child"), (), {}))
assert expected == recorded
try:
    recorded == expected
except LookupError as error:
    assert error is name_error
else:
    raise AssertionError("expected name comparison was skipped")
