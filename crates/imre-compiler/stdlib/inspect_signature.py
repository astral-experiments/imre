"""Signature support for boxed Python functions. See CPython 3.14.5 Lib/inspect.py."""

import enum
import importlib.machinery
import itertools
import os
import re
import types
from annotationlib import Format, ForwardRef
from collections import OrderedDict
from keyword import iskeyword
from inspect import _signature_identifier, _signature_layout, _signature_mapping, _static_member


_sentinel = object()


def getattr_static(obj, attr, default=_sentinel):
    """Retrieve an attribute without invoking descriptors or attribute hooks."""
    value = _static_member(obj, attr, _sentinel)
    if value is not _sentinel:
        return value
    if default is not _sentinel:
        return default
    raise AttributeError(attr)


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


def formatannotation(annotation, base_module=None, *, quote_annotation_strings=True):
    if not quote_annotation_strings and isinstance(annotation, str):
        return annotation
    if getattr(annotation, '__module__', None) == 'typing':
        # Preserve punctuation and strip typing. only at the start of a dotted name.
        return ''.join(part.removeprefix('typing.') for part in
                       re.split(r'([^\w.]+)', repr(annotation)))
    if isinstance(annotation, types.GenericAlias):
        return str(annotation)
    if isinstance(annotation, type):
        if annotation.__module__ in ('builtins', base_module):
            return annotation.__qualname__
        return annotation.__module__+'.'+annotation.__qualname__
    if isinstance(annotation, ForwardRef):
        return annotation.__forward_arg__
    return repr(annotation)


def _constructor(obj):
    # Inspect binds a metaclass's __call__ descriptor to the metaclass itself.
    meta = type(obj)
    for base in getattr(meta, "__mro__", ()):
        if base is type or base is object:
            break
        if "__call__" in base.__dict__:
            method = base.__dict__["__call__"]
            if method is not None:
                return _bind_signature_method(method, meta)
            break

    # A shadowed __init__ and an inherited __new__ can coexist. Find the first
    # definition of each, preserving the order in which their classes appear.
    seen = set()
    uses_object_defaults = True
    for base in getattr(obj, "__mro__", (obj,)):
        if base is object:
            if uses_object_defaults:
                return None, False
            break
        namespace = getattr(base, "__dict__", None)
        if namespace is None:
            break  # The native builtin exposes no inspectable constructor.
        for name, default in (("__new__", object.__new__), ("__init__", object.__init__)):
            if name not in seen and name in namespace:
                seen.add(name)
                method = obj.__new__ if name == "__new__" else namespace[name]
                if method is default:
                    continue
                uses_object_defaults = False
                if method is None:
                    continue
                if name == "__new__":
                    return method, True
                return _bind_signature_method(method, obj)
    raise ValueError(f"no signature found for builtin type {obj!r}")


def _bind_signature_method(method, owner):
    if isinstance(method, types.FunctionType):
        # A bound Python function omits the receiver from its signature.
        return method, True
    if not isinstance(method, type):
        get = getattr(type(method), "__get__", _sentinel)
        if get is not _sentinel:
            return get(method, owner, type(owner)), False
    return method, False


def _instance_call(obj):
    method = getattr_static(type(obj), "__call__", None)
    if method is not None and not getattr(obj, "__text_signature__", None):
        bound, drop_first = _bind_signature_method(method, obj)
        # Native call wrappers have no Python parameter layout to recurse into.
        function = bound.__func__ if isinstance(bound, types.MethodType) else bound
        if isinstance(function, types.FunctionType) or not callable(bound):
            return bound, drop_first
    raise ValueError(f"callable {obj!r} is not supported by signature")


def _signature_from_callable(obj, *, sigcls, follow_wrapper_chains=True,
                             globals=None, locals=None, eval_str=False,
                             annotation_format=Format.VALUE):
    if not callable(obj):
        raise TypeError(f"{obj!r} is not a callable object")
    if follow_wrapper_chains and not isinstance(obj, types.MethodType):
        obj = unwrap(obj, stop=lambda f: hasattr(f, "__signature__")
                     or isinstance(f, types.MethodType))
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
        if isinstance(obj, type):
            method, drop_first = _constructor(obj)
            if method is None:
                return sigcls()
        else:
            method, drop_first = _instance_call(obj)
        inner = _signature_from_callable(
            method, sigcls=sigcls, follow_wrapper_chains=follow_wrapper_chains,
            globals=globals, locals=locals, eval_str=eval_str,
            annotation_format=annotation_format)
        return _signature_bound_method(inner) if drop_first else inner
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
