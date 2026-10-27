"""Translate messages with GNU MO catalogs selected by domain and language."""

from _imre_gettext_locale import normalize as _normalize_locale
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
    base, at, modifier = _normalize_locale(language).partition("@")
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
            if name == "C":
                return paths
            filename = os.path.join(directory, name, "LC_MESSAGES", domain + ".mo")
            if filename not in paths and os.path.exists(filename):
                paths.append(filename)
    return paths


_PRECEDENCE = {"?": 1, ":": 1, "||": 2, "&&": 3, "!": 4, "==": 5, "!=": 5,
               "<": 6, "<=": 6, ">": 6, ">=": 6, "+": 7, "-": 7,
               "*": 8, "/": 8, "%": 8}


def _tokens(expression: str) -> list[str]:
    words: list[str] = []
    i = 0
    while i < len(expression):
        start = i
        token = expression[i]
        i += 1
        if token in (" ", "\t"):
            continue
        if token.isalnum() or token == "_":
            while i < len(expression) and (expression[i].isalnum() or expression[i] == "_"):
                i += 1
            token = expression[start:i]
            if token != "n" and token.strip("0123456789"):
                raise ValueError("invalid token in plural form: " + token)
        elif expression[start:i + 1] in ("!=", "==", "<=", ">=", "&&", "||"):
            i += 1
            token = expression[start:i]
        elif token not in _PRECEDENCE and token not in ("(", ")"):
            raise ValueError("invalid token in plural form: " + token)
        words.append(token)
    return words


def _plural_program(expression: str) -> list[tuple[str, int, int, int]]:
    """Compile catalog plural forms to an expression tree without executing code."""
    if len(expression) > 1000:
        raise ValueError("plural form expression is too long")
    result: list[str] = []
    operators: list[str] = []
    operand = True
    depth = 0
    conditions = 0
    for token in _tokens(expression):
        if token == "n" or token[0] in "0123456789":
            if not operand:
                raise ValueError("unexpected token in plural form: " + token)
            result.append(token)
            operand = False
        elif token == "(":
            if not operand:
                raise ValueError("unexpected token in plural form: (")
            depth += 1
            if depth > 20 or depth + conditions > 21:
                raise ValueError("plural form expression is too complex")
            operators.append(token)
        elif token == ")":
            if operand or depth == 0:
                raise ValueError("unexpected token in plural form: )")
            while operators and operators[-1] != "(":
                if operators[-1] == "?":
                    raise ValueError("unexpected token in plural form: )")
                result.append(operators.pop())
            operators.pop()
            depth -= 1
        else:
            if operand != (token == "!"):
                raise ValueError("unexpected token in plural form: " + token)
            if token == "!" and operators and _PRECEDENCE.get(operators[-1], 0) > _PRECEDENCE["!"]:
                raise ValueError("unparenthesized negation after " + operators[-1] + " in plural form")
            if token == ":":
                while operators and operators[-1] not in ("?", "("):
                    result.append(operators.pop())
                if not operators or operators.pop() != "?":
                    raise ValueError("unexpected token in plural form: :")
                conditions -= 1
            elif token != "!":
                while operators and operators[-1] not in ("?", "("):
                    previous = _PRECEDENCE[operators[-1]]
                    rank = _PRECEDENCE[token]
                    if previous < rank or (previous == rank and token == "?"):
                        break
                    result.append(operators.pop())
                if token == "?":
                    conditions += 1
                    if depth + conditions > 21:
                        raise ValueError("plural form expression is too complex")
            operators.append(token)
            operand = True
    if operand:
        raise ValueError("unexpected end of plural form")
    while operators:
        token = operators.pop()
        if token == "(":
            raise ValueError("unbalanced parenthesis in plural form")
        if token == "?":
            raise ValueError("unexpected end of plural form")
        result.append(token)

    tree: list[tuple[str, int, int, int]] = []
    roots: list[int] = []
    for token in result:
        left, right, other = -1, -1, -1
        if token == "!":
            left = roots.pop()
        elif token in _PRECEDENCE:
            if token == ":":
                other = roots.pop()
            right = roots.pop()
            left = roots.pop()
        roots.append(len(tree))
        tree.append((token, left, right, other))
    return tree


def _binary(op: str, left: int, right: int) -> int:
    if op == "+":
        return left + right
    if op == "-":
        return left - right
    if op == "*":
        return left * right
    if op == "/":
        return left // right
    if op == "%":
        return left % right
    if op == "==":
        return int(left == right)
    if op == "!=":
        return int(left != right)
    if op == "<":
        return int(left < right)
    if op == "<=":
        return int(left <= right)
    if op == ">":
        return int(left > right)
    return int(left >= right)


def _plural_index(tree: list[tuple[str, int, int, int]], n: int) -> int:
    # Evaluate only the selected side of logical and conditional operators.
    # Each frame resumes after its first or second operand is available.
    pending = [(len(tree) - 1, 0)]
    values: list[int] = []
    while pending:
        index, step = pending.pop()
        token, left, right, other = tree[index]
        if left < 0:
            values.append(n if token == "n" else int(token))
        elif step == 0:
            pending.append((index, 1))
            pending.append((left, 0))
        elif step == 2:
            rhs = values.pop()
            lhs = values.pop()
            values.append(_binary(token, lhs, rhs))
        elif token == "!":
            values[-1] = int(not values[-1])
        elif token == ":":
            pending.append((right if values.pop() else other, 0))
        elif token == "&&" or token == "||":
            if bool(values[-1]) == (token == "&&"):
                values.pop()
                pending.append((right, 0))
        else:
            pending.append((index, 2))
            pending.append((right, 0))
    return values[0]


class _PluralRule:
    program: list[tuple[str, int, int, int]]

    def __init__(self, expression: str) -> None:
        self.program = _plural_program(expression)

    def select(self, n: int) -> int:
        return _plural_index(self.program, n)


def c2py(plural: str):
    return _PluralRule(plural).select


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
    program: list[tuple[str, int, int, int]]

    def __init__(self) -> None:
        self.singles = {}
        self.plurals = {}
        self.program = _plural_program("n != 1")

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
