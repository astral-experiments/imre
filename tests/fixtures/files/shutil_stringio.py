from io import BytesIO, StringIO
from shutil import copyfileobj


for length in [1, 2, 0, -1]:
    source = StringIO("0é🐍\r\n尾")
    target = StringIO("HEAD-old")
    source.seek(1)
    target.seek(5)
    print(copyfileobj(source, target, length))
    print(source.tell(), target.tell(), repr(target.getvalue()))
    print(source.closed, target.closed)

byte_source = BytesIO(b"x")
target = StringIO()
try:
    copyfileobj(byte_source, target, 1)
except TypeError as error:
    print(str(error), byte_source.tell(), repr(target.getvalue()))
target.close()
byte_source.seek(0)
try:
    copyfileobj(byte_source, target, 1)
except TypeError as error:
    print(str(error), byte_source.tell())

text = StringIO("x")
binary = BytesIO()
try:
    copyfileobj(text, binary, 1)
except TypeError as error:
    print(str(error), text.tell(), binary.getvalue())
text.close()
target = StringIO()
try:
    copyfileobj(text, target, 1)
except ValueError as error:
    print(str(error))
try:
    copyfileobj(text, target, 1 << 100)
except OverflowError as error:
    print(str(error))
