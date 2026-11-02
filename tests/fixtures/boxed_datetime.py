import json
from datetime import UTC, date, datetime as dt, time, timedelta, timezone, tzinfo


def test_module_exports():
    import _datetime
    import datetime

    assert (datetime.MINYEAR, datetime.MAXYEAR) == (1, 9999)
    assert datetime.__all__ == (
        "date", "datetime", "time", "timedelta", "timezone", "tzinfo", "MINYEAR", "MAXYEAR", "UTC",
    )
    assert datetime is not _datetime and not hasattr(_datetime, "__all__")
    for name in datetime.__all__:
        assert vars(datetime)[name] is vars(_datetime)[name]
    assert datetime.UTC is _datetime.timezone.utc
    assert issubclass(_datetime.timezone, datetime.tzinfo)
    assert isinstance(datetime.UTC, _datetime.tzinfo)

    class Zone(datetime.tzinfo):
        pass

    assert isinstance(Zone(), _datetime.tzinfo)
    assert timezone.__mro__ == (timezone, datetime.tzinfo, object)
    try:
        Zone().utcoffset(None)
    except NotImplementedError as exc:
        assert str(exc) == "a tzinfo subclass must implement utcoffset()"
    else:
        raise AssertionError("missing abstract timezone offset")

    class IntegerZone(datetime.tzinfo, int):
        pass

    assert IntegerZone(7) == 7
    try:
        datetime.tzinfo.__new__(IntegerZone)
    except TypeError as exc:
        assert "is not safe" in str(exc)
    else:
        raise AssertionError("accepted plain tzinfo storage for an integer subtype")


    class ListZone(list, datetime.tzinfo):
        pass

    zone = datetime.tzinfo.__new__(ListZone)
    zone.append(5)
    assert type(zone) is ListZone and zone == [5]

    class Mixin:
        def __new__(cls):
            return "mixin"

    class ZoneFirst(datetime.tzinfo, Mixin):
        pass

    class MixinFirst(Mixin, datetime.tzinfo):
        pass

    assert type(ZoneFirst()) is ZoneFirst
    assert MixinFirst() == "mixin"


def test_class_constants():
    for cls, smallest, largest, step in (
        (date, date(1, 1, 1), date(9999, 12, 31), timedelta(days=1)),
        (dt, dt(1, 1, 1), dt(9999, 12, 31, 23, 59, 59, 999999), timedelta(microseconds=1)),
        (time, time(), time(23, 59, 59, 999999), timedelta(microseconds=1)),
        (timedelta, timedelta(days=-999999999), timedelta(days=999999999, seconds=86399, microseconds=999999), timedelta(microseconds=1)),
    ):
        assert type(cls.min) is cls and type(cls.max) is cls
        assert (cls.min, cls.max, cls.resolution) == (smallest, largest, step)
        assert smallest.min is cls.min and largest.max is cls.max
        assert smallest.resolution is cls.resolution
    limit = timedelta(hours=23, minutes=59)
    for edge, offset in [(timezone.min, -limit), (timezone.max, limit)]:
        assert type(edge) is timezone and edge == timezone(offset)
        assert edge.min is timezone.min and edge.max is timezone.max and edge.utc is UTC
    assert UTC.utc is UTC is timezone.utc
    for constant in ("min", "max", "utc"):
        try:
            setattr(timezone, constant, None)
        except TypeError:
            pass
        else:
            raise AssertionError(constant)
        try:
            delattr(UTC, constant)
        except AttributeError:
            pass
        else:
            raise AssertionError(constant)
    assert dt.resolution is not time.resolution
    assert time.resolution is not timedelta.resolution

    class Child(time):
        min = "override"
    obj = Child()
    assert obj.min == Child.min == "override"
    assert obj.max is Child.max is time.max
    obj.resolution = "instance override"
    assert obj.resolution != Child.resolution and Child.resolution is time.resolution


def test_http_date():
    assert UTC is timezone.utc
    assert timezone(timedelta(0)) is UTC
    # JSON and the system clock both supply a boxed float to the HTTP date path.
    stamp = dt.fromtimestamp(json.loads("1700000000.5625"), timezone.utc)
    assert stamp.tzinfo is timezone.utc and stamp.microsecond == 562500
    assert isinstance(stamp, dt) and isinstance(stamp, date)
    fields = stamp.timetuple()
    assert fields == (2023, 11, 14, 22, 13, 20, 1, 318, -1)
    assert (fields.tm_wday, fields.tm_yday, fields.tm_isdst) == (1, 318, -1)
    assert fields.tm_zone is None and fields.tm_gmtoff is None
    assert repr(fields).startswith("time.struct_time(tm_year=2023,")
    leap = date(2000, 2, 29)
    assert leap.timetuple() == (2000, 2, 29, 0, 0, 0, 1, 60, -1)
    assert leap.weekday() == 1 and leap.toordinal() == 730179
    print(str(stamp), repr(leap))


def test_cookie_timestamp():
    lifetime = timedelta(days=31)
    assert int(lifetime.total_seconds()) == 2678400
    assert str(lifetime.total_seconds()) == "2678400.0"
    before = dt.fromtimestamp(-1, tz=UTC)
    assert before == dt(1969, 12, 31, 23, 59, 59, tzinfo=UTC)
    assert dt.fromtimestamp(json.loads("-0.125"), UTC).microsecond == 875000
    assert dt.fromtimestamp(json.loads("0.9999999"), UTC) == dt.fromtimestamp(1, UTC)
    assert dt.fromtimestamp(json.loads("-0.0000005"), UTC) == dt.fromtimestamp(0, UTC)
    east = timezone(timedelta(hours=2), "EET")
    shifted = dt.fromtimestamp(timestamp=-1, tz=east)
    assert shifted.tzinfo is east and shifted.hour == 1 and shifted.day == 1
    assert shifted == before
    small = timedelta(microseconds=-1)
    assert (small.days, small.seconds, small.microseconds) == (-1, 86399, 999999)
    assert str(small.total_seconds()) == "-1e-06"
    print(str(before), str(shifted), int(lifetime.total_seconds()))


def test_time_fromisoformat():
    parse = time.fromisoformat
    assert parse("T24:00:00.0000000Z").tzinfo is UTC
    assert parse("12-00:00:00.123").tzinfo is UTC
    assert parse("123456,1234567-01:02:03.4") == time(
        12, 34, 56, 123456, timezone(-timedelta(hours=1, minutes=2, seconds=3, microseconds=400000))
    )

    class Clock(time):
        pass

    assert type(Clock().fromisoformat("07:59")) is Clock
    assert Clock.fromisoformat("07:59") == time(7, 59)

    class Capture(time):
        def __new__(cls, *args):
            return args

    assert Capture.fromisoformat("25:01+00") == (25, 1, 0, 0, UTC)
    for text in ("12:30:00.", "12:00+24", "24:00:01", "12\ud80000"):
        try:
            parse(text)
        except ValueError:
            pass
        else:
            raise AssertionError(text)


def test_time_strptime():
    class Clock(time):
        pass

    value = Clock().strptime("4:05:06.123 -0130", "%H:%M:%S.%f %z")
    assert type(value) is Clock
    assert value.isoformat() == "04:05:06.123000-01:30"
    try:
        time.strptime("25", "%H")
    except ValueError:
        pass
    else:
        raise AssertionError("invalid hour")


def test_time_of_day():
    midnight = time()
    assert type(midnight) is time and isinstance(midnight, time)
    assert (time.__module__, time.__name__, time.__qualname__) == ("datetime", "time", "time")
    assert not isinstance(midnight, date)
    assert midnight.tzinfo is None and midnight.fold == 0
    day = date(2000, 2, 29)
    # Werkzeug http_date combines a plain date with UTC midnight.
    assert dt.combine(day, midnight, tzinfo=UTC) == dt(2000, 2, 29, tzinfo=UTC)
    east = timezone(timedelta(hours=2), "east")
    clock = time(12, 34, second=56, microsecond=7, tzinfo=east, fold=1)
    assert (clock.hour, clock.minute, clock.second, clock.microsecond, clock.fold) == (12, 34, 56, 7, 1)
    assert clock.tzinfo is east
    combined = dt.combine(dt(2000, 2, 29, 23), clock)
    assert combined.tzinfo is east and combined.fold == 1 and combined.hour == 12
    assert dt.combine(day, clock, tzinfo=None).tzinfo is None
    assert clock == time(10, 34, 56, 7, UTC)
    assert clock > time(10, 34, 56, 6, UTC)
    assert clock != time(12, 34, 56, 7)
    try:
        clock < midnight
    except TypeError as error:
        print(str(error))
    else:
        raise AssertionError("ordered aware and naive times")
    class Hour:
        def __index__(self):
            return 17
    assert time(Hour()).hour == 17
    extracted = combined.time()
    assert type(extracted) is time and extracted.tzinfo is None and extracted.fold == 1
    assert extracted == time(12, 34, 56, 7)
    replaced = clock.replace(Hour(), microsecond=8)
    assert replaced == time(17, 34, 56, 8, east) and replaced.fold == 1 and replaced.tzinfo is east
    cleared = time.__replace__(clock, tzinfo=None, fold=0)
    assert cleared == extracted and cleared.fold == 0 and clock.replace() is not clock

    class Moment(dt):
        @property
        def hour(self):
            raise AssertionError("used overridden hour")

    assert type(Moment(1, 1, 1).time()) is time
    for method, args, kwargs in (
        (time.__replace__, (), {}), (time.replace, (object(),), {}),
        (dt.time, (object(),), {}), (combined.time, (1,), {}), (combined.time, (), {"bad": 1}),
    ):
        try:
            method(*args, **kwargs)
        except TypeError as error:
            print(str(error))
        else:
            raise AssertionError("invalid clock method arguments accepted")

    calls = []
    class Clock(time):
        def __new__(cls, *args, **kwargs):
            calls.append((args, kwargs))
            return super().__new__(cls, *args, **kwargs)

    child = Clock(1, fold=1)
    assert type(child.replace(Hour())) is Clock
    assert calls[-1] == ((17, 0, 0, 0, None), {"fold": 1})
    assert child.replace(fold=0).fold == 0 and calls[-1] == ((1, 0, 0, 0, None), {})
    for args, kwargs, error_type in (
        ((24,), {}, ValueError), ((), {"fold": 2}, ValueError),
        ((1 << 100,), {}, OverflowError), (("bad",), {}, TypeError),
        ((0, 0, 0, 0, None, 1), {}, TypeError), ((1,), {"hour": 2}, TypeError),
        ((), {"tzinfo": True}, TypeError), ((), {"span": 1}, TypeError),
    ):
        try:
            clock.replace(*args, **kwargs)
        except error_type:
            pass
        else:
            raise AssertionError("invalid replacement accepted")
    for args, keywords in (
        ((24,), {}), ((0, 60), {}), ((0, 0, -1), {}), ((0, 0, 0, 1000000), {}),
        ((), {"fold": 2}), ((0, 0, 0, 0, True), {}), ((0, 0, 0, 0, None, 1), {}),
        (("bad",), {}), ((1 << 100,), {}), ((1,), {"hour": 2}), ((), {"span": 1}),
        ((24, "bad"), {}), ((24,), {"tzinfo": True}),
    ):
        try:
            time(*args, **keywords)
        except (TypeError, ValueError, OverflowError) as error:
            print(str(error))
        else:
            raise AssertionError("invalid time accepted")
    print(str(midnight), repr(clock), str(clock), str(combined))


def test_now():
    from time import time as timestamp

    now = dt.now
    east = timezone(timedelta(hours=5, minutes=30))
    for args, kwargs, zone in (((), {}, None), ((None,), {}, None),
                               ((UTC,), {}, UTC), ((), {"tz": east}, east)):
        before = int(timestamp())
        stamp = now(*args, **kwargs)
        after = int(timestamp())
        assert type(stamp) is dt and stamp.tzinfo is zone
        assert 0 <= stamp.microsecond < 1000000
        assert any(stamp.timetuple() == dt.fromtimestamp(sec, zone).timetuple()
                   for sec in range(before, after + 1))
    for args, kwargs in (((1,), {}), ((None, None), {}), ((), {"foo": 1}), ((None,), {"tz": None})):
        try:
            now(*args, **kwargs)
        except TypeError as exc:
            print(str(exc))
        else:
            raise AssertionError("invalid now() arguments accepted")


def test_subclass_construction():
    class Date(date):
        pass

    class Clock(time):
        pass

    class Delta(timedelta):
        pass

    class Moment(dt):
        def __new__(cls, year, label):
            return super().__new__(cls, year, 2, 1, minute=42, fold=1)

        def __init__(self, year, label):
            self.label = label

    class Grandchild(Moment):
        pass

    value = Grandchild(2000, label="leap")
    assert type(value) is Grandchild and isinstance(value, dt) and isinstance(value, date)
    assert (value.year, value.minute, value.fold, value.label) == (2000, 42, 1, "leap")
    assert value.toordinal() == date(2000, 2, 1).toordinal()
    assert type(dt.__new__(Moment, 2000, 1, 1)) is Moment
    assert repr(value) == "Grandchild(2000, 2, 1, 0, 42, fold=1)"
    assert date.__repr__(value) == "Grandchild(2000, 2, 1)"
    assert str(value) == "2000-02-01 00:42:00"

    class BadFormat(Grandchild):
        @property
        def isoformat(self):
            raise AttributeError("blocked format")

    try:
        str(BadFormat(2000, "leap"))
    except AttributeError as error:
        assert str(error) == "blocked format"
    else:
        raise AssertionError("calendar formatting ignored a descriptor error")

    assert repr(Clock(7, 8)) == "Clock(7, 8)"
    assert repr(Delta(0)) == "Delta(0)"
    assert not Delta() and Delta(seconds=1)

    class Custom(Delta):
        def __repr__(self):
            return "custom " + super().__repr__()
        def __bool__(self):
            return True

    custom = Custom()
    assert repr(custom) == "custom Custom(0)" and bool(custom)
    assert str(custom) == "0:00:00"

    class HasLength(Delta):
        def __len__(self):
            return 5

    assert not HasLength()

    assert Date(2024, 2, 29).day == 29
    assert Clock(microsecond=321).microsecond == 321
    assert Delta(days=1, seconds=2).total_seconds() == 86402

    class Shadow(Date):
        year = "overridden"
        @property
        def day(self):
            return self.month

    shadow = Shadow(2024, 2, 29)
    assert shadow.year == "overridden" and shadow.day == 2
    try:
        shadow.month = 7
    except AttributeError:
        pass
    else:
        raise AssertionError("calendar field was writable")

    class Other(Date):
        def __new__(cls):
            return "foreign result"
        def __init__(self):
            raise AssertionError("initializer called for a foreign result")


    class Mixin:
        __slots__ = ()

    class Mixed(Mixin, dt, date):
        pass

    assert Mixed(2001, 2, 3).year == 2001
    try:
        class Bad(date, time):
            pass
    except TypeError as error:
        assert str(error) == "multiple bases have instance lay-out conflict"
    else:
        raise AssertionError("incompatible native layouts accepted")

    assert Other() == "foreign result"
    try:
        date.__new__(Moment, 2000, 1, 1)
    except TypeError as error:
        assert "is not safe" in str(error)
    else:
        raise AssertionError("date allocated an incompatible datetime layout")


def test_timedelta_negation():
    class Delta(timedelta):
        @property
        def days(self):
            raise AssertionError("negation read a Python attribute")

    class Override(Delta):
        def __neg__(self):
            return "overridden"

    zero = timedelta()
    assert -zero is zero
    assert -timedelta(microseconds=1) == timedelta(-1, 86399, 999999)
    assert -timedelta(-1, 86399, 999999) == timedelta(microseconds=1)
    assert -timedelta.min == timedelta(days=999999999)
    assert -timedelta(days=999999999) == timedelta.min
    try:
        -timedelta.max
    except OverflowError as error:
        assert str(error) == "days=-1000000000; must have magnitude <= 999999999"
    else:
        raise AssertionError("negation exceeded the minimum duration")

    inherited = -Delta(microseconds=5)
    assert type(inherited) is timedelta and inherited == timedelta(microseconds=-5)
    custom = Override(days=1)
    assert -custom == "overridden"
    assert timedelta.__neg__(custom) == timedelta(days=-1)


def test_date_clock_constructors():
    import time as clock

    assert date.fromtimestamp(-0.0000001).timetuple()[:3] == clock.localtime(-1)[:3]

    class Day(date):
        @classmethod
        def fromtimestamp(cls, stamp):
            return cls, stamp

    old = clock.time
    token = object()
    try:
        clock.time = lambda: token
        assert Day.today() == (Day, token)
        assert Day(2001, 1, 1).today() == (Day, token)

        class Clock:
            def __call__(self):
                clock.time = old
                return token

            def __del__(self):
                Day.fromtimestamp = classmethod(lambda cls, stamp: stamp)

        clock.time = Clock()
        assert Day.today() is token
    finally:
        clock.time = old

    def import_clock(name, caller, locals, fromlist, level):
        assert (name, caller is locals, fromlist, level) == ("time", True, [], 0)
        return object()

    namespace = {"__builtins__": {"__import__": import_clock}, "Day": Day}
    try:
        clock.time = lambda: token
        assert eval("Day.today()", namespace) is token
        namespace["__builtins__"] = {}
        try:
            eval("Day.today()", namespace)
        except KeyError as exc:
            assert exc.args == ("__import__",)
        else:
            raise AssertionError("date.today ignored the caller's missing import hook")
    finally:
        clock.time = old


def test_timedelta_remainder():
    unit = timedelta(microseconds=3)
    for left, right, quotient, remainder in ((7, 3, 2, 1), (-7, 3, -3, 2),
                                             (7, -3, -3, -2), (-7, -3, 2, -1)):
        a = timedelta(microseconds=left)
        b = timedelta(microseconds=right)
        assert a % b == timedelta(microseconds=remainder)
        assert divmod(a, b) == (quotient, a % b)
        assert b.__rdivmod__(a) == divmod(a, b)
    assert divmod(timedelta.max, timedelta.resolution) == (86399999999999999999, timedelta(0))
    assert timezone(unit).utcoffset(None) % timedelta(minutes=1) == unit
    assert unit.__mod__(5) is NotImplemented
    try:
        divmod(unit, timedelta(0))
    except ZeroDivisionError:
        pass
    else:
        raise AssertionError("accepted zero timedelta divisor")


def test_timedelta_positive_and_absolute():
    zero = timedelta()
    assert +zero is zero and abs(zero) is zero
    tiny = timedelta(microseconds=-1)
    for value, magnitude in (
        (tiny, timedelta(microseconds=1)),
        (timedelta.min, timedelta(days=999999999)),
        (timedelta.max, timedelta.max),
    ):
        positive, absolute = +value, abs(value)
        assert positive == value and absolute == magnitude
        assert positive is not value and absolute is not value

    class Inherited(timedelta):
        def __neg__(self):
            raise AssertionError("abs called subclass __neg__")

    class Override(Inherited):
        def __pos__(self):
            return "positive"

        def __abs__(self):
            return "absolute"

    value = Inherited(microseconds=-1)
    assert type(+value) is type(abs(value)) is timedelta
    assert +value == tiny and abs(value) == timedelta(microseconds=1)
    custom = Override(days=-1)
    assert +custom == "positive" and abs(custom) == "absolute"
    assert timedelta.__pos__(custom) == timedelta(days=-1)
    assert timedelta.__abs__(custom) == timedelta(days=1)


def test_calendar_format():
    class Day(date):
        def __str__(self):
            return "custom day"

        def strftime(self, spec):
            return "called " + spec

    class Moment(dt):
        pass

    class Clock(time):
        def strftime(self, spec):
            return 37

    day = Day(2024, 2, 29)
    assert dt.__format__ is date.__format__
    assert type(date.__format__).__name__ == type(time.__format__).__name__ == "method_descriptor"
    assert type(day.__format__).__name__ == "builtin_function_or_method"
    assert format(day) == "custom day" and day.__format__("%Y") == "called %Y"
    moment = Moment(2024, 2, 29)
    moment.strftime = lambda spec: "instance " + spec
    assert format(moment, "%m") == "instance %m"
    assert format(date(2024, 2, 29)) == "2024-02-29"
    assert Clock().__format__("%H") == 37
    try:
        format(Clock(), "%H")
    except TypeError as exc:
        assert str(exc) == "__format__ must return a str, not int"
    else:
        raise AssertionError("format accepted a non-string result")
    for value in (day, dt(2024, 2, 29), time()):
        try:
            value.__format__(123)
        except TypeError as exc:
            assert str(exc) == "__format__() argument 1 must be str, not int"
        else:
            raise AssertionError("non-string format specifier accepted")


def test_calendar_reduction():
    class Moment(dt):
        pass

    class Clock(time):
        pass

    stamp = Moment(2000, 2, 29, 12, 34, 56, 0x012345, UTC, fold=1)
    stamp.note = "not part of immutable storage"
    for value, state, folded in (
        (date(2000, 2, 29), b"\x07\xd0\x02\x1d", b"\x07\xd0\x02\x1d"),
        (Clock(1, 2, 3, 4, fold=1), b"\x01\x02\x03\0\0\x04", b"\x81\x02\x03\0\0\x04"),
        (stamp, b"\x07\xd0\x02\x1d\x0c\x22\x38\x01\x23\x45", b"\x07\xd0\x82\x1d\x0c\x22\x38\x01\x23\x45"),
    ):
        for protocol in range(6):
            cls, args = value.__reduce_ex__(protocol)
            assert cls is type(value)
            assert args == ((folded if protocol >= 4 else state),) + ((UTC,) if value is stamp else ())
            restored = cls(*args)
            assert type(restored) is cls and restored == value
            assert not hasattr(restored, "note")
            if isinstance(value, (time, dt)):
                assert restored.fold == (protocol >= 4)
        assert value.__reduce__() == value.__reduce_ex__(2)
    assert date(b"\x07\xd0\x02\x1d".decode("latin1")) == date(2000, 2, 29)
    assert timedelta(microseconds=-1).__reduce_ex__(3) == (timedelta, (-1, 86399, 999999))

    class Custom(date):
        def __reduce__(self):
            return "class hook"

    custom = Custom(2000, 1, 1)
    assert custom.__reduce_ex__(3) == "class hook"
    custom.__reduce__ = lambda: "instance hook"
    assert custom.__reduce_ex__(3) == "instance hook"

    class Mixin:
        def __reduce_ex__(self, protocol):
            return protocol

    class Mixed(date, Mixin):
        pass

    item = Mixed(2000, 1, 1)
    assert super(Mixed, item).__reduce_ex__(4) == 4


def test_fixed_timezone_methods():
    class Offset(timedelta):
        pass

    class Name(str):
        def __repr__(self):
            return "Name()"

    class Stamp(dt):
        pass

    offset = Offset(hours=-2, microseconds=1)
    name = Name("")
    zone = timezone(offset, name)
    foreign = Stamp(2020, 1, 1, tzinfo=UTC)
    assert zone.utcoffset(foreign) is offset and zone.tzname(None) is name
    assert str(zone) is name
    assert repr(zone) == f"datetime.timezone({offset!r}, Name())"
    assert repr(time(tzinfo=zone, fold=1)) == f"datetime.time(0, 0, tzinfo={zone!r}, fold=1)"
    lone = timezone(timedelta(), "\ud800")
    assert lone is not UTC and lone.tzname(None) == str(lone) == "\ud800"
    assert repr(lone) == r"datetime.timezone(datetime.timedelta(0), '\ud800')"
    assert lone.__getinitargs__()[1] is lone.tzname(None)
    assert timezone.dst(zone, foreign) is None
    assert timezone(offset).tzname(foreign) == "UTC-01:59:59.999999"
    assert UTC.tzname(None) == "UTC"
    for descriptor in (timezone.utcoffset, timezone.tzname, timezone.dst):
        assert descriptor.__objclass__ is timezone
        assert descriptor.__text_signature__ == "($self, object, /)"
        try:
            descriptor(zone, date(2020, 1, 1))
        except TypeError as exc:
            assert str(exc) == (descriptor.__name__
                + "(dt) argument must be a datetime instance or None, not datetime.date")
        else:
            raise AssertionError("fixed timezone accepted a plain date")


def test_custom_tzinfo():
    offset = timedelta(seconds=29, microseconds=7)
    calls = []

    class Zone(tzinfo):
        def utcoffset(self, value):
            calls.append(value)
            return offset

    zone = Zone()
    day = date(2000, 1, 2)
    clock = time(0, 0, 29, 7, zone)
    local = dt.combine(day, clock)
    assert clock.tzinfo is local.tzinfo is zone
    assert dt(2000, 1, 2, tzinfo=zone).tzinfo is zone and not calls
    assert clock.utcoffset() is offset and local.utcoffset() is offset
    assert len(calls) == 2 and calls[0] is None and calls[1] is local
    assert clock.isoformat() == "00:00:29.000007+00:00:29.000007"
    assert str(local) == "2000-01-02 00:00:29.000007+00:00:29.000007"

    instant = dt(2000, 1, 2, tzinfo=UTC)
    later = dt(2000, 1, 2, microsecond=1, tzinfo=UTC)
    assert local == instant and instant == local and hash(local) == hash(instant)
    assert local < later and later > local
    try:
        dt.combine(day, clock, tzinfo=Zone)
    except TypeError:
        pass
    else:
        raise AssertionError("accepted a tzinfo class instead of an instance")


def test_explicit_zone_conversion():
    calls = []

    class Zone(tzinfo):
        def utcoffset(self, value):
            return timedelta(minutes=-30)

        def dst(self, value):
            calls.append((value.hour, value.minute))
            return timedelta(0)

    zone = Zone()
    result = dt.fromtimestamp(0, zone)
    assert result.tzinfo is zone and (result.day, result.hour, result.minute) == (31, 23, 30)
    assert calls == [(0, 0), (23, 30)]

    class Child(dt):
        pass

    class Hook(Zone):
        def fromutc(self, value):
            return type(value), value.tzinfo is self, value.hour

    assert Child.fromtimestamp(0, Hook()) == (Child, True, 0)
    child = Child(2000, 1, 2)
    assert child.fromtimestamp(0, Hook()) == (Child, True, 0)
    assert type(child.combine(child, time())) is Child
    shifted = Child(2000, 1, 1, 23, 45, tzinfo=zone).utctimetuple()
    assert tuple(shifted) == (2000, 1, 2, 0, 15, 0, 6, 2, 0)
    assert shifted.tm_zone is shifted.tm_gmtoff is None


def test_astimezone_subtype():
    constructed = []

    class Child(dt):
        def __new__(cls, *args, **kwargs):
            value = dt.__new__(cls, *args, **kwargs)
            constructed.append((value, value.tzinfo))
            return value

    class Target(tzinfo):
        def fromutc(self, value):
            return value

    source = timezone(timedelta(hours=1))
    target = Target()
    result = Child(2000, 1, 1, 6, tzinfo=source, fold=1).astimezone(target)
    intermediate, constructor_zone = constructed[1]
    assert constructor_zone is source and result is intermediate
    assert result.tzinfo is target and (result.hour, result.fold) == (5, 0)


def test_tzinfo_reduction():
    from datetime import tzinfo

    assert timezone.__reduce__ is tzinfo.__reduce__
    assert tzinfo().__reduce__() == (tzinfo, (), None)
    assert UTC.__reduce__() == (timezone, (timedelta(0),), None)
    offset = timedelta(minutes=7)
    name = "east"
    fixed = timezone(offset, name)
    cls, args, state = fixed.__reduce__()
    assert cls is timezone and args[0] is offset and args[1] is name and state is None

    class Zone(tzinfo):
        def __getinitargs__(self):
            return self.arguments

        def __getstate__(self):
            return self.state

    zone = Zone()
    zone.arguments = []
    zone.state = {}
    cls, args, state = zone.__reduce_ex__(4)
    assert cls is Zone and args is zone.arguments and state is zone.state

    class Stateful(tzinfo):
        pass

    obj = Stateful()
    obj.name = "east"
    assert obj.__reduce__() == (Stateful, (), {"name": "east"})
    assert obj.__reduce__()[2] is obj.__dict__


def test_calendar_strftime():
    calls = []

    class Name(str):
        def replace(self, old, new):
            calls.append((old, new))
            return super().replace(old, new)

    class Moment(dt):
        def timetuple(self):
            return (7, 1, 2, 3, 4, 5, 1, 2, -1)

        @property
        def microsecond(self):
            raise AssertionError("strftime read the Python attribute")

        def utcoffset(self):
            raise AssertionError("strftime used a receiver method")

        def tzname(self):
            raise AssertionError("strftime used a receiver method")

    zone = timezone(timedelta(seconds=-75, microseconds=-2), Name("west%Y"))
    stamp = Moment(2001, 2, 3, 8, 9, 10, 123, zone)
    assert stamp.strftime(format="%F %C %T %f") == "0007-01-02 00 03:04:05 000123"
    assert stamp.strftime("%%Z/%:z/%z") == "%Z/-00:01:15.000002/-000115.000002"
    assert calls == []
    assert stamp.strftime("%Z %Z %%f") == "west%Y west%Y %f"
    assert calls == [("%", "%%")]
    clock = time(8, 9, 10, 456, UTC)
    assert clock.strftime("%F %H:%M:%S.%f %z %:z %Z") == "1900-01-01 08:09:10.000456 +0000 +00:00 UTC"
    assert date(2000, 1, 1).strftime("\ud800%f\0%Z\udcff") == "\ud800000000\0\udcff"
    assert time().strftime("%z%:z%Z") == ""


def current():
    test_calendar_strftime()
    test_calendar_reduction()
    test_fixed_timezone_methods()
    test_custom_tzinfo()
    test_explicit_zone_conversion()
    test_astimezone_subtype()
    test_tzinfo_reduction()
    test_calendar_format()
    test_timedelta_negation()
    test_timedelta_positive_and_absolute()
    test_timedelta_remainder()
    test_date_clock_constructors()
    test_subclass_construction()
    test_now()
    test_time_fromisoformat()
    test_time_strptime()
    test_time_of_day()
    test_http_date()
    test_module_exports()
    test_class_constants()
    test_cookie_timestamp()
    for timestamp in (None, "0", 1 << 100):
        try:
            dt.fromtimestamp(timestamp, UTC)
        except (TypeError, OverflowError) as exc:
            print(str(exc))
        else:
            raise AssertionError("invalid timestamp accepted")
    try:
        dt.fromtimestamp(0, 1)
    except TypeError as exc:
        print(str(exc))
    else:
        raise AssertionError("invalid timezone accepted")
    test_calendar_ordering()
    test_calendar_day_methods()
    test_timestamp()


def test_calendar_ordering():
    leap = date(2000, 2, 29)
    assert date(2000, 2, 28) < leap <= date(2000, 2, 29)
    assert timedelta(microseconds=-1) < timedelta() < timedelta(days=1)
    naive = dt(2000, 2, 29, 23)
    assert naive < dt(2000, 3, 1) and naive >= dt(2000, 2, 29, 23, fold=1)
    offset = timezone(timedelta(hours=2, microseconds=1))
    local = dt(2000, 3, 1, 1, 0, 0, 1, offset, fold=1)
    instant = dt(2000, 2, 29, 23, tzinfo=UTC)
    assert instant <= local <= instant
    assert dt(2000, 2, 29, 23, 0, 0, 1, UTC) > local
    for left, right in ((naive, instant), (instant, naive), (leap, naive), (naive, leap),
                        (offset, UTC), (leap, timedelta())):
        try:
            left < right
        except TypeError as error:
            print(str(error))
        else:
            raise AssertionError("ordered incompatible calendar operands")


    class Declines(date):
        def __lt__(self, other):
            return NotImplemented

        def __gt__(self, other):
            return NotImplemented

    class Reflected(Declines):
        def __gt__(self, other):
            return "reflected"

    first = Declines(2000, 2, 28)
    try:
        first < Declines(2000, 2, 29)
    except TypeError:
        pass
    else:
        raise AssertionError("both calendar ordering hooks declined")
    assert first < leap
    assert (first < Reflected(2000, 2, 29)) == "reflected"


def test_calendar_day_methods():
    class Stamp(dt):
        def weekday(self):
            raise AssertionError("native isoweekday called weekday")

        def timetuple(self):
            raise AssertionError("native ctime called timetuple")

        def ctime(self):
            return "custom " + super().ctime()

    value = Stamp(30, 12, 31, 22, 23, 24, tzinfo=UTC)
    assert value.ctime() == "custom Tue Dec 31 22:23:24 0030"
    assert date.ctime(value) == "Tue Dec 31 00:00:00 0030"
    assert date(1, 1, 1).ctime() == "Mon Jan  1 00:00:00 0001"
    assert value.isoweekday() == 2 and date(2024, 3, 3).isoweekday() == 7


def test_timestamp():
    import os
    import time as clock

    class Stamp(dt):
        pass

    offset = timezone(timedelta(microseconds=1))
    assert Stamp(1970, 1, 1, tzinfo=offset).timestamp() == -0.000001
    assert dt.timestamp(dt(1970, 1, 1, tzinfo=UTC)) == 0.0
    if hasattr(clock, "tzset"):
        saved = os.environ.get("TZ")
        try:
            os.environ["TZ"] = "UTC0"
            clock.tzset()
            assert dt(1969, 12, 31, 23, 59, 59, 999999).timestamp() == -1 + 999999 / 1000000
            os.environ["TZ"] = "EST+05EDT,M3.2.0,M11.1.0"
            clock.tzset()
            for fold, gap, repeat in ((0, 1331451000, 1352007000), (1, 1331447400, 1352010600)):
                missing = dt(2012, 3, 11, 2, 30, fold=fold)
                repeated = dt(2012, 11, 4, 1, 30, fold=fold)
                assert missing.timestamp() == gap
                assert repeated.timestamp() == repeat
                local = repeated.astimezone()
                assert (local.tzname(), local.fold) == (("EDT", "EST")[fold], 0)
                assert repeated.astimezone(UTC) == dt.fromtimestamp(repeat, UTC)
                assert missing.astimezone(UTC) == dt.fromtimestamp(gap, UTC)
                assert missing.astimezone().hour == (3, 1)[fold]
        finally:
            if saved is None:
                del os.environ["TZ"]
            else:
                os.environ["TZ"] = saved
            clock.tzset()


if __name__ == "__main__":
    current()
