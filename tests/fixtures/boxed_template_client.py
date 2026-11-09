import annotationlib
from string.templatelib import Interpolation, Template


def parameterize(query):
    sql = []
    parameters = []
    for part in query:
        if isinstance(part, Interpolation):
            sql.append("?")
            parameters.append(part.value)
        else:
            sql.append(part)
    return "".join(sql), parameters


def query_client():
    events = []

    def record(tag, value):
        events.append(tag)
        return value

    user = "O'Brien"
    query = (t"select * from users where name = { (record('user', user))  !s:>{record('width', 9)}}"
             t" and id = {record('id', 42)}")
    sql, parameters = parameterize(query)
    assert sql == "select * from users where name = ? and id = ?"
    assert parameters[0] is user and parameters[1] == 42
    assert events == ["user", "width", "id"]
    first = query.interpolations[0]
    spelling = " (record('user', user))"
    assert first.expression is spelling
    assert (first.conversion, first.format_spec) == ("s", ">9")
    print(sql, parameters, events)

    debug = t"""{ (user # comment
 ) = }{user=:>9}"""
    assert debug.strings == (" (user \n ) = ", "user=", "")
    first, second = debug.interpolations
    assert (first.expression, first.conversion, first.format_spec) == (" (user \n )", "r", "")
    spec = ">9"
    assert (second.expression, second.conversion) == ("user", None)
    assert second.format_spec is spec

    events.clear()
    try:
        t"{record('first', user):{int(record('width', 'invalid'))}}{record('late', user)}"
    except ValueError:
        assert events == ["first", "width"]
    else:
        raise AssertionError("invalid nested width")


def method_fields():
    events = []

    def note(tag, value):
        events.append(tag)
        return value

    class Recorder:
        def emit(self, value, *, tag, suffix):
            events.append(tag + suffix)
            return value

    class Width:
        def __format__(self, spec):
            events.append("format " + spec)
            return "8"

    value = []
    args = (value,)
    kwargs = {"suffix": "!"}
    recorder = Recorder()
    template = (t"{recorder.emit(*note('args', args), tag=note('tag', 'first'), **note('kwargs', kwargs))!r:>{Width():x}}"
                t"{note('last', 3)}")
    assert events == ["args", "tag", "kwargs", "first!", "format x", "last"]
    assert template.values[0] is value and template.values[1] == 3
    assert template.interpolations[0].format_spec == ">8"
    print(template.interpolations[0].format_spec, events)


def capture(value):
    return t"\ud800{ (value)!r:\udfff}�"


def current():
    method_fields()
    query_client()

    value = []
    first = capture(value)
    second = capture(None)
    assert type(first) is Template and first is not second
    assert first.strings is second.strings
    assert tuple(map(ord, first.strings)) == (0xD800, 0xFFFD)
    left, right = first.interpolations[0], second.interpolations[0]
    assert type(left) is Interpolation and left is not right
    assert left.expression is right.expression and left.expression == " (value)"
    assert left.format_spec is right.format_spec and ord(left.format_spec) == 0xDFFF
    assert first.values[0] is left.value is value
    print(annotationlib.type_repr(t"field { (value)!r:>8}"))
