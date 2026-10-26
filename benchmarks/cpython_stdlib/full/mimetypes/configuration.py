"""Run once per process with a custom known file and CPython's UTF-8 test fixture."""
import mimetypes
import sys
from io import StringIO
from pathlib import Path

custom = sys.argv[1]
original = sys.argv[2]
mimetypes.knownfiles = [custom]
before = mimetypes.types_map
print(mimetypes.inited)
# On the first init, even an explicit empty list is appended to knownfiles.
mimetypes.init([])
print(mimetypes.inited)
print(before is mimetypes.types_map)
print(mimetypes.guess_type("some.imre-custom"))
print(mimetypes.guess_type("some.imre-common", False))
db = mimetypes.MimeTypes()
print(db.guess_file_type("some.imre-custom"))
db.readfp(StringIO("application/local one two # comment\n"))
print(db.guess_type("another.ONE"))
print(db.guess_all_extensions("application/local"))
table = "text/stream-first cr-first\rtext/stream-second cr-second"
db.readfp(StringIO(table, newline="\n"))
assert db.guess_file_type("a.cr-second")[0] == "text/stream-first"
db.readfp(StringIO(table, newline=""))
assert db.guess_file_type("a.cr-second")[0] == "text/stream-second"
db.add_type("text/local", ".two", False)
print(db.guess_type("name.two", False))
db.add_type("text/local", ".three", False)
print(db.guess_type("name.three", True))
print(db.guess_type("name.three", False))
print(db.guess_extension("TEXT/LOCAL", False))
db.add_type("application/default", "")
print(db.guess_file_type(""))
db.suffix_map[".alias"] = ".tar.Z"
print(db.guess_type("name.ALIAS"))
db.encodings_map[".cv"] = "custom"
print(db.guess_type("name.html.cv"))
print(db.guess_type("name.html.CV"))
print(mimetypes.guess_type("name.html.cv"))
mimetypes.add_type("text/changed", ".imre-custom")
held = mimetypes.types_map
mimetypes.init([])
print(held is mimetypes.types_map)
print(mimetypes.guess_file_type("foo.imre-custom"))
mimetypes.init([original])
print(mimetypes.guess_all_extensions("application/vnd.geocube+xml"))
mimetypes.init()
print(held is mimetypes.types_map)
print(held[".imre-custom"])
print(mimetypes.types_map[".imre-custom"])
mimetypes.types_map[".edited"] = "application/edited"
print(mimetypes.guess_type("name.edited"))
print(db.guess_type("name.edited"))
found = mimetypes.read_mime_types(custom)
if found is not None:
    print(found[".imre-custom"])
print(mimetypes.read_mime_types(custom + ".absent") is None)
print(mimetypes.guess_file_type(Path("folder/name.TGZ")))
print(mimetypes.guess_type(Path("folder/name.TGZ")))
print(mimetypes.guess_file_type(b"folder/name.tar.Z"))
print(mimetypes.guess_type(b"https://example.org/name.txt"))
