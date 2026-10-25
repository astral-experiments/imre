"""Return the actual exception message from each native path constructor call."""
from pathlib import Path
import sys


def arg(i: int) -> bytes:
    with open(sys.argv[1] + '/' + str(i), 'rb') as stream:
        return stream.read()


print('READY')
while True:
    command = input()
    if command == 'quit':
        break
    try:
        if command == 'b':
            path = Path(arg(0))
        elif command == 'bs':
            path = Path(arg(0), arg(1).decode())
        elif command == 'sb':
            path = Path(arg(0).decode(), arg(1))
        else:
            raise AssertionError('unknown constructor signature')
        print(repr(str(path)))
    except TypeError as error:
        print('!TypeError ' + repr(str(error)))
