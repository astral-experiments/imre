import asyncio
from collections.abc import Awaitable
from typing import TypeVar

T = TypeVar("T")


async def command(value: int, running: list[int]) -> int:
    running.append(value)
    assert len(running) == 1
    child = await asyncio.create_subprocess_exec("/bin/sh", "-c", "sleep .01")
    await child.wait()
    return running.pop()


async def main() -> None:
    semaphore = asyncio.Semaphore(1)

    async def limit(coroutine: Awaitable[T]) -> T:
        async with semaphore:
            return await coroutine

    running: list[int] = []
    values = await asyncio.gather(
        *[limit(command(i, running)) for i in [1, 2, 3, 4]]
    )
    print(sum(values))

    entered: list[str] = []

    async def reacquire() -> None:
        async with semaphore:
            entered.append("first")
            child = await asyncio.create_subprocess_exec("/bin/sleep", ".05")
            await child.wait()
        async with semaphore:
            entered.append("again")

    async def waiting() -> None:
        async with semaphore:
            entered.append("waiting")

    async with asyncio.TaskGroup() as group:
        group.create_task(reacquire())
        group.create_task(waiting())
    print(repr(entered))


asyncio.run(main())
