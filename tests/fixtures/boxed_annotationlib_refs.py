import annotationlib
from importlib import import_module


def current():
    module = import_module("annotationlib")
    assert module.Format is annotationlib.Format
    assert module.ForwardRef is annotationlib.ForwardRef
    print(module.Format.VALUE, module.Format.FORWARDREF.value, module.Format.STRING.value)

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
