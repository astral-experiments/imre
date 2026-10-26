import asyncio
from dataclasses import dataclass


@dataclass
class Row:
    name: str


async def row(name: str, fail: bool = False) -> Row:
    if fail:
        raise ValueError(name)
    return Row(name)


async def maybe_row(name: str) -> Row | None:
    if name == "empty":
        return None
    if name == "error":
        raise ValueError(name)
    return Row(name)


async def stopped(error: Exception) -> Row:
    raise error


async def main() -> None:
    stop = StopIteration("inside coroutine")
    try:
        await stopped(stop)
    except RuntimeError as error:
        print(str(error))
        print(error.__cause__ is stop)
    for item in await asyncio.gather(stopped(stop), return_exceptions=True):
        if isinstance(item, BaseException):
            print(repr(item))
            print(item.__cause__ is stop)
    entries: list[BaseException | Row] = await asyncio.gather(
        *[row(name, name == "b") for name in ["a", "b", "c"]],
        return_exceptions=True,
    )
    for entry in entries:
        if isinstance(entry, BaseException):
            print(str(entry))
        else:
            print(entry.name)
    for value in await asyncio.gather(
        *[maybe_row(name) for name in ["empty", "present", "error"]], return_exceptions=True
    ):
        if value is None:
            print("empty")
        elif isinstance(value, BaseException):
            print(str(value))
        else:
            print(value.name)
    try:
        async with asyncio.TaskGroup() as group:
            group.create_task(row("first", True))
            group.create_task(row("second", True))
    except ExceptionGroup as errors:
        print(str(errors))
        print(str(errors.exceptions[0]))
        print(str(errors.exceptions[-1]))


asyncio.run(main())
