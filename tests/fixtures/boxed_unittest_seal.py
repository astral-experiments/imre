from unittest.mock import Mock, seal


root = Mock(name="root", value=1)
root.factory().enabled = True
attached = Mock()
independent = Mock(name="independent")
root.attached = attached
root.independent = independent
root.alias = root
attached.return_value = 0
attached(3)
assert attached.call_count == 1
seal(root)
root.reset_mock()
assert attached.call_count == 0
assert root.factory().enabled
assert root.alias is root
root.value = 2
assert root.value == 2
independent.extra = 3
assert root.independent.extra == 3

for child, path in ((root.factory(), "root.factory().missing"), (attached, "root.attached.missing")):
    try:
        child.missing
    except AttributeError as error:
        assert str(error) == path
    else:
        raise AssertionError("sealed child created a new attribute")
    try:
        child.missing = None
    except AttributeError as error:
        assert str(error) == "Cannot set " + path
    else:
        raise AssertionError("sealed child accepted a new attribute")
strict = Mock()
seal(strict)
try:
    strict()
except AttributeError as error:
    assert str(error) == "mock.return_value"
else:
    raise AssertionError("sealed mock created a return value")
print("seal checked")
