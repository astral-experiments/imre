import errno
from pathlib import Path


def failure(path: str, strict: bool) -> str:
    try:
        Path(path).resolve(strict=strict)
    except FileNotFoundError:
        return "missing"
    except NotADirectoryError:
        return "not-directory"
    except PermissionError:
        return "permission"
    except ValueError:
        return "value"
    except OSError:
        return "os-error"
    return "resolved"


def receiver(events: list[int]) -> Path:
    events.append(1)
    return Path("file-link")


def choose_strict(events: list[int]) -> bool:
    events.append(2)
    return True


base = Path(".").resolve(strict=True)
print(repr((base.is_absolute(), Path("relative").is_absolute())))
print(repr((Path("//").resolve() == Path("/"), Path("//a").is_absolute())))
print(Path("../").resolve() == base.parent)
for raw, expected in [
    ("file-link", "input.txt"),
    ("directory-link", "tree"),
    ("directory-link/../input.txt", "input.txt"),
    ("missing/child", "missing/child"),
    ("missing/../input.txt", "input.txt"),
    ("dangling-link/child", "missing/child"),
    ("dangling-link/../input.txt", "input.txt"),
    ("file-link/child", "input.txt/child"),
    ("file-link/../input.txt", "input.txt"),
    ("loop-a/tail", "loop-a/tail"),
    ("loop-b/../input.txt", "input.txt"),
    ("repeated-link", "tree"),
    ("trailing-link", "input.txt"),
    ("chain-0", "input.txt"),
    ("absolute-link", "/tmp"),
    ("tree/λ.json", "tree/λ.json"),
]:
    path = Path(raw)
    alias = path
    actual = path.resolve()
    print(repr((raw, actual == base / expected, actual.is_absolute(), alias == Path(raw))))
print(Path("chain-0").resolve(True) == base / "input.txt")
print(Path("absolute-link").resolve(True) == Path("/tmp"))
for raw in [
    "missing", "dangling-link", "file-link/child", "file-link/../input.txt",
    "loop-a", "loop-b", "denied/secret.json", "trailing-link",
    "\0", "missing/\0", "input.txt/\0",
]:
    print(repr((repr(raw), failure(raw, False), failure(raw, True))))

for raw, component, code in [
    ("missing/child", "missing", errno.ENOENT),
    ("dangling-link/child", "missing", errno.ENOENT),
    ("file-link/child", "input.txt", errno.ENOTDIR),
    ("loop-a/child", "loop-a", errno.ELOOP),
]:
    try:
        Path(raw).resolve(strict=True)
    except OSError as error:
        filename = error.filename
        assert isinstance(filename, str)
        print(repr((error.errno == code, filename == str(base / component), error.filename2 is None)))

events: list[int] = []
print(repr((receiver(events).resolve(strict=choose_strict(events)) == base / "input.txt", events)))
