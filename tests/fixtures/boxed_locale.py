import _locale
import io
import locale


def current():
    for name in ("LC_ALL", "LC_COLLATE", "LC_CTYPE", "LC_MESSAGES", "LC_MONETARY",
                 "LC_NUMERIC", "LC_TIME"):
        assert getattr(locale, name) == getattr(_locale, name)
    assert locale.Error is _locale.Error
    original = locale.setlocale(locale.LC_ALL)
    try:
        assert locale.setlocale(category=locale.LC_ALL, locale=(None, None)) == "C"
        assert _locale.setlocale(locale.LC_CTYPE, None) == "C"
        assert locale.getlocale() == (None, None)
        assert locale.getlocale(category=locale.LC_ALL) == (None, None)
        try:
            locale.setlocale(locale.LC_CTYPE, "C.UTF-8")
        except locale.Error:
            pass
        else:
            assert locale.getlocale() == ("C", "UTF-8")
            assert locale.getlocale(locale.LC_NUMERIC) == (None, None)
            if ";" in locale.setlocale(locale.LC_ALL):
                try:
                    locale.getlocale(locale.LC_ALL)
                except TypeError as error:
                    assert str(error) == "category LC_ALL is not supported"
                else:
                    raise AssertionError("accepted mixed LC_ALL")
            locale.setlocale(locale.LC_CTYPE, "C")
        encoding = locale.getencoding()
        assert _locale.getencoding() == encoding
        with io.TextIOWrapper(io.BytesIO(), encoding="locale") as stream:
            assert stream.encoding == encoding
        for category, name, expected in (
            (locale.LC_ALL, "imre_missing_locale_12345678", "unsupported locale setting"),
            (-1, None, "locale query failed"),
        ):
            try:
                locale.setlocale(category, name)
            except locale.Error as error:
                assert str(error) == expected
            else:
                raise AssertionError("invalid locale accepted")
        assert locale.setlocale(locale.LC_ALL) == "C"
        try:
            _locale.setlocale(locale.LC_CTYPE, "C\0")
        except ValueError:
            pass
        else:
            raise AssertionError("embedded NUL accepted")
        assert locale.setlocale(locale.LC_ALL, "") == _locale.setlocale(locale.LC_ALL)
    finally:
        _locale.setlocale(locale.LC_ALL, original)
    assert locale.setlocale(locale.LC_ALL) == original

if hasattr(locale, "nl_langinfo"):
    import _locale
    class Item:
        def __index__(self):
            return locale.DAY_1
    print(locale.nl_langinfo is _locale.nl_langinfo)
    print(locale.nl_langinfo(Item()) == locale.nl_langinfo(locale.DAY_1))
    for key in [-1, 2**40, None]:
        try:
            locale.nl_langinfo(key)
        except (TypeError, ValueError, OverflowError) as exc:
            print(type(exc).__name__, str(exc))
