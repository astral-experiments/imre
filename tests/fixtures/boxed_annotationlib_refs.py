import annotationlib
from importlib import import_module


def current():
    module = import_module("annotationlib")
    assert module.Format is annotationlib.Format
    assert module.ForwardRef is annotationlib.ForwardRef
    print(module.Format.VALUE, module.Format.FORWARDREF.value, module.Format.STRING.value)

    class Text(str):
        pass

    class Display:
        def __repr__(self):
            return "display"

    text = Text("T")
    annotations = {"text": text, "type": int, "object": Display(), "dots": ...}
    rendered = module.annotations_to_string(annotations=annotations)
    assert list(rendered.items()) == [
        ("text", "T"), ("type", "int"), ("object", "display"), ("dots", "...")
    ]
    assert rendered["text"] is text and annotations["type"] is int
    assert module.type_repr(module.type_repr) == "annotationlib.type_repr"

    from string.templatelib import Interpolation, Template

    class Token(str):
        def __radd__(self, other):
            raise AssertionError("template formatting concatenated user values")

    rendered_values = []

    class Value:
        @property
        def __class__(self):
            assert not rendered_values
            return Value

        def __repr__(self):
            rendered_values.append(self)
            return Token("shown")

    class Literal(str):
        def replace(self, old, new):
            return self

        def __iter__(self):
            raise AssertionError("iterated a buffered string subclass")

    literal = Template("a{", Interpolation(1, "{x: y}", "r", "{spec}"))
    assert module.type_repr(literal) == "t'a{{{ {x: y}!r:{{spec}}}'"
    assert module.type_repr(Template(Interpolation(0, Token("await v"), Token("s")))) == "t'{await v!s}'"
    assert module.type_repr(Template(Interpolation((Value(),), ""))) == (
        "Template(Interpolation((shown,), '', None, ''))"
    )
    assert module.type_repr(Template(Literal("text"))) == "t'text'"
    fallback = Template(Interpolation(..., "ok"), Interpolation(1, "x +"))
    assert module.annotations_to_string({"a": fallback}) == {
        "a": "Template(Interpolation(..., 'ok', None, ''), Interpolation(1, 'x +', None, ''))"
    }

    ref = module.ForwardRef("Thing", module="models", is_class=True)
    same = module.ForwardRef("Thing", module="models", is_class=True, is_argument=False)
    print(repr(ref), ref.__forward_arg__, ref.__forward_module__, ref.__forward_is_class__)
    print(ref == same, hash(ref) == hash(same), ref == "Thing")
    try:
        module.ForwardRef(None)
    except TypeError as error:
        print(str(error))
    else:
        raise AssertionError("ForwardRef accepted a non-string annotation")

    try:
        class Derived(annotationlib.ForwardRef):
            pass
    except TypeError as error:
        print(str(error))
    else:
        raise AssertionError("ForwardRef accepted a subclass")

    from typing import Annotated, get_args

    def annotated(value: Annotated[Later, "marker"]):
        pass

    annotations = module.get_annotations(annotated, format=module.Format.FORWARDREF)
    ref, metadata = get_args(annotations["value"])
    assert type(ref) is module.ForwardRef and ref.__owner__ is annotated
    assert ref.__forward_arg__ == "Later" and metadata == "marker"
    Later = str
    assert ref.evaluate() is Later
    resolved = module.get_annotations(annotated)
    assert resolved == {"value": Annotated[Later, "marker"]}
    resolved.clear()
    assert module.get_annotations(annotated) == annotated.__annotations__ != {}

    class Meta(type):
        @property
        def __annotations__(cls):
            return {"masked": str}

    class WithOwnAnnotations(metaclass=Meta):
        field: int

    assert module.get_annotations(WithOwnAnnotations) == {"field": int}

    def sums(reflected: 1 + int, nested: Left + (Right + 1)):
        pass

    texts = {"reflected": "1 + int", "nested": "Left + (Right + 1)"}
    assert module.get_annotations(sums, format=module.Format.STRING) == texts
    refs = module.get_annotations(sums, format=module.Format.FORWARDREF)
    for name, ref in refs.items():
        assert type(ref) is module.ForwardRef and ref.__owner__ is sums
        assert ref.__forward_arg__ == texts[name]
