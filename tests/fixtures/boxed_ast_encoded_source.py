import ast
from array import array


class Source(bytes):
    pass


source = b"# coding: latin-1\r\nvalue = '\xe9'\r\n"
for data in (source, Source(source), bytearray(source), memoryview(source), array("B", source)):
    value = ast.parse(data, filename="encoded.py").body[0].value
    assert value.value == "é"
    assert (value.lineno, value.col_offset, value.end_lineno, value.end_col_offset) == (2, 8, 2, 12)

value = ast.parse(bytearray(b"\xef\xbb\xbf'\xc3\xa9'"), mode="eval").body
assert value.value == "é" and (value.col_offset, value.end_col_offset) == (0, 4)

try:
    ast.parse(b"\xef\xbb\xbf# coding: latin-1\nx", filename="encoded.py")
except SyntaxError as exc:
    assert exc.args == ("encoding problem: iso-8859-1 with BOM", (None, 1, 0, "# coding: latin-1", 1, 17))
    assert (exc.filename, exc.lineno, exc.offset, exc.end_lineno, exc.end_offset) == ("encoded.py", 1, 0, 1, 17)
else:
    raise AssertionError("accepted a conflicting encoding cookie")

try:
    ast.parse(b"# coding: nonexistent-encoding\nx")
except SyntaxError as exc:
    assert exc.args == ("unknown encoding: nonexistent-encoding", ("<unknown>", 0, -1, None))
    assert exc.filename == "<unknown>"
else:
    raise AssertionError("accepted an unknown source encoding")

try:
    ast.parse(b"x\x00")
except SyntaxError as exc:
    assert exc.args == ("source code string cannot contain null bytes",)
    assert exc.filename is None
else:
    raise AssertionError("accepted a null byte")

try:
    ast.parse(memoryview(b"1x2")[::2])
except TypeError as exc:
    assert exc.args == ("compile() arg 1 must be a string, bytes or AST object",)
else:
    raise AssertionError("accepted a noncontiguous source buffer")


try:
    ast.parse(b"\xff", "encoded.py", "bad")
except ValueError as exc:
    assert exc.args == ("compile() mode must be 'exec', 'eval', 'single' or 'func_type'",)
else:
    raise AssertionError("invalid mode must be rejected before decoding")

try:
    ast.parse(b"1\n\xff", "encoded.py")
except SyntaxError as exc:
    assert (exc.filename, exc.lineno, exc.offset, exc.end_lineno, exc.end_offset) == ("encoded.py", 2, 1, 2, 1)
    assert exc.args[1] == (None, 2, 1, "�", 2, 1)
else:
    raise AssertionError("accepted invalid UTF-8 source")
