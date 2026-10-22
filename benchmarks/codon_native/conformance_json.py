"""Appended after the unchanged conformance parse module's declarations."""

from codon_json_helpers import array, integer, load, record, required, string


def _json_position(value: json.JSON) -> Position:
    record(value)
    return Position(line=integer(required(value, "line")), column=integer(required(value, "column")))


def _json_location(value: json.JSON) -> Location:
    record(value)
    positions = record(required(value, "positions"))
    return Location(
        path=string(required(value, "path")),
        positions=Positions(
            begin=_json_position(required(positions, "begin")),
            end=_json_position(required(positions, "end")),
        ),
    )


def decode(text: str, schema):
    value = load(text)
    if schema is Manifest:
        record(value)
        aliases: list[PathAlias] = []
        for alias in array(required(value, "diagnostic_paths")).values():
            record(alias)
            aliases.append(PathAlias(string(required(alias, "raw")), string(required(alias, "path"))))
        files = [string(path) for path in array(required(value, "test_files")).values()]
        return Manifest(string(required(value, "suite_revision")), files, aliases)
    elif schema is list[WireDiagnostic]:
        diagnostics: list[WireDiagnostic] = []
        for item in array(value).values():
            record(item)
            diagnostics.append(WireDiagnostic(
                check_name=string(required(item, "check_name")),
                description=string(required(item, "description")),
                severity=string(required(item, "severity")),
                location=_json_location(required(item, "location")),
            ))
        return diagnostics
    else:
        compile_error("unsupported conformance JSON schema")
