from pathlib import Path, PosixPath, PurePosixPath, PureWindowsPath


def check(path: Path, pattern: Path, sensitive: bool | None) -> None:
    try:
        right = path.match(path_pattern=pattern, case_sensitive=sensitive)
        print(right)
    except ValueError as error:
        print(str(error))
    print(path.full_match(case_sensitive=sensitive, pattern=pattern))


class Pattern:
    def __init__(self, value: str):
        self.value = value

    def __fspath__(self) -> str:
        print(f"convert {self.value}")
        return self.value


class InheritedPattern(Pattern):
    pass


class BytesPattern:
    def __fspath__(self) -> bytes:
        return b"*.py"


class FailingPattern:
    def __fspath__(self) -> str:
        raise ValueError("pattern conversion")


def make_pattern(value: str) -> InheritedPattern:
    print("evaluate pattern")
    return InheritedPattern(value)


def make_sensitivity(fail: bool) -> bool:
    print("evaluate sensitivity")
    if fail:
        raise ValueError("sensitivity expression")
    return True


posix_target = PurePosixPath("src/app.py")
windows_target = PureWindowsPath("src/app.py")
print(posix_target.match(path_pattern=make_pattern("src/*.py"), case_sensitive=make_sensitivity(False)))
print(windows_target.full_match(case_sensitive=make_sensitivity(False), pattern=make_pattern("src/*.rst")))
print(posix_target.full_match(InheritedPattern("src/*.py")))
print(windows_target.match(InheritedPattern("SRC/*.PY")))
print(posix_target.match(PureWindowsPath("src/*")))
print(posix_target.full_match(PureWindowsPath("src/*")))
try:
    windows_target.full_match(FailingPattern(), case_sensitive=make_sensitivity(True))
except ValueError as error:
    print(str(error))
try:
    posix_target.match(BytesPattern())
except TypeError as error:
    print(str(error))
try:
    posix_target.full_match(FailingPattern())
except ValueError as error:
    print(str(error))


for text in ["", "/", "//", "a", "/a", "//a", "a/b", "/a/b", ".dot",
             "a\nb", "İ", "ı", "K", "ſ", "]", "雪"]:
    for pattern in ["", ".", "/", "//", "*", "?", "**", "***", "**/*", "a/**",
                    "**/a", "/**/a", "//**", "a/**/**/b", "a[!x]b", "a*b", "a**b", "a*b*c", "[!a]", "[z-a]",
                    "[!z-a]", "[a-c]", "[[]", "[]]", "[a-z]", "[!I]", "[ſ]"]:
        p = PosixPath(text)
        q = Path(pattern)
        check(p, q, None)
        check(p, q, False)
        check(p, q, True)


for path, pattern in [("SRC/lib/Test_main.PY", "src/*/test_*.py"),
                      ("test", "tes*est"), ("abc", "a**c"), ("a/b", "a*b")]:
    windows = PureWindowsPath(path)
    print(windows.match(pattern))
    print(windows.full_match(pattern))
    print(windows.full_match(pattern, case_sensitive=True))
