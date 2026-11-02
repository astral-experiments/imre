from string.templatelib import Interpolation, Template


def capture(value):
    return t"prefix{value}suffix"


def current():
    first_template = capture(1)
    second_template = capture(2)
    assert first_template is not second_template
    assert first_template.strings is second_template.strings
    expected = ("prefix", "suffix")
    assert first_template.strings is expected
    assert first_template.interpolations is not second_template.interpolations
    assert first_template.values == (1,) and second_template.values == (2,)
    assert Template("prefix").strings is not Template("prefix").strings

    namespace = {}
    exec(r'def dormant(): return t"\ud800{1:\udfff}�"', namespace)
    sample = namespace["dormant"]()
    assert [ord(part) for part in sample.strings] == [0xD800, 0xFFFD]
    assert ord(sample.interpolations[0].format_spec) == 0xDFFF

    try:
        "x" + first_template
    except TypeError as error:
        assert str(error) == 'can only concatenate str (not "string.templatelib.Template") to str'
    else:
        raise AssertionError("concatenated str and Template")

    empty = t""
    assert type(empty) is Template
    assert empty.strings == ("",)
    assert empty.interpolations == empty.values == ()
    assert empty.values is empty.interpolations is empty.values
    assert t"literal {{text}}".strings == ("literal {text}",)

    text = "".join(["dynamic", " segment"])
    assert Template("", text).strings[0] is text
    assert Template(text, "").strings[0] is text

    item = []
    template = t"prefix{item}{item!r:>4}suffix"
    assert type(template) is Template
    assert template.strings == ("prefix", "", "suffix")
    first, second = template.interpolations
    assert type(first) is type(second) is Interpolation
    assert Template.__module__ == Interpolation.__module__ == "string.templatelib"
    assert (Template.__qualname__, Interpolation.__qualname__) == ("Template", "Interpolation")
    assert (first.expression, first.conversion, first.format_spec) == ("item", None, "")
    assert (second.expression, second.conversion, second.format_spec) == ("item", "r", ">4")
    assert first.value is second.value is item
    assert template.values[0] is template.values[1] is item
    item.append(7)
    print(first.value, second.value)
