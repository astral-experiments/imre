"""Locale categories and process locale selection."""

import sys

from _locale import (
    Error, LC_ALL, LC_COLLATE, LC_CTYPE, LC_MESSAGES, LC_MONETARY, LC_NUMERIC,
    LC_TIME, getencoding, setlocale as _setlocale,
)
from _imre_gettext_locale import normalize

try:
    from _locale import (
        nl_langinfo, DAY_1, DAY_2, DAY_3, DAY_4, DAY_5, DAY_6, DAY_7, ABDAY_1,
        ABDAY_2, ABDAY_3, ABDAY_4, ABDAY_5, ABDAY_6, ABDAY_7, MON_1, MON_2, MON_3,
        MON_4, MON_5, MON_6, MON_7, MON_8, MON_9, MON_10, MON_11, MON_12, ABMON_1,
        ABMON_2, ABMON_3, ABMON_4, ABMON_5, ABMON_6, ABMON_7, ABMON_8, ABMON_9,
        ABMON_10, ABMON_11, ABMON_12, D_T_FMT, D_FMT, T_FMT, AM_STR, PM_STR,
        T_FMT_AMPM, ERA, ERA_D_FMT, ERA_D_T_FMT, ERA_T_FMT, ALT_DIGITS, CODESET,
        RADIXCHAR, THOUSEP, CRNCYSTR, YESEXPR, NOEXPR,
    )
except ImportError:
    pass


def _parse_localename(localename):

    """ Parses the locale code for localename and returns the
        result as tuple (language code, encoding).

        The localename is normalized and passed through the locale
        alias engine. A ValueError is raised in case the locale name
        cannot be parsed.

        The language code corresponds to RFC 1766.  code and encoding
        can be None in case the values cannot be determined or are
        unknown to this implementation.

    """
    code = normalize(localename)
    if '@' in code:
        # Deal with locale modifiers
        code, modifier = code.split('@', 1)
        if modifier == 'euro' and '.' not in code:
            # Assume Latin-9 for @euro locales. This is bogus,
            # since some systems may use other encodings for these
            # locales. Also, we ignore other modifiers.
            return code, 'iso-8859-15'

    if '.' in code:
        return tuple(code.split('.')[:2])
    elif code == 'C':
        return None, None
    elif code == 'UTF-8':
        # On macOS "LC_CTYPE=UTF-8" is a valid locale setting
        # for getting UTF-8 handling for text.
        return None, 'UTF-8'
    raise ValueError('unknown locale: %s' % localename)


def getlocale(category=LC_CTYPE):

    """ Returns the current setting for the given locale category as
        tuple (language code, encoding).

        category may be one of the LC_* value except LC_ALL. It
        defaults to LC_CTYPE.

        Except for the code 'C', the language code corresponds to RFC
        1766.  code and encoding can be None in case the values cannot
        be determined.

    """
    localename = _setlocale(category)
    if category == LC_ALL and ';' in localename:
        raise TypeError('category LC_ALL is not supported')
    return _parse_localename(localename)


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


def getpreferredencoding(do_setlocale=True):
    """Return the charset that the user is likely using."""
    if sys.flags.warn_default_encoding:
        import warnings
        warnings.warn(
            "UTF-8 Mode affects locale.getpreferredencoding(). Consider locale.getencoding() instead.",
            EncodingWarning, 2)
    if sys.flags.utf8_mode:
        return "utf-8"

    if not do_setlocale:
        return getencoding()

    old_loc = setlocale(LC_CTYPE)
    try:
        try:
            setlocale(LC_CTYPE, "")
        except Error:
            pass
        return getencoding()
    finally:
        setlocale(LC_CTYPE, old_loc)
