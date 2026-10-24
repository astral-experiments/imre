"""Concrete JSON codecs for CPython execution of Imre programs.

The native compiler recognizes this import and generates a concrete decoder.
Python's dynamic parse tree stays private to this compatibility implementation.
"""

import dataclasses
import inspect
import json
import math
import sys
import types
from functools import cache
from typing import TypeVar, Union, get_args, get_origin, get_type_hints

T = TypeVar("T")


class _ObjectPairs(list):
    """Retain duplicates until a requested record schema determines known keys."""


def _integer(text: str) -> int:
    # JSON integers retain arbitrary precision independently of CPython's
    # configurable decimal conversion limit. Small chunks avoid that limit.
    negative = text.startswith("-")
    digits = text[1:] if negative else text
    value = 0
    for position in range(0, len(digits), 9):
        chunk = digits[position:position + 9]
        value = value * 10 ** len(chunk) + int(chunk)
    return -value if negative else value


def _constant(value: str):
    raise ValueError("non-finite constants are not JSON")


def _check_depth(text: str) -> None:
    depth = 0
    quoted = False
    escaped = False
    for character in text:
        if quoted:
            if escaped:
                escaped = False
            elif character == "\\":
                escaped = True
            elif character == '"':
                quoted = False
        elif character == '"':
            quoted = True
        elif character in "[{":
            depth += 1
            if depth > 128:
                raise ValueError("JSON nesting exceeds 128 levels")
        elif character in "]}":
            depth -= 1


def _check_string(value: str) -> None:
    if any(0xD800 <= ord(character) <= 0xDFFF for character in value):
        raise ValueError("JSON strings must contain Unicode scalar values")


def _convert(value, annotation):
    if annotation is None:
        annotation = type(None)
    if annotation is float:
        if type(value) not in (int, float):
            raise ValueError("JSON value does not match schema")
        try:
            result = float(value)
        except OverflowError as error:
            raise ValueError("JSON float is out of range") from error
        if not math.isfinite(result):
            raise ValueError("JSON float is out of range")
        return result
    if annotation in (int, bool, str, type(None)):
        if type(value) is not annotation:
            raise ValueError("JSON value does not match schema")
        if annotation is str:
            _check_string(value)
        return value
    origin = get_origin(annotation)
    arguments = get_args(annotation)
    if origin in (Union, types.UnionType):
        non_null = [argument for argument in arguments if argument is not type(None)]
        if len(arguments) != 2 or len(non_null) != 1:
            raise TypeError("JSON schemas support only optional unions")
        return None if value is None else _convert(value, non_null[0])
    if origin is list:
        if type(value) is not list or len(arguments) != 1:
            raise ValueError("JSON value does not match schema")
        return [_convert(item, arguments[0]) for item in value]
    if origin is dict:
        if len(arguments) != 2 or arguments[0] is not str:
            raise TypeError("JSON dictionaries require exact str keys")
        if not isinstance(value, _ObjectPairs):
            raise ValueError("JSON value does not match schema")
        converted = {}
        for name, child in value:
            _check_string(name)
            if name in converted:
                raise ValueError("duplicate JSON dictionary key")
            converted[name] = _convert(child, arguments[1])
        return converted
    if not isinstance(annotation, type) or "__init__" not in annotation.__dict__:
        raise TypeError("JSON schemas require concrete annotated record classes")
    if not isinstance(value, _ObjectPairs):
        raise ValueError("JSON value does not match schema")
    fields, parameters, factories = _schema(annotation)
    converted = {}
    for name, child in value:
        _check_string(name)
        if name not in fields:
            continue
        if name in converted:
            raise ValueError("duplicate JSON field: " + name)
        converted[name] = _convert(child, fields[name])
    for name, field_type in fields.items():
        if name in converted:
            continue
        parameter = parameters.get(name)
        if parameter is None or parameter.default is inspect.Parameter.empty:
            raise ValueError("missing JSON field: " + name)
        default = parameter.default
        if name in factories:
            converted[name] = _convert(factories[name](), field_type)
        elif get_origin(field_type) is list and default is None:
            converted[name] = []
        elif type(default) in (int, float, bool, str, type(None)):
            converted[name] = _convert(default, field_type)
        else:
            raise TypeError("JSON record defaults must be exact scalars or fresh lists")
    positional = [converted[name] for name, parameter in parameters.items()
                  if parameter.kind is inspect.Parameter.POSITIONAL_ONLY]
    keywords = {name: value for name, value in converted.items()
                if parameters[name].kind is not inspect.Parameter.POSITIONAL_ONLY}
    return annotation(*positional, **keywords)


@cache
def _schema(annotation):
    fields = get_type_hints(annotation)
    parameters = inspect.signature(annotation).parameters
    if fields.keys() != parameters.keys():
        raise TypeError("JSON record constructor parameters must match its fields")
    factories = {}
    if dataclasses.is_dataclass(annotation):
        declared = {field.name: field for field in dataclasses.fields(annotation)}
        if fields.keys() != declared.keys():
            raise TypeError("JSON dataclasses require ordinary initialized fields")
        for name, parameter in parameters.items():
            # inspect.signature exposes this sentinel for generated parameters.
            # Comparing its identity preserves explicit __init__ defaults even
            # when a dataclass field separately declares a default factory.
            if parameter.default is dataclasses._HAS_DEFAULT_FACTORY:
                factory = declared[name].default_factory
                if factory is not list and factory is not dict and factory is not set:
                    raise TypeError("JSON dataclass factories must be list, dict, or set")
                factories[name] = factory
    return fields, parameters, factories


def decode(text: str, schema: type[T], /) -> T:
    """Decode strict JSON into a concrete record, list, or dict[str, T] schema.

    Unknown fields are syntax-checked and ignored. Duplicate known fields are
    rejected; duplicates entirely in ignored data have no observable effect.
    Missing scalar fields use exact constructor defaults. Dataclass list
    factories and explicit list parameters defaulting to None receive fresh
    empty lists. Explicit JSON null
    is accepted only by nullable field annotations. Nesting is limited to 128
    object/array levels, including ignored values.
    """
    if type(text) is not str:
        raise TypeError("JSON input must be an exact str")
    _check_depth(text)
    try:
        value = json.loads(text, object_pairs_hook=_ObjectPairs,
                           parse_int=_integer, parse_constant=_constant)
    except json.JSONDecodeError as error:
        raise ValueError("invalid JSON") from error
    return _convert(value, schema)


@cache
def _encoding_fields(record_type):
    if "__init__" not in record_type.__dict__:
        raise TypeError("JSON encoding requires fixed annotated record classes")
    return tuple(get_type_hints(record_type))


def _encoding_value(value, active, depth):
    kind = type(value)
    if kind in (int, bool, type(None)):
        return value
    if kind is str:
        _check_string(value)
        return value
    if kind is float:
        if not math.isfinite(value):
            raise ValueError("non-finite floats are not JSON")
        return value
    if depth >= 128:
        raise ValueError("JSON nesting exceeds 128 containers")
    identity = id(value)
    if identity in active:
        raise ValueError("JSON values must be acyclic")
    active.add(identity)
    try:
        if kind is list:
            return [_encoding_value(child, active, depth + 1) for child in value]
        if kind is dict:
            result = {}
            for key, child in value.items():
                if type(key) is not str:
                    raise TypeError("JSON dictionaries require exact str keys")
                _check_string(key)
                result[key] = _encoding_value(child, active, depth + 1)
            return result
        if kind in (bytes, tuple, set):
            raise TypeError("unsupported JSON encoding type")
        return {name: _encoding_value(getattr(value, name), active, depth + 1)
                for name in _encoding_fields(kind)}
    finally:
        active.remove(identity)


def _integer_text(value):
    """Keep native arbitrary precision independent of CPython's decimal limit."""
    negative = value < 0
    value = abs(value)
    chunks = []
    while value >= 1_000_000_000:
        value, chunk = divmod(value, 1_000_000_000)
        chunks.append(f"{chunk:09d}")
    return ("-" if negative else "") + str(value) + "".join(reversed(chunks))


def _dump_unbounded(value, indent, ensure_ascii, depth=0):
    # The normal path uses the standard encoder. Only integers beyond its
    # configurable decimal conversion limit need this formatting fallback.
    if type(value) is int:
        return _integer_text(value)
    if type(value) not in (list, dict):
        return json.dumps(value, ensure_ascii=ensure_ascii, allow_nan=False)
    if type(value) is list:
        opening, closing = "[", "]"
        items = [_dump_unbounded(child, indent, ensure_ascii, depth + 1)
                 for child in value]
    else:
        opening, closing = "{", "}"
        items = [json.dumps(key, ensure_ascii=ensure_ascii) + ": "
                 + _dump_unbounded(child, indent, ensure_ascii, depth + 1)
                 for key, child in value.items()]
    if not items:
        return opening + closing
    if indent is None:
        return opening + ", ".join(items) + closing
    width = max(0, indent)
    prefix = "\n" + " " * (width * (depth + 1))
    return opening + prefix + ("," + prefix).join(items) + "\n" + " " * (width * depth) + closing


def encode(value: object, /, *, indent: int | None = None, ensure_ascii: bool = True) -> str:
    """Encode concrete values, preserving record field and dictionary order.

    Accepted values are finite scalars, lists, string-key dictionaries, nullable
    values, and fixed annotated records. Records serialize declared fields;
    their constructors are not called. Integer indentation has platform-index
    bounds, Unicode strings contain scalar values, and nesting is limited to
    128 containers. The native implementation writes directly from typed storage.
    """
    if indent is not None:
        if type(indent) is not int:
            raise TypeError("JSON indent must be an exact int or None")
        if not -sys.maxsize - 1 <= indent <= sys.maxsize:
            raise OverflowError("JSON indent does not fit in a platform index")
    if type(ensure_ascii) is not bool:
        raise TypeError("ensure_ascii must be an exact bool")
    converted = _encoding_value(value, set(), 0)
    try:
        return json.dumps(converted, indent=indent, ensure_ascii=ensure_ascii, allow_nan=False)
    except ValueError:
        return _dump_unbounded(converted, indent, ensure_ascii)
