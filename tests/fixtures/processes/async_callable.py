import asyncio
from collections.abc import Awaitable, Callable


async def scale(items: list[int], value: int) -> int:
    items.append(value)
    return value * 2


async def double(value: int) -> int:
    return value * 2


async def main() -> None:
    items: list[int] = []
    callback: Callable[[list[int], int], Awaitable[int]] = scale
    future = callback(items, 3)
    print(len(items))
    print(await future)
    print(items[0])
    try:
        await future
    except RuntimeError as error:
        print(str(error))
    async with asyncio.TaskGroup() as group:
        task = group.create_task(callback(items, 4))
    print(await task)
    print(await task)
    results = await asyncio.gather(*list(map(double, [1, 2, 3])))
    print(sum(results))


asyncio.run(main())
