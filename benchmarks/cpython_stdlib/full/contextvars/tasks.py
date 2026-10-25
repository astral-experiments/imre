import asyncio
from contextvars import Context, ContextVar

binding: ContextVar[int] = ContextVar("binding", default=-1)


async def observe(change: int) -> int:
    before = binding.get()
    binding.set(change)
    process = await asyncio.create_subprocess_exec("/bin/sh", "-c", "sleep 0.015")
    await process.wait()
    assert binding.get() == change
    return before * 100 + binding.get()


async def parent() -> None:
    binding.set(3)
    cold = observe(30)
    binding.set(4)
    async with asyncio.TaskGroup() as group:
        left = group.create_task(cold)
        binding.set(5)
        right = group.create_task(observe(40))
        binding.set(6)
    print(left.result())
    print(right.result())
    print(binding.get())
    print(await observe(55))
    print(binding.get())
    # A cold coroutine returned by Context.run executes in its awaiter's context.
    isolated = Context()
    isolated.run(binding.set, 73)
    cold_again = isolated.run(observe, 61)
    print(await cold_again)
    print(binding.get())
    print(await asyncio.gather(observe(1), observe(2)))
    print(binding.get())


binding.set(2)
asyncio.run(parent())
print(binding.get())
