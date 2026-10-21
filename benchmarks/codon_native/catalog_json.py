"""Appended after the unchanged catalog module's declarations."""

from codon_json_helpers import array, load, optional_string, record, required, string


def decode(text: str, schema):
    if schema is not Release:
        compile_error("unsupported catalog JSON schema")
    value = record(load(text))
    artifacts: list[Artifact] = []
    for item in array(required(value, "artifacts")).values():
        record(item)
        artifacts.append(Artifact(
            url=string(required(item, "url")),
            platform=string(required(item, "platform")),
            variant=string(required(item, "variant")),
            sha256=optional_string(item.get("sha256")),
        ))
    return Release(string(required(value, "version")), artifacts)


def encode(values: dict[str, CatalogEntry], indent: int = 2, ensure_ascii: bool = True) -> str:
    result = json.JSON.object()
    for key, entry in values.items():
        architecture = json.JSON.object()
        architecture["family"] = entry.arch.family
        architecture["variant"] = entry.arch.variant
        item = json.JSON.object()
        item["name"] = entry.name
        item["arch"] = architecture
        item["os"] = entry.os
        item["libc"] = entry.libc
        item["major"] = entry.major
        item["minor"] = entry.minor
        item["patch"] = entry.patch
        item["prerelease"] = entry.prerelease
        item["url"] = entry.url
        item["sha256"] = entry.sha256
        item["variant"] = entry.variant
        item["build"] = entry.build
        result[key] = item
    rendered = json.dumps(result, indent=indent, ensure_ascii=ensure_ascii, allow_nan=False, max_depth=128)
    # Codon 0.20.4 escapes code points above 127, while Python also escapes DEL.
    return rendered.replace("\x7f", "\\u007f") if ensure_ascii else rendered
