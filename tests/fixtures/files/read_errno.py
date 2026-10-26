import os
from pathlib import Path


Path("source").write_bytes(b"one\ntwo\n")
for operation in range(5):
    if operation < 3:
        binary = open("source", "rb")
        os.close(binary.fileno())
        try:
            if operation == 0:
                print(binary.read(1))
            elif operation == 1:
                print(binary.readline())
            else:
                print(binary.read())
        except OSError as error:
            print((error.errno, error.strerror, repr(error.args)))
        try:
            binary.close()
        except OSError:
            pass
    else:
        text = open("source", "r", encoding="utf-8")
        os.close(text.fileno())
        try:
            print(text.readline() if operation == 3 else text.read())
        except OSError as error:
            print((error.errno, error.strerror, repr(error.args)))
        try:
            text.close()
        except OSError:
            pass
