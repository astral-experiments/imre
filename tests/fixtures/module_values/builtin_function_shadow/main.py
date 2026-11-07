from builtins import print as output

import library

print("entry before")


def print(label: str) -> None:
    output("entry shadow " + label)


library.call_later()
print("entry after")
