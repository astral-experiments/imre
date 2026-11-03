from base64 import a85decode, a85encode, b85decode, b85encode, z85decode, z85encode

data = bytearray(b"    \x00\x00\x00\x00abc")
view = memoryview(data)
framed = a85encode(view, foldspaces=True, wrapcol=5, adobe=True)
print(framed, a85decode(framed, foldspaces=True, adobe=True))
print(b85decode(b85encode(view[::2])))
print(z85decode(z85encode(view).decode("ascii")))
for decode, invalid in ((a85decode, b"!!!!z"), (b85decode, b"|NsC1"), (z85decode, b"0000;")):
    try:
        decode(invalid)
    except ValueError as exc:
        print(str(exc))
