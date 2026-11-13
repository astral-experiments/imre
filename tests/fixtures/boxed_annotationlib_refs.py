import annotationlib
import typing
from annotationlib import get_annotate_from_class_namespace as get_annotate
from importlib import import_module
from typing import evaluate_forward_ref


def current():
    for newline in ("\n", "\r", "\r\n"):
        source = f"({newline}[(i := 3) for i in []]{newline})"
        try:
            typing._make_forward_ref(source)
        except SyntaxError as error:
            cause = error.__context__
            assert cause.args == ("assignment expression cannot rebind comprehension iteration variable 'i'",)
            assert (cause.filename, cause.lineno, cause.offset, cause.end_lineno, cause.end_offset) == (
                "<string>", 2, 3, 2, 4
            )
        else:
            raise AssertionError("expected invalid comprehension assignment")
    module = import_module("annotationlib")
    assert get_annotate is module.get_annotate_from_class_namespace
    assert get_annotate({}) is None
    assert get_annotate({"__annotate_func__": current}) is current
    assert get_annotate({"__annotate__": current}) is current
    assert get_annotate({"__annotate__": None, "__annotate_func__": current}) is None

    assert module.Format is annotationlib.Format
    assert module.ForwardRef is annotationlib.ForwardRef
    print(module.Format.VALUE, module.Format.FORWARDREF.value, module.Format.STRING.value)

    def field(format, exc=NotImplementedError):
        if format > 2:
            raise exc
        return {"x": UnboundFieldType}

    def limit(format, exc=NotImplementedError):
        if format > 2:
            raise exc
        return UnboundFieldType

    assert module.call_annotate_function(field, module.Format.STRING) == {"x": "UnboundFieldType"}
    ref = module.call_evaluate_function(limit, module.Format.FORWARDREF, owner=field)
    assert type(ref) is module.ForwardRef and ref.__owner__ is field
    assert module.call_evaluate_function(limit, 4) == "UnboundFieldType"

    class Opaque:
        def __repr__(self):
            raise AssertionError("rendered an opaque FORWARDREF operand")
    token = Opaque()
    def unknown(format, item=token, exc=NotImplementedError):
        if format > 2:
            raise exc
        range[1]
        return UnboundFieldType[item]
    ref = module.call_evaluate_function(unknown, module.Format.FORWARDREF)
    assert ref.__forward_arg__ == "UnboundFieldType[__annotationlib_name_1__]"
    assert ref.__extra_names__["__annotationlib_name_1__"] is token

    assert typing.evaluate_forward_ref is evaluate_forward_ref
    import warnings
    bound = module.ForwardRef('ForwardRef', module='annotationlib')
    scope = {'ForwardRef': int}
    assert typing._eval_type(bound, scope, {}, ()) is int
    assert typing._eval_type(list[bound], scope, {}, (), prefer_fwd_module=True) == list[module.ForwardRef]
    assert typing._eval_type(bound, scope, {'ForwardRef': bytes}, (), prefer_fwd_module=True) is bytes

    ref = module.ForwardRef("int")
    with warnings.catch_warnings(record=True) as captured:
        warnings.simplefilter("always")
        assert typing._eval_type(list["int"], {}, {}) == list[int]
        assert ref._evaluate({}, {}, recursive_guard=frozenset()) is int
        assert ref._evaluate({}, {}, (), recursive_guard=frozenset({"int"})) is ref
    assert len(captured) == 4
    for warning in captured:
        assert warning.category is DeprecationWarning and warning.filename == __file__
    assert "'typing._eval_type' is deprecated" in str(captured[0].message)
    assert "'typing.ForwardRef._evaluate' is deprecated" in str(captured[1].message)
    assert "ForwardRef._evaluate is a private API" in str(captured[2].message)
    assert str(captured[3].message) == str(captured[2].message)

    class RawMeta(type):
        def __eq__(cls, other):
            raise AssertionError("compared the type of a raw result")

    class Raw(metaclass=RawMeta):
        pass

    raw = Raw()
    namespace = set(vars(typing))
    assert evaluate_forward_ref(module.ForwardRef("raw"), globals={"raw": raw}) is raw
    assert set(vars(typing)) == namespace
    ns = {"Alias": bytes, "Annotated": typing.Annotated, "Literal": typing.Literal}
    assert evaluate_forward_ref(module.ForwardRef("list['Alias']"), globals=ns) == list[bytes]
    assert evaluate_forward_ref(module.ForwardRef("Literal['Alias']"), globals=ns) == typing.Literal["Alias"]
    value = evaluate_forward_ref(module.ForwardRef("Annotated['Alias', 'Alias']"), globals=ns)
    assert value.__origin__ is bytes and value.__metadata__ == ("Alias",)
    missing = module.ForwardRef("missing")
    assert evaluate_forward_ref(missing, format=module.Format.FORWARDREF) is missing
    try:
        evaluate_forward_ref(missing)
    except NameError as exc:
        assert exc.name == "missing"
    else:
        raise AssertionError("unresolved forward reference")

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

    def quoted(value: "mark(x)", rest: "*items"):
        pass

    seen = []
    scope = {"mark": seen.append, "x": 14, "items": (int,)}
    assert module.get_annotations(quoted, globals=scope, locals=scope, eval_str=True) == {
        "value": None, "rest": int,
    }
    assert seen == [14]

    def sums(reflected: 1 + int, nested: Left + (Right + 1)):
        pass

    texts = {"reflected": "1 + int", "nested": "Left + (Right + 1)"}
    assert module.get_annotations(sums, format=module.Format.STRING) == texts
    refs = module.get_annotations(sums, format=module.Format.FORWARDREF)
    for name, ref in refs.items():
        assert type(ref) is module.ForwardRef and ref.__owner__ is sums
        assert ref.__forward_arg__ == texts[name]
