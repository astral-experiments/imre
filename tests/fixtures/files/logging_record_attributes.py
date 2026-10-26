"""Observe record attributes before and after exposing or mutating the live dictionary."""
import logging


def fallback(label: str) -> str:
    print(label)
    return label


def inspect(record: logging.LogRecord) -> None:
    for key in ["name", "args", "foo", "bar", "absent", "getMessage", "__dict__",
                "__weakref__", "__class__", "__dir__", "__static_attributes__", "__qualname__", "__missing__"]:
        print(hasattr(record, key))
    print(record.foo == 'one')
    print(repr(record.bar))
    print(repr(record.args))
    print(getattr(record, 'foo', fallback('evaluated for present')))
    print(getattr(record, 'absent', fallback('evaluated for absent')))
    print(repr(getattr(record, 'absent', None)))
    fields = vars(record)
    fields["__qualname__"] = "instance"
    print(hasattr(record, "__qualname__"))
    del fields["__qualname__"]
    print(hasattr(record, "__qualname__"))
    print(fields is record.__dict__)
    print(getattr(record, '__dict__', fallback('dictionary default')) is fields)
    held = record.foo
    fields['foo'] = 'two'
    print(record.foo == 'two')
    print(held == 'one')
    del fields['foo']
    print(hasattr(record, 'foo'))
    try:
        print(record.foo)
    except AttributeError as exc:
        print(str(exc))
    try:
        print(getattr(record, 'foo'))
    except AttributeError as exc:
        print(str(exc))
    fields['foo'] = [3, 4]
    print(len(record.foo))
    other = logging.LogRecord('name', 20, 'file.py', 6, record.foo, (), None)
    print(other.getMessage())
    other = logging.LogRecord('name', 20, 'file.py', 6, held, (), None)
    print(other.getMessage())
    print(repr(other.__dict__['msg']))
    try:
        print(getattr(record, 17, fallback('invalid name default')))
    except TypeError as exc:
        print(str(exc))
    try:
        print(hasattr(record, None))
    except TypeError as exc:
        print(str(exc))


inspect(logging.makeLogRecord({'foo': 'one', 'bar': None, 'args': ('arg',)}))
