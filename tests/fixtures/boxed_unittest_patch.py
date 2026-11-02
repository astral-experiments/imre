import sys
from unittest.mock import DEFAULT, Mock, patch, sentinel

assert DEFAULT is sentinel.DEFAULT
assert sentinel.value is sentinel.value and sentinel.value is not DEFAULT
assert str(sentinel.value) == 'sentinel.value'

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

with patch('__main__.with_default.__defaults__', (9,), create=True):
    assert with_default() == 9
assert with_default.__defaults__ is None

class Mapping:
    def __init__(self):
        self.items = {"original": 1}

    def __getitem__(self, key):
        return self.items[key]

    def __setitem__(self, key, value):
        self.items[key] = value

    def __delitem__(self, key):
        del self.items[key]

    def __iter__(self):
        return iter(self.items)


mapping = Mapping()
try:
    with patch.dict(mapping, {"new": 2}, clear=True) as patched:
        assert patched is mapping
        assert mapping.items == {"new": 2}
        raise ValueError("mapping body")
except ValueError as error:
    assert str(error) == "mapping body"
assert mapping.items == {"original": 1}

main_module = sys.modules[__name__]
with patch('__main__.open', with_default):
    assert main_module.open(7) == 7
assert not hasattr(main_module, 'open')

assert DEFAULT.name == "DEFAULT" and sentinel.alpha.name == "alpha"
configured = Mock(return_value=42, side_effect=lambda: DEFAULT)
assert configured() == 42
configured.return_value = DEFAULT
child = configured()
assert child is configured() and isinstance(child, Mock)
configured.return_value = DEFAULT
assert configured() is not child
configured.side_effect = lambda: None
assert configured() is None

patch_object = patch.object
with patch_object(owner, "value", None):
    assert owner.value is None
assert "value" not in owner.__dict__
with patch.object(owner, "additional", create=True) as created:
    assert owner.additional is created
assert not hasattr(owner, "additional")

print("patch restored")
