"""Line protocol for exercising public MIME operations under the original host callers."""
import mimetypes
import sys
from io import StringIO

if sys.argv[1] == "builtins":
    mimetypes.knownfiles = []
    mimetypes.init([])
db = mimetypes.MimeTypes()
held_extensions: list[str] = []

while True:
    op = input()
    if op == "quit":
        break
    arg = input()
    strict = input() == "1"
    if op == "url":
        print(mimetypes.guess_type(arg, strict))
    elif op == "file":
        print(mimetypes.guess_file_type(arg, strict=strict))
    elif op == "db-url":
        print(db.guess_type(arg, strict))
    elif op == "db-file":
        print(db.guess_file_type(arg, strict=strict))
    elif op == "db-extension":
        print(repr(db.guess_extension(arg, strict)))
    elif op == "extension":
        print(repr(mimetypes.guess_extension(arg, strict)))
    elif op == "db-extensions":
        held_extensions = db.guess_all_extensions(arg, strict)
        print(held_extensions)
    elif op == "append-held":
        held_extensions.append(arg)
        print(held_extensions)
    elif op == "types-map":
        print(repr(mimetypes.types_map))
    elif op == "db-add":
        db.add_type(arg, input(), strict)
        print("None")
    elif op == "add":
        mimetypes.add_type(arg, input(), strict)
        print("None")
    elif op == "db-new":
        db = mimetypes.MimeTypes([arg], strict)
        print("None")
    elif op == "db-readfp":
        with open(arg, encoding="utf-8") as source:
            db.readfp(StringIO(source.read()), strict)
        print("None")
    elif op == "init":
        mimetypes.init()
        print("None")
    else:
        raise ValueError("unknown MIME operation")
    sys.stdout.flush()
