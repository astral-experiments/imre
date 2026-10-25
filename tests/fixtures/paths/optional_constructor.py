from pathlib import Path, PurePosixPath, PureWindowsPath


def later(log: list[int], fail: bool) -> str:
    log.append(1)
    if fail:
        raise ValueError("later argument")
    return "tail"


def path(value: str | None, fail: bool) -> None:
    log: list[int] = []
    try:
        item = Path(value, later(log, fail))  # ty: ignore[invalid-argument-type]
        print(str(item))
    except ValueError as error:
        print(str(error))
    except TypeError as error:
        print(str(error))
    print(len(log))


path("head", False)
path(None, False)
path(None, True)


def pure(parts: str | None) -> None:
    try:
        print(str(PurePosixPath("one", parts)))  # ty: ignore[invalid-argument-type]
        print(str(PureWindowsPath("one", parts)))  # ty: ignore[invalid-argument-type]
    except TypeError as error:
        print(str(error))


pure("two")
pure(None)
