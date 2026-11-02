from string.templatelib import Interpolation, Template


def current():
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
