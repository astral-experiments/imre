import codecs
import io
import sys

sys.path.append("unused")
seen = []

def replace_bad_byte(error):
    seen.append((error.encoding, error.object, error.start, error.end))
    return "?", error.end

codecs.register_error("wrapper_bad_byte", replace_bad_byte)
raw = io.BytesIO(b"a\xffb\x80c")
text = io.TextIOWrapper(raw, encoding="utf-8", errors="wrapper_bad_byte")
assert text.read() == "a?b?c"
assert seen == [("utf-8", b"a\xffb\x80c", 1, 2), ("utf-8", b"a\xffb\x80c", 3, 4)]
assert text.detach() is raw
assert not raw.closed
print("registered UTF-8 callback through the original buffer")
