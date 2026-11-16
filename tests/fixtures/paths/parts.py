from pathlib import Path, PurePosixPath, PureWindowsPath


def components(spelling: str) -> tuple[str, ...]:
    return Path(spelling).parts


saved = components("//A//λ/./../leaf/")
for part in saved:
    print(part)
print(saved == ("//", "A", "λ", "..", "leaf"))
print(saved[-1])
print(components(".") == ())
print(PurePosixPath("///foo").parts == ("/", "foo"))
print(PureWindowsPath("C:Dir/file").parts == ("C:", "Dir", "file"))
print(PureWindowsPath("C:/Dir/file").parts == ("C:\\", "Dir", "file"))
print(PureWindowsPath("//Srv/SHARE/Dir").parts == ("\\\\Srv\\SHARE\\", "Dir"))
print(PureWindowsPath("C:/Dir/dir").parts.count("Dir"))


class Record:
    @property
    def parts(self) -> "Record":
        print("parts")
        return self

    def count(self, component: str) -> int:
        print(component)
        return 37


def owner(number: bytes, text: str) -> Record:
    print(number, text)
    return Record()


def measure(value: str | bytes) -> None:
    if isinstance(value, bytes):
        print(owner(value, (value := "changed")).parts.count(value))


measure(b"v")
