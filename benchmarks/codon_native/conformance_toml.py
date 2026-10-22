"""Appended after the live module; decode only its concrete test-group schema."""

import json

from codon_platform import call as platform_call
from codon_json_helpers import record, required, string


def decode_toml(text: str, schema):
    if schema is dict[str, TestGroup]:
        value = record(json.loads(platform_call(1, text)))
        result: dict[str, TestGroup] = {}
        for key in value.keys():
            group = record(value[key])
            result[key] = TestGroup(string(required(group, "name")), string(required(group, "href")))
        return result
    else:
        compile_error("unsupported native conformance TOML schema")
