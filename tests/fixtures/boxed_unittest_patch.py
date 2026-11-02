import sys
from unittest.mock import patch

original_exit = sys.exit
original_stderr = sys.stderr
original_stdout = sys.stdout


class Owner:
    value = original_exit


owner = Owner()
first = owner
pending = patch('__main__.owner.value', None)
owner = Owner()
with pending:
    assert owner.value is None and first.value is original_exit
assert 'value' not in owner.__dict__


@patch('sys.exit')
def decorated(status, replacement):
    assert replacement is sys.exit
    sys.exit(status)
    replacement.assert_called_once_with(status)
    return status


with patch('sys.stdout', original_stderr):
    assert sys.stdout is original_stderr
    try:
        with patch('sys.stderr', None):
            assert sys.stderr is None
            raise ValueError("from body")
    except ValueError as exc:
        assert str(exc) == "from body"
    else:
        raise AssertionError("patch suppressed body exception")
    assert sys.stderr is original_stderr
assert sys.stdout is original_stdout
assert decorated(23) == 23
assert decorated(42) == 42
assert sys.exit is original_exit


def with_default(value=7):
    return value


with patch('__main__.with_default.__defaults__', (9,)):
    assert with_default() == 9
assert with_default() == 7

main_module = sys.modules[__name__]
with patch('__main__.open', with_default):
    assert main_module.open() == 7
assert not hasattr(main_module, 'open')

print("patch restored")
