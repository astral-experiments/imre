"""ASCII character sets and word capitalization."""

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
