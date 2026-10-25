from enum import Enum


class Mode(Enum):
    check = "lint"
    alias = check
    empty = ""


def display(mode: Mode) -> None:
    print(mode.name)
    print(mode.value)
    print(str(mode))
    print(repr(mode))
    print(bool(mode))
    print(mode == "lint")
    match mode:
        case Mode.check:
            print("check")
        case Mode.empty:
            print("empty")


def lookup(value: str | Mode) -> Mode:
    return Mode(value)


display(Mode("lint"))
display(Mode(Mode.empty))
print(Mode.alias is Mode.check)
try:
    display(Mode("other"))
except ValueError as error:
    print(str(error))


print(lookup(Mode.alias) is Mode.check)
print(lookup("") is Mode.empty)
