import annotationlib
import typing
from importlib import import_module
from typing import evaluate_forward_ref


def current():
    module = import_module("annotationlib")
    assert module.Format is annotationlib.Format
    assert module.ForwardRef is annotationlib.ForwardRef
    print(module.Format.VALUE, module.Format.FORWARDREF.value, module.Format.STRING.value)

    assert typing.evaluate_forward_ref is evaluate_forward_ref
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
