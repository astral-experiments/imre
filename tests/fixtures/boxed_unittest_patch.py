import sys
from inspect import iscoroutinefunction
from unittest import TestCase
from unittest.mock import DEFAULT, InvalidSpecError, Mock, call, patch, sentinel

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
with patch('sys.exit', side_effect=ValueError) as failing:
    try:
        sys.exit(9)
    except ValueError:
        assert failing.call_args.args == (9,)
    else:
        raise AssertionError("missing mock side effect")
assert sys.exit is original_exit

with patch.object(sys, "exit", return_value=42, side_effect=lambda: DEFAULT) as returning:
    assert sys.exit() == 42
    returning.assert_called_once_with()
assert sys.exit is original_exit

@patch('sys.exit', return_value=13)
class ConfiguredTests:
    def test_outer(self, outer):
        inner = self.test_inner()
        assert sys.exit is outer and inner is not outer
        outer.assert_not_called()
        assert sys.exit(1) == 13

    def test_inner(self, inner):
        assert sys.exit(2) == 13
        inner.assert_called_once_with(2)
        return inner


ConfiguredTests().test_outer()
assert sys.exit is original_exit

pending = patch('sys.exit', None, return_value=DEFAULT)
try:
    with pending:
        raise AssertionError("explicit replacement accepted mock options")
except TypeError as error:
    assert str(error) == "Can't pass kwargs to a mock we aren't creating"
assert sys.exit is original_exit

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

class PatchBase:
    def test_inherited(self):
        return sys.stdout


class PatchTests(PatchBase):
    test_builtin = getattr
    test_data = 123

    def test_nested(self):
        return self.test_inherited()

    def other(self):
        return sys.stdout


assert patch.object(sys, "stdout", original_stderr)(PatchTests) is PatchTests
assert PatchTests.test_builtin(sys, "stdout") is original_stderr
case = PatchTests()
assert case.test_nested() is original_stderr and sys.stdout is original_stdout
assert case.test_inherited() is original_stderr and case.other() is original_stdout
assert case.test_data == 123 and PatchBase().test_inherited() is original_stdout
assert patch.TEST_PREFIX == "test"
patch.TEST_PREFIX = "check"
try:
    @patch("sys.stdout", original_stderr)
    class CustomPrefix(PatchBase):
        def check_output(self):
            return sys.stdout

    assert CustomPrefix().check_output() is original_stderr
    assert CustomPrefix().test_inherited() is original_stdout
finally:
    patch.TEST_PREFIX = "test"

class Service:
    resource = Owner


check = TestCase()
class AlreadyMocked(Mock):
    @property
    def __func__(self):
        raise AssertionError("patch inspected an existing mock as a function")


with patch.object(Service, "resource", AlreadyMocked()):
    with check.assertRaises(InvalidSpecError):
        with patch.object(Service, "resource", spec=True):
            pass

with patch.object(Service, "resource", spec_set=True, new_callable=dict, label="service") as options:
    assert options == {"spec_set": Owner, "label": "service"}
with patch.object(Service, "resource", spec=Owner, spec_set=1, new_callable=dict) as options:
    assert options == {"spec": Owner, "spec_set": 1}
assert Service.resource is Owner


class CallableResource:
    def __call__(self, code):
        pass


labels = []


class LabeledMock(Mock):
    def __init__(self, *, label, **kwargs):
        labels.append(label)
        kwargs.setdefault("name", "named-instance")
        super().__init__(**kwargs)


Service.resource = CallableResource
with patch.object(Service, "resource", spec=True, new_callable=LabeledMock, label="resource") as factory:
    instance = factory()
    instance.return_value = "done"
    assert instance(7) == "done"
    assert factory.mock_calls == [call(), call()(7)]
assert labels == ["resource", "resource"]
assert Service.resource is CallableResource
Service.resource = Owner

named = Mock(name="independent")
parent = Mock(return_value=named)
assert parent() is named
named(3)
assert parent.mock_calls == [call()]

manual = patch.object(Service, "resource", spec_set=True)
with manual as factory:
    assert manual.stop() is None and Service.resource is factory
    with check.assertRaises(RuntimeError):
        manual.start()
assert Service.resource is Owner

factory = manual.start()
handle = factory()
assert not callable(handle)
handle.value(3)
assert factory.mock_calls == [call(), call().value(3)]
with check.assertRaises(AttributeError):
    handle.extra = 4
assert manual.stop() is False and Service.resource is Owner
assert manual.stop() is None

missing = patch.object(Service, "missing", create=True, spec=True)
with check.assertRaises(TypeError):
    missing.start()
assert missing.stop() is None and not hasattr(Service, "missing")

with patch.object(Service, "resource", None, spec=True) as explicit:
    assert explicit is None
with patch.object(Service, "resource", spec=False) as unrestricted:
    unrestricted.extra()
with patch.object(Service, "resource", spec=["read", "__call__"]) as reader:
    reader().read()
    assert reader.mock_calls == [call(), call().read()]

@patch.object(Service, "resource", spec_set=["read"])
class ReaderTests:
    def test_spec(self, reader):
        assert not callable(reader)
        reader.read()
        with check.assertRaises(AttributeError):
            reader.extra = True

ReaderTests().test_spec()
assert Service.resource is Owner


class Group:
    first = property(lambda self: 1)
    second = 2


first_descriptor = Group.__dict__['first']
with patch.multiple('__main__.Group', first=DEFAULT, second=3) as replacements:
    assert list(replacements) == ['first']
    assert replacements['first'] is Group.first and Group.second == 3
assert Group.first is first_descriptor and Group.second == 2

with check.assertRaises(AttributeError):
    patch.multiple(Group, first=0, second=0, missing=0).start()
assert Group.first is first_descriptor and Group.second == 2

@patch.multiple(Group, first=DEFAULT, second=DEFAULT)
class GroupTests:
    @patch('sys.exit')
    def test_call(self, exit_mock, first, second):
        assert sys.exit is exit_mock and Group.first is first and Group.second is second
        assert first is not second


GroupTests().test_call()
assert sys.exit is original_exit and Group.first is first_descriptor and Group.second == 2


def lookup(parcel, /, *, depot):
    return (parcel, depot)


with patch('__main__.lookup', autospec=True, return_value="queued") as replacement:
    with check.assertRaises(TypeError):
        main_module.lookup("A17")
    replacement.assert_not_called()
    assert main_module.lookup("A17", depot="west") == "queued"
    replacement.assert_called_once_with("A17", depot="west")
assert lookup("A17", depot="west") == ("A17", "west")

class Pause:
    def __await__(self):
        yield "paused"


@patch('__main__.lookup', autospec=True, return_value="queued")
async def dispatch(replacement):
    assert main_module.lookup is replacement
    await Pause()
    return main_module.lookup("A17", depot="west")


original_lookup = lookup
assert iscoroutinefunction(dispatch)
pending = dispatch()
assert main_module.lookup is original_lookup
assert pending.send(None) == "paused"
installed = main_module.lookup
assert installed is not original_lookup
with check.assertRaises(StopIteration) as completed:
    pending.send(None)
assert completed.exception.value == "queued"
installed.assert_called_once_with("A17", depot="west")
assert main_module.lookup is original_lookup

print("patch restored")
