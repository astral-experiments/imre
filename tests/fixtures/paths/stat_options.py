from pathlib import Path


class BadFlag:
    def __bool__(self) -> bool:
        print("convert flag")
        raise RuntimeError("flag conversion failed")


def flag() -> BadFlag:
    print("evaluate flag")
    return BadFlag()


def argument(label: str) -> int:
    print(label)
    return 1


for name in ("bad\x00name", "missing-stat-options"):
    try:
        Path(name).stat(follow_symlinks=flag())
    except ValueError as e:
        print(str(e))
    except RuntimeError as e:
        print(str(e))

try:
    Path(".").stat(argument("positional"), follow_symlinks=argument("keyword"))
except TypeError as e:
    print(str(e))
try:
    Path(".").lstat(argument("positional"), other=argument("other"))
except TypeError as e:
    print(str(e))
try:
    Path(".").stat(self=argument("duplicate self"))
except TypeError as e:
    print(str(e))
try:
    Path(".").lstat(follow_symlinks=argument("lstat keyword"))
except TypeError as e:
    print(str(e))
