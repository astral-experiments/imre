"""Locale categories and process locale selection."""

from _locale import (
    Error, LC_ALL, LC_COLLATE, LC_CTYPE, LC_MESSAGES, LC_MONETARY, LC_NUMERIC,
    LC_TIME, getencoding, setlocale as _setlocale,
)
from _imre_gettext_locale import normalize


def setlocale(category, locale=None):
    """Query or change a category using a locale name or a language/encoding pair."""
    if locale and not isinstance(locale, str):
        try:
            language, encoding = locale
            if language is None:
                language = "C"
            name = language if encoding is None else language + "." + encoding
        except (TypeError, ValueError):
            raise TypeError("Locale must be None, a string, or an iterable of "
                            "two strings -- language code, encoding.") from None
        locale = normalize(name)
    return _setlocale(category, locale)
