import os
import tempfile as files
from tempfile import gettempdir as directory, gettempdirb, gettempprefix, gettempprefixb

initial = directory()
assert isinstance(initial, str)
assert files.gettempdir() is initial
assert gettempdirb() == os.fsencode(initial)
assert gettempprefix() == "tmp"
assert files.gettempprefixb() == gettempprefixb() == b"tmp"

os.environ["TMPDIR"] = os.path.join(initial, "changed-after-first-lookup")
assert directory() is initial

files.tempdir = b"cached-\xff"
assert isinstance(directory(), str)
assert os.fsencode(directory()) == gettempdirb() == b"cached-\xff"
files.tempdir = files.template = "\u00e9"
for lookup in (files.gettempdirb, gettempprefixb):
    try:
        encoded = os.fsencode(files.tempdir)
    except UnicodeEncodeError:
        try:
            lookup()
        except UnicodeEncodeError:
            pass
        else:
            raise AssertionError("accepted an unencodable filesystem string")
    else:
        assert lookup() == encoded

print("tempfile accessors ok")
