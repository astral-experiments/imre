import asyncio
from collections.abc import Awaitable, Callable


async def scale(items: list[int], value: int) -> int:
    items.append(value)
    return value * 2


async def double(value: int) -> int:
    return value * 2


async def wait_for_double(value: int) -> int:
    results = await asyncio.gather(double(value))
    return results[0]


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
    try:
        await asyncio.gather(future)
    except RuntimeError as error:
        print(str(error))
    retry = asyncio.ensure_future(future)
    print("scheduled")
    try:
        await retry
    except RuntimeError as error:
        print(str(error))
    try:
        async with asyncio.TaskGroup() as group:
            group.create_task(future)
            print("added")
    except ExceptionGroup as error:
        print(str(error.exceptions[0]))
    async with asyncio.TaskGroup() as group:
        task = group.create_task(callback(items, 4))
    print(await task)
    print(await task)
    print(repr(await asyncio.gather(task, task)))
    fresh = callback(items, 5)
    print(repr(await asyncio.gather(fresh, fresh)))
    print(repr(items))
    for coro, cancel_second in [(double(7), False), (wait_for_double(7), False), (wait_for_double(7), True)]:
        first = asyncio.ensure_future(coro)
        second = asyncio.ensure_future(coro)
        if cancel_second:
            second.cancel()
        for scheduled in [first, second]:
            try:
                print(await scheduled)
            except asyncio.CancelledError:
                print("cancelled")
            except RuntimeError as error:
                print(str(error))
    results = await asyncio.gather(*list(map(double, [1, 2, 3])))
    print(sum(results))


asyncio.run(main())

loop = asyncio.new_event_loop()
future = double(6)
print(loop.run_until_complete(future))
try:
    loop.run_until_complete(future)
except RuntimeError as error:
    print(str(error))
loop.close()
