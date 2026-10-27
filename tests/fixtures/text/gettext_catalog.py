from base64 import b64decode
from gettext import bindtextdomain, c2py, dgettext, gettext as _, ngettext, npgettext, pgettext, textdomain
from pathlib import Path
from tempfile import TemporaryDirectory


# GNU MO files with three plural forms. The second language supplies a missing key.
FIRST = (
    b"3hIElQAAAAAHAAAAHAAAAFQAAAAAAAAAAAAAAAAAAACMAAAADAAAAI0AAAAQAAAAmgAAAAkA"
    b"AACrAAAABAAAALUAAAAIAAAAugAAAAQAAADDAAAAYgAAAMgAAAAPAAAAKwEAAB0AAAA7AQAA"
    b"CwAAAFkBAAAHAAAAZQEAABIAAABtAQAABgAAAIABAAAAY29udGV4dARmaWxlAGNvbnRleHQE"
    b"b25lAG1hbnkAZGlyZWN0b3J5AGZpbGUAb25lAG1hbnkAcGF0aABDb250ZW50LVR5cGU6IHRl"
    b"eHQvcGxhaW47IGNoYXJzZXQ9VVRGLTgKUGx1cmFsLUZvcm1zOiBucGx1cmFscz0zOyBwbHVy"
    b"YWw9bj09MSA/IDAgOiBuPT0yID8gMSA6IDI7CgBjb250ZXh0dWFsIGZpbGUAY29udGV4dCAx"
    b"AGNvbnRleHQgMgBjb250ZXh0IDMAcsOpcGVydG9pcmUAZmljaGllcgBmaXJzdABzZWNvbmQA"
    b"dGhpcmQAY2hlbWluAA=="
)
SECOND = (
    b"3hIElQAAAAADAAAAHAAAADQAAAAAAAAAAAAAAAAAAABMAAAABAAAAE0AAAAQAAAAUgAAAGIA"
    b"AABjAAAADgAAAMYAAAAOAAAA1QAAAABmaWxlAG1pc3NpbmctaW4tZmlyc3QAQ29udGVudC1U"
    b"eXBlOiB0ZXh0L3BsYWluOyBjaGFyc2V0PVVURi04ClBsdXJhbC1Gb3JtczogbnBsdXJhbHM9"
    b"MzsgcGx1cmFsPW49PTEgPyAwIDogbj09MiA/IDEgOiAyOwoAc2Vjb25kYXJ5IGZpbGUAc2Vj"
    b"b25kIGNhdGFsb2cA"
)


with TemporaryDirectory() as directory:
    for language, content in (("fr_FR", FIRST), ("yy", SECOND)):
        path = Path(directory) / language / "LC_MESSAGES"
        path.mkdir(parents=True)
        (path / "click.mo").write_bytes(b64decode(content))
    bindtextdomain("click", directory)
    textdomain("click")
    print(_("file"))
    print(_("path"))
    print(_("missing-in-first"))
    print(_("unknown"))
    print(pgettext("context", "file"))
    for n in (0, 1, 2, -1, 1 << 100):
        print(ngettext("one", "many", n))
        print(npgettext("context", "one", "many", n))
    print(dgettext("unbound-domain", "file"))
    textdomain("unbound-domain")
    print(_("file"))
    print(ngettext("one", "many", 1))
    print(ngettext("one", "many", 2))

for expression in ("1 || 2 % 0", "0 && 2 % 0", "0 ? 1 / 0 : 7", "1 / n", "!n == 1", "(n > 1", "0xa"):
    try:
        print(c2py(expression)(0))
    except ValueError as error:
        print(str(error))
    except ZeroDivisionError as error:
        print(str(error))
