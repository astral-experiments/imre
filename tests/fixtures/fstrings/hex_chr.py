"""Compare hexadecimal padding, signs, large integers, and chr's range checks."""

for number in [0, 15, 255, -1, -255, 1 << 100, -(1 << 100)]:
    print(f"{number:x}|{number:6X}|{number:04x}|{number:04X}")
print(f"{False:03x}|{True:4X}")
for code in [0, 127, 0xff, 0x100, 0xd7ff, 0xe000, 0x1d120, 0x10ffff]:
    print(repr(chr(code)))
print(repr(chr(False)))
print(repr(chr(True)))
for code in [-1, 0x110000, 1 << 100]:
    try:
        print(chr(code))
    except ValueError as error:
        print(str(error))
