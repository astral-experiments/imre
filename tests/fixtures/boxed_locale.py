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

        conv = _locale.localeconv()
        assert conv["decimal_point"] == "." and conv["grouping"] == []
        conv["grouping"].append(3)
        assert locale.localeconv()["grouping"] == []
        assert locale.localeconv()["frac_digits"] == _locale.CHAR_MAX
        assert int.__format__(1234567, "n") == "1234567"
        assert float.__format__(1234.5, ".6n") == "1234.5"

        previous = locale._override_localeconv
        locale._override_localeconv = {
            "grouping": [3, 2, 0], "thousands_sep": "_", "decimal_point": ",",
            "mon_grouping": [3, locale.CHAR_MAX], "mon_thousands_sep": " ", "mon_decimal_point": ":",
        }
        try:
            assert locale.format_string("%(value).1f%%", {"value": 1234567.5}, grouping=True) == "12_34_567,5%"
            assert locale.format_string("%.1f", 1234567.5, grouping=True, monetary=True) == "1234 567:5"
        finally:
            locale._override_localeconv = previous
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
