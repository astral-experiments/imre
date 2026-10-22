"""Native Codon JSON boundary helpers; imported only by prepared Codon sources."""

import json


def string(value: json.JSON) -> str:
    if not value.is_str():
        raise ValueError("JSON value does not match string schema")
    result = value.as_str()
    for character in result:
        code = ord(character)
        if 0xD800 <= code <= 0xDFFF:
            raise ValueError("JSON strings must contain Unicode scalar values")
    return result


def optional_string(value: json.JSON) -> str | None:
    return None if value.is_null() else string(value)


def integer(value: json.JSON) -> int:
    if value.kind != "int":
        raise ValueError("JSON value does not match integer schema")
    return value.as_int()


def record(value: json.JSON) -> json.JSON:
    if not value.is_object():
        raise ValueError("JSON value does not match record schema")
    for key in value.keys():
        string(json.JSON(key))
    return value


def required(value: json.JSON, key: str) -> json.JSON:
    if not value.is_object() or key not in value:
        raise ValueError("missing required JSON field: " + key)
    return value[key]


def array(value: json.JSON) -> json.JSON:
    if not value.is_array():
        raise ValueError("JSON value does not match array schema")
    return value


def load(text: str) -> json.JSON:
    return json.loads(text, max_depth=128)
