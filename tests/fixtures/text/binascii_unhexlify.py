from binascii import Error, a2b_hex, unhexlify

print((unhexlify(""), unhexlify("00aBff"), a2b_hex(b"00AbFF")))
for data in (b"z", b"0g", b"g0", b"aa bb ", b"\xff\xff"):
    try:
        print(a2b_hex(data))
    except Error as exc:
        print(str(exc))
try:
    unhexlify("é")
except Error:
    print("unexpected binascii.Error")
except ValueError as exc:
    print(str(exc))
try:
    unhexlify(17)
except TypeError as exc:
    print(str(exc))
try:
    a2b_hex(hexstr=b"00")
except TypeError as exc:
    print(str(exc))
try:
    unhexlify()
except TypeError as exc:
    print(str(exc))


for pairs in (15, 16, 17, 31, 32, 33):
    assert unhexlify(b"aB" * pairs) == b"\xab" * pairs
for index in (0, 1, 30, 31, 32, 33, 62, 63, 64, 65):
    for digit in (b"/", b":", b"@", b"G", b"`", b"g", b"\xff"):
        data = b"0" * index + digit + b"a" * (65 - index)
        try:
            unhexlify(data)
        except Error as exc:
            assert str(exc) == "Non-hexadecimal digit found"
        else:
            raise AssertionError("accepted an invalid hex digit")
