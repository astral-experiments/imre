"""Signature support for boxed Python functions. See CPython 3.14.5 Lib/inspect.py."""

import enum
import itertools
import re
import types
from annotationlib import Format, ForwardRef
from collections import OrderedDict
from keyword import iskeyword
from inspect import _signature_identifier, _signature_layout, _signature_mapping


class _ParameterKind(enum.IntEnum):
    POSITIONAL_ONLY = 0
    POSITIONAL_OR_KEYWORD = 1
    VAR_POSITIONAL = 2
    KEYWORD_ONLY = 3
    VAR_KEYWORD = 4

    @property
    def description(self):
        return ("positional-only", "positional or keyword", "variadic positional",
                "keyword-only", "variadic keyword")[self.value]

    def __str__(self):
        return self.name


def _signature_from_callable(obj, *, sigcls, follow_wrapper_chains=True,
                             globals=None, locals=None, eval_str=False,
                             annotation_format=Format.VALUE):
    if not callable(obj):
        raise TypeError(f"{obj!r} is not a callable object")
    if follow_wrapper_chains and not isinstance(obj, types.MethodType):
        # Keep each object live while comparing identities. The boxed runtime
        # caps nested calls at 1000, matching CPython's default recursion limit.
        original = obj
        seen = {id(obj): obj}
        while not isinstance(obj, type) and hasattr(obj, "__wrapped__"):
            if hasattr(obj, "__signature__") or isinstance(obj, types.MethodType):
                break
            obj = obj.__wrapped__
            key = id(obj)
            if key in seen or len(seen) >= 1000:
                raise ValueError(f"wrapper loop when unwrapping {original!r}")
            seen[key] = obj
    if isinstance(obj, types.MethodType):
        inner = _signature_from_callable(
            obj.__func__, sigcls=sigcls, follow_wrapper_chains=follow_wrapper_chains,
            globals=globals, locals=locals, eval_str=eval_str,
            annotation_format=annotation_format)
        return _signature_bound_method(inner)
    explicit = getattr(obj, "__signature__", None)
    if explicit is not None:
        if not isinstance(explicit, Signature):
            raise TypeError(f"unexpected object {explicit!r} in __signature__ attribute")
        return explicit
    if not isinstance(obj, types.FunctionType):
        raise ValueError(f"callable {obj!r} is not supported by signature")
    if getattr(obj, "__text_signature__", None):
        raise NotImplementedError("text signatures are not yet supported")
    if eval_str or annotation_format != Format.VALUE:
        raise NotImplementedError("native signatures currently support value annotations without string evaluation")
    layout = _signature_layout(obj)
    annotations = obj.__annotations__
    defaults = obj.__defaults__ or ()
    keyword_defaults = obj.__kwdefaults__ or {}
    positional = [name for kind, name in layout if kind < _VAR_POSITIONAL]
    # CPython slices the names first; a replaced __defaults__ may be longer
    # than the argument list, in which case the slice uses a negative start.
    default_names = positional[len(positional) - len(defaults):]
    positional_defaults = dict(zip(default_names, defaults))
    parameters = []
    for kind, name in layout:
        if kind == _KEYWORD_ONLY:
            default = keyword_defaults.get(name, _empty)
        else:
            default = positional_defaults.get(name, _empty)
        parameters.append(sigcls._parameter_cls(name, kind, default=default,
                                                annotation=annotations.get(name, _empty)))
    return sigcls(parameters, return_annotation=annotations.get("return", _empty),
                  __validate_parameters__=False)
