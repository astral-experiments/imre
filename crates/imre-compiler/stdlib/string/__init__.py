"""ASCII character sets, word capitalization, and dollar substitution."""

import re
from collections.abc import Mapping

# Constant definitions from CPython 3.14.5 Lib/string/__init__.py (PSF-2.0).
whitespace = ' \t\n\r\v\f'
ascii_lowercase = 'abcdefghijklmnopqrstuvwxyz'
ascii_uppercase = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ'
ascii_letters = ascii_lowercase + ascii_uppercase
digits = '0123456789'
hexdigits = digits + 'abcdef' + 'ABCDEF'
octdigits = '01234567'
punctuation = r"""!"#$%&'()*+,-./:;<=>?@[\]^_`{|}~"""
printable = digits + ascii_letters + punctuation + whitespace


def capwords(s: str, sep: str | None = None) -> str:
    if sep is None:
        return " ".join([word.capitalize() for word in s.split()])
    return sep.join([word.capitalize() for word in s.split(sep)])


# Default Template groups. The ASCII spelling keeps Unicode case folding out of
# identifiers, just as CPython's inline (?a:...) scope does under IGNORECASE.
_template_pattern = re.compile(r"\$(?:(?P<escaped>\$)|(?P<named>[A-Za-z_][A-Za-z_0-9]*)|\{(?P<braced>[A-Za-z_][A-Za-z_0-9]*)\}|(?P<invalid>))")
# Consume plain text in the same forward scan as placeholders, so producing a
# literal gap never recounts all preceding Unicode code points. Template.pattern
# remains CPython's public four-group matcher, which skips these literal runs.
_template_tokens = re.compile(r"(?P<literal>[^$]+)|\$(?:(?P<escaped>\$)|(?P<named>[A-Za-z_][A-Za-z_0-9]*)|\{(?P<braced>[A-Za-z_][A-Za-z_0-9]*)\}|(?P<invalid>))")


class Template:
    template: str
    pattern = _template_pattern

    def __init__(self, template: str) -> None:
        self.template = template

    def is_valid(self) -> bool:
        for match in _template_pattern.finditer(self.template):
            if match.group("invalid") is not None:
                return False
        return True

    def get_identifiers(self) -> list[str]:
        ids: list[str] = []
        for match in _template_pattern.finditer(self.template):
            name = match.group("named") or match.group("braced")
            if name is not None and name not in ids:
                ids.append(name)
        return ids

    def _render(self, mapping: Mapping[str, object] | None, keywords: dict[str, object], safe: bool) -> str:
        source = self.template
        if not source:
            return source
        pieces: list[str] = []
        for match in _template_tokens.finditer(source):
            literal = match.group("literal")
            if literal is not None:
                pieces.append(literal)
                continue
            name = match.group("named") or match.group("braced")
            if name is not None:
                if name in keywords:
                    pieces.append(str(keywords[name]))
                elif mapping is None:
                    raise TypeError("'NoneType' object is not subscriptable")
                elif name in mapping:
                    pieces.append(str(mapping[name]))
                elif safe:
                    pieces.append(match.group(0))
                else:
                    raise KeyError(name)
            elif match.group("escaped") is not None:
                pieces.append("$")
            elif safe:
                pieces.append(match.group(0))
            else:
                # The prefix ends with the non-newline delimiter. splitlines()
                # therefore gives the final line and its exact code-point column.
                lines = source[:match.end()].splitlines()
                raise ValueError(f"Invalid placeholder in string: line {len(lines)}, col {len(lines[-1])}")
        return "".join(pieces)

    def substitute(self, mapping: Mapping[str, object] | None = {}, /, **kws: object) -> str:
        return self._render(mapping, kws, False)

    def safe_substitute(self, mapping: Mapping[str, object] | None = {}, /, **kws: object) -> str:
        return self._render(mapping, kws, True)
