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
