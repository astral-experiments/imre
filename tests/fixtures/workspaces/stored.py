from contextlib import nullcontext
from pathlib import Path
from tempfile import TemporaryDirectory


def count() -> int:
    return len(list(Path("scratch").iterdir()))


def choose(cache: Path | None) -> None:
    manager = TemporaryDirectory(dir="scratch") if not cache else nullcontext(cache)
    print(count())
    with manager as value:
        print(isinstance(value, str))
        print(isinstance(value, Path))
        chosen = Path(value)
        print(chosen.is_dir())
    print(chosen.exists())
    print(count())


def abandon() -> None:
    manager = TemporaryDirectory(dir="scratch")
    print(count())
    raise ValueError("before enter")


def caught() -> None:
    kept: Exception | None = None
    try:
        abandon()
    except ValueError as err:
        kept = err
    print(kept is not None)
    print(count())
    kept = None
    print(count())


def reenter() -> None:
    manager = TemporaryDirectory(dir="scratch")
    owner = manager
    manager = TemporaryDirectory(dir="scratch")
    print(count())
    first = Path("unentered")
    try:
        with owner as name:
            first = Path(name)
            raise RuntimeError("inside with")
    except RuntimeError:
        print(first.exists())
    print(count())
    first.mkdir()
    with owner as name:
        print(first == Path(name))
        print(first.is_dir())
    print(first.exists())
    print(count())


choose(None)
choose(Path("existing"))
try:
    abandon()
except ValueError:
    print(count())
caught()
reenter()
print(count())
with nullcontext("a//b") as text:
    print(text)
with nullcontext(Path("a/b")) as path:
    print(path.name)
with nullcontext() as empty:
    print(empty is None)
