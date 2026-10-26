"""Translate messages with GNU MO catalogs selected by domain and language."""

import codecs
import os
import sys


def _default_directory() -> str:
    prefix = os.path.dirname(sys.executable)
    if prefix.endswith(os.sep + "bin"):
        prefix = os.path.dirname(prefix)
    return os.path.join(prefix, "share", "locale")


def _expand_lang(language: str) -> list[str]:
    # GNU gettext tries the most specific combination first, then each subset.
    base, at, modifier = language.partition("@")
    base, dot, encoding = base.partition(".")
    base, underscore, territory = base.partition("_")
    components = [dot + encoding, underscore + territory, at + modifier]
    result: list[str] = []
    for mask in range(7, -1, -1):
        name = base
        if mask & 2:
            name += components[1]
        if mask & 1:
            name += components[0]
        if mask & 4:
            name += components[2]
        if name not in result:
            result.append(name)
    return result


def _languages() -> list[str]:
    for key in ("LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"):
        value = os.getenv(key, "")
        if value:
            return value.split(":")
    return []


def _paths(domain: str, directory: str, languages: list[str]) -> list[str]:
    paths: list[str] = []
    for language in languages:
        for name in _expand_lang(language):
            if name == "C" or name == "POSIX":
                return paths
            filename = os.path.join(directory, name, "LC_MESSAGES", domain + ".mo")
            if filename not in paths and os.path.exists(filename):
                paths.append(filename)
    return paths


_PRECEDENCE = {"?": 1, ":": 1, "||": 2, "&&": 3, "==": 4, "!=": 4,
               "<": 5, "<=": 5, ">": 5, ">=": 5, "+": 6, "-": 6,
               "*": 7, "/": 7, "%": 7, "!": 8}


def _plural_program(expression: str) -> list[str]:
    """Compile the restricted C plural expression to postfix operators."""
    if len(expression) > 1000:
        raise ValueError("plural form expression is too long")
    result: list[str] = []
    operators: list[str] = []
    operand = True
    depth = 0
    i = 0
    while i < len(expression):
        token = expression[i]
        i += 1
        if token.isspace():
            continue
        if token == "n" or token in "0123456789":
            if not operand:
                raise ValueError("invalid plural form")
            if token != "n":
                start = i - 1
                while i < len(expression) and expression[i] in "0123456789":
                    i += 1
                token = expression[start:i]
            result.append(token)
            operand = False
        elif token == "(":
            if not operand:
                raise ValueError("invalid plural form")
            depth += 1
            if depth > 20:
                raise ValueError("plural form expression is too complex")
            operators.append(token)
        elif token == ")":
            if operand or depth == 0:
                raise ValueError("unbalanced parenthesis in plural form")
            while operators and operators[-1] != "(":
                result.append(operators.pop())
            operators.pop()
            depth -= 1
        else:
            pair = expression[i - 1:i + 1]
            if pair in ("!=", "==", "<=", ">=", "&&", "||"):
                token = pair
                i += 1
            rank = _PRECEDENCE.get(token, 0)
            if not rank or operand != (token == "!"):
                raise ValueError("invalid plural form")
            if token == ":":
                while operators and operators[-1] != "?" and operators[-1] != "(":
                    result.append(operators.pop())
                if not operators or operators.pop() != "?":
                    raise ValueError("invalid plural form")
            else:
                while operators and operators[-1] not in ("?", "("):
                    previous = _PRECEDENCE[operators[-1]]
                    if previous < rank or (previous == rank and token in ("?", "!")):
                        break
                    result.append(operators.pop())
            operators.append(token)
            operand = True
    if operand or depth:
        raise ValueError("invalid plural form")
    while operators:
        result.append(operators.pop())
    if "?" in result:
        raise ValueError("invalid plural form")
    return result


def _binary(op: str, left: int, right: int) -> int:
    if op == "+": return left + right
    if op == "-": return left - right
    if op == "*": return left * right
    if op == "/": return left // right
    if op == "%": return left % right
    if op == "==": return int(left == right)
    if op == "!=": return int(left != right)
    if op == "<": return int(left < right)
    if op == "<=": return int(left <= right)
    if op == ">": return int(left > right)
    return int(left >= right)


def _plural_index(program: list[str], number: int) -> int:
    # Keep arithmetic errors with their operand until a short-circuit operator
    # selects it. An unselected ternary or logical branch cannot raise.
    values: list[tuple[int, str]] = []
    for token in program:
        if token == "n":
            values.append((number, ""))
        elif token[0] in "0123456789":
            values.append((int(token), ""))
        elif token == "!":
            value, error = values.pop()
            values.append((int(not value), error))
        else:
            right = values.pop()
            left = values.pop()
            if token == ":":
                condition = values.pop()
                if condition[1]: values.append(condition)
                elif condition[0]: values.append(left)
                else: values.append(right)
            elif left[1] or (token == "&&" and not left[0]) or (token == "||" and left[0]):
                values.append(left)
            elif right[1] or token == "&&" or token == "||":
                values.append(right)
            elif right[0] == 0 and token in ("%", "/"):
                values.append((0, "integer modulo by zero" if token == "%" else "integer division or modulo by zero"))
            else:
                values.append((_binary(token, left[0], right[0]), ""))
    value, error = values[0]
    if error:
        raise ZeroDivisionError(error)
    return value


def _word(data: bytes, offset: int, big: bool) -> int:
    if offset + 4 > len(data):
        raise OSError("File is corrupt")
    if big:
        return (data[offset] << 24) | (data[offset + 1] << 16) | (data[offset + 2] << 8) | data[offset + 3]
    return data[offset] | (data[offset + 1] << 8) | (data[offset + 2] << 16) | (data[offset + 3] << 24)


def _entry(data: bytes, table: int, index: int, big: bool) -> bytes:
    length = _word(data, table + 8 * index, big)
    start = _word(data, table + 8 * index + 4, big)
    if start + length >= len(data):
        raise OSError("File is corrupt")
    return data[start:start + length]


class _Catalog:
    singles: dict[str, str]
    plurals: dict[str, list[str]]
    program: list[str]

    def __init__(self) -> None:
        self.singles = {}
        self.plurals = {}
        self.program = ["n", "1", "!="]

    def load(self, data: bytes) -> None:
        magic = _word(data, 0, False)
        big = magic == 0xde120495
        if not big and magic != 0x950412de:
            raise OSError("Bad magic number")
        if _word(data, 4, big) >> 16 > 1:
            raise OSError("Bad version number")
        count = _word(data, 8, big)
        originals = _word(data, 12, big)
        translations = _word(data, 16, big)
        encoding = "ascii"
        for i in range(count):
            original = _entry(data, originals, i, big)
            translated = _entry(data, translations, i, big)
            if not original:
                for line in translated.decode().split("\n"):
                    key, sep, value = line.partition(":")
                    if sep and key.strip().lower() == "content-type":
                        encoding = value.split("charset=")[1].strip()
                    if sep and key.strip().lower() == "plural-forms":
                        rule = value.split(";")[1].split("plural=")[1]
                        self.program = _plural_program(rule)
            codec = codecs.lookup(encoding)
            message = codec.decode(original)[0]
            text = codec.decode(translated)[0]
            if "\x00" in message:
                self.plurals[message.split("\x00")[0]] = text.split("\x00")
            else:
                self.singles[message] = text


_localedirs: dict[str, str] = {}
_current_domain = ["messages"]
_translations: dict[str, _Catalog] = {}


def textdomain(domain: str | None = None) -> str:
    if domain is not None:
        _current_domain[0] = domain
    return _current_domain[0]


def bindtextdomain(domain: str, localedir: str | None = None) -> str:
    if localedir is not None:
        _localedirs[domain] = localedir
    directory = _localedirs.get(domain)
    if directory is not None:
        return directory
    return _default_directory()


def _lookup(domain: str, context: str | None, message: str, plural: str | None, n: int) -> str:
    key = message if context is None else context + "\x04" + message
    paths = _paths(domain, bindtextdomain(domain), _languages())
    try:
        catalogs: list[_Catalog] = []
        for path in paths:
            absolute = os.path.abspath(path)
            catalog = _translations.get(absolute)
            if catalog is None:
                with open(path, "rb") as stream:
                    catalog = _Catalog()
                    catalog.load(stream.read())
                _translations[absolute] = catalog
            catalogs.append(catalog)
        for catalog in catalogs:
            if plural is None and key in catalog.singles:
                return catalog.singles[key]
            forms = catalog.plurals.get(key)
            if forms is not None:
                index = _plural_index(catalog.program, n)
                if 0 <= index < len(forms):
                    return forms[index]
    except OSError:
        pass
    if plural is None or n == 1:
        return message
    return plural


def dgettext(domain: str, message: str) -> str:
    return _lookup(domain, None, message, None, 1)


def dngettext(domain: str, msgid1: str, msgid2: str, n: int) -> str:
    return _lookup(domain, None, msgid1, msgid2, n)


def dpgettext(domain: str, context: str, message: str) -> str:
    return _lookup(domain, context, message, None, 1)


def dnpgettext(domain: str, context: str, msgid1: str, msgid2: str, n: int) -> str:
    return _lookup(domain, context, msgid1, msgid2, n)


def gettext(message: str) -> str:
    return dgettext(textdomain(), message)


def ngettext(msgid1: str, msgid2: str, n: int) -> str:
    return dngettext(textdomain(), msgid1, msgid2, n)


def pgettext(context: str, message: str) -> str:
    return dpgettext(textdomain(), context, message)


def npgettext(context: str, msgid1: str, msgid2: str, n: int) -> str:
    return dnpgettext(textdomain(), context, msgid1, msgid2, n)
