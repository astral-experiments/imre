import unicodedata
from unicodedata import bidirectional

for value in ["A", "ש", "ش", "1", "\u2066", "\ue000", "\u0378", "\U0001ccfa"]:
    print(unicodedata.bidirectional(value))
for point in [0xd800, 0xdfff, 0x10ffff, 0x110000]:
    try:
        print(bidirectional(chr(point)))
    except ValueError as error:
        print(str(error))
