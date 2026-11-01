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
