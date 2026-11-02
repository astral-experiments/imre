from unittest.mock import Mock

m = Mock()
m(1, flag=True)
assert len(m.call_args) == 2
assert m.call_args.args == (1,)
assert m.call_args.kwargs == {"flag": True}
assert m.call_args_list == [((1,), {"flag": True})]
m.assert_called_with(1, flag=True)
try:
    m.assert_called_with(2, flag=True)
except AssertionError:
    pass
else:
    raise AssertionError("wrong positional argument matched")
