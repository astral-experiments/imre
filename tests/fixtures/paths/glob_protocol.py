from pathlib import Path, PureWindowsPath


class Pattern:
    def __init__(self, value: str | bytes) -> None:
        self.value = value

    def __fspath__(self) -> str | bytes:
        print("convert pattern")
        return self.value

    def __str__(self) -> str:
        print("display pattern")
        return "it's a pattern"


class Inherited(Pattern):
    pass


class Broken:
    def __fspath__(self) -> str:
        raise AttributeError("inside __fspath__")


def option(fail: bool) -> bool:
    print("evaluate option")
    if fail:
        raise ValueError("later argument")
    return False


root = Path("glob-protocol")
(root / "NEST").mkdir(parents=True)
(root / "NEST" / "item.txt").write_bytes(b"")
(root / "file.txt").write_bytes(b"")
pattern = Inherited("*.TXT")
print("|".join(sorted(p.name for p in root.glob(pattern, case_sensitive=option(False)))))
print("|".join(sorted(p.name for p in root.rglob(pattern, case_sensitive=False))))
print("|".join(p.name for p in root.glob(Inherited("*/"))))
print(len(list(root.glob(PureWindowsPath("NEST/*.txt")))))
try:
    root.glob(pattern, recurse_symlinks=option(True))
except ValueError as error:
    print(str(error))
for value in ["", "././"]:
    try:
        root.glob(Inherited(value))
    except ValueError as error:
        print(str(error))
try:
    root.glob(Inherited(b"/absolute"))
except NotImplementedError as error:
    print(str(error))
try:
    root.glob(Inherited(b"relative"))
except TypeError as error:
    print(str(error))
try:
    root.glob(Broken())
except AttributeError as error:
    print(str(error))
try:
    root.rglob(Broken())
except TypeError as error:
    print(str(error))
    print(error.__suppress_context__)
