"""Interpret sys.version using the formats from CPython 3.14.5 Lib/platform.py."""

import re
import sys


# CPython and Jython parse with re.ASCII. Spell out its word and space classes
# so the same patterns also work in the typed regex backend.
_cpython = re.compile(
    r"([a-zA-Z0-9_.+]+)[ \t\n\r\f\v]*(?:free-threading build[ \t\n\r\f\v]+)?"
    r"\(#?([^,]+)(?:,[ \t\n\r\f\v]*([a-zA-Z0-9_ ]*)(?:,[ \t\n\r\f\v]*([a-zA-Z0-9_ :]*))?)?\)"
    r"[ \t\n\r\f\v]*\[([^\]]+)\]?"
)
_jython = re.compile(
    r"([a-zA-Z0-9_.+]+)[ \t\n\r\f\v]*"
    r"\(#?([^,]+)(?:,[ \t\n\r\f\v]*([a-zA-Z0-9_ ]*)(?:,[ \t\n\r\f\v]*([a-zA-Z0-9_ :]*))?)?\)"
    r"[ \t\n\r\f\v]*\[([^\]]+)\]?"
)
_pypy = re.compile(r"([\w.+]+)\s*\(#?([^,]+),\s*([\w ]+),\s*([\w :]+)\)\s*\[PyPy [^\]]+\]?")
_imre = re.compile(r"([0-9]+\.[0-9]+\.[0-9]+) \(Imre ([0-9]+\.[0-9]+\.[0-9]+)\)\Z")


def _parse_version(text: str, host: str) -> tuple[str, str, str, str, str]:
    if host.startswith("java"):
        name = "Jython"
        match = _jython.match(text)
    elif "PyPy" in text:
        name = "PyPy"
        match = _pypy.match(text)
    else:
        native = _imre.match(text)
        if native is not None:
            return "Imre", native.group(1) or "", native.group(2) or "", "", "Imre"
        name = "CPython"
        match = _cpython.match(text)
    if match is None:
        raise ValueError("failed to parse " + name + " sys.version: " + repr(text))
    version = match.group(1) or ""
    if len(version.split(".")) == 2:
        version += ".0"
    build = match.group(2) or ""
    date = match.group(3) or ""
    compiler = ""
    if name == "CPython":
        time = match.group(4)
        if time:
            date += " " + time
        compiler = match.group(5) or ""
    elif name == "Jython":
        compiler = host
    return name, version, build, date, compiler
