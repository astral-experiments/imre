import os
import sys


if sys.argv[1] == "cwd":
    try:
        print(os.getcwd())
    except FileNotFoundError as error:
        print(str(error))
elif sys.argv[1] == "bytes":
    data: bytes = os.fspath(sys.argv[2].encode())
    print(data.decode())
else:
    text: str = os.fspath(sys.argv[2])
    print(text)
