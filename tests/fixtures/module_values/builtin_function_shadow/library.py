from builtins import print as output

print("library before")


def call_later() -> None:
    print("library after")


def print(label: str) -> None:
    output("library shadow " + label)
