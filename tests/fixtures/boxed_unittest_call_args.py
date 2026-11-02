from unittest.mock import Mock


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
