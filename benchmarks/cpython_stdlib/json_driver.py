import json
import sys


def inspect(text: str) -> None:
    try:
        rows = json.loads(text)
    except json.JSONDecodeError as error:
        print("JSONDecodeError")
        print(str(error))
    else:
        print("array-length")
        print(len(rows))


inspect(sys.argv[1])
