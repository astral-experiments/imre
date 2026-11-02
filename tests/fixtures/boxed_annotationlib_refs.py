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
    assert evaluate_forward_ref(module.ForwardRef("raw"), globals={"raw": raw}) is raw
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
