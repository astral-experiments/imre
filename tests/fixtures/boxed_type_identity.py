import json
import types
import uuid
from datetime import date, datetime, timedelta, timezone
from operator import attrgetter


def classify():
    pass


class ReportsString:
    @property
    def __class__(self):
        return str


def current():
    class Reporter:
        def render(self):
            return self

    report = Reporter()
    saved = report.render
    again = report.render
    other = Reporter().render
    explicit = types.MethodType(Reporter.render, report)
    methods = {saved: 7}
    print(saved is again, saved == again, saved != other)
    print(methods[again], methods[explicit], methods.get(other))
    samples = (
        ("NoneType", None), ("EllipsisType", ...), ("NotImplementedType", NotImplemented),
        ("BuiltinFunctionType", len), ("FunctionType", Reporter.render),
        ("MethodType", report.render), ("CodeType", Reporter.render.__code__),
    )
    by_type = {}
    for name, sample in samples:
        alias = getattr(types, name)
        print(name, alias is type(sample), sample.__class__ is alias, type(alias) is type,
              isinstance(sample, alias))
        by_type[alias] = name
    print(len(by_type), [by_type[type(sample)] for name, sample in samples])
    class Number(int):
        pass

    class Child(Number):
        pass

    for value in (Number(), Number(7), Child("-17")):
        print(type(value).__name__, int(value), value + 1)

    for cls, instance in ((int, 43), (str, "report")):
        new = cls.__new__
        print(type(new) is type(len), isinstance(new, types.BuiltinFunctionType),
              new.__self__ is cls, instance.__new__ is new, new(cls, instance) == instance)
    print(int.__new__ is str.__new__,
          isinstance(dict.get, types.BuiltinFunctionType), isinstance(object.__init__, types.BuiltinFunctionType))
    for obj in (len, [].append, Reporter.render, report.render, attrgetter, attrgetter("render")):
        print(isinstance(obj, types.BuiltinFunctionType), isinstance(obj, types.FunctionType),
              isinstance(obj, types.MethodType))

    marker, other = object(), object()
    retained = {marker: 1, other: 2}
    print(type(marker) is object, marker.__class__ is object, isinstance(marker, object))
    print(marker is other, marker == other, retained[marker], retained[other])
    print(bool(marker), hasattr(marker, "__dict__"))
    try:
        marker.value = 1
    except AttributeError as error:
        print(str(error))
    for args, kwargs in (((None,), {}), ((), {"value": 1})):
        try:
            object(*args, **kwargs)
        except TypeError as error:
            print(str(error))
    identifier = uuid.UUID("12345678-1234-5678-9ABC-DEF001234567")
    print(type(identifier) is uuid.UUID, isinstance(1, uuid.UUID), str(identifier), identifier.hex)
    value = ReportsString()
    print(type("Málaga <&> 東京") is str, type(value) is str, value.__class__ is str)
    print(type(value) is ReportsString, type(ReportsString) is type, type(type) is type)
    print(type(True) is bool, type(42) is int, isinstance(True, int))
    first_float, second_float = json.loads("[1.5, 2.5]")
    print(type(first_float).__name__, type(first_float) is type(second_float), type(...).__name__)
    print(type(b"") is bytes, type(bytearray()) is bytearray, type(()) is tuple, type([]) is list, type({}) is dict)
    print(type(set()) is set, type(range(2)) is range, type(None).__name__)
    print(type(classify).__name__, type(value.__class__).__name__, type(len).__name__)
    print(isinstance(str, type), isinstance(classify, type(classify)), isinstance(value, type))
    print(
        type(date(2024, 1, 2)) is date,
        type(datetime(2024, 1, 2, 3, 4)) is datetime,
        type(timedelta(days=1)) is timedelta,
        type(timezone(timedelta())) is timezone,
    )
    for _ in range(1000):
        type([])
    print(type(None) is type(None), type(classify) is type(lambda: None))
    try:
        type(1, label=True)
    except TypeError as error:
        print(str(error))

    class Missing(KeyError):
        pass

    print(issubclass(Missing, Missing), issubclass(Missing, LookupError), issubclass(KeyError, Missing))
    print(issubclass(bool, int), issubclass(SystemExit, BaseException), issubclass(SystemExit, Exception))
    print(issubclass(EOFError, Exception), issubclass(IsADirectoryError, OSError))
    for child, parent in ((Missing, (str, (LookupError, 7))), (7, ()), (Missing, (str, 7)), (7, Missing)):
        try:
            print(issubclass(child, parent))
        except TypeError as error:
            print(str(error))

    class MissingAttribute(AttributeError):
        pass

    class DetailMissing(MissingAttribute):
        pass

    details = [7]
    error = DetailMissing(details)
    details.append(8)
    try:
        raise error
    except AttributeError as caught:
        print(isinstance(caught, DetailMissing), caught.args, caught.args[0] is details)
