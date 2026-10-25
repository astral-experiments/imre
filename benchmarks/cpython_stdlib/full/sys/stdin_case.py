"""Report the actual input code points and stream field ownership using ASCII output."""
import sys
from sys import stdin as stream


def consume() -> None:
    try:
        while True:
            line = input()
            checksum = 0
            for char in line:
                checksum = (checksum * 131 + ord(char)) % 2147483647
            print(str(len(line)) + ":" + str(checksum))
    except EOFError as error:
        print("EOFError: " + str(error))
    except UnicodeDecodeError as error:
        print("UnicodeDecodeError: " + str(error))
    except LookupError as error:
        print("LookupError: " + str(error))
    except NotImplementedError as error:
        print("NotImplementedError: " + str(error))


print(stream.encoding + "|" + sys.__stdin__.errors)
print(stream.encoding is sys.__stdin__.encoding)
print(stream.encoding is sys.stdout.encoding)
print(sys.stdout.encoding is sys.stderr.encoding)
print(stream.errors is sys.__stdin__.errors)
print(stream.errors is sys.stdout.errors)

consume()
consume()
