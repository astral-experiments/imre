"""Locale categories, process locale selection, and number formatting."""

import sys
from functools import wraps as _wraps

from _locale import (
    CHAR_MAX, Error, LC_ALL, LC_COLLATE, LC_CTYPE, LC_MESSAGES, LC_MONETARY, LC_NUMERIC,
    LC_TIME, getencoding, localeconv as _localeconv, setlocale as _setlocale,
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


# Number formatting from CPython 3.14.5 Lib/locale.py (PSF-2.0).
# With this dict, you can override some items of localeconv's return value.
# This is useful for testing purposes.
_override_localeconv = {}

@_wraps(_localeconv)
def localeconv():
    d = _localeconv()
    if _override_localeconv:
        d.update(_override_localeconv)
    return d


### Number formatting APIs

# Author: Martin von Loewis
# improved by Georg Brandl

# Iterate over grouping intervals
def _grouping_intervals(grouping):
    last_interval = None
    for interval in grouping:
        # if grouping is -1, we are done
        if interval == CHAR_MAX:
            return
        # 0: re-use last group ad infinitum
        if interval == 0:
            if last_interval is None:
                raise ValueError("invalid grouping")
            while True:
                yield last_interval
        yield interval
        last_interval = interval

#perform the grouping from right to left
def _group(s, monetary=False):
    conv = localeconv()
    thousands_sep = conv[monetary and 'mon_thousands_sep' or 'thousands_sep']
    grouping = conv[monetary and 'mon_grouping' or 'grouping']
    if not grouping:
        return (s, 0)
    if s[-1] == ' ':
        stripped = s.rstrip()
        right_spaces = s[len(stripped):]
        s = stripped
    else:
        right_spaces = ''
    left_spaces = ''
    groups = []
    for interval in _grouping_intervals(grouping):
        if not s or s[-1] not in "0123456789":
            # only non-digit characters remain (sign, spaces)
            left_spaces = s
            s = ''
            break
        groups.append(s[-interval:])
        s = s[:-interval]
    if s:
        groups.append(s)
    groups.reverse()
    return (
        left_spaces + thousands_sep.join(groups) + right_spaces,
        len(thousands_sep) * (len(groups) - 1)
    )

# Strip a given amount of excess padding from the given string
def _strip_padding(s, amount):
    lpos = 0
    while amount and s[lpos] == ' ':
        lpos += 1
        amount -= 1
    rpos = len(s) - 1
    while amount and s[rpos] == ' ':
        rpos -= 1
        amount -= 1
    return s[lpos:rpos+1]

_percent_re = None

def _format(percent, value, grouping=False, monetary=False, *additional):
    if additional:
        formatted = percent % ((value,) + additional)
    else:
        formatted = percent % value
    if percent[-1] in 'eEfFgGdiu':
        formatted = _localize(formatted, grouping, monetary)
    return formatted

# Transform formatted as locale number according to the locale settings
def _localize(formatted, grouping=False, monetary=False):
    # floats and decimal ints need special action!
    if '.' in formatted:
        seps = 0
        parts = formatted.split('.')
        if grouping:
            parts[0], seps = _group(parts[0], monetary=monetary)
        decimal_point = localeconv()[monetary and 'mon_decimal_point'
                                              or 'decimal_point']
        formatted = decimal_point.join(parts)
        if seps:
            formatted = _strip_padding(formatted, seps)
    else:
        seps = 0
        if grouping:
            formatted, seps = _group(formatted, monetary=monetary)
        if seps:
            formatted = _strip_padding(formatted, seps)
    return formatted

def format_string(f, val, grouping=False, monetary=False):
    """Formats a string in the same way that the % formatting would use,
    but takes the current locale into account.

    Grouping is applied if the third parameter is true.
    Conversion uses monetary thousands separator and grouping strings if
    forth parameter monetary is true."""
    from _collections_abc import Mapping

    global _percent_re
    if _percent_re is None:
        import re

        _percent_re = re.compile(r'%(?:\((?P<key>.*?)\))?(?P<modifiers'
                                 r'>[-#0-9 +*.hlL]*?)[eEfFgGdiouxXcrs%]')

    percents = list(_percent_re.finditer(f))
    new_f = _percent_re.sub('%s', f)

    if isinstance(val, Mapping):
        new_val = []
        for perc in percents:
            if perc.group()[-1]=='%':
                new_val.append('%')
            else:
                new_val.append(_format(perc.group(), val, grouping, monetary))
    else:
        if not isinstance(val, tuple):
            val = (val,)
        new_val = []
        i = 0
        for perc in percents:
            if perc.group()[-1]=='%':
                new_val.append('%')
            else:
                starcount = perc.group('modifiers').count('*')
                new_val.append(_format(perc.group(),
                                      val[i],
                                      grouping,
                                      monetary,
                                      *val[i+1:i+1+starcount]))
                i += (1 + starcount)
    val = tuple(new_val)

    return new_f % val


def localize(string, grouping=False, monetary=False):
    """Parses a string as locale number according to the locale settings."""
    return _localize(string, grouping, monetary)
