"""Annotation formats and forward reference values from CPython 3.14.5 (PSF-2.0)."""

import enum

__all__ = ["Format", "ForwardRef"]


class Format(enum.IntEnum):
    VALUE = 1
    VALUE_WITH_FAKE_GLOBALS = 2
    FORWARDREF = 3
    STRING = 4


class ForwardRef:
    """An unevaluated annotation with its defining scope."""

    __slots__ = (
        "__arg__",
        "__forward_module__",
        "__owner__",
        "__forward_is_argument__",
        "__forward_is_class__",
        "__weakref__",
    )

    def __init__(self, arg, *, module=None, owner=None, is_argument=True, is_class=False):
        if not isinstance(arg, str):
            raise TypeError(f"Forward reference must be a string -- got {arg!r}")
        self.__arg__ = arg
        self.__forward_module__ = module
        self.__owner__ = owner
        self.__forward_is_argument__ = is_argument
        self.__forward_is_class__ = is_class

    def __init_subclass__(cls, /, *args, **kwargs):
        raise TypeError("Cannot subclass ForwardRef")

    @property
    def __forward_arg__(self):
        return self.__arg__

    def __eq__(self, other):
        if not isinstance(other, ForwardRef):
            return NotImplemented
        return (
            self.__forward_arg__ == other.__forward_arg__
            and self.__forward_module__ == other.__forward_module__
            and self.__forward_is_class__ == other.__forward_is_class__
            and self.__owner__ == other.__owner__
        )

    def __hash__(self):
        return hash((
            self.__forward_arg__, self.__forward_module__,
            self.__forward_is_class__, self.__owner__,
        ))

    def __repr__(self):
        extra = []
        if self.__forward_module__ is not None:
            extra.append(f", module={self.__forward_module__!r}")
        if self.__forward_is_class__:
            extra.append(", is_class=True")
        if self.__owner__ is not None:
            extra.append(f", owner={self.__owner__!r}")
        return f"ForwardRef({self.__forward_arg__!r}{''.join(extra)})"


def _get_value_annotations(obj):
    if isinstance(obj, type) and '__annotations__' not in getattr(obj, '__dict__', {}):
        annotations = None
    else:
        annotations = getattr(obj, '__annotations__', None)
    if annotations is None:
        annotate = getattr(obj, '__annotate__', None)
        if annotate is not None:
            annotations = annotate(1)
            if not isinstance(annotations, dict):
                raise ValueError(f"{obj!r}.__annotate__ returned a non-dict")
        elif isinstance(obj, type) or callable(obj):
            return {}
        else:
            raise TypeError(f"{obj!r} does not have annotations")
    if not isinstance(annotations, dict):
        raise ValueError(f"{obj!r}.__annotations__ is neither a dict nor None")
    return dict(annotations)
