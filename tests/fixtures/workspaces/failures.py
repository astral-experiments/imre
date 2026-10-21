from pathlib import Path


def outcome(operation: int, path: str, target: str = "unused", flag: bool = False, mode: int = 0o777) -> str:
    try:
        if operation == 0:
            Path(path).mkdir(mode=mode, parents=flag)
        elif operation == 1:
            Path(path).mkdir(mode=mode, exist_ok=flag)
        elif operation == 2:
            Path(path).unlink(missing_ok=flag)
        elif operation == 3:
            Path(path).rmdir()
        else:
            Path(path).replace(target)
    except FileNotFoundError:
        return "FileNotFoundError"
    except NotADirectoryError:
        return "NotADirectoryError"
    except IsADirectoryError:
        return "IsADirectoryError"
    except FileExistsError:
        return "FileExistsError"
    except PermissionError:
        return "PermissionError"
    except OverflowError:
        return "OverflowError"
    except ValueError:
        return "ValueError"
    except OSError:
        return "OSError"
    return "ok"


for path in ["missing/child", "plain-file/child", "existing", "plain-file", "file-link", "dangling-link", "denied/child", "null\0path"]:
    print(path)
    print(outcome(0, path))
    print(outcome(1, path, flag=True))
for path in ["missing", "missing/child", "plain-file/child", "existing", "nonempty", "denied/child", "null\0path"]:
    print(path)
    print(outcome(2, path, flag=True))
    print(outcome(3, path))
for target in ["missing/child", "plain-file/child", "existing", "nonempty", "denied/child", "null\0path"]:
    print(target)
    print(outcome(4, "source.txt", target))
print(outcome(4, "missing", "destination.txt"))
print(outcome(4, "null\0source", "destination.txt"))
print(outcome(4, "nonempty", "plain-file"))
print(outcome(3, "directory-link"))
print(outcome(3, "file-link"))
print(outcome(0, "too-large", mode=2147483648))
print(outcome(0, "too-small", mode=-2147483649))
print(outcome(0, "much-too-large", mode=1000000000000000000000000000000000))
print(outcome(1, "existing", mode=2147483648, flag=True))
print(outcome(0, "null\0first", mode=2147483648))
print(Path("sentinel.txt").read_text(encoding="utf-8"))
