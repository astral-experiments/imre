import re
from re import escape


def current():
    print(re.escape is escape)
    print(escape(''.join(chr(i) for i in range(256))))
    print(escape(bytes(range(256))))
    print(escape(bytearray(b'hi.\xff')))
    print(escape('😊.$'))

    class Text(str):
        def translate(self, table):
            return table[ord('(')]

    print(escape(Text('unused')))
    try:
        escape(None)
    except TypeError:
        print('not a buffer')
