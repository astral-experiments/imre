"""Concrete TOML 1.1 decoding for CPython execution of Imre programs.

Python 3.15+ uses tomllib. Earlier Python versions require ``tomli>=2.4``
(``python -m pip install 'tomli>=2.4'``). Native executables link a Rust parser
and never load this module or a Python runtime.
"""

import inspect
import sys
import types
from typing import TypeVar, Union, get_args, get_origin

from .json import _convert as _convert_default, _schema

T = TypeVar("T")


def _load_parser():
    # Importing an application with an optional TOML path needs no dependency.
    if sys.version_info >= (3, 15):
        import tomllib
        return tomllib
    try:
        import tomli
    except ImportError as error:
        raise ImportError("imre.toml requires tomli>=2.4 on Python <3.15") from error
    if tuple(int(part) for part in tomli.__version__.split(".")[:2]) < (2, 4):
        raise ImportError("imre.toml requires tomli>=2.4 for TOML 1.1 support")
    return tomli


def _convert(value, annotation):
    if annotation is None:
        annotation = type(None)
    if annotation in (int, float, bool, str, type(None)):
        if type(value) is not annotation:
            raise ValueError("TOML value does not match schema")
        return value
    origin = get_origin(annotation)
    arguments = get_args(annotation)
    if origin in (Union, types.UnionType):
        non_null = [argument for argument in arguments if argument is not type(None)]
        if len(arguments) != 2 or len(non_null) != 1:
            raise TypeError("TOML schemas support only optional unions")
        return _convert(value, non_null[0])
    if origin is list:
        if type(value) is not list or len(arguments) != 1:
            raise ValueError("TOML value does not match schema")
        return [_convert(child, arguments[0]) for child in value]
    if origin is dict:
        if len(arguments) != 2 or arguments[0] is not str:
            raise TypeError("TOML dictionaries require exact str keys")
        if type(value) is not dict:
            raise ValueError("TOML value does not match schema")
        return {key: _convert(child, arguments[1]) for key, child in value.items()}
    if not isinstance(annotation, type) or "__init__" not in annotation.__dict__:
        raise TypeError("TOML schemas require concrete annotated record classes")
    if type(value) is not dict:
        raise ValueError("TOML value does not match schema")
    fields, parameters, factories = _schema(annotation)
    converted = {name: _convert(child, fields[name])
                 for name, child in value.items() if name in fields}
    for name, field_type in fields.items():
        if name in converted:
            continue
        parameter = parameters.get(name)
        if parameter is None or parameter.default is inspect.Parameter.empty:
            raise ValueError("missing TOML field: " + name)
        default = parameter.default
        if name in factories:
            converted[name] = _convert_default(factories[name](), field_type)
        elif get_origin(field_type) is list and default is None:
            converted[name] = []
        elif type(default) in (int, float, bool, str, type(None)):
            converted[name] = _convert_default(default, field_type)
        else:
            raise TypeError("TOML record defaults must be exact scalars or fresh lists")
    positional = [converted[name] for name, parameter in parameters.items()
                  if parameter.kind is inspect.Parameter.POSITIONAL_ONLY]
    keywords = {name: value for name, value in converted.items()
                if parameters[name].kind is not inspect.Parameter.POSITIONAL_ONLY}
    return annotation(*positional, **keywords)


def decode(text: str, schema: type[T], /) -> T:
    """Decode a TOML 1.1 document into a concrete record or dict[str, T].

    Scalar types remain exact: integers and booleans do not satisfy float fields.
    Integers retain arbitrary precision; TOML inf/nan remain native floats.
    CPython's configured decimal conversion limit applies in its parser.
    Optional fields use their declared defaults when absent; TOML has no null.
    Unknown fields are fully parsed and ignored, including valid date/time
    values. Date/time values cannot be exposed by a schema. Nesting is limited
    to 64 table/array containers, including the document and ignored values.
    """
    if type(text) is not str:
        raise TypeError("TOML input must be an exact str")
    if get_origin(schema) is not dict and (
        not isinstance(schema, type) or "__init__" not in schema.__dict__
        or schema in (int, float, bool, str, list, dict)
    ):
        raise TypeError("TOML decode schema must be a fixed record or string-key dictionary")
    parser = _load_parser()
    try:
        value = parser.loads(text.removeprefix("\ufeff"))
    except (parser.TOMLDecodeError, RecursionError) as error:
        raise ValueError("invalid TOML") from error
    pending = [(value, 1)]
    while pending:
        current, depth = pending.pop()
        if type(current) in (list, dict):
            if depth > 64:
                raise ValueError("TOML nesting exceeds 64 containers")
            children = current.values() if type(current) is dict else current
            pending.extend((child, depth + 1) for child in children)
    return _convert(value, schema)
