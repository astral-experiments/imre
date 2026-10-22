from pathlib import Path
from tempfile import TemporaryDirectory


def create(directory: str, prefix: str, suffix: str) -> str:
    try:
        with TemporaryDirectory(dir=directory, prefix=prefix, suffix=suffix):
            return "ok"
    except FileNotFoundError:
        return "FileNotFoundError"
    except NotADirectoryError:
        return "NotADirectoryError"
    except PermissionError:
        return "PermissionError"
    except ValueError:
        return "ValueError"
    except OSError:
        return "OSError"


print(create("missing", "tmp", ""))
print(create("plain-file", "tmp", ""))
print(create("denied", "tmp", ""))
print(create("scratch", "missing/child-", ""))
print(create("scratch", "tmp", "/missing"))
print(create("null\0directory", "tmp", ""))
print(create("scratch", "null\0prefix", ""))
print(create("scratch", "tmp", "null\0suffix"))
print(create("", "tmp", ""))
print(create("scratch/../scratch", "name-λ-", "-🙂"))
print(len(list(Path("scratch").iterdir())))
