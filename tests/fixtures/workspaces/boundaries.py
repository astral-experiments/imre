from pathlib import Path
from tempfile import TemporaryDirectory


def create(directory: str, prefix: str, suffix: str) -> str:
    try:
        with TemporaryDirectory(dir=directory, prefix=prefix, suffix=suffix):
            return "ok"
    except ValueError:
        return "ValueError"
    except OSError as error:
        filename = repr(error.filename)
        return repr((repr(error), error.errno, error.strerror, repr(error.args),
                     filename.startswith("'" + directory + "/" + prefix),
                     filename.endswith(suffix + "'"), repr(error.filename2)))


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
