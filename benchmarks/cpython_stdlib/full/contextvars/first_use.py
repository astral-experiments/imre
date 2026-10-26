import asyncio
from contextvars import ContextVar

binding: ContextVar[int] = ContextVar("first use", default=0)


async def worker(value: int) -> int:
    # The parent and both children were scheduled before the first context read.
    assert binding.get() == 0
    with binding.set(value):
        child = await asyncio.create_subprocess_exec("/bin/sh", "-c", "sleep .015")
        await child.wait()
        assert binding.get() == value
    return binding.get()


async def parent() -> None:
    async with asyncio.TaskGroup() as group:
        first = group.create_task(worker(1))
        second = group.create_task(worker(2))
    print(first.result())
    print(second.result())
    print(binding.get())


asyncio.run(parent())
print(binding.get())
