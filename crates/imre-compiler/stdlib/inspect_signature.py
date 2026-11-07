"""Signature support for boxed Python functions. See CPython 3.14.5 Lib/inspect.py."""

import enum
import importlib.machinery
import itertools
import os
import re
import types
from annotationlib import Format, ForwardRef, get_annotations
from collections import OrderedDict
from keyword import iskeyword
from inspect import (
    _descriptor_get,
    _native_data_descriptor,
    _signature_layout,
    _static_member,
)


_sentinel = object()


def getargs(co):
    """Return the named, variadic positional and variadic keyword arguments of a code object."""
    if not iscode(co):
        raise TypeError(f"{co!r} is not a code object")
    args = []
    varargs = varkw = None
    for kind, name in _signature_layout(co):
        if kind == _VAR_POSITIONAL:
            varargs = name
        elif kind == _VAR_KEYWORD:
            varkw = name
        else:
            args.append(name)
    return Arguments(args, varargs, varkw)


def getattr_static(obj, attr, default=_sentinel):
    """Retrieve an attribute without invoking descriptors or attribute hooks."""
    value = _static_member(obj, attr, _sentinel)
    if value is not _sentinel:
        return value
    if default is not _sentinel:
        return default
    raise AttributeError(attr)


def _unwrap_constructor_method(method, follow_wrapper_chains):
    if follow_wrapper_chains:
        return unwrap(method, stop=lambda f: hasattr(f, "__signature__") or _signature_is_builtin(f))
    return method


def _signature_from_class(obj, sigcls, follow_wrapper_chains, signature_of_method):
    # Only a user-defined metaclass __call__ takes precedence over the class constructor.
    meta = type(obj)
    for base in getattr(meta, "__mro__", ()):
        if base is type or base is object:
            break
        if "__call__" in base.__dict__:
            method = base.__dict__["__call__"]
            if method is not None:
                original = _unwrap_constructor_method(method, follow_wrapper_chains)
                if not isinstance(original, _NonUserDefinedCallables):
                    return signature_of_method(*_bind_signature_method(method, meta))
            break

    # A shadowed __init__ and an inherited __new__ can coexist. Find the first
    # definition of each, preserving the order in which their classes appear.
    seen = set()
    uses_object_defaults = True
    mro = getattr(obj, "__mro__", (obj,))
    for base in mro:
        if base is object:
            break
        namespace = getattr(base, "__dict__", None)
        if namespace is None:
            uses_object_defaults = False
            continue
        for name, default in (("__new__", object.__new__), ("__init__", object.__init__)):
            if name not in seen and name in namespace:
                seen.add(name)
                method = obj.__new__ if name == "__new__" else namespace[name]
                # A decorator can wrap the object default without defining a constructor.
                original = _unwrap_constructor_method(method, follow_wrapper_chains)
                if original is default:
                    continue
                uses_object_defaults = False
                if method is None or isinstance(original, _NonUserDefinedCallables):
                    continue
                if name == "__new__":
                    return signature_of_method(method, True)
                return signature_of_method(*_bind_signature_method(method, obj))
    for base in mro:
        if base is object:
            break
        text_signature = getattr(base, "__text_signature__", None)
        if text_signature:
            return _signature_fromstr(sigcls, base, text_signature)
    if any(base is type for base in mro) and obj is not type:
        raise ValueError(f"callable {obj!r} is not supported by signature")
    if uses_object_defaults:
        return sigcls()
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


def _signature_from_callable(obj, *, sigcls, follow_wrapper_chains=True,
                             skip_bound_arg=True, globals=None, locals=None,
                             eval_str=False, annotation_format=Format.VALUE):
    if not callable(obj):
        raise TypeError(f"{obj!r} is not a callable object")
    if follow_wrapper_chains and not isinstance(obj, types.MethodType):
        obj = unwrap(obj, stop=lambda f: hasattr(f, "__signature__")
                     or isinstance(f, types.MethodType))
    if isinstance(obj, types.MethodType):
        inner = _signature_from_callable(
            obj.__func__, sigcls=sigcls, follow_wrapper_chains=follow_wrapper_chains,
            skip_bound_arg=skip_bound_arg, globals=globals, locals=locals,
            eval_str=eval_str, annotation_format=annotation_format)
        return _signature_bound_method(inner) if skip_bound_arg else inner
    explicit = getattr(obj, "__signature__", None)
    if explicit is not None:
        if not isinstance(explicit, Signature):
            raise TypeError(f"unexpected object {explicit!r} in __signature__ attribute")
        return explicit
    partialmethod = getattr(obj, "__partialmethod__", None)
    if isinstance(partialmethod, functools.partialmethod):
        wrapped = _signature_from_callable(
            partialmethod.func, sigcls=sigcls, follow_wrapper_chains=follow_wrapper_chains,
            skip_bound_arg=skip_bound_arg, globals=globals, locals=locals,
            eval_str=eval_str, annotation_format=annotation_format)
        # Reserve the receiver while binding the partial arguments, then restore it
        # in the signature of the unbound method.
        sig = _signature_get_partial(wrapped, partialmethod, (None,))
        receiver = tuple(wrapped.parameters.values())[0]
        if receiver.kind is Parameter.VAR_POSITIONAL:
            return sig
        if partialmethod.args.count(functools.Placeholder):
            receiver = receiver.replace(kind=Parameter.POSITIONAL_ONLY)
        return sig.replace(parameters=(receiver,) + tuple(sig.parameters.values()))
    if isinstance(obj, functools.partial):
        wrapped = _signature_from_callable(
            obj.func, sigcls=sigcls, follow_wrapper_chains=follow_wrapper_chains,
            skip_bound_arg=skip_bound_arg, globals=globals, locals=locals,
            eval_str=eval_str, annotation_format=annotation_format)
        return _signature_get_partial(wrapped, obj)
    if isinstance(obj, types.FunctionType) or _signature_is_functionlike(obj):
        return _signature_from_function(
            obj, sigcls, globals, locals, eval_str, annotation_format, skip_bound_arg=skip_bound_arg)
    kind = type(obj)
    if (kind is types.BuiltinFunctionType or kind is types.WrapperDescriptorType
            or kind is types.MethodWrapperType or kind is types.MethodDescriptorType):
        text_signature = getattr(obj, "__text_signature__", None)
        if not text_signature:
            raise ValueError(f"no signature found for builtin {obj!r}")
        return _signature_fromstr(sigcls, obj, text_signature, skip_bound_arg=skip_bound_arg)

    def signature_of_method(method, drop_first):
        inner = _signature_from_callable(
            method, sigcls=sigcls, follow_wrapper_chains=follow_wrapper_chains,
            skip_bound_arg=skip_bound_arg, globals=globals, locals=locals,
            eval_str=eval_str, annotation_format=annotation_format)
        return _signature_bound_method(inner) if drop_first and skip_bound_arg else inner

    if isinstance(obj, type):
        return _signature_from_class(obj, sigcls, follow_wrapper_chains, signature_of_method)
    call = getattr_static(type(obj), "__call__", None)
    if call is None:
        raise ValueError(f"callable {obj!r} is not supported by signature")
    text_signature = getattr(obj, "__text_signature__", None)
    if text_signature:
        return _signature_fromstr(sigcls, obj, text_signature)
    return signature_of_method(_descriptor_get(call, obj), False)


def _signature_from_function(obj, sigcls, globals, locals, eval_str, annotation_format,
                             *, skip_bound_arg=True):
    is_duck = not isinstance(obj, types.FunctionType)
    if is_duck and not _signature_is_functionlike(obj):
        raise TypeError(f"{obj!r} is not a Python function")
    text_signature = getattr(obj, "__text_signature__", None)
    if text_signature:
        return _signature_fromstr(sigcls, obj, text_signature, skip_bound_arg=skip_bound_arg)
    layout = _signature_layout(obj.__code__ if is_duck else obj)
    annotations = get_annotations(
        obj, globals=globals, locals=locals, eval_str=eval_str, format=annotation_format)
    defaults = obj.__defaults__
    keyword_defaults = obj.__kwdefaults__
    positional = [name for kind, name in layout if kind < _VAR_POSITIONAL]
    # CPython slices the names first; a replaced __defaults__ may be longer
    # than the argument list, in which case the slice uses a negative start.
    default_count = len(defaults) if defaults else 0
    default_names = positional[len(positional) - default_count:]
    default_indices = {name: index for index, name in enumerate(default_names)}
    parameters = []
    for kind, name in layout:
        default = _empty
        if kind == _KEYWORD_ONLY and keyword_defaults is not None:
            default = keyword_defaults.get(name, _empty)
        elif name in default_indices:
            default = defaults[default_indices[name]]
        parameters.append(sigcls._parameter_cls(name, kind, default=default,
                                                annotation=annotations.get(name, _empty)))
    return sigcls(parameters, return_annotation=annotations.get("return", _empty),
                  __validate_parameters__=is_duck)
