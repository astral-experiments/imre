from unittest.mock import ANY, Mock

assert repr(ANY) == '<ANY>'
assert ANY == 42 and not (ANY != 42)

mock = Mock()
mock(1, keep="x", ignore=object())
mock.assert_called_with(ANY, keep="x", ignore=ANY)
mock.assert_any_call(ANY, keep="x", ignore=ANY)
try:
    mock.assert_called_with(ANY, keep="y", ignore=ANY)
except AssertionError:
    pass
else:
    raise AssertionError('ANY matched an adjacent argument')
try:
    mock.assert_called_with(1, ANY, keep="x", ignore=ANY)
except AssertionError:
    pass
else:
    raise AssertionError('ANY matched a missing positional argument')
print('mock ANY checks passed')
