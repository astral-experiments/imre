from unittest.mock import Mock


def later() -> int:
    print("later argument")
    return 2


huge = 10**5000
mock = Mock(side_effect=LookupError("after recording"))
try:
    mock(huge, later())
except LookupError:
    print("after recording", mock.call_count)
mock.assert_called_once_with(huge, 2)
